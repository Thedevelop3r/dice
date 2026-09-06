//! Shared furniture every wagering table needs: restaking a broke player,
//! taking a stake, settling a bet against both wallets, and the play-again
//! prompt.
//!
//! Fourteen tables opened at once; without this each would have carried its
//! own slightly-different copy of the same four beats, and the house ledger
//! would have been wired up fourteen slightly-different ways. Anything a
//! single game does specially it still does itself — this is only the part
//! that must be identical everywhere.

use super::Ctx;
use crate::economy::{House, Wallet};
use crate::ui::{self, widgets};

/// What the house restakes a cleaned-out player with, so no table is ever
/// a dead end.
pub const BAILOUT: i64 = 50;

/// Names the AI seats in idle mode draw from. Idle players are fictional
/// even by this app's standards — they wager nothing real and never touch
/// the ledger.
pub const BOT_NAMES: [&str; 14] = [
    "Ada", "Blitz", "Cricket", "Domino", "Echo", "Fable", "Gambit", "Halcyon", "Ivory", "Jinx", "Koda", "Lux", "Mirth", "Nova",
];

/// Restakes the player if they are cleaned out, then reports the bank.
/// Returns `(bank, was_restaked)` so a table can say so out loud.
pub fn open_bank(ctx: &mut Ctx) -> (i64, bool) {
    let mut w = Wallet::new(ctx.store);
    let restaked = w.chips() <= 0 && w.ensure_solvent(BAILOUT);
    (Wallet::new(ctx.store).chips(), restaked)
}

/// The standard stake stepper, drawn the same way at every table.
/// `subtitle` is the line above the amount — usually what is being backed.
/// `None` means the player cancelled and the table should let them go.
pub fn stake(ctx: &mut Ctx, title: &str, subtitle: &str, bank: i64, default: i64) -> Option<i64> {
    let bank = bank.max(1);
    let subtitle = subtitle.to_string();
    let title = title.to_string();
    widgets::number_picker(ctx.screen, 1, bank, default.clamp(1, bank), 5, &[('m', bank)], move |s, v| {
        let theme = s.theme;
        s.begin();
        ui::header(s, &title);
        s.blank();
        s.line(&format!("  bank: {}", theme.win(&format!("{bank} chips"))));
        s.blank();
        if !subtitle.is_empty() {
            s.line(&format!("  {subtitle}"));
            s.blank();
        }
        s.line(&format!("  stake: {}", theme.paint(ui::theme::GOLD, &format!("{v} chips"))));
        s.blank();
        s.line(&widgets::footer(&theme, &[('↑', "+5"), ('↓', "-5"), ('m', "max"), ('\u{23ce}', "confirm")]));
    })
}

/// Settles one bet. `stake` must already have been taken with
/// `spend_chips`; `payout` is the gross returned to the player, so 0 is a
/// total loss and `stake` itself is a push. Pays the player, books the
/// exact mirror image to the house, records the win/loss and saves.
/// Returns the player's net for the round.
pub fn settle(ctx: &mut Ctx, key: &str, stake: i64, payout: i64) -> i64 {
    if payout > 0 {
        Wallet::new(ctx.store).add_chips(payout);
    }
    let delta = payout - stake;
    House::record(ctx.store, key, delta);
    if delta > 0 {
        ctx.store.bump(&format!("{key}.wins"), 1);
        ctx.store.record_best(&format!("{key}.best_win"), delta);
    } else if delta == 0 {
        ctx.store.bump(&format!("{key}.pushes"), 1);
    } else {
        ctx.store.bump(&format!("{key}.losses"), 1);
    }
    let _ = ctx.store.save();
    delta
}

/// The one-line verdict every table ends a round on.
pub fn verdict(ctx: &mut Ctx, delta: i64) {
    let theme = ctx.theme();
    ctx.screen.blank();
    if delta > 0 {
        ctx.screen.line(&format!("  {}", theme.win(&format!("win +{delta} chips"))));
    } else if delta == 0 {
        ctx.screen.line(&format!("  {}", theme.accent("push — your stake comes back")));
    } else {
        ctx.screen.line(&format!("  {}", theme.lose(&format!("lose {delta} chips"))));
    }
}

/// Appends the play-again prompt to the frame the caller has *already*
/// drawn (this deliberately does not `begin()` a new one), presents it,
/// and reports whether to run another round.
pub fn again(ctx: &mut Ctx, label: &str) -> bool {
    let theme = ctx.theme();
    ctx.screen.blank();
    ctx.screen.line(&widgets::footer(&theme, &[('y', label), ('n', "leave the table")]));
    ctx.screen.present();
    ui::confirm_key(true)
}

/// A short notice on an otherwise empty frame — "broke, here's a stake",
/// "not enough chips for that".
pub fn message(ctx: &mut Ctx, title: &str, text: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, title);
    ctx.screen.blank();
    ctx.screen.line(&format!("  {}", theme.dim(text)));
    ctx.screen.present();
    ui::sleep_ms(700);
}

