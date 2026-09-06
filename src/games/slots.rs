//! Slots — three weighted reels that stop left to right.
//!
//! The reels are real strips rather than a payout table with a random
//! number bolted on: each symbol appears on the strip as many times as its
//! weight, and a spin lands on a uniformly chosen stop. That is how a
//! physical machine works, and it means the odds are visible in `strip()`
//! instead of hidden in a probability constant.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::rng::Rng;
use crate::ui::{self, widgets};

const KEY: &str = "slots";
const TITLE: &str = "SLOTS";

#[derive(Clone, Copy)]
struct Symbol {
    face: &'static str,
    /// How many places this symbol takes up on the strip.
    weight: usize,
    /// Gross multiplier on the stake for three of these on the payline.
    three: i64,
}

/// Faces are all three columns wide so the reel window never shifts.
const SYMBOLS: [Symbol; 6] = [
    Symbol { face: " 7 ", weight: 1, three: 120 },
    Symbol { face: " ★ ", weight: 2, three: 60 },
    Symbol { face: " ♦ ", weight: 3, three: 35 },
    Symbol { face: " ♠ ", weight: 4, three: 20 },
    Symbol { face: " ● ", weight: 5, three: 12 },
    Symbol { face: "BAR", weight: 6, three: 8 },
];

/// The seven is the wild-ish top symbol: it pays on its own even when the
/// line does not match, which is what keeps a losing spin interesting.
const SEVEN: usize = 0;

/// The reel strip: every symbol repeated to its weight, dealt round-robin
/// so the rare faces sit apart rather than clumping into one arc.
fn strip() -> Vec<usize> {
    let total: usize = SYMBOLS.iter().map(|s| s.weight).sum();
    let mut left: Vec<usize> = SYMBOLS.iter().map(|s| s.weight).collect();
    let mut out = Vec::with_capacity(total);
    while out.len() < total {
        for (i, remaining) in left.iter_mut().enumerate() {
            if *remaining > 0 {
                out.push(i);
                *remaining -= 1;
                if out.len() == total {
                    break;
                }
            }
        }
    }
    out
}

/// Gross multiplier on the stake — 0 means the spin returned nothing.
fn payout_mult(line: [usize; 3]) -> i64 {
    if line[0] == line[1] && line[1] == line[2] {
        return SYMBOLS[line[0]].three;
    }
    match line.iter().filter(|s| **s == SEVEN).count() {
        2 => 5,
        1 => 2,
        _ => 0,
    }
}

/// What a settled or spinning reel shows: the symbol on the payline plus
/// its neighbours above and below.
fn window(strip: &[usize], pos: usize) -> [usize; 3] {
    let n = strip.len();
    [strip[(pos + n - 1) % n], strip[pos % n], strip[(pos + 1) % n]]
}

fn draw_machine(ctx: &mut Ctx, strip: &[usize], pos: [usize; 3], stake: i64, note: &str, credits: Option<i64>) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    match credits {
        Some(c) => ctx.screen.line(&format!("  credits: {}", theme.win(&format!("{c} chips")))),
        None => ctx.screen.line(&format!("  stake: {}", theme.paint(ui::theme::GOLD, &format!("{stake} chips")))),
    }
    ctx.screen.blank();

    let windows: Vec<[usize; 3]> = pos.iter().map(|p| window(strip, *p)).collect();
    let top = format!("  {}", ["┌─────┐"; 3].join(" "));
    ctx.screen.line(&theme.paint(ui::theme::GOLD_DIM, &top));
    for row in 0..3 {
        let mut line = String::from("  ");
        for w in &windows {
            let face = SYMBOLS[w[row]].face;
            line.push_str(&theme.paint(ui::theme::GOLD_DIM, "│"));
            if row == 1 {
                // The payline is the only row that pays, so it is the only
                // one drawn bright.
                theme.paint_into(&mut line, ui::theme::GOLD, &format!(" {face} "));
            } else {
                theme.paint_into(&mut line, ui::theme::DIM, &format!(" {face} "));
            }
            line.push_str(&theme.paint(ui::theme::GOLD_DIM, "│"));
            line.push(' ');
        }
        if row == 1 {
            line.push_str(&theme.dim("  ◄ payline"));
        }
        ctx.screen.line(&line);
    }
    let bottom = format!("  {}", ["└─────┘"; 3].join(" "));
    ctx.screen.line(&theme.paint(ui::theme::GOLD_DIM, &bottom));
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

