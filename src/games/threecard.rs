//! Three Card Poker — ante up, look at three cards, and decide whether they
//! are worth backing.
//!
//! Two things make this table its own game rather than a short poker hand.
//! The dealer must hold queen high or better to play at all, and when they
//! cannot the ante pays anyway while the raise merely comes back — so a
//! rotten dealer hand is worth as much as a good one of yours. And the ante
//! bonus pays on your cards alone, whether or not you go on to win.

use super::cards::{self, Card, Shoe, ThreeRank};
use super::{table, Ctx};
use crate::economy::Wallet;
use crate::rng::Rng;
use crate::ui::{self, card_art, widgets};

const KEY: &str = "threecard";
const TITLE: &str = "THREE CARD POKER";

/// The dealer plays only with queen high or better. Anything less and the
/// hand is over before it starts.
fn dealer_qualifies(hand: &[Card]) -> bool {
    let score = cards::evaluate3(hand);
    score.rank > ThreeRank::HighCard || score.kickers.first().is_some_and(|r| *r >= 12)
}

/// Paid on the player's own three cards regardless of the dealer, as a
/// multiple of the ante.
fn ante_bonus(hand: &[Card]) -> (i64, &'static str) {
    match cards::evaluate3(hand).rank {
        ThreeRank::StraightFlush => (5, "straight flush bonus"),
        ThreeRank::Trips => (4, "three of a kind bonus"),
        ThreeRank::Straight => (1, "straight bonus"),
        _ => (0, ""),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Folded,
    DealerShort,
    PlayerWins,
    DealerWins,
    Push,
}

impl Outcome {
    fn line(self) -> &'static str {
        match self {
            Outcome::Folded => "folded — the ante goes to the house",
            Outcome::DealerShort => "the dealer never qualified — the ante pays",
            Outcome::PlayerWins => "your hand takes it",
            Outcome::DealerWins => "the dealer's hand takes it",
            Outcome::Push => "dead heat — everything comes back",
        }
    }
}

/// How the hand ended, ignoring the ante bonus.
fn outcome(player: &[Card], dealer: &[Card], folded: bool) -> Outcome {
    if folded {
        return Outcome::Folded;
    }
    if !dealer_qualifies(dealer) {
        return Outcome::DealerShort;
    }
    match cards::evaluate3(player).cmp(&cards::evaluate3(dealer)) {
        std::cmp::Ordering::Greater => Outcome::PlayerWins,
        std::cmp::Ordering::Less => Outcome::DealerWins,
        std::cmp::Ordering::Equal => Outcome::Push,
    }
}

/// Gross chips back. `staked` is the ante alone on a fold, or ante plus an
/// equal play bet otherwise — `total_staked` reports which.
fn payout(out: Outcome, bonus: i64, ante: i64) -> i64 {
    let bonus_pay = bonus * ante;
    match out {
        // A fold forfeits the ante, and with it the bonus.
        Outcome::Folded => 0,
        // Ante paid, play returned untouched.
        Outcome::DealerShort => ante * 2 + ante + bonus_pay,
        // Both bets paid even money.
        Outcome::PlayerWins => ante * 4 + bonus_pay,
        Outcome::DealerWins => bonus_pay,
        Outcome::Push => ante * 2 + bonus_pay,
    }
}

fn total_staked(out: Outcome, ante: i64) -> i64 {
    match out {
        Outcome::Folded => ante,
        _ => ante * 2,
    }
}

fn draw_table(ctx: &mut Ctx, player: &[Card], dealer: &[Card], hide_dealer: bool, ante: i64, note: &str, footer: Option<String>) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    ctx.screen.line(&format!("  ante: {}", theme.paint(ui::theme::GOLD, &format!("{ante} chips"))));
    ctx.screen.blank();

    let label = if hide_dealer {
        "  dealer".to_string()
    } else {
        format!("  dealer — {}", theme.accent(cards::evaluate3(dealer).rank.name()))
    };
    ctx.screen.line(&label);
    let hidden = vec![hide_dealer; dealer.len()];
    for line in card_art::hand_block(&theme, &cards::labels(dealer), &hidden) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line(&format!("  you — {}", theme.accent(cards::evaluate3(player).rank.name())));
    let none = vec![false; player.len()];
    for line in card_art::hand_block(&theme, &cards::labels(player), &none) {
        ctx.screen.line(&line);
    }
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
    if let Some(f) = footer {
        ctx.screen.blank();
        ctx.screen.line(&f);
    }
}