/// The closing frame a table leaves on.
pub fn cash_out(ctx: &mut Ctx, title: &str) {
    let chips = Wallet::new(ctx.store).chips();
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, title);
    ctx.screen.blank();
    ctx.screen.line(&format!("  leaving the table with {}", theme.win(&format!("{chips} chips"))));
    ctx.screen.present();
    ui::pause(ctx.screen);
}

thread_local! {
    /// When the table currently running idle should hand the floor on.
    /// `None` means it runs until the watcher leaves, which is what a
    /// single table opened from the menu does.
    static IDLE_UNTIL: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    /// Whether the last idle run ended because a person asked it to,
    /// rather than because its slot was up.
    static IDLE_LEFT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Gives the next idle table a slot of `budget`, after which `idle_hold`
/// reports done even though nobody pressed anything. `None` lets it run
/// indefinitely. This is what lets the floor cycler show every table in
/// turn without any of them knowing they are being cycled.
pub fn idle_budget(budget: Option<std::time::Duration>) {
    IDLE_UNTIL.with(|c| c.set(budget.map(|d| std::time::Instant::now() + d)));
    IDLE_LEFT.with(|c| c.set(false));
}

/// Did the idle run that just finished end because the watcher left?
/// Distinguishes "they want out" from "this table's slot expired".
pub fn idle_left() -> bool {
    IDLE_LEFT.with(|c| c.get())
}

/// Sleeps in short slices so an idle table stays responsive, returning
/// `true` the moment the watcher asks to leave — or the moment this
/// table's slot on the floor cycler runs out. Every idle loop must hold
/// through this rather than `sleep_ms`, or `Q` would do nothing until the
/// animation happened to end.
pub fn idle_hold(ms: u64) -> bool {
    let expired = || IDLE_UNTIL.with(|c| c.get()).is_some_and(|t| std::time::Instant::now() >= t);
    let mut left = 0;
    while left < ms {
        if ui::poll_leave_signal() || ui::quit_requested() {
            IDLE_LEFT.with(|c| c.set(true));
            return true;
        }
        if expired() {
            return true;
        }
        let slice = 40.min(ms - left);
        ui::sleep_ms(slice);
        left += slice;
    }
    if ui::poll_leave_signal() || ui::quit_requested() {
        IDLE_LEFT.with(|c| c.set(true));
        return true;
    }
    expired()
}

/// The dim strip every idle screen carries, so it is always obvious the
/// table is running itself and how to get out.
pub fn idle_footer(ctx: &mut Ctx, note: &str) {
    let theme = ctx.theme();
    ctx.screen.blank();
    ctx.screen.line(&theme.dim(&format!("  {note}")));
    ctx.screen.line(&widgets::footer(&theme, &[('q', "back to the floor")]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Store;

    #[test]
    fn a_losing_bet_moves_the_house_by_the_whole_stake() {
        let mut store = Store::blank();
        let mut rng = crate::rng::Rng::from_seed(1);
        let mut screen = crate::ui::Screen::headless();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        // 20 staked, nothing back.
        let delta = settle(&mut ctx, "probe", 20, 0);
        assert_eq!(delta, -20);
        assert_eq!(House::balance(ctx.store), 20);
        assert_eq!(ctx.store.get_i64("probe.losses", 0), 1);
    }

    #[test]
    fn a_push_moves_neither_side() {
        let mut store = Store::blank();
        let mut rng = crate::rng::Rng::from_seed(1);
        let mut screen = crate::ui::Screen::headless();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        let delta = settle(&mut ctx, "probe", 20, 20);
        assert_eq!(delta, 0);
        assert_eq!(House::balance(ctx.store), 0);
        assert_eq!(ctx.store.get_i64("probe.pushes", 0), 1);
    }

    #[test]
    fn a_win_pays_the_player_and_costs_the_house_the_same() {
        let mut store = Store::blank();
        let mut rng = crate::rng::Rng::from_seed(1);
        let mut screen = crate::ui::Screen::headless();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        // 10 staked, 60 back — the player is up 50.
        let delta = settle(&mut ctx, "probe", 10, 60);
        assert_eq!(delta, 50);
        assert_eq!(House::balance(ctx.store), -50);
        assert_eq!(ctx.store.get_i64("probe.wins", 0), 1);
        assert_eq!(ctx.store.get_i64("probe.best_win", 0), 50);
    }

    #[test]
    fn the_best_win_only_ever_climbs() {
        let mut store = Store::blank();
        let mut rng = crate::rng::Rng::from_seed(1);
        let mut screen = crate::ui::Screen::headless();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        settle(&mut ctx, "probe", 10, 110);
        settle(&mut ctx, "probe", 10, 30);
        assert_eq!(ctx.store.get_i64("probe.best_win", 0), 100);
    }
}
