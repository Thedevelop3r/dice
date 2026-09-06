//! Video Poker — Jacks or Better. Five cards, hold what you like, draw
//! once, and take whatever the paytable says.
//!
//! The name of the game is the low end of that table: a pair only pays if
//! it is jacks or better, so half the pairs you are dealt are worth
//! nothing. `payout_mult` is the whole game, and it leans on the shared
//! `cards::evaluate5` so the rankings here and at every other card table
//! are the same rankings.

use super::cards::{self, Card, HandRank, Shoe};
use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, card_art, widgets};

const KEY: &str = "vidpoker";
const TITLE: &str = "VIDEO POKER";
const HAND: usize = 5;

/// The paytable, best first. Multipliers are gross on the stake.
const PAYTABLE: [(&str, i64); 9] = [
    ("royal flush", 800),
    ("straight flush", 50),
    ("four of a kind", 25),
    ("full house", 9),
    ("flush", 6),
    ("straight", 4),
    ("three of a kind", 3),
    ("two pair", 2),
    ("jacks or better", 1),
];

/// What a finished hand pays, and what to call it. A pair below jacks is
/// the one hand that scores in poker and pays nothing here.
fn payout_mult(hand: &[Card]) -> (i64, &'static str) {
    let score = cards::evaluate5(hand);
    match score.rank {
        HandRank::StraightFlush if cards::is_royal(hand) => (800, "royal flush"),
        HandRank::StraightFlush => (50, "straight flush"),
        HandRank::Quads => (25, "four of a kind"),
        HandRank::FullHouse => (9, "full house"),
        HandRank::Flush => (6, "flush"),
        HandRank::Straight => (4, "straight"),
        HandRank::Trips => (3, "three of a kind"),
        HandRank::TwoPair => (2, "two pair"),
        // `kickers[0]` is the pair's own rank, ace-high.
        HandRank::Pair if score.kickers.first().is_some_and(|r| *r >= 11) => (1, "jacks or better"),
        _ => (0, "nothing"),
    }
}

fn draw_screen(ctx: &mut Ctx, hand: &[Card], held: &[bool], stake: i64, note: &str, footer: Option<String>) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    ctx.screen.line(&format!("  stake: {}", theme.paint(ui::theme::GOLD, &format!("{stake} chips"))));
    ctx.screen.blank();

    let hidden = vec![false; hand.len()];
    let scale = card_art::fit(hand.len(), 1, 16);
    for line in card_art::hand_block_scaled(&theme, &cards::labels(hand), &hidden, scale) {
        ctx.screen.line(&line);
    }
    // The hold markers sit directly under the card they belong to.
    let cell = scale.card_width() + 1;
    let mut marks = String::new();
    for (i, h) in held.iter().enumerate() {
        let tag = if *h { format!("[{}] HELD", i + 1) } else { format!("[{}]", i + 1) };
        let padded = format!("{tag:<cell$}");
        if *h {
            theme.paint_into(&mut marks, ui::theme::GOLD, &padded);
        } else {
            theme.paint_into(&mut marks, ui::theme::DIM, &padded);
        }
    }
    ctx.screen.line(&marks);

    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
    if let Some(f) = footer {
        ctx.screen.blank();
        ctx.screen.line(&f);
    }
}

