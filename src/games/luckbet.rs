//! Luck Bet — eight dice named A..H sit on the table. Up to three players back
//! one letter each, guessing the face it will land on (6 by default). Hits are
//! paid by the house; misses feed a carried-over jackpot.
//!
//! `play_turbo` is the same game with no betting input: press Enter and the
//! table spins at 20 frames a second for three seconds before it settles.

use super::{Ctx, Difficulty};
use crate::dice;
use crate::economy::{Item, Wallet};
use crate::ui::{self, *};

pub const DICE: usize = 8;
pub const LETTERS: [char; DICE] = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H'];
const AI_START: i64 = 150;
const RAKE_PERCENT: i64 = 20;
const SWEEP_COPIES: u32 = 3;

#[derive(Debug, Clone, Copy)]
pub struct Bet {
    pub letter: usize,
    pub number: u32,
    pub stake: i64,
}

pub struct Seat {
    pub name: String,
    pub ai: Option<Difficulty>,
    /// AI stacks live here; the human's balance is the shared wallet.
    pub chips: i64,
    pub bet: Option<Bet>,
    pub last_delta: i64,
    pub note: String,
}

impl Seat {
    fn human(name: &str) -> Seat {
        Seat { name: name.to_string(), ai: None, chips: 0, bet: None, last_delta: 0, note: String::new() }
    }

    fn bot(name: &str, d: Difficulty) -> Seat {
        Seat { name: name.to_string(), ai: Some(d), chips: AI_START, bet: None, last_delta: 0, note: String::new() }
    }
}

pub struct Table {
    pub values: [u32; DICE],
    pub jackpot: i64,
    pub round: u32,
}

impl Table {
    fn new(jackpot: i64) -> Table {
        Table { values: [1; DICE], jackpot, round: 0 }
    }
}

/// Chips paid per chip staked on a correct call.
fn payout_multiplier(vip: bool) -> i64 {
    if vip { 6 } else { 5 }
}

pub fn play(ctx: &mut Ctx, opponents: usize, turbo: bool) {
    let name = ctx.store.get_str("player.name", "Player");
    let mut seats = vec![Seat::human(&name)];
    for i in 0..opponents.min(2) {
        let d = pick_ai_difficulty(i + 1, turbo);
        seats.push(Seat::bot(&format!("CPU-{}", i + 1), d));
    }

    let jackpot = ctx.store.get_i64("luckbet.jackpot", 0);
    let mut table = Table::new(jackpot);
    let mut last_bet: Option<Bet> = None;

    ui::header(ctx.colors, if turbo { "LUCK BET — TURBO" } else { "LUCK BET" });
    println!(
        "  Eight dice, {}. Back one letter and the face you think it lands on.",
        color(ctx.colors, BOLD, "A through H")
    );
    {
        let w = Wallet::new(ctx.store);
        println!(
            "  A hit pays {}:1. Misses feed the jackpot — land {} copies of your number and you take it all.",
            payout_multiplier(w.owns(Item::VipTable)),
            SWEEP_COPIES
        );
    }
    if turbo {
        println!("  {}", color(ctx.colors, DIM, "Turbo: press Enter to spin. Bets repeat automatically."));
    }

    loop {
        table.round += 1;
        let (chips, dollars, vip, gold) = {
            let w = Wallet::new(ctx.store);
            (w.chips(), w.dollars(), w.owns(Item::VipTable), w.owns(Item::GoldDice))
        };
        println!();
        ui::rule(ctx.colors, 60);
        println!(
            "  round {}   you: {}   jackpot: {}",
            table.round,
            ui::money(ctx.colors, chips, dollars),
            color(ctx.colors, MAGENTA, &format!("{} chips", table.jackpot))
        );
        for s in seats.iter().filter(|s| s.ai.is_some()) {
            println!(
                "  {}",
                color(ctx.colors, DIM, &format!("{} ({}) — {} chips", s.name, s.ai.unwrap().label(), s.chips))
            );
        }

        {
            let mut w = Wallet::new(ctx.store);
            if w.chips() <= 0 {
                if w.dollars() > 0 {
                    println!("  {}", color(ctx.colors, YELLOW, "out of chips — visit the store to exchange dollars."));
                    break;
                }
                if w.ensure_solvent(50) {
                    println!("  {}", color(ctx.colors, DIM, "the house stakes you 50 chips."));
                }
            }
        }

        // --- place bets ---
        let human_bet = if turbo {
            let ans = ui::prompt("\n  press Enter to spin (or q to leave the table): ");
            if ans.to_lowercase().starts_with('q') {
                break;
            }
            let b = auto_bet(ctx, last_bet);
            println!(
                "  auto-bet: {} chips on {} showing {}",
                b.stake,
                color(ctx.colors, BOLD, &LETTERS[b.letter].to_string()),
                b.number
            );
            Some(b)
        } else {
            match ask_bet(ctx, last_bet) {
                Some(b) => Some(b),
                None => break,
            }
        };
        let Some(hb) = human_bet else { break };
        {
            let mut w = Wallet::new(ctx.store);
            if !w.spend_chips(hb.stake) {
                println!("  {}", color(ctx.colors, RED, "not enough chips for that stake."));
                continue;
            }
        }
        seats[0].bet = Some(hb);
        last_bet = Some(hb);

        for i in 1..seats.len() {
            let b = ai_bet(ctx, &seats[i]);
            seats[i].chips -= b.stake;
            seats[i].bet = Some(b);
            println!(
                "  {} backs {} on {} for {} chips",
                color(ctx.colors, MAGENTA, &seats[i].name),
                color(ctx.colors, BOLD, &LETTERS[b.letter].to_string()),
                b.number,
                b.stake
            );
        }

        // --- spin ---
        spin(ctx, &mut table, turbo, gold);

        // --- second chances (consumables) ---
        if !turbo {
            offer_second_chance(ctx, &mut table, &seats[0], gold);
        }

        // --- settle ---
        settle(ctx, &mut table, &mut seats, vip, gold);
        ctx.store.set_i64("luckbet.jackpot", table.jackpot);
        ctx.store.bump("luckbet.rounds", 1);
        let _ = ctx.store.save();

        for s in seats.iter_mut() {
            if s.ai.is_some() && s.chips <= 0 {
                s.chips = AI_START / 2;
                println!("  {}", color(ctx.colors, DIM, &format!("{} rebuys.", s.name)));
            }
        }

        if turbo {
            ui::pause();
        } else if !ui::confirm("\n  another round?") {
            break;
        }
    }

    println!();
    let w = Wallet::new(ctx.store);
    println!("  leaving the table with {}", ui::money(ctx.colors, w.chips(), w.dollars()));
    ui::pause();
}

