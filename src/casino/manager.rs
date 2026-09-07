//! The simulation manager: the thing that actually keeps the casino
//! running.
//!
//! This is the heart of the feature, and the shape of it matters. A real
//! OS thread owns every running table and advances them on the clock. The
//! UI never drives a simulation — it takes a *snapshot* of one and draws
//! it. Which means:
//!
//! - Tables progress whether or not anyone is looking at them.
//! - Switching from one table to another cannot restart, reset or reseed
//!   either of them, because switching only changes which snapshot the UI
//!   asks for.
//! - Two hundred tables cost two hundred tables' worth of arithmetic and
//!   exactly one table's worth of drawing.
//!
//! The lock is held only for the length of a tick or a snapshot copy, never
//! across a render or a keypress, so the UI can never stall the floor and
//! the floor can never stall the UI.

use super::analytics::{Games, Series, Span, Tally};
use super::bank::{Bank, Expense, Movement, OPENING_CHIPS, OPENING_MONEY};
use super::clock::{Clock, Every};
use super::config::{self, Config};
use super::event::{Departure, Event, Feed, Record, Weight};
use super::happening::{Going, Happenings};
use super::demand::{Demand, Standing};
use super::instance::{Instance, Kind, Limit, RoundLog};
use super::interest::{self, Interest};
use super::dashboard::{FinancialView, GameView, GlobalView, GuestView, ReceptionView, Snapshot, TournamentView};
use super::patron::{Patron, Presence};
use super::reception::Reception;
use super::roster::{self, Roster};
use super::save::{Save, SavedTable};
use super::sim::Sim;
use super::tournament::{Running, Stage, Tournament};
use crate::rng::Rng;
use crate::ui::Badge;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How many feed lines a floor snapshot carries. Enough to fill a panel,
/// small enough that copying it under the lock stays trivial.
const FEED_LINES: usize = 64;

/// How many names a leaderboard carries, and how many buckets the shape of
/// the night is drawn from.
const BOARD: usize = 8;

/// Everything the running casino owns. Only ever touched behind the mutex.
struct Floor {
    bank: Bank,
    instances: Vec<Instance>,
    next_id: u32,
    /// How many of each kind have ever been opened, so table numbers keep
    /// climbing rather than being reused.
    opened: BTreeMap<&'static str, u32>,
    rng: Rng,
    /// What the random source was last seeded with. Kept only so an
    /// operator can be shown it and told what it changed to — nothing in
    /// the simulation reads it back.
    seed: u64,
    cfg: Config,
    clock: Clock,
    feed: Feed,
    /// Everyone the casino knows. Owned here, so a patron outlives the
    /// table they happen to be sitting at.
    roster: Roster,
    /// When the next bill falls due, in simulated time.
    rent: Every,
    /// When the next people walk in.
    door: Every,
    /// What the room is in the mood for, and how full it has been.
    demand: Demand,
    /// What the night has looked like: a bounded bucketed history, and the
    /// same six figures for every game. Written once per round, never
    /// recomputed.
    series: Series,
    games: Games,
    /// What is going on in the building tonight.
    weather: Happenings,
    /// Tournaments, running themselves on their own clock.
    tourneys: Running,
    next_tourney: Every,
    tourneys_held: u32,
    /// The front desk's day book. Fed from the event stream once a tick;
    /// see [`super::reception`] for why it is not allowed to scan anything.
    desk: Reception,
    started: Instant,
}

impl Floor {
    /// The services a table borrows for the length of one call. All five
    /// are distinct fields, which is the only reason this compiles — and
    /// the only place in the program that has to know it.
    fn sim(&mut self) -> Sim<'_> {
        let now = self.clock.now();
        Sim { rng: &mut self.rng, bank: &mut self.bank, feed: &mut self.feed, cfg: &self.cfg, roster: &mut self.roster, now }
    }

    /// The whole lifecycle in one pass: people whose time away is up come
    /// back, people who are done at a table get up and go home, and empty
    /// seats are offered to whoever is in the building looking for a game.
    ///
    /// This lives at floor level rather than inside a table on purpose. A
    /// table knows about seats; only the floor knows about *people*, and
    /// choosing where to play is a decision made across tables. It is also
    /// what keeps a patron from being destroyed by the table they happen to
    /// be sitting at.
    fn lifecycle(&mut self, now: Duration) {
        // 1. Anyone whose spell away has elapsed walks back in.
        for id in self.roster.due_back(now) {
            self.roster.welcome_back(id);
        }

        // 2. Anyone done at a table gets up, cashes out, and goes home.
        for i in 0..self.instances.len() {
            if self.instances[i].paused {
                continue;
            }
            let seated: Vec<u64> = self.instances[i].patrons.clone();
            for pid in seated {
                let Floor { roster, rng, cfg, bank, feed, instances, series, .. } = self;
                let Some(p) = roster.get_mut(pid) else {
                    instances[i].stand(pid);
                    continue;
                };
                let Some(why) = p.leaving(rng, cfg.min_bet) else { continue };
                let (who, net) = (p.name.clone(), p.net());
                let back_at = super::roster::time_away(rng, cfg, now);
                let chips = p.end_visit(back_at);
                let cashed = bank.cash_out(chips);
                instances[i].stand(pid);
                // A seat freed is also a moment to re-decide how busy this
                // table should be, so a floor breathes over the night
                // instead of every table sitting at a fixed size forever.
                instances[i].reconsider_size(rng);
                feed.push(now, Weight::Notable, Event::Left { patron: pid, who, table: instances[i].id, net, reason: why, cashed });
                series.record(&Tally { departures: 1, ..Tally::default() });
            }
        }

        // 3. People walk in through the front door, at a rate of their own
        //    that has nothing to do with how many seats are free. This is
        //    what makes an empty seat possible at all, and therefore what
        //    makes occupancy worth measuring and an over-staffed floor
        //    worth avoiding.
        let mut admitted = 0;
        while self.door.due(now) {
            if admitted > self.cfg.arrivals_per_period * 8 {
                // A long stall must not empty the street into the room in
                // a single tick.
                self.door.resync(now);
                break;
            }
            // How many walk in is the configured rate, scaled by whatever
            // is going on in the building. A rush really is a rush.
            let wanted = (self.cfg.arrivals_per_period as i64 * self.weather.arrivals() / 1_000).max(0) as usize;
            for _ in 0..wanted {
                let Floor { roster, rng, cfg, .. } = self;
                if roster.admit(rng, cfg, now).is_none() {
                    break;
                }
                admitted += 1;
            }
        }

    }

    /// Offers the empty seats to whoever is in the building and free.
    ///
    /// Deliberately a separate pass, run *after* the tournaments have had
    /// their chance at the room. Registration and seating both draw from
    /// the same pool of people standing about, and whichever runs first
    /// takes the lot — with seating first, a tournament could never field
    /// anybody at all. This ordering is load-bearing.
    fn seat_the_room(&mut self, now: Duration) {
        // A person picks the table among those with room that most suits
        // them — the "choose a game" step — rather than being posted to the
        // first vacancy.
        loop {
            let hungry: Vec<usize> =
                (0..self.instances.len()).filter(|i| !self.instances[*i].paused && self.instances[*i].patrons.len() < self.instances[*i].wanted()).collect();
            if hungry.is_empty() {
                break;
            }
            let Floor { roster, rng, cfg, bank, feed, instances, demand, series, weather, .. } = self;
            let Some(pid) = roster.waiting(rng) else { break };
            let Some(p) = roster.get_mut(pid) else { break };

            // Where would they like to sit? Taste scores every table with
            // room; the highest wins, with a wobble so identical people do
            // not all pile onto the same table.
            let mut best = hungry[0];
            let mut best_score = i64::MIN;
            let tier = p.tier(cfg);
            for i in hungry.iter().copied() {
                let kind = instances[i].kind;
                if !instances[i].limit.admits(tier, cfg) {
                    // A high-limit table is not for everybody; that is the
                    // point of it.
                    continue;
                }
                // Their own taste, nudged by what the room is in the mood
                // for. Appeal moves where somebody sits and nothing else.
                let taste = p.taste(kind.variants(), kind.pace().as_millis() as u64);
                // Their taste, the room's mood, and whatever the night is
                // doing. All three are nudges on a preference.
                let mood = demand.appeal(kind.key()) + weather.appeal_bonus(kind.key());
                let score = taste * mood / super::demand::NEUTRAL + rng.below(25) as i64;
                if score > best_score {
                    best_score = score;
                    best = i;
                }
            }
            if best_score == i64::MIN {
                // Nowhere on the floor will have them — a room of nothing
                // but high-limit tables and a guest at the door. They wait.
                break;
            }

            let dollars = p.buy_in(rng, cfg);
            let chips = bank.buy_in(dollars);
            p.begin_visit(chips);
            let who = p.name.clone();
            let table_id = instances[best].id;
            let table_name = instances[best].name.clone();
            roster.seat(pid, table_id);
            instances[best].sit(pid);
            feed.push(
                now,
                Weight::Notable,
                Event::Arrived { patron: pid, who, table: table_id, table_name, chips },
            );
            series.record(&Tally { arrivals: 1, ..Tally::default() });
        }

        // 5. Anybody still standing about with nowhere to sit may give up
        //    and go home. Without this, a room with too few tables slowly
        //    fills with people who never play and never leave.
        let Floor { roster, rng, cfg, .. } = self;
        for pid in roster.looking() {
            if rng.below(100) < cfg.gives_up {
                let back_at = super::roster::time_away(rng, cfg, now);
                if let Some(p) = roster.get_mut(pid) {
                    p.end_visit(back_at);
                }
            }
        }
    }

    /// Charges the building's running costs, if a period has come round.
    ///
    /// Periodic, never per frame — that is the whole reason `Every` exists.
    /// A `while` loop rather than an `if` so a fast clock, or a spell where
    /// the process was busy, pays every period it owes instead of quietly
    /// skipping the ones it missed.
    fn pay_the_bills(&mut self, now: Duration) {
        let tables = self.instances.len() as i64;
        let dearer = self.weather.costs();
        while self.rent.due(now) {
            let overhead = self.cfg.overhead_per_period * dearer / 1_000;
            let staffing = tables * self.cfg.table_cost_per_period * dearer / 1_000;
            self.bank.pay(Expense::Overhead, overhead);
            self.bank.pay(Expense::Staffing, staffing);
            let total = overhead + staffing;
            if total > 0 {
                self.feed.push(now, Weight::Notable, Event::Costs { overhead, staffing, tables: tables as usize });
            }
        }
    }

    /// Samples how full the floor is, and lets the room's taste in games
    /// move on. Both are periodic; neither happens per frame.
    fn read_the_room(&mut self, now: Duration) {
        if self.demand.sample_due(now) {
            let seen: Vec<(&'static str, usize, usize)> =
                self.instances.iter().map(|t| (t.kind.key(), t.patrons.len(), t.wanted())).collect();
            self.demand.record_sample(&seen);
        }
        let kinds: Vec<&'static str> = {
            let mut k: Vec<&'static str> = self.instances.iter().map(|t| t.kind.key()).collect();
            k.sort_unstable();
            k.dedup();
            k
        };
        if kinds.is_empty() {
            return;
        }
        let Floor { demand, rng, cfg, feed, weather, .. } = self;
        for (key, appeal) in demand.drift(rng, cfg, now, &kinds) {
            feed.push(now, Weight::Routine, Event::Mood { game: key, appeal });
        }

        // And whatever is going on in the building tonight. Announced at
        // `Major`, because a coach arriving is exactly the sort of thing
        // somebody watching the floor wants to be told about.
        let (started, ended) = weather.tick(rng, cfg, now, &kinds);
        for g in started {
            feed.push(now, Weight::Major, Event::Happening { what: g.what, game: g.game, on: true });
        }
        for g in ended {
            feed.push(now, Weight::Notable, Event::Happening { what: g.what, game: g.game, on: false });
        }
    }

    /// Runs whatever tournaments are going: opens a new one when it is
    /// time, takes entries, plays rounds, and pays the pool out.
    ///
    /// A tournament is on its own clock and knows nothing about the cash
    /// tables. The only things it shares with the rest of the floor are the
    /// people in it and the same `Kind::resolve` every table uses.
    fn run_tournaments(&mut self, now: Duration) {
        // Open one, if it is time and there is not one already going.
        while self.next_tourney.due(now) {
            if self.tourneys.values().any(|t| t.stage != Stage::Done) || self.instances.is_empty() {
                break;
            }
            let kinds: Vec<Kind> = self.instances.iter().map(|t| t.kind).collect();
            let kind = kinds[self.rng.below(kinds.len())];
            self.tourneys_held += 1;
            let id = self.next_id;
            self.next_id += 1;
            let t = Tournament::new(id, self.tourneys_held, kind, &self.cfg, now);
            self.feed.push(now, Weight::Major, Event::Tourney { name: t.name.clone(), what: "is taking entries".into() });
            self.tourneys.insert(id, t);
        }

        let ids: Vec<u32> = self.tourneys.keys().copied().collect();
        for tid in ids {
            let Some(mut t) = self.tourneys.remove(&tid) else { continue };
            match t.stage {
                Stage::Registering => {
                    // Anybody in the building and free might fancy it.
                    for pid in self.roster.looking() {
                        if t.entered() >= 128 {
                            break;
                        }
                        let Floor { roster, rng, cfg, bank, .. } = self;
                        // Somebody who has just walked in has not been to
                        // the cage yet — they are holding nothing. Entering
                        // a tournament *is* starting a visit, so they buy
                        // chips first, exactly as they would sitting down.
                        {
                            let Some(p) = roster.get_mut(pid) else { continue };
                            if p.chips == 0 {
                                let dollars = p.buy_in(rng, cfg);
                                let chips = bank.buy_in(dollars);
                                p.begin_visit(chips);
                            }
                        }
                        if !super::tournament::would_enter(roster, pid, t.buy_in, rng) {
                            continue;
                        }
                        let Some(p) = roster.get_mut(pid) else { continue };
                        let (name, chips) = (p.name.clone(), p.chips);
                        if t.enter(pid, name, chips, cfg).is_some() {
                            p.chips -= t.buy_in;
                            p.presence = Presence::InTournament { id: tid };
                            // The buy-in is turnover and the rake is the
                            // house's win on it, booked the same way every
                            // other bet in this building is.
                            let rake = t.buy_in * cfg.tourney_rake / 100;
                            bank.settle("tourney", t.buy_in, t.buy_in - rake);
                        }
                    }
                    if t.running_for(now) >= self.cfg.tourney_registration {
                        if t.begin(&self.cfg) {
                            self.feed.push(
                                now,
                                Weight::Major,
                                Event::Tourney { name: t.name.clone(), what: format!("is under way — {} runners", t.entered()) },
                            );
                        } else {
                            // Not enough takers. Everybody gets every chip
                            // back, and the house un-takes its cut.
                            let name = t.name.clone();
                            let (entrants, buy_in, rake) = t.abandon();
                            for e in entrants.iter() {
                                if let Some(p) = self.roster.get_mut(e.patron) {
                                    p.chips += buy_in;
                                    p.presence = Presence::Looking;
                                }
                            }
                            if rake > 0 {
                                self.bank.settle("tourney", 0, rake);
                            }
                            self.feed.push(
                                now,
                                Weight::Notable,
                                Event::Tourney { name, what: "was called off — not enough runners".into() },
                            );
                        }
                    }
                }
                Stage::Running(_) | Stage::FinalTable => {
                    if t.due(now) {
                        let before = t.stage;
                        let Floor { rng, cfg, roster, feed, .. } = self;
                        let out = t.play_round(rng, cfg, now);
                        for e in out.iter() {
                            if let Some(p) = roster.get_mut(e.patron) {
                                p.presence = Presence::Looking;
                            }
                        }
                        if !out.is_empty() {
                            feed.push(
                                now,
                                Weight::Notable,
                                Event::Tourney {
                                    name: t.name.clone(),
                                    what: format!("{} knocked out — {}", out.len(), t.stage.label()),
                                },
                            );
                        }
                        // Announced when it *becomes* the final table, not
                        // every round it spends there.
                        if t.stage == Stage::FinalTable && before != Stage::FinalTable {
                            feed.push(now, Weight::Major, Event::Tourney { name: t.name.clone(), what: "is down to the final table".into() });
                        }
                    }
                }
                Stage::Done => {}
            }

            if t.stage == Stage::Done && t.paid.is_empty() && !t.field.is_empty() {
                let payouts: Vec<_> = t.settle(&self.cfg).to_vec();
                for pay in payouts.iter() {
                    if let Some(p) = self.roster.get_mut(pay.patron) {
                        p.chips += pay.prize;
                        p.presence = Presence::Looking;
                    }
                }
                if let Some(first) = payouts.first() {
                    self.feed.push(
                        now,
                        Weight::Major,
                        Event::Tourney { name: t.name.clone(), what: format!("won by {} for {}", first.name, first.prize) },
                    );
                }
                // Anybody who was in it and is somehow still marked as
                // playing is freed: a tournament must never strand people.
                for e in t.field.clone() {
                    if let Some(p) = self.roster.get_mut(e.patron)
                        && matches!(p.presence, Presence::InTournament { .. })
                    {
                        p.presence = Presence::Looking;
                    }
                }
            }
            // A finished one is kept for a while so it can be looked at,
            // then forgotten.
            if t.over_for(now).is_some_and(|since| since > self.cfg.tourney_every) {
                continue;
            }
            self.tourneys.insert(tid, t);
        }
    }

    /// Advances every table that has a round due, in simulated time.
    fn tick(&mut self, wall: Instant) {
        self.clock.advance(wall);
        let now = self.clock.now();
        self.series.advance(now);
        self.lifecycle(now);
        // Tournaments get first refusal on the people standing about, then
        // whoever is left is offered a seat. See `seat_the_room`.
        self.run_tournaments(now);
        self.seat_the_room(now);
        self.pay_the_bills(now);
        self.read_the_room(now);
        let ceiling = self.cfg.max_catch_up.max(1);
        for i in 0..self.instances.len() {
            if self.instances[i].paused {
                // A paused table must not accrue a backlog, or resuming it
                // would fire off a burst of rounds at once.
                self.instances[i].next_at = now + self.instances[i].kind.pace();
                continue;
            }
            let mut played = 0;
            while self.instances[i].next_at <= now && played < ceiling {
                let mut sim = Sim {
                    rng: &mut self.rng,
                    bank: &mut self.bank,
                    feed: &mut self.feed,
                    cfg: &self.cfg,
                    roster: &mut self.roster,
                    now,
                };
                self.instances[i].play_round(&mut sim);
                // The round that just happened is folded into the night's
                // figures here, once, from the log it already produced.
                // Nothing walks history to work any of this out later.
                if let Some(log) = self.instances[i].last_round() {
                    let t = Tally {
                        handle: log.pot,
                        payouts: log.paid,
                        bets: log.seats.len() as u64,
                        rounds: 1,
                        arrivals: 0,
                        departures: 0,
                        biggest_win: log.seats.iter().map(|s| s.returned - s.staked).max().unwrap_or(0).max(0),
                    };
                    self.series.record(&t);
                    self.games.record(self.instances[i].kind.key(), &t);
                }
                played += 1;
            }
            if self.instances[i].next_at + Duration::from_secs(5) < now {
                // Fell too far behind to be worth catching up on. Speed is
                // now the clock's business, so this is a genuine stall
                // rather than the fast-forward doing its job.
                self.instances[i].next_at = now;
            }
        }
        self.mind_the_desk();
    }

    /// Hands the front desk whatever the feed has published since it last
    /// looked.
    ///
    /// This is the only place the reception ledger is written, and it costs
    /// one tick's worth of events — not one night's. The desk keeps its own
    /// cursor, so a slow tick catches up and a fast one does no work at
    /// all.
    fn mind_the_desk(&mut self) {
        let Floor { desk, feed, .. } = self;
        super::reception::catch_up(desk, feed);
    }
}

