//! What the night has actually looked like.
//!
//! The floor already knows what is happening *now* — a snapshot says so.
//! What it could not say until this file existed is what happened over the
//! last ten minutes, or which game has been carrying the room, or whether
//! things are picking up or dying off. That is a different question, and
//! answering it naively is how a simulation ends up spending more time
//! reporting on itself than running.
//!
//! So the rule here is the same one the event feed and the lifetime records
//! follow, stated as strongly as it can be:
//!
//! > **Nothing is ever recomputed from history.** Every figure on every
//! > screen is a sum of counters that were incremented once, when the thing
//! > they count happened.
//!
//! Two structures do all of it:
//!
//! - [`Series`] — a ring of fixed-length time buckets. A range query adds
//!   up the handful of buckets it covers, so "the last ten minutes" costs
//!   the same as "the last minute", and both cost the same at 3am as they
//!   did at opening. The ring is bounded, so an all-night run does not grow
//!   a byte after the first hour.
//! - [`Games`] — one accumulator per kind of game, all of them the same
//!   shape. This is Phase 14's "common interface" and it is deliberately
//!   not a trait: every game is measured by the same six numbers, so a new
//!   table needs no analytics code at all, and there is nowhere for a
//!   per-game special case to hide.
//!
//! A few queries here are not on a screen yet — they are the shape the
//! persistence and tournament phases read, and are written and tested
//! alongside the thing they measure. Same justification as `games::cards`.
//!
//! What analytics must never do is influence anything. It is a set of
//! counters that the simulation writes to and screens read from; there is
//! no path from here back into a decision.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

/// One slice of the night.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    /// Chips staked and chips paid back.
    pub handle: i64,
    pub payouts: i64,
    pub bets: u64,
    pub rounds: u64,
    /// People sitting down and getting up.
    pub arrivals: u64,
    pub departures: u64,
    /// The biggest single win anybody took, in chips.
    pub biggest_win: i64,
}

impl Tally {
    /// The house's win over this slice, in chips.
    pub fn ggr(&self) -> i64 {
        self.handle - self.payouts
    }

    /// The house's win as a share of the handle, in hundredths of a
    /// percent. Zero handle is zero hold, not a divide by zero.
    pub fn hold(&self) -> i64 {
        if self.handle == 0 {
            return 0;
        }
        self.ggr() * 10_000 / self.handle
    }

    /// The average stake, which says more about who is in the room than
    /// the handle does.
    pub fn average_bet(&self) -> i64 {
        if self.bets == 0 {
            return 0;
        }
        self.handle / self.bets as i64
    }

    fn add(&mut self, other: &Tally) {
        self.handle += other.handle;
        self.payouts += other.payouts;
        self.bets += other.bets;
        self.rounds += other.rounds;
        self.arrivals += other.arrivals;
        self.departures += other.departures;
        self.biggest_win = self.biggest_win.max(other.biggest_win);
    }
}

/// A stretch of the night to ask about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    LastMinute,
    LastTenMinutes,
    LastHour,
    AllNight,
}

impl Span {
    pub const ALL: [Span; 4] = [Span::LastMinute, Span::LastTenMinutes, Span::LastHour, Span::AllNight];

    pub fn label(self) -> &'static str {
        match self {
            Span::LastMinute => "the last minute",
            Span::LastTenMinutes => "the last ten minutes",
            Span::LastHour => "the last hour",
            Span::AllNight => "all night",
        }
    }

    /// How far back this span reaches. `None` means the whole run, which
    /// is answered from a single running total rather than from the ring.
    pub fn behind(self) -> Option<Duration> {
        match self {
            Span::LastMinute => Some(Duration::from_secs(60)),
            Span::LastTenMinutes => Some(Duration::from_secs(600)),
            Span::LastHour => Some(Duration::from_secs(3_600)),
            Span::AllNight => None,
        }
    }

    pub fn next(self) -> Span {
        let at = Span::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Span::ALL[(at + 1) % Span::ALL.len()]
    }
}

/// How long one bucket covers, and how many are kept. Thirty seconds by
/// a hundred and twenty is an hour of history in a fixed 120 slots — which
/// is why an all-night run costs no more memory than a five-minute one.
const BUCKET: Duration = Duration::from_secs(30);
const BUCKETS: usize = 120;

/// A bounded, bucketed history of the night.
#[derive(Debug, Clone)]
pub struct Series {
    /// Closed buckets, oldest first, each stamped with when it started.
    ring: VecDeque<(Duration, Tally)>,
    /// The bucket still being filled.
    open_at: Duration,
    open: Tally,
    /// Everything since the doors opened, kept as one running total so
    /// `AllNight` never has to walk anything.
    total: Tally,
}

impl Default for Series {
    fn default() -> Series {
        Series::new()
    }
}

impl Series {
    pub fn new() -> Series {
        Series { ring: VecDeque::new(), open_at: Duration::ZERO, open: Tally::default(), total: Tally::default() }
    }

