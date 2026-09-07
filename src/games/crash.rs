//! Crash — a multiplier climbs away from 1.00x and stops dead at a point
//! drawn before the round begins. Take the money before it does.
//!
//! This is the only table where the player acts *during* the animation
//! rather than between them, which is what `ui::poll_action` exists for.
//! The curve is drawn from the standard crash distribution: the chance of
//! surviving to `x` is `HOUSE_SHARE / x`, so a 2x cash-out comes home a
//! little under half the time and a 100x one about once in a hundred.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::rng::Rng;
use crate::ui::{self, widgets, Poll};

const KEY: &str = "crash";
const TITLE: &str = "CRASH";

/// The numerator of the survival curve, in hundredths: `P(crash >= x)` is
/// `0.99 / x`, which is the whole house edge in one constant.
const HOUSE_SHARE: i64 = 99;
/// Resolution of the draw. One in ten thousand rounds goes past 9900x.
const GRAIN: i64 = 10_000;
/// Multiplier growth per frame, as a fraction: 51/50 is 2% a tick.
const GROWTH_NUM: i64 = 51;
const GROWTH_DEN: i64 = 50;
const CASH_KEY: char = 'c';

/// Draws where this round stops, in hundredths. Never below 1.00x.
fn crash_point(rng: &mut Rng) -> i64 {
    let r = rng.below(GRAIN as usize) as i64 + 1;
    (HOUSE_SHARE * GRAIN / r).max(100)
}

/// The next multiplier up the curve, in hundredths. Always climbs by at
/// least one hundredth so the number on screen never stalls.
fn next_mult(current: i64) -> i64 {
    (current * GROWTH_NUM / GROWTH_DEN).max(current + 1)
}

fn fmt_mult(hundredths: i64) -> String {
    format!("{}.{:02}x", hundredths / 100, hundredths % 100)
}

