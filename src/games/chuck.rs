//! Chuck-a-Luck — a three-dice wagering game with a persistent bankroll.

use super::{table, Ctx};
use crate::economy::{House, Wallet};
use crate::rng::Rng;
use crate::ui::{self, dice_art, widgets};

const BAILOUT: i64 = 50;

#[derive(Clone, Copy)]
enum Bet {
    Number(u32),
    High,
    Low,
    Triple,
}

impl Bet {
    /// Net multiplier applied to the stake (negative means the stake is lost).
    fn payout(&self, roll: &[u32]) -> i64 {
        let sum: u32 = roll.iter().sum();
        let triple = roll.windows(2).all(|w| w[0] == w[1]);
        match self {
            Bet::Number(n) => {
                let hits = roll.iter().filter(|v| *v == n).count() as i64;
                if hits == 0 { -1 } else { hits }
            }
            Bet::High => {
                if triple { -1 } else if sum >= 11 { 1 } else { -1 }
            }
            Bet::Low => {
                if triple { -1 } else if sum <= 10 { 1 } else { -1 }
            }
            Bet::Triple => if triple { 30 } else { -1 },
        }
    }

    fn label(&self) -> String {
        match self {
            Bet::Number(n) => format!("number {n} (pays per hit)"),
            Bet::High => "HIGH 11-17 (evens)".into(),
            Bet::Low => "LOW 4-10 (evens)".into(),
            Bet::Triple => "any triple (30:1)".into(),
        }
    }
}

pub fn play(ctx: &mut Ctx) {
    loop {
        let bailed = {
            let mut w = Wallet::new(ctx.store);
            w.chips() <= 0 && w.ensure_solvent(BAILOUT)
        };
        if bailed {
            message(ctx, &format!("broke — the house stakes you {BAILOUT} chips."));
        }
        let bank = Wallet::new(ctx.store).chips();

        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, "CHUCK-A-LUCK");
        ctx.screen.blank();
        ctx.screen.line(&format!("  bank: {}", theme.win(&format!("{bank} chips"))));
        ctx.screen.blank();
        ctx.screen.line("  three dice — back a number, high/low, or any triple.");
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(
            &theme,
            &[('1', "…"), ('6', "number"), ('h', "high"), ('l', "low"), ('t', "triple"), ('q', "leave")],
        ));
        ctx.screen.present();

        let choice = ui::choose_key(&['1', '2', '3', '4', '5', '6', 'h', 'l', 't', 'q'], 'q');
        let bet = match choice {
            Some('h') => Bet::High,
            Some('l') => Bet::Low,
            Some('t') => Bet::Triple,
            Some(c) if c.is_ascii_digit() => Bet::Number(c.to_digit(10).unwrap()),
            _ => break,
        };

        let Some(stake) = widgets::number_picker(ctx.screen, 1, bank.max(1), 10.min(bank.max(1)), 5, &[('m', bank.max(1))], |s, v| {
            let theme = s.theme;
            s.begin();
            ui::header(s, "CHUCK-A-LUCK");
            s.blank();
            s.line(&format!("  bet on {}", bet.label()));
            s.blank();
            s.line(&format!("  stake: {}", theme.paint(ui::theme::GOLD, &format!("{v} chips"))));
            s.blank();
            s.line(&widgets::footer(&theme, &[('↑', "+5"), ('↓', "-5"), ('m', "max"), ('\u{23ce}', "roll")]));
        }) else {
            continue;
        };

        let seed = vec![1u32; 3];
        let roll = dice_art::animate_roll(ctx.screen, ctx.rng, 6, &seed, &[false, false, false], |screen, values| {
            let theme = screen.theme;
            screen.begin();
            ui::header(screen, "CHUCK-A-LUCK");
            screen.blank();
            screen.line(&format!("  {stake} chips on {}", bet.label()));
            screen.blank();
            for line in dice_art::dice_block(&theme, values, None) {
                screen.line(&line);
            }
        });
        ctx.store.bump("chuck.rolls", 3);

        let mult = bet.payout(&roll);
        let delta = stake * mult;
        Wallet::new(ctx.store).add_chips(delta);
        House::record(ctx.store, "chuck", delta);
        if delta >= 0 {
            ctx.store.bump("chuck.wins", 1);
        } else {
            ctx.store.bump("chuck.losses", 1);
        }
        let _ = ctx.store.save();

        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, "CHUCK-A-LUCK");
        ctx.screen.blank();
        for line in dice_art::dice_block(&theme, &roll, None) {
            ctx.screen.line(&line);
        }
        ctx.screen.blank();
        if delta >= 0 {
            ctx.screen.line(&theme.win(&format!("win +{delta} chips")));
        } else {
            ctx.screen.line(&theme.lose(&format!("lose {delta} chips")));
        }
        ctx.screen.present();
        ui::pause(ctx.screen);

        if Wallet::new(ctx.store).chips() <= 0 {
            ctx.screen.begin();
            ui::header(ctx.screen, "CHUCK-A-LUCK");
            ctx.screen.blank();
            ctx.screen.line(&theme.lose("you're cleaned out."));
            ctx.screen.line(&widgets::footer(&theme, &[('y', "keep playing"), ('n', "cash out")]));
            ctx.screen.present();
            if !ui::confirm_key(true) {
                break;
            }
        }
    }
    let final_chips = Wallet::new(ctx.store).chips();
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, "CHUCK-A-LUCK");
    ctx.screen.blank();
    ctx.screen.line(&format!("  cashing out with {}", theme.win(&format!("{final_chips} chips"))));
    ctx.screen.present();
    ui::pause(ctx.screen);
}

