//! Mines — twenty-five tiles, a few of them loaded. Turn over as many as
//! your nerve allows, then take the money before you turn over the wrong
//! one.
//!
//! The multiplier is not a table: it is the actual reciprocal of the odds
//! of having got this far, trimmed by `EDGE`. So choosing more mines
//! genuinely pays more for the same number of picks, and the game needs no
//! balancing pass to stay honest.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, widgets};

const KEY: &str = "mines";
const TITLE: &str = "MINES";

const SIDE: usize = 5;
const GRID: usize = SIDE * SIDE;
/// The house's cut, in hundredths, applied once to the fair price.
const EDGE: i64 = 97;
/// Tiles are picked by letter, a through y — one keypress per tile.
const FIRST_TILE: u8 = b'a';
/// Not a tile letter, so it can safely mean "cash out".
const CASH_KEY: char = '0';

/// The fair multiplier for surviving `picks` turns against `mines` mines,
/// in hundredths, less the house edge. With no picks yet there is nothing
/// to multiply.
fn multiplier(mines: usize, picks: usize) -> i64 {
    let safe = GRID - mines;
    if picks == 0 || picks > safe {
        return 100;
    }
    let mut m: i64 = 100;
    for i in 0..picks {
        m = m * (GRID - i) as i64 / (safe - i) as i64;
    }
    m * EDGE / 100
}

fn fmt_mult(hundredths: i64) -> String {
    format!("{}.{:02}x", hundredths / 100, hundredths % 100)
}

fn tile_key(i: usize) -> char {
    (FIRST_TILE + i as u8) as char
}

fn lay_mines(ctx: &mut Ctx, mines: usize) -> Vec<bool> {
    let mut field = vec![false; GRID];
    let mut placed = 0;
    while placed < mines {
        let i = ctx.rng.below(GRID);
        if !field[i] {
            field[i] = true;
            placed += 1;
        }
    }
    field
}

/// The three numbers that describe a round in progress. Bundled so the
/// draw call reads as a picture of a round rather than a list of loose
/// integers.
#[derive(Debug, Clone, Copy)]
struct Round {
    stake: i64,
    mines: usize,
    picks: usize,
}

fn draw_grid(ctx: &mut Ctx, field: &[bool], opened: &[bool], reveal: bool, round: Round, note: &str, footer: Option<String>) {
    let Round { stake, mines, picks } = round;
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    let mult = multiplier(mines, picks);
    ctx.screen.line(&format!(
        "  {} · {} mines · {} safe · running {} worth {}",
        theme.paint(ui::theme::GOLD, &format!("{stake} chips")),
        theme.lose(&mines.to_string()),
        theme.accent(&picks.to_string()),
        theme.accent(&fmt_mult(mult)),
        theme.win(&format!("{}", stake * mult / 100))
    ));
    ctx.screen.blank();
    for row in 0..SIDE {
        let mut line = String::from("   ");
        for col in 0..SIDE {
            let i = row * SIDE + col;
            if opened[i] {
                theme.paint_into(&mut line, ui::theme::GREEN, "  ◆  ");
            } else if reveal && field[i] {
                theme.paint_into(&mut line, ui::theme::RED, "  ✱  ");
            } else if reveal {
                theme.paint_into(&mut line, ui::theme::DIM, "  ·  ");
            } else {
                theme.paint_into(&mut line, ui::theme::GOLD_DIM, &format!("  {}  ", tile_key(i)));
            }
        }
        ctx.screen.line(&line);
    }
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
    if let Some(f) = footer {
        ctx.screen.blank();
        ctx.screen.line(&f);
    }
}

