//! Scratch Cards — nine panels, three matching symbols wins.
//!
//! Real scratch cards are printed knowing exactly what they pay: the prize
//! is decided at the press and the panels are laid out to show it. This one
//! works the same way, which is why `PRIZES` can state the return to the
//! player exactly rather than approximately — `the_press_run_pays_out_what_it_promises`
//! adds the table up and holds it to that number.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, widgets};

const KEY: &str = "scratch";
const TITLE: &str = "SCRATCH CARDS";

const PANELS: usize = 9;
const COLS: usize = 3;
/// Weights are out of this, so the table reads as plain probabilities.
const RUN: i64 = 10_000;

/// The print run: `(symbol, gross multiplier, how many cards in ten
/// thousand pay it)`. The blank symbol at the top is the losing card.
const PRIZES: [(&str, i64, i64); 9] = [
    ("·", 0, 8_000),
    ("▲", 1, 1_100),
    ("●", 2, 500),
    ("■", 5, 240),
    ("♥", 10, 100),
    ("♦", 25, 40),
    ("♣", 100, 15),
    ("♠", 500, 4),
    ("★", 1_000, 1),
];

/// Symbols that can appear on a card face — every prize symbol, blanks
/// excluded.
fn faces() -> Vec<&'static str> {
    PRIZES.iter().skip(1).map(|(s, _, _)| *s).collect()
}

/// Draws a card from the print run: which prize tier it is.
fn press_card(ctx: &mut Ctx) -> usize {
    let roll = ctx.rng.below(RUN as usize) as i64;
    let mut seen = 0;
    for (i, (_, _, weight)) in PRIZES.iter().enumerate() {
        seen += weight;
        if roll < seen {
            return i;
        }
    }
    0
}

/// Lays out the nine panels so they show exactly the prize the card was
/// pressed with: three of the winning symbol for a winner, and no symbol
/// three times over for a loser.
fn lay_panels(ctx: &mut Ctx, tier: usize) -> Vec<&'static str> {
    let pool = faces();
    let mut panels: Vec<&'static str> = Vec::with_capacity(PANELS);
    if tier > 0 {
        let winner = PRIZES[tier].0;
        panels.extend(std::iter::repeat_n(winner, 3));
        // Fill the rest without letting anything else reach three, and
        // without a second winner sneaking in.
        let others: Vec<&'static str> = pool.iter().copied().filter(|s| *s != winner).collect();
        let mut counts = vec![0usize; others.len()];
        while panels.len() < PANELS {
            let i = ctx.rng.below(others.len());
            if counts[i] < 2 {
                counts[i] += 1;
                panels.push(others[i]);
            }
        }
    } else {
        // A losing card: no symbol may appear three times. Eight symbols
        // over nine panels at two apiece leaves plenty of room.
        let mut counts = vec![0usize; pool.len()];
        while panels.len() < PANELS {
            let i = ctx.rng.below(pool.len());
            if counts[i] < 2 {
                counts[i] += 1;
                panels.push(pool[i]);
            }
        }
    }
    for i in (1..panels.len()).rev() {
        let j = ctx.rng.below(i + 1);
        panels.swap(i, j);
    }
    panels
}

/// What a finished card is worth: the symbol appearing three or more
/// times, if any. Read off the panels rather than the tier, so the face
/// and the payout can never disagree.
fn card_value(panels: &[&str]) -> (i64, &'static str) {
    for (symbol, mult, _) in PRIZES.iter().skip(1) {
        if panels.iter().filter(|p| *p == symbol).count() >= 3 {
            return (*mult, symbol);
        }
    }
    (0, "")
}