fn pick_ai_difficulty(n: usize, turbo: bool) -> Difficulty {
    if turbo {
        return Difficulty::Normal;
    }
    println!("   1. Easy   2. Normal   3. Hard");
    Difficulty::from_index(ui::prompt_usize(&format!("  CPU-{n} difficulty"), 1, 3, 2))
}

/// Reads the human's bet. `None` means they want to leave.
fn ask_bet(ctx: &mut Ctx, last: Option<Bet>) -> Option<Bet> {
    let chips = Wallet::new(ctx.store).chips();
    println!();
    let letter = loop {
        let hint = last.map(|b| LETTERS[b.letter]).unwrap_or('A');
        let ans = ui::prompt(&format!("  back which die? A-H [{hint}] (r = repeat last, q = leave): "));
        if ui::eof_reached() {
            return None;
        }
        let low = ans.to_lowercase();
        match low.chars().next() {
            None => break LETTERS.iter().position(|c| *c == hint).unwrap_or(0),
            Some('q') => return None,
            Some('r') => {
                if let Some(b) = last {
                    println!("  repeating: {} on {} for {}", LETTERS[b.letter], b.number, b.stake);
                    let stake = b.stake.min(chips.max(1));
                    return Some(Bet { stake, ..b });
                }
                println!("  ! no previous bet yet");
            }
            Some(c) if c.is_ascii_alphabetic() => {
                let up = c.to_ascii_uppercase();
                match LETTERS.iter().position(|l| *l == up) {
                    Some(i) => break i,
                    None => println!("  ! pick a letter A through H"),
                }
            }
            _ => println!("  ! pick a letter A through H"),
        }
    };

    let number = ui::prompt_usize("  which face should it show? 1-6", 1, 6, 6) as u32;
    let max = chips.max(1);
    let default = 10.min(max);
    let stake = ui::prompt_usize(&format!("  stake (1-{max})"), 1, max as usize, default as usize) as i64;
    Some(Bet { letter, number, stake })
}