pub fn play(ctx: &mut Ctx) {
    let mut mines = 3usize;
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }

        let Some(chosen) = widgets::number_picker(ctx.screen, 1, (GRID - 1) as i64, mines as i64, 1, &[], |s, v| {
            let theme = s.theme;
            s.begin();
            ui::header(s, TITLE);
            s.blank();
            s.line(&format!("  bank: {}", theme.win(&format!("{bank} chips"))));
            s.blank();
            s.line("  how many mines do you want under there?");
            s.blank();
            s.line(&format!("   {}", theme.lose(&format!("{v}"))));
            s.blank();
            let m = v as usize;
            s.line(&theme.dim(&format!(
                "  first safe tile pays {} · five in a row pays {}",
                fmt_mult(multiplier(m, 1)),
                fmt_mult(multiplier(m, 5.min(GRID - m)))
            )));
            s.blank();
            s.line(&widgets::footer(&theme, &[('↑', "more"), ('↓', "fewer"), ('\u{23ce}', "confirm"), ('\u{238b}', "leave")]));
        }) else {
            break;
        };
        mines = chosen as usize;

        let Some(stake) = table::stake(ctx, TITLE, &format!("{mines} mines under twenty-five tiles"), bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let field = lay_mines(ctx, mines);
        let mut opened = vec![false; GRID];
        let mut picks = 0usize;
        let mut hit = false;
        ctx.store.bump("mines.rounds", 1);

        while picks < GRID - mines {
            let theme = ctx.theme();
            let mut keys: Vec<char> = (0..GRID).filter(|i| !opened[*i]).map(tile_key).collect();
            let banked = stake * multiplier(mines, picks) / 100;
            let cash_label = format!("take {banked} chips");
            let mut hints: Vec<(char, &str)> = vec![('a', "…"), ('y', "turn a tile")];
            if picks > 0 {
                keys.push(CASH_KEY);
                hints.push((CASH_KEY, &cash_label));
            }
            let foot = widgets::footer(&theme, &hints);
            let note = theme.dim("pick a letter");
            draw_grid(ctx, &field, &opened, false, Round { stake, mines, picks }, &note, Some(foot));
            ctx.screen.present();

            let Some(c) = ui::choose_key(&keys, CASH_KEY) else { break };
            if c == CASH_KEY {
                break;
            }
            let i = (c as u8 - FIRST_TILE) as usize;
            if i >= GRID || opened[i] {
                continue;
            }
            if field[i] {
                hit = true;
                break;
            }
            opened[i] = true;
            picks += 1;
            let theme = ctx.theme();
            let note = theme.win(&format!("clear — {}", fmt_mult(multiplier(mines, picks))));
            draw_grid(ctx, &field, &opened, false, Round { stake, mines, picks }, &note, None);
            ctx.screen.present();
            ui::sleep_ms(240);
        }

        let payout = if hit { 0 } else { stake * multiplier(mines, picks) / 100 };
        let delta = table::settle(ctx, KEY, stake, payout);
        if !hit {
            ctx.store.record_best("mines.best_run", picks as i64);
        }

        let theme = ctx.theme();
        let note = if hit {
            theme.lose(&format!("a mine on pick {} — the lot goes", picks + 1))
        } else {
            theme.win(&format!("walked with {} tiles clear at {}", picks, fmt_mult(multiplier(mines, picks))))
        };
        draw_grid(ctx, &field, &opened, true, Round { stake, mines, picks }, &note, None);
        table::verdict(ctx, delta);
        if !table::again(ctx, "another field") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// A field being turned over by a punter who always stops at a target
/// multiplier — so some runs cash and some go bang.
pub fn idle(ctx: &mut Ctx) {
    loop {
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let mines = 2 + ctx.rng.below(5);
        let stake = 10 + 5 * ctx.rng.below(5) as i64;
        let field = lay_mines(ctx, mines);
        let mut opened = vec![false; GRID];
        let mut picks = 0usize;
        let mut hit = false;
        let target = 150 + 100 * ctx.rng.below(6) as i64;

        loop {
            let closed: Vec<usize> = (0..GRID).filter(|i| !opened[*i]).collect();
            if closed.is_empty() || multiplier(mines, picks) >= target {
                break;
            }
            let theme = ctx.theme();
            let note = theme.dim(&format!("{punter} is {} tiles in", picks));
            draw_grid(ctx, &field, &opened, false, Round { stake, mines, picks }, &note, None);
            table::idle_footer(ctx, "a demo field — none of these tiles are yours");
            ctx.screen.present();
            if table::idle_hold(750) {
                return;
            }
            let i = closed[ctx.rng.below(closed.len())];
            if field[i] {
                hit = true;
                break;
            }
            opened[i] = true;
            picks += 1;
        }

        let theme = ctx.theme();
        let note = if hit {
            theme.lose(&format!("{punter} found one on pick {}", picks + 1))
        } else {
            theme.win(&format!("{punter} takes {} at {}", stake * multiplier(mines, picks) / 100, fmt_mult(multiplier(mines, picks))))
        };
        draw_grid(ctx, &field, &opened, true, Round { stake, mines, picks }, &note, None);
        table::idle_footer(ctx, "a demo field — none of these tiles are yours");
        ctx.screen.present();
        if table::idle_hold(2_400) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_turned_over_is_worth_nothing_extra() {
        for mines in 1..GRID {
            assert_eq!(multiplier(mines, 0), 100, "{mines} mines, no picks");
        }
    }

    #[test]
    fn every_safe_tile_raises_the_price() {
        for mines in [1usize, 3, 5, 10, 20] {
            let safe = GRID - mines;
            let mut last = 100;
            for picks in 1..=safe {
                let m = multiplier(mines, picks);
                // With a single mine the first tile is very nearly a
                // certainty, so once the edge is taken it is worth the
                // stake and no more — the price may hold there, but it
                // must never fall.
                assert!(m >= last, "{mines} mines: pick {picks} pays {m}, down from {last}");
                last = m;
            }
            assert!(
                multiplier(mines, safe) > multiplier(mines, 1),
                "{mines} mines: clearing the field is worth no more than one tile"
            );
        }
    }

    #[test]
    fn more_mines_pay_more_for_the_same_risk_taken() {
        for picks in 1..=5 {
            let mut last = 0;
            for mines in 1..=10 {
                let m = multiplier(mines, picks);
                assert!(m > last, "{picks} picks: {mines} mines pays {m}, not more than {last}");
                last = m;
            }
        }
    }

    #[test]
    fn the_price_sits_below_the_fair_odds() {
        // The fair multiplier is the reciprocal of surviving this far; the
        // table must always pay a little less than that.
        for mines in [1usize, 3, 8] {
            let safe = GRID - mines;
            for picks in 1..=safe.min(8) {
                let mut fair: i64 = 100;
                for i in 0..picks {
                    fair = fair * (GRID - i) as i64 / (safe - i) as i64;
                }
                assert!(multiplier(mines, picks) <= fair, "{mines} mines, {picks} picks");
            }
        }
    }

    #[test]
    fn clearing_the_whole_field_is_the_biggest_prize() {
        for mines in [1usize, 3, 5] {
            let safe = GRID - mines;
            let full = multiplier(mines, safe);
            for picks in 1..safe {
                assert!(multiplier(mines, picks) < full);
            }
        }
    }

    #[test]
    fn the_field_holds_exactly_the_mines_asked_for() {
        let mut store = crate::stats::Store::blank();
        let mut rng = crate::rng::Rng::from_seed(21);
        let mut screen = crate::ui::Screen::headless();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        for mines in [1usize, 3, 12, 24] {
            let field = lay_mines(&mut ctx, mines);
            assert_eq!(field.len(), GRID);
            assert_eq!(field.iter().filter(|m| **m).count(), mines, "{mines} mines asked for");
        }
    }

    #[test]
    fn every_tile_has_its_own_letter() {
        let keys: Vec<char> = (0..GRID).map(tile_key).collect();
        assert_eq!(keys[0], 'a');
        assert_eq!(keys[GRID - 1], 'y');
        let mut sorted = keys.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), GRID);
        assert!(!keys.contains(&CASH_KEY), "the cash-out key must not also be a tile");
    }
}
