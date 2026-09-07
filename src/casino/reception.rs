//! The front desk and the cage.
//!
//! A casino is not only tables. It is a door people come through, a
//! counter where cash becomes chips, and the same counter where chips
//! become cash again on the way out. All of that already *happens* — the
//! floor admits people, [`Bank::buy_in`](super::bank::Bank::buy_in) issues
//! their chips, [`Bank::cash_out`](super::bank::Bank::cash_out) takes them
//! back — but until now none of it was written down anywhere a person
//! could read it.
//!
//! This module is that ledger, and the shape of it matters:
//!
//! - **It owns no money.** Every figure here is a *record* of a movement
//!   the bank already made. The bank stays the only place a balance
//!   changes; ask it for the totals and this for the story.
//! - **It is fed by the event bus, not by scanning.** Once a tick it reads
//!   whatever the feed has published since it last looked and folds those
//!   records in. Nothing here walks the roster, and nothing recomputes a
//!   count that was already counted when the thing happened.
//! - **It is bounded.** The activity list is a ring; the counters are
//!   integers. A casino that runs all night costs the same as one that has
//!   been open a minute.
//!
//! The one thing it does convert is chips to dollars, and it does that
//! with [`crate::economy`]'s rates rather than a number written down here
//! — the cage's spread lives in exactly one place.

use std::collections::VecDeque;
use std::time::Duration;

use crate::economy::{CHIPS_PER_DOLLAR, CHIPS_PER_DOLLAR_SELL};

use super::event::{Departure, Event, Feed, Record, Weight};

/// What sort of thing happened at the desk.
///
/// Kept as data rather than a sentence for the same reason [`Event`] is:
/// a screen decides the wording, and a test can assert on the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cage {
    /// Somebody walked in and bought chips at the counter.
    BuyIn,
    /// Somebody brought chips back and left with cash.
    CashOut,
    /// A buy-in large enough to be worth a look.
    BigBuyIn,
    /// A cash-out large enough to be worth a look.
    BigCashOut,
    /// Somebody crossed into a higher standing while in the building.
    Standing,
}

impl Cage {
    pub fn label(self) -> &'static str {
        match self {
            Cage::BuyIn => "bought chips",
            Cage::CashOut => "cashed out",
            Cage::BigBuyIn => "high roller in",
            Cage::BigCashOut => "large cash-out",
            Cage::Standing => "new standing",
        }
    }

    /// Whether this is the sort of line worth picking out on a screen.
    pub fn notable(self) -> bool {
        matches!(self, Cage::BigBuyIn | Cage::BigCashOut | Cage::Standing)
    }
}

/// One line of the desk's day book.
#[derive(Debug, Clone)]
pub struct Visit {
    pub at: Duration,
    pub patron: u64,
    pub who: String,
    pub what: Cage,
    /// Chips over the counter, whichever way they went.
    pub chips: i64,
    /// Cash over the counter, whichever way it went.
    pub cash: i64,
    /// Where they went, or why they left — whatever the event knew.
    pub note: String,
}

/// How much of the day book is kept. Deliberately small: this is a strip
/// on a screen, not an archive, and the archive already exists in the feed.
pub const KEPT: usize = 64;

/// What has come through the door, and what has crossed the counter.
#[derive(Debug, Clone)]
pub struct Reception {
    book: VecDeque<Visit>,
    cap: usize,
    /// Where this desk has read up to on the feed, or `None` if it has
    /// never read. The whole reason it never has to scan anything.
    ///
    /// It has to be an option: sequence numbers start at zero, and
    /// [`Feed::since`] means *after* the sequence given, so "I have seen
    /// nothing" and "I have seen record zero" are different facts that a
    /// bare `0` could not tell apart. The very first event a casino
    /// publishes would be the one lost.
    seen: Option<u64>,
    arrivals: u64,
    departures: u64,
    /// Chips issued and taken back, and the cash that crossed with them.
    /// Counted as the events go past, never summed from the book.
    chips_out: i64,
    chips_in: i64,
    cash_in: i64,
    cash_out: i64,
    /// The largest single movement either way, and who made it.
    biggest_buy: (i64, String),
    biggest_cash: (i64, String),
}

impl Default for Reception {
    fn default() -> Reception {
        Reception::new(KEPT)
    }
}

impl Reception {
    pub fn new(cap: usize) -> Reception {
        Reception {
            book: VecDeque::new(),
            cap: cap.max(1),
            seen: None,
            arrivals: 0,
            departures: 0,
            chips_out: 0,
            chips_in: 0,
            cash_in: 0,
            cash_out: 0,
            biggest_buy: (0, String::new()),
            biggest_cash: (0, String::new()),
        }
    }

