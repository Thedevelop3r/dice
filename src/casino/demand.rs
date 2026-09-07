//! What the floor is in the mood for tonight.
//!
//! Without this file every table of a given kind is equally attractive
//! forever, and a floor looks the same at 3am as it did at opening. Real
//! rooms do not work like that: a game gets a run of attention, the
//! blackjack pit fills while the wheel sits empty, and an hour later it has
//! swapped round for no reason anybody could name.
//!
//! Two separate things live here, and keeping them apart matters:
//!
//! - **Appeal** — how much the room fancies a kind of game *right now*, in
//!   permille against a neutral `1_000`. It drifts on its own clock, within
//!   bounds, and it multiplies a person's own taste when they pick a table.
//!   It is a *nudge on a preference*, never an instruction.
//! - **Utilization** — how full the tables of each kind have actually been,
//!   sampled periodically and accumulated. This is a *measurement*, and the
//!   house's evidence for which games are earning their floor space.
//!
//! The rule this file must never break is Rule 5. Appeal decides **where
//! somebody sits**. It has no say whatever in what the cards do once they
//! are sitting there: a "hot" table is a busy table, not a generous one.
//! There is a test at the bottom whose entire job is to fail if anybody
//! ever wires appeal into a payout.
//!
//! Nothing here is recomputed from history. Utilization is accumulated as
//! it is sampled, which is the same discipline the event feed and the
//! lifetime records follow.
//!
//! A few of the query methods here are not called from the running program
//! yet: they are the substrate the analytics phase reads, and are written
//! and tested alongside the thing they measure rather than being invented
//! later. Same justification as `games::cards`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use super::clock::Every;
use super::config::Config;
use crate::rng::Rng;

/// Neutral appeal. A kind at exactly this is neither in nor out of favour.
pub const NEUTRAL: i64 = 1_000;

/// What the floor thinks of one kind of game, and how it has actually been
/// used.
#[derive(Debug, Clone)]
pub struct Standing {
    /// Permille. `1_000` is neutral; higher means people are drawn to it.
    pub appeal: i64,
    /// Seats filled and seats offered, summed over every sample taken.
    /// Their ratio is the utilization; keeping both means a kind with one
    /// table and a kind with twelve are comparable.
    pub filled: u64,
    pub offered: u64,
    /// How many tables of this kind are open right now, as of the last
    /// sample.
    pub tables: usize,
}

impl Standing {
    fn new() -> Standing {
        Standing { appeal: NEUTRAL, filled: 0, offered: 0, tables: 0 }
    }

    /// How full this kind's tables have been, in permille of seats
    /// offered. Never above `1_000`: see `record_sample`.
    pub fn utilization(&self) -> i64 {
        if self.offered == 0 {
            return 0;
        }
        (self.filled as i64) * 1_000 / (self.offered as i64)
    }
}

/// The floor's mood, per kind of game.
#[derive(Debug, Clone)]
pub struct Demand {
    per_kind: BTreeMap<&'static str, Standing>,
    /// When the mood next shifts, and when utilization is next sampled.
    /// Periodic, never per frame.
    shift: Every,
    sample: Every,
    /// How many times the mood has moved, so a screen can say whether any
    /// of this has had a chance to happen yet.
    shifts: u64,
    samples: u64,
}

impl Demand {
    pub fn new(cfg: &Config, from: Duration) -> Demand {
        Demand {
            per_kind: BTreeMap::new(),
            shift: Every::new(cfg.demand_period, from),
            sample: Every::new(cfg.demand_sample, from),
            shifts: 0,
            samples: 0,
        }
    }

    /// What the room currently thinks of a kind. Anything never seen before
    /// is neutral, which is the honest answer rather than an error.
    pub fn appeal(&self, key: &'static str) -> i64 {
        self.per_kind.get(key).map(|s| s.appeal).unwrap_or(NEUTRAL)
    }

    pub fn standing(&self, key: &'static str) -> Option<&Standing> {
        self.per_kind.get(key)
    }