/// Turbo mode repeats the last bet, or opens with a sensible default.
fn auto_bet(ctx: &mut Ctx, last: Option<Bet>) -> Bet {
    let chips = Wallet::new(ctx.store).chips();
    let mut b = last.unwrap_or(Bet { letter: ctx.rng.below(DICE), number: 6, stake: 10 });
    b.stake = b.stake.clamp(1, chips.max(1));
    b
}

fn ai_bet(ctx: &mut Ctx, seat: &Seat) -> Bet {
    let d = seat.ai.unwrap_or(Difficulty::Normal);
    let letter = ctx.rng.below(DICE);
    let number = match d {
        // Easy scatters its guesses; the others chase the jackpot number.
        Difficulty::Easy => ctx.rng.roll(6),
        _ => 6,
    };
    let stack = seat.chips.max(1);
    let stake = match d {
        Difficulty::Easy => (stack / 5).max(1),
        Difficulty::Normal => (stack / 10).max(1),
        Difficulty::Hard => (stack / 12).max(1).min(25),
    };
    Bet { letter, number, stake: stake.min(stack) }
}

/// Rolls the table. Turbo animates 20 frames a second for three seconds.
fn spin(ctx: &mut Ctx, table: &mut Table, turbo: bool, gold: bool) {
    let no_hits = [false; DICE];
    println!();
    if turbo {
        let frames = 60; // 20 fps * 3 s
        ui::hide_cursor();
        let mut lines = 0;
        for f in 0..frames {
            for v in table.values.iter_mut() {
                *v = ctx.rng.roll(6);
            }
            if f > 0 {
                ui::cursor_up(lines);
            }
            lines = ui::draw_table(&table.values, &LETTERS, &no_hits, ctx.colors, gold);
            ui::sleep_ms(50);
        }
        ui::cursor_up(lines);
        ui::show_cursor();
    } else {
        for _ in 0..6 {
            for v in table.values.iter_mut() {
                *v = ctx.rng.roll(6);
            }
        }
    }
    let final_roll = dice::roll_n(ctx.rng, DICE as u32, 6);
    table.values.copy_from_slice(&final_roll);
    ctx.store.bump("luckbet.rolls", DICE as i64);
    ui::draw_table(&table.values, &LETTERS, &no_hits, ctx.colors, gold);
}

/// Uses a Reroll Token or Lucky Charm when the human's die missed.
fn offer_second_chance(ctx: &mut Ctx, table: &mut Table, seat: &Seat, gold: bool) {
    let Some(bet) = seat.bet else { return };
    if table.values[bet.letter] == bet.number {
        return;
    }
    let (has_reroll, has_charm) = {
        let w = Wallet::new(ctx.store);
        (w.count(Item::Reroll) > 0, w.count(Item::LuckyCharm) > 0)
    };
    if !has_reroll && !has_charm {
        return;
    }
    println!(
        "  {} — missed. {}{}",
        color(ctx.colors, RED, &format!("{} showed {}", LETTERS[bet.letter], table.values[bet.letter])),
        if has_charm { "(c)harm rerolls your die  " } else { "" },
        if has_reroll { "(r)eroll spins the table  " } else { "" }
    );
    let ans = ui::prompt("  use an item? (c/r/n): ").to_lowercase();
    match ans.chars().next() {
        Some('c') if has_charm => {
            if Wallet::new(ctx.store).consume(Item::LuckyCharm) {
                table.values[bet.letter] = ctx.rng.roll(6);
                println!("  {}", color(ctx.colors, CYAN, "the charm turns your die..."));
            }
        }
        Some('r') if has_reroll => {
            if Wallet::new(ctx.store).consume(Item::Reroll) {
                let roll = dice::roll_n(ctx.rng, DICE as u32, 6);
                table.values.copy_from_slice(&roll);
                println!("  {}", color(ctx.colors, CYAN, "the table spins again..."));
            }
        }
        _ => return,
    }
    let no_hits = [false; DICE];
    ui::draw_table(&table.values, &LETTERS, &no_hits, ctx.colors, gold);
}

/// Was the call right, and did the number land often enough to sweep the pot?
pub fn evaluate(bet: &Bet, values: &[u32], counts: &[u32]) -> (bool, bool) {
    let hit = values.get(bet.letter).copied() == Some(bet.number);
    let sweep = hit && counts.get(bet.number as usize).copied().unwrap_or(0) >= SWEEP_COPIES;
    (hit, sweep)
}

