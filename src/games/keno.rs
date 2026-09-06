//! Keno — mark up to ten spots on an eighty-number board, then watch
//! twenty balls come out one at a time.
//!
//! Marking ten numbers out of eighty with single keypresses would be a
//! chore, so the board is quick-picked the way most keno terminals do it:
//! the player chooses how many spots to cover and can re-pick as often as
//! they like before committing. The paytable is the standard one, where
//! covering more spots pays far more for a full sweep and far less for a
//! near miss.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, widgets};

const KEY: &str = "keno";
const TITLE: &str = "KENO";

const BOARD: usize = 80;
const BALLS: usize = 20;
const MAX_SPOTS: usize = 10;

/// Gross multiplier on the stake for `hits` out of `picks` spots.
/// Anything not listed pays nothing.
fn payout_mult(picks: usize, hits: usize) -> i64 {
    match (picks, hits) {
        (1, 1) => 3,
        (2, 2) => 12,
        (3, 2) => 1,
        (3, 3) => 42,
        (4, 2) => 1,
        (4, 3) => 4,
        (4, 4) => 100,
        (5, 3) => 2,
        (5, 4) => 12,
        (5, 5) => 800,
        (6, 3) => 1,
        (6, 4) => 4,
        (6, 5) => 70,
        (6, 6) => 1_600,
        (7, 4) => 2,
        (7, 5) => 20,
        (7, 6) => 100,
        (7, 7) => 7_000,
        (8, 5) => 10,
        (8, 6) => 50,
        (8, 7) => 1_000,
        (8, 8) => 10_000,
        (9, 5) => 5,
        (9, 6) => 20,
        (9, 7) => 100,
        (9, 8) => 4_000,
        (9, 9) => 10_000,
        (10, 5) => 2,
        (10, 6) => 10,
        (10, 7) => 50,
        (10, 8) => 500,
        (10, 9) => 5_000,
        (10, 10) => 10_000,
        _ => 0,
    }
}

/// `count` distinct numbers from 1..=80, sorted so the board reads
/// naturally.
fn pick_spots(ctx: &mut Ctx, count: usize) -> Vec<usize> {
    let mut pool: Vec<usize> = (1..=BOARD).collect();
    for i in (1..pool.len()).rev() {
        let j = ctx.rng.below(i + 1);
        pool.swap(i, j);
    }
    let mut spots: Vec<usize> = pool.into_iter().take(count).collect();
    spots.sort_unstable();
    spots
}

