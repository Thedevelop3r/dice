//! Pig — press-your-luck dice race. Classic (1d6) and Two-Dice variants.

use super::{Ctx, Difficulty, Player};
use crate::dice;
use crate::ui::{self, *};

pub struct Config {
    pub target: i64,
    pub two_dice: bool,
}

pub fn play(ctx: &mut Ctx, players: Vec<Player>, cfg: Config) {
    if let Some(w) = play_match(ctx, players, cfg, true) {
        let _ = w;
    }
}

/// Runs one match and returns the winning seat, or `None` if the human quit.
/// `record` controls whether the result lands in the saved statistics.
pub fn play_match(ctx: &mut Ctx, mut players: Vec<Player>, cfg: Config, record: bool) -> Option<usize> {
    ui::header(ctx.colors, "PIG");
    println!(
        "  First to {} wins. {}",
        color(ctx.colors, BOLD, &cfg.target.to_string()),
        if cfg.two_dice {
            "Two dice: a single 1 ends your turn, snake eyes wipes your score, doubles score double."
        } else {
            "One die: rolling a 1 ends your turn with nothing."
        }
    );

    let mut turn = 0usize;
    let winner = loop {
        let idx = turn % players.len();
        let turn_points = take_turn(ctx, &players, idx, &cfg);
        {
            let p = &mut players[idx];
            match turn_points {
                Outcome::Banked(n) => p.score += n,
                Outcome::Bust => {}
                Outcome::Wipe => p.score = 0,
                Outcome::Quit => return None,
            }
        }
        scoreboard(ctx.colors, &players, cfg.target);
        if players[idx].score >= cfg.target {
            break idx;
        }
        turn += 1;
    };

    announce(ctx, &players, winner, "pig", record);
    Some(winner)
}

/// Plays out an AI-only match with no output — used for tournament byes and
/// the branches of the bracket the human is not sitting at.
pub fn simulate(ctx: &mut Ctx, difficulties: &[Difficulty], cfg: &Config) -> usize {
    let mut scores = vec![0i64; difficulties.len()];
    let mut turn = 0usize;
    loop {
        let i = turn % difficulties.len();
        let best_other = scores.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, s)| *s).max().unwrap_or(0);
        let mut total = 0i64;
        loop {
            if !ai_rolls(difficulties[i], total, scores[i], best_other, cfg, ctx) {
                scores[i] += total;
                break;
            }
            let roll = dice::roll_n(ctx.rng, if cfg.two_dice { 2 } else { 1 }, 6);
            if cfg.two_dice {
                let (a, b) = (roll[0], roll[1]);
                if a == 1 && b == 1 {
                    scores[i] = 0;
                    break;
                }
                if a == 1 || b == 1 {
                    break;
                }
                total += if a == b { (a + b) as i64 * 2 } else { (a + b) as i64 };
            } else {
                if roll[0] == 1 {
                    break;
                }
                total += roll[0] as i64;
            }
        }
        if scores[i] >= cfg.target {
            return i;
        }
        turn += 1;
    }
}

enum Outcome {
    Banked(i64),
    Bust,
    Wipe,
    Quit,
}