pub fn play(ctx: &mut Ctx) {
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }

        // Playing on needs a second bet equal to the ante, so the ante is
        // capped at half the bank.
        let ceiling = (bank / 2).max(1);
        let Some(ante) = table::stake(ctx, TITLE, "dealer needs queen high to play", ceiling, 10.min(ceiling)) else {
            break;
        };
        if !Wallet::new(ctx.store).spend_chips(ante) {
            continue;
        }

        let mut shoe = Shoe::new(ctx.rng, 1);
        let player: Vec<Card> = shoe.deal(ctx.rng, 3);
        let dealer: Vec<Card> = shoe.deal(ctx.rng, 3);

        let theme = ctx.theme();
        draw_table(ctx, &player, &dealer, true, ante, &theme.dim("dealing..."), None);
        ctx.screen.present();
        ui::sleep_ms(700);

        let (bonus, bonus_name) = ante_bonus(&player);
        let theme = ctx.theme();
        let note = if bonus > 0 {
            theme.win(&format!("{bonus_name} — {bonus}:1 on your ante whatever happens"))
        } else {
            theme.dim("play on, or fold and lose the ante")
        };
        let foot = widgets::footer(&theme, &[('p', &format!("play (another {ante})")), ('f', "fold")]);
        draw_table(ctx, &player, &dealer, true, ante, &note, Some(foot));
        ctx.screen.present();

        let played = ui::choose_key(&['p', 'f'], 'p') == Some('p') && Wallet::new(ctx.store).spend_chips(ante);
        let out = outcome(&player, &dealer, !played);

        if played {
            let theme = ctx.theme();
            draw_table(ctx, &player, &dealer, false, ante, &theme.dim("the dealer turns over..."), None);
            ctx.screen.present();
            ui::sleep_ms(900);
        }

        ctx.store.bump("threecard.hands", 1);
        if bonus > 0 {
            ctx.store.bump("threecard.bonuses", 1);
        }
        let staked = total_staked(out, ante);
        let delta = table::settle(ctx, KEY, staked, payout(out, bonus, ante));

        let theme = ctx.theme();
        let mut note = if delta > 0 { theme.win(out.line()) } else { theme.lose(out.line()) };
        if bonus > 0 && played {
            note.push_str(&theme.accent(&format!(" · {bonus_name} paid {}", bonus * ante)));
        }
        draw_table(ctx, &player, &dealer, !played, ante, &note, None);
        table::verdict(ctx, delta);
        if !table::again(ctx, "another hand") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// A seat playing itself, folding the genuinely hopeless hands the way a
/// sensible player would — anything below queen-six-four.
pub fn idle(ctx: &mut Ctx) {
    loop {
        let mut shoe = Shoe::new(ctx.rng, 1);
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let ante = 10 + 5 * ctx.rng.below(4) as i64;
        let player: Vec<Card> = shoe.deal(ctx.rng, 3);
        let dealer: Vec<Card> = shoe.deal(ctx.rng, 3);

        let theme = ctx.theme();
        let note = theme.dim(&format!("{punter} antes {ante}"));
        draw_table(ctx, &player, &dealer, true, ante, &note, None);
        table::idle_footer(ctx, "the seat plays itself — no ante of yours is down");
        ctx.screen.present();
        if table::idle_hold(1_500) {
            return;
        }

        // The textbook line: play anything queen-six-four or better.
        let score = cards::evaluate3(&player);
        let plays = score.rank > ThreeRank::HighCard || score.kickers.first().is_some_and(|r| *r >= 12);
        let out = outcome(&player, &dealer, !plays);
        let (bonus, bonus_name) = ante_bonus(&player);

        let theme = ctx.theme();
        let action = if plays {
            theme.accent(&format!("{punter} plays on"))
        } else {
            theme.dim(&format!("{punter} folds"))
        };
        draw_table(ctx, &player, &dealer, !plays, ante, &action, None);
        table::idle_footer(ctx, "the seat plays itself — no ante of yours is down");
        ctx.screen.present();
        if table::idle_hold(1_400) {
            return;
        }

        let won = payout(out, bonus, ante) - total_staked(out, ante);
        let theme = ctx.theme();
        let mut note = if won > 0 {
            theme.win(&format!("{} — {punter} is up {won}", out.line()))
        } else if won == 0 {
            theme.accent(out.line())
        } else {
            theme.lose(&format!("{} — {punter} is down {}", out.line(), -won))
        };
        if bonus > 0 && plays {
            note.push_str(&theme.accent(&format!(" · {bonus_name}")));
        }
        draw_table(ctx, &player, &dealer, !plays, ante, &note, None);
        table::idle_footer(ctx, "the seat plays itself — no ante of yours is down");
        ctx.screen.present();
        if table::idle_hold(2_400) {
            return;
        }
    }
}

