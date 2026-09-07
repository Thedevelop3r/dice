//! The economy manager — the casino's single source of truth for money and
//! chips.
//!
//! Money and chips are genuinely separate quantities here, not two views of
//! one number, and they move for different reasons:
//!
//! - **Chips** are the float in the house tray. They move bet by bet, one
//!   settlement at a time, and the house's edge is what makes the tray grow.
//! - **Money** is cash in the cage. It moves only when a patron buys in or
//!   cashes out.
//!
//! Because the cage buys chips back at a worse rate than it sells them
//! (`CHIPS_PER_DOLLAR` against `CHIPS_PER_DOLLAR_SELL` — the Store's own
//! long-standing spread), the house takes a cut on every visit regardless of
//! how the tables run. That is why both numbers move independently, and why
//! watching them is worth doing.
//!
//! Nothing outside this file is allowed to hold its own copy of either
//! total. Every game instance reports its settlements here and reads back
//! what it needs.

use crate::economy::{CHIPS_PER_DOLLAR, CHIPS_PER_DOLLAR_SELL};
use std::collections::BTreeMap;

/// Every way money or chips can move in this building.
///
/// The point of naming them is that a movement is then a *thing* the rest
/// of the program can count, filter and report on, rather than an
/// unlabelled `+=` somewhere. What this deliberately is **not** is a log:
/// a busy floor makes millions of these an hour, so each kind is
/// accumulated as it happens and nothing is stored per movement. The event
/// feed already keeps the handful worth reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Movement {
    /// Cash over the counter, chips out of the tray.
    BuyIn,
    /// Chips back into the tray, cash out of the cage.
    CashOut,
    /// Chips staked at a table.
    Wager,
    /// Chips paid back out on a winning bet.
    Payout,
    /// Cash the house spent to keep the doors open.
    Expense,
}

impl Movement {
    pub const ALL: [Movement; 5] =
        [Movement::BuyIn, Movement::CashOut, Movement::Wager, Movement::Payout, Movement::Expense];

    pub fn label(self) -> &'static str {
        match self {
            Movement::BuyIn => "buy-ins",
            Movement::CashOut => "cash-outs",
            Movement::Wager => "wagers",
            Movement::Payout => "payouts",
            Movement::Expense => "expenses",
        }
    }

    /// Whether the amount is in dollars (`true`) or chips (`false`). The
    /// two are never added together anywhere in this program.
    pub fn in_dollars(self) -> bool {
        matches!(self, Movement::BuyIn | Movement::CashOut | Movement::Expense)
    }
}

/// What the house spends money on. Kept as data so the books can say where
/// the night went, rather than showing one undifferentiated "costs" figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Expense {
    /// The building: lights, rent, security, the licence.
    Overhead,
    /// Dealers and floor staff, charged per open table.
    Staffing,
}

impl Expense {
    pub fn label(self) -> &'static str {
        match self {
            Expense::Overhead => "overhead",
            Expense::Staffing => "staffing",
        }
    }
}

/// What the house opens the doors with when a casino is started fresh.
pub const OPENING_MONEY: i64 = 250_000;
pub const OPENING_CHIPS: i64 = 500_000;

/// The casino's books. One of these exists per running casino, behind the
/// simulation manager's lock.
#[derive(Debug, Clone)]
pub struct Bank {
    money: i64,
    chips: i64,
    /// Chips taken off losing bets, and paid out on winning ones.
    collected: i64,
    paid: i64,
    /// Cash over the counter, both directions.
    bought_in: i64,
    cashed_out: i64,
    /// Everything staked at the tables, and everything paid back out. The
    /// difference is the gaming win; the *handle* is the more interesting
    /// number, because it is what the house edge is a percentage of.
    handle: i64,
    payouts: i64,
    rounds: u64,
    bets: u64,
    /// Cash spent keeping the doors open, in total and by kind.
    spent: i64,
    per_expense: BTreeMap<&'static str, i64>,
    /// How many of each kind of movement, and how much they came to.
    counts: [u64; 5],
    volume: [i64; 5],
    /// Per-table chip profit, keyed the same way `economy::House` keys it,
    /// so the two ledgers can be read side by side.
    per_table: BTreeMap<&'static str, i64>,
}