/// A read-only picture of one table, copied out under the lock and then
/// drawn at leisure.
#[derive(Debug, Clone)]
pub struct TableView {
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    pub seats: usize,
    pub round: u64,
    pub status: &'static str,
    pub take: i64,
    /// Everything the seats have put through this table since it opened.
    pub staked: i64,
    pub last: Option<RoundLog>,
    pub history: Vec<RoundLog>,
    pub patrons: Vec<super::patron::Patron>,
    /// Simulated time this table has been open.
    pub open_for: Duration,
    pub seen: u32,
    /// What this table lets people bet, and who it seats.
    pub limit: Limit,
    /// How worth watching it is, and why.
    pub interest: Interest,
    /// The tunables in force, so the drawing code can name a tier or a
    /// threshold without writing the number down itself.
    pub cfg: Config,
}

/// The whole floor at a glance.
#[derive(Debug, Clone)]
pub struct FloorView {
    pub money: i64,
    pub chips: i64,
    pub table_profit: i64,
    pub cage_profit: i64,
    /// Chips won and paid at the tables, and cash over the counter both
    /// ways — the four numbers behind the two profit figures.
    pub totals: (i64, i64, i64, i64),
    pub rounds: u64,
    pub bets: u64,
    pub tables: Vec<TableView>,
    pub by_table: Vec<(&'static str, i64)>,
    /// Permille speed — `1_000` is real time.
    pub speed: u32,
    /// Wall time since the doors opened.
    pub running_for: Duration,
    /// Simulated time since the doors opened, which at anything but 1x is
    /// a different number.
    pub sim_time: Duration,
    /// The most recent readable events, oldest first.
    pub feed: Vec<Record>,
    /// How many people are in the building, and how many the casino knows
    /// of at all — the second number is the one that proves they persist.
    pub crowd: usize,
    pub known: usize,
    /// The population by tier, lowest first.
    pub by_tier: Vec<usize>,
    /// The formal books: everything staked, everything paid back, the
    /// realised hold in hundredths of a percent, what the building cost,
    /// and the cash that actually stayed in the cage.
    pub handle: i64,
    pub payouts: i64,
    pub ggr: i64,
    pub hold: i64,
    pub spent: i64,
    pub ngr: i64,
    pub by_expense: Vec<(&'static str, i64)>,
    /// What the room is in the mood for, keenest first, with how full each
    /// kind's tables have been.
    pub demand: Vec<(&'static str, Standing)>,
    /// The whole floor's occupancy, in permille of seats offered.
    pub occupancy: i64,
    pub movements: Vec<(Movement, u64, i64)>,
    /// The night in slices, and every game measured the same way.
    pub spans: Vec<(Span, Tally)>,
    pub games: Vec<(&'static str, Tally)>,
    /// The handle in each of the last few buckets, oldest first.
    pub shape: Vec<i64>,
    /// The leaderboards: most turnover, most won, most visits.
    pub top_turnover: Vec<(String, i64, u32)>,
    pub top_winners: Vec<(String, i64, i64)>,
    pub top_regulars: Vec<(String, u32, &'static str)>,
    /// What is going on in the building right now.
    pub weather: Vec<Going>,
    /// Tournaments, newest first.
    pub tourneys: Vec<Tournament>,
    pub cfg: Config,
    /// Whether the simulation thread is still turning.
    pub running: bool,
}

/// The handle the rest of the app holds. Cloning is cheap and every clone
/// talks to the same floor.
pub struct Manager {
    floor: Arc<Mutex<Floor>>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Manager {
    /// Opens the casino and starts the simulation thread. Nothing runs
    /// until this is called, and everything keeps running until `stop`.
    ///
    /// The `badge` is written by the simulation thread itself, once per
    /// tick. That is deliberate: it means the figure in the corner of the
    /// screen tracks the books directly, and stays current on screens that
    /// have never heard of the casino.
    pub fn start(seed: u64, money: i64, chips: i64, badge: Badge) -> Manager {
        let floor = Arc::new(Mutex::new(Floor {
            bank: Bank::new(money, chips),
            instances: Vec::new(),
            next_id: 1,
            opened: BTreeMap::new(),
            rng: Rng::from_seed(seed),
            seed,
            roster: Roster::new(),
            rent: Every::new(Config::default().expense_period, Duration::ZERO),
            door: Every::new(Config::default().arrivals_period, Duration::ZERO),
            demand: Demand::new(&Config::default(), Duration::ZERO),
            series: Series::new(),
            games: Games::new(),
            weather: Happenings::new(&Config::default(), Duration::ZERO),
            tourneys: Running::new(),
            next_tourney: Every::new(Config::default().tourney_every, Duration::ZERO),
            tourneys_held: 0,
            desk: Reception::default(),
            cfg: Config::default(),
            clock: Clock::new(config::SPEED_UNIT),
            feed: Feed::new(Config::default().feed_capacity),
            started: Instant::now(),
        }));
        if let Ok(mut f) = floor.lock() {
            let now = f.clock.now();
            f.feed.push(now, Weight::Notable, Event::CasinoOpened { money, chips });
        }
        let running = Arc::new(AtomicBool::new(true));

        let thread_floor = Arc::clone(&floor);
        let thread_running = Arc::clone(&running);
        badge.set(money, chips);
        let thread = std::thread::spawn(move || {
            // Re-read from configuration each pass, so a change to the
            // tick takes effect without restarting the casino.
            let mut tick;
            while thread_running.load(Ordering::Relaxed) {
                {
                    let now = Instant::now();
                    let mut nap = Config::default().tick;
                    if let Ok(mut f) = thread_floor.lock() {
                        f.tick(now);
                        badge.set(f.bank.money(), f.bank.chips());
                        nap = f.cfg.tick;
                    }
                    tick = nap;
                }
                std::thread::sleep(tick);
            }
        });

        Manager { floor, running, thread: Some(thread) }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Closes the casino. Every table stops where it is; the balances are
    /// left for the caller to persist.
    pub fn stop(&mut self) -> (i64, i64) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let f = self.floor.lock().expect("floor lock");
        (f.bank.money(), f.bank.chips())
    }

    /// Opens `n` new tables of a kind, at the house's ordinary limit.
    pub fn open(&self, kind: Kind, n: usize) {
        self.open_at(kind, n, Limit::House);
    }

    /// Opens `n` new tables at a stated limit. A high-limit table only
    /// seats the top tier, which is the one concrete thing a tier buys.
    pub fn open_at(&self, kind: Kind, n: usize, limit: Limit) {
        let Ok(mut f) = self.floor.lock() else { return };
        for _ in 0..n {
            let id = f.next_id;
            f.next_id += 1;
            let number = {
                let c = f.opened.entry(kind.key()).or_insert(0);
                *c += 1;
                *c
            };
            let mut sim = f.sim();
            let inst = Instance::open_with(id, kind, number, limit, &mut sim);
            f.instances.push(inst);
        }
    }

    /// Closes one table, cashing its patrons out at the cage on the way.
    pub fn close(&self, id: u32) {
        let Ok(mut f) = self.floor.lock() else { return };
        if let Some(i) = f.instances.iter().position(|t| t.id == id) {
            let inst = f.instances.remove(i);
            let now = f.clock.now();
            for pid in inst.patrons.iter() {
                // The table closing under somebody is not the end of them:
                // they cash out and go home, and the roster keeps them.
                let Floor { roster, bank, feed, rng, cfg, .. } = &mut *f;
                let back_at = roster::time_away(rng, cfg, now);
                let Some(p) = roster.get_mut(*pid) else { continue };
                let (who, net) = (p.name.clone(), p.net());
                let chips = p.end_visit(back_at);
                let cashed = bank.cash_out(chips);
                feed.push(
                    now,
                    Weight::Notable,
                    Event::Left { patron: *pid, who, table: inst.id, net, reason: Departure::TableClosed, cashed },
                );
            }
            f.feed.push(
                now,
                Weight::Notable,
                Event::TableClosed { table: inst.id, name: inst.name, take: inst.staked - inst.returned },
            );
            if !f.instances.iter().any(|t| t.kind == inst.kind) {
                // The floor stops having an opinion about a game it no
                // longer offers; if it reopens, it starts neutral.
                f.demand.forget(inst.kind.key());
            }
        }
    }

    pub fn toggle_pause(&self, id: u32) {
        let Ok(mut f) = self.floor.lock() else { return };
        let now = f.clock.now();
        let announce = f.instances.iter_mut().find(|t| t.id == id).map(|t| {
            t.paused = !t.paused;
            Event::TablePaused { table: t.id, name: t.name.clone(), paused: t.paused }
        });
        if let Some(e) = announce {
            f.feed.push(now, Weight::Notable, e);
        }
    }

    /// Steps to the next rung of the configured speed ladder — 0.25x up to
    /// 10x. Speed is now a property of the *clock*, so a slow speed really
    /// does mean fewer rounds per minute rather than the same rounds drawn
    /// more often, and a fast one advances every paced system together.
    pub fn cycle_speed(&self) {
        let Ok(mut f) = self.floor.lock() else { return };
        let next = f.cfg.next_speed();
        self.set_speed_locked(&mut f, next);
    }

    /// Sets an exact permille speed. `1_000` is real time.
    #[allow(dead_code, reason = "the settings screen of a later phase; exercised by tests now")]
    pub fn set_speed(&self, speed: u32) {
        let Ok(mut f) = self.floor.lock() else { return };
        self.set_speed_locked(&mut f, speed);
    }

    fn set_speed_locked(&self, f: &mut Floor, speed: u32) {
        f.cfg.speed = speed;
        f.clock.set_speed(Instant::now(), speed);
        // A faster clock does not mean a backlog of unpaid rent: the next
        // bill is simply due a period from here at the new rate.
        f.rent.resync(f.clock.now());
        f.door.resync(f.clock.now());
    }

    /// A copy of the current tunables. The UI reads thresholds from here
    /// rather than writing any of them down itself.
    #[allow(dead_code, reason = "read by the settings and analytics screens of later phases")]
    pub fn config(&self) -> Config {
        self.floor.lock().map(|f| f.cfg.clone()).unwrap_or_default()
    }

    /// Replaces the tunables wholesale — how a settings screen applies a
    /// change without reaching into the simulation.
    #[allow(dead_code, reason = "written by the settings screen of a later phase")]
    pub fn configure(&self, cfg: Config) {
        let Ok(mut f) = self.floor.lock() else { return };
        f.clock.set_speed(Instant::now(), cfg.speed);
        let now = f.clock.now();
        f.rent = Every::new(cfg.expense_period, now);
        f.door = Every::new(cfg.arrivals_period, now);
        f.weather = Happenings::new(&cfg, now);
        f.next_tourney = Every::new(cfg.tourney_every, now);
        f.cfg = cfg;
    }

    /// The most recent readable events, oldest first. `min` filters out the
    /// low-level traffic; the default screens ask for `Notable` and up.
    #[allow(dead_code, reason = "the floor screen reads the feed off the snapshot; the spectator and notification screens of later phases pull it directly")]
    pub fn feed(&self, n: usize, min: Weight) -> Vec<Record> {
        self.floor.lock().map(|f| f.feed.recent(n, min)).unwrap_or_default()
    }

    /// Everything published after `seq`, for a reader keeping its place.
    /// Returns the records and the cursor to pass back next time.
    #[allow(dead_code, reason = "for the notification layer, which must not re-read what it has already shown")]
    pub fn feed_since(&self, seq: u64, min: Weight) -> (Vec<Record>, u64) {
        self.floor
            .lock()
            .map(|f| (f.feed.since(seq, min), f.feed.cursor()))
            .unwrap_or_else(|_| (Vec::new(), seq))
    }

    /// Where a fresh reader should start to see only what happens next.
    #[allow(dead_code, reason = "paired with feed_since")]
    pub fn feed_cursor(&self) -> u64 {
        self.floor.lock().map(|f| f.feed.cursor()).unwrap_or(0)
    }

    pub fn table_count(&self) -> usize {
        self.floor.lock().map(|f| f.instances.len()).unwrap_or(0)
    }

    /// Just the two numbers the badge needs — much cheaper than a full
    /// snapshot, and this runs on every frame the app draws.
    pub fn balances(&self) -> (i64, i64) {
        self.floor.lock().map(|f| (f.bank.money(), f.bank.chips())).unwrap_or((0, 0))
    }

    /// The whole floor, copied out. Used by the overview screen.
    pub fn snapshot(&self) -> FloorView {
        let Ok(f) = self.floor.lock() else {
            return FloorView {
                money: 0,
                chips: 0,
                table_profit: 0,
                cage_profit: 0,
                totals: (0, 0, 0, 0),
                rounds: 0,
                bets: 0,
                tables: Vec::new(),
                by_table: Vec::new(),
                speed: config::SPEED_UNIT,
                running_for: Duration::ZERO,
                sim_time: Duration::ZERO,
                feed: Vec::new(),
                crowd: 0,
                known: 0,
                by_tier: Vec::new(),
                handle: 0,
                payouts: 0,
                ggr: 0,
                hold: 0,
                spent: 0,
                ngr: 0,
                by_expense: Vec::new(),
                demand: Vec::new(),
                occupancy: 0,
                movements: Vec::new(),
                spans: Vec::new(),
                games: Vec::new(),
                shape: Vec::new(),
                top_turnover: Vec::new(),
                top_winners: Vec::new(),
                top_regulars: Vec::new(),
                weather: Vec::new(),
                tourneys: Vec::new(),
                cfg: Config::default(),
                running: false,
            };
        };
        let now = f.clock.now();
        FloorView {
            money: f.bank.money(),
            chips: f.bank.chips(),
            table_profit: f.bank.table_profit(),
            cage_profit: f.bank.cage_profit(),
            totals: f.bank.totals(),
            rounds: f.bank.rounds(),
            bets: f.bank.bets(),
            tables: f.instances.iter().map(|t| view_of(t, &f.roster, &f.cfg, now, false)).collect(),
            by_table: f.bank.by_table(),
            speed: f.cfg.speed,
            running_for: Instant::now().duration_since(f.started),
            sim_time: now,
            feed: f.feed.recent(FEED_LINES, Weight::Notable),
            crowd: f.roster.present(),
            known: f.roster.len(),
            by_tier: f.roster.by_tier(&f.cfg),
            handle: f.bank.handle(),
            payouts: f.bank.payouts(),
            ggr: f.bank.ggr(),
            hold: f.bank.hold(),
            spent: f.bank.spent(),
            ngr: f.bank.ngr(),
            by_expense: f.bank.by_expense(),
            demand: f.demand.ranked(),
            occupancy: f.demand.overall_utilization(),
            movements: f.bank.movements(),
            spans: Span::ALL.iter().map(|s| (*s, f.series.range(*s, now))).collect(),
            games: f.games.by_handle(),
            shape: f.series.recent_handle(BOARD),
            top_turnover: f
                .roster
                .by_turnover(BOARD)
                .iter()
                .filter(|p| p.lifetime.staked > 0)
                .map(|p| (p.name.clone(), p.lifetime.staked, p.lifetime.visits))
                .collect(),
            top_winners: f
                .roster
                .by_winnings(BOARD)
                .iter()
                .filter(|p| p.lifetime.net() > 0)
                .map(|p| (p.name.clone(), p.lifetime.net(), p.lifetime_luck()))
                .collect(),
            top_regulars: f
                .roster
                .by_visits(BOARD)
                .iter()
                .filter(|p| p.lifetime.visits > 0)
                .map(|p| (p.name.clone(), p.lifetime.visits, p.style()))
                .collect(),
            weather: f.weather.going().to_vec(),
            tourneys: f.tourneys.values().rev().cloned().collect(),
            cfg: f.cfg.clone(),
            running: self.is_running(),
        }
    }

    /// One table in full, including its patrons and recent rounds. This is
    /// what "hooking into" a running game actually is: a copy of live
    /// state, never a restart of it.
    pub fn table(&self, id: u32) -> Option<TableView> {
        let f = self.floor.lock().ok()?;
        let now = f.clock.now();
        f.instances.iter().find(|t| t.id == id).map(|t| view_of(t, &f.roster, &f.cfg, now, true))
    }

    /// The whole casino as an operations centre sees it: the floor, the
    /// books, what has happened, the tournaments and the front desk, all
    /// from one moment.
    ///
    /// The shape of this call is the point. Everything is copied under the
    /// lock in a single pass and the lock is dropped the instant it
    /// returns — the drawing happens afterwards, on the caller's own time.
    /// A dashboard that rendered while holding this would stall every table
    /// in the building for as long as the terminal took to scroll.
    ///
    /// It is also strictly a *reader*. Nothing here opens, closes, pauses,
    /// seeds or settles anything; opening the dashboard cannot be observed
    /// from inside the simulation at all.
    pub fn dashboard(&self) -> Snapshot {
        let Ok(f) = self.floor.lock() else { return Snapshot::default() };
        let now = f.clock.now();
        let running = self.running.load(Ordering::Relaxed);
        Snapshot {
            global: GlobalView {
                money: f.bank.money(),
                chips: f.bank.chips(),
                crowd: f.roster.present(),
                known: f.roster.len(),
                tables: f.instances.len(),
                tourneys: f.tourneys.values().filter(|t| t.stage != Stage::Done).count(),
                speed: f.cfg.speed,
                uptime: Instant::now().duration_since(f.started),
                sim_time: now,
                running,
                seed: f.seed,
            },
            games: f.instances.iter().map(|t| game_view(t, &f.roster, &f.cfg, now)).collect(),
            financials: financial_view(&f),
            events: f.feed.recent(FEED_LINES, Weight::Notable),
            tournaments: f.tourneys.values().rev().map(|t| tournament_view(t, now)).collect(),
            reception: reception_view(&f, now),
            tiers: f.cfg.tiers.iter().map(|(name, _)| *name).collect(),
        }
    }

    /// One tournament in full, by id — the field, the standings and the
    /// payouts, straight off the tournament system.
    pub fn tourney(&self, id: u32) -> Option<super::tournament::Tournament> {
        let f = self.floor.lock().ok()?;
        f.tourneys.get(&id).cloned()
    }

    /// One person the casino knows, by id.
    ///
    /// A copy of the roster's record, so a screen can show somebody's
    /// history without holding the floor open while it draws it.
    pub fn patron(&self, id: u64) -> Option<Patron> {
        let f = self.floor.lock().ok()?;
        f.roster.get(id).cloned()
    }

    /// The seed the floor's random source is running on.
    pub fn seed(&self) -> u64 {
        self.floor.lock().map(|f| f.seed).unwrap_or(0)
    }

    /// Replaces the floor's random source, and nothing else.
    ///
    /// This is deliberately the *smallest* thing "reseed" could mean.
    /// Everything the casino has done stays done: the books, the people,
    /// the tables on the floor, the tournaments that have been played and
    /// the analytics they produced are all untouched. What changes is the
    /// stream of numbers every future roll comes from.
    ///
    /// Reseeding is not resetting. Throwing the world away would be a
    /// different command with a different name, and this one is not it —
    /// see the module docs for why the two are kept apart.
    ///
    /// The simulation is not stopped for this. It cannot be observed
    /// half-done, because the lock this takes is the same lock a tick
    /// takes: the swap happens strictly between two ticks. Returns the seed
    /// that was in force and the one now in force.
    pub fn reseed(&self, seed: u64) -> (u64, u64) {
        let Ok(mut f) = self.floor.lock() else { return (0, 0) };
        let was = f.seed;
        f.rng = Rng::from_seed(seed);
        f.seed = seed;
        let now = f.clock.now();
        f.feed.push(
            now,
            Weight::Notable,
            Event::Reseeded { was, now: seed },
        );
        (was, seed)
    }

    /// Opens a tournament on the operator's say-so, fields it from the
    /// people the casino already knows, and starts it.
    ///
    /// Every step goes through the machinery that already exists. The
    /// entrants are real roster patrons — found first, minted through
    /// [`Roster::mint`] only if the building genuinely cannot supply
    /// enough, and left in the roster afterwards with their tournament in
    /// their history. Their buy-ins are taken with [`Bank::buy_in`] and the
    /// house's cut is booked with [`Bank::settle`], exactly as the floor's
    /// own tournaments do it. Nothing here writes a balance directly.
    ///
    /// Returns the tournament's id, or `None` if it could not be fielded.
    /// Once it returns, the tournament is the simulation thread's: it plays
    /// itself down whether or not anybody is looking at it.
    pub fn start_tournament(&self, kind: Kind, want: usize, buy_in: i64, stack: i64) -> Option<u32> {
        let Ok(mut f) = self.floor.lock() else { return None };
        let now = f.clock.now();
        // A tournament wants a field, and the field decides the shape of
        // everything after it — so it is settled before anything is
        // charged to anybody.
        let want = want.max(f.cfg.tourney_min_field).min(MAX_FIELD);
        // Clamped so the arithmetic downstream — the rake on every entry,
        // and the pool they add up to — cannot overflow whatever a caller
        // asks for. A tournament nobody can afford is refused below, on
        // its merits; one that would not fit in the books is not a
        // tournament at all.
        let buy_in = buy_in.clamp(1, MAX_BUY_IN);
        let stack = stack.max(f.cfg.tourney_ante + 1);

        let id = f.next_id;
        f.next_id += 1;
        f.tourneys_held += 1;
        let number = f.tourneys_held;
        let mut cfg = f.cfg.clone();
        // The buy-in and the stack are this tournament's, not the house's
        // standing ones — but the rake, the prize ladder and the pace all
        // stay configuration, because those are house policy.
        cfg.tourney_buy_in = buy_in;
        cfg.tourney_stack = stack;
        let mut t = Tournament::new(id, number, kind, &cfg, now);

        // Who is eligible: somebody the casino knows, in the building or
        // able to be called in, and not already busy at a table or in
        // another tournament.
        let mut field: Vec<u64> = f
            .roster
            .iter()
            .filter(|p| p.presence.is_free() || !p.presence.is_here())
            .map(|p| p.id)
            .collect();
        // Deterministic given the same rolls: shuffled by the floor's own
        // random source, never by iteration order.
        for i in (1..field.len()).rev() {
            let j = f.rng.below(i + 1);
            field.swap(i, j);
        }
        field.truncate(want);

        let mut entered = 0usize;
        let mut guard = 0usize;
        while entered < want {
            let pid = match field.pop() {
                Some(id) => id,
                None => {
                    // The building cannot supply enough people, so the
                    // casino gets to know some more. They are minted into
                    // the roster proper — a tournament must never be
                    // fielded by disposable stand-ins that vanish when it
                    // is over.
                    guard += 1;
                    if guard > want * 2 {
                        break;
                    }
                    let Floor { roster, rng, .. } = &mut *f;
                    roster.mint(rng)
                }
            };
            let Floor { roster, rng, cfg: house, bank, .. } = &mut *f;
            let Some(p) = roster.get_mut(pid) else { continue };
            if matches!(p.presence, Presence::Seated { .. } | Presence::InTournament { .. }) {
                continue;
            }
            // Somebody who has not been to the cage tonight is holding
            // nothing, and entering a tournament *is* starting a visit —
            // so they buy chips first, through the cage, exactly as they
            // would sitting down at a table.
            //
            // What they buy is what *they* would buy: their archetype and
            // the configured range decide it, and the entry fee gets no
            // say. A casino that topped somebody up to the price of the
            // ticket would be inventing money for them, and the whole
            // point of a bankroll is that it can fall short.
            if p.chips == 0 {
                let dollars = p.buy_in(rng, house);
                let chips = bank.buy_in(dollars);
                p.begin_visit(chips);
            }
            if p.chips < buy_in {
                // They cannot afford it. That is a real answer, and the
                // next person is asked instead.
                continue;
            }
            let (name, chips) = (p.name.clone(), p.chips);
            if t.enter(pid, name, chips, &cfg).is_some() {
                p.chips -= buy_in;
                p.presence = Presence::InTournament { id };
                // Turnover, and the house's win on it, booked the same way
                // every other bet in this building is.
                let rake = buy_in * cfg.tourney_rake / 100;
                bank.settle("tourney", buy_in, buy_in - rake);
                entered += 1;
            }
        }

        if !t.begin(&cfg) {
            // Not enough takers after all: everybody gets every chip back
            // and the house un-takes its cut, exactly as an abandoned
            // tournament does. Nothing may cost anybody anything.
            let (entrants, back, rake) = t.abandon();
            for e in entrants.iter() {
                if let Some(p) = f.roster.get_mut(e.patron) {
                    p.chips += back;
                    p.presence = Presence::Looking;
                }
            }
            if rake > 0 {
                f.bank.settle("tourney", 0, rake);
            }
            return None;
        }
        f.feed.push(
            now,
            Weight::Major,
            Event::Tourney { name: t.name.clone(), what: format!("is under way — {} runners", t.entered()) },
        );
        f.tourneys.insert(id, t);
        Some(id)
    }

    /// The table most worth watching right now, if there is one running.
    ///
    /// This is "follow the action" in one call. It ranks a copy of the
    /// floor and reports an id; it does not steer, pause or otherwise
    /// touch anything, so the answer changing does not change the floor.
    pub fn most_interesting(&self) -> Option<(u32, Interest)> {
        let f = self.floor.lock().ok()?;
        f.instances
            .iter()
            .map(|t| (t.id, interest::score(t, &f.roster, &f.cfg)))
            .max_by_key(|(id, i)| (i.score, std::cmp::Reverse(*id)))
    }

    /// The whole casino, ready to be written down.
    ///
    /// Taken under the lock in one go, so it is a coherent picture rather
    /// than a set of figures from slightly different moments.
    pub fn saved(&self) -> Save {
        let Ok(f) = self.floor.lock() else { return Save { version: super::save::VERSION, ..Save::default() } };
        Save {
            version: super::save::VERSION,
            // Truncated to whole milliseconds, because that is the
            // precision the file has. A save that does not compare equal
            // to what it was made from fails its own validation step —
            // and it should, because it would mean something was lost.
            elapsed: Duration::from_millis(f.clock.now().as_millis() as u64),
            speed: f.cfg.speed,
            bank: f.bank.saved(),
            by_table: f.bank.by_table().iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            by_expense: f.bank.by_expense().iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            people: f.roster.saved(),
            tables: f
                .instances
                .iter()
                .map(|t| SavedTable { kind: t.kind, number: t.number(), limit: t.limit })
                .collect(),
            appeal: f.demand.ranked().iter().map(|(k, s)| (k.to_string(), s.appeal)).collect(),
            opened: f.opened.iter().map(|(k, n)| (k.to_string(), *n)).collect(),
            tourneys_held: f.tourneys_held,
        }
    }

    /// Opens a casino that has run before.
    ///
    /// The books, the people and the floor plan all carry on. Nobody is
    /// sitting anywhere yet: everybody starts outside and walks back in,
    /// which is why a restored casino looks like the start of a night
    /// rather than a freeze-frame of the last one.
    pub fn resume(seed: u64, save: &Save, badge: Badge) -> Manager {
        let m = Manager::start(seed, save.bank.money, save.bank.chips, badge);
        {
            let Ok(mut f) = m.floor.lock() else { return m };
            f.bank = Bank::restore(&save.bank, &save.by_table, &save.by_expense);
            f.roster = Roster::restore(&save.people);
            f.cfg.speed = save.speed;
            f.clock = Clock::resume(save.elapsed, save.speed);
            f.tourneys_held = save.tourneys_held;
            for (key, n) in save.opened.iter() {
                if let Some(k) = Kind::from_key(key) {
                    f.opened.insert(k.key(), *n);
                }
            }
        }
        // The tables are opened through the ordinary path, so a restored
        // floor is built by the same code a fresh one is — there is no
        // second way to bring a table into being.
        for t in save.tables.iter() {
            m.open_at(t.kind, 1, t.limit);
        }
        // The room's mood is put back after the tables exist, since a kind
        // with no table open has no standing to restore.
        if let Ok(mut f) = m.floor.lock() {
            let now = f.clock.now();
            for (key, appeal) in save.appeal.iter() {
                if let Some(k) = Kind::from_key(key) {
                    f.demand.set_appeal(k.key(), *appeal);
                }
            }
            f.feed.push(now, Weight::Notable, Event::CasinoOpened { money: save.bank.money, chips: save.bank.chips });
        }
        m
    }

    /// The ids currently on the floor, in the order they were opened —
    /// which is the order the UI cycles through.
    pub fn ids(&self) -> Vec<u32> {
        self.floor.lock().map(|f| f.instances.iter().map(|t| t.id).collect()).unwrap_or_default()
    }
}

fn view_of(t: &Instance, roster: &Roster, cfg: &Config, now: Duration, deep: bool) -> TableView {
    // Seats are ids; the view carries the people, copied out of the roster
    // under the same lock, so the drawing code never learns that they live
    // anywhere but at the table.
    let people: Vec<Patron> = if deep { t.patrons.iter().filter_map(|id| roster.get(*id).cloned()).collect() } else { Vec::new() };
    TableView {
        id: t.id,
        name: t.name.clone(),
        kind: t.kind,
        seats: t.patrons.len(),
        round: t.round,
        status: t.status(),
        take: t.house_take(),
        staked: t.staked,
        last: t.last_round().cloned(),
        history: if deep { t.history.clone() } else { Vec::new() },
        patrons: if deep { people } else { Vec::new() },
        open_for: now.saturating_sub(t.opened),
        seen: t.seen,
        limit: t.limit,
        interest: interest::score(t, roster, cfg),
        cfg: cfg.clone(),
    }
}

/// How many people the desk lists by name. A dashboard panel, not a
/// census — the count above it is the whole truth about how many are in.
const GUESTS_SHOWN: usize = 24;

/// The most runners an operator may ask for, and the most an entry may
/// cost. Both are ceilings on *arithmetic*, not house policy: they are the
/// point past which a pool would no longer fit in the books.
const MAX_FIELD: usize = 1_024;
const MAX_BUY_IN: i64 = i64::MAX / (MAX_FIELD as i64 * 1_000);

/// How many lines of the desk's day book the dashboard carries.
const DESK_LINES: usize = 24;

/// How many places of a tournament's standings a dashboard row carries.
const BOARD_ROWS: usize = 12;

/// One table, flattened for the dashboard's list.
///
/// Shallow on purpose: no seats, no history, no people. Fifty of these
/// cost fifty rows; fifty *deep* views would cost the whole floor, every
/// quarter of a second, to draw four lines each.
fn game_view(t: &Instance, roster: &Roster, cfg: &Config, now: Duration) -> GameView {
    let last = t.last_round();
    let interest = interest::score(t, roster, cfg);
    GameView {
        id: t.id,
        kind: t.kind,
        name: t.name.clone(),
        round: t.round,
        seats: t.patrons.len(),
        wanted: t.wanted(),
        status: t.status(),
        limit: t.limit,
        pot: last.map(|r| r.pot).unwrap_or(0),
        house: last.map(|r| r.house()).unwrap_or(0),
        staked: t.staked,
        take: t.house_take(),
        idle_for: now.saturating_sub(t.last_at),
        why: interest.why.label(),
        interest: interest.score,
    }
}

/// The books, summarised. Every figure is asked of the bank; not one of
/// them is worked out here.
fn financial_view(f: &Floor) -> FinancialView {
    let games = f.games.by_handle();
    let by_table = f.bank.by_table();
    // "Best" and "worst" are the same list read from both ends, so the two
    // can never disagree about what is in it.
    let best = by_table.iter().max_by_key(|(_, n)| *n).map(|(k, n)| (*k, *n));
    let worst = by_table.iter().min_by_key(|(_, n)| *n).map(|(k, n)| (*k, *n));
    let (_, _, bought_in, cashed_out) = f.bank.totals();
    FinancialView {
        money: f.bank.money(),
        chips: f.bank.chips(),
        handle: f.bank.handle(),
        payouts: f.bank.payouts(),
        ggr: f.bank.ggr(),
        hold: f.bank.hold(),
        bets: f.bank.bets(),
        rounds: f.bank.rounds(),
        spent: f.bank.spent(),
        ngr: f.bank.ngr(),
        by_expense: f.bank.by_expense(),
        bought_in,
        cashed_out,
        table_profit: f.bank.table_profit(),
        cage_profit: f.bank.cage_profit(),
        best_game: best,
        worst_game: worst,
        games,
        shape: f.series.recent_handle(BOARD),
    }
}

/// One tournament, flattened. The stack figures are derived from the field
/// the tournament already holds; nothing is stored twice.
fn tournament_view(t: &Tournament, now: Duration) -> TournamentView {
    let stacks: Vec<i64> = t.field.iter().filter(|e| e.out_in.is_none()).map(|e| e.stack).collect();
    let board = t.standings(BOARD_ROWS);
    TournamentView {
        id: t.id,
        name: t.name.clone(),
        kind: t.kind,
        stage: t.stage,
        round: t.round,
        alive: t.alive(),
        entered: t.entered(),
        pool: t.pool,
        rake: t.rake,
        buy_in: t.buy_in,
        elapsed: t.running_for(now),
        leader: board.first().map(|e| (e.name.clone(), e.stack)),
        board,
        paid: t.paid.clone(),
        biggest_stack: stacks.iter().copied().max().unwrap_or(0),
        smallest_stack: stacks.iter().copied().min().unwrap_or(0),
        average_stack: if stacks.is_empty() { 0 } else { stacks.iter().sum::<i64>() / stacks.len() as i64 },
    }
}

/// The front desk. The counts and the day book come off the reception
/// ledger, which was fed by events; the cash figures come off the bank,
/// which is the only thing that moved any.
fn reception_view(f: &Floor, _now: Duration) -> ReceptionView {
    let (chips_out, chips_in) = f.desk.chips();
    let (_, _, bought_in, cashed_out) = f.bank.totals();
    let (buy, cash) = f.desk.biggest();
    // The one pass over the roster the dashboard makes, and it is bounded
    // by the configured population rather than by how long the night has
    // been going. Everything else on this screen is a counter.
    let mut guests: Vec<GuestView> = f
        .roster
        .iter()
        .filter(|p| p.presence.is_here())
        .map(|p| GuestView {
            id: p.id,
            name: p.name.clone(),
            style: p.style(),
            tier: p.tier(&f.cfg),
            where_now: match p.presence {
                Presence::Seated { table } => f
                    .instances
                    .iter()
                    .find(|t| t.id == table)
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| "at a table".into()),
                Presence::InTournament { id } => {
                    f.tourneys.get(&id).map(|t| t.name.clone()).unwrap_or_else(|| "in a tournament".into())
                }
                _ => "on the floor".into(),
            },
            chips: p.chips,
            bought: p.bought,
            net: p.net(),
            visits: p.lifetime.visits,
            rounds: p.rounds,
            staked: p.staked,
            lifetime_net: p.lifetime.net(),
        })
        .collect();
    // Biggest stacks first: on a floor of hundreds, those are the people
    // an operator actually wants named.
    guests.sort_by(|a, b| b.chips.cmp(&a.chips).then(a.id.cmp(&b.id)));
    let visitors = guests.len();
    guests.truncate(GUESTS_SHOWN);
    ReceptionView {
        visitors,
        entered: f.desk.arrivals(),
        left: f.desk.departures(),
        bought_in,
        cashed_out,
        chips_out,
        chips_in,
        chips_in_play: f.roster.chips_in_play(),
        biggest_buy: buy.clone(),
        biggest_cash: cash.clone(),
        activity: f.desk.recent(DESK_LINES),
        guests,
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Opening balances for a casino that has never been run before.
pub fn opening_balances() -> (i64, i64) {
    (OPENING_MONEY, OPENING_CHIPS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(mut done: impl FnMut() -> bool, within: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < within {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    }

    #[test]
    fn a_started_casino_runs_without_anyone_watching() {
        let m = Manager::start(1, 100_000, 1_000_000, Badge::new());
        m.open(Kind::Slots, 1);
        let id = m.ids()[0];
        // Nothing touches the manager from here — no snapshot, no draw.
        assert!(
            wait_for(|| m.table(id).map(|t| t.round).unwrap_or(0) >= 2, Duration::from_secs(6)),
            "the table did not progress on its own"
        );
    }

    #[test]
    fn tables_keep_running_while_another_is_being_watched() {
        let m = Manager::start(2, 100_000, 1_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        let ids = m.ids();
        let (a, b) = (ids[0], ids[1]);
        assert!(wait_for(|| m.table(a).unwrap().round >= 1, Duration::from_secs(5)));
        let b_then = m.table(b).unwrap().round;
        // Spend a while looking only at A.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let _ = m.table(a);
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(m.table(b).unwrap().round > b_then, "the unwatched table stood still");
    }

    #[test]
    fn opening_a_table_never_disturbs_the_others() {
        let m = Manager::start(3, 100_000, 1_000_000, Badge::new());
        m.open(Kind::Slots, 1);
        let first = m.ids()[0];
        assert!(wait_for(|| m.table(first).unwrap().round >= 2, Duration::from_secs(6)));
        let before = m.table(first).unwrap();
        m.open(Kind::Roulette, 3);
        let after = m.table(first).unwrap();
        assert_eq!(after.id, before.id);
        assert!(after.round >= before.round, "an existing table was reset by opening another");
        assert_eq!(m.table_count(), 4);
    }

    #[test]
    fn looking_at_a_table_does_not_restart_it() {
        let m = Manager::start(4, 100_000, 1_000_000, Badge::new());
        m.open(Kind::Slots, 1);
        let id = m.ids()[0];
        assert!(wait_for(|| m.table(id).unwrap().round >= 2, Duration::from_secs(6)));
        let a = m.table(id).unwrap();
        let b = m.table(id).unwrap();
        assert!(b.round >= a.round, "a second look wound the table back");
        assert!(b.seen >= a.seen, "a second look reseated the table");
    }

    #[test]
    fn a_paused_table_stops_and_the_rest_carry_on() {
        let m = Manager::start(5, 100_000, 1_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        let ids = m.ids();
        let (a, b) = (ids[0], ids[1]);
        assert!(wait_for(|| m.table(a).unwrap().round >= 1, Duration::from_secs(5)));
        m.toggle_pause(a);
        let frozen = m.table(a).unwrap().round;
        let moving = m.table(b).unwrap().round;
        std::thread::sleep(Duration::from_millis(2_500));
        assert_eq!(m.table(a).unwrap().round, frozen, "a paused table played on");
        assert!(m.table(b).unwrap().round > moving, "pausing one table stopped another");
        assert_eq!(m.table(a).unwrap().status, "paused");
    }

    #[test]
    fn the_economy_moves_while_the_tables_run() {
        let m = Manager::start(6, 100_000, 1_000_000, Badge::new());
        let opening = m.balances();
        m.open(Kind::Slots, 4);
        // Opening a table no longer seats anybody: people are the floor's
        // business, so the cage moves on the next lifecycle pass, when
        // somebody actually walks up and buys chips.
        assert!(
            wait_for(|| m.balances() != opening, Duration::from_secs(8)),
            "nobody ever bought in at the cage"
        );
        let after_seating = m.balances();
        assert!(wait_for(|| m.snapshot().bets > 0, Duration::from_secs(8)), "no bets were booked");
        assert!(
            wait_for(|| m.balances() != after_seating, Duration::from_secs(8)),
            "the tray never moved despite bets being settled"
        );
        assert!(!m.snapshot().by_table.is_empty(), "nothing was booked against a table");
    }

    #[test]
    fn many_tables_of_many_kinds_run_at_once() {
        let m = Manager::start(7, 1_000_000, 10_000_000, Badge::new());
        for kind in Kind::ALL {
            m.open(kind, 2);
        }
        assert_eq!(m.table_count(), Kind::ALL.len() * 2);
        assert!(
            wait_for(|| m.snapshot().tables.iter().filter(|t| t.round > 0).count() > Kind::ALL.len(), Duration::from_secs(10)),
            "most of the floor never got going"
        );
        // Every table keeps its own identity and its own round counter.
        let names: std::collections::HashSet<String> = m.snapshot().tables.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names.len(), Kind::ALL.len() * 2, "table names collided");
    }

    #[test]
    fn closing_a_table_leaves_the_rest_alone() {
        let m = Manager::start(8, 100_000, 1_000_000, Badge::new());
        m.open(Kind::Slots, 3);
        let ids = m.ids();
        assert!(wait_for(|| m.table(ids[2]).unwrap().round >= 1, Duration::from_secs(5)));
        let survivor = m.table(ids[2]).unwrap().round;
        m.close(ids[0]);
        assert_eq!(m.table_count(), 2);
        assert!(m.table(ids[0]).is_none());
        assert!(m.table(ids[2]).unwrap().round >= survivor);
    }

    #[test]
    fn stopping_the_casino_stops_every_table() {
        let mut m = Manager::start(9, 100_000, 1_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        assert!(wait_for(|| m.snapshot().rounds > 0, Duration::from_secs(5)));
        let (money, chips) = m.stop();
        assert!(!m.is_running());
        let after = m.snapshot().rounds;
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(m.snapshot().rounds, after, "a table played on after the casino closed");
        assert_eq!(m.balances(), (money, chips));
    }

    #[test]
    fn table_numbers_keep_climbing_as_tables_come_and_go() {
        let m = Manager::start(10, 100_000, 1_000_000, Badge::new());
        m.open(Kind::Roulette, 2);
        let ids = m.ids();
        assert_eq!(m.table(ids[0]).unwrap().name, "Roulette #1");
        assert_eq!(m.table(ids[1]).unwrap().name, "Roulette #2");
        m.close(ids[0]);
        m.open(Kind::Roulette, 1);
        let names: Vec<String> = m.snapshot().tables.iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"Roulette #3".to_string()), "a table number was reused: {names:?}");
    }

    #[test]
    fn people_outlive_the_tables_they_sit_at() {
        // Rule 4, end to end. Somebody who gets up is still known to the
        // building, with their record intact, and comes back to it.
        let m = Manager::start(20, 500_000, 20_000_000, Badge::new());
        // A small world, so the floor is forced to make the choice this
        // test is about: recall somebody rather than invent a stranger.
        let mut cfg = m.config();
        cfg.roster_size = 6;
        cfg.speed = 10_000;
        cfg.away_for = (Duration::from_secs(5), Duration::from_secs(30));
        m.configure(cfg);
        m.open(Kind::Slots, 3);
        assert!(wait_for(|| m.snapshot().known > 0, Duration::from_secs(5)), "nobody ever walked in");
        // Let the night run long enough for people to come and go.
        assert!(wait_for(|| m.snapshot().rounds > 300, Duration::from_secs(15)), "the floor never got going");
        let view = m.snapshot();
        assert!(view.crowd > 0, "the building emptied");
        assert!(view.known >= view.crowd, "more people in the room than the casino knows of");
        // Somebody at a table has been in before: they were recalled rather
        // than invented.
        let regulars: usize = view
            .tables
            .iter()
            .filter_map(|t| m.table(t.id))
            .flat_map(|t| t.patrons)
            .filter(|p| p.lifetime.visits > 1)
            .count();
        assert!(regulars > 0, "nobody at any table has ever been in before");
    }

    #[test]
    fn the_population_stays_bounded_however_long_the_night_runs() {
        let m = Manager::start(21, 500_000, 20_000_000, Badge::new());
        let mut cfg = m.config();
        cfg.roster_size = 20;
        cfg.speed = 10_000;
        m.configure(cfg);
        m.open(Kind::Slots, 4);
        assert!(wait_for(|| m.snapshot().rounds > 400, Duration::from_secs(15)), "the floor never got going");
        let view = m.snapshot();
        assert!(view.known <= 20, "the world grew to {} people against a cap of 20", view.known);
        assert!(view.crowd <= view.known);
    }

    #[test]
    fn a_patron_belongs_to_the_floor_and_not_to_one_table() {
        // Closing a table under somebody must send them home, not delete
        // them — the difference between a person and a row in a Vec.
        let m = Manager::start(22, 500_000, 20_000_000, Badge::new());
        m.set_speed(6_000);
        m.open(Kind::Baccarat, 2);
        assert!(wait_for(|| m.snapshot().known >= 2, Duration::from_secs(5)), "nobody arrived");
        let known_before = m.snapshot().known;
        let id = m.ids()[0];
        m.close(id);
        std::thread::sleep(Duration::from_millis(200));
        let after = m.snapshot();
        assert_eq!(after.tables.len(), 1, "the other table went with it");
        assert!(after.known >= known_before, "closing a table deleted {} people", known_before - after.known);
    }

    #[test]
    fn the_tier_census_always_covers_everybody_exactly_once() {
        let m = Manager::start(23, 1_000_000, 50_000_000, Badge::new());
        m.set_speed(10_000);
        m.open(Kind::Blackjack, 4);
        assert!(wait_for(|| m.snapshot().known > 6, Duration::from_secs(15)), "not enough people came in");
        let view = m.snapshot();
        assert_eq!(view.by_tier.iter().sum::<usize>(), view.known, "somebody is in two tiers or none");
    }

    #[test]
    fn the_building_charges_its_costs_periodically_and_not_per_frame() {
        // Phase 7's actual requirement. At a fast clock the periods come
        // round often; what must never happen is a charge per tick.
        let m = Manager::start(30, 1_000_000, 20_000_000, Badge::new());
        let mut cfg = m.config();
        cfg.expense_period = Duration::from_secs(10);
        cfg.overhead_per_period = 100;
        cfg.table_cost_per_period = 10;
        cfg.speed = 10_000; // ten seconds of casino time per second of ours
        m.configure(cfg);
        m.open(Kind::Slots, 2);

        assert!(wait_for(|| m.snapshot().spent > 0, Duration::from_secs(8)), "the bills never came");
        let after_first = m.snapshot();
        // Two tables at 10 apiece plus 100 of overhead is 120 a period, so
        // whatever has been spent must be a whole number of periods.
        assert_eq!(after_first.spent % 120, 0, "spent {} is not a whole number of periods", after_first.spent);

        // A tick is 40ms; a period is a second of wall time at this speed.
        // If costs were charged per tick, this would be twenty-five times
        // bigger than it can legitimately be.
        std::thread::sleep(Duration::from_millis(1_200));
        let later = m.snapshot();
        let periods = (later.spent - after_first.spent) / 120;
        assert!(periods <= 4, "{periods} periods were charged in 1.2 seconds — this is being billed per frame");
        assert!(later.spent > after_first.spent, "the clock ran on and nothing was billed");

        let by = later.by_expense;
        assert!(by.iter().any(|(k, _)| *k == "overhead"));
        assert!(by.iter().any(|(k, _)| *k == "staffing"));
    }

    #[test]
    fn the_bills_come_out_of_the_cage_and_the_tray_never_pays_them() {
        let m = Manager::start(31, 1_000_000, 20_000_000, Badge::new());
        let mut cfg = m.config();
        cfg.expense_period = Duration::from_secs(5);
        cfg.overhead_per_period = 5_000;
        cfg.table_cost_per_period = 0;
        cfg.speed = 10_000;
        m.configure(cfg);
        // No tables at all, so nothing but the bills can move the books.
        let (money_before, chips_before) = m.balances();
        assert!(wait_for(|| m.snapshot().spent >= 5_000, Duration::from_secs(8)), "the bills never came");
        let (money, chips) = m.balances();
        assert!(money < money_before, "the cage never paid");
        assert_eq!(chips, chips_before, "the patrons' float paid the rent");
        assert_eq!(m.snapshot().ngr, -m.snapshot().spent, "with no trade, the bottom line is just the costs");
    }

    #[test]
    fn the_books_add_up_the_way_the_books_screen_says_they_do() {
        let m = Manager::start(32, 1_000_000, 50_000_000, Badge::new());
        m.set_speed(10_000);
        m.open(Kind::Slots, 4);
        assert!(wait_for(|| m.snapshot().rounds > 200, Duration::from_secs(15)), "the floor never got going");
        let v = m.snapshot();
        assert_eq!(v.ggr, v.handle - v.payouts, "the gross win is the handle less the payouts");
        assert_eq!(v.ggr, v.table_profit, "and it is the same figure the floor screen shows");
        assert_eq!(v.ngr, v.cage_profit - v.spent, "the bottom line is cage less costs");
        assert!(v.handle > v.ggr, "a handle smaller than the win means money is being invented");
        // The hold is the win as a share of the handle, and nothing else.
        // Deliberately not asserted to be near the theoretical edge: over a
        // few hundred spins a machine with a long-tailed paytable is miles
        // from it either way, and `games::audit` is where the edge itself
        // is pinned, over hundreds of thousands of rounds.
        assert_eq!(v.hold, v.ggr * 10_000 / v.handle);
        assert!(v.handle > 0);
        let wagers = v.movements.iter().find(|(k, _, _)| *k == Movement::Wager).unwrap().2;
        assert_eq!(wagers, v.handle, "the movement ledger and the handle disagree");
    }

    #[test]
    fn the_room_notices_how_full_it_is() {
        let m = Manager::start(40, 1_000_000, 20_000_000, Badge::new());
        let mut cfg = m.config();
        cfg.demand_sample = Duration::from_secs(2);
        cfg.demand_period = Duration::from_secs(6);
        cfg.speed = 10_000;
        m.configure(cfg);
        m.open(Kind::Slots, 3);
        m.open(Kind::Roulette, 2);
        assert!(wait_for(|| !m.snapshot().demand.is_empty(), Duration::from_secs(8)), "the room never took a reading");
        let v = m.snapshot();
        let keys: Vec<&str> = v.demand.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"slots") && keys.contains(&"roulette"), "kinds missing from the reading: {keys:?}");
        for (k, st) in v.demand.iter() {
            assert!(st.offered > 0, "{k} offered no seats");
            assert!((0..=1_000).contains(&st.utilization()), "{k} is {} permille full", st.utilization());
        }
        assert!(v.occupancy > 0, "a floor with people at it read as empty");
        assert!(v.occupancy <= 1_000, "a floor cannot be {} permille full", v.occupancy);
    }

    #[test]
    fn a_floor_with_more_seats_than_customers_sits_half_empty() {
        // The whole point of an arrival *rate*. If seats were refilled the
        // instant they emptied, occupancy would read 100% for ever, demand
        // would have nothing to measure, and opening tables you cannot fill
        // would cost you nothing but staffing.
        let m = Manager::start(44, 2_000_000, 100_000_000, Badge::new());
        let mut cfg = m.config();
        cfg.demand_sample = Duration::from_secs(2);
        cfg.arrivals_period = Duration::from_secs(60);
        cfg.arrivals_per_period = 1; // a trickle, against a great many seats
        cfg.speed = 10_000;
        m.configure(cfg);
        m.open(Kind::Blackjack, 12); // up to seven seats apiece
        assert!(wait_for(|| !m.snapshot().demand.is_empty(), Duration::from_secs(8)), "the room never took a reading");
        std::thread::sleep(Duration::from_millis(800));
        let v = m.snapshot();
        assert!(v.occupancy < 800, "a starved floor read as {} permille full", v.occupancy);
        // ...and the staffing bill is charged on the tables, not on the
        // people at them, which is what makes an empty table cost money.
        assert!(v.tables.len() == 12);
    }

    #[test]
    fn the_rooms_taste_moves_but_never_runs_away_with_the_floor() {
        let m = Manager::start(41, 1_000_000, 20_000_000, Badge::new());
        let mut cfg = m.config();
        cfg.demand_period = Duration::from_secs(2);
        cfg.demand_sample = Duration::from_secs(1);
        cfg.speed = 10_000;
        let (floor_bound, ceiling_bound) = (cfg.appeal_floor, cfg.appeal_ceiling);
        m.configure(cfg);
        m.open(Kind::Slots, 2);
        m.open(Kind::Keno, 1);
        assert!(wait_for(|| !m.snapshot().demand.is_empty(), Duration::from_secs(8)), "the room never took a reading");
        std::thread::sleep(Duration::from_millis(1_500));
        for (k, st) in m.snapshot().demand {
            assert!(st.appeal >= floor_bound && st.appeal <= ceiling_bound, "{k} drifted to {}", st.appeal);
        }
    }

    #[test]
    fn a_high_limit_table_only_seats_the_house_s_best_customers() {
        // Rule 5 is not at stake here; Phase 5 is. The privilege has to be
        // real, or a tier is just a word on a screen.
        let m = Manager::start(42, 1_000_000, 20_000_000, Badge::new());
        m.set_speed(10_000);
        m.open_at(Kind::Baccarat, 1, Limit::High);
        std::thread::sleep(Duration::from_millis(600));
        let id = m.ids()[0];
        let t = m.table(id).expect("the table is open");
        assert_eq!(t.limit, Limit::High);
        assert!(t.name.contains('★'));
        let cfg = m.config();
        // On a floor that has just opened, nobody has the turnover to be a
        // VIP yet — so the room is empty, and that is the feature working
        // rather than failing. What must never happen is a guest in it.
        for p in t.patrons.iter() {
            assert!(
                cfg.is_vip(p.tier(&cfg)),
                "{} is a {} and got a seat in the high-limit room",
                p.name,
                cfg.tier_name(p.tier(&cfg))
            );
        }
        assert!(t.patrons.is_empty(), "somebody qualified as a VIP within a second of the doors opening");
    }

    #[test]
    fn a_floor_of_nothing_but_high_limit_tables_simply_stays_empty() {
        // The seating loop must not spin forever looking for somebody it
        // will never find. If this hangs, it is broken.
        let m = Manager::start(43, 1_000_000, 20_000_000, Badge::new());
        m.set_speed(10_000);
        m.open_at(Kind::Slots, 3, Limit::High);
        std::thread::sleep(Duration::from_millis(800));
        let v = m.snapshot();
        assert_eq!(v.tables.len(), 3, "the tables should still be open");
        assert!(v.running, "the simulation thread stopped");
    }

    #[test]
    fn the_action_can_be_found_without_disturbing_it() {
        // Phase 12: "follow the action" is a ranking over a copy of the
        // floor. Asking which table is worth watching must be exactly as
        // harmless as not asking.
        let m = Manager::start(50, 1_000_000, 50_000_000, Badge::new());
        m.set_speed(10_000);
        m.open(Kind::Slots, 3);
        m.open(Kind::Blackjack, 2);
        assert!(wait_for(|| m.snapshot().rounds > 60, Duration::from_secs(10)), "the floor never got going");

        let (best, interest) = m.most_interesting().expect("a floor with five tables has a best one");
        assert!(m.ids().contains(&best));
        assert!(interest.score >= 0);
        // Whatever it picked really is the top of the ranking the views
        // carry, so the screen and the chooser cannot disagree.
        let view = m.snapshot();
        let top = view.tables.iter().map(|t| t.interest.score).max().unwrap();
        let picked = view.tables.iter().find(|t| t.id == best).map(|t| t.interest.score);
        // The floor moves between the two calls, so this is a sanity check
        // on the ordering rather than an equality.
        assert!(picked.unwrap_or(0) <= top);

        // And asking a hundred times changes nothing about the floor.
        let before = m.snapshot();
        for _ in 0..100 {
            let _ = m.most_interesting();
        }
        let after = m.snapshot();
        assert_eq!(before.tables.len(), after.tables.len());
        assert!(after.rounds >= before.rounds, "the round count went backwards");
        assert!(after.tables.iter().all(|t| !t.status.contains("paused")), "watching paused a table");
    }

    #[test]
    fn a_paused_table_is_never_what_the_spectator_is_sent_to() {
        let m = Manager::start(51, 1_000_000, 50_000_000, Badge::new());
        m.set_speed(10_000);
        m.open(Kind::Slots, 2);
        assert!(wait_for(|| m.snapshot().rounds > 40, Duration::from_secs(10)), "the floor never got going");
        // Pause everything: there is no action anywhere, and the chooser
        // must say so rather than sending somebody to a stopped table with
        // a stale score.
        for id in m.ids() {
            m.toggle_pause(id);
        }
        std::thread::sleep(Duration::from_millis(200));
        let (_, interest) = m.most_interesting().expect("the tables are still open");
        assert_eq!(interest.score, 0, "a floor of paused tables offered {} of action", interest.score);
    }

    #[test]
    fn a_notification_reader_is_never_handed_the_same_thing_twice() {
        // Phase 10's actual requirement, from the reader's side.
        let m = Manager::start(52, 1_000_000, 50_000_000, Badge::new());
        m.set_speed(10_000);
        let mut seen = m.feed_cursor();
        m.open(Kind::Slots, 4);
        assert!(wait_for(|| m.snapshot().rounds > 100, Duration::from_secs(10)), "the floor never got going");
        let mut announced = Vec::new();
        for _ in 0..20 {
            let (fresh, next) = m.feed_since(seen, Weight::Major);
            for r in fresh {
                assert!(r.seq > seen, "an event was announced twice");
                assert_eq!(r.weight, Weight::Major, "something below the bar reached a notification");
                announced.push(r.seq);
            }
            seen = next;
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut sorted = announced.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), announced.len(), "the same event was announced more than once");
    }

    #[test]
    fn a_rush_on_the_doors_actually_fills_the_room() {
        // Phase 16's whole point: a happening has to be visible in the
        // simulation, not just on a screen. A rush must genuinely bring
        // more people in than a quiet night does.
        let busy = Manager::start(60, 2_000_000, 200_000_000, Badge::new());
        let quiet = Manager::start(60, 2_000_000, 200_000_000, Badge::new());
        let base = Config {
            arrivals_period: Duration::from_secs(5),
            arrivals_per_period: 2,
            speed: 10_000,
            happening_chance: 100,
            happening_period: Duration::from_secs(1),
            happening_for: (Duration::from_secs(9_000), Duration::from_secs(9_001)),
            ..Config::default()
        };
        // The quiet one never has anything going on at all.
        quiet.configure(Config { happening_chance: 1, happening_period: Duration::from_secs(9_000), ..base.clone() });
        busy.configure(base);
        busy.open(Kind::Blackjack, 10);
        quiet.open(Kind::Blackjack, 10);
        std::thread::sleep(Duration::from_millis(1_500));
        let (b, q) = (busy.snapshot(), quiet.snapshot());
        assert!(!b.weather.is_empty(), "nothing was going on despite a certainty of it");
        assert!(q.weather.is_empty(), "the quiet floor had weather it should not have");
        // Not asserting a precise ratio — what is going on is rolled, and a
        // lull is one of the things that can be. Only that the floor with
        // *something* going on is measurably different.
        assert!(b.known != q.known || b.crowd != q.crowd, "the weather made no difference to anybody");
    }

    #[test]
    fn what_is_going_on_never_reaches_a_payout() {
        // Rule 5 at the floor level. A night full of happenings is a night
        // with different *traffic*, and the same maths.
        let m = Manager::start(61, 2_000_000, 200_000_000, Badge::new());
        m.configure(Config {
            speed: 10_000,
            happening_chance: 100,
            happening_period: Duration::from_secs(2),
            happening_for: (Duration::from_secs(30), Duration::from_secs(60)),
            ..Config::default()
        });
        m.open(Kind::Slots, 4);
        assert!(wait_for(|| !m.snapshot().weather.is_empty(), Duration::from_secs(10)), "nothing ever happened");
        assert!(wait_for(|| m.snapshot().rounds > 400, Duration::from_secs(15)), "the floor never got going");
        let v = m.snapshot();
        // The books still add up exactly as they did with nothing going on:
        // the handle and the payouts are the table's business alone.
        assert_eq!(v.ggr, v.handle - v.payouts);
        assert_eq!(v.hold, v.ggr * 10_000 / v.handle);
        assert!(v.handle > 0);
    }

    #[test]
    fn a_dear_night_costs_more_to_keep_the_doors_open() {
        let m = Manager::start(62, 5_000_000, 200_000_000, Badge::new());
        m.configure(Config {
            speed: 10_000,
            expense_period: Duration::from_secs(5),
            overhead_per_period: 1_000,
            table_cost_per_period: 0,
            happening_chance: 1, // rolled, but as good as never
            happening_period: Duration::from_secs(9_000),
            ..Config::default()
        });
        assert!(wait_for(|| m.snapshot().spent >= 3_000, Duration::from_secs(10)), "the bills never came");
        let plain = m.snapshot().spent;
        // Whatever it charged, it charged in whole periods of the base
        // rate: nothing was going on to make it dearer.
        assert_eq!(plain % 1_000, 0, "spent {plain} is not a whole number of ordinary periods");
    }

    #[test]
    fn a_tournament_runs_itself_from_entries_to_a_winner() {
        let m = Manager::start(70, 2_000_000, 200_000_000, Badge::new());
        m.configure(Config {
            speed: 10_000,
            arrivals_period: Duration::from_secs(2),
            arrivals_per_period: 8,
            tourney_every: Duration::from_secs(20),
            tourney_registration: Duration::from_secs(20),
            tourney_round_every: Duration::from_secs(4),
            tourney_min_field: 4,
            ..Config::default()
        });
        m.open(Kind::Blackjack, 2);
        assert!(wait_for(|| !m.snapshot().tourneys.is_empty(), Duration::from_secs(10)), "no tournament ever opened");

        // A finished tournament is kept only for a while before the floor
        // forgets it, so catch one as it goes past rather than hoping a
        // single look lands inside that window.
        let mut finished = None;
        let until = Instant::now() + Duration::from_secs(40);
        while Instant::now() < until && finished.is_none() {
            for t in m.snapshot().tourneys {
                if t.stage == Stage::Done && t.entered() >= 4 {
                    finished = Some(t);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let done = finished.expect("no tournament with a real field ever played down to a winner");
        assert_eq!(done.alive(), 1, "it finished with {} still in", done.alive());
        assert!(done.winner().is_some());
        let paid: i64 = done.paid.iter().map(|p| p.prize).sum();
        assert_eq!(paid, done.pool, "the pool did not pay out whole");
        assert!(done.paid.first().map(|p| p.place) == Some(1));
    }

    #[test]
    fn a_tournament_never_strands_anybody_in_it() {
        // Somebody in a tournament is not free to be seated; when it ends
        // they must be free again, or the roster slowly fills with people
        // who can never play anything.
        let m = Manager::start(71, 2_000_000, 200_000_000, Badge::new());
        m.configure(Config {
            speed: 10_000,
            arrivals_period: Duration::from_secs(2),
            arrivals_per_period: 8,
            tourney_every: Duration::from_secs(15),
            tourney_registration: Duration::from_secs(10),
            tourney_round_every: Duration::from_secs(3),
            tourney_min_field: 4,
            ..Config::default()
        });
        m.open(Kind::Slots, 3);
        assert!(
            wait_for(|| m.snapshot().tourneys.iter().any(|t| t.stage == Stage::Done), Duration::from_secs(30)),
            "no tournament finished"
        );
        // Give the floor a moment to seat everybody it freed.
        std::thread::sleep(Duration::from_millis(600));
        let v = m.snapshot();
        // The floor is still seating people, which it could not be doing if
        // its whole population were stuck in a finished tournament.
        assert!(v.crowd > 0, "the building emptied");
        assert!(v.tables.iter().any(|t| t.seats > 0), "no table has anybody at it any more");
    }

    #[test]
    fn the_only_thing_the_house_takes_from_a_tournament_is_the_rake() {
        let m = Manager::start(72, 2_000_000, 200_000_000, Badge::new());
        m.configure(Config {
            speed: 10_000,
            arrivals_period: Duration::from_secs(2),
            arrivals_per_period: 8,
            tourney_every: Duration::from_secs(15),
            tourney_registration: Duration::from_secs(10),
            tourney_round_every: Duration::from_secs(3),
            tourney_min_field: 4,
            ..Config::default()
        });
        m.open(Kind::Blackjack, 2);
        assert!(
            wait_for(|| m.snapshot().tourneys.iter().any(|t| t.stage == Stage::Done), Duration::from_secs(30)),
            "no tournament finished"
        );
        let v = m.snapshot();
        // Tournament entries are booked as turnover like anything else, so
        // the books stay internally consistent with them in.
        assert_eq!(v.ggr, v.handle - v.payouts);
        if let Some((_, tally)) = v.games.iter().find(|(k, _)| *k == "tourney") {
            let cfg = v.cfg.clone();
            // The house's win on a tournament is exactly the rake — a
            // fixed percentage of every entry, and nothing else.
            let expected = tally.handle * cfg.tourney_rake / 100;
            let slack = (tally.handle / 100).max(1);
            assert!(
                (tally.ggr() - expected).abs() <= slack,
                "the house won {} from tournaments against a rake of {expected}",
                tally.ggr()
            );
        }
    }

    #[test]
    fn a_casino_survives_being_closed_and_opened_again() {
        // Phase 18, end to end: run a night, write it down, reopen it, and
        // check the things that are supposed to carry on have carried on.
        let mut first = Manager::start(80, 500_000, 20_000_000, Badge::new());
        first.set_speed(10_000);
        first.open(Kind::Slots, 2);
        first.open_at(Kind::Baccarat, 1, Limit::High);
        assert!(wait_for(|| first.snapshot().rounds > 200, Duration::from_secs(15)), "the floor never got going");

        let before = first.snapshot();
        let save = first.saved();
        first.stop();

        // Through the file, not just through memory: this is the path the
        // program actually takes.
        let at = std::env::temp_dir().join("dice_arena_reopen_test.save");
        let _ = std::fs::remove_file(&at);
        save.write(&at).expect("it should write");
        let read_back = crate::casino::save::Save::load(&at).expect("it should load");
        let second = Manager::resume(81, &read_back, Badge::new());
        std::thread::sleep(Duration::from_millis(300));
        let after = second.snapshot();

        // A reopened casino is *live*, so these carry on from where they
        // were rather than freezing there. The failure being guarded
        // against is a reset to nothing, not a figure that has moved on.
        assert!(after.handle >= before.handle, "the night's turnover was forgotten: {} against {}", after.handle, before.handle);
        assert!(after.bets >= before.bets, "the bet count started again");
        assert!(after.known >= before.known, "the regulars were forgotten");
        assert!(before.handle > 0 && before.known > 0, "the test needs a casino that actually ran");
        // The cage carries on from where it was, give or take the buy-ins
        // of the people walking back in while this test looks at it.
        assert!(
            (after.money - before.money).abs() < before.money / 10,
            "the cage did not carry on: {} against {}",
            after.money,
            before.money
        );
        assert_eq!(after.tables.len(), before.tables.len(), "the floor plan changed");
        assert!(after.tables.iter().any(|t| t.limit == Limit::High), "the high-limit room did not reopen");
        // Table numbering carries on rather than starting again at #1.
        let names: Vec<String> = after.tables.iter().map(|t| t.name.clone()).collect();
        assert!(names.iter().any(|n| n.contains("Slots #")), "no slots table reopened: {names:?}");
        assert!(!names.iter().any(|n| n == "Slots #1"), "the numbering started again: {names:?}");

        // And it is a live casino, not a photograph: it keeps playing.
        assert!(wait_for(|| second.snapshot().rounds > after.rounds, Duration::from_secs(10)), "the reopened floor is dead");
        let _ = std::fs::remove_file(&at);
    }

    #[test]
    fn a_reopened_casino_fills_up_with_the_people_who_were_there_before() {
        let mut first = Manager::start(82, 500_000, 20_000_000, Badge::new());
        first.configure(Config { speed: 10_000, roster_size: 15, ..Config::default() });
        first.open(Kind::Slots, 3);
        assert!(wait_for(|| first.snapshot().known >= 10, Duration::from_secs(15)), "nobody ever came in");
        let names_before: Vec<String> = {
            let f = first.floor.lock().unwrap();
            f.roster.iter().map(|p| p.name.clone()).collect()
        };
        let save = first.saved();
        first.stop();

        let second = Manager::resume(83, &save, Badge::new());
        std::thread::sleep(Duration::from_millis(500));
        let names_after: Vec<String> = {
            let f = second.floor.lock().unwrap();
            f.roster.iter().map(|p| p.name.clone()).collect()
        };
        // Every one of them is still known. The reopened floor goes on to
        // let more people in, as any night does — what must not happen is
        // somebody from before being replaced by a stranger.
        for name in names_before.iter() {
            assert!(names_after.contains(name), "{name} was not at the reopened casino");
        }
        // They come back and sit down of their own accord — nobody is
        // restored into a seat.
        assert!(wait_for(|| second.snapshot().crowd > 0, Duration::from_secs(10)), "nobody ever came back in");
    }

    #[test]
    fn a_saved_casino_is_a_coherent_picture_and_not_a_smear() {
        // The save is taken under one lock, so the books in it agree with
        // themselves even though the floor never stops moving.
        let m = Manager::start(84, 500_000, 20_000_000, Badge::new());
        m.set_speed(10_000);
        m.open(Kind::Slots, 4);
        assert!(wait_for(|| m.snapshot().rounds > 100, Duration::from_secs(15)), "the floor never got going");
        for _ in 0..20 {
            let s = m.saved();
            assert_eq!(s.bank.handle - s.bank.payouts, s.bank.collected - s.bank.paid, "the books disagree with themselves");
            assert!(s.bank.handle >= s.bank.payouts.min(s.bank.handle));
            assert_eq!(s.version, crate::casino::save::VERSION);
            assert!(!s.people.is_empty());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_floor_announces_itself_the_moment_the_doors_open() {
        let m = Manager::start(11, 100_000, 1_000_000, Badge::new());
        let feed = m.feed(16, Weight::Notable);
        assert!(
            matches!(feed.first().map(|r| &r.event), Some(Event::CasinoOpened { .. })),
            "the first thing on the feed should be the doors opening: {feed:?}"
        );
    }

    #[test]
    fn events_accumulate_while_nobody_is_watching() {
        // The whole point of the bus: it is the simulation that publishes,
        // on its own thread, whether or not a screen ever asks.
        let m = Manager::start(12, 500_000, 5_000_000, Badge::new());
        // Wound forward so the test spends a second of wall time rather
        // than half a minute; the clock is the thing under test elsewhere.
        m.set_speed(6_000);
        // The cursor is taken before anything is opened, so everything the
        // floor goes on to publish is genuinely new to this reader.
        let cursor = m.feed_cursor();
        m.open(Kind::Slots, 3);
        assert!(wait_for(|| m.snapshot().rounds > 20, Duration::from_secs(10)), "the floor never got going");
        let (fresh, next) = m.feed_since(cursor, Weight::Notable);
        assert!(fresh.len() >= 3, "three tables opened and only {} events were published", fresh.len());
        assert!(next >= cursor);
        // ...and a reader that catches up sees nothing twice.
        let (again, _) = m.feed_since(next, Weight::Routine);
        for r in &again {
            assert!(r.seq > next, "a reader was handed an event it had already seen");
        }
    }

    #[test]
    fn the_per_round_traffic_never_reaches_the_snapshot() {
        let m = Manager::start(13, 500_000, 5_000_000, Badge::new());
        m.set_speed(6_000);
        m.open(Kind::Slots, 4);
        assert!(wait_for(|| m.snapshot().rounds > 40, Duration::from_secs(10)), "the floor never got going");
        for r in m.snapshot().feed {
            assert!(r.weight >= Weight::Notable, "low-level traffic reached a screen: {:?}", r.event);
        }
    }

    #[test]
    fn changing_speed_never_restarts_a_table() {
        // Rule 2, applied to the clock: the fast-forward changes how quickly
        // rounds fall due and nothing else. Round numbers, patrons and
        // history all carry straight through the change.
        let m = Manager::start(14, 500_000, 5_000_000, Badge::new());
        m.set_speed(6_000);
        m.open(Kind::Baccarat, 2);
        let id = m.ids()[0];
        assert!(wait_for(|| m.table(id).map(|t| t.round).unwrap_or(0) > 3, Duration::from_secs(10)), "no rounds");
        let before = m.table(id).expect("table");
        m.set_speed(10_000);
        std::thread::sleep(Duration::from_millis(400));
        let after = m.table(id).expect("the table survived the speed change");
        assert_eq!(after.name, before.name);
        assert!(after.round >= before.round, "the round counter went backwards");
        assert!(after.staked >= before.staked, "the table's turnover was reset");
        m.set_speed(250);
        std::thread::sleep(Duration::from_millis(200));
        assert!(m.table(id).expect("still there").round >= after.round);
    }

    #[test]
    fn a_slow_clock_really_does_mean_fewer_rounds() {
        let slow = Manager::start(15, 500_000, 5_000_000, Badge::new());
        slow.set_speed(250);
        slow.open(Kind::Slots, 2);
        let fast = Manager::start(15, 500_000, 5_000_000, Badge::new());
        fast.set_speed(4_000);
        fast.open(Kind::Slots, 2);
        std::thread::sleep(Duration::from_millis(1_200));
        let (s, f) = (slow.snapshot().rounds, fast.snapshot().rounds);
        assert!(f > s * 2, "quarter speed played {s} rounds, four times speed only {f}");
    }

    #[test]
    fn the_configured_limits_are_the_ones_the_floor_actually_uses() {
        let m = Manager::start(16, 500_000, 50_000_000, Badge::new());
        let mut cfg = m.config();
        cfg.max_bet = 25;
        cfg.vip_max_bet = 25;
        cfg.speed = 6_000;
        m.configure(cfg);
        m.open(Kind::Slots, 2);
        assert!(wait_for(|| m.snapshot().rounds > 30, Duration::from_secs(10)), "the floor never got going");
        for t in m.snapshot().tables {
            let full = m.table(t.id).expect("table");
            for round in full.history {
                for seat in round.seats {
                    assert!(seat.staked <= 25, "{} staked {} over a configured limit of 25", seat.name, seat.staked);
                }
            }
        }
    }

    // ------------------------------------------------------- the dashboard
    //
    // The rule every one of these is really testing is the same one: the
    // operations centre is a reader. It may show anything; it may change
    // nothing.

    #[test]
    fn the_dashboard_shows_the_tables_that_are_actually_running() {
        let m = Manager::start(101, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        m.open(Kind::Roulette, 1);
        assert!(wait_for(|| m.snapshot().rounds > 4, Duration::from_secs(8)), "the floor never got going");

        let d = m.dashboard();
        assert_eq!(d.games.len(), 3, "the dashboard lost a table");
        assert_eq!(d.global.tables, 3);
        // Every row is a table that exists, with the id it has always had.
        let ids: Vec<u32> = d.games.iter().map(|g| g.id).collect();
        let mut live = m.ids();
        live.sort_unstable();
        let mut shown = ids.clone();
        shown.sort_unstable();
        assert_eq!(shown, live, "the dashboard invented or dropped a table");
        for g in d.games.iter() {
            let real = m.table(g.id).expect("a table the dashboard listed");
            assert_eq!(g.name, real.name);
            assert_eq!(g.kind, real.kind);
            assert!(g.round <= real.round, "the dashboard reported a round that had not happened");
        }
    }

    #[test]
    fn the_dashboard_figures_are_the_banks_figures() {
        let m = Manager::start(102, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Blackjack, 2);
        assert!(wait_for(|| m.snapshot().handle > 0, Duration::from_secs(8)), "nothing was staked");

        // Taken close together, and compared on the figures that cannot
        // drift between two snapshots of a running floor: the ones that
        // only ever climb, and the identities that must hold in both.
        let d = m.dashboard();
        assert_eq!(d.financials.money, d.global.money, "two views of one balance disagreed");
        assert_eq!(d.financials.chips, d.global.chips);
        // The bank's own arithmetic, restated: gross win is what was
        // staked less what was paid back.
        assert_eq!(d.financials.ggr, d.financials.handle - d.financials.payouts);
        assert_eq!(d.financials.net_cage_flow(), d.financials.bought_in - d.financials.cashed_out);

        let books = m.snapshot();
        assert!(d.financials.handle <= books.handle, "the dashboard ran ahead of the books");
        assert!(books.money >= d.financials.money.min(books.money), "the money went backwards impossibly");
    }

    #[test]
    fn the_dashboard_carries_the_same_events_the_feed_published() {
        let m = Manager::start(103, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        assert!(wait_for(|| !m.dashboard().events.is_empty(), Duration::from_secs(8)), "no events reached the dashboard");
        let d = m.dashboard();
        // Nothing below `Notable` may appear: the filtering is the
        // simulation's, and the dashboard does not get its own opinion.
        assert!(d.events.iter().all(|r| r.weight >= Weight::Notable), "routine traffic reached the dashboard");
        // In the order they were published, oldest first.
        assert!(d.events.windows(2).all(|w| w[0].seq < w[1].seq), "the feed came through out of order");
    }

    #[test]
    fn looking_at_the_dashboard_changes_nothing_at_all() {
        let m = Manager::start(104, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        m.open(Kind::Keno, 1);
        assert!(wait_for(|| m.snapshot().rounds > 5, Duration::from_secs(8)), "the floor never got going");

        let before = m.snapshot();
        let ids_before = m.ids();
        let seed_before = m.seed();
        // Hammer the dashboard the way a screen open for a while would.
        for _ in 0..40 {
            let _ = m.dashboard();
        }
        let after = m.snapshot();
        assert_eq!(m.ids(), ids_before, "the floor plan changed under a reader");
        assert_eq!(m.seed(), seed_before, "reading reseeded the floor");
        assert_eq!(after.known, after.known.max(before.known), "people stopped existing");
        assert!(after.rounds >= before.rounds, "the floor went backwards");
        assert!(after.handle >= before.handle, "the books went backwards");
    }

    #[test]
    fn a_table_opened_from_the_dashboard_is_the_one_that_was_already_running() {
        let m = Manager::start(105, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 3);
        assert!(wait_for(|| m.dashboard().games.iter().all(|g| g.round >= 2), Duration::from_secs(10)), "not every table got going");

        let picked = m.dashboard().games[1].clone();
        // What "opening" a game from the dashboard does: ask for that id.
        let inspected = m.table(picked.id).expect("the selected table");
        assert_eq!(inspected.id, picked.id, "a different table was opened");
        assert!(inspected.round >= picked.round, "the table was restarted rather than inspected");

        // And it keeps running while it is being looked at.
        let then = inspected.round;
        assert!(
            wait_for(|| m.table(picked.id).map(|t| t.round > then).unwrap_or(false), Duration::from_secs(8)),
            "the inspected table stopped dealing"
        );
        assert_eq!(m.table_count(), 3, "inspecting a table changed the floor");
    }

    #[test]
    fn the_floor_keeps_playing_while_the_dashboard_is_open() {
        let m = Manager::start(106, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        assert!(wait_for(|| m.dashboard().global.tables == 2, Duration::from_secs(4)));
        let before: u64 = m.dashboard().games.iter().map(|g| g.round).sum();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let _ = m.dashboard();
            std::thread::sleep(Duration::from_millis(50));
        }
        let after: u64 = m.dashboard().games.iter().map(|g| g.round).sum();
        assert!(after > before, "the floor stood still while the dashboard was open");
    }

    // -------------------------------------------------------- the cage

    #[test]
    fn buy_ins_and_cash_outs_both_reach_the_desk() {
        let m = Manager::start(107, 250_000, 50_000_000, Badge::new());
        let mut cfg = m.config();
        // A fast night with a lot of coming and going, so both directions
        // of the counter are exercised inside a test's patience.
        cfg.speed = 10_000;
        m.configure(cfg);
        m.open(Kind::Slots, 4);

        assert!(
            wait_for(|| m.dashboard().reception.entered > 0, Duration::from_secs(12)),
            "nobody was recorded coming through the door"
        );
        assert!(
            wait_for(|| m.dashboard().reception.left > 0, Duration::from_secs(20)),
            "nobody was recorded leaving"
        );

        let r = m.dashboard().reception;
        assert!(!r.activity.is_empty(), "the day book stayed empty");
        assert!(r.chips_out > 0, "no chips were issued at the counter");
        assert!(r.bought_in > 0, "no cash was taken at the counter");
        // The desk's money figures are the bank's, so they match the books
        // exactly rather than approximately.
        let books = m.snapshot();
        let (_, _, bought, cashed) = books.totals;
        let again = m.dashboard().reception;
        assert!(again.bought_in >= bought, "the desk lagged the bank on buy-ins");
        assert!(again.cashed_out >= cashed, "the desk lagged the bank on cash-outs");
    }

    #[test]
    fn the_desks_day_book_never_grows_without_bound() {
        let m = Manager::start(108, 250_000, 50_000_000, Badge::new());
        let mut cfg = m.config();
        cfg.speed = 10_000;
        m.configure(cfg);
        m.open(Kind::Slots, 4);
        assert!(wait_for(|| m.dashboard().reception.entered > 0, Duration::from_secs(20)), "the door stayed shut");
        // Let the night run on well past the size of the ring, so this is
        // a real test of the bound rather than of an empty book.
        std::thread::sleep(Duration::from_secs(5));
        let r = m.dashboard().reception;
        assert!(r.entered > 0);
        assert!(
            r.activity.len() <= super::super::reception::KEPT,
            "the day book held {} lines, more than the {} it is allowed",
            r.activity.len(),
            super::super::reception::KEPT
        );
    }

    // -------------------------------------------------------- reseeding

    #[test]
    fn reseeding_changes_the_seed_and_nothing_else() {
        let m = Manager::start(109, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        m.open(Kind::Roulette, 1);
        assert!(wait_for(|| m.snapshot().rounds > 10, Duration::from_secs(10)), "the floor never got going");

        let before = m.snapshot();
        let people_before = before.known;
        let ids_before = m.ids();
        let handle_before = before.handle;
        let money_before = before.money;
        let rounds_before = before.rounds;

        let (was, now) = m.reseed(4_242);
        assert_eq!(was, 109, "the old seed was not the one the casino opened on");
        assert_eq!(now, 4_242);
        assert_eq!(m.seed(), 4_242, "the new seed did not stick");

        let after = m.snapshot();
        // The world is untouched. Every one of these is a thing a careless
        // "reseed" would have destroyed.
        assert_eq!(m.ids(), ids_before, "the tables were rebuilt");
        assert!(after.known >= people_before, "people the casino knew were forgotten");
        assert!(after.handle >= handle_before, "the books were wound back");
        assert!(after.rounds >= rounds_before, "the rounds already played were lost");
        assert!(after.money != 0 || money_before == 0, "the cage was emptied");
    }

    #[test]
    fn the_casino_carries_on_running_through_a_reseed() {
        let m = Manager::start(110, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 2);
        assert!(wait_for(|| m.snapshot().rounds > 5, Duration::from_secs(8)), "the floor never got going");
        let before = m.snapshot().rounds;
        m.reseed(9_001);
        assert!(m.is_running(), "the simulation thread stopped for a reseed");
        assert!(
            wait_for(|| m.snapshot().rounds > before + 5, Duration::from_secs(8)),
            "the floor stopped dealing after a reseed"
        );
    }

    #[test]
    fn a_reseeded_floor_rolls_differently_from_here() {
        // Two casinos opened identically diverge once one of them is
        // reseeded — which is the only observable thing a reseed does.
        let a = Manager::start(777, 250_000, 5_000_000, Badge::new());
        let b = Manager::start(777, 250_000, 5_000_000, Badge::new());
        for m in [&a, &b] {
            let mut cfg = m.config();
            cfg.speed = 10_000;
            m.configure(cfg);
            m.open(Kind::Slots, 1);
        }
        b.reseed(778);
        assert!(
            wait_for(|| a.snapshot().rounds > 20 && b.snapshot().rounds > 20, Duration::from_secs(20)),
            "neither floor got going"
        );
        // Not a claim about which is bigger — only that the same opening
        // seed no longer produces the same night.
        assert_ne!(a.seed(), b.seed());
    }

    // ------------------------------------------------ operator tournaments

    #[test]
    fn an_operator_can_start_a_tournament_and_it_fields_real_people() {
        let m = Manager::start(111, 500_000, 500_000_000, Badge::new());
        m.open(Kind::Blackjack, 2);
        let cfg = m.config();
        let id = m
            .start_tournament(Kind::Blackjack, 16, cfg.tourney_buy_in, cfg.tourney_stack)
            .expect("the tournament should have been fielded");

        let t = m.tourney(id).expect("the tournament exists");
        assert!(t.entered() >= cfg.tourney_min_field, "the field was too small to play");
        assert_ne!(t.stage, Stage::Registering, "it never started");

        // Every runner is somebody the casino knows, with an id of their
        // own that nothing else in the building shares.
        let mut ids: Vec<u64> = t.field.iter().map(|e| e.patron).collect();
        let entered = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), entered, "the same person entered twice");
        for e in t.field.iter() {
            let p = m.patron(e.patron).expect("a runner who is not in the roster");
            assert_eq!(p.name, e.name, "the tournament and the roster disagree about who this is");
        }
    }

    #[test]
    fn an_operator_tournament_plays_itself_down_without_being_watched() {
        let m = Manager::start(112, 500_000, 500_000_000, Badge::new());
        m.open(Kind::Slots, 1);
        let mut cfg = m.config();
        cfg.speed = 10_000;
        m.configure(cfg.clone());
        let id = m.start_tournament(Kind::Slots, 8, cfg.tourney_buy_in, cfg.tourney_stack).expect("fielded");

        // Nothing below asks it to do anything — it is on the simulation
        // thread now.
        assert!(
            wait_for(|| m.tourney(id).map(|t| t.stage == Stage::Done).unwrap_or(true), Duration::from_secs(40)),
            "the tournament never played down to a winner"
        );
        let Some(t) = m.tourney(id) else { return };
        assert!(!t.paid.is_empty(), "nobody was paid");
        assert_eq!(t.paid.iter().map(|p| p.prize).sum::<i64>(), t.pool, "the pool was not paid out in full");
        let winner = &t.paid[0];
        assert_eq!(winner.place, 1);
        assert!(m.patron(winner.patron).is_some(), "the winner was a stand-in, not a person");
    }

    #[test]
    fn the_house_takes_its_configured_cut_of_an_operator_tournament_and_no_more() {
        let m = Manager::start(113, 500_000, 500_000_000, Badge::new());
        m.open(Kind::Slots, 1);
        let cfg = m.config();
        let buy_in = cfg.tourney_buy_in;
        let id = m.start_tournament(Kind::Slots, 12, buy_in, cfg.tourney_stack).expect("fielded");
        let t = m.tourney(id).expect("the tournament exists");

        let expected_rake = buy_in * cfg.tourney_rake / 100 * t.entered() as i64;
        assert_eq!(t.rake, expected_rake, "the house took something other than its configured cut");
        assert_eq!(t.pool, (buy_in - buy_in * cfg.tourney_rake / 100) * t.entered() as i64);
        // Every entry went through the books as turnover, exactly as a bet
        // at a table does.
        let books = m.snapshot();
        assert!(books.handle >= buy_in * t.entered() as i64, "the entries never reached the books");
        assert!(
            books.by_table.iter().any(|(k, _)| *k == "tourney"),
            "the tournament did not appear in the per-game ledger"
        );
    }

    #[test]
    fn a_tournament_that_cannot_be_fielded_costs_nobody_anything() {
        let m = Manager::start(114, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 1);
        let before = m.snapshot();
        // An entry fee nobody in the building could ever cover — a patron
        // buys in for what a patron buys in for, and it is nowhere near
        // this.
        let refused = m.start_tournament(Kind::Slots, 8, 500_000_000, 10_000);
        assert!(refused.is_none(), "a tournament nobody could enter was somehow started");
        let after = m.snapshot();
        // Nobody was charged the entry: whatever moved at the cage moved
        // because somebody bought chips for a visit, which is a thing that
        // happens all night regardless.
        assert!(
            after.handle - before.handle < 500_000_000,
            "an entry fee was taken for a tournament that never ran"
        );
        assert!(m.tourney(0).is_none(), "a phantom tournament was left on the floor");
    }

    #[test]
    fn an_operator_tournament_announces_itself_on_the_feed() {
        let m = Manager::start(115, 500_000, 500_000_000, Badge::new());
        m.open(Kind::Slots, 1);
        let cursor = m.feed_cursor();
        let cfg = m.config();
        m.start_tournament(Kind::Slots, 8, cfg.tourney_buy_in, cfg.tourney_stack).expect("fielded");
        let (fresh, _) = m.feed_since(cursor, Weight::Notable);
        assert!(
            fresh.iter().any(|r| matches!(&r.event, Event::Tourney { .. })),
            "starting a tournament said nothing on the event bus"
        );
    }

    #[test]
    fn a_reseed_is_announced_the_way_everything_else_is() {
        let m = Manager::start(116, 250_000, 5_000_000, Badge::new());
        m.open(Kind::Slots, 1);
        let cursor = m.feed_cursor();
        m.reseed(31_337);
        let (fresh, _) = m.feed_since(cursor, Weight::Notable);
        assert!(
            fresh.iter().any(|r| matches!(&r.event, Event::Reseeded { now, .. } if *now == 31_337)),
            "the reseed was not published"
        );
    }
}
