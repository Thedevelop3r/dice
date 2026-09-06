//! Baccarat (punto banco) — back the player, the banker, or the tie, then
//! watch a hand that plays itself.
//!
//! Nobody makes a decision after the bet is placed: the third-card rules
//! are fixed, which is why this is the one table where the whole drama is
//! in the deal. Those rules are in `banker_draws`, and they are the real
//! ones — the banker's move depends on the player's third card, not just
//! on its own total.

use super::cards::{self, Card, Shoe};
use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, card_art, widgets};

const KEY: &str = "baccarat";
const TITLE: &str = "BACCARAT";

/// The banker's 5% cut on a winning banker bet, which is what stops the
/// better of the two bets from being a free lunch.
const COMMISSION: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bet {
    Player,
    Banker,
    Tie,
}

impl Bet {
    fn name(self) -> &'static str {
        match self {
            Bet::Player => "player",
            Bet::Banker => "banker",
            Bet::Tie => "the tie",
        }
    }
}

/// Baccarat counts tens and courts as nothing and drops the tens digit.
fn pip(c: Card) -> u32 {
    match c.rank {
        1 => 1,
        n if n >= 10 => 0,
        n => n as u32,
    }
}

fn total(hand: &[Card]) -> u32 {
    hand.iter().map(|c| pip(*c)).sum::<u32>() % 10
}

/// Either side showing 8 or 9 on the deal ends the hand where it stands.
fn natural(hand: &[Card]) -> bool {
    matches!(total(hand), 8 | 9)
}

/// The banker's third-card rule. `player_third` is the pip value of the
/// card the player drew, or `None` if the player stood.
fn banker_draws(banker_total: u32, player_third: Option<u32>) -> bool {
    match player_third {
        // Player stood: the banker plays the player's own rule.
        None => banker_total <= 5,
        Some(t) => match banker_total {
            0..=2 => true,
            3 => t != 8,
            4 => (2..=7).contains(&t),
            5 => (4..=7).contains(&t),
            6 => (6..=7).contains(&t),
            _ => false,
        },
    }
}

/// Gross returned to the player. A player/banker bet pushes on a tie —
/// that is the one thing about this game that surprises people.
fn payout(bet: Bet, player: u32, banker: u32, stake: i64) -> i64 {
    let winner = match player.cmp(&banker) {
        std::cmp::Ordering::Greater => Bet::Player,
        std::cmp::Ordering::Less => Bet::Banker,
        std::cmp::Ordering::Equal => Bet::Tie,
    };
    match (bet, winner) {
        (Bet::Tie, Bet::Tie) => stake * 9,
        (Bet::Tie, _) => 0,
        (_, Bet::Tie) => stake,
        (b, w) if b == w => {
            if b == Bet::Banker {
                stake + stake * (100 - COMMISSION) / 100
            } else {
                stake * 2
            }
        }
        _ => 0,
    }
}

fn draw_table(ctx: &mut Ctx, player: &[Card], banker: &[Card], headline: &str, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    if !headline.is_empty() {
        ctx.screen.line(headline);
        ctx.screen.blank();
    }
    ctx.screen.line(&format!("  banker ({})", theme.paint(ui::theme::GOLD, &total(banker).to_string())));
    let none = vec![false; banker.len()];
    for line in card_art::hand_block(&theme, &cards::labels(banker), &none) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line(&format!("  player ({})", theme.paint(ui::theme::GOLD, &total(player).to_string())));
    let none = vec![false; player.len()];
    for line in card_art::hand_block(&theme, &cards::labels(player), &none) {
        ctx.screen.line(&line);
    }
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {note}"));
    }
}