impl Bank {
    pub fn new(money: i64, chips: i64) -> Bank {
        Bank {
            money,
            chips,
            collected: 0,
            paid: 0,
            bought_in: 0,
            cashed_out: 0,
            handle: 0,
            payouts: 0,
            rounds: 0,
            bets: 0,
            spent: 0,
            per_expense: BTreeMap::new(),
            counts: [0; 5],
            volume: [0; 5],
            per_table: BTreeMap::new(),
        }
    }

    /// A patron hands over cash at the cage and walks away with chips.
    /// Returns the chips issued.
    pub fn buy_in(&mut self, dollars: i64) -> i64 {
        let dollars = dollars.max(0);
        let chips = dollars * CHIPS_PER_DOLLAR;
        self.money += dollars;
        self.chips -= chips;
        self.bought_in += dollars;
        self.record(Movement::BuyIn, dollars);
        chips
    }

    /// A patron brings chips back to the cage. Returns the cash paid out —
    /// at the sell rate, which is where the house's spread lives.
    pub fn cash_out(&mut self, chips: i64) -> i64 {
        let chips = chips.max(0);
        let dollars = chips / CHIPS_PER_DOLLAR_SELL;
        self.chips += chips;
        self.money -= dollars;
        self.cashed_out += dollars;
        self.record(Movement::CashOut, dollars);
        dollars
    }

    /// Books one settled bet: what went down, and what came back.
    ///
    /// The tray gains `staked - returned`, which is positive when the
    /// patron lost — the exact mirror of the wallet-side convention in
    /// `economy::House`.
    pub fn settle(&mut self, table: &'static str, staked: i64, returned: i64) {
        let house_delta = staked - returned;
        self.handle += staked;
        self.payouts += returned;
        self.record(Movement::Wager, staked);
        self.record(Movement::Payout, returned);
        self.chips += house_delta;
        if house_delta > 0 {
            self.collected += house_delta;
        } else {
            self.paid += -house_delta;
        }
        *self.per_table.entry(table).or_insert(0) += house_delta;
        self.bets += 1;
    }

    pub fn count_round(&mut self) {
        self.rounds += 1;
    }

    /// The house pays a bill. Cash only — the tray is the patrons' float
    /// and never pays for anything.
    ///
    /// The cage is allowed to go negative. A casino that is losing money
    /// faster than it takes it should look like one, not silently refuse
    /// to pay its staff.
    pub fn pay(&mut self, kind: Expense, dollars: i64) {
        let dollars = dollars.max(0);
        if dollars == 0 {
            return;
        }
        self.money -= dollars;
        self.spent += dollars;
        *self.per_expense.entry(kind.label()).or_insert(0) += dollars;
        self.record(Movement::Expense, dollars);
    }

    fn record(&mut self, m: Movement, amount: i64) {
        let at = m as usize;
        self.counts[at] += 1;
        self.volume[at] += amount;
    }

    /// Everything ever staked at the tables. The denominator the house
    /// edge is measured against.
    pub fn handle(&self) -> i64 {
        self.handle
    }

    pub fn payouts(&self) -> i64 {
        self.payouts
    }

    /// Gross gaming revenue, in chips: everything staked, less everything
    /// paid back. Identical to `table_profit`, and named twice on purpose —
    /// one name is what a dealer would call it, the other is what a set of
    /// books would.
    pub fn ggr(&self) -> i64 {
        self.handle - self.payouts
    }

    /// The house edge actually realised, in hundredths of a percent, so a
    /// 4.15% hold reads as `415`. Integers all the way down.
    pub fn hold(&self) -> i64 {
        if self.handle == 0 {
            return 0;
        }
        self.ggr() * 10_000 / self.handle
    }

    /// Cash spent keeping the doors open.
    pub fn spent(&self) -> i64 {
        self.spent
    }

    /// Net gaming revenue, in dollars: the cash the cage actually kept,
    /// less what the building cost to run.
    ///
    /// This is deliberately measured in *cash*, not in chips. Chips only
    /// ever leave the building through the cage, so cage flow is the gaming
    /// win already converted at the house's own spread — adding a chip
    /// figure to it would count the same money twice.
    pub fn ngr(&self) -> i64 {
        self.cage_profit() - self.spent
    }