/// One hand played the textbook line — play anything queen-six-four or
/// better, fold the rest.
pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    let mut shoe = Shoe::new(rng, 1);
    let player: Vec<Card> = shoe.deal(rng, 3);
    let dealer: Vec<Card> = shoe.deal(rng, 3);
    let score = cards::evaluate3(&player);
    let plays = score.rank > ThreeRank::HighCard || score.kickers.first().is_some_and(|r| *r >= 12);
    let out = outcome(&player, &dealer, !plays);
    let ante = 100;
    (total_staked(out, ante), payout(out, ante_bonus(&player).0, ante))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand(spec: &[(u8, u8)]) -> Vec<Card> {
        spec.iter().map(|(r, s)| Card::new(*r, *s)).collect()
    }

    #[test]
    fn the_dealer_needs_queen_high_to_play() {
        assert!(dealer_qualifies(&hand(&[(12, 0), (5, 1), (2, 2)])), "queen high qualifies");
        assert!(!dealer_qualifies(&hand(&[(11, 0), (9, 1), (2, 2)])), "jack high does not");
        assert!(dealer_qualifies(&hand(&[(1, 0), (5, 1), (2, 2)])), "ace high qualifies");
        // Any made hand qualifies however low it is.
        assert!(dealer_qualifies(&hand(&[(2, 0), (2, 1), (5, 2)])), "a pair of deuces qualifies");
    }

    #[test]
    fn the_ante_bonus_pays_on_your_cards_alone() {
        assert_eq!(ante_bonus(&hand(&[(11, 0), (12, 0), (13, 0)])).0, 5, "straight flush");
        assert_eq!(ante_bonus(&hand(&[(7, 0), (7, 1), (7, 2)])).0, 4, "trips");
        assert_eq!(ante_bonus(&hand(&[(5, 0), (6, 1), (7, 2)])).0, 1, "straight");
        assert_eq!(ante_bonus(&hand(&[(2, 0), (9, 0), (13, 0)])).0, 0, "a flush pays no bonus");
        assert_eq!(ante_bonus(&hand(&[(7, 0), (7, 1), (9, 2)])).0, 0, "nor does a pair");
    }

    #[test]
    fn a_fold_loses_the_ante_and_the_bonus_with_it() {
        assert_eq!(payout(Outcome::Folded, 4, 10), 0);
        assert_eq!(total_staked(Outcome::Folded, 10), 10);
    }

    #[test]
    fn a_dealer_who_cannot_play_still_pays_the_ante() {
        // Two bets of ten down, thirty back: the ante paid and the play
        // came home untouched.
        assert_eq!(payout(Outcome::DealerShort, 0, 10), 30);
        assert_eq!(total_staked(Outcome::DealerShort, 10), 20);
        assert_eq!(payout(Outcome::DealerShort, 0, 10) - 20, 10);
    }

    #[test]
    fn beating_a_qualified_dealer_pays_both_bets() {
        // Twenty down, forty back — even money on each.
        assert_eq!(payout(Outcome::PlayerWins, 0, 10), 40);
        assert_eq!(payout(Outcome::PlayerWins, 0, 10) - total_staked(Outcome::PlayerWins, 10), 20);
    }

    #[test]
    fn a_push_returns_everything() {
        assert_eq!(payout(Outcome::Push, 0, 10), 20);
        assert_eq!(payout(Outcome::Push, 0, 10) - total_staked(Outcome::Push, 10), 0);
    }

    #[test]
    fn the_bonus_is_paid_even_on_a_losing_hand() {
        // Beaten by the dealer, but a straight flush still pays 5:1.
        assert_eq!(payout(Outcome::DealerWins, 5, 10), 50);
        assert_eq!(payout(Outcome::DealerWins, 5, 10) - total_staked(Outcome::DealerWins, 10), 30);
    }

    #[test]
    fn outcomes_read_off_the_two_hands() {
        let strong = hand(&[(1, 0), (1, 1), (5, 2)]);
        let weak = hand(&[(2, 0), (7, 1), (9, 2)]);
        let queen_high = hand(&[(12, 0), (7, 1), (3, 2)]);
        assert_eq!(outcome(&strong, &weak, false), Outcome::DealerShort, "nine high cannot play");
        assert_eq!(outcome(&strong, &queen_high, false), Outcome::PlayerWins);
        assert_eq!(outcome(&queen_high, &strong, false), Outcome::DealerWins);
        assert_eq!(outcome(&strong, &weak, true), Outcome::Folded);
    }

    #[test]
    fn a_three_card_straight_outranks_a_three_card_flush() {
        // Which is the rule that makes this game's bonus table make sense.
        let straight = hand(&[(5, 0), (6, 1), (7, 2)]);
        let flush = hand(&[(2, 0), (9, 0), (13, 0)]);
        assert_eq!(outcome(&straight, &flush, false), Outcome::PlayerWins);
        assert!(ante_bonus(&straight).0 > ante_bonus(&flush).0);
    }
}
