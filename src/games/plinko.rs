//! Plinko — drop a ball down twelve rows of pegs and take whatever slot it
//! falls into.
//!
//! Multipliers are held as hundredths so the whole game stays in integer
//! chips; a slot worth `170` pays 1.7x. The three risk profiles all return
//! very close to the same amount over time — the
//! `every_risk_level_keeps_the_same_house_edge` test pins that down against
//! the actual binomial odds of each slot — so picking one is a choice about
//! variance rather than about value.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, widgets};

const KEY: &str = "plinko";
const TITLE: &str = "PLINKO";

const ROWS: usize = 12;
const SLOTS: usize = ROWS + 1;
/// Columns between neighbouring peg positions — wide enough that a slot
/// label sits under the gap it belongs to.
const SPAN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    fn label(self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
        }
    }

    /// Payout per slot in hundredths of the stake, edges outward.
    fn slots(self) -> [i64; SLOTS] {
        match self {
            Risk::Low => [500, 250, 160, 120, 105, 90, 80, 90, 105, 120, 160, 250, 500],
            Risk::Medium => [3_000, 900, 350, 170, 105, 65, 50, 65, 105, 170, 350, 900, 3_000],
            Risk::High => [22_000, 2_800, 650, 170, 60, 30, 20, 30, 60, 170, 650, 2_800, 22_000],
        }
    }
}

/// `1.7x`, `220x`, `0.5x` — one decimal, and none at all when it is whole.
fn fmt_mult(hundredths: i64) -> String {
    if hundredths % 100 == 0 {
        format!("{}x", hundredths / 100)
    } else {
        format!("{}.{}x", hundredths / 100, (hundredths % 100) / 10)
    }
}

/// What a stake returns from a given slot, rounded down to whole chips.
fn payout(risk: Risk, slot: usize, stake: i64) -> i64 {
    stake * risk.slots()[slot] / 100
}

/// Drops the ball, returning the number of rightward bounces — which is
/// the slot it lands in.
fn drop_path(ctx: &mut Ctx) -> Vec<bool> {
    (0..ROWS).map(|_| ctx.rng.below(2) == 1).collect()
}

