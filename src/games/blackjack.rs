//! Blackjack — heads-up against the house. Standard rules: dealer stands
//! on all 17s, blackjack pays 3:2, one card on a double down.
//!
//! The deck, the shoe and the hand rankings all live in `games::cards`,
//! shared with every other card table on the floor.

use super::cards::{self, Card, Shoe};
use super::{table, Ctx};
use crate::economy::{House, Wallet};
use crate::rng::Rng;
use crate::ui::{self, card_art, widgets};

/// What a finished hand returns, gross, and what to call it. Lifted out
/// of `play` so the audit harness settles hands through the same code the
/// table does rather than a restatement of it.
fn settle_hand(player: u32, dealer: u32, player_bj: bool, dealer_bj: bool, bet: i64) -> (i64, &'static str) {
    if player > 21 {
        (0, "you bust — dealer wins")
    } else if player_bj && dealer_bj {
        (bet, "both blackjack — push")
    } else if player_bj {
        (bet + bet * 3 / 2, "BLACKJACK! pays 3:2")
    } else if dealer_bj {
        (0, "dealer has blackjack")
    } else if dealer > 21 {
        (bet * 2, "dealer busts — you win")
    } else if player > dealer {
        (bet * 2, "you win")
    } else if player == dealer {
        (bet, "push")
    } else {
        (0, "dealer wins")
    }
}

/// Best total <=21 (aces flex between 11 and 1), and whether it's soft.
fn hand_total(cards: &[Card]) -> (u32, bool) {
    let mut total: u32 = cards.iter().map(|c| c.blackjack_value()).sum();
    let mut aces = cards.iter().filter(|c| c.rank == 1).count();
    let mut soft = aces > 0;
    while total > 21 && aces > 0 {
        total -= 10;
        aces -= 1;
    }
    if aces == 0 {
        soft = false;
    }
    (total, soft)
}

