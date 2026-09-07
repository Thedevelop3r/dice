//! Big Six — the money wheel, and the one table on the floor whose whole
//! point is the board itself.
//!
//! A real Big Six wheel is a vertical disc of 54 segments. Drawn head-on in
//! a terminal that would be a circle nobody could read, so it is drawn the
//! way a punter actually experiences it: the rim running past a fixed
//! pointer, decelerating into its stop. The spin is not decoration — the
//! landing segment is chosen first and the scroll is arranged to arrive on
//! it exactly, so what you watch is what you get.

use super::{table, Ctx};
use crate::economy::Wallet;
use crate::rng::Rng;
use crate::ui::{self, widgets};

const KEY: &str = "bigsix";
const TITLE: &str = "BIG SIX";

/// Cells shown either side of the pointer. Nine total keeps the rim inside
/// a narrow terminal while still reading as a moving wheel.
const VIEW: usize = 9;
const CENTRE: usize = VIEW / 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seg {
    One,
    Two,
    Five,
    Ten,
    Twenty,
    Joker,
    Logo,
}

impl Seg {
    /// The classic 54-segment layout: the more a segment pays, the fewer of
    /// them there are.
    const GROUPS: [(Seg, usize); 7] = [
        (Seg::One, 24),
        (Seg::Two, 15),
        (Seg::Five, 7),
        (Seg::Ten, 4),
        (Seg::Twenty, 2),
        (Seg::Joker, 1),
        (Seg::Logo, 1),
    ];

    fn face(self) -> &'static str {
        match self {
            Seg::One => "1",
            Seg::Two => "2",
            Seg::Five => "5",
            Seg::Ten => "10",
            Seg::Twenty => "20",
            Seg::Joker => "JKR",
            Seg::Logo => "★",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Seg::One => "the 1",
            Seg::Two => "the 2",
            Seg::Five => "the 5",
            Seg::Ten => "the 10",
            Seg::Twenty => "the 20",
            Seg::Joker => "the joker",
            Seg::Logo => "the house star",
        }
    }

    /// Odds against: a winning bet returns the stake plus this multiple.
    fn pays(self) -> i64 {
        match self {
            Seg::One => 1,
            Seg::Two => 2,
            Seg::Five => 5,
            Seg::Ten => 10,
            Seg::Twenty => 20,
            Seg::Joker | Seg::Logo => 40,
        }
    }

}

/// The rim, dealt round-robin so the big payouts sit apart rather than
/// bunching into one arc — which is also how a real wheel is laid out.
fn rim() -> Vec<Seg> {
    let total: usize = Seg::GROUPS.iter().map(|(_, n)| n).sum();
    let mut left: Vec<(Seg, usize)> = Seg::GROUPS.to_vec();
    let mut out = Vec::with_capacity(total);
    while out.len() < total {
        for (seg, remaining) in left.iter_mut() {
            if *remaining > 0 {
                out.push(*seg);
                *remaining -= 1;
                if out.len() == total {
                    break;
                }
            }
        }
    }
    out
}

/// Gross returned to the player: stake plus winnings on a match, nothing
/// otherwise.
fn payout(bet: Seg, landed: Seg, stake: i64) -> i64 {
    if bet == landed {
        stake * (bet.pays() + 1)
    } else {
        0
    }
}

