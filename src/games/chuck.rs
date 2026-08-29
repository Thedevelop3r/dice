//! Chuck-a-Luck — a three-dice wagering game with a persistent bankroll.

use super::Ctx;
use crate::dice;
use crate::economy::Wallet;
use crate::ui::{self, *};

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
    ui::header(ctx.colors, "CHUCK-A-LUCK");
    println!("  Three dice. Back a number, the high/low halves, or any triple.");

    loop {
        let bank = {
            let mut w = Wallet::new(ctx.store);
            if w.chips() <= 0 && w.ensure_solvent(BAILOUT) {
                println!("  {}", color(ctx.colors, DIM, &format!("broke — the house stakes you {BAILOUT} chips.")));
            }
            w.chips()
        };
        println!();
        println!("  bank: {}", color(ctx.colors, GREEN, &format!("{bank} chips")));
        let choice = ui::prompt("  bet on (1-6 = number, h = high, l = low, t = triple, q = quit): ");
        let low = choice.to_lowercase();
        let bet = match low.chars().next() {
            Some('q') | None => break,
            Some('h') => Bet::High,
            Some('l') => Bet::Low,
            Some('t') => Bet::Triple,
            Some(c) if c.is_ascii_digit() && ('1'..='6').contains(&c) => {
                Bet::Number(c.to_digit(10).unwrap())
            }
            _ => {
                println!("  ! 1-6, h, l, t or q");
                continue;
            }
        };

        let stake = ui::prompt_usize("  stake", 1, bank.max(1) as usize, 10.min(bank.max(1) as usize)) as i64;
        println!("  {} on {}", color(ctx.colors, BOLD, &format!("{stake} chips")), bet.label());

        let roll = dice::roll_n(ctx.rng, 3, 6);
        ui::draw_dice(&roll, None, ctx.colors);
        ctx.store.bump("chuck.rolls", 3);

        let mult = bet.payout(&roll);
        let delta = stake * mult;
        Wallet::new(ctx.store).add_chips(delta);
        if delta >= 0 {
            println!("  {}", color(ctx.colors, GREEN, &format!("win +{delta} chips")));
            ctx.store.bump("chuck.wins", 1);
        } else {
            println!("  {}", color(ctx.colors, RED, &format!("lose {delta} chips")));
            ctx.store.bump("chuck.losses", 1);
        }

        let _ = ctx.store.save();

        if Wallet::new(ctx.store).chips() <= 0 {
            println!("  {}", color(ctx.colors, RED, "you're cleaned out."));
            if !ui::confirm("  keep playing?") {
                break;
            }
        }
    }
    let final_chips = Wallet::new(ctx.store).chips();
    println!("  cashing out with {final_chips} chips.");
    ui::pause();
}