fn take_turn(ctx: &mut Ctx, players: &[Player], idx: usize, cfg: &Config) -> Outcome {
    let me = &players[idx];
    let best_other = players
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, p)| p.score)
        .max()
        .unwrap_or(0);

    ui::header(ctx.colors, &format!("{}'s turn (score {})", me.name, me.score));
    let mut turn_total: i64 = 0;

    loop {
        let go_on = if let Some(d) = me.ai {
            let choice = ai_rolls(d, turn_total, me.score, best_other, cfg, ctx);
            println!(
                "  {} {}",
                color(ctx.colors, DIM, "AI thinks..."),
                color(ctx.colors, MAGENTA, if choice { "roll" } else { "hold" })
            );
            choice
        } else {
            let ans = ui::prompt(&format!(
                "  turn total {} — (r)oll, (h)old, (q)uit: ",
                color(ctx.colors, BOLD, &turn_total.to_string())
            ));
            match ans.to_lowercase().chars().next() {
                Some('r') | None => true,
                Some('h') => false,
                Some('q') => return Outcome::Quit,
                _ => {
                    println!("  ! r, h or q");
                    continue;
                }
            }
        };

        if !go_on {
            if turn_total == 0 {
                println!("  {}", color(ctx.colors, DIM, "held with nothing."));
            } else {
                println!("  {} {}", color(ctx.colors, GREEN, "banked"), turn_total);
            }
            ctx.store.bump("pig.turns", 1);
            return Outcome::Banked(turn_total);
        }

        let n = if cfg.two_dice { 2 } else { 1 };
        let roll = dice::roll_n(ctx.rng, n, 6);
        ui::draw_dice(&roll, None, ctx.colors);
        ctx.store.bump("pig.rolls", n as i64);

        if cfg.two_dice {
            let (a, b) = (roll[0], roll[1]);
            if a == 1 && b == 1 {
                println!("  {}", color(ctx.colors, RED, "SNAKE EYES — entire score wiped!"));
                ctx.store.bump("pig.busts", 1);
                return Outcome::Wipe;
            }
            if a == 1 || b == 1 {
                println!("  {}", color(ctx.colors, RED, &format!("a 1 — {turn_total} lost.")));
                ctx.store.bump("pig.busts", 1);
                return Outcome::Bust;
            }
            let gain = if a == b { (a + b) as i64 * 2 } else { (a + b) as i64 };
            if a == b {
                println!("  {}", color(ctx.colors, CYAN, &format!("doubles! +{gain}")));
            }
            turn_total += gain;
        } else {
            if roll[0] == 1 {
                println!("  {}", color(ctx.colors, RED, &format!("rolled a 1 — {turn_total} lost.")));
                ctx.store.bump("pig.busts", 1);
                return Outcome::Bust;
            }
            turn_total += roll[0] as i64;
        }

        println!("  turn total: {}", color(ctx.colors, BOLD, &turn_total.to_string()));

        if me.score + turn_total >= cfg.target && me.is_ai() {
            println!("  {}", color(ctx.colors, MAGENTA, "AI holds for the win."));
            return Outcome::Banked(turn_total);
        }
    }
}

/// Returns true when the AI wants to keep rolling.
fn ai_rolls(d: Difficulty, turn_total: i64, score: i64, best_other: i64, cfg: &Config, ctx: &mut Ctx) -> bool {
    if score + turn_total >= cfg.target {
        return false;
    }
    let base = match d {
        Difficulty::Easy => 12 + ctx.rng.below(10) as i64, // erratic and short-sighted
        Difficulty::Normal => 20,
        // "Hold at 25 minus what you've banked" — the classic near-optimal rule.
        Difficulty::Hard => (25 - score / 4).max(14),
    };
    let mut threshold = base.min(cfg.target - score);
    if d != Difficulty::Easy && best_other >= cfg.target - 15 {
        // Opponent is about to win: push harder.
        threshold = (cfg.target - score).max(threshold);
    }
    if cfg.two_dice {
        threshold += 4; // busts are rarer per roll, so ride longer
    }
    turn_total < threshold
}

pub fn scoreboard(colors: bool, players: &[Player], target: i64) {
    println!();
    for p in players {
        let width = 24usize;
        let filled = ((p.score.max(0) as f64 / target as f64) * width as f64).round() as usize;
        let filled = filled.min(width);
        let bar = format!("{}{}", "█".repeat(filled), "·".repeat(width - filled));
        let tag = if p.is_ai() { format!(" ({})", p.ai.unwrap().label()) } else { String::new() };
        println!(
            "  {:<14} {} {:>4}",
            format!("{}{}", p.name, tag),
            color(colors, if filled >= width { GREEN } else { CYAN }, &bar),
            p.score
        );
    }
}

fn announce(ctx: &mut Ctx, players: &[Player], winner: usize, game_key: &str, record: bool) {
    let w = &players[winner];
    println!();
    println!("  {}", color(ctx.colors, GREEN, &format!("★ {} wins with {}! ★", w.name, w.score)));
    if record {
        ctx.store.bump(&format!("{game_key}.games"), 1);
        if !w.is_ai() {
            ctx.store.bump(&format!("{game_key}.wins"), 1);
        }
        ctx.store.record_best(&format!("{game_key}.best"), w.score);
        let _ = ctx.store.save();
    }
    ui::pause();
}
