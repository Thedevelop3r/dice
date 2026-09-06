//! Hi-Lo — call the next card higher or lower, and keep calling for as long
//! as your nerve holds.
//!
//! Every call is priced off its real chance: with an ace showing there is
//! only one way for the next card to be higher, so that call pays a great
//! deal, and with a two showing it pays almost nothing. `call_mult` derives
//! that from the thirteen ranks rather than from a hand-written table, so
//! the odds and the prices can never drift apart. A tie loses — that, and
//! the small margin baked into `EDGE`, is the whole house edge.

use super::cards::{self, Card, Shoe};
use super::{table, Ctx};
use crate::economy::Wallet;
use crate::ui::{self, card_art, widgets};

const KEY: &str = "hilo";
const TITLE: &str = "HI-LO";

/// What the house keeps, in hundredths: every call is priced at 97% of its
/// true odds.
const EDGE: i64 = 97;
/// Ranks in a suit — the denominator behind every price on this table.
const RANKS: i64 = 13;

/// The multiplier, in hundredths, for calling higher (or lower) on a card
/// of `rank` (2..=14). `None` when the call is impossible — nothing beats
/// an ace, and nothing loses to a two.
fn call_mult(rank: u8, higher: bool) -> Option<i64> {
    let ways = if higher { 14 - rank as i64 } else { rank as i64 - 2 };
    if ways <= 0 {
        return None;
    }
    Some(EDGE * RANKS / ways)
}

/// `2.15x` — how a running multiplier reads.
fn fmt_mult(hundredths: i64) -> String {
    format!("{}.{:02}x", hundredths / 100, hundredths % 100)
}