    /// Rolls the clock forward, closing buckets as their time passes.
    ///
    /// Called once per tick. A long stall closes at most `BUCKETS` of them,
    /// because closing ten thousand empty buckets to catch up would be
    /// exactly the sort of per-frame work this file exists to avoid.
    pub fn advance(&mut self, now: Duration) {
        let mut closed = 0;
        while now >= self.open_at + BUCKET && closed < BUCKETS {
            let finished = std::mem::take(&mut self.open);
            self.ring.push_back((self.open_at, finished));
            self.open_at += BUCKET;
            closed += 1;
            while self.ring.len() > BUCKETS {
                self.ring.pop_front();
            }
        }
        if now >= self.open_at + BUCKET {
            // Fell too far behind to be worth walking. Resync rather than
            // spending the tick on empty buckets nobody will read.
            self.open_at = now;
            self.ring.clear();
        }
    }

    /// Adds to the bucket currently being filled, and to the night's total.
    pub fn record(&mut self, t: &Tally) {
        self.open.add(t);
        self.total.add(t);
    }

    /// The tally over a span, ending now.
    ///
    /// Costs the number of buckets the span covers — at most `BUCKETS` —
    /// and `AllNight` costs nothing at all.
    pub fn range(&self, span: Span, now: Duration) -> Tally {
        let Some(behind) = span.behind() else { return self.total };
        let from = now.saturating_sub(behind);
        let mut out = self.open;
        for (at, t) in self.ring.iter().rev() {
            if *at + BUCKET <= from {
                break;
            }
            out.add(t);
        }
        out
    }

    /// Everything since the doors opened.
    pub fn total(&self) -> Tally {
        self.total
    }

    /// How many closed buckets are held. Bounded by `BUCKETS`, for ever.
    pub fn held(&self) -> usize {
        self.ring.len()
    }

    /// The handle in each of the last `n` buckets, oldest first — the
    /// shape of the night, for drawing a sparkline against.
    pub fn recent_handle(&self, n: usize) -> Vec<i64> {
        let mut out: Vec<i64> = self.ring.iter().rev().take(n).map(|(_, t)| t.handle).collect();
        out.reverse();
        out.push(self.open.handle);
        out
    }
}

/// Every game measured the same way.
///
/// Phase 14 asks for game-specific analytics "via a common interface". The
/// interface is this: six numbers, identical for every kind, keyed by the
/// same string the books and the stats file already use. A new table gets
/// analytics for free and cannot invent its own — which is the point.
#[derive(Debug, Clone, Default)]
pub struct Games {
    per_kind: BTreeMap<&'static str, Tally>,
}

impl Games {
    pub fn new() -> Games {
        Games::default()
    }

    pub fn record(&mut self, key: &'static str, t: &Tally) {
        self.per_kind.entry(key).or_default().add(t);
    }

    pub fn get(&self, key: &'static str) -> Option<&Tally> {
        self.per_kind.get(key)
    }

    pub fn len(&self) -> usize {
        self.per_kind.len()
    }

    pub fn is_empty(&self) -> bool {
        self.per_kind.is_empty()
    }

