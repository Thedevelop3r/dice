//! Horse Racing — six runners, fixed odds, and a race you actually watch.
//!
//! The board is built the other way round from most: the odds come first
//! and each runner's chance is derived from them, so every horse returns
//! exactly the same percentage of your stake over time and the whole book
//! carries one consistent margin. `every_runner_is_priced_the_same_way`
//! holds that to account.
//!
//! The winner is drawn before the tapes go up and the race is then run to
//! that result — the same honesty the wheel and the reels use. What the
//! running order does between the start and the line is genuinely random,
//! so the lead changes hands, but the finish was never in doubt.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::rng::Rng;
use crate::ui::{self, widgets};

const KEY: &str = "horses";
const TITLE: &str = "HORSE RACING";

const TRACK: usize = 48;
const RUNNERS: usize = 6;

/// The card: name and odds-against. Weights are derived from these, never
/// the other way round.
const CARD: [(&str, i64); RUNNERS] = [
    ("Ninepin", 2),
    ("Copper Bell", 3),
    ("Marchpane", 4),
    ("Kestrel Lane", 6),
    ("Ash Moth", 10),
    ("Long Odds Lil", 20),
];

/// Each runner's share of the race, scaled so they are whole numbers.
/// A horse at `n`-to-1 wins one race in `n + 1` before the margin, so the
/// weights are proportional to `1 / (odds + 1)`.
fn weights() -> [i64; RUNNERS] {
    // 4620 is the lowest common multiple of every (odds + 1) on the card,
    // so every weight below comes out exact rather than rounded.
    const SCALE: i64 = 4_620;
    let mut w = [0i64; RUNNERS];
    for (i, (_, odds)) in CARD.iter().enumerate() {
        w[i] = SCALE / (odds + 1);
    }
    w
}

/// Draws the winner from the book's own weights.
fn draw_winner(rng: &mut Rng) -> usize {
    let w = weights();
    let total: i64 = w.iter().sum();
    let mut roll = rng.below(total as usize) as i64;
    for (i, weight) in w.iter().enumerate() {
        roll -= weight;
        if roll < 0 {
            return i;
        }
    }
    RUNNERS - 1
}

/// Gross returned on a winning ticket: the stake plus its odds.
fn payout(backed: usize, winner: usize, stake: i64) -> i64 {
    if backed == winner {
        stake * (CARD[backed].1 + 1)
    } else {
        0
    }
}

