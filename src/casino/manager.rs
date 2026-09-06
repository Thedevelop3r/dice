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
use super::instance::{Instance, Kind, RoundLog};
use crate::rng::Rng;
use crate::ui::Badge;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often the simulation thread wakes. Tables are paced by their own
/// clocks, so this only bounds how *promptly* a due round is played, not
/// how often rounds happen.
const TICK: Duration = Duration::from_millis(40);

/// A ceiling on how much catch-up a single tick will do. If the machine is
/// heavily loaded, or the process was suspended, tables resync to the clock
/// rather than trying to replay an hour of missed rounds at once.
const MAX_CATCH_UP: u32 = 4;

/// Everything the running casino owns. Only ever touched behind the mutex.
struct Floor {
    bank: Bank,
    instances: Vec<Instance>,
    next_id: u32,
    /// How many of each kind have ever been opened, so table numbers keep
    /// climbing rather than being reused.
    opened: BTreeMap<&'static str, u32>,
    rng: Rng,
    speed: u32,
    started: Instant,
}

impl Floor {
    /// Advances every table that has a round due.
    fn tick(&mut self, now: Instant) {
        let speed = self.speed.max(1);
        for i in 0..self.instances.len() {
            if self.instances[i].paused {
                // A paused table must not accrue a backlog, or resuming it
                // would fire off a burst of rounds at once.
                self.instances[i].next_at = now + self.instances[i].kind.pace();
                continue;
            }
            let mut played = 0;
            while self.instances[i].next_at <= now && played < MAX_CATCH_UP * speed {
                // `instances`, `rng` and `bank` are distinct fields, so all
                // three can be borrowed at once.
                self.instances[i].play_round(&mut self.rng, &mut self.bank);
                played += 1;
            }
            if self.instances[i].next_at + Duration::from_secs(5) < now {
                // Fell too far behind to be worth catching up on.
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
    pub open_for: Duration,
    pub seen: u32,
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
    pub speed: u32,
    pub running_for: Duration,
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
            speed: 1,
            started: Instant::now(),
        }));
        let running = Arc::new(AtomicBool::new(true));

        let thread_floor = Arc::clone(&floor);
        let thread_running = Arc::clone(&running);
        badge.set(money, chips);
        let thread = std::thread::spawn(move || {
            while thread_running.load(Ordering::Relaxed) {
                {
                    let now = Instant::now();
                    if let Ok(mut f) = thread_floor.lock() {
                        f.tick(now);
                        badge.set(f.bank.money(), f.bank.chips());
                    }
                }
                std::thread::sleep(TICK);
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
            let Floor { rng, bank, instances, .. } = &mut *f;
            instances.push(Instance::new(id, kind, number, rng, bank));
        }
    }

    /// Closes one table, cashing its patrons out at the cage on the way.
    pub fn close(&self, id: u32) {
        let Ok(mut f) = self.floor.lock() else { return };
        if let Some(i) = f.instances.iter().position(|t| t.id == id) {
            let inst = f.instances.remove(i);
            for p in inst.patrons {
                f.bank.cash_out(p.chips);
            }
        }
    }

    pub fn toggle_pause(&self, id: u32) {
        let Ok(mut f) = self.floor.lock() else { return };
        if let Some(t) = f.instances.iter_mut().find(|t| t.id == id) {
            t.paused = !t.paused;
        }
    }

    /// 1x, 2x or 4x — how many rounds a table may catch up per tick.
    pub fn cycle_speed(&self) {
        let Ok(mut f) = self.floor.lock() else { return };
        f.speed = match f.speed {
            1 => 2,
            2 => 4,
            _ => 1,
        };
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
                speed: 1,
                running_for: Duration::ZERO,
                running: false,
            };
        };
        let now = Instant::now();
        FloorView {
            money: f.bank.money(),
            chips: f.bank.chips(),
            table_profit: f.bank.table_profit(),
            cage_profit: f.bank.cage_profit(),
            totals: f.bank.totals(),
            rounds: f.bank.rounds(),
            bets: f.bank.bets(),
            tables: f.instances.iter().map(|t| view_of(t, now, false)).collect(),
            by_table: f.bank.by_table(),
            speed: f.speed,
            running_for: now.duration_since(f.started),
            running: self.is_running(),
        }
    }

    /// One table in full, including its patrons and recent rounds. This is
    /// what "hooking into" a running game actually is: a copy of live
    /// state, never a restart of it.
    pub fn table(&self, id: u32) -> Option<TableView> {
        let f = self.floor.lock().ok()?;
        let now = Instant::now();
        f.instances.iter().find(|t| t.id == id).map(|t| view_of(t, now, true))
    }

    /// The ids currently on the floor, in the order they were opened —
    /// which is the order the UI cycles through.
    pub fn ids(&self) -> Vec<u32> {
        self.floor.lock().map(|f| f.instances.iter().map(|t| t.id).collect()).unwrap_or_default()
    }
}

fn view_of(t: &Instance, now: Instant, deep: bool) -> TableView {
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
        patrons: if deep { t.patrons.clone() } else { Vec::new() },
        open_for: now.saturating_duration_since(t.opened),
        seen: t.seen,
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
        // Seating patrons already moves the books — chips leave the tray and
        // cash enters the cage — before a single bet is struck.
        assert_ne!(m.balances(), opening, "buying four tables in never touched the cage");
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
}
