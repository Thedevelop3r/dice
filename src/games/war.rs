//! Casino War — the simplest game in the building: high card wins.
//!
//! The only decision is what to do on a tie, and it is a real one. Going to
//! war doubles what is at risk to win back even money on the raise alone,
//! while surrender hands back half the ante and ends it. Both are worse
//! than even, which is exactly where the house lives on this table.

use super::cards::{self, Card, Shoe};
use super::{table, Ctx};
use crate::economy::Wallet;
use crate::rng::Rng;
use crate::ui::{self, card_art, widgets};

const KEY: &str = "war";
const TITLE: &str = "CASINO WAR";

/// Cards burned between the tie and the second deal, purely for the
/// theatre of it — the same three a real pit would burn.
const BURN: usize = 3;

/// What a settled round returns, gross, against everything staked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Won on the first card.
    Straight,
    /// Took half the ante back rather than fight.
    Surrender,
    /// Went to war and took it.
    WarWon,
    /// Went to war and lost the lot.
    WarLost,
    /// Went to war and tied again — the best result on the table.
    WarTied,
    /// Lost on the first card.
    Lost,
}

impl Outcome {
    /// Gross chips back, given the ante and (where it applies) an equal raise.
    fn payout(self, ante: i64) -> i64 {
        match self {
            // Even money on the ante.
            Outcome::Straight => ante * 2,
            // Half the ante is kept by the house; half comes back.
            Outcome::Surrender => ante / 2,
            // The ante pushes and the raise is paid even money, so the two
            // antes staked return three.
            Outcome::WarWon => ante * 3,
            // A second tie pays 2:1 on the raise on top of the push.
            Outcome::WarTied => ante * 4,
            Outcome::WarLost | Outcome::Lost => 0,
        }
    }

    fn total_staked(self, ante: i64) -> i64 {
        match self {
            Outcome::WarWon | Outcome::WarLost | Outcome::WarTied => ante * 2,
            _ => ante,
        }
    }

    fn line(self) -> &'static str {
        match self {
            Outcome::Straight => "your card takes it",
            Outcome::Surrender => "you took half back",
            Outcome::WarWon => "you won the war",
            Outcome::WarLost => "the war went to the dealer",
            Outcome::WarTied => "tied again — the war pays 2:1",
            Outcome::Lost => "the dealer's card takes it",
        }
    }
}

