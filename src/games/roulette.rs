//! Roulette — single-zero wheel. Straight numbers pay 35:1; the outside
//! bets (red/black, odd/even, high/low, dozens) pay evens or 2:1.

use super::Ctx;
use crate::economy::{House, Wallet};
use crate::ui::{self, dice_art, widgets};

const RED_NUMBERS: [u32; 18] = [1, 3, 5, 7, 9, 12, 14, 16, 18, 19, 21, 23, 25, 27, 30, 32, 34, 36];

fn is_red(n: u32) -> bool {
    RED_NUMBERS.contains(&n)
}

#[derive(Clone, Copy)]
enum Bet {
    Straight(u32),
    Red,
    Black,
    Odd,
    Even,
    High,
    Low,
    Dozen(u8),
}

impl Bet {
    /// Net multiplier on the stake; negative means the stake is lost.
    fn payout(&self, n: u32) -> i64 {
        match self {
            Bet::Straight(x) => if n == *x { 35 } else { -1 },
            Bet::Red => if n != 0 && is_red(n) { 1 } else { -1 },
            Bet::Black => if n != 0 && !is_red(n) { 1 } else { -1 },
            Bet::Odd => if n != 0 && n % 2 == 1 { 1 } else { -1 },
            Bet::Even => if n != 0 && n.is_multiple_of(2) { 1 } else { -1 },
            Bet::High => if (19..=36).contains(&n) { 1 } else { -1 },
            Bet::Low => if (1..=18).contains(&n) { 1 } else { -1 },
            Bet::Dozen(d) => {
                let hit = match d {
                    1 => (1..=12).contains(&n),
                    2 => (13..=24).contains(&n),
                    _ => (25..=36).contains(&n),
                };
                if hit { 2 } else { -1 }
            }
        }
    }

    fn label(&self) -> String {
        match self {
            Bet::Straight(n) => format!("straight up on {n} (35:1)"),
            Bet::Red => "RED (evens)".into(),
            Bet::Black => "BLACK (evens)".into(),
            Bet::Odd => "ODD (evens)".into(),
            Bet::Even => "EVEN (evens)".into(),
            Bet::High => "19-36 HIGH (evens)".into(),
            Bet::Low => "1-18 LOW (evens)".into(),
            Bet::Dozen(1) => "1st dozen 1-12 (2:1)".into(),
            Bet::Dozen(2) => "2nd dozen 13-24 (2:1)".into(),
            Bet::Dozen(_) => "3rd dozen 25-36 (2:1)".into(),
        }
    }
}

pub fn play(ctx: &mut Ctx) {
    loop {
        {
            let mut w = Wallet::new(ctx.store);
            if w.chips() <= 0 {
                w.ensure_solvent(50);
            }
        }
        let bank = Wallet::new(ctx.store).chips();

        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, "ROULETTE");
        ctx.screen.blank();
        ctx.screen.line(&format!("  bank: {}", theme.win(&format!("{bank} chips"))));
        ctx.screen.blank();
        ctx.screen.line("  place your bet");
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&theme, &[('n', "number"), ('r', "red"), ('b', "black"), ('o', "odd"), ('e', "even")]));
        ctx.screen.line(&widgets::footer(&theme, &[('h', "1-18/19-36"), ('1', "…dozens…"), ('3', "3rd"), ('q', "leave")]));
        ctx.screen.present();

        let choice = ui::choose_key(&['n', 'r', 'b', 'o', 'e', 'h', 'l', '1', '2', '3', 'q'], 'q');
        let bet = match choice {
            Some('n') => {
                let Some(n) = widgets::number_picker(ctx.screen, 0, 36, 17, 1, &[], |s, v| {
                    let theme = s.theme;
                    s.begin();
                    ui::header(s, "ROULETTE — pick a number");
                    s.blank();
                    s.line(&format!("  {}", pocket_tag(&theme, v as u32)));
                    s.blank();
                    s.line(&widgets::footer(&theme, &[('↑', "+1"), ('↓', "-1"), ('\u{23ce}', "confirm")]));
                }) else {
                    continue;
                };
                Bet::Straight(n as u32)
            }
            Some('r') => Bet::Red,
            Some('b') => Bet::Black,
            Some('o') => Bet::Odd,
            Some('e') => Bet::Even,
            Some('h') => Bet::High,
            Some('l') => Bet::Low,
            Some('1') => Bet::Dozen(1),
            Some('2') => Bet::Dozen(2),
            Some('3') => Bet::Dozen(3),
            _ => break,
        };

        let Some(stake) = widgets::number_picker(ctx.screen, 1, bank.max(1), 10.min(bank.max(1)), 5, &[('m', bank.max(1))], |s, v| {
            let theme = s.theme;
            s.begin();
            ui::header(s, "ROULETTE");
            s.blank();
            s.line(&format!("  betting on {}", bet.label()));
            s.blank();
            s.line(&format!("  stake: {}", theme.paint(ui::theme::GOLD, &format!("{v} chips"))));
            s.blank();
            s.line(&widgets::footer(&theme, &[('↑', "+5"), ('↓', "-5"), ('m', "max"), ('\u{23ce}', "spin")]));
        }) else {
            continue;
        };

        Wallet::new(ctx.store).spend_chips(stake);
        let landed = spin(ctx, &bet, stake);
        ctx.store.bump("roulette.spins", 1);

        let mult = bet.payout(landed);
        let delta = stake * mult;
        if delta > 0 {
            Wallet::new(ctx.store).add_chips(delta + stake);
            ctx.store.bump("roulette.wins", 1);
        } else {
            ctx.store.bump("roulette.losses", 1);
        }
        House::record(ctx.store, "roulette", delta);
        let _ = ctx.store.save();

        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, "ROULETTE");
        ctx.screen.blank();
        for line in big_pocket(&theme, landed) {
            ctx.screen.line(&line);
        }
        ctx.screen.blank();
        ctx.screen.line(&format!("  you bet on {}", bet.label()));
        if delta > 0 {
            ctx.screen.line(&theme.win(&format!("win +{delta} chips")));
        } else {
            ctx.screen.line(&theme.lose(&format!("lose {} chips", -delta)));
        }
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&theme, &[('y', "spin again"), ('n', "leave the table")]));
        ctx.screen.present();
        if !ui::confirm_key(true) {
            break;
        }
    }

    let final_chips = Wallet::new(ctx.store).chips();
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, "ROULETTE");
    ctx.screen.blank();
    ctx.screen.line(&format!("  cashing out with {}", theme.win(&format!("{final_chips} chips"))));
    ctx.screen.present();
    ui::pause(ctx.screen);
}

