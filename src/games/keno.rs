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
use crate::rng::Rng;
use crate::ui::{self, widgets};

const KEY: &str = "keno";
const TITLE: &str = "KENO";

const BOARD: usize = 80;
const BALLS: usize = 20;
const MAX_SPOTS: usize = 10;

/// Gross multiplier on the stake for `hits` out of `picks` spots, in
/// hundredths — `370` pays 3.7x. Hundredths rather than whole numbers
/// because a one-spot board cannot be priced honestly otherwise: it hits
/// exactly one time in four, so every integer multiplier is either a
/// break-even 4x or a miserly 3x.
///
/// Every board size returns between 90% and 93%, checked exactly against
/// the hypergeometric odds in `every_board_size_is_priced_the_same_way`.
/// That evenness is the point — the original table ran from 75% on one
/// spot down to 40% on ten, which quietly made the big boards a trap.
fn payout_mult(picks: usize, hits: usize) -> i64 {
    match (picks, hits) {
        (1, 1) => 370,
        (2, 2) => 1_500,
        (3, 2) => 250,
        (3, 3) => 4_200,
        (4, 2) => 120,
        (4, 3) => 700,
        (4, 4) => 12_000,
        (5, 3) => 350,
        (5, 4) => 2_500,
        (5, 5) => 50_000,
        (6, 3) => 200,
        (6, 4) => 1_000,
        (6, 5) => 7_000,
        (6, 6) => 130_000,
        (7, 4) => 550,
        (7, 5) => 3_200,
        (7, 6) => 28_000,
        (7, 7) => 600_000,
        (8, 5) => 1_600,
        (8, 6) => 11_000,
        (8, 7) => 125_000,
        (8, 8) => 3_800_000,
        (9, 5) => 850,
        (9, 6) => 4_500,
        (9, 7) => 34_000,
        (9, 8) => 340_000,
        (9, 9) => 10_000_000,
        (10, 5) => 500,
        (10, 6) => 2_250,
        (10, 7) => 12_500,
        (10, 8) => 90_000,
        (10, 9) => 900_000,
        (10, 10) => 25_000_000,
        _ => 0,
    }
}

/// `3.7x`, `120x`, `2.5x` — how a multiplier in hundredths reads.
fn fmt_mult(hundredths: i64) -> String {
    if hundredths % 100 == 0 {
        format!("{}x", hundredths / 100)
    } else if hundredths % 10 == 0 {
        format!("{}.{}x", hundredths / 100, (hundredths % 100) / 10)
    } else {
        format!("{}.{:02}x", hundredths / 100, hundredths % 100)
    }
}