fn draw_race(ctx: &mut Ctx, positions: &[usize], backed: Option<usize>, stake: i64, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    if let Some(b) = backed.filter(|_| stake > 0) {
        ctx.screen.line(&format!(
            "  {} on {} at {}",
            theme.paint(ui::theme::GOLD, &format!("{stake} chips")),
            theme.accent(CARD[b].0),
            theme.dim(&format!("{}:1", CARD[b].1))
        ));
        ctx.screen.blank();
    }
    for (i, (name, odds)) in CARD.iter().enumerate() {
        let pos = positions[i].min(TRACK);
        let mut lane = String::from("  ");
        let yours = backed == Some(i);
        let label = format!("{name:<14}");
        if yours {
            theme.paint_into(&mut lane, ui::theme::GOLD, &label);
        } else {
            theme.paint_into(&mut lane, ui::theme::DIM, &label);
        }
        theme.paint_into(&mut lane, ui::theme::DIM, &"·".repeat(pos));
        theme.paint_into(&mut lane, if yours { ui::theme::GOLD } else { ui::theme::CYAN }, "▶");
        theme.paint_into(&mut lane, ui::theme::DIM, &" ".repeat(TRACK - pos));
        theme.paint_into(&mut lane, ui::theme::GOLD_DIM, &format!("│ {odds}:1"));
        ctx.screen.line(&lane);
    }
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

/// Runs the race to a decided winner. Every runner is given a finishing
/// time, the winner's the shortest, and the field is then advanced along
/// those times with a little wobble — so places swap on the way round but
/// the right horse hits the line first.
fn run_race(ctx: &mut Ctx, winner: usize, backed: Option<usize>, stake: i64, idle_mode: bool) -> bool {
    let mut order: Vec<usize> = (0..RUNNERS).filter(|i| *i != winner).collect();
    for i in (1..order.len()).rev() {
        let j = ctx.rng.below(i + 1);
        order.swap(i, j);
    }
    let mut finish = [0usize; RUNNERS];
    finish[winner] = 34;
    for (rank, horse) in order.iter().enumerate() {
        finish[*horse] = 36 + rank * 2 + ctx.rng.below(3);
    }

    let frames = finish[winner];
    for f in 1..=frames {
        let mut positions = [0usize; RUNNERS];
        for (i, p) in positions.iter_mut().enumerate() {
            let base = TRACK * f / finish[i];
            // A little jostling, but never enough to put a horse on the
            // line before the one that is supposed to win it.
            let wobble = ctx.rng.below(3);
            *p = (base + wobble).min(if i == winner { TRACK } else { TRACK - 1 });
        }
        let note = if f == frames { String::new() } else { ctx.theme().dim("and they're away...").to_string() };
        draw_race(ctx, &positions, backed, stake, &note);
        if idle_mode {
            table::idle_footer(ctx, "a demo card — no ticket of yours is on it");
        }
        ctx.screen.present();
        if idle_mode {
            if table::idle_hold(90) {
                return true;
            }
        } else {
            ui::sleep_ms(90);
        }
    }
    false
}

fn board(ctx: &mut Ctx) {
    let theme = ctx.theme();
    ctx.screen.line(&theme.dim("  today's card"));
    for (i, (name, odds)) in CARD.iter().enumerate() {
        ctx.screen.line(&format!(
            "   {}  {}  {}",
            theme.paint(ui::theme::GOLD, &format!("[{}]", i + 1)),
            theme.accent(&format!("{name:<14}")),
            theme.dim(&format!("{odds}:1"))
        ));
    }
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
        board(ctx);
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&ctx.theme(), &[('1', "…"), ('6', "back a runner"), ('q', "leave")]));
        ctx.screen.present();

        let keys: Vec<char> = (1..=RUNNERS).map(|i| char::from_digit(i as u32, 10).unwrap()).chain(['q']).collect();
        let Some(c) = ui::choose_key(&keys, 'q') else { break };
        if c == 'q' {
            break;
        }
        let backed = c.to_digit(10).unwrap() as usize - 1;

        let label = format!("{} at {}:1", CARD[backed].0, CARD[backed].1);
        let Some(stake) = table::stake(ctx, TITLE, &label, bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let winner = draw_winner(ctx.rng);
        run_race(ctx, winner, Some(backed), stake, false);

        ctx.store.bump("horses.races", 1);
        let delta = table::settle(ctx, KEY, stake, payout(backed, winner, stake));

        let theme = ctx.theme();
        let note = if delta > 0 {
            theme.win(&format!("{} takes it at {}:1", CARD[winner].0, CARD[winner].1))
        } else {
            theme.lose(&format!("{} takes it — your ticket's no good", CARD[winner].0))
        };
        let mut positions = [TRACK - 1; RUNNERS];
        positions[winner] = TRACK;
        draw_race(ctx, &positions, Some(backed), stake, &note);
        table::verdict(ctx, delta);
        if !table::again(ctx, "next race") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// Races run back to back with a fictional punter's ticket on each.
pub fn idle(ctx: &mut Ctx) {
    loop {
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let backed = ctx.rng.below(RUNNERS);
        let stake = 10 + 5 * ctx.rng.below(5) as i64;
        let winner = draw_winner(ctx.rng);

        let theme = ctx.theme();
        let note = theme.dim(&format!("{punter} is on {} at {}:1", CARD[backed].0, CARD[backed].1));
        let mut positions = [0usize; RUNNERS];
        draw_race(ctx, &positions, Some(backed), stake, &note);
        table::idle_footer(ctx, "a demo card — no ticket of yours is on it");
        ctx.screen.present();
        if table::idle_hold(1_300) {
            return;
        }

        if run_race(ctx, winner, Some(backed), stake, true) {
            return;
        }

        let won = payout(backed, winner, stake) - stake;
        let theme = ctx.theme();
        let note = if won > 0 {
            theme.win(&format!("{} at {}:1 — {punter} collects {won}", CARD[winner].0, CARD[winner].1))
        } else {
            theme.lose(&format!("{} takes it — {punter} tears it up", CARD[winner].0))
        };
        positions = [TRACK - 1; RUNNERS];
        positions[winner] = TRACK;
        draw_race(ctx, &positions, Some(backed), stake, &note);
        table::idle_footer(ctx, "a demo card — no ticket of yours is on it");
        ctx.screen.present();
        if table::idle_hold(2_400) {
            return;
        }
    }
}

/// One race with a ticket on `backed`, no rendering.
pub fn simulate_at(rng: &mut Rng, backed: usize) -> (i64, i64) {
    (100, payout(backed, draw_winner(rng), 100))
}

#[cfg(test)]
pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    simulate_at(rng, 0)
}