/// Deals the hand out card by card so the count builds in front of you.
fn deal_hand(ctx: &mut Ctx, shoe: &mut Shoe, headline: &str) -> (Vec<Card>, Vec<Card>) {
    let mut player: Vec<Card> = Vec::with_capacity(3);
    let mut banker: Vec<Card> = Vec::with_capacity(3);
    for i in 0..4 {
        let c = shoe.draw(ctx.rng);
        if i % 2 == 0 {
            player.push(c);
        } else {
            banker.push(c);
        }
        draw_table(ctx, &player, &banker, headline, "dealing...");
        ctx.screen.present();
        ui::sleep_ms(420);
    }

    if natural(&player) || natural(&banker) {
        let theme = ctx.theme();
        let note = theme.accent("a natural — the hand stands");
        draw_table(ctx, &player, &banker, headline, &note);
        ctx.screen.present();
        ui::sleep_ms(900);
        return (player, banker);
    }

    let mut player_third = None;
    if total(&player) <= 5 {
        let c = shoe.draw(ctx.rng);
        player.push(c);
        player_third = Some(pip(c));
        draw_table(ctx, &player, &banker, headline, "player draws...");
        ctx.screen.present();
        ui::sleep_ms(700);
    }
    if banker_draws(total(&banker), player_third) {
        banker.push(shoe.draw(ctx.rng));
        draw_table(ctx, &player, &banker, headline, "banker draws...");
        ctx.screen.present();
        ui::sleep_ms(700);
    }
    (player, banker)
}

