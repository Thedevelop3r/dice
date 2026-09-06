//! Bingo — a 75-ball card, and a payout that depends on how *fast* your
//! first line comes in.
//!
//! Flat-rate bingo does not work as a casino game: a line inside forty
//! balls turns up in nearly half of all cards, so any payout worth having
//! would hand the building over. Pricing the speed instead keeps the game
//! tense the whole way down the card — a line on ball fourteen is worth
//! thirty times the stake, the same line on ball thirty-eight is worth the
//! stake back — and lands the return at a shade under 97%.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, widgets};

const KEY: &str = "bingo";
const TITLE: &str = "BINGO";

const SIDE: usize = 5;
/// Numbers per column: B is 1-15, I is 16-30, and so on up to O at 61-75.
const PER_COLUMN: usize = 15;
const BALLS: usize = 75;
/// The caller stops here. Past this the card is dead.
const LIMIT: usize = 40;
const HEADS: [char; SIDE] = ['B', 'I', 'N', 'G', 'O'];

/// `(no later than this ball, gross multiplier)`, quickest first. The
/// bands were priced against the real distribution of first-line times;
/// see `the_paytable_returns_what_it_should`.
const PAYTABLE: [(usize, i64); 5] = [(15, 30), (20, 10), (25, 4), (30, 2), (40, 1)];

/// The twelve ways to complete a line, as `(column, row)` cells.
fn lines() -> Vec<Vec<(usize, usize)>> {
    let mut out: Vec<Vec<(usize, usize)>> = Vec::with_capacity(12);
    for r in 0..SIDE {
        out.push((0..SIDE).map(|c| (c, r)).collect());
    }
    for c in 0..SIDE {
        out.push((0..SIDE).map(|r| (c, r)).collect());
    }
    out.push((0..SIDE).map(|i| (i, i)).collect());
    out.push((0..SIDE).map(|i| (i, SIDE - 1 - i)).collect());
    out
}

/// A card is five columns of five, each drawn from that column's own range
/// and sorted, with the centre square free.
#[derive(Debug, Clone)]
struct Card {
    /// `cells[column][row]`.
    cells: [[u32; SIDE]; SIDE],
    marked: [[bool; SIDE]; SIDE],
}

impl Card {
    fn new(ctx: &mut Ctx) -> Card {
        let mut cells = [[0u32; SIDE]; SIDE];
        for (c, col) in cells.iter_mut().enumerate() {
            let lo = (c * PER_COLUMN + 1) as u32;
            let mut pool: Vec<u32> = (lo..lo + PER_COLUMN as u32).collect();
            for i in (1..pool.len()).rev() {
                let j = ctx.rng.below(i + 1);
                pool.swap(i, j);
            }
            let mut chosen: Vec<u32> = pool.into_iter().take(SIDE).collect();
            chosen.sort_unstable();
            col.copy_from_slice(&chosen);
        }
        let mut marked = [[false; SIDE]; SIDE];
        marked[SIDE / 2][SIDE / 2] = true;
        Card { cells, marked }
    }

    fn mark(&mut self, ball: u32) -> bool {
        for c in 0..SIDE {
            for r in 0..SIDE {
                if self.cells[c][r] == ball && !(c == SIDE / 2 && r == SIDE / 2) {
                    self.marked[c][r] = true;
                    return true;
                }
            }
        }
        false
    }

    fn completed_lines(&self, lines: &[Vec<(usize, usize)>]) -> usize {
        lines.iter().filter(|l| l.iter().all(|(c, r)| self.marked[*c][*r])).count()
    }

    /// How close the nearest incomplete line is — the number the caller
    /// still needs. Used to show a card that is waiting on one.
    fn closest_gap(&self, lines: &[Vec<(usize, usize)>]) -> usize {
        lines
            .iter()
            .map(|l| l.iter().filter(|(c, r)| !self.marked[*c][*r]).count())
            .filter(|n| *n > 0)
            .min()
            .unwrap_or(0)
    }
}

/// What a first line on ball `n` is worth, gross on the stake.
fn payout_mult(first_line: Option<usize>) -> i64 {
    let Some(n) = first_line else { return 0 };
    for (limit, mult) in PAYTABLE {
        if n <= limit {
            return mult;
        }
    }
    0
}