pub fn play(ctx: &mut Ctx) {
    let mut shoe = Shoe::new(ctx.rng, 1);
    loop {
        {
            let mut w = Wallet::new(ctx.store);
            if w.chips() <= 0 {
                w.ensure_solvent(50);
            }
        }
        let bank = Wallet::new(ctx.store).chips().max(1);

        let Some(stake) = widgets::number_picker(ctx.screen, 1, bank, 10.min(bank), 5, &[('m', bank)], |s, v| {
            let theme = s.theme;
            s.begin();
            ui::header(s, "BLACKJACK");
            s.blank();
            s.line(&format!("  bank: {}", theme.win(&format!("{bank} chips"))));
            s.blank();
            s.line(&format!("  bet: {}", theme.paint(ui::theme::GOLD, &format!("{v} chips"))));
            s.blank();
            s.line(&widgets::footer(&theme, &[('↑', "+5"), ('↓', "-5"), ('m', "max"), ('\u{23ce}', "deal")]));
        }) else {
            return;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let mut player = vec![shoe.draw(ctx.rng), shoe.draw(ctx.rng)];
        let mut dealer = vec![shoe.draw(ctx.rng), shoe.draw(ctx.rng)];
        let mut bet = stake;

        render(ctx, &player, &dealer, true, stake, "");
        ui::sleep_ms(500);

        let (player_bj, _) = hand_total(&player);
        let (dealer_bj, _) = hand_total(&dealer);
        let player_has_bj = player_bj == 21;
        let dealer_has_bj = dealer_bj == 21;

        if !(player_has_bj || dealer_has_bj) {
            // player's turn
            let mut can_double = true;
            loop {
                let (total, _) = hand_total(&player);
                if total >= 21 {
                    break;
                }
                let can_afford_double = can_double && Wallet::new(ctx.store).chips() >= bet;
                render_prompt(ctx, &player, &dealer, bet, can_afford_double);
                let mut valid = vec!['h', 's'];
                if can_afford_double {
                    valid.push('d');
                }
                match ui::choose_key(&valid, 's') {
                    Some('h') => {
                        player.push(shoe.draw(ctx.rng));
                        can_double = false;
                        render(ctx, &player, &dealer, true, bet, "");
                        ui::sleep_ms(400);
                    }
                    Some('d') => {
                        let _ = Wallet::new(ctx.store).spend_chips(bet);
                        bet *= 2;
                        player.push(shoe.draw(ctx.rng));
                        render(ctx, &player, &dealer, true, bet, "doubled down");
                        ui::sleep_ms(600);
                        break;
                    }
                    _ => break,
                }
            }
        }

        let (player_total, _) = hand_total(&player);
        let mut dealer_note = String::new();
        if player_total <= 21 && !player_has_bj {
            render(ctx, &player, &dealer, false, bet, "dealer reveals...");
            ui::sleep_ms(600);
            loop {
                let (total, soft) = hand_total(&dealer);
                if total > 21 || total >= 17 {
                    let _ = soft;
                    break;
                }
                dealer.push(shoe.draw(ctx.rng));
                render(ctx, &player, &dealer, false, bet, "dealer hits...");
                ui::sleep_ms(650);
            }
            dealer_note = "dealer stands".to_string();
        } else if player_has_bj || dealer_has_bj {
            render(ctx, &player, &dealer, false, bet, "");
            ui::sleep_ms(900);
        }

        let (player_final, _) = hand_total(&player);
        let (dealer_final, _) = hand_total(&dealer);
        let theme = ctx.theme();

        let (delta, outcome) = settle_hand(player_final, dealer_final, player_has_bj, dealer_has_bj, bet);

        if delta > 0 {
            Wallet::new(ctx.store).add_chips(delta);
        }
        House::record(ctx.store, "blackjack", delta - bet);
        if delta > bet {
            ctx.store.bump("blackjack.wins", 1);
        } else if delta == bet {
            ctx.store.bump("blackjack.pushes", 1);
        } else {
            ctx.store.bump("blackjack.losses", 1);
        }
        ctx.store.bump("blackjack.hands", 1);
        let _ = ctx.store.save();

        ctx.screen.begin();
        ui::header(ctx.screen, "BLACKJACK");
        ctx.screen.blank();
        draw_hands(ctx, &player, &dealer, false);
        ctx.screen.blank();
        if !dealer_note.is_empty() {
            ctx.screen.line(&theme.dim(&format!("  {dealer_note}")));
        }
        let win = delta > bet;
        for line in widgets::banner(&theme, &outcome.to_uppercase(), win || delta == bet) {
            ctx.screen.line(&format!("  {line}"));
        }
        let net = delta - bet;
        let sign = if net >= 0 { "+" } else { "" };
        ctx.screen.line(&format!("  {}", theme.paint(if net >= 0 { ui::theme::GREEN } else { ui::theme::RED }, &format!("{sign}{net} chips"))));
        ctx.screen.blank();
        ctx.screen.line(&widgets::footer(&theme, &[('y', "another hand"), ('n', "leave the table")]));
        ctx.screen.present();
        if !ui::confirm_key(true) {
            return;
        }
    }
}

fn draw_hands(ctx: &mut Ctx, player: &[Card], dealer: &[Card], hide_hole: bool) {
    let theme = ctx.theme();
    let (ptotal, _) = hand_total(player);
    let dshown: Vec<Card> = if hide_hole { dealer[..1].to_vec() } else { dealer.to_vec() };
    let (dtotal, _) = hand_total(&dshown);

    ctx.screen.line(&format!("  dealer {}", if hide_hole { String::new() } else { format!("({dtotal})") }));
    let hidden: Vec<bool> = (0..dealer.len()).map(|i| hide_hole && i == 1).collect();
    let cards = cards::labels(dealer);
    for line in card_art::hand_block(&theme, &cards, &hidden) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line(&format!("  you ({ptotal})"));
    let cards = cards::labels(player);
    let none_hidden = vec![false; player.len()];
    for line in card_art::hand_block(&theme, &cards, &none_hidden) {
        ctx.screen.line(&line);
    }
}

fn render(ctx: &mut Ctx, player: &[Card], dealer: &[Card], hide_hole: bool, bet: i64, note: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, "BLACKJACK");
    ctx.screen.blank();
    ctx.screen.line(&format!("  bet: {}", theme.paint(ui::theme::GOLD, &format!("{bet} chips"))));
    ctx.screen.blank();
    draw_hands(ctx, player, dealer, hide_hole);
    if !note.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&format!("  {}", theme.dim(note)));
    }
    ctx.screen.present();
}

fn render_prompt(ctx: &mut Ctx, player: &[Card], dealer: &[Card], bet: i64, can_double: bool) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, "BLACKJACK");
    ctx.screen.blank();
    ctx.screen.line(&format!("  bet: {}", theme.paint(ui::theme::GOLD, &format!("{bet} chips"))));
    ctx.screen.blank();
    draw_hands(ctx, player, dealer, true);
    ctx.screen.blank();
    let mut hints = vec![('h', "hit"), ('s', "stand")];
    if can_double {
        hints.push(('d', "double down"));
    }
    ctx.screen.line(&widgets::footer(&theme, &hints));
    ctx.screen.present();
}

