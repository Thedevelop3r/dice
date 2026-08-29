//! Shared in-game economy: chips (table currency), dollars (in-game "cash"),
//! and the store inventory. All values are fictional in-game currency.

use crate::stats::Store as Save;

/// Dollars -> chips when buying.
pub const CHIPS_PER_DOLLAR: i64 = 10;
/// Chips -> dollars when cashing out (the spread is the house edge).
pub const CHIPS_PER_DOLLAR_SELL: i64 = 12;
pub const START_CHIPS: i64 = 100;
pub const START_DOLLARS: i64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Reroll,
    Insurance,
    LuckyCharm,
    VipTable,
    GoldDice,
}

impl Item {
    pub const ALL: [Item; 5] = [Item::Reroll, Item::Insurance, Item::LuckyCharm, Item::VipTable, Item::GoldDice];

    pub fn key(&self) -> &'static str {
        match self {
            Item::Reroll => "inv.reroll",
            Item::Insurance => "inv.insurance",
            Item::LuckyCharm => "inv.charm",
            Item::VipTable => "inv.vip",
            Item::GoldDice => "inv.gold",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Item::Reroll => "Reroll Token",
            Item::Insurance => "Insurance Chit",
            Item::LuckyCharm => "Lucky Charm",
            Item::VipTable => "VIP Table Pass",
            Item::GoldDice => "Gold Dice Skin",
        }
    }

    pub fn blurb(&self) -> &'static str {
        match self {
            Item::Reroll => "Reroll the whole Luck Bet table once after a losing round.",
            Item::Insurance => "Refunds half your stake on a losing round.",
            Item::LuckyCharm => "Rerolls only your own die once if it missed.",
            Item::VipTable => "Permanent: Luck Bet pays 6:1 instead of 5:1.",
            Item::GoldDice => "Permanent: your dice render in gold.",
        }
    }

    /// Price in dollars.
    pub fn price(&self) -> i64 {
        match self {
            Item::Reroll => 3,
            Item::Insurance => 2,
            Item::LuckyCharm => 4,
            Item::VipTable => 40,
            Item::GoldDice => 15,
        }
    }

    /// Permanent upgrades are owned once; the rest are stackable consumables.
    pub fn permanent(&self) -> bool {
        matches!(self, Item::VipTable | Item::GoldDice)
    }
}

pub struct Wallet<'a> {
    save: &'a mut Save,
}

impl<'a> Wallet<'a> {
    pub fn new(save: &'a mut Save) -> Wallet<'a> {
        if save.get_str("econ.init", "").is_empty() {
            // First run: seed the starting balances, honouring any legacy bankroll.
            let legacy = save.get_i64("chuck.bank", START_CHIPS);
            save.set_i64("econ.chips", legacy.max(START_CHIPS));
            save.set_i64("econ.dollars", START_DOLLARS);
            save.set_str("econ.init", "1");
        }
        Wallet { save }
    }

    pub fn chips(&self) -> i64 {
        self.save.get_i64("econ.chips", START_CHIPS)
    }

    pub fn dollars(&self) -> i64 {
        self.save.get_i64("econ.dollars", START_DOLLARS)
    }

    pub fn add_chips(&mut self, n: i64) {
        let v = self.chips() + n;
        self.save.set_i64("econ.chips", v);
        self.save.record_best("econ.peak_chips", v);
        let _ = self.save.save();
    }

    pub fn add_dollars(&mut self, n: i64) {
        let v = self.dollars() + n;
        self.save.set_i64("econ.dollars", v);
        let _ = self.save.save();
    }

    /// Removes `n` chips if they are there. Returns false when short.
    pub fn spend_chips(&mut self, n: i64) -> bool {
        if self.chips() < n {
            return false;
        }
        self.add_chips(-n);
        true
    }

    pub fn spend_dollars(&mut self, n: i64) -> bool {
        if self.dollars() < n {
            return false;
        }
        self.add_dollars(-n);
        true
    }

    pub fn count(&self, item: Item) -> i64 {
        self.save.get_i64(item.key(), 0)
    }

    pub fn owns(&self, item: Item) -> bool {
        self.count(item) > 0
    }

    pub fn grant(&mut self, item: Item, n: i64) {
        let v = if item.permanent() { 1 } else { self.count(item) + n };
        self.save.set_i64(item.key(), v);
        let _ = self.save.save();
    }

    /// Consumes one consumable. Returns false if none are held.
    pub fn consume(&mut self, item: Item) -> bool {
        if item.permanent() || self.count(item) <= 0 {
            return false;
        }
        self.save.set_i64(item.key(), self.count(item) - 1);
        let _ = self.save.save();
        true
    }

    /// The house restakes a broke player so the game is never a dead end.
    pub fn ensure_solvent(&mut self, floor: i64) -> bool {
        if self.chips() <= 0 && self.dollars() <= 0 {
            self.save.set_i64("econ.chips", floor);
            self.save.bump("econ.bailouts", 1);
            let _ = self.save.save();
            return true;
        }
        false
    }
}

/// The casino's own ledger — the mirror image of the player's wallet.
/// Every wagering game settles between a player and the house: whatever
/// chips a player's wallet gains, the house's balance loses, and vice
/// versa, so there's nowhere else for the money to have come from or
/// gone to. This deliberately covers only *games* (Chuck-a-Luck, Luck
/// Bet, Roulette, Blackjack, Tournament, and Ultra Casino Dice's
/// unclaimed pots) — not Store currency exchanges or item purchases,
/// which move chips for a reason other than a wager's outcome.
pub struct House;

impl House {
    /// Records one settlement: `player_delta` is how many chips the
    /// player's wallet just changed by for this bet/hand/round (positive
    /// on a win, negative on a loss — exactly what you'd add to a
    /// wallet). The house's balance moves by the exact opposite amount.
    /// `game` is the same short prefix each game already uses for its own
    /// stats (`"roulette"`, `"blackjack"`, ...), so a running per-game
    /// house balance shows up right alongside that game's existing
    /// numbers on the Stats screen, under `{game}.house_pl`.
    pub fn record(save: &mut Save, game: &str, player_delta: i64) {
        let house_delta = -player_delta;
        let balance = save.get_i64("house.balance", 0) + house_delta;
        save.set_i64("house.balance", balance);
        if house_delta > 0 {
            save.bump("house.collected", house_delta);
        } else if house_delta < 0 {
            save.bump("house.paid", -house_delta);
        }
        save.bump(&format!("{game}.house_pl"), house_delta);
        let _ = save.save();
    }

