//! The casino's event bus.
//!
//! Before this file, the only way to learn anything about the floor was to
//! poll a snapshot and diff it against what you remembered. That works for
//! "how much is in the tray"; it cannot answer "what just happened", which
//! is what a feed, a notification, a spectator view and a leaderboard all
//! need.
//!
//! The shape is deliberately small:
//!
//! - [`Event`] — a closed enum. Anything the simulation wants to announce
//!   is a variant here, carrying the facts and no formatting.
//! - [`Weight`] — how much attention an event deserves. This is what makes
//!   "do not display every low-level simulation event" enforceable: the
//!   feed asks for `Notable` and up, a notification asks for `Major`, and
//!   the noisy per-round traffic is `Routine` and simply never surfaces.
//! - [`Feed`] — a bounded ring of [`Record`]s behind the floor lock, plus a
//!   monotonic sequence number so a reader can ask for "everything since I
//!   last looked" instead of re-reading the buffer.
//!
//! What this is **not** is a callback registry. Publishers push; readers
//! pull under the same lock they already take for a snapshot. That keeps
//! the one rule that matters — the UI never runs the simulation — true by
//! construction, because there is no path by which a reader's code can be
//! called from the simulation thread at all.
//!
//! Not every part of this module is called yet: it is the substrate the
//! later phases of the autonomous-simulation roadmap are built on, and the
//! pieces are written and tested together so that the phase that needs them
//! does not also have to invent them. Same justification as `games::cards`.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::time::Duration;

/// How much attention an event deserves.
///
/// The ordering is the whole point: filters are written as `>= Notable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Weight {
    /// Per-round traffic. Accumulated into statistics, never displayed.
    Routine,
    /// Worth a line in the feed: an arrival, a table opening, a good win.
    Notable,
    /// Worth interrupting somebody for: a jackpot, a whale at the door, a
    /// table taken for everything it had.
    Major,
}

/// Something that happened on the floor.
///
/// Variants carry facts, never sentences — the wording lives in `describe`
/// so a different surface can word it differently, and so a test can assert
/// on the event rather than on prose.
#[derive(Debug, Clone)]
pub enum Event {
    /// The doors opened.
    CasinoOpened { money: i64, chips: i64 },
    TableOpened { table: u32, name: String },
    TableClosed { table: u32, name: String, take: i64 },
    TablePaused { table: u32, name: String, paused: bool },

    /// A patron sat down, having bought in at the cage.
    Arrived { patron: u64, who: String, table: u32, table_name: String, chips: i64 },
    /// A patron got up. `net` is chips against what they bought in for;
    /// `cashed` is the cash the cage actually handed back for the chips
    /// they were holding. The second figure is on the event because only
    /// the bank knows the sell rate, and a reader must never have to guess
    /// at the house's spread.
    Left { patron: u64, who: String, table: u32, net: i64, reason: Departure, cashed: i64 },

    /// One settled bet. Almost always `Routine`; the weight is decided by
    /// the size of the swing, not by the variant.
    Settled { patron: u64, who: String, table: u32, table_name: String, bet: String, staked: i64, returned: i64 },

    /// A round finished. `Routine` by definition — this is the traffic the
    /// feed must not drown in.
    Round { table: u32, number: u64, pot: i64, paid: i64 },