fn draw_table(ctx: &mut Ctx, current: Card, previous: &[Card], pot: i64, stake: i64, note: &str, footer: Option<String>) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, TITLE);
    ctx.screen.blank();
    ctx.screen.line(&format!(
        "  {} · running {} · worth {}",
        theme.paint(ui::theme::GOLD, &format!("{stake} chips")),
        theme.accent(&fmt_mult(pot)),
        theme.win(&format!("{} chips", stake * pot / 100))
    ));
    ctx.screen.blank();
    let none = [false];
    for line in card_art::hand_block(&theme, &cards::labels(&[current]), &none) {
        ctx.screen.line(&line);
    }
    if !previous.is_empty() {
        ctx.screen.blank();
        let run: Vec<String> = previous.iter().map(|c| c.label()).collect();
        ctx.screen.line(&theme.dim(&format!("  so far: {}", run.join("  "))));
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

/// The price strip under the card, so the player is never guessing blind
/// about what a call is worth.
fn prices(theme: &crate::ui::Theme, rank: u8) -> String {
    let hi = call_mult(rank, true).map(fmt_mult).unwrap_or_else(|| "—".to_string());
    let lo = call_mult(rank, false).map(fmt_mult).unwrap_or_else(|| "—".to_string());
    format!("  higher pays {}   ·   lower pays {}", theme.win(&hi), theme.paint(ui::theme::CYAN, &lo))
}

/// Flips the next card over with a short beat, so the reveal lands.
fn reveal(ctx: &mut Ctx, next: Card, previous: &[Card], pot: i64, stake: i64) {
    let theme = ctx.theme();
    draw_table(ctx, next, previous, pot, stake, &theme.dim("turning..."), None);
    ctx.screen.present();
    ui::sleep_ms(550);
}

pub fn play(ctx: &mut Ctx) {
    let mut shoe = Shoe::new(ctx.rng, 1);
    loop {
        let (bank, restaked) = table::open_bank(ctx);
        if restaked {
            table::message(ctx, TITLE, &format!("broke — the house stakes you {} chips.", table::BAILOUT));
        }

        let Some(stake) = table::stake(ctx, TITLE, "call the next card higher or lower", bank, 10) else {
            break;
        };
        if !Wallet::new(ctx.store).spend_chips(stake) {
            continue;
        }

        let mut current = shoe.draw(ctx.rng);
        let mut seen: Vec<Card> = Vec::new();
        let mut pot: i64 = 100;
        let mut busted = false;
        ctx.store.bump("hilo.rounds", 1);

        loop {
            let rank = current.poker_rank();
            let can_hi = call_mult(rank, true).is_some();
            let can_lo = call_mult(rank, false).is_some();
            let theme = ctx.theme();
            let mut keys: Vec<(char, &str)> = Vec::new();
            if can_hi {
                keys.push(('h', "higher"));
            }
            if can_lo {
                keys.push(('l', "lower"));
            }
            let banked = stake * pot / 100;
            let cash_label = format!("take {banked} chips");
            if pot > 100 {
                keys.push(('c', &cash_label));
            }
            let foot = widgets::footer(&theme, &keys);
            draw_table(ctx, current, &seen, pot, stake, &prices(&theme, rank), Some(foot));
            ctx.screen.present();

            let mut valid: Vec<char> = Vec::new();
            if can_hi {
                valid.push('h');
            }
            if can_lo {
                valid.push('l');
            }
            if pot > 100 {
                valid.push('c');
            }
            let default = if can_hi { 'h' } else { 'l' };
            let choice = ui::choose_key(&valid, default);
            if choice == Some('c') {
                break;
            }
            let higher = choice == Some('h') && can_hi;
            let Some(mult) = call_mult(rank, higher) else { break };

            let next = shoe.draw(ctx.rng);
            seen.push(current);
            reveal(ctx, next, &seen, pot, stake);

            let correct = if higher { next.poker_rank() > rank } else { next.poker_rank() < rank };
            if !correct {
                busted = true;
                current = next;
                break;
            }
            pot = pot * mult / 100;
            current = next;
            ctx.store.bump("hilo.calls", 1);
        }

        let payout = if busted { 0 } else { stake * pot / 100 };
        let delta = table::settle(ctx, KEY, stake, payout);

        let theme = ctx.theme();
        let note = if busted {
            theme.lose(&format!("{} — the run ends there", current.label()))
        } else {
            theme.win(&format!("cashed out at {}", fmt_mult(pot)))
        };
        draw_table(ctx, current, &seen, pot, stake, &note, None);
        table::verdict(ctx, delta);
        if !table::again(ctx, "another run") {
            break;
        }
    }
    table::cash_out(ctx, TITLE);
}

/// A demo run that always calls the side with the better chance and cashes
/// out once the multiplier gets interesting — which is the strategy that
/// makes the game look most like itself.
pub fn idle(ctx: &mut Ctx) {
    let mut shoe = Shoe::new(ctx.rng, 1);
    loop {
        let punter = table::BOT_NAMES[ctx.rng.below(table::BOT_NAMES.len())];
        let stake = 10 + 5 * ctx.rng.below(5) as i64;
        let mut current = shoe.draw(ctx.rng);
        let mut seen: Vec<Card> = Vec::new();
        let mut pot: i64 = 100;
        // Cash out somewhere between 2x and 5x, so runs end at different
        // places rather than all looking the same.
        let target = 200 + 100 * ctx.rng.below(4) as i64;
        let busted = loop {
            let rank = current.poker_rank();
            // Call whichever side has more ranks behind it.
            let higher = (14 - rank as i64) >= (rank as i64 - 2);
            let Some(mult) = call_mult(rank, higher) else { break false };
            let theme = ctx.theme();
            let note = theme.dim(&format!("{punter} calls {}", if higher { "higher" } else { "lower" }));
            draw_table(ctx, current, &seen, pot, stake, &note, None);
            table::idle_footer(ctx, "a demo run — nothing of yours is riding on it");
            ctx.screen.present();
            if table::idle_hold(1_100) {
                return;
            }

            let next = shoe.draw(ctx.rng);
            seen.push(current);
            let correct = if higher { next.poker_rank() > rank } else { next.poker_rank() < rank };
            current = next;
            if !correct {
                break true;
            }
            pot = pot * mult / 100;
            if pot >= target {
                break false;
            }
        };

        let theme = ctx.theme();
        let note = if busted {
            theme.lose(&format!("{} — {punter}'s run ends", current.label()))
        } else {
            theme.win(&format!("{punter} takes {} at {}", stake * pot / 100, fmt_mult(pot)))
        };
        draw_table(ctx, current, &seen, pot, stake, &note, None);
        table::idle_footer(ctx, "a demo run — nothing of yours is riding on it");
        ctx.screen.present();
        if table::idle_hold(2_200) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_beats_an_ace_and_nothing_loses_to_a_two() {
        assert_eq!(call_mult(14, true), None, "no rank is higher than an ace");
        assert_eq!(call_mult(2, false), None, "no rank is lower than a two");
        // The other call is always available on those cards.
        assert!(call_mult(14, false).is_some());
        assert!(call_mult(2, true).is_some());
    }

    #[test]
    fn a_longer_shot_pays_more() {
        // Calling higher gets dearer as the card on the table climbs.
        let mut last = 0;
        for rank in 2..14u8 {
            let m = call_mult(rank, true).expect("higher is possible below an ace");
            assert!(m > last, "higher on {rank} pays {m}, not more than {last}");
            last = m;
        }
    }

    #[test]
    fn the_two_calls_mirror_each_other() {
        // Higher on a 3 and lower on a king are the same shot, so they
        // must be the same price.
        assert_eq!(call_mult(3, true), call_mult(13, false));
        assert_eq!(call_mult(5, true), call_mult(11, false));
    }

    #[test]
    fn every_price_sits_below_true_odds() {
        // True odds of a call with n ways out of 13 is 13/n; the table must
        // always pay strictly less than that.
        for rank in 2..=14u8 {
            for higher in [true, false] {
                let Some(m) = call_mult(rank, higher) else { continue };
                let ways = if higher { 14 - rank as i64 } else { rank as i64 - 2 };
                let fair = 100 * RANKS / ways;
                assert!(m < fair, "rank {rank} higher={higher} pays {m} against fair {fair}");
            }
        }
    }

    #[test]
    fn the_shortest_call_still_pays_a_profit() {
        // Higher on a two is the safest call on the table; it must still
        // return more than the stake or nobody would ever make it.
        let m = call_mult(2, true).unwrap();
        assert!(m > 100, "the safest call pays {m}, which is not a profit");
    }

    #[test]
    fn a_run_of_calls_compounds() {
        // Two 1.05x calls are worth more than one, and the arithmetic is
        // the same integer maths the game runs on.
        let mut pot = 100i64;
        let m = call_mult(2, true).unwrap();
        pot = pot * m / 100;
        let after_one = pot;
        pot = pot * m / 100;
        assert!(pot > after_one && after_one > 100);
    }

    #[test]
    fn multipliers_read_to_two_places() {
        assert_eq!(fmt_mult(100), "1.00x");
        assert_eq!(fmt_mult(215), "2.15x");
        assert_eq!(fmt_mult(1_261), "12.61x");
    }
}