/// A seat playing itself off the book: hit below seventeen, stand on it,
/// and never touch the player's wallet doing so.
pub fn idle(ctx: &mut Ctx) {
    let mut shoe = Shoe::new(ctx.rng, 6);
    loop {
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let bet = 10 + 5 * ctx.rng.below(5) as i64;
        let mut player = vec![shoe.draw(ctx.rng), shoe.draw(ctx.rng)];
        let mut dealer = vec![shoe.draw(ctx.rng), shoe.draw(ctx.rng)];

        render(ctx, &player, &dealer, true, bet, &format!("{punter} is dealt in"));
        if table::idle_hold(1_100) {
            return;
        }

        while hand_total(&player).0 < 17 {
            player.push(shoe.draw(ctx.rng));
            render(ctx, &player, &dealer, true, bet, &format!("{punter} hits"));
            if table::idle_hold(900) {
                return;
            }
        }
        if hand_total(&player).0 <= 21 {
            while hand_total(&dealer).0 < 17 {
                dealer.push(shoe.draw(ctx.rng));
                render(ctx, &player, &dealer, false, bet, "the dealer draws");
                if table::idle_hold(900) {
                    return;
                }
            }
        }

        let p = hand_total(&player).0;
        let d = hand_total(&dealer).0;
        let won = if p > 21 {
            -bet
        } else if d > 21 || p > d {
            bet
        } else if p == d {
            0
        } else {
            -bet
        };

        let theme = ctx.theme();
        ctx.screen.begin();
        ui::header(ctx.screen, "BLACKJACK");
        ctx.screen.blank();
        ctx.screen.line(&format!("  {} · {}", theme.accent(punter), theme.paint(ui::theme::GOLD, &format!("{bet} chips"))));
        ctx.screen.blank();
        draw_hands(ctx, &player, &dealer, false);
        let theme = ctx.theme();
        ctx.screen.blank();
        ctx.screen.line(&format!(
            "  {}",
            if won > 0 {
                theme.win(&format!("{p} against {d} — {punter} collects {won}"))
            } else if won == 0 {
                theme.accent(&format!("{p} against {d} — a push"))
            } else if p > 21 {
                theme.lose(&format!("{punter} busts on {p}"))
            } else {
                theme.lose(&format!("{p} against {d} — the dealer takes it"))
            }
        ));
        table::idle_footer(ctx, "the seat plays itself — no bet of yours is on the table");
        ctx.screen.present();
        if table::idle_hold(2_400) {
            return;
        }
    }
}

/// One hand played off the book — hit below seventeen, stand on it, no
/// doubling. That is short of full basic strategy, so the return this
/// produces is a floor rather than the table's headline figure.
pub fn simulate(rng: &mut Rng) -> (i64, i64) {
    let mut shoe = Shoe::new(rng, 6);
    let bet = 100;
    let mut player = vec![shoe.draw(rng), shoe.draw(rng)];
    let mut dealer = vec![shoe.draw(rng), shoe.draw(rng)];
    let player_bj = hand_total(&player).0 == 21;
    let dealer_bj = hand_total(&dealer).0 == 21;
    if !(player_bj || dealer_bj) {
        while hand_total(&player).0 < 17 {
            player.push(shoe.draw(rng));
        }
        if hand_total(&player).0 <= 21 {
            while hand_total(&dealer).0 < 17 {
                dealer.push(shoe.draw(rng));
            }
        }
    }
    let (back, _) = settle_hand(hand_total(&player).0, hand_total(&dealer).0, player_bj, dealer_bj, bet);
    (bet, back)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(rank: u8, suit: u8) -> Card {
        Card::new(rank, suit)
    }

    #[test]
    fn aces_flex_between_eleven_and_one() {
        // A + K = soft 21 (blackjack).
        assert_eq!(hand_total(&[card(1, 0), card(13, 0)]), (21, true));
        // A + 5 + 8 = 11+5+8=24 busts soft, so the ace drops to 1 -> 14.
        assert_eq!(hand_total(&[card(1, 0), card(5, 0), card(8, 0)]), (14, false));
        // A + A + 9 = 11+11+9=31 -> one ace drops -> 21, still soft (one ace left at 11).
        assert_eq!(hand_total(&[card(1, 0), card(1, 1), card(9, 0)]), (21, true));
    }

    #[test]
    fn face_cards_are_worth_ten() {
        assert_eq!(hand_total(&[card(11, 0), card(12, 0)]), (20, false));
        assert_eq!(hand_total(&[card(13, 0), card(10, 0)]), (20, false));
    }

    #[test]
    fn plain_bust_has_no_flexible_aces_left() {
        assert_eq!(hand_total(&[card(10, 0), card(9, 0), card(5, 0)]), (24, false));
    }

    #[test]
    fn shoe_deals_all_fifty_two_cards_before_reshuffling() {
        let mut rng = crate::rng::Rng::from_seed(1);
        let mut shoe = Shoe::new(&mut rng, 1);
        assert_eq!(shoe.len(), 52);
        for _ in 0..52 {
            shoe.draw(&mut rng);
        }
        assert!(shoe.is_empty());
    }
}