fn draw_wheel(ctx: &mut Ctx, rim: &[Seg], offset: usize, headline: &str, note: &str) {
    let theme = ctx.theme();
    let n = rim.len();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    if !headline.is_empty() {
        ctx.screen.line(headline);
        ctx.screen.blank();
    }

    // The pointer sits over the middle cell; each cell is five columns
    // wide including its right-hand divider.
    let pointer_col = 3 + CENTRE * 5 + 2;
    ctx.screen.line(&format!("{}{}", " ".repeat(pointer_col), theme.paint(ui::theme::NEON_PINK, "▼")));

    let mut top = String::from("   ┌");
    let mut mid = String::from("   │");
    let mut bot = String::from("   └");
    for i in 0..VIEW {
        let seg = rim[(offset + i) % n];
        let cell = format!("{:^4}", seg.face());
        if i == CENTRE {
            theme.paint_into(&mut mid, ui::theme::GOLD, &cell);
        } else {
            theme.paint_into(&mut mid, ui::theme::DIM, &cell);
        }
        top.push_str("────");
        bot.push_str("────");
        if i + 1 < VIEW {
            top.push('┬');
            bot.push('┴');
            mid.push('│');
        }
    }
    top.push('┐');
    bot.push('┘');
    mid.push('│');
    ctx.screen.line(&theme.paint(ui::theme::GOLD_DIM, &top));
    ctx.screen.line(&mid);
    ctx.screen.line(&theme.paint(ui::theme::GOLD_DIM, &bot));
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

/// Spins the rim to a stop on `target`. The scroll is worked backwards
/// from where it must finish, so the wheel really does land where the
/// draw already decided it would.
fn spin(ctx: &mut Ctx, rim: &[Seg], target: usize, headline: &str) {
    let n = rim.len();
    let frames = 46usize;
    // At the last frame the pointer must sit over `target`.
    let final_offset = (target + n - CENTRE) % n;
    let start = (final_offset + n - (frames - 1) % n) % n;
    for f in 0..frames {
        let offset = (start + f) % n;
        let note = if f + 1 == frames { "" } else { "the wheel is turning..." };
        draw_wheel(ctx, rim, offset, headline, note);
        ctx.screen.present();
        // Quadratic ease-out: a blur at first, one clack at a time by the end.
        ui::sleep_ms(22 + (f as u64 * f as u64) / 14);
    }
}

fn board(ctx: &mut Ctx) {
    let theme = ctx.theme();
    ctx.screen.line(&theme.dim("  segments        pays"));
    for (seg, count) in Seg::GROUPS {
        ctx.screen.line(&format!(
            "   {}  {:>2} of 54    {}",
            theme.paint(ui::theme::GOLD, &format!("{:<4}", seg.face())),
            count,
            theme.dim(&format!("{}:1", seg.pays()))
        ));
    }
}

pub fn play(ctx: &mut Ctx) {
    let rim = rim();
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
        ctx.screen.line(&widgets::footer(
            &ctx.theme(),
            &[('1', "the 1"), ('2', "the 2"), ('5', "the 5"), ('t', "the 10"), ('w', "the 20"), ('j', "joker"), ('x', "star"), ('q', "leave")],
        ));
        ctx.screen.present();

        let bet = match ui::choose_key(&['1', '2', '5', 't', 'w', 'j', 'x', 'q'], 'q') {
            Some('1') => Seg::One,
            Some('2') => Seg::Two,
            Some('5') => Seg::Five,
            Some('t') => Seg::Ten,
            Some('w') => Seg::Twenty,
            Some('j') => Seg::Joker,
            Some('x') => Seg::Logo,
            _ => break,
        };

        let label = format!("backing {} at {}:1", bet.name(), bet.pays());
        let Some(stake) = table::stake(ctx, TITLE, &label, bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let target = ctx.rng.below(rim.len());
        let landed = rim[target];
        let theme = ctx.theme();
        let headline = format!("  {} · {}", theme.dim(&label), theme.paint(ui::theme::GOLD, &format!("{stake} chips")));
        spin(ctx, &rim, target, &headline);

        ctx.store.bump("bigsix.spins", 1);
        let delta = table::settle(ctx, KEY, stake, payout(bet, landed, stake));

        let theme = ctx.theme();
        let verdict = if delta > 0 {
            theme.win(&format!("{} — paid {}:1", landed.name(), bet.pays()))
        } else {
            theme.lose(&format!("{} — not yours", landed.name()))
        };
        draw_wheel(ctx, &rim, (target + rim.len() - CENTRE) % rim.len(), &headline, &verdict);
        table::verdict(ctx, delta);
        if !table::again(ctx, "spin again") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// The wheel left turning on its own, with a fictional punter backing a
/// segment each spin.
pub fn idle(ctx: &mut Ctx) {
    let rim = rim();
    let choices = [Seg::One, Seg::Two, Seg::Five, Seg::Ten, Seg::Twenty, Seg::Joker, Seg::Logo];
    let mut bankrolls: Vec<(usize, i64)> = (0..4).map(|i| (i, 300i64)).collect();
    loop {
        let who = ctx.rng.below(bankrolls.len());
        let bet = choices[ctx.rng.below(choices.len())];
        let stake = 10 + 5 * ctx.rng.below(5) as i64;
        let name = table::BOT_NAMES[bankrolls[who].0 % table::BOT_NAMES.len()];
        let theme = ctx.theme();
        let headline = format!("  {} backs {} for {} chips", theme.accent(name), theme.paint(ui::theme::GOLD, bet.name()), stake);

        let target = ctx.rng.below(rim.len());
        let landed = rim[target];
        spin(ctx, &rim, target, &headline);

        let won = payout(bet, landed, stake) - stake;
        bankrolls[who].1 += won;
        if bankrolls[who].1 <= 0 {
            bankrolls[who].1 = 300;
        }
        let theme = ctx.theme();
        let verdict = if won > 0 {
            theme.win(&format!("{} — {name} collects {won}", landed.name()))
        } else {
            theme.lose(&format!("{} — {name} is down {}", landed.name(), -won))
        };
        draw_wheel(ctx, &rim, (target + rim.len() - CENTRE) % rim.len(), &headline, &verdict);
        table::idle_footer(ctx, "the wheel plays itself — no chips of yours are riding on it");
        ctx.screen.present();
        if table::idle_hold(2_200) {
            return;
        }
    }
}

/// One spin with a bet on `bet`, no rendering. The wheel's own draw is a
/// uniform pick over the rim, which is exactly what `play` does.
pub fn simulate_at(rng: &mut Rng, which: usize) -> (i64, i64) {
    let bet = bet(which);
    let wheel = rim();
    let landed = wheel[rng.below(wheel.len())];
    (100, payout(bet, landed, 100))
}

#[cfg(test)]
pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    simulate_at(rng, 0)
}

/// The segments the audit harness walks, by index — `Seg` stays private.
pub const BETS: usize = 7;

fn bet(i: usize) -> Seg {
    [Seg::One, Seg::Two, Seg::Five, Seg::Ten, Seg::Twenty, Seg::Joker, Seg::Logo][i % BETS]
}

pub fn bet_name(i: usize) -> &'static str {
    bet(i).name()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rim_is_the_classic_fifty_four_segments() {
        let r = rim();
        assert_eq!(r.len(), 54);
        for (seg, count) in Seg::GROUPS {
            assert_eq!(r.iter().filter(|s| **s == seg).count(), count, "{seg:?}");
        }
    }

    #[test]
    fn a_matching_bet_returns_the_stake_plus_its_odds() {
        assert_eq!(payout(Seg::One, Seg::One, 10), 20); // 1:1
        assert_eq!(payout(Seg::Five, Seg::Five, 10), 60); // 5:1
        assert_eq!(payout(Seg::Twenty, Seg::Twenty, 10), 210); // 20:1
        assert_eq!(payout(Seg::Joker, Seg::Joker, 10), 410); // 40:1
    }

    #[test]
    fn a_missed_bet_returns_nothing() {
        assert_eq!(payout(Seg::One, Seg::Two, 10), 0);
        assert_eq!(payout(Seg::Joker, Seg::Logo, 10), 0, "the joker and the star are separate bets");
    }

    #[test]
    fn every_segment_pays_less_than_its_true_odds() {
        // The house edge is the whole reason this wheel exists: a segment
        // appearing n times in 54 has true odds of (54-n)/n to 1, and the
        // board must always pay strictly less than that.
        let r = rim();
        for (seg, count) in Seg::GROUPS {
            let appearances = r.iter().filter(|s| **s == seg).count() as i64;
            assert_eq!(appearances, count as i64);
            let true_odds = (54 - appearances) as f64 / appearances as f64;
            assert!(
                (seg.pays() as f64) < true_odds,
                "{seg:?} pays {}:1 against true odds of {true_odds:.2}:1",
                seg.pays()
            );
        }
    }

    #[test]
    fn a_spin_lands_the_pointer_on_the_drawn_segment() {
        // The arithmetic the animation relies on: after `frames` steps from
        // `start`, the cell under the pointer must be the target.
        let r = rim();
        let n = r.len();
        let frames = 46usize;
        for target in 0..n {
            let final_offset = (target + n - CENTRE) % n;
            let start = (final_offset + n - (frames - 1) % n) % n;
            let landed_at = (start + frames - 1) % n;
            assert_eq!((landed_at + CENTRE) % n, target, "target {target}");
        }
    }

    #[test]
    fn every_segment_has_a_distinct_face() {
        let faces: Vec<&str> = Seg::GROUPS.iter().map(|(s, _)| s.face()).collect();
        let mut sorted = faces.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), faces.len(), "two segments would be indistinguishable on the rim");
    }

    #[test]
    fn every_segment_prices_exactly() {
        // Fifty-four segments, so no simulation is needed to know what the
        // wheel returns on any bet.
        let wheel = rim();
        for (seg, _) in Seg::GROUPS {
            let back: i64 = wheel.iter().map(|landed| payout(seg, *landed, 100)).sum();
            let rtp = back as f64 / (wheel.len() as f64 * 100.0);
            assert!(rtp < 1.0, "{} returns {rtp:.4}", seg.name());
            // Big Six is famously the meanest wheel on any floor; this is
            // the real thing, not a softened version, so the floor is low.
            assert!(rtp > 0.70, "{} returns {rtp:.4}", seg.name());
        }
    }
}