fn message(ctx: &mut Ctx, text: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, "CHUCK-A-LUCK");
    ctx.screen.blank();
    ctx.screen.line(&format!("  {}", theme.dim(text)));
    ctx.screen.present();
    ui::sleep_ms(600);
}

/// The table rolling on its own, with a fictional punter working the
/// board. Nothing here touches the player's wallet or the house ledger —
/// it is a spectacle, not a wager.
pub fn idle(ctx: &mut Ctx) {
    let board = [Bet::High, Bet::Low, Bet::Triple, Bet::Number(2), Bet::Number(4), Bet::Number(6)];
    loop {
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let bet = board[ctx.rng.below(board.len())];
        let stake = 10 + 5 * ctx.rng.below(5) as i64;
        let theme = ctx.theme();
        let headline = format!("  {} backs {} for {stake}", theme.accent(punter), theme.paint(ui::theme::GOLD, &bet.label()));

        let seed = vec![1u32; 3];
        let roll = dice_art::animate_roll(ctx.screen, ctx.rng, 6, &seed, &[false, false, false], |screen, values| {
            let theme = screen.theme;
            screen.begin();
            ui::header(screen, "CHUCK-A-LUCK");
            screen.blank();
            screen.line(&headline);
            screen.blank();
            for line in dice_art::dice_block(&theme, values, None) {
                screen.line(&line);
            }
        });

        let won = stake * bet.payout(&roll);
        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, "CHUCK-A-LUCK");
        ctx.screen.blank();
        ctx.screen.line(&headline);
        ctx.screen.blank();
        for line in dice_art::dice_block(&theme, &roll, None) {
            ctx.screen.line(&line);
        }
        ctx.screen.blank();
        ctx.screen.line(&format!(
            "  {}",
            if won >= 0 {
                theme.win(&format!("{punter} collects {won}"))
            } else {
                theme.lose(&format!("{punter} is down {}", -won))
            }
        ));
        table::idle_footer(ctx, "the table rolls itself — no stake of yours is on the board");
        ctx.screen.present();
        if table::idle_hold(2_200) {
            return;
        }
    }
}

/// Every bet the board offers, for the audit harness.
/// The bets the audit harness walks, addressed by index so this module's
/// private `Bet` never has to leave it.
pub const BETS: usize = 4;

fn bet(i: usize) -> Bet {
    [Bet::High, Bet::Low, Bet::Triple, Bet::Number(6)][i % BETS]
}

pub fn bet_name(i: usize) -> String {
    bet(i).label()
}

/// One roll against a given bet, no rendering.
pub fn simulate_at(rng: &mut Rng, which: usize) -> (i64, i64) {
    let bet = bet(which);
    let roll: Vec<u32> = (0..3).map(|_| rng.roll(6)).collect();
    let stake = 100;
    // Chuck-a-Luck states a *net* multiplier, so the gross back is the
    // stake plus it — and a losing bet returns nothing rather than less.
    (stake, (stake + stake * bet.payout(&roll)).max(0))
}

#[cfg(test)]
pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    simulate_at(rng, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bet_on_the_board_keeps_a_house_edge() {
        // Three dice is 216 outcomes, so every bet can be priced exactly.
        for bet in [Bet::High, Bet::Low, Bet::Triple, Bet::Number(1), Bet::Number(3), Bet::Number(6)] {
            let mut net = 0i64;
            for a in 1..=6u32 {
                for b in 1..=6u32 {
                    for c in 1..=6u32 {
                        net += bet.payout(&[a, b, c]);
                    }
                }
            }
            let rtp = 1.0 + net as f64 / 216.0;
            assert!(rtp < 1.0, "{} returns {rtp:.4} — that bet is a giveaway", bet.label());
            assert!(rtp > 0.70, "{} returns {rtp:.4} — that bet is daylight robbery", bet.label());
        }
    }
}