    /// Folds in everything published since the last look.
    ///
    /// `fresh` is what the caller got from the feed; `cursor` is where the
    /// feed has reached. Taking both means this can never re-read a record
    /// and can never miss one — and it costs whatever happened in one
    /// tick, not whatever has happened all night.
    pub fn absorb(&mut self, fresh: &[Record], cursor: u64) {
        for r in fresh {
            match &r.event {
                Event::Arrived { patron, who, chips, table_name, .. } => {
                    // Chips are issued at the buy rate, so the cash that
                    // crossed the counter is exactly recoverable. The rate
                    // lives in `economy`; this only applies it.
                    let cash = *chips / CHIPS_PER_DOLLAR;
                    self.arrivals += 1;
                    self.chips_out += *chips;
                    self.cash_in += cash;
                    // "Big" means bigger than anything else tonight, which
                    // is a fact about the night rather than a threshold
                    // this module invented.
                    let big = cash > self.biggest_buy.0 && self.arrivals > 1;
                    if cash > self.biggest_buy.0 {
                        self.biggest_buy = (cash, who.clone());
                    }
                    self.push(Visit {
                        at: r.at,
                        patron: *patron,
                        who: who.clone(),
                        what: if big { Cage::BigBuyIn } else { Cage::BuyIn },
                        chips: *chips,
                        cash,
                        note: format!("to {table_name}"),
                    });
                }
                Event::Left { patron, who, net, reason, cashed, .. } => {
                    // The cash paid out is on the event because only the
                    // bank could know it: the sell rate is not the buy
                    // rate, and this desk must never guess at a spread.
                    let chips = *cashed * CHIPS_PER_DOLLAR_SELL;
                    self.departures += 1;
                    self.chips_in += chips;
                    self.cash_out += *cashed;
                    let big = *cashed > self.biggest_cash.0 && self.departures > 1;
                    if *cashed > self.biggest_cash.0 {
                        self.biggest_cash = (*cashed, who.clone());
                    }
                    self.push(Visit {
                        at: r.at,
                        patron: *patron,
                        who: who.clone(),
                        what: if big { Cage::BigCashOut } else { Cage::CashOut },
                        chips,
                        cash: *cashed,
                        note: describe_departure(*reason, *net),
                    });
                }
                Event::Tier { patron, who, name, .. } => {
                    self.push(Visit {
                        at: r.at,
                        patron: *patron,
                        who: who.clone(),
                        what: Cage::Standing,
                        chips: 0,
                        cash: 0,
                        note: (*name).to_string(),
                    });
                }
                _ => {}
            }
        }
        self.seen = Some(cursor);
    }

    fn push(&mut self, v: Visit) {
        if self.book.len() == self.cap {
            self.book.pop_front();
        }
        self.book.push_back(v);
    }

    /// Where this desk has read up to, or `None` if it never has.
    pub fn cursor(&self) -> Option<u64> {
        self.seen
    }

    /// The most recent lines, newest last.
    pub fn recent(&self, n: usize) -> Vec<Visit> {
        let start = self.book.len().saturating_sub(n);
        self.book.iter().skip(start).cloned().collect()
    }

    pub fn arrivals(&self) -> u64 {
        self.arrivals
    }

    pub fn departures(&self) -> u64 {
        self.departures
    }

    /// Chips issued at the counter, and chips taken back.
    pub fn chips(&self) -> (i64, i64) {
        (self.chips_out, self.chips_in)
    }

    /// Cash taken at the counter, and cash paid out — as the *desk* saw
    /// it, counted from the events it was handed.
    ///
    /// The dashboard deliberately shows the bank's figures instead, since
    /// the bank is the only thing that moved any money and there must be
    /// exactly one answer to "what did the cage take". This is the desk's
    /// own tally, kept so the two can be compared: if they ever disagree,
    /// an event went missing, and that is worth being able to find out.
    #[allow(dead_code, reason = "the desk's own audit tally, checked against the bank by tests")]
    pub fn cash(&self) -> (i64, i64) {
        (self.cash_in, self.cash_out)
    }

    /// What stayed at the counter: cash in, less cash out.
    #[allow(dead_code, reason = "paired with `cash`, for the same reason")]
    pub fn net_flow(&self) -> i64 {
        self.cash_in - self.cash_out
    }

    /// The largest single buy-in and cash-out of the night, with the name
    /// against each.
    pub fn biggest(&self) -> (&(i64, String), &(i64, String)) {
        (&self.biggest_buy, &self.biggest_cash)
    }

    /// How many lines are being kept. The ring's size, which is what
    /// proves it is one.
    #[allow(dead_code, reason = "the boundedness the tests assert on")]
    pub fn held(&self) -> usize {
        self.book.len()
    }
}

