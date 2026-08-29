//! Knockout tournament: pay a chip buy-in, get drawn into a single-elimination
//! bracket of Pig matches, and play up the bracket for the prize pool.

use super::{pig, Ctx, Difficulty, Player};
use crate::economy::{House, Wallet};
use crate::ui::{self, widgets};

const BOT_NAMES: [&str; 7] = ["Ada", "Blitz", "Cricket", "Domino", "Echo", "Fable", "Gambit"];

struct Entrant {
    name: String,
    ai: Option<Difficulty>,
}

pub fn play(ctx: &mut Ctx) {
    let theme = ctx.theme();
    let (chips, dollars) = {
        let w = Wallet::new(ctx.store);
        (w.chips(), w.dollars())
    };
    ctx.screen.begin();
    ui::header(ctx.screen, "TOURNAMENT");
    ctx.screen.blank();
    ctx.screen.line("  single-elimination Pig. Winner takes 70% of the pool, runner-up 30%.");
    ctx.screen.line(&format!("  you hold {}", ui::money(&theme, chips, dollars)));
    ctx.screen.blank();
    ctx.screen.line(&format!("  {} 10 chip buy-in, 4 players", theme.paint(ui::theme::GOLD, "[1] Local")));
    ctx.screen.line(&format!("  {} 25 chip buy-in, 8 players", theme.paint(ui::theme::GOLD, "[2] Open")));
    ctx.screen.line(&format!("  {} 100 chip buy-in, 8 players, Hard field", theme.paint(ui::theme::GOLD, "[3] High Roller")));
    ctx.screen.line(&widgets::footer(&theme, &[('q', "back")]));
    ctx.screen.present();
    let tier = match ui::choose_key(&['1', '2', '3', 'q'], 'q') {
        Some('2') => 2,
        Some('3') => 3,
        Some('1') => 1,
        _ => return,
    };
    let (buy_in, field, hard_field) = match tier {
        2 => (25i64, 8usize, false),
        3 => (100, 8, true),
        _ => (10, 4, false),
    };

    if !Wallet::new(ctx.store).spend_chips(buy_in) {
        message(ctx, &theme.lose("not enough chips for that buy-in."));
        return;
    }
    let pool = buy_in * field as i64;
    let target = if tier == 3 { 75 } else { 50 };
    ctx.store.bump("tourney.entries", 1);
    message(ctx, &format!("paid {buy_in} chips — prize pool {}", theme.paint(ui::theme::YELLOW, &format!("{pool} chips"))));

    let name = ctx.store.get_str("player.name", "Player");
    let mut bracket: Vec<Entrant> = vec![Entrant { name: name.clone(), ai: None }];
    for i in 0..field - 1 {
        let d = if hard_field { Difficulty::Hard } else { Difficulty::from_index(1 + ctx.rng.below(3)) };
        bracket.push(Entrant { name: BOT_NAMES[i % BOT_NAMES.len()].to_string(), ai: Some(d) });
    }
    for i in (1..bracket.len()).rev() {
        let j = ctx.rng.below(i + 1);
        bracket.swap(i, j);
    }

    let mut round = 1;
    let mut human_out_at: Option<usize> = None;
    while bracket.len() > 1 {
        let stage = match bracket.len() {
            2 => "FINAL".to_string(),
            4 => "SEMI-FINALS".to_string(),
            n => format!("ROUND {round} — {n} players"),
        };
        draw_bracket(ctx, &stage, &bracket);

        let mut survivors: Vec<Entrant> = Vec::new();
        for pair in bracket.chunks(2) {
            if pair.len() == 1 {
                survivors.push(Entrant { name: pair[0].name.clone(), ai: pair[0].ai });
                continue;
            }
            let human_seat = pair.iter().position(|e| e.ai.is_none());
            let cfg = pig::Config { target, two_dice: false };
            let winner = if human_seat.is_some() {
                message(ctx, &theme.bold(&format!("YOUR MATCH: {} vs {}", pair[0].name, pair[1].name)));
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
                        // The buy-in was already spent and there's no
                        // refund for walking away mid-bracket — the house
                        // keeps the whole entry fee.
                        House::record(ctx.store, "tourney", -buy_in);
                        let _ = ctx.store.save();
                        message(ctx, &theme.dim("you walked away from the table."));
                        return;
                    }
                }
            } else {
                let diffs: Vec<Difficulty> = pair.iter().map(|e| e.ai.unwrap()).collect();
                let w = pig::simulate(ctx, &diffs, &cfg);
                message(ctx, &format!("{} vs {} → {}", pair[0].name, pair[1].name, theme.paint(ui::theme::CYAN, &pair[w].name)));
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
    ctx.screen.begin();
    ui::header(ctx.screen, "RESULT");
    ctx.screen.blank();
    ctx.screen.line(&format!("  🏆 {}", theme.paint(ui::theme::YELLOW, &format!("{} takes the tournament", champion.name))));

    let champion_is_human = champion.ai.is_none();
    let prize = if champion_is_human {
        let prize = pool * 70 / 100;
        Wallet::new(ctx.store).add_chips(prize);
        ctx.screen.line(&theme.win(&format!("you win {prize} chips!")));
        ctx.store.bump("tourney.titles", 1);
        ctx.store.record_best("tourney.best_prize", prize);
        prize
    } else if human_out_at == Some(1) {
        let prize = pool * 30 / 100;
        Wallet::new(ctx.store).add_chips(prize);
        ctx.screen.line(&theme.paint(ui::theme::CYAN, &format!("runner-up — you collect {prize} chips.")));
        prize
    } else {
        ctx.screen.line(&theme.dim("knocked out. Better draw next time."));
        0
    };
    // The buy-in is already spent; the round's net for the player is
    // whatever prize came back from it, if any.
    House::record(ctx.store, "tourney", prize - buy_in);
    let _ = ctx.store.save();
    ctx.screen.present();
    ui::pause(ctx.screen);
}

fn draw_bracket(ctx: &mut Ctx, stage: &str, bracket: &[Entrant]) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, stage);
    ctx.screen.blank();
    for pair in bracket.chunks(2) {
        let a = &pair[0];
        let b = pair.get(1);
        let fmt = |e: &Entrant| match e.ai {
            Some(d) => format!("{} ({})", e.name, d.label()),
            None => format!("{} (you)", e.name),
        };
        match b {
            Some(b) => ctx.screen.line(&format!("   {:<22} vs  {}", fmt(a), fmt(b))),
            None => ctx.screen.line(&format!("   {:<22} {}", fmt(a), theme.dim("bye"))),
        }
    }
    ctx.screen.present();
    ui::sleep_ms(500);
}

fn message(ctx: &mut Ctx, text: &str) {
    ctx.screen.blank();
    ctx.screen.line(&format!("  {text}"));
    ctx.screen.present();
    ui::pause(ctx.screen);
}