/// The caller's list: every ball, shuffled, capped at the limit.
fn call_sheet(ctx: &mut Ctx) -> Vec<u32> {
    let mut pool: Vec<u32> = (1..=BALLS as u32).collect();
    for i in (1..pool.len()).rev() {
        let j = ctx.rng.below(i + 1);
        pool.swap(i, j);
    }
    pool.truncate(LIMIT);
    pool
}

/// `B-7`, `N-42` — how a ball is called.
fn call_name(ball: u32) -> String {
    let col = ((ball - 1) as usize / PER_COLUMN).min(SIDE - 1);
    format!("{}-{ball}", HEADS[col])
}

fn draw_card(ctx: &mut Ctx, card: &Card, called: &[u32], stake: i64, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    ctx.screen.line(&format!(
        "  {} · ball {} of {LIMIT}",
        theme.paint(ui::theme::GOLD, &format!("{stake} chips")),
        theme.accent(&called.len().to_string())
    ));
    ctx.screen.blank();

    let mut head = String::from("   ");
    for h in HEADS {
        theme.paint_into(&mut head, ui::theme::GOLD, &format!("  {h}  "));
    }
    ctx.screen.line(&head);
    for r in 0..SIDE {
        let mut line = String::from("   ");
        for c in 0..SIDE {
            if c == SIDE / 2 && r == SIDE / 2 {
                theme.paint_into(&mut line, ui::theme::GREEN, " FREE");
            } else if card.marked[c][r] {
                theme.paint_into(&mut line, ui::theme::GREEN, &format!(" [{:>2}]", card.cells[c][r]));
            } else {
                theme.paint_into(&mut line, ui::theme::DIM, &format!("  {:>2} ", card.cells[c][r]));
            }
        }
        ctx.screen.line(&line);
    }

    if !called.is_empty() {
        ctx.screen.blank();
        let recent: Vec<String> = called.iter().rev().take(10).rev().map(|b| call_name(*b)).collect();
        ctx.screen.line(&theme.dim(&format!("  called: {}", recent.join("  "))));
    }
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

fn paytable_lines(ctx: &mut Ctx) {
    let theme = ctx.theme();
    ctx.screen.line(&theme.dim("  a first line pays by how quickly it comes:"));
    let mut prev = 0;
    let mut parts: Vec<String> = Vec::new();
    for (limit, mult) in PAYTABLE {
        parts.push(format!("by ball {limit} {mult}x"));
        prev = limit;
    }
    let _ = prev;
    ctx.screen.line(&theme.dim(&format!("   {}", parts.join(" · "))));
}

pub fn play(ctx: &mut Ctx) {
    let lines = lines();
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
        paytable_lines(ctx);
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&ctx.theme(), &[('b', "buy a card"), ('q', "leave")]));
        ctx.screen.present();
        if ui::choose_key(&['b', 'q'], 'q') != Some('b') {
            break;
        }

        let Some(stake) = table::stake(ctx, TITLE, "one card, forty balls", bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let mut card = Card::new(ctx);
        let sheet = call_sheet(ctx);
        let mut called: Vec<u32> = Vec::with_capacity(LIMIT);
        let mut first_line: Option<usize> = None;

        for (i, ball) in sheet.iter().enumerate() {
            let hit = card.mark(*ball);
            called.push(*ball);
            let done = card.completed_lines(&lines);
            if done > 0 {
                first_line = Some(i + 1);
            }
            let theme = ctx.theme();
            let gap = card.closest_gap(&lines);
            let note = if done > 0 {
                theme.win(&format!("{} — that's a line!", call_name(*ball)))
            } else if hit {
                theme.accent(&format!("{} — marked{}", call_name(*ball), if gap == 1 { ", one away" } else { "" }))
            } else if gap == 1 {
                theme.paint(ui::theme::GOLD, &format!("{} — not yours, and you're one away", call_name(*ball)))
            } else {
                theme.dim(&call_name(*ball))
            };
            draw_card(ctx, &card, &called, stake, &note);
            ctx.screen.present();
            ui::sleep_ms(if done > 0 {
                600
            } else if hit || gap == 1 {
                340
            } else {
                150_u64.saturating_sub(i as u64)
            });
            if done > 0 {
                break;
            }
        }

        let mult = payout_mult(first_line);
        ctx.store.bump("bingo.cards", 1);
        if let Some(n) = first_line {
            ctx.store.bump("bingo.lines", 1);
            ctx.store.record_best("bingo.fastest_line", (LIMIT + 1 - n) as i64);
        }
        let delta = table::settle(ctx, KEY, stake, stake * mult);

        let theme = ctx.theme();
        let note = match first_line {
            Some(n) if mult > 0 => theme.win(&format!("a line on ball {n} — pays {mult}x")),
            _ => theme.lose("forty balls and no line"),
        };
        draw_card(ctx, &card, &called, stake, &note);
        table::verdict(ctx, delta);
        if !table::again(ctx, "another card") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// A hall running its own game, with three cards on the board so there is
/// usually someone close.
pub fn idle(ctx: &mut Ctx) {
    let lines = lines();
    loop {
        let mut cards: Vec<(usize, Card)> = (0..3).map(|_| (ctx.rng.below(table::BOT_NAMES.len()), Card::new(ctx))).collect();
        let sheet = call_sheet(ctx);
        let mut called: Vec<u32> = Vec::with_capacity(LIMIT);
        let mut winner: Option<(usize, usize)> = None;

        for (i, ball) in sheet.iter().enumerate() {
            for (_, card) in cards.iter_mut() {
                card.mark(*ball);
            }
            called.push(*ball);
            for (who, card) in cards.iter() {
                if card.completed_lines(&lines) > 0 {
                    winner = Some((*who, i + 1));
                    break;
                }
            }

            let theme = ctx.theme();
            let close = cards.iter().find(|(_, c)| c.closest_gap(&lines) == 1).map(|(who, _)| table::BOT_NAMES[who % table::BOT_NAMES.len()]);
            let note = match (&winner, close) {
                (Some((who, n)), _) => theme.win(&format!("{} has a line on ball {n}!", table::BOT_NAMES[who % table::BOT_NAMES.len()])),
                (None, Some(name)) => theme.paint(ui::theme::GOLD, &format!("{} — {name} is one away", call_name(*ball))),
                (None, None) => theme.dim(&call_name(*ball)),
            };
            // Only the first card is shown in full; the others are the
            // reason the hall feels occupied.
            draw_card(ctx, &cards[0].1, &called, 0, &note);
            let theme = ctx.theme();
            let others: Vec<String> = cards
                .iter()
                .skip(1)
                .map(|(who, c)| format!("{} needs {}", table::BOT_NAMES[who % table::BOT_NAMES.len()], c.closest_gap(&lines)))
                .collect();
            ctx.screen.line(&theme.dim(&format!("  also on: {}", others.join(" · "))));
            table::idle_footer(ctx, "the hall calls its own game — no card here is yours");
            ctx.screen.present();
            if table::idle_hold(if winner.is_some() { 1_800 } else { 220 }) {
                return;
            }
            if winner.is_some() {
                break;
            }
        }

        if winner.is_none() {
            let theme = ctx.theme();
            let note = theme.dim("forty balls, no line — fresh cards");
            draw_card(ctx, &cards[0].1, &called, 0, &note);
            table::idle_footer(ctx, "the hall calls its own game — no card here is yours");
            ctx.screen.present();
            if table::idle_hold(1_800) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> (crate::rng::Rng, crate::stats::Store, crate::ui::Screen) {
        (crate::rng::Rng::from_seed(41), crate::stats::Store::blank(), crate::ui::Screen::headless())
    }

    #[test]
    fn a_card_draws_each_column_from_its_own_range() {
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        for _ in 0..300 {
            let card = Card::new(&mut ctx);
            for c in 0..SIDE {
                let lo = (c * PER_COLUMN + 1) as u32;
                let hi = lo + PER_COLUMN as u32 - 1;
                let col = card.cells[c];
                assert!(col.iter().all(|n| (lo..=hi).contains(n)), "column {c} strayed: {col:?}");
                let mut sorted = col.to_vec();
                sorted.dedup();
                assert_eq!(sorted.len(), SIDE, "column {c} repeated a number: {col:?}");
                assert!(col.windows(2).all(|w| w[0] < w[1]), "column {c} is not sorted");
            }
        }
    }

    #[test]
    fn the_centre_square_starts_marked_and_stays_that_way() {
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        let mut card = Card::new(&mut ctx);
        assert!(card.marked[SIDE / 2][SIDE / 2]);
        // Calling the number printed under the free square changes nothing.
        let under = card.cells[SIDE / 2][SIDE / 2];
        assert!(!card.mark(under), "the free square is not a callable cell");
        assert!(card.marked[SIDE / 2][SIDE / 2]);
    }

    #[test]
    fn there_are_twelve_ways_to_make_a_line() {
        let l = lines();
        assert_eq!(l.len(), 12, "five rows, five columns and two diagonals");
        assert!(l.iter().all(|line| line.len() == SIDE));
    }

    #[test]
    fn rows_columns_and_both_diagonals_all_count() {
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        let l = lines();
        for target in [
            (0..SIDE).map(|c| (c, 2)).collect::<Vec<_>>(),
            (0..SIDE).map(|r| (3, r)).collect::<Vec<_>>(),
            (0..SIDE).map(|i| (i, i)).collect::<Vec<_>>(),
            (0..SIDE).map(|i| (i, SIDE - 1 - i)).collect::<Vec<_>>(),
        ] {
            let mut card = Card::new(&mut ctx);
            assert_eq!(card.completed_lines(&l), 0);
            for (c, r) in &target {
                card.marked[*c][*r] = true;
            }
            assert!(card.completed_lines(&l) >= 1, "line {target:?} was not spotted");
        }
    }

    #[test]
    fn the_gap_counts_down_as_a_line_fills() {
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        let l = lines();
        let mut card = Card::new(&mut ctx);
        // The middle row already has the free square, so it needs four.
        assert_eq!(card.closest_gap(&l), 4);
        card.marked[0][2] = true;
        assert_eq!(card.closest_gap(&l), 3);
    }

    #[test]
    fn a_quicker_line_never_pays_less() {
        let mults: Vec<i64> = PAYTABLE.iter().map(|(_, m)| *m).collect();
        assert!(mults.windows(2).all(|w| w[0] > w[1]), "the paytable must descend: {mults:?}");
        assert_eq!(payout_mult(Some(1)), 30);
        assert_eq!(payout_mult(Some(15)), 30);
        assert_eq!(payout_mult(Some(16)), 10);
        assert_eq!(payout_mult(Some(40)), 1);
        assert_eq!(payout_mult(Some(41)), 0, "past the caller's limit there is nothing");
        assert_eq!(payout_mult(None), 0);
    }

    #[test]
    fn the_paytable_returns_what_it_should() {
        // Deal a few thousand real cards against real call sheets and add
        // up what the table would have paid. This is the number the bands
        // were chosen for, so a change to either must move it back.
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        let l = lines();
        let rounds = 4_000;
        let mut paid = 0i64;
        for _ in 0..rounds {
            let mut card = Card::new(&mut ctx);
            let mut first = None;
            for (i, ball) in call_sheet(&mut ctx).iter().enumerate() {
                card.mark(*ball);
                if card.completed_lines(&l) > 0 {
                    first = Some(i + 1);
                    break;
                }
            }
            paid += payout_mult(first);
        }
        let rtp = paid as f64 / rounds as f64;
        assert!(rtp > 0.88 && rtp < 1.04, "bingo returned {rtp}");
    }

    #[test]
    fn a_call_sheet_never_repeats_a_ball() {
        let (mut rng, mut store, mut screen) = probe();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        for _ in 0..200 {
            let sheet = call_sheet(&mut ctx);
            assert_eq!(sheet.len(), LIMIT);
            let mut seen = std::collections::HashSet::new();
            assert!(sheet.iter().all(|b| seen.insert(*b)), "a ball was called twice");
            assert!(sheet.iter().all(|b| (1..=BALLS as u32).contains(b)));
        }
    }

    #[test]
    fn balls_are_called_by_their_column_letter() {
        assert_eq!(call_name(1), "B-1");
        assert_eq!(call_name(15), "B-15");
        assert_eq!(call_name(16), "I-16");
        assert_eq!(call_name(42), "N-42");
        assert_eq!(call_name(75), "O-75");
    }
}