/// Where the three reels stop. Shared with the audit harness so the
/// simulated odds are the machine's own odds, not a second opinion.
fn draw_stops(rng: &mut Rng, n: usize) -> [usize; 3] {
    [rng.below(n), rng.below(n), rng.below(n)]
}

/// Spins the reels, stopping them left to right, and reports the payline.
fn spin(ctx: &mut Ctx, strip: &[usize], stake: i64, credits: Option<i64>) -> [usize; 3] {
    let n = strip.len();
    let stops = draw_stops(ctx.rng, n);
    // Each reel runs a little longer than the one to its left; the frame
    // delay grows throughout so the whole machine eases to a stop.
    let halt = [16usize, 24, 33];
    let frames = 36;
    let start = ctx.rng.below(n);
    for f in 0..frames {
        let mut pos = [0usize; 3];
        for (i, p) in pos.iter_mut().enumerate() {
            *p = if f >= halt[i] { stops[i] } else { (start + f * (2 + i)) % n };
        }
        let note = if f >= halt[2] { "" } else { "spinning..." };
        draw_machine(ctx, strip, pos, stake, note, credits);
        ctx.screen.present();
        let delay = 28 + (f as u64 * f as u64) / 22;
        ui::sleep_ms(delay);
    }
    [strip[stops[0]], strip[stops[1]], strip[stops[2]]]
}

/// The settled frame: the payline alone, without the dim neighbours the
/// spinning window shows, so the result reads unambiguously.
fn draw_settled(ctx: &mut Ctx, line: [usize; 3], top: &str, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    ctx.screen.line(top);
    ctx.screen.blank();
    ctx.screen.line(&theme.paint(ui::theme::GOLD_DIM, "  ┌─────┐ ┌─────┐ ┌─────┐"));
    let mut mid = String::from("  ");
    for s in line.iter() {
        mid.push_str(&theme.paint(ui::theme::GOLD_DIM, "│"));
        theme.paint_into(&mut mid, ui::theme::GOLD, &format!(" {} ", SYMBOLS[*s].face));
        mid.push_str(&theme.paint(ui::theme::GOLD_DIM, "│"));
        mid.push(' ');
    }
    ctx.screen.line(&mid);
    ctx.screen.line(&theme.paint(ui::theme::GOLD_DIM, "  └─────┘ └─────┘ └─────┘"));
    ctx.screen.blank();
    ctx.screen.line(&format!("  {note}"));
}

/// How a settled line reads out loud.
fn describe(line: [usize; 3], mult: i64) -> String {
    if mult == 0 {
        "no line".to_string()
    } else if line[0] == line[1] && line[1] == line[2] {
        format!("three {} — {mult}x", SYMBOLS[line[0]].face.trim())
    } else {
        format!("sevens pay — {mult}x")
    }
}

fn paytable(ctx: &mut Ctx) {
    let theme = ctx.theme();
    ctx.screen.line(&theme.dim("  three of a kind pays:"));
    let mut line = String::from("  ");
    for s in SYMBOLS.iter() {
        line.push_str(&theme.paint(ui::theme::GOLD, s.face.trim()));
        line.push_str(&theme.dim(&format!(" {}x   ", s.three)));
    }
    ctx.screen.line(&line);
    ctx.screen.line(&theme.dim("  two 7s anywhere 5x · one 7 anywhere 2x"));
}