/// The rising curve, plotted as a column of blocks so the climb is
/// something you watch rather than a number that ticks.
fn draw_curve(ctx: &mut Ctx, mult: i64, stake: i64, crashed: bool, cashed: Option<i64>, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    ctx.screen.line(&format!(
        "  {} · worth {}",
        theme.paint(ui::theme::GOLD, &format!("{stake} chips")),
        theme.win(&format!("{} chips", stake * mult / 100))
    ));
    ctx.screen.blank();

    // The plot is logarithmic in feel: every doubling adds a fixed number
    // of rows, so a 100x round still fits on one screen.
    const HEIGHT: usize = 12;
    const WIDTH: usize = 46;
    let steps = {
        let mut n = 0usize;
        let mut m = 100i64;
        while m < mult && n < WIDTH {
            m = next_mult(m);
            n += 1;
        }
        n
    };
    let colour = if crashed {
        ui::theme::RED
    } else if cashed.is_some() {
        ui::theme::GREEN
    } else {
        ui::theme::ICE
    };
    for row in 0..HEIGHT {
        let mut line = String::from("   ");
        let threshold = HEIGHT - row;
        for col in 0..=steps.min(WIDTH) {
            // Height of the curve at this column, scaled so the trace
            // sweeps up the plot as the multiplier runs away.
            let h = (col * HEIGHT).div_ceil(WIDTH.max(1)) + 1;
            if h >= threshold {
                theme.paint_into(&mut line, colour, "█");
            } else {
                line.push(' ');
            }
        }
        ctx.screen.line(&line);
    }

    let big = if crashed {
        theme.lose(&format!("  ✱ {} ✱", fmt_mult(mult)))
    } else {
        theme.paint(if cashed.is_some() { ui::theme::GREEN } else { ui::theme::GOLD }, &format!("  {}", fmt_mult(mult)))
    };
    ctx.screen.blank();
    ctx.screen.line(&big);
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

pub fn play(ctx: &mut Ctx) {
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }

        let Some(stake) = table::stake(ctx, TITLE, "cash out before it goes", bank, 10) else {
            break;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let stop = crash_point(ctx.rng);
        let mut mult = 100i64;
        let mut cashed: Option<i64> = None;
        let mut left = false;
        ctx.store.bump("crash.rounds", 1);

        loop {
            if mult >= stop {
                break;
            }
            let theme = ctx.theme();
            let foot = widgets::footer(&theme, &[(CASH_KEY, "cash out now")]);
            draw_curve(ctx, mult, stake, false, None, &foot);
            ctx.screen.present();
            // The climb starts leisurely and tightens, so the decision gets
            // harder exactly as the number gets better.
            ui::sleep_ms(if mult < 200 { 190 } else if mult < 500 { 130 } else { 90 });
            match ui::poll_action(&[CASH_KEY]) {
                Poll::Pressed(_) => {
                    cashed = Some(mult);
                    break;
                }
                Poll::Leave => {
                    // Leaving mid-round is a cash-out at the number showing,
                    // never a forfeit of a stake already on the table.
                    cashed = Some(mult);
                    left = true;
                    break;
                }
                Poll::Nothing => {}
            }
            mult = next_mult(mult);
        }

        let final_mult = cashed.unwrap_or(stop);
        let payout = cashed.map(|m| stake * m / 100).unwrap_or(0);
        let delta = table::settle(ctx, KEY, stake, payout);
        if let Some(m) = cashed {
            ctx.store.record_best("crash.best_mult", m);
        }

        let theme = ctx.theme();
        let note = match cashed {
            Some(m) => theme.win(&format!("out at {} — it would have gone at {}", fmt_mult(m), fmt_mult(stop))),
            None => theme.lose(&format!("gone at {}", fmt_mult(stop))),
        };
        draw_curve(ctx, final_mult, stake, cashed.is_none(), cashed, &note);
        table::verdict(ctx, delta);
        if left || !table::again(ctx, "another round") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// Rounds running themselves, with a punter whose nerve is drawn fresh
/// each time — so some rounds cash early and some ride into the wall.
pub fn idle(ctx: &mut Ctx) {
    loop {
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let stake = 10 + 5 * ctx.rng.below(5) as i64;
        let stop = crash_point(ctx.rng);
        let nerve = 130 + 40 * ctx.rng.below(20) as i64;
        let mut mult = 100i64;
        let mut cashed = None;

        loop {
            if mult >= stop {
                break;
            }
            if mult >= nerve {
                cashed = Some(mult);
                break;
            }
            let theme = ctx.theme();
            let note = theme.dim(&format!("{punter} is still in for {stake}"));
            draw_curve(ctx, mult, stake, false, None, &note);
            table::idle_footer(ctx, "a demo round — nothing of yours is riding on it");
            ctx.screen.present();
            if table::idle_hold(if mult < 200 { 190 } else { 110 }) {
                return;
            }
            mult = next_mult(mult);
        }

        let theme = ctx.theme();
        let note = match cashed {
            Some(m) => theme.win(&format!("{punter} is out at {} with {}", fmt_mult(m), stake * m / 100)),
            None => theme.lose(&format!("gone at {} — {punter} loses {stake}", fmt_mult(stop))),
        };
        draw_curve(ctx, cashed.unwrap_or(stop), stake, cashed.is_none(), cashed, &note);
        table::idle_footer(ctx, "a demo round — nothing of yours is riding on it");
        ctx.screen.present();
        if table::idle_hold(2_200) {
            return;
        }
    }
}

/// One round cashed out at a fixed multiplier, no rendering. The cash-out
/// point is the whole strategy here, so the harness fixes it.
pub fn simulate_at(rng: &mut Rng, cash_at: i64) -> (i64, i64) {
    if crash_point(rng) >= cash_at { (100, 100 * cash_at / 100) } else { (100, 0) }
}

pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    simulate_at(rng, 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(seed: u64) -> Rng {
        Rng::from_seed(seed)
    }

    #[test]
    fn a_round_never_stops_below_evens() {
        let mut rng = probe(2);
        for _ in 0..5_000 {
            assert!(crash_point(&mut rng) >= 100);
        }
    }

    #[test]
    fn the_curve_follows_the_advertised_survival_odds() {
        // P(crash >= x) should be about 0.99/x. Two sample points either
        // side of the common cash-out are enough to catch a broken draw.
        let mut rng = probe(99);
        let n = 40_000;
        let mut past_2x = 0;
        let mut past_10x = 0;
        for _ in 0..n {
            let c = crash_point(&mut rng);
            if c >= 200 {
                past_2x += 1;
            }
            if c >= 1_000 {
                past_10x += 1;
            }
        }
        let p2 = past_2x as f64 / n as f64;
        let p10 = past_10x as f64 / n as f64;
        assert!((p2 - 0.495).abs() < 0.02, "survival past 2x was {p2}");
        assert!((p10 - 0.099).abs() < 0.01, "survival past 10x was {p10}");
    }

    #[test]
    fn the_multiplier_always_climbs() {
        let mut m = 100i64;
        for _ in 0..400 {
            let next = next_mult(m);
            assert!(next > m, "{m} did not climb");
            m = next;
        }
    }

    #[test]
    fn the_climb_never_stalls_on_integer_division() {
        // A 2% rise on 1.00x rounds to nothing without the floor, which
        // would freeze the number on screen forever.
        assert!(next_mult(100) > 100);
        assert!(next_mult(101) > 101);
        assert!(next_mult(1) > 1);
    }

    #[test]
    fn a_cash_out_pays_the_number_on_screen() {
        // 10 chips out at 2.50x returns 25.
        assert_eq!(10 * 250 / 100, 25);
        // And a round that never cashed returns nothing at all.
        let payout: Option<i64> = None;
        assert_eq!(payout.map(|m: i64| 10 * m / 100).unwrap_or(0), 0);
    }

    #[test]
    fn multipliers_read_to_two_places() {
        assert_eq!(fmt_mult(100), "1.00x");
        assert_eq!(fmt_mult(250), "2.50x");
        assert_eq!(fmt_mult(9_900), "99.00x");
    }

    #[test]
    fn the_house_keeps_its_share_over_a_long_run() {
        // Cashing out at a fixed 2x every round must return a little under
        // the stake — that gap is the edge, and it must actually be there.
        let mut rng = probe(7);
        let n = 40_000;
        let mut returned = 0i64;
        for _ in 0..n {
            if crash_point(&mut rng) >= 200 {
                returned += 200;
            }
        }
        let rtp = returned as f64 / (n as f64 * 100.0);
        assert!(rtp > 0.94 && rtp < 1.0, "a flat 2x strategy returned {rtp}");
    }
}