fn draw_board(ctx: &mut Ctx, spots: &[usize], drawn: &[usize], headline: &str, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    if !headline.is_empty() {
        ctx.screen.line(headline);
        ctx.screen.blank();
    }
    for row in 0..8 {
        let mut line = String::from("  ");
        for col in 0..10 {
            let n = row * 10 + col + 1;
            let marked = spots.contains(&n);
            let hit = drawn.contains(&n);
            let cell = format!("{n:>3}");
            match (marked, hit) {
                // A marked number that has come out is the only thing on
                // this board worth looking at, so it is the only thing lit.
                (true, true) => theme.paint_into(&mut line, ui::theme::GREEN, &format!("[{}]", cell.trim_start())),
                (true, false) => theme.paint_into(&mut line, ui::theme::GOLD, &format!(" {cell}")),
                (false, true) => theme.paint_into(&mut line, ui::theme::CYAN, &format!(" {cell}")),
                (false, false) => theme.paint_into(&mut line, ui::theme::DIM, &format!(" {cell}")),
            }
            line.push(' ');
        }
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line(&theme.dim(&format!("  {} balls out of {BALLS}", drawn.len())));
    if !note.is_empty() {
        ctx.screen.line(&format!("  {note}"));
    }
}

/// Draws the twenty balls one at a time, lighting the board as they land.
fn draw_balls(ctx: &mut Ctx, spots: &[usize], headline: &str) -> Vec<usize> {
    let mut pool: Vec<usize> = (1..=BOARD).collect();
    for i in (1..pool.len()).rev() {
        let j = ctx.rng.below(i + 1);
        pool.swap(i, j);
    }
    let balls: Vec<usize> = pool.into_iter().take(BALLS).collect();
    let mut out: Vec<usize> = Vec::with_capacity(BALLS);
    for (i, b) in balls.iter().enumerate() {
        out.push(*b);
        let hits = spots.iter().filter(|s| out.contains(s)).count();
        let theme = ctx.theme();
        let note = if spots.contains(b) {
            theme.win(&format!("ball {} — {b} — that's yours ({hits} up)", i + 1))
        } else {
            theme.dim(&format!("ball {} — {b}", i + 1))
        };
        draw_board(ctx, spots, &out, headline, &note);
        ctx.screen.present();
        // Balls come faster as the draw goes on, the way a real caller
        // speeds up once the board is mostly full.
        ui::sleep_ms(if spots.contains(b) { 420 } else { 200_u64.saturating_sub(i as u64 * 6) });
    }
    out
}

pub fn play(ctx: &mut Ctx) {
    let mut picks = 4usize;
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }

        let Some(chosen) = widgets::number_picker(ctx.screen, 1, MAX_SPOTS as i64, picks as i64, 1, &[], |s, v| {
            let theme = s.theme;
            s.begin();
            ui::header(s, TITLE);
            s.blank();
            s.line(&format!("  bank: {}", theme.win(&format!("{bank} chips"))));
            s.blank();
            s.line("  how many spots do you want to cover?");
            s.blank();
            s.line(&format!("   {}", theme.paint(ui::theme::GOLD, &format!("{v}"))));
            s.blank();
            let mut wins: Vec<String> = Vec::new();
            for hits in 0..=(v as usize) {
                let m = payout_mult(v as usize, hits);
                if m > 0 {
                    wins.push(format!("{hits} hits {m}x"));
                }
            }
            s.line(&theme.dim(&format!("  pays: {}", wins.join(" · "))));
            s.blank();
            s.line(&widgets::footer(&theme, &[('↑', "more"), ('↓', "fewer"), ('\u{23ce}', "confirm"), ('\u{238b}', "leave")]));
        }) else {
            break;
        };
        picks = chosen as usize;

        // Quick-pick, with a re-pick loop before anything is staked.
        let spots = loop {
            let spots = pick_spots(ctx, picks);
            let theme = ctx.theme();
            let headline = format!("  {}", theme.dim("your board"));
            draw_board(ctx, &spots, &[], &headline, "");
            let theme = ctx.theme();
            ctx.screen.line(&widgets::footer(&theme, &[('y', "play this board"), ('r', "re-pick"), ('q', "leave")]));
            ctx.screen.present();
            match ui::choose_key(&['y', 'r', 'q'], 'y') {
                Some('y') => break Some(spots),
                Some('r') => continue,
                _ => break None,
            }
        };
        let Some(spots) = spots else { break };

        let label = format!("{} spots covered", spots.len());
        let Some(stake) = table::stake(ctx, TITLE, &label, bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let theme = ctx.theme();
        let headline = format!("  {} · {}", theme.dim(&label), theme.paint(ui::theme::GOLD, &format!("{stake} chips")));
        let drawn = draw_balls(ctx, &spots, &headline);

        let hits = spots.iter().filter(|s| drawn.contains(s)).count();
        let mult = payout_mult(spots.len(), hits);
        ctx.store.bump("keno.games", 1);
        ctx.store.bump("keno.hits", hits as i64);
        let delta = table::settle(ctx, KEY, stake, stake * mult);

        let theme = ctx.theme();
        let note = if mult > 0 {
            theme.win(&format!("{hits} of {} — pays {mult}x", spots.len()))
        } else {
            theme.lose(&format!("{hits} of {} — nothing this time", spots.len()))
        };
        draw_board(ctx, &spots, &drawn, &headline, &note);
        table::verdict(ctx, delta);
        if !table::again(ctx, "another board") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// The lounge board running draws on its own against a fixed demo ticket.
pub fn idle(ctx: &mut Ctx) {
    loop {
        let picks = 4 + ctx.rng.below(5);
        let spots = pick_spots(ctx, picks);
        let theme = ctx.theme();
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let headline = format!("  {} is covering {} spots", theme.accent(punter), theme.paint(ui::theme::GOLD, &picks.to_string()));

        let drawn = draw_balls(ctx, &spots, &headline);
        let hits = spots.iter().filter(|s| drawn.contains(s)).count();
        let mult = payout_mult(picks, hits);
        let theme = ctx.theme();
        let note = if mult > 0 {
            theme.win(&format!("{hits} of {picks} — the board pays {mult}x"))
        } else {
            theme.dim(&format!("{hits} of {picks} — no good"))
        };
        draw_board(ctx, &spots, &drawn, &headline, &note);
        table::idle_footer(ctx, "the lounge board draws on its own — nothing of yours is covered");
        ctx.screen.present();
        if table::idle_hold(2_600) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_pick_covers_distinct_numbers_on_the_board() {
        let mut store = crate::stats::Store::blank();
        let mut rng = crate::rng::Rng::from_seed(7);
        let mut screen = crate::ui::Screen::headless();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        for count in 1..=MAX_SPOTS {
            let spots = pick_spots(&mut ctx, count);
            assert_eq!(spots.len(), count);
            assert!(spots.iter().all(|n| (1..=BOARD).contains(n)), "off the board: {spots:?}");
            let mut sorted = spots.clone();
            sorted.dedup();
            assert_eq!(sorted.len(), count, "duplicate spot in {spots:?}");
            assert!(spots.windows(2).all(|w| w[0] < w[1]), "not sorted: {spots:?}");
        }
    }

    #[test]
    fn covering_every_spot_pays_the_top_prize() {
        assert_eq!(payout_mult(1, 1), 3);
        assert_eq!(payout_mult(5, 5), 800);
        assert_eq!(payout_mult(8, 8), 10_000);
        assert_eq!(payout_mult(10, 10), 10_000);
    }

    #[test]
    fn missing_everything_never_pays() {
        for picks in 1..=MAX_SPOTS {
            assert_eq!(payout_mult(picks, 0), 0, "covering {picks} and hitting none");
        }
    }

    #[test]
    fn more_hits_never_pay_less() {
        for picks in 1..=MAX_SPOTS {
            let mut best = 0;
            for hits in 0..=picks {
                let m = payout_mult(picks, hits);
                assert!(m >= best || m == 0, "covering {picks}, {hits} hits pays {m} after {best}");
                if m > 0 {
                    best = m;
                }
            }
        }
    }

    #[test]
    fn a_big_board_needs_more_hits_before_it_pays_at_all() {
        // Covering one spot pays on a single hit; covering ten needs five
        // before anything comes back. That asymmetry is the whole game.
        assert!(payout_mult(1, 1) > 0);
        assert_eq!(payout_mult(10, 4), 0);
        assert!(payout_mult(10, 5) > 0);
    }

    #[test]
    fn a_hit_count_above_the_spots_covered_is_impossible_and_pays_nothing() {
        assert_eq!(payout_mult(3, 4), 0);
        assert_eq!(payout_mult(10, 11), 0);
    }

    #[test]
    fn the_draw_produces_twenty_distinct_balls() {
        let mut store = crate::stats::Store::blank();
        let mut rng = crate::rng::Rng::from_seed(3);
        let mut screen = crate::ui::Screen::headless();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        // Reuse the same shuffle the caller uses, without the animation.
        let spots = pick_spots(&mut ctx, 4);
        assert_eq!(spots.len(), 4);
        let mut pool: Vec<usize> = (1..=BOARD).collect();
        for i in (1..pool.len()).rev() {
            let j = ctx.rng.below(i + 1);
            pool.swap(i, j);
        }
        let balls: Vec<usize> = pool.into_iter().take(BALLS).collect();
        assert_eq!(balls.len(), BALLS);
        let mut seen = std::collections::HashSet::new();
        assert!(balls.iter().all(|b| seen.insert(*b)), "a ball came out twice");
    }
}
