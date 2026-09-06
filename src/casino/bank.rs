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
    rounds: u64,
    bets: u64,
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
            rounds: 0,
            bets: 0,
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
        dollars
    }

    /// Books one settled bet. `house_delta` is what the tray gains, so it is
    /// positive when the patron lost — the exact mirror of the wallet-side
    /// convention in `economy::House`.
    pub fn settle(&mut self, table: &'static str, house_delta: i64) {
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
        b.settle("slots", 40); // a patron lost 40
        assert_eq!(b.chips(), 10_040);
        assert_eq!(b.money(), 1_000, "a bet never touches the cage");
        b.settle("slots", -100); // and then won 100
        assert_eq!(b.chips(), 9_940);
        assert_eq!(b.table_profit(), -60);
    }

    #[test]
    fn table_profit_is_always_collected_minus_paid() {
        let mut b = Bank::new(0, 0);
        for d in [-50, 120, -8, 0, 6_000, -6_000, 30] {
            b.settle("probe", d);
            let (collected, paid, _, _) = b.totals();
            assert_eq!(b.table_profit(), collected - paid);
        }
    }

    #[test]
    fn every_table_is_booked_separately() {
        let mut b = Bank::new(0, 0);
        b.settle("slots", 40);
        b.settle("roulette", -15);
        b.settle("slots", 100);
        let by = b.by_table();
        assert_eq!(by.iter().find(|(k, _)| *k == "slots").unwrap().1, 140);
        assert_eq!(by.iter().find(|(k, _)| *k == "roulette").unwrap().1, -15);
        assert_eq!(by[0].0, "slots", "the busiest table sorts first");
    }

    #[test]
    fn a_zero_settlement_still_counts_as_a_bet() {
        let mut b = Bank::new(0, 0);
        b.settle("probe", 0);
        assert_eq!(b.bets(), 1);
        assert_eq!(b.chips(), 0);
    }
}