pub fn play(ctx: &mut Ctx) {
    let strip = strip();
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }

        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, TITLE);
        ctx.screen.blank();
        ctx.screen.line(&format!("  credits: {}", theme.win(&format!("{bank} chips"))));
        ctx.screen.blank();
        paytable(ctx);
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&ctx.theme(), &[('s', "set a stake and spin"), ('q', "leave")]));
        ctx.screen.present();
        if ui::choose_key(&['s', 'q'], 'q') != Some('s') {
            break;
        }

        let Some(stake) = table::stake(ctx, TITLE, "one payline, three reels", bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let line = spin(ctx, &strip, stake, None);
        ctx.store.bump("slots.spins", 1);
        let mult = payout_mult(line);
        let delta = table::settle(ctx, KEY, stake, stake * mult);

        let theme = ctx.theme();
        let top = format!("  stake: {}", theme.paint(ui::theme::GOLD, &format!("{stake} chips")));
        let note = theme.accent(&describe(line, mult));
        draw_settled(ctx, line, &top, &note);
        table::verdict(ctx, delta);
        if !table::again(ctx, "spin again") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// A machine left running on demo credits — no real chips move.
pub fn idle(ctx: &mut Ctx) {
    let strip = strip();
    let mut credits: i64 = 500;
    let stake = 10;
    loop {
        if credits < stake {
            credits = 500;
        }
        credits -= stake;
        let line = spin(ctx, &strip, stake, Some(credits));
        let mult = payout_mult(line);
        let won = stake * mult;
        credits += won;
        let theme = ctx.theme();
        let top = format!("  credits: {}", theme.win(&format!("{credits} chips")));
        let note = if won > 0 {
            theme.win(&format!("demo machine pays {won}"))
        } else {
            theme.dim(&describe(line, mult))
        };
        draw_settled(ctx, line, &top, &note);
        table::idle_footer(ctx, "demo credits only — nothing here touches your wallet");
        ctx.screen.present();
        if table::idle_hold(if won > 0 { 1_600 } else { 900 }) {
            return;
        }
    }
}

/// One spin at the machine's own odds, no rendering: chips staked, chips
/// returned. Used by the audit harness in `games::audit`.
pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    let strip = strip();
    let stops = draw_stops(rng, strip.len());
    let line = [strip[stops[0]], strip[stops[1]], strip[stops[2]]];
    (100, 100 * payout_mult(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_holds_every_symbol_to_its_weight() {
        let s = strip();
        assert_eq!(s.len(), SYMBOLS.iter().map(|x| x.weight).sum::<usize>());
        for (i, sym) in SYMBOLS.iter().enumerate() {
            assert_eq!(s.iter().filter(|x| **x == i).count(), sym.weight, "symbol {i}");
        }
    }

    #[test]
    fn three_of_a_kind_pays_its_listed_multiplier() {
        for (i, sym) in SYMBOLS.iter().enumerate() {
            assert_eq!(payout_mult([i, i, i]), sym.three, "three of symbol {i}");
        }
    }

    #[test]
    fn loose_sevens_pay_even_without_a_line() {
        // Two sevens and something else.
        assert_eq!(payout_mult([SEVEN, SEVEN, 3]), 5);
        assert_eq!(payout_mult([SEVEN, 3, SEVEN]), 5);
        // One seven.
        assert_eq!(payout_mult([SEVEN, 2, 3]), 2);
        assert_eq!(payout_mult([2, 3, SEVEN]), 2);
    }

    #[test]
    fn three_sevens_pay_the_jackpot_not_the_loose_seven_rate() {
        assert_eq!(payout_mult([SEVEN, SEVEN, SEVEN]), SYMBOLS[SEVEN].three);
        assert!(payout_mult([SEVEN, SEVEN, SEVEN]) > payout_mult([SEVEN, SEVEN, 3]));
    }

    #[test]
    fn a_mixed_line_without_a_seven_pays_nothing() {
        assert_eq!(payout_mult([1, 2, 3]), 0);
        assert_eq!(payout_mult([5, 5, 4]), 0);
    }

    #[test]
    fn the_window_wraps_around_the_strip_ends() {
        let s = strip();
        let n = s.len();
        let w = window(&s, 0);
        assert_eq!(w[0], s[n - 1], "the symbol above position 0 is the last on the strip");
        assert_eq!(w[1], s[0]);
        assert_eq!(w[2], s[1]);
    }

    #[test]
    fn rarer_symbols_pay_more() {
        // The strip is the odds, so the paytable must run the other way.
        for pair in SYMBOLS.windows(2) {
            assert!(pair[0].weight < pair[1].weight, "weights must ascend");
            assert!(pair[0].three > pair[1].three, "payouts must descend as weight rises");
        }
    }

    #[test]
    fn the_machine_returns_what_it_should() {
        // Twenty-one stops on three reels is 9,261 outcomes — few enough to
        // add up exactly, so this is the machine's true return rather than
        // an estimate of it.
        let s = strip();
        let n = s.len();
        let mut paid = 0i64;
        for a in 0..n {
            for b in 0..n {
                for c in 0..n {
                    paid += payout_mult([s[a], s[b], s[c]]);
                }
            }
        }
        let rtp = paid as f64 / (n * n * n) as f64;
        assert!(rtp > 0.90 && rtp < 0.96, "the machine returns {rtp:.4}");
    }
}