/// Brings a desk up to date with a feed.
///
/// The cursor protocol lives here and nowhere else, because it is the one
/// part of this that is easy to get subtly wrong: a desk that has never
/// read takes everything the ring still holds, and a desk that has takes
/// only what came after it. Getting either wrong would double a count or
/// silently drop the first thing that ever happened.
///
/// Costs one tick's worth of events, and nothing at all when the feed has
/// not moved.
pub fn catch_up(desk: &mut Reception, feed: &Feed) {
    let cursor = feed.cursor();
    if desk.cursor() == Some(cursor) {
        return;
    }
    let fresh = match desk.cursor() {
        Some(seen) => feed.since(seen, Weight::Notable),
        // Never read before: take whatever the ring still holds. Anything
        // already evicted is genuinely gone, which is what bounded means.
        None => feed.recent(usize::MAX, Weight::Notable),
    };
    desk.absorb(&fresh, cursor);
}

/// How a departure reads at the desk. The reason is the event's; this only
/// puts the visit's result beside it.
fn describe_departure(reason: Departure, net: i64) -> String {
    if net >= 0 {
        format!("{} · up {net}", reason.label())
    } else {
        format!("{} · down {}", reason.label(), -net)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_with(events: Vec<(Weight, Event)>) -> Feed {
        let mut f = Feed::new(256);
        for (w, e) in events {
            f.push(Duration::from_secs(1), w, e);
        }
        f
    }

    fn arrival(patron: u64, chips: i64) -> Event {
        Event::Arrived {
            patron,
            who: format!("Patron {patron}"),
            table: 1,
            table_name: "Blackjack #1".into(),
            chips,
        }
    }

    #[test]
    fn a_buy_in_reaches_the_desk_with_the_cash_it_crossed_for() {
        let f = feed_with(vec![(Weight::Notable, arrival(7, 5_000))]);
        let mut desk = Reception::default();
        catch_up(&mut desk, &f);

        assert_eq!(desk.arrivals(), 1);
        assert_eq!(desk.chips(), (5_000, 0));
        // 5,000 chips at the buy rate is what the cage took in cash.
        assert_eq!(desk.cash(), (5_000 / CHIPS_PER_DOLLAR, 0));
        assert_eq!(desk.recent(4).len(), 1);
    }

    #[test]
    fn a_cash_out_uses_the_banks_figure_rather_than_guessing_a_rate() {
        let f = feed_with(vec![(
            Weight::Notable,
            Event::Left { patron: 7, who: "Ada".into(), table: 1, net: 1_200, reason: Departure::Ahead, cashed: 620 },
        )]);
        let mut desk = Reception::default();
        catch_up(&mut desk, &f);

        assert_eq!(desk.departures(), 1);
        assert_eq!(desk.cash(), (0, 620));
        assert_eq!(desk.net_flow(), -620);
    }

    #[test]
    fn the_desk_never_reads_the_same_record_twice() {
        let mut f = feed_with(vec![(Weight::Notable, arrival(1, 1_000))]);
        let mut desk = Reception::default();
        catch_up(&mut desk, &f);
        // Nothing new: catching up again must not double the counts.
        catch_up(&mut desk, &f);
        catch_up(&mut desk, &f);
        assert_eq!(desk.arrivals(), 1);

        f.push(Duration::from_secs(2), Weight::Notable, arrival(2, 2_000));
        catch_up(&mut desk, &f);
        assert_eq!(desk.arrivals(), 2);
        assert_eq!(desk.chips(), (3_000, 0));
    }

    #[test]
    fn the_day_book_stays_bounded_however_long_the_night_runs() {
        let mut desk = Reception::new(8);
        let mut f = Feed::new(4_096);
        for i in 0..200u64 {
            f.push(Duration::from_secs(i), Weight::Notable, arrival(i, 1_000));
        }
        catch_up(&mut desk, &f);
        assert_eq!(desk.held(), 8, "the ring must not grow with the night");
        assert_eq!(desk.arrivals(), 200, "but the counts must still be whole");
    }

    #[test]
    fn the_biggest_movements_of_the_night_are_remembered() {
        let f = feed_with(vec![
            (Weight::Notable, arrival(1, 1_000)),
            (Weight::Notable, arrival(2, 90_000)),
            (Weight::Notable, arrival(3, 4_000)),
        ]);
        let mut desk = Reception::default();
        catch_up(&mut desk, &f);
        let (buy, _) = desk.biggest();
        assert_eq!(buy.0, 9_000);
        assert_eq!(buy.1, "Patron 2");
    }
}
