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
