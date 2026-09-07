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

use super::bank::{Bank, OPENING_CHIPS, OPENING_MONEY};
use super::clock::Clock;
use super::config::{self, Config};
use super::event::{Departure, Event, Feed, Record, Weight};
use super::instance::{Instance, Kind, RoundLog};
use super::patron::Patron;
use super::roster::{self, Roster};
use super::sim::Sim;
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
                let Floor { roster, rng, cfg, bank, feed, instances, .. } = self;
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
            }
        }

        // 3. Empty seats are offered to whoever is here. A person picks the
        //    table among those with room that most suits them — the "choose
        //    a game" step — rather than being posted to the first vacancy.
        loop {
            let hungry: Vec<usize> =
                (0..self.instances.len()).filter(|i| !self.instances[*i].paused && self.instances[*i].patrons.len() < self.instances[*i].wanted()).collect();
            if hungry.is_empty() {
                break;
            }
            let Floor { roster, rng, cfg, bank, feed, instances, .. } = self;
            let Some(pid) = roster.someone(rng, cfg, now) else { break };
            let Some(p) = roster.get_mut(pid) else { break };

            // Where would they like to sit? Taste scores every table with
            // room; the highest wins, with a wobble so identical people do
            // not all pile onto the same table.
            let mut best = hungry[0];
            let mut best_score = i64::MIN;
            for i in hungry.iter().copied() {
                let kind = instances[i].kind;
                let score = p.taste(kind.variants(), kind.pace().as_millis() as u64) + rng.below(25) as i64;
                if score > best_score {
                    best_score = score;
                    best = i;
                }
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
        }
    }

    /// Advances every table that has a round due, in simulated time.
    fn tick(&mut self, wall: Instant) {
        self.clock.advance(wall);
        let now = self.clock.now();
        self.lifecycle(now);
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

    /// Opens `n` new tables of a kind. They begin running immediately.
    pub fn open(&self, kind: Kind, n: usize) {
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
            let inst = Instance::new(id, kind, number, &mut sim);
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