fn paytable(ctx: &mut Ctx, stake: i64) {
    let theme = ctx.theme();
    ctx.screen.line(&theme.dim("  paytable, per chip staked"));
    for (name, mult) in PAYTABLE {
        ctx.screen.line(&format!(
            "   {}  {}",
            theme.paint(ui::theme::GOLD, &format!("{name:<16}")),
            theme.dim(&format!("{mult:>4}x   pays {}", mult * stake))
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
        paytable(ctx, 1);
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&ctx.theme(), &[('d', "deal a hand"), ('q', "leave")]));
        ctx.screen.present();
        if ui::choose_key(&['d', 'q'], 'q') != Some('d') {
            break;
        }

        let Some(stake) = table::stake(ctx, TITLE, "five cards, one draw, jacks or better to pay", bank, 10) else {
            break;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        // A fresh deck every hand — a video poker machine never carries
        // cards over between deals.
        let mut shoe = Shoe::new(ctx.rng, 1);
        let mut hand: Vec<Card> = Vec::with_capacity(HAND);
        let mut held = [false; HAND];
        for _ in 0..HAND {
            hand.push(shoe.draw(ctx.rng));
            draw_screen(ctx, &hand, &held[..hand.len()], stake, "dealing...", None);
            ctx.screen.present();
            ui::sleep_ms(260);
        }

        // Hold phase: 1-5 toggle, D draws.
        loop {
            let theme = ctx.theme();
            let foot = widgets::footer(&theme, &[('1', "…"), ('5', "toggle a hold"), ('d', "draw")]);
            let note = theme.dim("keep what you want, then draw");
            draw_screen(ctx, &hand, &held, stake, &note, Some(foot));
            ctx.screen.present();
            match ui::choose_key(&['1', '2', '3', '4', '5', 'd'], 'd') {
                Some('d') | None => break,
                Some(c) => {
                    let i = c.to_digit(10).unwrap() as usize - 1;
                    held[i] = !held[i];
                }
            }
        }

        for i in 0..HAND {
            if !held[i] {
                hand[i] = shoe.draw(ctx.rng);
                draw_screen(ctx, &hand, &held, stake, &ctx.theme().dim("drawing..."), None);
                ctx.screen.present();
                ui::sleep_ms(300);
            }
        }

        let (mult, name) = payout_mult(&hand);
        ctx.store.bump("vidpoker.hands", 1);
        let delta = table::settle(ctx, KEY, stake, stake * mult);

        let theme = ctx.theme();
        let note = if mult > 0 {
            theme.win(&format!("{name} — pays {mult}x"))
        } else {
            theme.lose("nothing there")
        };
        draw_screen(ctx, &hand, &held, stake, &note, None);
        table::verdict(ctx, delta);
        if !table::again(ctx, "another hand") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// A machine playing itself, holding the way a reasonable player would:
/// keep anything already paying, otherwise keep the high cards.
pub fn idle(ctx: &mut Ctx) {
    let stake = 10;
    loop {
        let mut shoe = Shoe::new(ctx.rng, 1);
        let mut hand: Vec<Card> = shoe.deal(ctx.rng, HAND);
        let mut held = [false; HAND];

        let theme = ctx.theme();
        draw_screen(ctx, &hand, &held, stake, &theme.dim("demo machine — dealt"), None);
        table::idle_footer(ctx, "demo credits only — nothing here touches your wallet");
        ctx.screen.present();
        if table::idle_hold(1_400) {
            return;
        }

        // Hold whatever is already paying; failing that, hold the ranks
        // that could still make a paying pair.
        let score = cards::evaluate5(&hand);
        if payout_mult(&hand).0 > 0 {
            let keep: Vec<u8> = score.kickers.iter().take(2).copied().collect();
            for (i, c) in hand.iter().enumerate() {
                held[i] = keep.contains(&c.poker_rank()) || score.rank >= HandRank::Straight;
            }
        } else {
            for (i, c) in hand.iter().enumerate() {
                held[i] = c.poker_rank() >= 11;
            }
        }
        let theme = ctx.theme();
        draw_screen(ctx, &hand, &held, stake, &theme.dim("holding..."), None);
        table::idle_footer(ctx, "demo credits only — nothing here touches your wallet");
        ctx.screen.present();
        if table::idle_hold(1_200) {
            return;
        }

        for i in 0..HAND {
            if !held[i] {
                hand[i] = shoe.draw(ctx.rng);
            }
        }
        let (mult, name) = payout_mult(&hand);
        let theme = ctx.theme();
        let note = if mult > 0 {
            theme.win(&format!("{name} — {mult}x, {} chips", mult * stake))
        } else {
            theme.dim("nothing there")
        };
        draw_screen(ctx, &hand, &held, stake, &note, None);
        table::idle_footer(ctx, "demo credits only — nothing here touches your wallet");
        ctx.screen.present();
        if table::idle_hold(2_200) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand(spec: &[(u8, u8)]) -> Vec<Card> {
        spec.iter().map(|(r, s)| Card::new(*r, *s)).collect()
    }

    #[test]
    fn every_paying_hand_pays_its_listed_rate() {
        assert_eq!(payout_mult(&hand(&[(10, 0), (11, 0), (12, 0), (13, 0), (1, 0)])), (800, "royal flush"));
        assert_eq!(payout_mult(&hand(&[(5, 0), (6, 0), (7, 0), (8, 0), (9, 0)])), (50, "straight flush"));
        assert_eq!(payout_mult(&hand(&[(7, 0), (7, 1), (7, 2), (7, 3), (2, 0)])), (25, "four of a kind"));
        assert_eq!(payout_mult(&hand(&[(7, 0), (7, 1), (7, 2), (2, 3), (2, 0)])), (9, "full house"));
        assert_eq!(payout_mult(&hand(&[(2, 0), (5, 0), (9, 0), (11, 0), (13, 0)])), (6, "flush"));
        assert_eq!(payout_mult(&hand(&[(5, 0), (6, 1), (7, 2), (8, 3), (9, 0)])), (4, "straight"));
        assert_eq!(payout_mult(&hand(&[(7, 0), (7, 1), (7, 2), (4, 3), (2, 0)])), (3, "three of a kind"));
        assert_eq!(payout_mult(&hand(&[(7, 0), (7, 1), (4, 2), (4, 3), (2, 0)])), (2, "two pair"));
    }

    #[test]
    fn a_pair_only_pays_from_jacks_up() {
        for rank in [11u8, 12, 13, 1] {
            let h = hand(&[(rank, 0), (rank, 1), (2, 2), (5, 3), (8, 0)]);
            assert_eq!(payout_mult(&h).0, 1, "a pair of rank {rank} should pay");
        }
        for rank in [2u8, 5, 9, 10] {
            let h = hand(&[(rank, 0), (rank, 1), (3, 2), (6, 3), (12, 0)]);
            assert_eq!(payout_mult(&h).0, 0, "a pair of rank {rank} should not pay");
        }
    }

    #[test]
    fn an_ace_pair_counts_as_jacks_or_better() {
        // Aces sort above kings, which is exactly why the pair rank has to
        // be read ace-high rather than off the raw 1..=13 rank.
        assert_eq!(payout_mult(&hand(&[(1, 0), (1, 1), (3, 2), (6, 3), (9, 0)])).0, 1);
    }

    #[test]
    fn a_busted_hand_pays_nothing() {
        assert_eq!(payout_mult(&hand(&[(2, 0), (5, 1), (9, 2), (11, 3), (13, 0)])), (0, "nothing"));
    }

    #[test]
    fn the_wheel_straight_pays_as_a_straight_not_a_royal() {
        assert_eq!(payout_mult(&hand(&[(1, 0), (2, 1), (3, 2), (4, 3), (5, 0)])).0, 4);
        assert_eq!(payout_mult(&hand(&[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0)])), (50, "straight flush"));
    }

    #[test]
    fn the_paytable_never_pays_a_worse_hand_more() {
        let mults: Vec<i64> = PAYTABLE.iter().map(|(_, m)| *m).collect();
        assert!(mults.windows(2).all(|w| w[0] > w[1]), "the paytable must descend: {mults:?}");
    }

    #[test]
    fn every_paytable_row_is_reachable() {
        // Each named row must be produced by some real hand, or it is a
        // promise the machine cannot keep.
        let samples: [(&str, Vec<Card>); 9] = [
            ("royal flush", hand(&[(10, 0), (11, 0), (12, 0), (13, 0), (1, 0)])),
            ("straight flush", hand(&[(5, 0), (6, 0), (7, 0), (8, 0), (9, 0)])),
            ("four of a kind", hand(&[(7, 0), (7, 1), (7, 2), (7, 3), (2, 0)])),
            ("full house", hand(&[(7, 0), (7, 1), (7, 2), (2, 3), (2, 0)])),
            ("flush", hand(&[(2, 0), (5, 0), (9, 0), (11, 0), (13, 0)])),
            ("straight", hand(&[(5, 0), (6, 1), (7, 2), (8, 3), (9, 0)])),
            ("three of a kind", hand(&[(7, 0), (7, 1), (7, 2), (4, 3), (2, 0)])),
            ("two pair", hand(&[(7, 0), (7, 1), (4, 2), (4, 3), (2, 0)])),
            ("jacks or better", hand(&[(12, 0), (12, 1), (3, 2), (6, 3), (9, 0)])),
        ];
        for (name, h) in samples {
            let (mult, got) = payout_mult(&h);
            assert_eq!(got, name);
            let listed = PAYTABLE.iter().find(|(n, _)| *n == name).unwrap().1;
            assert_eq!(mult, listed, "{name} pays {mult} but the table says {listed}");
        }
    }
}