fn draw_table(ctx: &mut Ctx, you: &[Card], dealer: &[Card], burned: usize, headline: &str, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    if !headline.is_empty() {
        ctx.screen.line(headline);
        ctx.screen.blank();
    }
    ctx.screen.line("  dealer");
    let none = vec![false; dealer.len()];
    for line in card_art::hand_block(&theme, &cards::labels(dealer), &none) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line("  you");
    let none = vec![false; you.len()];
    for line in card_art::hand_block(&theme, &cards::labels(you), &none) {
        ctx.screen.line(&line);
    }
    if burned > 0 {
        ctx.screen.blank();
        ctx.screen.line(&theme.dim(&format!("  {burned} cards burned")));
    }
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

/// Deals both cards with a beat between them, the dealer's last.
fn deal_pair(ctx: &mut Ctx, shoe: &mut Shoe, you: &mut Vec<Card>, dealer: &mut Vec<Card>, burned: usize, headline: &str) {
    let mine = shoe.draw(ctx.rng);
    you.push(mine);
    draw_table(ctx, you, dealer, burned, headline, "and the dealer...");
    ctx.screen.present();
    ui::sleep_ms(650);
    let theirs = shoe.draw(ctx.rng);
    dealer.push(theirs);
    draw_table(ctx, you, dealer, burned, headline, "");
    ctx.screen.present();
    ui::sleep_ms(500);
}

pub fn play(ctx: &mut Ctx) {
    let mut shoe = Shoe::new(ctx.rng, 6);
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }

        // A war needs a second ante on the table, so the opening stake is
        // capped at half the bank — otherwise a tie would be unplayable.
        let ceiling = (bank / 2).max(1);
        let Some(ante) = table::stake(ctx, TITLE, "high card wins · a tie lets you go to war", ceiling, 10.min(ceiling)) else {
            break;
        };
        if !Wallet::new(ctx.store).spend_chips(ante) {
            continue;
        }

        let theme = ctx.theme();
        let headline = format!("  {}", theme.paint(ui::theme::GOLD, &format!("{ante} chips on the line")));
        let mut you: Vec<Card> = Vec::new();
        let mut dealer: Vec<Card> = Vec::new();
        deal_pair(ctx, &mut shoe, &mut you, &mut dealer, 0, &headline);

        let outcome = match you[0].poker_rank().cmp(&dealer[0].poker_rank()) {
            std::cmp::Ordering::Greater => Outcome::Straight,
            std::cmp::Ordering::Less => Outcome::Lost,
            std::cmp::Ordering::Equal => {
                let theme = ctx.theme();
                let note = theme.accent("a tie — go to war, or take half your ante back?");
                draw_table(ctx, &you, &dealer, 0, &headline, &note);
                let theme = ctx.theme();
                ctx.screen.blank();
                ctx.screen.line(&widgets::footer(&theme, &[('w', &format!("go to war (another {ante})")), ('s', "surrender for half")]));
                ctx.screen.present();
                if ui::choose_key(&['w', 's'], 's') == Some('w') && Wallet::new(ctx.store).spend_chips(ante) {
                    for _ in 0..BURN {
                        shoe.draw(ctx.rng);
                        draw_table(ctx, &you, &dealer, BURN, &headline, &ctx.theme().dim("burning three..."));
                        ctx.screen.present();
                        ui::sleep_ms(280);
                    }
                    deal_pair(ctx, &mut shoe, &mut you, &mut dealer, BURN, &headline);
                    match you[1].poker_rank().cmp(&dealer[1].poker_rank()) {
                        std::cmp::Ordering::Greater => Outcome::WarWon,
                        std::cmp::Ordering::Less => Outcome::WarLost,
                        std::cmp::Ordering::Equal => Outcome::WarTied,
                    }
                } else {
                    Outcome::Surrender
                }
            }
        };

        ctx.store.bump("war.hands", 1);
        if matches!(outcome, Outcome::WarWon | Outcome::WarLost | Outcome::WarTied) {
            ctx.store.bump("war.wars", 1);
        }
        let staked = outcome.total_staked(ante);
        let delta = table::settle(ctx, KEY, staked, outcome.payout(ante));

        let theme = ctx.theme();
        let note = if delta > 0 { theme.win(outcome.line()) } else { theme.lose(outcome.line()) };
        let burned = if staked > ante { BURN } else { 0 };
        draw_table(ctx, &you, &dealer, burned, &headline, &note);
        table::verdict(ctx, delta);
        if !table::again(ctx, "another card") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// A table dealing to itself, with a fictional punter who always goes to
/// war — which is the more watchable of the two choices.
pub fn idle(ctx: &mut Ctx) {
    let mut shoe = Shoe::new(ctx.rng, 6);
    loop {
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let ante = 10 + 5 * ctx.rng.below(5) as i64;
        let theme = ctx.theme();
        let headline = format!("  {} antes {ante}", theme.accent(punter));
        let mut you: Vec<Card> = Vec::new();
        let mut dealer: Vec<Card> = Vec::new();
        deal_pair(ctx, &mut shoe, &mut you, &mut dealer, 0, &headline);

        let outcome = match you[0].poker_rank().cmp(&dealer[0].poker_rank()) {
            std::cmp::Ordering::Greater => Outcome::Straight,
            std::cmp::Ordering::Less => Outcome::Lost,
            std::cmp::Ordering::Equal => {
                let theme = ctx.theme();
                let note = theme.accent(&format!("a tie — {punter} goes to war"));
                draw_table(ctx, &you, &dealer, 0, &headline, &note);
                ctx.screen.present();
                if table::idle_hold(1_100) {
                    return;
                }
                for _ in 0..BURN {
                    shoe.draw(ctx.rng);
                }
                deal_pair(ctx, &mut shoe, &mut you, &mut dealer, BURN, &headline);
                match you[1].poker_rank().cmp(&dealer[1].poker_rank()) {
                    std::cmp::Ordering::Greater => Outcome::WarWon,
                    std::cmp::Ordering::Less => Outcome::WarLost,
                    std::cmp::Ordering::Equal => Outcome::WarTied,
                }
            }
        };

        let won = outcome.payout(ante) - outcome.total_staked(ante);
        let theme = ctx.theme();
        let note = if won > 0 {
            theme.win(&format!("{} — {punter} is up {won}", outcome.line()))
        } else if won == 0 {
            theme.accent(outcome.line())
        } else {
            theme.lose(&format!("{} — {punter} is down {}", outcome.line(), -won))
        };
        let burned = if outcome.total_staked(ante) > ante { BURN } else { 0 };
        draw_table(ctx, &you, &dealer, burned, &headline, &note);
        table::idle_footer(ctx, "the table deals itself — no ante of yours is down");
        ctx.screen.present();
        if table::idle_hold(2_000) {
            return;
        }
    }
}

/// One hand, always going to war on a tie — the more interesting of the
/// two choices, and the one the table is really priced around.
pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    let mut shoe = Shoe::new(rng, 6);
    let ante = 100;
    let mine = shoe.draw(rng);
    let theirs = shoe.draw(rng);
    let out = match mine.poker_rank().cmp(&theirs.poker_rank()) {
        std::cmp::Ordering::Greater => Outcome::Straight,
        std::cmp::Ordering::Less => Outcome::Lost,
        std::cmp::Ordering::Equal => {
            for _ in 0..BURN {
                shoe.draw(rng);
            }
            match shoe.draw(rng).poker_rank().cmp(&shoe.draw(rng).poker_rank()) {
                std::cmp::Ordering::Greater => Outcome::WarWon,
                std::cmp::Ordering::Less => Outcome::WarLost,
                std::cmp::Ordering::Equal => Outcome::WarTied,
            }
        }
    };
    (out.total_staked(ante), out.payout(ante))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_straight_win_pays_even_money() {
        assert_eq!(Outcome::Straight.payout(10), 20);
        assert_eq!(Outcome::Straight.total_staked(10), 10);
    }

    #[test]
    fn a_loss_returns_nothing() {
        assert_eq!(Outcome::Lost.payout(10), 0);
        assert_eq!(Outcome::WarLost.payout(10), 0);
        assert_eq!(Outcome::WarLost.total_staked(10), 20, "a war puts two antes at risk");
    }

    #[test]
    fn surrender_hands_back_exactly_half_the_ante() {
        assert_eq!(Outcome::Surrender.payout(10), 5);
        assert_eq!(Outcome::Surrender.total_staked(10), 10);
        // Net of five lost on a ten-chip ante.
        assert_eq!(Outcome::Surrender.payout(10) - Outcome::Surrender.total_staked(10), -5);
    }

    #[test]
    fn winning_a_war_pays_the_raise_and_pushes_the_ante() {
        // Two antes at risk, three come back: a net win of exactly one ante.
        assert_eq!(Outcome::WarWon.payout(10), 30);
        assert_eq!(Outcome::WarWon.payout(10) - Outcome::WarWon.total_staked(10), 10);
    }

    #[test]
    fn a_second_tie_is_the_best_seat_in_the_house() {
        assert_eq!(Outcome::WarTied.payout(10), 40);
        assert_eq!(Outcome::WarTied.payout(10) - Outcome::WarTied.total_staked(10), 20);
        assert!(Outcome::WarTied.payout(10) > Outcome::WarWon.payout(10));
    }

    #[test]
    fn going_to_war_beats_surrender_only_if_you_win_it() {
        let ante = 10;
        let surrender = Outcome::Surrender.payout(ante) - Outcome::Surrender.total_staked(ante);
        let war_won = Outcome::WarWon.payout(ante) - Outcome::WarWon.total_staked(ante);
        let war_lost = Outcome::WarLost.payout(ante) - Outcome::WarLost.total_staked(ante);
        assert!(war_won > surrender, "winning a war must beat folding");
        assert!(war_lost < surrender, "losing a war must be worse than folding");
    }

    #[test]
    fn an_odd_ante_surrenders_in_the_houses_favour() {
        // Half of an odd ante rounds down, which is the house's way round.
        assert_eq!(Outcome::Surrender.payout(11), 5);
    }

    #[test]
    fn aces_are_the_top_card() {
        let ace = Card::new(1, 0);
        let king = Card::new(13, 1);
        assert!(ace.poker_rank() > king.poker_rank());
        let two = Card::new(2, 2);
        assert!(two.poker_rank() < king.poker_rank());
    }
}