/// Pays winners, rakes losses into the jackpot, and prints the round summary.
fn settle(ctx: &mut Ctx, table: &mut Table, seats: &mut [Seat], vip: bool, gold: bool) {
    let counts = dice::tally(&table.values, 6);
    let mult = payout_multiplier(vip);
    let mut hits = [false; DICE];
    let mut sweep_winner: Option<usize> = None;

    for i in 0..seats.len() {
        let Some(bet) = seats[i].bet else { continue };
        let (won, can_sweep) = evaluate(&bet, &table.values, &counts);
        if won {
            hits[bet.letter] = true;
        }
        let mut delta;
        let mut note = String::new();
        if won {
            delta = bet.stake * mult;
            note.push_str("HIT");
            if can_sweep && sweep_winner.is_none() {
                sweep_winner = Some(i);
                note.push_str(&format!(" + SWEEP ({} copies)", counts[bet.number as usize]));
            }
        } else {
            delta = -bet.stake;
            let rake = (bet.stake * RAKE_PERCENT / 100).max(1);
            table.jackpot += rake;
            note.push_str("miss");
            if seats[i].ai.is_none() && Wallet::new(ctx.store).count(Item::Insurance) > 0 {
                if Wallet::new(ctx.store).consume(Item::Insurance) {
                    let refund = bet.stake / 2;
                    delta += refund;
                    note.push_str(&format!(" (insurance refunded {refund})"));
                }
            }
        }
        seats[i].last_delta = delta;
        seats[i].note = note;
    }

    if let Some(w) = sweep_winner {
        let pot = table.jackpot;
        seats[w].last_delta += pot;
        table.jackpot = 0;
        ctx.store.bump("luckbet.sweeps", 1);
        ctx.store.record_best("luckbet.best_jackpot", pot);
    }

    // Apply balances: seat 0 is the human wallet, the rest are local stacks.
    for (i, s) in seats.iter_mut().enumerate() {
        // The stake left the balance when the bet was placed, so hand back the
        // stake plus the net result (zero on a plain loss).
        let credit = s.bet.map_or(0, |b| (b.stake + s.last_delta).max(0));
        if i == 0 {
            let mut w = Wallet::new(ctx.store);
            w.add_chips(credit);
        } else {
            s.chips += credit;
        }
    }

    // Redraw the settled table with the winning dice picked out.
    if hits.iter().any(|h| *h) {
        ui::cursor_up(6);
        ui::draw_table(&table.values, &LETTERS, &hits, ctx.colors, gold);
    }

    println!();
    for s in seats.iter() {
        let Some(bet) = s.bet else { continue };
        let sign = if s.last_delta >= 0 { "+" } else { "" };
        println!(
            "  {:<10} {} on {}  →  {}  {}",
            s.name,
            color(ctx.colors, BOLD, &LETTERS[bet.letter].to_string()),
            bet.number,
            color(
                ctx.colors,
                if s.last_delta >= 0 { GREEN } else { RED },
                &format!("{sign}{} chips", s.last_delta)
            ),
            color(ctx.colors, DIM, &s.note)
        );
    }
    if let Some(w) = sweep_winner {
        println!("  {}", color(ctx.colors, MAGENTA, &format!("★ {} SWEEPS THE JACKPOT ★", seats[w].name)));
    }

    if seats[0].last_delta > 0 {
        ctx.store.bump("luckbet.wins", 1);
        ctx.store.record_best("luckbet.best_win", seats[0].last_delta);
    } else {
        ctx.store.bump("luckbet.losses", 1);
    }
    for s in seats.iter_mut() {
        s.bet = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_cover_the_table() {
        assert_eq!(LETTERS.len(), DICE);
        assert_eq!(LETTERS[7], 'H');
    }

    #[test]
    fn hits_need_the_named_die_to_show_the_named_face() {
        let values = [6, 2, 6, 3, 1, 6, 4, 5];
        let counts = dice::tally(&values, 6);
        // A shows a 6, and three dice on the table do: hit and sweep.
        assert_eq!(evaluate(&Bet { letter: 0, number: 6, stake: 10 }, &values, &counts), (true, true));
        // B shows a 2 — right die, wrong face.
        assert_eq!(evaluate(&Bet { letter: 1, number: 6, stake: 10 }, &values, &counts), (false, false));
        // D shows the 3 that was called, but only one 3 is on the table.
        assert_eq!(evaluate(&Bet { letter: 3, number: 3, stake: 10 }, &values, &counts), (true, false));
    }

    #[test]
    fn vip_improves_the_payout() {
        assert_eq!(payout_multiplier(false), 5);
        assert_eq!(payout_multiplier(true), 6);
    }
}