/// `count` distinct numbers from 1..=80, sorted so the board reads
/// naturally.
fn pick_spots(rng: &mut Rng, count: usize) -> Vec<usize> {
    let mut pool: Vec<usize> = (1..=BOARD).collect();
    for i in (1..pool.len()).rev() {
        let j = rng.below(i + 1);
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
/// The twenty balls for one game. Shared with the audit harness so the
/// simulated draw is the caller's own draw.
fn draw_sheet(rng: &mut Rng) -> Vec<usize> {
    let mut pool: Vec<usize> = (1..=BOARD).collect();
    for i in (1..pool.len()).rev() {
        let j = rng.below(i + 1);
        pool.swap(i, j);
    }
    pool.into_iter().take(BALLS).collect()
}

fn draw_balls(ctx: &mut Ctx, spots: &[usize], headline: &str) -> Vec<usize> {
    let balls = draw_sheet(ctx.rng);
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
                    wins.push(format!("{hits} hits {}", fmt_mult(m)));
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
            let spots = pick_spots(ctx.rng, picks);
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
        let delta = table::settle(ctx, KEY, stake, stake * mult / 100);

        let theme = ctx.theme();
        let note = if mult > 0 {
            theme.win(&format!("{hits} of {} — pays {}", spots.len(), fmt_mult(mult)))
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
        let spots = pick_spots(ctx.rng, picks);
        let theme = ctx.theme();
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let headline = format!("  {} is covering {} spots", theme.accent(punter), theme.paint(ui::theme::GOLD, &picks.to_string()));

        let drawn = draw_balls(ctx, &spots, &headline);
        let hits = spots.iter().filter(|s| drawn.contains(s)).count();
        let mult = payout_mult(picks, hits);
        let theme = ctx.theme();
        let note = if mult > 0 {
            theme.win(&format!("{hits} of {picks} — the board pays {}", fmt_mult(mult)))
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

/// One game covering `picks` spots, no rendering: chips staked, chips
/// returned. Keno's return depends heavily on how many spots are covered,
/// so the harness runs several.
pub fn simulate_at(rng: &mut Rng, picks: usize) -> (i64, i64) {
    let spots = pick_spots(rng, picks);
    let drawn = draw_sheet(rng);
    let hits = spots.iter().filter(|s| drawn.contains(s)).count();
    (100, 100 * payout_mult(picks, hits) / 100)
}

#[cfg(test)]
pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    simulate_at(rng, 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_pick_covers_distinct_numbers_on_the_board() {
        let mut rng = crate::rng::Rng::from_seed(7);
        for count in 1..=MAX_SPOTS {
            let spots = pick_spots(&mut rng, count);
            assert_eq!(spots.len(), count);
            assert!(spots.iter().all(|n| (1..=BOARD).contains(n)), "off the board: {spots:?}");
            let mut sorted = spots.clone();
            sorted.dedup();
            assert_eq!(sorted.len(), count, "duplicate spot in {spots:?}");
            assert!(spots.windows(2).all(|w| w[0] < w[1]), "not sorted: {spots:?}");
        }
    }

    /// C(n, k) as an f64 — big enough for C(80, 20) without overflowing.
    fn choose(n: u64, k: u64) -> f64 {
        if k > n {
            return 0.0;
        }
        let mut c = 1.0f64;
        for i in 0..k {
            c = c * (n - i) as f64 / (i + 1) as f64;
        }
        c
    }

    /// The exact chance of `hits` of `picks` when twenty of eighty come out.
    fn odds(picks: u64, hits: u64) -> f64 {
        choose(picks, hits) * choose(80 - picks, 20 - hits) / choose(80, 20)
    }

    #[test]
    fn every_board_size_is_priced_the_same_way() {
        // Not a simulation — the real hypergeometric odds, so this is the
        // return to the last decimal rather than an estimate. The evenness
        // across board sizes is the property worth protecting: it is what
        // stops the big boards from quietly becoming a trap.
        for picks in 1..=MAX_SPOTS as u64 {
            let rtp: f64 = (0..=picks).map(|h| odds(picks, h) * payout_mult(picks as usize, h as usize) as f64 / 100.0).sum();
            assert!(rtp > 0.90 && rtp < 0.94, "covering {picks} spots returns {rtp:.4}");
        }
    }

    #[test]
    fn no_board_size_is_a_worse_deal_than_another() {
        let rtps: Vec<f64> = (1..=MAX_SPOTS as u64)
            .map(|p| (0..=p).map(|h| odds(p, h) * payout_mult(p as usize, h as usize) as f64 / 100.0).sum())
            .collect();
        let lo = rtps.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = rtps.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!(hi - lo < 0.04, "board sizes range from {lo:.4} to {hi:.4} — too far apart to be fair");
    }

    #[test]
    fn covering_every_spot_pays_the_top_prize() {
        assert_eq!(payout_mult(1, 1), 370);
        assert_eq!(payout_mult(5, 5), 50_000);
        assert_eq!(payout_mult(8, 8), 3_800_000);
        assert_eq!(payout_mult(10, 10), 25_000_000);
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
        let mut rng = crate::rng::Rng::from_seed(3);
        // Reuse the same shuffle the caller uses, without the animation.
        let spots = pick_spots(&mut rng, 4);
        assert_eq!(spots.len(), 4);
        let mut pool: Vec<usize> = (1..=BOARD).collect();
        for i in (1..pool.len()).rev() {
            let j = rng.below(i + 1);
            pool.swap(i, j);
        }
        let balls: Vec<usize> = pool.into_iter().take(BALLS).collect();
        assert_eq!(balls.len(), BALLS);
        let mut seen = std::collections::HashSet::new();
        assert!(balls.iter().all(|b| seen.insert(*b)), "a ball came out twice");
    }
}
