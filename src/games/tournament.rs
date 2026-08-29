//! Knockout tournament: pay a chip buy-in, get drawn into a single-elimination
//! bracket of Pig matches, and play up the bracket for the prize pool.

use super::{pig, Ctx, Difficulty, Player};
use crate::economy::Wallet;
use crate::ui::{self, *};

const BOT_NAMES: [&str; 7] = ["Ada", "Blitz", "Cricket", "Domino", "Echo", "Fable", "Gambit"];

struct Entrant {
    name: String,
    ai: Option<Difficulty>,
}

pub fn play(ctx: &mut Ctx) {
    ui::header(ctx.colors, "TOURNAMENT");
    let (chips, dollars) = {
        let w = Wallet::new(ctx.store);
        (w.chips(), w.dollars())
    };
    println!("  Single-elimination Pig. Winner takes 70% of the pool, runner-up 30%.");
    println!("  you hold {}", ui::money(ctx.colors, chips, dollars));

    println!("\n   1. Local  —  10 chip buy-in, 4 players");
    println!("   2. Open   —  25 chip buy-in, 8 players");
    println!("   3. High Roller — 100 chip buy-in, 8 players, Hard field");
    let tier = ui::prompt_usize("  choose a bracket", 1, 3, 1);
    let (buy_in, field, hard_field) = match tier {
        2 => (25i64, 8usize, false),
        3 => (100, 8, true),
        _ => (10, 4, false),
    };

    if !Wallet::new(ctx.store).spend_chips(buy_in) {
        println!("  {}", color(ctx.colors, RED, "not enough chips for that buy-in."));
        ui::pause();
        return;
    }
    let pool = buy_in * field as i64;
    let target = if tier == 3 { 75 } else { 50 };
    println!(
        "  {} — prize pool {}",
        color(ctx.colors, DIM, &format!("paid {buy_in} chips")),
        color(ctx.colors, YELLOW, &format!("{pool} chips"))
    );

    let name = ctx.store.get_str("player.name", "Player");
    let mut bracket: Vec<Entrant> = vec![Entrant { name: name.clone(), ai: None }];
    for i in 0..field - 1 {
        let d = if hard_field {
            Difficulty::Hard
        } else {
            Difficulty::from_index(1 + ctx.rng.below(3))
        };
        bracket.push(Entrant { name: BOT_NAMES[i % BOT_NAMES.len()].to_string(), ai: Some(d) });
    }
    // Shuffle the draw so the human is not always in the first seat.
    for i in (1..bracket.len()).rev() {
        let j = ctx.rng.below(i + 1);
        bracket.swap(i, j);
    }

    let mut round = 1;
    // Set to 1 when the human loses the final, i.e. finishes as runner-up.
    let mut human_out_at: Option<usize> = None;
    while bracket.len() > 1 {
        let stage = match bracket.len() {
            2 => "FINAL".to_string(),
            4 => "SEMI-FINALS".to_string(),
            n => format!("ROUND {round} — {n} players"),
        };
        ui::header(ctx.colors, &stage);
        print_bracket(ctx.colors, &bracket);

        let mut survivors: Vec<Entrant> = Vec::new();
        for pair in bracket.chunks(2) {
            if pair.len() == 1 {
                println!("  {} gets a bye.", pair[0].name);
                survivors.push(Entrant { name: pair[0].name.clone(), ai: pair[0].ai });
                continue;
            }
            let human_seat = pair.iter().position(|e| e.ai.is_none());
            let cfg = pig::Config { target, two_dice: false };
            let winner = if let Some(_seat) = human_seat {
                println!(
                    "\n  {} {} vs {}",
                    color(ctx.colors, BOLD, "YOUR MATCH:"),
                    pair[0].name,
                    pair[1].name
                );
                ui::pause();
                let players: Vec<Player> = pair
                    .iter()
                    .map(|e| match e.ai {
                        Some(d) => Player::bot(&e.name, d),
                        None => Player::human(&e.name),
                    })
                    .collect();
                match pig::play_match(ctx, players, cfg, false) {
                    Some(w) => w,
                    None => {
                        println!("  {}", color(ctx.colors, DIM, "you walked away from the table."));
                        ui::pause();
                        return;
                    }
                }
            } else {
                let diffs: Vec<Difficulty> = pair.iter().map(|e| e.ai.unwrap()).collect();
                let w = pig::simulate(ctx, &diffs, &cfg);
                println!(
                    "  {} vs {} → {}",
                    pair[0].name,
                    pair[1].name,
                    color(ctx.colors, CYAN, &pair[w].name)
                );
                w
            };
            if human_seat.is_some() && human_seat != Some(winner) {
                human_out_at = Some(bracket.len() / 2);
            }
            survivors.push(Entrant { name: pair[winner].name.clone(), ai: pair[winner].ai });
        }
        bracket = survivors;
        round += 1;
    }

    let champion = &bracket[0];
    ui::header(ctx.colors, "RESULT");
    println!("  🏆 {}", color(ctx.colors, YELLOW, &format!("{} takes the tournament", champion.name)));

    let mut w = Wallet::new(ctx.store);
    if champion.ai.is_none() {
        let prize = pool * 70 / 100;
        w.add_chips(prize);
        println!("  {}", color(ctx.colors, GREEN, &format!("you win {prize} chips!")));
        ctx.store.bump("tourney.titles", 1);
        ctx.store.record_best("tourney.best_prize", prize);
    } else if human_out_at == Some(1) {
        let prize = pool * 30 / 100;
        w.add_chips(prize);
        println!("  {}", color(ctx.colors, CYAN, &format!("runner-up — you collect {prize} chips.")));
    } else {
        println!("  {}", color(ctx.colors, DIM, "knocked out. Better draw next time."));
    }
    ctx.store.bump("tourney.entries", 1);
    let _ = ctx.store.save();
    ui::pause();
}

fn print_bracket(colors: bool, bracket: &[Entrant]) {
    for pair in bracket.chunks(2) {
        let a = &pair[0];
        let b = pair.get(1);
        let fmt = |e: &Entrant| match e.ai {
            Some(d) => format!("{} ({})", e.name, d.label()),
            None => format!("{} (you)", e.name),
        };
        match b {
            Some(b) => println!("   {:<22} vs  {}", fmt(a), fmt(b)),
            None => println!("   {:<22} {}", fmt(a), color(colors, DIM, "bye")),
        }
    }
}