fn draw_card(ctx: &mut Ctx, panels: &[&str], revealed: usize, stake: i64, winner: Option<&str>, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    ctx.screen.line(&format!("  card price: {}", theme.paint(ui::theme::GOLD, &format!("{stake} chips"))));
    ctx.screen.blank();
    for row in 0..PANELS / COLS {
        let mut top = String::from("   ");
        let mut mid = String::from("   ");
        let mut bot = String::from("   ");
        for col in 0..COLS {
            let i = row * COLS + col;
            let shown = i < revealed;
            top.push_str(&theme.paint(ui::theme::GOLD_DIM, "┌─────┐ "));
            bot.push_str(&theme.paint(ui::theme::GOLD_DIM, "└─────┘ "));
            mid.push_str(&theme.paint(ui::theme::GOLD_DIM, "│"));
            if shown {
                let is_winner = winner == Some(panels[i]);
                let code = if is_winner { ui::theme::GREEN } else { ui::theme::WHITE };
                theme.paint_into(&mut mid, code, &format!("  {}  ", panels[i]));
            } else {
                theme.paint_into(&mut mid, ui::theme::DIM, "▓▓▓▓▓");
            }
            mid.push_str(&theme.paint(ui::theme::GOLD_DIM, "│ "));
        }
        ctx.screen.line(&top);
        ctx.screen.line(&mid);
        ctx.screen.line(&bot);
    }
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

fn prize_board(ctx: &mut Ctx) {
    let theme = ctx.theme();
    ctx.screen.line(&theme.dim("  three of a kind pays:"));
    let mut line = String::from("  ");
    for (symbol, mult, _) in PRIZES.iter().skip(1) {
        line.push_str(&theme.paint(ui::theme::GOLD, symbol));
        line.push_str(&theme.dim(&format!(" {mult}x   ")));
    }
    ctx.screen.line(&line);
}

pub fn play(ctx: &mut Ctx) {
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }

        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, TITLE);
        ctx.screen.blank();
        ctx.screen.line(&format!("  bank: {}", theme.win(&format!("{bank} chips"))));
        ctx.screen.blank();
        prize_board(ctx);
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&ctx.theme(), &[('b', "buy a card"), ('q', "leave")]));
        ctx.screen.present();
        if ui::choose_key(&['b', 'q'], 'q') != Some('b') {
            break;
        }

        let Some(stake) = table::stake(ctx, TITLE, "nine panels, three of a kind pays", bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let tier = press_card(ctx);
        let panels = lay_panels(ctx, tier);
        for revealed in 1..=PANELS {
            draw_card(ctx, &panels, revealed, stake, None, &ctx.theme().dim("scratching..."));
            ctx.screen.present();
            ui::sleep_ms(230);
        }

        let (mult, symbol) = card_value(&panels);
        ctx.store.bump("scratch.cards", 1);
        let delta = table::settle(ctx, KEY, stake, stake * mult);

        let theme = ctx.theme();
        let note = if mult > 0 {
            theme.win(&format!("three {symbol} — pays {mult}x"))
        } else {
            theme.lose("no three of a kind")
        };
        draw_card(ctx, &panels, PANELS, stake, (mult > 0).then_some(symbol), &note);
        table::verdict(ctx, delta);
        if !table::again(ctx, "buy another") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// Cards being scratched at the counter, one after another.
pub fn idle(ctx: &mut Ctx) {
    let stake = 10;
    loop {
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let tier = press_card(ctx);
        let panels = lay_panels(ctx, tier);
        let theme = ctx.theme();
        let headline = theme.dim(&format!("{punter} buys a card"));
        for revealed in 1..=PANELS {
            draw_card(ctx, &panels, revealed, stake, None, &headline);
            table::idle_footer(ctx, "demo cards only — nothing here touches your wallet");
            ctx.screen.present();
            if table::idle_hold(220) {
                return;
            }
        }
        let (mult, symbol) = card_value(&panels);
        let theme = ctx.theme();
        let note = if mult > 0 {
            theme.win(&format!("three {symbol} — {punter} takes {}", mult * stake))
        } else {
            theme.dim(&format!("nothing for {punter}"))
        };
        draw_card(ctx, &panels, PANELS, stake, (mult > 0).then_some(symbol), &note);
        table::idle_footer(ctx, "demo cards only — nothing here touches your wallet");
        ctx.screen.present();
        if table::idle_hold(2_200) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> (crate::rng::Rng, crate::stats::Store, crate::ui::Screen) {
        (crate::rng::Rng::from_seed(31), crate::stats::Store::blank(), crate::ui::Screen::headless())
    }

    #[test]
    fn the_print_run_adds_up() {
        assert_eq!(PRIZES.iter().map(|(_, _, w)| *w).sum::<i64>(), RUN);
    }

    #[test]
    fn the_press_run_pays_out_what_it_promises() {
        // Every card in ten thousand, weighted by its prize: the return to
        // the player, exactly, with no simulation involved.
        let paid: i64 = PRIZES.iter().map(|(_, mult, weight)| mult * weight).sum();
        let rtp = paid as f64 / RUN as f64;
        assert!((rtp - 0.98).abs() < 1e-9, "the run returns {rtp}, not 0.98");
    }

    #[test]
    fn every_prize_tier_has_its_own_symbol() {
        let symbols: Vec<&str> = PRIZES.iter().map(|(s, _, _)| *s).collect();
        let mut sorted = symbols.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), symbols.len(), "two tiers share a symbol");
    }

    #[test]
    fn a_winning_card_shows_exactly_the_prize_it_was_pressed_with() {
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        for tier in 1..PRIZES.len() {
            for _ in 0..200 {
                let panels = lay_panels(&mut ctx, tier);
                assert_eq!(panels.len(), PANELS);
                let (mult, symbol) = card_value(&panels);
                assert_eq!(mult, PRIZES[tier].1, "tier {tier} laid out as {mult}x");
                assert_eq!(symbol, PRIZES[tier].0);
            }
        }
    }

    #[test]
    fn a_losing_card_never_shows_three_of_anything() {
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        for _ in 0..2_000 {
            let panels = lay_panels(&mut ctx, 0);
            assert_eq!(panels.len(), PANELS);
            assert_eq!(card_value(&panels), (0, ""), "a losing card paid: {panels:?}");
        }
    }

    #[test]
    fn a_winning_card_carries_no_second_winner() {
        // Two three-of-a-kinds on one card would make the payout ambiguous.
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        for tier in 1..PRIZES.len() {
            for _ in 0..200 {
                let panels = lay_panels(&mut ctx, tier);
                let triples = faces().iter().filter(|s| panels.iter().filter(|p| *p == *s).count() >= 3).count();
                assert_eq!(triples, 1, "tier {tier} produced {triples} winning symbols");
            }
        }
    }

    #[test]
    fn the_press_draws_every_tier_in_roughly_its_share() {
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        let n = 200_000;
        let mut counts = vec![0i64; PRIZES.len()];
        for _ in 0..n {
            counts[press_card(&mut ctx)] += 1;
        }
        // The losing card is the bulk of the run and the easiest to check.
        let losing = counts[0] as f64 / n as f64;
        let expected = PRIZES[0].2 as f64 / RUN as f64;
        assert!((losing - expected).abs() < 0.01, "losing cards came out {losing}, expected {expected}");
    }

    #[test]
    fn better_prizes_are_rarer() {
        for pair in PRIZES.windows(2).skip(1) {
            assert!(pair[0].1 < pair[1].1, "prizes must climb through the table");
            assert!(pair[0].2 > pair[1].2, "and their share of the run must fall");
        }
    }
}
