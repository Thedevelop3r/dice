//! Pig — press-your-luck dice race. Classic (1d6) and Two-Dice variants.

use super::{Ctx, Difficulty, Player};
use crate::ui::{self, dice_art, widgets};

pub struct Config {
    pub target: i64,
    pub two_dice: bool,
}

pub fn play(ctx: &mut Ctx, players: Vec<Player>, cfg: Config) {
    let _ = play_match(ctx, players, cfg, true);
}

/// Runs one match and returns the winning seat, or `None` if the human quit.
/// `record` controls whether the result lands in the saved statistics.
pub fn play_match(ctx: &mut Ctx, mut players: Vec<Player>, cfg: Config, record: bool) -> Option<usize> {
    let mut turn = 0usize;
    let winner = loop {
        let idx = turn % players.len();
        let turn_points = take_turn(ctx, &players, idx, &cfg);
        {
            let p = &mut players[idx];
            match &turn_points {
                Outcome::Banked(n) => p.score += *n,
                Outcome::Bust => {}
                Outcome::Wipe => p.score = 0,
                Outcome::Quit => return None,
            }
        }
        let done = players[idx].score >= cfg.target;
        end_of_turn(ctx, &players, cfg.target, idx, &turn_points, done);
        if done {
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
            let roll = crate::dice::roll_n(ctx.rng, if cfg.two_dice { 2 } else { 1 }, 6);
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

fn render_turn(ctx: &mut Ctx, players: &[Player], idx: usize, cfg: &Config, turn_total: i64, last_roll: Option<&[u32]>, note: &str) {
    let theme = ctx.theme();
    let me = &players[idx];
    ctx.screen.begin();
    ui::header(ctx.screen, &format!("PIG — {}'s turn", me.name));
    ctx.screen.blank();
    scoreboard_lines(ctx.screen, players, cfg.target);
    ctx.screen.blank();
    ctx.screen.line(&format!("  turn total: {}", theme.paint(ui::theme::BOLD, &turn_total.to_string())));
    if let Some(roll) = last_roll {
        ctx.screen.blank();
        for line in dice_art::dice_block(&theme, roll, None) {
            ctx.screen.line(&line);
        }
    }
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
    ctx.screen.blank();
    if me.is_ai() {
        ctx.screen.line(&theme.dim("the house AI is deciding..."));
    } else {
        ctx.screen.line(&widgets::footer(&theme, &[('r', "roll"), ('h', "hold"), ('q', "walk away")]));
    }
}

fn take_turn(ctx: &mut Ctx, players: &[Player], idx: usize, cfg: &Config) -> Outcome {
    let best_other = players.iter().enumerate().filter(|(i, _)| *i != idx).map(|(_, p)| p.score).max().unwrap_or(0);
    let mut turn_total: i64 = 0;
    let mut last_roll: Option<Vec<u32>> = None;

    loop {
        let is_ai = players[idx].ai.is_some();
        render_turn(ctx, players, idx, cfg, turn_total, last_roll.as_deref(), "");
        ctx.screen.present();

        let go_on = if let Some(d) = players[idx].ai {
            ui::sleep_ms(450);
            ai_rolls(d, turn_total, players[idx].score, best_other, cfg, ctx)
        } else {
            match ui::choose_key(&['r', 'h', 'q'], 'q') {
                Some('r') => true,
                Some('h') => false,
                _ => return Outcome::Quit,
            }
        };

        if !go_on {
            let note = if turn_total == 0 {
                "held with nothing.".to_string()
            } else {
                format!("banked {turn_total}.")
            };
            render_turn(ctx, players, idx, cfg, turn_total, last_roll.as_deref(), &note);
            ctx.screen.present();
            ui::sleep_ms(if is_ai { 500 } else { 0 });
            ctx.store.bump("pig.turns", 1);
            return Outcome::Banked(turn_total);
        }

        let n = if cfg.two_dice { 2 } else { 1 };
        let theme = ctx.theme();
        let seed = vec![1u32; n];
        let roll = dice_art::animate_roll(ctx.screen, ctx.rng, 6, &seed, &vec![false; n], |screen, values| {
            screen.begin();
            ui::header(screen, &format!("PIG — {}'s turn", players[idx].name));
            screen.blank();
            scoreboard_lines(screen, players, cfg.target);
            screen.blank();
            screen.line(&format!("  turn total: {}", theme.paint(ui::theme::BOLD, &turn_total.to_string())));
            screen.blank();
            for line in dice_art::dice_block(&theme, values, None) {
                screen.line(&line);
            }
            screen.blank();
            screen.line(&theme.dim("rolling..."));
        });
        ctx.store.bump("pig.rolls", n as i64);
        last_roll = Some(roll.clone());

        if cfg.two_dice {
            let (a, b) = (roll[0], roll[1]);
            if a == 1 && b == 1 {
                render_turn(ctx, players, idx, cfg, turn_total, last_roll.as_deref(), &ctx.theme().lose("SNAKE EYES — entire score wiped!"));
                ctx.screen.present();
                ui::sleep_ms(if is_ai { 700 } else { 0 });
                if !is_ai {
                    ui::pause(ctx.screen);
                }
                ctx.store.bump("pig.busts", 1);
                return Outcome::Wipe;
            }
            if a == 1 || b == 1 {
                render_turn(ctx, players, idx, cfg, turn_total, last_roll.as_deref(), &ctx.theme().lose(&format!("a 1 — {turn_total} lost.")));
                ctx.screen.present();
                if !is_ai {
                    ui::pause(ctx.screen);
                } else {
                    ui::sleep_ms(700);
                }
                ctx.store.bump("pig.busts", 1);
                return Outcome::Bust;
            }
            let gain = if a == b { (a + b) as i64 * 2 } else { (a + b) as i64 };
            turn_total += gain;
        } else {
            if roll[0] == 1 {
                render_turn(ctx, players, idx, cfg, turn_total, last_roll.as_deref(), &ctx.theme().lose(&format!("rolled a 1 — {turn_total} lost.")));
                ctx.screen.present();
                if !is_ai {
                    ui::pause(ctx.screen);
                } else {
                    ui::sleep_ms(700);
                }
                ctx.store.bump("pig.busts", 1);
                return Outcome::Bust;
            }
            turn_total += roll[0] as i64;
        }

        if players[idx].score + turn_total >= cfg.target && players[idx].is_ai() {
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
        threshold = (cfg.target - score).max(threshold);
    }
    if cfg.two_dice {
        threshold += 4;
    }
    turn_total < threshold
}

fn scoreboard_lines(screen: &mut crate::ui::Screen, players: &[Player], target: i64) {
    let theme = screen.theme;
    for p in players {
        let bar = widgets::bar(&theme, p.score, target, 24);
        let tag = if p.is_ai() { format!(" ({})", p.ai.unwrap().label()) } else { String::new() };
        screen.line(&format!("  {:<16} {} {:>4}", format!("{}{}", p.name, tag), bar, p.score));
    }
}

fn end_of_turn(ctx: &mut Ctx, players: &[Player], target: i64, idx: usize, outcome: &Outcome, done: bool) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, "SCOREBOARD");
    ctx.screen.blank();
    scoreboard_lines(ctx.screen, players, target);
    ctx.screen.blank();
    let note = match outcome {
        Outcome::Banked(n) if *n > 0 => format!("{} banked {n}.", players[idx].name),
        Outcome::Banked(_) => format!("{} held with nothing.", players[idx].name),
        Outcome::Bust => format!("{} busted.", players[idx].name),
        Outcome::Wipe => format!("{} was wiped out by snake eyes.", players[idx].name),
        Outcome::Quit => String::new(),
    };
    if !note.is_empty() {
        ctx.screen.line(&format!("  {note}"));
    }
    if done {
        for line in widgets::banner(&theme, &format!("{} WINS!", players[idx].name), true) {
            ctx.screen.line(&format!("  {line}"));
        }
    }
    ctx.screen.present();
    if players[idx].is_ai() {
        ui::sleep_ms(650);
    } else if !done {
        ui::sleep_ms(550);
    }
}

fn announce(ctx: &mut Ctx, players: &[Player], winner: usize, game_key: &str, record: bool) {
    let w = &players[winner];
    if record {
        ctx.store.bump(&format!("{game_key}.games"), 1);
        if !w.is_ai() {
            ctx.store.bump(&format!("{game_key}.wins"), 1);
        }
        ctx.store.record_best(&format!("{game_key}.best"), w.score);
        let _ = ctx.store.save();
    }
    ui::pause(ctx.screen);
}