    /// A patron crossed into a higher tier.
    Tier { patron: u64, who: String, tier: usize, name: &'static str },

    /// The room's taste in a game moved. Routine — this is background
    /// weather, not news.
    Mood { game: &'static str, appeal: i64 },

    /// Something started or finished going on in the building.
    Happening { what: super::happening::Happening, game: Option<&'static str>, on: bool },

    /// Something happened in a tournament.
    Tourney { name: String, what: String },

    /// The floor's random source was replaced. Announced because it
    /// changes how the rest of the night will go, and because an operator
    /// who does it deserves to see that it happened.
    Reseeded { was: u64, now: u64 },

    /// The building's running costs came due. Periodic, so it belongs on
    /// the feed — unlike a wager, there are only a handful an hour.
    Costs { overhead: i64, staffing: i64, tables: usize },
}

/// Why a patron got up. Kept as data rather than a string because Phase 13
/// wants to count these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Departure {
    /// Out of chips.
    Busted,
    /// Ahead, and disciplined enough to stop.
    Ahead,
    /// Down enough to have had enough.
    Down,
    /// Simply done for the night.
    Drifted,
    /// The table they were at closed under them.
    TableClosed,
}

impl Departure {
    pub fn label(self) -> &'static str {
        match self {
            Departure::Busted => "cleaned out",
            Departure::Ahead => "walked away ahead",
            Departure::Down => "had enough",
            Departure::Drifted => "called it a night",
            Departure::TableClosed => "table closed",
        }
    }
}

impl Event {
    /// A single line a person can read. No colour, no padding — the screen
    /// that shows it decides how it looks.
    pub fn describe(&self) -> String {
        match self {
            Event::CasinoOpened { money, chips } => {
                format!("Doors open — ${money} in the cage, {chips} chips in the tray")
            }
            Event::TableOpened { name, .. } => format!("{name} opened"),
            Event::TableClosed { name, take, .. } => {
                if *take >= 0 {
                    format!("{name} closed, {take} chips up")
                } else {
                    format!("{name} closed, {} chips down", -take)
                }
            }
            Event::TablePaused { name, paused, .. } => {
                if *paused { format!("{name} paused") } else { format!("{name} resumed") }
            }
            Event::Arrived { who, table_name, chips, .. } => {
                format!("{who} sat down at {table_name} with {chips}")
            }
            Event::Left { who, net, reason, .. } => {
                if *net >= 0 {
                    format!("{who} {} — up {net}", reason.label())
                } else {
                    format!("{who} {} — down {}", reason.label(), -net)
                }
            }
            Event::Settled { who, table_name, bet, staked, returned, .. } => {
                let swing = returned - staked;
                // A machine has no bets to name, so it gets no trailing
                // "on ", which reads as a sentence that lost its ending.
                let on = if bet.is_empty() { String::new() } else { format!(" on {bet}") };
                if swing >= 0 {
                    format!("{who} took {swing} off {table_name}{on}")
                } else {
                    format!("{who} dropped {} at {table_name}{on}", -swing)
                }
            }
            Event::Round { number, pot, paid, .. } => {
                format!("round {number}: {pot} staked, {paid} paid")
            }
            Event::Tier { who, name, .. } => format!("{who} is now a {name}"),
            Event::Mood { game, appeal } => {
                if *appeal >= super::demand::NEUTRAL {
                    format!("{game} is drawing a crowd")
                } else {
                    format!("{game} has gone quiet")
                }
            }
            Event::Happening { what, game, on } => {
                if *on {
                    what.blurb(*game)
                } else {
                    format!("{} — over", what.label())
                }
            }
            Event::Tourney { name, what } => format!("{name} {what}"),
            Event::Reseeded { was, now } => format!("the shoe was changed — seed {was} is now {now}"),
            Event::Costs { overhead, staffing, tables } => {
                format!("Costs: ${overhead} on the building, ${staffing} on staff for {tables} tables")
            }
        }
    }
}

/// An event, stamped with when it happened and where it sits in the stream.
#[derive(Debug, Clone)]
pub struct Record {
    /// Position in the stream. Strictly increasing, never reused, and the
    /// handle a reader keeps so it can ask for what it has not seen.
    pub seq: u64,
    /// Simulated time since the doors opened.
    pub at: Duration,
    pub weight: Weight,
    pub event: Event,
}

/// A bounded ring of recent events, plus running counts.
///
/// Bounded because the alternative is a `Vec` that grows for as long as the
/// casino runs, and this thing is going to run all night. Counts are kept
/// incrementally as events go in, because Phase 13's statistics must never
/// be recomputed by walking the buffer.
#[derive(Debug, Clone)]
pub struct Feed {
    ring: VecDeque<Record>,
    cap: usize,
    next_seq: u64,
    /// How many of each weight have ever been published — including the
    /// ones that have already fallen off the back of the ring.
    seen: [u64; 3],
    /// The sequence of the newest event judged `Major`, so a notification
    /// layer can tell "something big happened" without a scan.
    last_major: Option<u64>,
}

impl Feed {
    pub fn new(cap: usize) -> Feed {
        Feed { ring: VecDeque::new(), cap: cap.max(1), next_seq: 0, seen: [0; 3], last_major: None }
    }