fn pocket_color(n: u32) -> &'static str {
    if n == 0 {
        crate::ui::theme::GREEN
    } else if is_red(n) {
        crate::ui::theme::RED
    } else {
        crate::ui::theme::CHIP_BLACK
    }
}

fn pocket_tag(theme: &ui::Theme, n: u32) -> String {
    theme.paint(pocket_color(n), &format!("● {n:>2}"))
}

fn big_pocket(theme: &ui::Theme, n: u32) -> Vec<String> {
    let code = pocket_color(n);
    let label = format!("{n:^5}");
    vec![
        theme.paint(code, "╔═════╗"),
        theme.paint(code, &format!("║{label}║")),
        theme.paint(code, "╚═════╝"),
    ]
}

/// Spins the wheel — a decelerating flicker over ~1s before landing, same
/// pacing as every dice reveal in the app.
fn spin(ctx: &mut Ctx, bet: &Bet, stake: i64) -> u32 {
    let delays = dice_art::standard_frames();
    let final_n = ctx.rng.below(37) as u32;
    let last = delays.len().saturating_sub(1);
    let mut landed = final_n;
    for (i, &delay) in delays.iter().enumerate() {
        let n = if i == last { final_n } else { ctx.rng.below(37) as u32 };
        landed = n;
        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, "ROULETTE");
        ctx.screen.blank();
        ctx.screen.line(&format!("  {stake} chips on {}", bet.label()));
        ctx.screen.blank();
        for line in big_pocket(&theme, n) {
            ctx.screen.line(&line);
        }
        ctx.screen.blank();
        ctx.screen.line(&theme.dim("the wheel spins..."));
        ctx.screen.present();
        ui::sleep_ms(delay);
    }
    landed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_loses_every_outside_bet() {
        for bet in [Bet::Red, Bet::Black, Bet::Odd, Bet::Even, Bet::High, Bet::Low] {
            assert_eq!(bet.payout(0), -1);
        }
    }

    #[test]
    fn straight_up_pays_thirty_five_to_one() {
        assert_eq!(Bet::Straight(17).payout(17), 35);
        assert_eq!(Bet::Straight(17).payout(18), -1);
    }

    #[test]
    fn red_and_black_partition_the_nonzero_numbers() {
        for n in 1..=36u32 {
            assert_eq!(Bet::Red.payout(n) == 1, is_red(n));
            assert_eq!(Bet::Black.payout(n) == 1, !is_red(n));
        }
        assert_eq!(RED_NUMBERS.len(), 18);
    }

    #[test]
    fn dozens_cover_disjoint_thirds() {
        assert_eq!(Bet::Dozen(1).payout(12), 2);
        assert_eq!(Bet::Dozen(1).payout(13), -1);
        assert_eq!(Bet::Dozen(2).payout(13), 2);
        assert_eq!(Bet::Dozen(3).payout(36), 2);
    }
}