    /// What the house spent, by kind, biggest first.
    pub fn by_expense(&self) -> Vec<(&'static str, i64)> {
        let mut v: Vec<(&'static str, i64)> = self.per_expense.iter().map(|(k, n)| (*k, *n)).collect();
        v.sort_by_key(|(_, n)| -*n);
        v
    }

    /// Every kind of movement, with how many there have been and what they
    /// came to. Accumulated as they happened, never recomputed.
    pub fn movements(&self) -> Vec<(Movement, u64, i64)> {
        Movement::ALL.iter().map(|m| (*m, self.counts[*m as usize], self.volume[*m as usize])).collect()
    }

    pub fn money(&self) -> i64 {
        self.money
    }

    pub fn chips(&self) -> i64 {
        self.chips
    }

    pub fn table_profit(&self) -> i64 {
        self.collected - self.paid
    }

    pub fn cage_profit(&self) -> i64 {
        self.bought_in - self.cashed_out
    }

    /// Rebuilds a bank from a saved one, so the books carry on rather than
    /// starting again. Deliberately takes every figure explicitly: a field
    /// added later that this forgets is a figure silently reset to zero on
    /// every reopening, which is the sort of bug nobody notices for weeks.
    pub fn restore(s: &super::save::SavedBank, per_table: &[(String, i64)], per_expense: &[(String, i64)]) -> Bank {
        let mut b = Bank::new(s.money, s.chips);
        b.collected = s.collected;
        b.paid = s.paid;
        b.bought_in = s.bought_in;
        b.cashed_out = s.cashed_out;
        b.handle = s.handle;
        b.payouts = s.payouts;
        b.rounds = s.rounds;
        b.bets = s.bets;
        b.spent = s.spent;
        for (key, amount) in per_table {
            // The keys are `&'static str` everywhere else, and a table that
            // is no longer in the program is a book nobody can add to — so
            // only keys the build still knows about are carried forward.
            if let Some(k) = super::instance::Kind::from_key(key) {
                *b.per_table.entry(k.key()).or_insert(0) += amount;
            } else if *key == "tourney" {
                *b.per_table.entry("tourney").or_insert(0) += amount;
            }
        }
        for (key, amount) in per_expense {
            for e in [super::bank::Expense::Overhead, super::bank::Expense::Staffing] {
                if e.label() == key {
                    *b.per_expense.entry(e.label()).or_insert(0) += amount;
                }
            }
        }
        b
    }

    /// The books, ready to be written down.
    pub fn saved(&self) -> super::save::SavedBank {
        super::save::SavedBank {
            money: self.money,
            chips: self.chips,
            collected: self.collected,
            paid: self.paid,
            bought_in: self.bought_in,
            cashed_out: self.cashed_out,
            handle: self.handle,
            payouts: self.payouts,
            rounds: self.rounds,
            bets: self.bets,
            spent: self.spent,
        }
    }

    pub fn totals(&self) -> (i64, i64, i64, i64) {
        (self.collected, self.paid, self.bought_in, self.cashed_out)
    }

    pub fn rounds(&self) -> u64 {
        self.rounds
    }

    pub fn bets(&self) -> u64 {
        self.bets
    }

    /// Per-table chip profit, busiest first.
    pub fn by_table(&self) -> Vec<(&'static str, i64)> {
        let mut v: Vec<(&'static str, i64)> = self.per_table.iter().map(|(k, n)| (*k, *n)).collect();
        v.sort_by_key(|(_, n)| -*n);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buy_in_swaps_cash_for_chips() {
        let mut b = Bank::new(1_000, 10_000);
        let chips = b.buy_in(50);
        assert_eq!(chips, 50 * CHIPS_PER_DOLLAR);
        assert_eq!(b.money(), 1_050, "the cage took the cash");
        assert_eq!(b.chips(), 10_000 - chips, "and let the chips go");
    }

    #[test]
    fn the_cage_spread_is_the_houses_cut_on_every_visit() {
        // Buy 50 dollars of chips and immediately hand every one of them
        // back: the patron leaves with less than they arrived with, and the
        // difference is the house's, without a single bet being placed.
        let mut b = Bank::new(0, 100_000);
        let chips = b.buy_in(50);
        let back = b.cash_out(chips);
        assert!(back < 50, "the cage bought back at {back}, which is not a spread");
        assert_eq!(b.cage_profit(), 50 - back);
        assert!(b.cage_profit() > 0);
    }

    #[test]
    fn settling_moves_the_tray_and_nothing_else() {
        let mut b = Bank::new(1_000, 10_000);
        b.settle("slots", 40, 0); // a patron lost 40
        assert_eq!(b.chips(), 10_040);
        assert_eq!(b.money(), 1_000, "a bet never touches the cage");
        b.settle("slots", 0, 100); // and then won 100
        assert_eq!(b.chips(), 9_940);
        assert_eq!(b.table_profit(), -60);
    }

    #[test]
    fn table_profit_is_always_collected_minus_paid() {
        let mut b = Bank::new(0, 0);
        for (staked, returned) in [(0, 50), (120, 0), (0, 8), (0, 0), (6_000, 0), (0, 6_000), (30, 0)] {
            b.settle("probe", staked, returned);
            let (collected, paid, _, _) = b.totals();
            assert_eq!(b.table_profit(), collected - paid);
        }
    }

    #[test]
    fn every_table_is_booked_separately() {
        let mut b = Bank::new(0, 0);
        b.settle("slots", 40, 0);
        b.settle("roulette", 0, 15);
        b.settle("slots", 100, 0);
        let by = b.by_table();
        assert_eq!(by.iter().find(|(k, _)| *k == "slots").unwrap().1, 140);
        assert_eq!(by.iter().find(|(k, _)| *k == "roulette").unwrap().1, -15);
        assert_eq!(by[0].0, "slots", "the busiest table sorts first");
    }

    #[test]
    fn the_handle_is_everything_staked_and_the_win_is_what_stayed() {
        let mut b = Bank::new(0, 100_000);
        b.settle("probe", 100, 0);
        b.settle("probe", 100, 250);
        b.settle("probe", 100, 90);
        assert_eq!(b.handle(), 300, "the handle is turnover, not profit");
        assert_eq!(b.payouts(), 340);
        assert_eq!(b.ggr(), -40);
        assert_eq!(b.ggr(), b.table_profit(), "the two names must always mean the same thing");
    }

    #[test]
    fn the_hold_is_the_win_as_a_share_of_the_handle() {
        let mut b = Bank::new(0, 1_000_000);
        // A hundred bets of 100, returning 96 each: a 4% hold, exactly.
        for _ in 0..100 {
            b.settle("probe", 100, 96);
        }
        assert_eq!(b.handle(), 10_000);
        assert_eq!(b.ggr(), 400);
        assert_eq!(b.hold(), 400, "4.00% should read as 400 hundredths");
    }

    #[test]
    fn a_house_with_no_bets_has_no_hold_rather_than_a_divide_by_zero() {
        let b = Bank::new(0, 0);
        assert_eq!(b.hold(), 0);
        assert_eq!(b.ggr(), 0);
        assert_eq!(b.ngr(), 0);
    }

    #[test]
    fn the_bills_come_out_of_the_cage_and_never_out_of_the_tray() {
        let mut b = Bank::new(10_000, 500_000);
        let chips = b.chips();
        b.pay(Expense::Overhead, 400);
        b.pay(Expense::Staffing, 90);
        assert_eq!(b.money(), 10_000 - 490);
        assert_eq!(b.chips(), chips, "a bill was paid out of the patrons' float");
        assert_eq!(b.spent(), 490);
        let by = b.by_expense();
        assert_eq!(by[0], ("overhead", 400), "the biggest cost sorts first");
        assert_eq!(by[1], ("staffing", 90));
    }

    #[test]
    fn a_casino_that_is_losing_money_is_allowed_to_look_like_one() {
        let mut b = Bank::new(100, 0);
        b.pay(Expense::Overhead, 1_000);
        assert_eq!(b.money(), -900, "the cage must be allowed to go negative");
        assert!(b.ngr() < 0);
    }

    #[test]
    fn the_bottom_line_is_cash_the_cage_kept_less_what_the_doors_cost() {
        let mut b = Bank::new(0, 1_000_000);
        let chips = b.buy_in(1_000);
        // They lose a quarter of it and cash the rest back in.
        b.settle("probe", chips / 4, 0);
        b.cash_out(chips - chips / 4);
        let kept = b.cage_profit();
        assert!(kept > 0, "the house should be up on a losing patron");
        assert_eq!(b.ngr(), kept, "with no costs, the bottom line is the cage");
        b.pay(Expense::Overhead, 40);
        assert_eq!(b.ngr(), kept - 40);
    }

    #[test]
    fn every_movement_is_counted_and_kept_in_its_own_currency() {
        let mut b = Bank::new(10_000, 1_000_000);
        b.buy_in(100);
        b.buy_in(50);
        b.settle("probe", 30, 10);
        b.cash_out(20);
        b.pay(Expense::Staffing, 7);

        let m = b.movements();
        let find = |want: Movement| m.iter().find(|(k, _, _)| *k == want).copied().expect("kind is listed");
        assert_eq!(find(Movement::BuyIn), (Movement::BuyIn, 2, 150));
        assert_eq!(find(Movement::Wager), (Movement::Wager, 1, 30));
        assert_eq!(find(Movement::Payout), (Movement::Payout, 1, 10));
        assert_eq!(find(Movement::CashOut), (Movement::CashOut, 1, 1));
        assert_eq!(find(Movement::Expense), (Movement::Expense, 1, 7));
        // And the two currencies are never confused for each other.
        assert!(Movement::BuyIn.in_dollars() && Movement::CashOut.in_dollars() && Movement::Expense.in_dollars());
        assert!(!Movement::Wager.in_dollars() && !Movement::Payout.in_dollars());
    }

    #[test]
    fn a_bill_of_nothing_is_not_a_transaction() {
        let mut b = Bank::new(500, 0);
        b.pay(Expense::Overhead, 0);
        b.pay(Expense::Overhead, -50);
        assert_eq!(b.money(), 500);
        assert_eq!(b.spent(), 0);
        assert!(b.by_expense().is_empty());
    }

    #[test]
    fn a_bank_written_down_and_read_back_is_the_same_bank() {
        let mut b = Bank::new(10_000, 500_000);
        b.buy_in(400);
        b.settle("slots", 900, 700);
        b.settle("roulette", 500, 800);
        b.cash_out(1_200);
        b.pay(Expense::Overhead, 250);
        b.count_round();

        let saved = b.saved();
        let table_books: Vec<(String, i64)> = b.by_table().iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let expense_books: Vec<(String, i64)> = b.by_expense().iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let back = Bank::restore(&saved, &table_books, &expense_books);

        assert_eq!(back.money(), b.money());
        assert_eq!(back.chips(), b.chips());
        assert_eq!(back.handle(), b.handle());
        assert_eq!(back.payouts(), b.payouts());
        assert_eq!(back.ggr(), b.ggr());
        assert_eq!(back.hold(), b.hold(), "the realised hold changed across a reopening");
        assert_eq!(back.cage_profit(), b.cage_profit());
        assert_eq!(back.ngr(), b.ngr());
        assert_eq!(back.rounds(), b.rounds());
        assert_eq!(back.bets(), b.bets());
        assert_eq!(back.spent(), b.spent());
        assert_eq!(back.by_table(), b.by_table(), "a table's book was lost");
        assert_eq!(back.by_expense(), b.by_expense());
    }

    #[test]
    fn a_book_for_a_game_the_build_no_longer_has_is_dropped_rather_than_kept() {
        // Keys are `&'static str` throughout; a saved book for a table that
        // has since been removed has nowhere to live, and carrying it as a
        // leaked string would be worse than losing it.
        let saved = Bank::new(0, 0).saved();
        let b = Bank::restore(&saved, &[("quoits".into(), 500), ("slots".into(), 250)], &[]);
        let books = b.by_table();
        assert!(books.iter().any(|(k, v)| *k == "slots" && *v == 250));
        assert!(!books.iter().any(|(k, _)| *k == "quoits"), "a book survived its game");
    }

    #[test]
    fn a_zero_settlement_still_counts_as_a_bet() {
        let mut b = Bank::new(0, 0);
        b.settle("probe", 0, 0);
        assert_eq!(b.bets(), 1);
        assert_eq!(b.chips(), 0);
    }
}