    /// Every game, by how much has gone through it — the order that says
    /// which tables are earning their floor space.
    pub fn by_handle(&self) -> Vec<(&'static str, Tally)> {
        let mut v: Vec<(&'static str, Tally)> = self.per_kind.iter().map(|(k, t)| (*k, *t)).collect();
        v.sort_by(|a, b| b.1.handle.cmp(&a.1.handle).then(a.0.cmp(b.0)));
        v
    }

    /// Every game, by what the house actually kept.
    pub fn by_win(&self) -> Vec<(&'static str, Tally)> {
        let mut v: Vec<(&'static str, Tally)> = self.per_kind.iter().map(|(k, t)| (*k, *t)).collect();
        v.sort_by(|a, b| b.1.ggr().cmp(&a.1.ggr()).then(a.0.cmp(b.0)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bet(handle: i64, payouts: i64) -> Tally {
        Tally { handle, payouts, bets: 1, rounds: 1, biggest_win: (payouts - handle).max(0), ..Tally::default() }
    }

    #[test]
    fn a_tally_of_nothing_reports_nothing_rather_than_dividing_by_zero() {
        let t = Tally::default();
        assert_eq!(t.ggr(), 0);
        assert_eq!(t.hold(), 0);
        assert_eq!(t.average_bet(), 0);
    }

    #[test]
    fn the_hold_is_the_win_as_a_share_of_the_handle() {
        let mut t = Tally::default();
        for _ in 0..100 {
            t.add(&bet(100, 96));
        }
        assert_eq!(t.handle, 10_000);
        assert_eq!(t.ggr(), 400);
        assert_eq!(t.hold(), 400, "4.00% should read as 400 hundredths");
        assert_eq!(t.average_bet(), 100);
    }

    #[test]
    fn the_biggest_win_is_a_high_water_mark_and_never_a_sum() {
        let mut t = Tally::default();
        t.add(&bet(10, 500));
        t.add(&bet(10, 60));
        t.add(&bet(10, 300));
        assert_eq!(t.biggest_win, 490, "the biggest hit is the biggest, not the total");
    }

    #[test]
    fn a_span_covers_what_it_says_it_covers() {
        assert_eq!(Span::LastMinute.behind(), Some(Duration::from_secs(60)));
        assert_eq!(Span::AllNight.behind(), None, "all night is answered from a running total");
        // The cycle comes back round.
        let mut s = Span::LastMinute;
        for _ in 0..Span::ALL.len() {
            s = s.next();
        }
        assert_eq!(s, Span::LastMinute);
    }

    #[test]
    fn a_range_query_costs_the_same_however_long_the_night_has_run() {
        // The property that matters: the ring is bounded, so an all-night
        // run holds no more than an hour's worth of buckets.
        let mut s = Series::new();
        let mut at = Duration::ZERO;
        for _ in 0..5_000 {
            at += BUCKET;
            s.advance(at);
            s.record(&bet(100, 90));
        }
        assert_eq!(s.held(), BUCKETS, "the ring grew to {} buckets", s.held());
        assert_eq!(s.total().bets, 5_000, "but the night's total remembers everything");
    }

    #[test]
    fn a_range_sums_only_the_buckets_it_covers() {
        let mut s = Series::new();
        let mut at = Duration::ZERO;
        // Ten minutes of one bet per bucket: twenty buckets of 30s.
        for _ in 0..20 {
            s.record(&bet(100, 50));
            at += BUCKET;
            s.advance(at);
        }
        let minute = s.range(Span::LastMinute, at);
        let ten = s.range(Span::LastTenMinutes, at);
        let all = s.range(Span::AllNight, at);
        assert_eq!(all.bets, 20);
        assert_eq!(ten.bets, 20, "everything happened inside the last ten minutes");
        assert!(minute.bets <= 3, "a minute should hold two or three buckets, not {}", minute.bets);
        assert!(minute.bets >= 1);
        assert!(minute.handle < ten.handle, "a shorter span cannot hold more");
    }

    #[test]
    fn what_happens_now_is_in_the_range_before_its_bucket_closes() {
        let mut s = Series::new();
        s.advance(Duration::from_secs(5));
        s.record(&bet(500, 0));
        // Nothing has closed yet, and the figure is still visible.
        assert_eq!(s.held(), 0);
        assert_eq!(s.range(Span::LastMinute, Duration::from_secs(5)).handle, 500);
        assert_eq!(s.range(Span::AllNight, Duration::from_secs(5)).handle, 500);
    }

    #[test]
    fn a_long_stall_resyncs_instead_of_walking_ten_thousand_empty_buckets() {
        let mut s = Series::new();
        s.record(&bet(100, 0));
        // Away for a week of casino time.
        s.advance(Duration::from_secs(600_000));
        assert!(s.held() <= BUCKETS);
        // The night's running total survives a resync; only the shape of
        // the recent past is lost, which is the honest outcome.
        assert_eq!(s.total().handle, 100);
        // And it keeps working afterwards.
        s.record(&bet(50, 0));
        assert_eq!(s.total().handle, 150);
        assert_eq!(s.range(Span::LastMinute, Duration::from_secs(600_000)).handle, 50);
    }

    #[test]
    fn the_shape_of_the_night_reads_oldest_first() {
        let mut s = Series::new();
        let mut at = Duration::ZERO;
        for i in 1..=6i64 {
            s.record(&bet(i * 100, 0));
            at += BUCKET;
            s.advance(at);
        }
        let shape = s.recent_handle(4);
        assert_eq!(shape.len(), 5, "four closed buckets plus the one still filling");
        assert_eq!(&shape[..4], &[300, 400, 500, 600], "oldest first");
        assert_eq!(*shape.last().unwrap(), 0, "nothing has happened in the open bucket yet");
    }

    #[test]
    fn every_game_is_measured_by_the_same_six_numbers() {
        // Phase 14's "common interface": there is no per-game code here,
        // and no way to add any.
        let mut g = Games::new();
        g.record("slots", &bet(100, 90));
        g.record("slots", &bet(100, 0));
        g.record("roulette", &bet(1_000, 1_200));
        assert_eq!(g.len(), 2);
        assert_eq!(g.get("slots").unwrap().handle, 200);
        assert_eq!(g.get("slots").unwrap().ggr(), 110);
        assert_eq!(g.get("roulette").unwrap().ggr(), -200);
        assert!(g.get("baccarat").is_none(), "a game nobody has played has no figures, not zeroes");
    }

    #[test]
    fn games_rank_by_what_they_are_said_to_rank_by() {
        let mut g = Games::new();
        g.record("slots", &bet(10_000, 9_900)); // huge handle, thin win
        g.record("keno", &bet(1_000, 100)); // small handle, fat win
        let by_handle = g.by_handle();
        assert_eq!(by_handle[0].0, "slots");
        let by_win = g.by_win();
        assert_eq!(by_win[0].0, "keno", "the biggest earner is not the busiest table");
    }

    #[test]
    fn a_game_that_has_never_been_played_does_not_appear() {
        let g = Games::new();
        assert!(g.is_empty());
        assert!(g.by_handle().is_empty());
        assert!(g.by_win().is_empty());
    }
}