    /// Publishes an event. Returns its sequence number.
    pub fn push(&mut self, at: Duration, weight: Weight, event: Event) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.seen[weight as usize] += 1;
        if weight == Weight::Major {
            self.last_major = Some(seq);
        }
        // Routine traffic is counted but not kept: it exists so statistics
        // can accumulate, and keeping it would evict everything worth
        // reading within a few seconds of a busy floor.
        if weight > Weight::Routine {
            self.ring.push_back(Record { seq, at, weight, event });
            while self.ring.len() > self.cap {
                self.ring.pop_front();
            }
        }
        seq
    }

    /// The most recent `n` records of at least `min` weight, oldest first.
    pub fn recent(&self, n: usize, min: Weight) -> Vec<Record> {
        let mut out: Vec<Record> = self.ring.iter().rev().filter(|r| r.weight >= min).take(n).cloned().collect();
        out.reverse();
        out
    }

    /// Everything published after `seq` of at least `min` weight, oldest
    /// first. A reader passes back the `seq` of the last record it saw.
    ///
    /// If the reader has been away long enough for its position to have
    /// fallen off the back of the ring, it gets whatever is still held —
    /// dropped events are gone, which is what "bounded" means, and the
    /// counts in `seen` are there precisely so nothing that mattered is
    /// only knowable from the buffer.
    pub fn since(&self, seq: u64, min: Weight) -> Vec<Record> {
        self.ring.iter().filter(|r| r.seq > seq && r.weight >= min).cloned().collect()
    }

    /// The sequence a fresh reader should start from to see only what
    /// happens next.
    pub fn cursor(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    /// Total events ever published at each weight, ring evictions included.
    pub fn seen(&self, weight: Weight) -> u64 {
        self.seen[weight as usize]
    }

    pub fn published(&self) -> u64 {
        self.next_seq
    }

    pub fn last_major(&self) -> Option<u64> {
        self.last_major
    }

    pub fn held(&self) -> usize {
        self.ring.len()
    }
}

/// Decides how loudly a settled bet should be announced.
///
/// Kept here, next to the weights, rather than at the call site — the call
/// site has a `&Config` and passes the thresholds in, so no number is
/// written down twice.
pub fn settlement_weight(staked: i64, returned: i64, big: i64, huge: i64, jackpot_multiple: i64) -> Weight {
    let swing = returned - staked;
    if swing <= 0 {
        // A loss is only ever traffic; a patron busting out is announced by
        // their departure, which carries the fact that matters.
        return Weight::Routine;
    }
    let multiple = if staked > 0 { returned / staked } else { 0 };
    if swing >= huge || multiple >= jackpot_multiple {
        Weight::Major
    } else if swing >= big {
        Weight::Notable
    } else {
        Weight::Routine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(n: u64) -> Event {
        Event::TableOpened { table: n as u32, name: format!("Table #{n}") }
    }

    #[test]
    fn weights_compare_the_way_filters_assume() {
        assert!(Weight::Major > Weight::Notable);
        assert!(Weight::Notable > Weight::Routine);
    }

    #[test]
    fn routine_traffic_is_counted_but_never_kept() {
        let mut f = Feed::new(8);
        for i in 0..1_000 {
            f.push(Duration::ZERO, Weight::Routine, Event::Round { table: 1, number: i, pot: 10, paid: 9 });
        }
        assert_eq!(f.seen(Weight::Routine), 1_000);
        assert_eq!(f.held(), 0, "a busy floor must not evict the readable feed with its own noise");
        assert!(f.recent(50, Weight::Routine).is_empty());
    }

    #[test]
    fn the_ring_is_bounded_and_keeps_the_newest() {
        let mut f = Feed::new(4);
        for i in 0..20 {
            f.push(Duration::ZERO, Weight::Notable, ev(i));
        }
        assert_eq!(f.held(), 4);
        let recent = f.recent(10, Weight::Routine);
        assert_eq!(recent.len(), 4);
        assert_eq!(recent.first().unwrap().seq, 16, "oldest first");
        assert_eq!(recent.last().unwrap().seq, 19, "newest last");
    }

    #[test]
    fn a_reader_can_ask_for_only_what_it_has_not_seen() {
        let mut f = Feed::new(64);
        f.push(Duration::ZERO, Weight::Notable, ev(0));
        let cursor = f.cursor();
        f.push(Duration::ZERO, Weight::Notable, ev(1));
        f.push(Duration::ZERO, Weight::Major, ev(2));
        let fresh = f.since(cursor, Weight::Routine);
        assert_eq!(fresh.len(), 2);
        assert_eq!(fresh[0].seq, 1);
        // ...and can narrow to the things worth interrupting for.
        let big = f.since(cursor, Weight::Major);
        assert_eq!(big.len(), 1);
        assert_eq!(big[0].seq, 2);
    }

    #[test]
    fn sequence_numbers_never_repeat_even_across_evictions() {
        let mut f = Feed::new(2);
        let mut seqs = Vec::new();
        for i in 0..50 {
            seqs.push(f.push(Duration::ZERO, Weight::Notable, ev(i)));
        }
        let mut sorted = seqs.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), seqs.len());
        assert_eq!(seqs, sorted, "and they only ever climb");
        assert_eq!(f.published(), 50);
    }