fn draw_field(ctx: &mut Ctx, risk: Risk, depth: usize, right_so_far: usize, headline: &str, landed: Option<usize>, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    if !headline.is_empty() {
        ctx.screen.line(headline);
        ctx.screen.blank();
    }

    for d in 0..=ROWS {
        let mut line = String::from("  ");
        let indent = SPAN / 2 * (ROWS - d);
        line.push_str(&" ".repeat(indent));
        for i in 0..=d {
            if d == depth && i == right_so_far {
                theme.paint_into(&mut line, ui::theme::NEON_PINK, "●");
            } else if d <= depth {
                theme.paint_into(&mut line, ui::theme::DIM, "·");
            } else {
                theme.paint_into(&mut line, ui::theme::GOLD_DIM, "o");
            }
            if i < d {
                line.push_str(&" ".repeat(SPAN - 1));
            }
        }
        ctx.screen.line(&line);
    }

    let mut slot_line = String::from("  ");
    for (i, m) in risk.slots().iter().enumerate() {
        let cell = format!("{:^4}", fmt_mult(*m));
        if landed == Some(i) {
            theme.paint_into(&mut slot_line, ui::theme::GREEN, &cell);
        } else if *m >= 100 {
            theme.paint_into(&mut slot_line, ui::theme::GOLD, &cell);
        } else {
            theme.paint_into(&mut slot_line, ui::theme::DIM, &cell);
        }
    }
    ctx.screen.line(&slot_line);
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

/// Animates one drop and reports the slot.
fn drop_ball(ctx: &mut Ctx, risk: Risk, headline: &str) -> usize {
    let path = drop_path(ctx);
    let mut right = 0usize;
    for (d, went_right) in path.iter().enumerate() {
        draw_field(ctx, risk, d, right, headline, None, "");
        ctx.screen.present();
        // The ball accelerates as it falls, so the gaps shorten.
        ui::sleep_ms(150_u64.saturating_sub(d as u64 * 7).max(55));
        if *went_right {
            right += 1;
        }
    }
    draw_field(ctx, risk, ROWS, right, headline, None, "");
    ctx.screen.present();
    ui::sleep_ms(200);
    right
}

fn pick_risk(ctx: &mut Ctx, bank: i64) -> Option<Risk> {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    ctx.screen.line(&format!("  bank: {}", theme.win(&format!("{bank} chips"))));
    ctx.screen.blank();
    ctx.screen.line("  how sharp do you want the edges?");
    ctx.screen.blank();
    for r in [Risk::Low, Risk::Medium, Risk::High] {
        let s = r.slots();
        ctx.screen.line(&format!(
            "   {}  centre {}  ·  edge {}",
            theme.paint(ui::theme::GOLD, &format!("{:<7}", r.label())),
            theme.dim(&fmt_mult(s[SLOTS / 2])),
            theme.paint(ui::theme::NEON_PINK, &fmt_mult(s[0]))
        ));
    }
    ctx.screen.blank();
    ctx.screen.line(&widgets::footer(&theme, &[('l', "low"), ('m', "medium"), ('h', "high"), ('q', "leave")]));
    ctx.screen.present();
    match ui::choose_key(&['l', 'm', 'h', 'q'], 'q') {
        Some('l') => Some(Risk::Low),
        Some('m') => Some(Risk::Medium),
        Some('h') => Some(Risk::High),
        _ => None,
    }
}

pub fn play(ctx: &mut Ctx) {
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }
        let Some(risk) = pick_risk(ctx, bank) else { break };

        let Some(stake) = table::stake(ctx, TITLE, &format!("{} risk, twelve rows", risk.label()), bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let theme = ctx.theme();
        let headline = format!(
            "  {} · {}",
            theme.dim(&format!("{} risk", risk.label())),
            theme.paint(ui::theme::GOLD, &format!("{stake} chips"))
        );
        let slot = drop_ball(ctx, risk, &headline);

        ctx.store.bump("plinko.drops", 1);
        let payout = payout(risk, slot, stake);
        let delta = table::settle(ctx, KEY, stake, payout);

        let theme = ctx.theme();
        let mult = fmt_mult(risk.slots()[slot]);
        let note = if delta > 0 {
            theme.win(&format!("landed on {mult} — {payout} chips back"))
        } else {
            theme.lose(&format!("landed on {mult} — {payout} chips back"))
        };
        draw_field(ctx, risk, ROWS, slot, &headline, Some(slot), &note);
        table::verdict(ctx, delta);
        if !table::again(ctx, "drop another") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// Balls dropping on their own, cycling the risk profiles so all three
/// spreads get seen.
pub fn idle(ctx: &mut Ctx) {
    let profiles = [Risk::Low, Risk::Medium, Risk::High];
    let mut n = 0usize;
    loop {
        let risk = profiles[n % profiles.len()];
        n += 1;
        let stake = 10;
        let theme = ctx.theme();
        let headline = format!("  {} {}", theme.dim("demo drop —"), theme.paint(ui::theme::GOLD, &format!("{} risk", risk.label())));
        let slot = drop_ball(ctx, risk, &headline);
        let won = payout(risk, slot, stake);
        let theme = ctx.theme();
        let mult = fmt_mult(risk.slots()[slot]);
        let note = if won > stake {
            theme.win(&format!("{mult} — a good one"))
        } else {
            theme.dim(&mult)
        };
        draw_field(ctx, risk, ROWS, slot, &headline, Some(slot), &note);
        table::idle_footer(ctx, "demo drops only — nothing here touches your wallet");
        ctx.screen.present();
        if table::idle_hold(1_500) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C(12, k) — how many of the 4096 equally likely paths reach slot k.
    fn paths_to(slot: usize) -> i64 {
        let mut c: i64 = 1;
        for i in 0..slot {
            c = c * (ROWS - i) as i64 / (i as i64 + 1);
        }
        c
    }

    #[test]
    fn the_paths_across_every_slot_account_for_every_drop() {
        let total: i64 = (0..SLOTS).map(paths_to).sum();
        assert_eq!(total, 1 << ROWS, "twelve coin flips make 4096 paths");
    }

    #[test]
    fn every_risk_level_keeps_the_same_house_edge() {
        // Weighted by the real binomial odds of each slot, all three
        // profiles must return between 95% and 100% of the stake — so the
        // choice is about variance, never about value.
        for risk in [Risk::Low, Risk::Medium, Risk::High] {
            let slots = risk.slots();
            let weighted: i64 = (0..SLOTS).map(|k| paths_to(k) * slots[k]).sum();
            let rtp = weighted as f64 / (1 << ROWS) as f64 / 100.0;
            assert!(rtp > 0.95 && rtp <= 1.0, "{} risk returns {rtp:.4}", risk.label());
        }
    }

    #[test]
    fn every_profile_is_symmetric_and_lowest_in_the_middle() {
        for risk in [Risk::Low, Risk::Medium, Risk::High] {
            let s = risk.slots();
            for i in 0..SLOTS {
                assert_eq!(s[i], s[SLOTS - 1 - i], "{} risk is lopsided at {i}", risk.label());
            }
            let centre = s[SLOTS / 2];
            assert!(s.iter().all(|m| *m >= centre), "{} risk pays less than its centre somewhere", risk.label());
            assert!(s[0] > centre, "{} risk has no edge prize", risk.label());
        }
    }

    #[test]
    fn sharper_risk_means_a_bigger_edge_and_a_worse_middle() {
        let low = Risk::Low.slots();
        let med = Risk::Medium.slots();
        let high = Risk::High.slots();
        assert!(high[0] > med[0] && med[0] > low[0], "edges must climb with risk");
        assert!(high[SLOTS / 2] < med[SLOTS / 2] && med[SLOTS / 2] < low[SLOTS / 2], "centres must fall with risk");
    }

    #[test]
    fn a_payout_is_the_stake_scaled_by_the_slot() {
        assert_eq!(payout(Risk::Medium, 0, 10), 300); // 30x
        assert_eq!(payout(Risk::Medium, SLOTS / 2, 10), 5); // 0.5x
        assert_eq!(payout(Risk::High, 0, 1), 220); // 220x on a single chip
        assert_eq!(payout(Risk::Low, 0, 10), 50); // 5x
    }

    #[test]
    fn a_drop_always_lands_on_the_board() {
        let mut store = crate::stats::Store::blank();
        let mut rng = crate::rng::Rng::from_seed(5);
        let mut screen = crate::ui::Screen::headless();
        let mut ctx = Ctx { rng: &mut rng, store: &mut store, screen: &mut screen };
        for _ in 0..500 {
            let path = drop_path(&mut ctx);
            assert_eq!(path.len(), ROWS);
            let slot = path.iter().filter(|r| **r).count();
            assert!(slot < SLOTS);
        }
    }

    #[test]
    fn multipliers_read_the_way_a_punter_says_them() {
        assert_eq!(fmt_mult(22_000), "220x");
        assert_eq!(fmt_mult(650), "6.5x");
        assert_eq!(fmt_mult(170), "1.7x");
        assert_eq!(fmt_mult(20), "0.2x");
        assert_eq!(fmt_mult(100), "1x");
    }
}