pub fn play(ctx: &mut Ctx) {
    let mut shoe = Shoe::new(ctx.rng, 6);
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
        ctx.screen.line(&theme.dim("  player pays 1:1 · banker pays 1:1 less 5% · the tie pays 8:1"));
        ctx.screen.line(&theme.dim("  a tie hands a player or banker bet its stake straight back"));
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&theme, &[('p', "player"), ('b', "banker"), ('t', "tie"), ('q', "leave")]));
        ctx.screen.present();

        let bet = match ui::choose_key(&['p', 'b', 't', 'q'], 'q') {
            Some('p') => Bet::Player,
            Some('b') => Bet::Banker,
            Some('t') => Bet::Tie,
            _ => break,
        };

        let Some(stake) = table::stake(ctx, TITLE, &format!("backing {}", bet.name()), bank, 10) else {
            continue;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let theme = ctx.theme();
        let headline = format!("  {} · {}", theme.dim(&format!("on {}", bet.name())), theme.paint(ui::theme::GOLD, &format!("{stake} chips")));
        let (player, banker) = deal_hand(ctx, &mut shoe, &headline);

        let (p, b) = (total(&player), total(&banker));
        ctx.store.bump("baccarat.hands", 1);
        let delta = table::settle(ctx, KEY, stake, payout(bet, p, b, stake));

        let theme = ctx.theme();
        let outcome = match p.cmp(&b) {
            std::cmp::Ordering::Greater => format!("player wins {p} to {b}"),
            std::cmp::Ordering::Less => format!("banker wins {b} to {p}"),
            std::cmp::Ordering::Equal => format!("tie at {p}"),
        };
        let note = if delta > 0 { theme.win(&outcome) } else { theme.lose(&outcome) };
        draw_table(ctx, &player, &banker, &headline, &note);
        table::verdict(ctx, delta);
        if !table::again(ctx, "another hand") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// A table dealing itself, with fictional punters split across the three
/// bets so every outcome gets a reaction.
pub fn idle(ctx: &mut Ctx) {
    let mut shoe = Shoe::new(ctx.rng, 6);
    loop {
        let bets = [Bet::Player, Bet::Banker, Bet::Tie];
        let backer = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let bet = bets[ctx.rng.below(bets.len())];
        let stake = 20 + 10 * ctx.rng.below(5) as i64;
        let theme = ctx.theme();
        let headline = format!("  {} is on {} for {stake}", theme.accent(backer), theme.paint(ui::theme::GOLD, bet.name()));

        let (player, banker) = deal_hand(ctx, &mut shoe, &headline);
        let (p, b) = (total(&player), total(&banker));
        let won = payout(bet, p, b, stake) - stake;
        let theme = ctx.theme();
        let outcome = match p.cmp(&b) {
            std::cmp::Ordering::Greater => format!("player {p}, banker {b}"),
            std::cmp::Ordering::Less => format!("banker {b}, player {p}"),
            std::cmp::Ordering::Equal => format!("tie at {p}"),
        };
        let note = if won > 0 {
            theme.win(&format!("{outcome} — {backer} collects {won}"))
        } else if won == 0 {
            theme.accent(&format!("{outcome} — {backer} gets the stake back"))
        } else {
            theme.lose(&format!("{outcome} — {backer} loses {stake}"))
        };
        draw_table(ctx, &player, &banker, &headline, &note);
        table::idle_footer(ctx, "the shoe deals itself — none of these bets are yours");
        ctx.screen.present();
        if table::idle_hold(2_400) {
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
    fn tens_and_courts_count_as_nothing() {
        for rank in [10, 11, 12, 13] {
            assert_eq!(pip(Card::new(rank, 0)), 0, "rank {rank}");
        }
        assert_eq!(pip(Card::new(1, 0)), 1, "an ace is one, not eleven");
        assert_eq!(pip(Card::new(9, 0)), 9);
    }

    #[test]
    fn a_total_drops_its_tens_digit() {
        // 9 + 8 = 17, which is a baccarat 7.
        assert_eq!(total(&hand(&[(9, 0), (8, 1)])), 7);
        // 10 + K = 0, the worst hand in the game.
        assert_eq!(total(&hand(&[(10, 0), (13, 1)])), 0);
        // 5 + 5 = 10, also nothing.
        assert_eq!(total(&hand(&[(5, 0), (5, 1)])), 0);
    }

    #[test]
    fn eights_and_nines_are_naturals() {
        assert!(natural(&hand(&[(4, 0), (4, 1)])));
        assert!(natural(&hand(&[(4, 0), (5, 1)])));
        assert!(!natural(&hand(&[(4, 0), (3, 1)])));
    }

    #[test]
    fn the_banker_follows_the_players_third_card() {
        // Standing player: the banker draws on five or less.
        assert!(banker_draws(5, None));
        assert!(!banker_draws(6, None));
        // Banker on 3 draws against anything but an eight.
        assert!(banker_draws(3, Some(7)));
        assert!(!banker_draws(3, Some(8)));
        // Banker on 4 draws only against 2-7.
        assert!(!banker_draws(4, Some(1)));
        assert!(banker_draws(4, Some(2)));
        assert!(banker_draws(4, Some(7)));
        assert!(!banker_draws(4, Some(8)));
        // Banker on 5 needs a 4-7.
        assert!(!banker_draws(5, Some(3)));
        assert!(banker_draws(5, Some(4)));
        // Banker on 6 needs a 6 or 7.
        assert!(!banker_draws(6, Some(5)));
        assert!(banker_draws(6, Some(6)));
        // Banker on 7 always stands.
        assert!(!banker_draws(7, Some(6)));
        // Two or less always draws.
        assert!(banker_draws(0, Some(8)));
        assert!(banker_draws(2, Some(8)));
    }

    #[test]
    fn the_banker_bet_pays_its_commission() {
        // 100 staked on a winning banker bet returns 100 + 95.
        assert_eq!(payout(Bet::Banker, 3, 7, 100), 195);
        // The player bet has no cut taken.
        assert_eq!(payout(Bet::Player, 7, 3, 100), 200);
    }

    #[test]
    fn a_tie_pushes_the_side_bets_and_pays_the_tie_bet() {
        assert_eq!(payout(Bet::Player, 5, 5, 10), 10, "a tie hands the stake back");
        assert_eq!(payout(Bet::Banker, 5, 5, 10), 10);
        assert_eq!(payout(Bet::Tie, 5, 5, 10), 90, "the tie pays 8:1");
    }

    #[test]
    fn a_losing_bet_returns_nothing() {
        assert_eq!(payout(Bet::Player, 3, 7, 10), 0);
        assert_eq!(payout(Bet::Banker, 7, 3, 10), 0);
        assert_eq!(payout(Bet::Tie, 7, 3, 10), 0);
    }

    #[test]
    fn the_banker_is_the_better_bet_but_only_just() {
        // Both win-side bets return more than the stake, and the banker's
        // commission keeps it below the player's headline price.
        assert!(payout(Bet::Banker, 3, 7, 100) > 100);
        assert!(payout(Bet::Banker, 3, 7, 100) < payout(Bet::Player, 7, 3, 100));
    }
}