    #[test]
    fn the_newest_major_is_findable_without_a_scan() {
        let mut f = Feed::new(4);
        assert_eq!(f.last_major(), None);
        f.push(Duration::ZERO, Weight::Notable, ev(0));
        assert_eq!(f.last_major(), None);
        let seq = f.push(Duration::ZERO, Weight::Major, ev(1));
        for i in 2..20 {
            f.push(Duration::ZERO, Weight::Notable, ev(i));
        }
        assert_eq!(f.last_major(), Some(seq), "even after it has been evicted from the ring");
    }

    #[test]
    fn a_settlement_is_announced_by_its_size_not_its_kind() {
        // big = 1_000, huge = 10_000, jackpot at 25x.
        let w = |s, r| settlement_weight(s, r, 1_000, 10_000, 25);
        assert_eq!(w(100, 0), Weight::Routine, "a loss is traffic");
        assert_eq!(w(100, 150), Weight::Routine, "so is a small win");
        assert_eq!(w(1_000, 2_500), Weight::Notable);
        assert_eq!(w(5_000, 20_000), Weight::Major, "a big enough swing");
        assert_eq!(w(10, 300), Weight::Major, "a 30x on ten chips is a jackpot, small though it is");
        assert_eq!(w(10, 200), Weight::Routine, "20x is not");
    }

    #[test]
    fn a_table_with_no_bets_to_name_still_reads_as_a_sentence() {
        // A machine has one thing you can do at it, so its bet name is
        // empty — and the line must not trail off into "... on ".
        let machine = Event::Settled {
            patron: 1,
            who: "Ada".into(),
            table: 1,
            table_name: "Slots #3".into(),
            bet: String::new(),
            staked: 100,
            returned: 900,
        };
        assert_eq!(machine.describe(), "Ada took 800 off Slots #3");
        let board = Event::Settled {
            patron: 1,
            who: "Ada".into(),
            table: 1,
            table_name: "Roulette #1".into(),
            bet: "straight up on 17 (35:1)".into(),
            staked: 100,
            returned: 3_600,
        };
        assert_eq!(board.describe(), "Ada took 3500 off Roulette #1 on straight up on 17 (35:1)");
        assert!(!machine.describe().ends_with(' '));
    }

    #[test]
    fn every_event_words_itself_without_panicking() {
        let all = [
            Event::CasinoOpened { money: 1, chips: 2 },
            Event::TableOpened { table: 1, name: "T".into() },
            Event::TableClosed { table: 1, name: "T".into(), take: -5 },
            Event::TablePaused { table: 1, name: "T".into(), paused: true },
            Event::Arrived { patron: 1, who: "A".into(), table: 1, table_name: "T".into(), chips: 10 },
            Event::Left { patron: 1, who: "A".into(), table: 1, net: -3, reason: Departure::Busted, cashed: 0 },
            Event::Settled {
                patron: 1,
                who: "A".into(),
                table: 1,
                table_name: "T".into(),
                bet: "b".into(),
                staked: 5,
                returned: 9,
            },
            Event::Round { table: 1, number: 3, pot: 9, paid: 8 },
            Event::Tier { patron: 1, who: "A".into(), tier: 2, name: "high roller" },
            Event::Costs { overhead: 400, staffing: 90, tables: 2 },
            Event::Mood { game: "slots", appeal: 1_200 },
            Event::Tourney { name: "Blackjack Tournament #1".into(), what: "is under way".into() },
            Event::Happening { what: crate::casino::happening::Happening::Rush, game: None, on: true },
            Event::Happening { what: crate::casino::happening::Happening::Craze, game: Some("keno"), on: false },
        ];
        for e in all {
            assert!(!e.describe().is_empty());
        }
    }
}