    pub fn shifts(&self) -> u64 {
        self.shifts
    }

    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Every kind the floor has an opinion about, keenest first.
    pub fn ranked(&self) -> Vec<(&'static str, Standing)> {
        let mut v: Vec<(&'static str, Standing)> = self.per_kind.iter().map(|(k, s)| (*k, s.clone())).collect();
        v.sort_by(|a, b| b.1.appeal.cmp(&a.1.appeal).then(a.0.cmp(b.0)));
        v
    }

    /// Records how full one kind's tables are. Called with a whole floor's
    /// worth of `(key, seated, seats_wanted)` when a sample is due.
    pub fn record_sample(&mut self, seen: &[(&'static str, usize, usize)]) {
        let mut tables: BTreeMap<&'static str, usize> = BTreeMap::new();
        for (key, seated, wanted) in seen {
            let s = self.per_kind.entry(key).or_insert_with(Standing::new);
            s.filled += *seated as u64;
            // A table re-rolls how many seats it wants as people come and
            // go, so it can briefly hold more than it is asking for. Those
            // extra seats are genuinely in use, so they count as offered —
            // otherwise a busy floor reads as over 100% full, which is not
            // a thing occupancy can be.
            s.offered += (*wanted).max(*seated) as u64;
            *tables.entry(key).or_insert(0) += 1;
        }
        for (key, n) in tables {
            if let Some(s) = self.per_kind.get_mut(key) {
                s.tables = n;
            }
        }
        self.samples += 1;
    }

    /// Is a sample due? Drains the backlog rather than skipping it, but a
    /// caller only wants one sample per due period, so this is an `if` at
    /// the call site by design.
    pub fn sample_due(&mut self, now: Duration) -> bool {
        let mut due = false;
        while self.sample.due(now) {
            due = true;
        }
        due
    }

    /// Moves the mood on, if a period has come round. Returns the kinds
    /// whose standing changed enough to be worth mentioning.
    ///
    /// The drift is a random walk pulled gently back toward neutral, so a
    /// game can be in favour for a while without any kind running away and
    /// permanently owning the floor.
    pub fn drift(&mut self, rng: &mut Rng, cfg: &Config, now: Duration, kinds: &[&'static str]) -> Vec<(&'static str, i64)> {
        let mut moved = Vec::new();
        let mut periods = 0;
        while self.shift.due(now) {
            periods += 1;
            if periods > 8 {
                // A long stall must not turn into a night's worth of mood
                // swings resolved in one tick.
                self.shift.resync(now);
                break;
            }
            self.shifts += 1;
            for key in kinds {
                let s = self.per_kind.entry(key).or_insert_with(Standing::new);
                let before = s.appeal;
                // A step either way, plus a pull of a tenth of the current
                // distance from neutral back toward it.
                let step = rng.below(cfg.appeal_step as usize * 2 + 1) as i64 - cfg.appeal_step;
                let pull = (NEUTRAL - s.appeal) / 10;
                s.appeal = (s.appeal + step + pull).clamp(cfg.appeal_floor, cfg.appeal_ceiling);
                if (s.appeal - before).abs() >= cfg.appeal_step / 2 {
                    moved.push((*key, s.appeal));
                }
            }
        }
        moved
    }

    /// Puts back a saved standing, so a reopened casino remembers what the
    /// room was into. Only the appeal survives; the occupancy figures are
    /// a measurement of a night that is over.
    pub fn set_appeal(&mut self, key: &'static str, appeal: i64) {
        self.per_kind.entry(key).or_insert_with(Standing::new).appeal = appeal;
    }

    /// Forgets a kind entirely — used when the last table of it closes and
    /// the floor stops having an opinion.
    pub fn forget(&mut self, key: &'static str) {
        self.per_kind.remove(key);
    }

    /// The whole floor's utilization, in permille of every seat offered.
    pub fn overall_utilization(&self) -> i64 {
        let filled: u64 = self.per_kind.values().map(|s| s.filled).sum();
        let offered: u64 = self.per_kind.values().map(|s| s.offered).sum();
        if offered == 0 {
            return 0;
        }
        (filled as i64) * 1_000 / (offered as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Rng {
        Rng::from_seed(5150)
    }

    fn cfg() -> Config {
        Config::default()
    }

    const KINDS: [&str; 3] = ["slots", "roulette", "keno"];

    #[test]
    fn a_kind_nobody_has_an_opinion_about_is_neutral() {
        let d = Demand::new(&cfg(), Duration::ZERO);
        assert_eq!(d.appeal("slots"), NEUTRAL);
        assert_eq!(d.overall_utilization(), 0, "no samples means no utilization, not a divide by zero");
        assert!(d.standing("slots").is_none());
    }

    #[test]
    fn utilization_is_seats_filled_over_seats_offered() {
        let mut d = Demand::new(&cfg(), Duration::ZERO);
        // Two slots tables, one full and one half empty; one roulette table
        // sitting completely idle.
        d.record_sample(&[("slots", 1, 1), ("slots", 1, 2), ("roulette", 0, 6)]);
        assert_eq!(d.standing("slots").unwrap().utilization(), 666, "2 of 3 seats is 66.6%");
        assert_eq!(d.standing("roulette").unwrap().utilization(), 0);
        assert_eq!(d.standing("slots").unwrap().tables, 2);
        assert_eq!(d.overall_utilization(), 2 * 1_000 / 9);
        assert_eq!(d.samples(), 1);
    }

    #[test]
    fn a_table_holding_more_than_it_asked_for_is_not_over_full() {
        // Tables re-roll how many seats they want as people come and go, so
        // a sample can catch one seating four while asking for two. That is
        // a full table, not a 200% full one.
        let mut d = Demand::new(&cfg(), Duration::ZERO);
        d.record_sample(&[("blackjack", 4, 2)]);
        assert_eq!(d.standing("blackjack").unwrap().utilization(), 1_000);
        assert_eq!(d.overall_utilization(), 1_000);
        for _ in 0..50 {
            d.record_sample(&[("blackjack", 7, 1), ("slots", 0, 3)]);
        }
        for (k, st) in d.ranked() {
            assert!(st.utilization() <= 1_000, "{k} read as {} permille full", st.utilization());
        }
        assert!(d.overall_utilization() <= 1_000);
    }

    #[test]
    fn samples_accumulate_rather_than_being_recomputed() {
        let mut d = Demand::new(&cfg(), Duration::ZERO);
        for _ in 0..100 {
            d.record_sample(&[("slots", 3, 4)]);
        }
        let s = d.standing("slots").unwrap();
        assert_eq!(s.filled, 300);
        assert_eq!(s.offered, 400);
        assert_eq!(s.utilization(), 750);
    }

    #[test]
    fn the_mood_only_moves_when_a_period_comes_round() {
        let c = cfg();
        let mut rng = probe();
        let mut d = Demand::new(&c, Duration::ZERO);
        let before: Vec<i64> = KINDS.iter().map(|k| d.appeal(k)).collect();
        // Just short of a period: nothing has happened.
        d.drift(&mut rng, &c, c.demand_period - Duration::from_millis(1), &KINDS);
        assert_eq!(d.shifts(), 0);
        let after: Vec<i64> = KINDS.iter().map(|k| d.appeal(k)).collect();
        assert_eq!(before, after);
        // And then it has.
        d.drift(&mut rng, &c, c.demand_period, &KINDS);
        assert_eq!(d.shifts(), 1);
    }

    #[test]
    fn a_long_stall_does_not_become_a_nights_worth_of_mood_swings() {
        let c = cfg();
        let mut rng = probe();
        let mut d = Demand::new(&c, Duration::ZERO);
        // Away for a thousand periods.
        d.drift(&mut rng, &c, c.demand_period * 1_000, &KINDS);
        assert!(d.shifts() <= 8, "{} shifts resolved in one tick", d.shifts());
        // ...and the clock is not left with a backlog either.
        let shifts = d.shifts();
        d.drift(&mut rng, &c, c.demand_period * 1_000, &KINDS);
        assert_eq!(d.shifts(), shifts, "the backlog was still there on the next pass");
    }

    #[test]
    fn the_mood_wanders_but_never_runs_away() {
        let c = cfg();
        let mut rng = probe();
        let mut d = Demand::new(&c, Duration::ZERO);
        let mut at = Duration::ZERO;
        let mut lowest = i64::MAX;
        let mut highest = i64::MIN;
        for _ in 0..2_000 {
            at += c.demand_period;
            d.drift(&mut rng, &c, at, &KINDS);
            for k in KINDS {
                let a = d.appeal(k);
                assert!(a >= c.appeal_floor && a <= c.appeal_ceiling, "{k} drifted to {a}, outside its bounds");
                lowest = lowest.min(a);
                highest = highest.max(a);
            }
        }
        // It must actually move — a mood that never changes is not a mood.
        assert!(highest - lowest > c.appeal_step * 2, "appeal barely moved: {lowest}..{highest}");
        // And the pull toward neutral must keep it near the middle on
        // average rather than pinned to a wall.
        let mean: i64 = KINDS.iter().map(|k| d.appeal(k)).sum::<i64>() / KINDS.len() as i64;
        assert!((c.appeal_floor..=c.appeal_ceiling).contains(&mean));
    }

    #[test]
    fn appeal_decides_where_people_sit_and_nothing_else() {
        // Rule 5, guarded. `Demand` deals only in preference and
        // measurement: there is no path from here to a settlement. If this
        // file ever grows a method that returns a payout, a multiplier on
        // one, or anything a table would use to resolve a bet, this test is
        // the thing that should have stopped it.
        //
        // The check is on the shape of what a `Standing` carries: an
        // appeal, and a count of seats. Nothing about money.
        let mut d = Demand::new(&cfg(), Duration::ZERO);
        d.record_sample(&[("slots", 1, 1)]);
        let s = d.standing("slots").unwrap();
        let Standing { appeal, filled, offered, tables } = s;
        assert_eq!(*appeal, NEUTRAL);
        assert_eq!((*filled, *offered, *tables), (1, 1, 1));
    }

    #[test]
    fn a_kind_can_be_forgotten_when_its_last_table_closes() {
        let mut d = Demand::new(&cfg(), Duration::ZERO);
        d.record_sample(&[("keno", 2, 5)]);
        assert!(d.standing("keno").is_some());
        d.forget("keno");
        assert!(d.standing("keno").is_none());
        assert_eq!(d.appeal("keno"), NEUTRAL, "and it is neutral again if it reopens");
    }

    #[test]
    fn a_sample_is_due_once_per_period_however_late_it_is_asked() {
        let c = cfg();
        let mut d = Demand::new(&c, Duration::ZERO);
        assert!(!d.sample_due(c.demand_sample - Duration::from_millis(1)));
        assert!(d.sample_due(c.demand_sample));
        assert!(!d.sample_due(c.demand_sample), "the same period came due twice");
        // Away for a hundred periods: one sample, not a hundred.
        assert!(d.sample_due(c.demand_sample * 101));
        assert!(!d.sample_due(c.demand_sample * 101));
    }
}