    /// The house's current net balance — positive means it's ahead across
    /// every game played so far, negative means players are collectively
    /// up on the house.
    pub fn balance(save: &Save) -> i64 {
        save.get_i64("house.balance", 0)
    }

    /// Lifetime totals: chips the house has collected from losing bets,
    /// and chips it's paid out on winning ones. `collected - paid` always
    /// equals `balance()`.
    pub fn totals(save: &Save) -> (i64, i64) {
        (save.get_i64("house.collected", 0), save.get_i64("house.paid", 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_win_is_an_equal_house_loss() {
        let mut save = Save::blank();
        House::record(&mut save, "roulette", 350); // player nets +350
        assert_eq!(House::balance(&save), -350);
        assert_eq!(House::totals(&save), (0, 350));
    }

    #[test]
    fn a_player_loss_is_an_equal_house_win() {
        let mut save = Save::blank();
        House::record(&mut save, "blackjack", -20); // player nets -20
        assert_eq!(House::balance(&save), 20);
        assert_eq!(House::totals(&save), (20, 0));
    }

    #[test]
    fn balance_always_equals_collected_minus_paid() {
        let mut save = Save::blank();
        for delta in [-50, 120, -8, 0, 6000, -6000, 30] {
            House::record(&mut save, "chuck", delta);
            let (collected, paid) = House::totals(&save);
            assert_eq!(House::balance(&save), collected - paid);
        }
    }

    #[test]
    fn balance_accumulates_across_settlements_and_games() {
        let mut save = Save::blank();
        House::record(&mut save, "chuck", 40); // house -40
        House::record(&mut save, "roulette", -15); // house +15
        House::record(&mut save, "chuck", -100); // house +100
        assert_eq!(House::balance(&save), 75);
    }

    #[test]
    fn per_game_totals_are_tracked_separately() {
        let mut save = Save::blank();
        House::record(&mut save, "chuck", 40);
        House::record(&mut save, "roulette", -15);
        House::record(&mut save, "chuck", -100);
        assert_eq!(save.get_i64("chuck.house_pl", 0), 60); // -40 + 100
        assert_eq!(save.get_i64("roulette.house_pl", 0), 15);
    }

    #[test]
    fn a_zero_delta_settlement_moves_nothing() {
        let mut save = Save::blank();
        House::record(&mut save, "tourney", 0);
        assert_eq!(House::balance(&save), 0);
        assert_eq!(House::totals(&save), (0, 0));
    }
}
