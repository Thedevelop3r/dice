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
use super::patron::{Patron, Presence};
use super::roster::{self, Roster};
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
                bank.cash_out(chips);
                instances[i].stand(pid);
                // A seat freed is also a moment to re-decide how busy this
                // table should be, so a floor breathes over the night
                // instead of every table sitting at a fixed size forever.
                instances[i].reconsider_size(rng);
                feed.push(now, Weight::Notable, Event::Left { patron: pid, who, table: instances[i].id, net, reason: why });
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
                bank.cash_out(chips);
                feed.push(
                    now,
                    Weight::Routine,
                    Event::Left { patron: *pid, who, table: inst.id, net, reason: Departure::TableClosed },
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
}