pub const FIELD: usize = RUNNERS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_runners_weight_divides_out_exactly() {
        // The scale is chosen so no weight is ever rounded — if a horse is
        // added at odds that do not divide it, this catches it.
        let w = weights();
        for (i, (_, odds)) in CARD.iter().enumerate() {
            assert_eq!(w[i] * (odds + 1), 4_620, "{} at {odds}:1 does not divide the scale", CARD[i].0);
        }
    }

    #[test]
    fn every_runner_is_priced_the_same_way() {
        // Backing any horse must return the same share of the stake over
        // time — that share is the book's margin, and it must be one number.
        let w = weights();
        let total: i64 = w.iter().sum();
        let paybacks: Vec<f64> = CARD.iter().enumerate().map(|(i, (_, odds))| (odds + 1) as f64 * w[i] as f64 / total as f64).collect();
        let first = paybacks[0];
        for (i, p) in paybacks.iter().enumerate() {
            assert!((p - first).abs() < 1e-9, "{} returns {p}, others return {first}", CARD[i].0);
        }
        assert!(first > 0.90 && first < 0.96, "the book returns {first}");
    }

    #[test]
    fn the_favourite_is_the_most_likely_winner() {
        let w = weights();
        assert!(w.windows(2).all(|p| p[0] > p[1]), "the card must run from favourite to outsider: {w:?}");
    }

    #[test]
    fn a_winning_ticket_pays_the_odds_on_the_board() {
        assert_eq!(payout(0, 0, 10), 30, "2:1 returns the stake plus twice it");
        assert_eq!(payout(5, 5, 10), 210, "20:1 returns the stake plus twenty times it");
    }

    #[test]
    fn a_losing_ticket_returns_nothing() {
        assert_eq!(payout(0, 1, 10), 0);
        assert_eq!(payout(5, 0, 10), 0);
    }

    #[test]
    fn winners_come_out_in_roughly_their_advertised_share() {
        let mut rng = crate::rng::Rng::from_seed(17);
        let n = 120_000;
        let mut wins = [0i64; RUNNERS];
        for _ in 0..n {
            wins[draw_winner(&mut rng)] += 1;
        }
        let w = weights();
        let total: i64 = w.iter().sum();
        for i in 0..RUNNERS {
            let got = wins[i] as f64 / n as f64;
            let want = w[i] as f64 / total as f64;
            assert!((got - want).abs() < 0.01, "{} won {got} of the time, expected {want}", CARD[i].0);
        }
    }

    #[test]
    fn the_whole_field_is_drawable() {
        let mut rng = crate::rng::Rng::from_seed(23);
        let mut seen = [false; RUNNERS];
        for _ in 0..5_000 {
            seen[draw_winner(&mut rng)] = true;
        }
        assert!(seen.iter().all(|s| *s), "some runner can never win: {seen:?}");
    }
}
