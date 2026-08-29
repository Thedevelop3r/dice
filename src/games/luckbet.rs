//! Luck Bet — eight dice named A..H sit on the table. Up to three players back
//! one letter each, guessing the face it will land on (6 by default). Hits are
//! paid by the house; misses feed a carried-over jackpot.
//!
//! `turbo` is the same game with no betting input: press a key and the
//! table spins at 20 frames a second for three seconds before it settles.

use super::{Ctx, Difficulty};
use crate::dice;
use crate::economy::{Item, Wallet};
use crate::ui::{self, dice_art, widgets};

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
        let d = if turbo { Difficulty::Normal } else { super::pick_difficulty(ctx, &format!("CPU-{} DIFFICULTY", i + 1)) };
        seats.push(Seat::bot(&format!("CPU-{}", i + 1), d));
    }

    let jackpot = ctx.store.get_i64("luckbet.jackpot", 0);
    let mut table = Table::new(jackpot);
    let mut last_bet: Option<Bet> = None;
    let title = if turbo { "LUCK BET — TURBO" } else { "LUCK BET" };

    loop {
        table.round += 1;
        {
            let mut w = Wallet::new(ctx.store);
            if w.chips() <= 0 {
                if w.dollars() > 0 {
                    info(ctx, title, "out of chips — visit the store to exchange dollars.");
                    break;
                }
                if w.ensure_solvent(50) {
                    info(ctx, title, "the house stakes you 50 chips.");
                }
            }
        }

        let human_bet = if turbo {
            draw_spin_prompt(ctx, title, &seats, &table);
            if !ui::confirm_key(true) {
                break;
            }
            let b = auto_bet(ctx, last_bet);
            Some(b)
        } else {
            match ask_bet(ctx, title, &seats, &table, last_bet) {
                Some(b) => Some(b),
                None => break,
            }
        };
        let Some(hb) = human_bet else { break };
        {
            let mut w = Wallet::new(ctx.store);
            if !w.spend_chips(hb.stake) {
                info(ctx, title, "not enough chips for that stake.");
                continue;
            }
        }
        seats[0].bet = Some(hb);
        last_bet = Some(hb);

        for seat in seats.iter_mut().skip(1) {
            let b = ai_bet(ctx, seat);
            seat.chips -= b.stake;
            seat.bet = Some(b);
        }

        spin(ctx, title, &seats, &mut table, turbo);

        if !turbo {
            offer_second_chance(ctx, title, &mut table, &seats[0]);
        }

        settle(ctx, title, &mut table, &mut seats, turbo);
        ctx.store.set_i64("luckbet.jackpot", table.jackpot);
        ctx.store.bump("luckbet.rounds", 1);
        let _ = ctx.store.save();

        for s in seats.iter_mut() {
            if s.ai.is_some() && s.chips <= 0 {
                s.chips = AI_START / 2;
            }
        }

        if !turbo && !ui::confirm_key(true) {
            break;
        }
    }

    let theme = ctx.theme();
    let (chips, dollars) = {
        let w = Wallet::new(ctx.store);
        (w.chips(), w.dollars())
    };
    let msg = format!("leaving the table with {}", ui::money(&theme, chips, dollars));
    info(ctx, title, &msg);
}

fn table_header(ctx: &mut Ctx, title: &str, seats: &[Seat], table: &Table) {
    let theme = ctx.theme();
    ui::header(ctx.screen, title);
    ctx.screen.blank();
    let (chips, dollars, vip) = {
        let w = Wallet::new(ctx.store);
        (w.chips(), w.dollars(), w.owns(Item::VipTable))
    };
    ctx.screen.line(&format!(
        "  round {}   you: {}   jackpot: {}",
        table.round.max(1),
        ui::money(&theme, chips, dollars),
        theme.paint(ui::theme::MAGENTA, &format!("{} chips", table.jackpot))
    ));
    ctx.screen.line(&theme.dim(&format!("hits pay {}:1 · land {SWEEP_COPIES}+ copies of your number and sweep the jackpot", payout_multiplier(vip))));
    for s in seats.iter().filter(|s| s.ai.is_some()) {
        ctx.screen.line(&theme.dim(&format!("  {} ({}) — {} chips", s.name, s.ai.unwrap().label(), s.chips)));
    }
}

fn draw_spin_prompt(ctx: &mut Ctx, title: &str, seats: &[Seat], table: &Table) {
    let theme = ctx.theme();
    ctx.screen.begin();
    table_header(ctx, title, seats, table);
    ctx.screen.blank();
    let gold = Wallet::new(ctx.store).owns(Item::GoldDice);
    let no_hits = [false; DICE];
    for line in dice_art::table_block(&theme, &table.values, &LETTERS, &no_hits, gold, DICE) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line(&widgets::footer(&theme, &[('y', "spin"), ('n', "leave the table")]));
    ctx.screen.present();
}

fn info(ctx: &mut Ctx, title: &str, msg: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, title);
    ctx.screen.blank();
    ctx.screen.line(&format!("  {}", theme.dim(msg)));
    ctx.screen.present();
    ui::pause(ctx.screen);
}

/// Reads the human's bet. `None` means they want to leave.
fn ask_bet(ctx: &mut Ctx, title: &str, seats: &[Seat], table: &Table, last: Option<Bet>) -> Option<Bet> {
    let theme = ctx.theme();
    let gold = Wallet::new(ctx.store).owns(Item::GoldDice);

    ctx.screen.begin();
    table_header(ctx, title, seats, table);
    ctx.screen.blank();
    let no_hits = [false; DICE];
    for line in dice_art::table_block(&theme, &table.values, &LETTERS, &no_hits, gold, DICE) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    if let Some(b) = last {
        ctx.screen.line(&theme.dim(&format!("last bet: {} on {} for {}", LETTERS[b.letter], b.number, b.stake)));
    }
    ctx.screen.line("  back which die?");
    ctx.screen.line(&widgets::footer(&theme, &[('a', "…"), ('h', "die A-H"), ('r', "repeat last"), ('q', "leave")]));
    ctx.screen.present();

    let mut valid: Vec<char> = LETTERS.iter().map(|c| c.to_ascii_lowercase()).collect();
    valid.push('r');
    valid.push('q');
    let letter = match ui::choose_key(&valid, 'q') {
        Some('q') | None => return None,
        Some('r') => {
            if let Some(b) = last {
                let chips = Wallet::new(ctx.store).chips();
                let stake = b.stake.min(chips.max(1));
                return Some(Bet { stake, ..b });
            }
            LETTERS.iter().position(|c| *c == 'A').unwrap()
        }
        Some(c) => LETTERS.iter().position(|l| l.to_ascii_lowercase() == c).unwrap_or(0),
    };

    ctx.screen.begin();
    table_header(ctx, title, seats, table);
    ctx.screen.blank();
    ctx.screen.line(&format!("  backing {} — which face?", LETTERS[letter]));
    ctx.screen.line(&widgets::footer(&theme, &[('1', "…"), ('6', "face")]));
    ctx.screen.present();
    let number = ui::choose_key(&['1', '2', '3', '4', '5', '6'], '6').and_then(|c| c.to_digit(10)).unwrap_or(6);

    let chips = Wallet::new(ctx.store).chips();
    let max = chips.max(1);
    let default = 10.min(max);
    let stake = widgets::number_picker(ctx.screen, 1, max, default, 5, &[('m', max)], |s, v| {
        let theme = s.theme;
        s.begin();
        ui::header(s, title);
        s.blank();
        s.line(&format!("  {} on {} — stake?", LETTERS[letter], number));
        s.blank();
        s.line(&format!("  {}", theme.paint(ui::theme::GOLD, &format!("{v} chips"))));
        s.blank();
        s.line(&widgets::footer(&theme, &[('↑', "+5"), ('↓', "-5"), ('m', "max"), ('\u{23ce}', "confirm")]));
    })?;
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
        Difficulty::Easy => ctx.rng.roll(6),
        _ => 6,
    };
    let stack = seat.chips.max(1);
    let stake = match d {
        Difficulty::Easy => (stack / 5).max(1),
        Difficulty::Normal => (stack / 10).max(1),
        Difficulty::Hard => (stack / 12).clamp(1, 25),
    };
    Bet { letter, number, stake: stake.min(stack) }
}

/// Rolls the table: a one-second settle normally, a sustained 20fps/3s
/// spin in Turbo.
fn spin(ctx: &mut Ctx, title: &str, seats: &[Seat], table: &mut Table, turbo: bool) {
    let gold = Wallet::new(ctx.store).owns(Item::GoldDice);
    let delays: &[u64] = if turbo { &dice_art::TURBO_FRAME_DELAYS } else { dice_art::standard_frames() };
    let no_hits = [false; DICE];
    let final_values = dice_art::animate_table_roll(ctx.screen, ctx.rng, DICE, delays, |screen, frame| {
        let theme = screen.theme;
        screen.begin();
        ui::header(screen, title);
        screen.blank();
        screen.line(&format!("  round {}", table.round));
        for s in seats.iter().filter(|s| s.ai.is_some()) {
            screen.line(&theme.dim(&format!("  {} ({}) — {} chips", s.name, s.ai.unwrap().label(), s.chips)));
        }
        screen.blank();
        for line in dice_art::table_block(&theme, frame, &LETTERS, &no_hits, gold, DICE) {
            screen.line(&line);
        }
        screen.blank();
        screen.line(&theme.dim("spinning..."));
    });
    table.values = final_values.try_into().expect("animate_table_roll returns exactly DICE values");
    ctx.store.bump("luckbet.rolls", DICE as i64);
}

/// Uses a Reroll Token or Lucky Charm when the human's die missed.
fn offer_second_chance(ctx: &mut Ctx, title: &str, table: &mut Table, seat: &Seat) {
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

    let theme = ctx.theme();
    let gold = Wallet::new(ctx.store).owns(Item::GoldDice);
    ctx.screen.begin();
    ui::header(ctx.screen, title);
    ctx.screen.blank();
    let no_hits = [false; DICE];
    for line in dice_art::table_block(&theme, &table.values, &LETTERS, &no_hits, gold, DICE) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    ctx.screen.line(&theme.lose(&format!("{} showed {} — missed.", LETTERS[bet.letter], table.values[bet.letter])));
    let mut hints = Vec::new();
    if has_charm {
        hints.push(('c', "charm rerolls your die"));
    }
    if has_reroll {
        hints.push(('r', "reroll spins the table"));
    }
    hints.push(('n', "no thanks"));
    ctx.screen.line(&widgets::footer(&theme, &hints));
    ctx.screen.present();

    let valid: Vec<char> = hints.iter().map(|(k, _)| *k).collect();
    match ui::choose_key(&valid, 'n') {
        Some('c') if has_charm
            && Wallet::new(ctx.store).consume(Item::LuckyCharm) => {
                table.values[bet.letter] = ctx.rng.roll(6);
            }
        Some('r') if has_reroll
            && Wallet::new(ctx.store).consume(Item::Reroll) => {
                let roll = dice::roll_n(ctx.rng, DICE as u32, 6);
                table.values.copy_from_slice(&roll);
            }
        _ => {}
    }
}

/// Was the call right, and did the number land often enough to sweep the pot?
pub fn evaluate(bet: &Bet, values: &[u32], counts: &[u32]) -> (bool, bool) {
    let hit = values.get(bet.letter).copied() == Some(bet.number);
    let sweep = hit && counts.get(bet.number as usize).copied().unwrap_or(0) >= SWEEP_COPIES;
    (hit, sweep)
}

/// Pays winners, rakes losses into the jackpot, and shows the round summary.
fn settle(ctx: &mut Ctx, title: &str, table: &mut Table, seats: &mut [Seat], turbo: bool) {
    let counts = dice::tally(&table.values, 6);
    let vip = Wallet::new(ctx.store).owns(Item::VipTable);
    let gold = Wallet::new(ctx.store).owns(Item::GoldDice);
    let mult = payout_multiplier(vip);
    let mut hits = [false; DICE];
    let mut sweep_winner: Option<usize> = None;

    for (i, seat) in seats.iter_mut().enumerate() {
        let Some(bet) = seat.bet else { continue };
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
            if seat.ai.is_none() && Wallet::new(ctx.store).count(Item::Insurance) > 0 && Wallet::new(ctx.store).consume(Item::Insurance) {
                let refund = bet.stake / 2;
                delta += refund;
                note.push_str(&format!(" (insurance refunded {refund})"));
            }
        }
        seat.last_delta = delta;
        seat.note = note;
    }

    if let Some(w) = sweep_winner {
        let pot = table.jackpot;
        seats[w].last_delta += pot;
        table.jackpot = 0;
        ctx.store.bump("luckbet.sweeps", 1);
        ctx.store.record_best("luckbet.best_jackpot", pot);
    }

    for (i, s) in seats.iter_mut().enumerate() {
        let credit = s.bet.map_or(0, |b| (b.stake + s.last_delta).max(0));
        if i == 0 {
            let mut w = Wallet::new(ctx.store);
            w.add_chips(credit);
        } else {
            s.chips += credit;
        }
    }

    if seats[0].last_delta > 0 {
        ctx.store.bump("luckbet.wins", 1);
        ctx.store.record_best("luckbet.best_win", seats[0].last_delta);
    } else {
        ctx.store.bump("luckbet.losses", 1);
    }

    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, title);
    ctx.screen.blank();
    for line in dice_art::table_block(&theme, &table.values, &LETTERS, &hits, gold, DICE) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    for s in seats.iter() {
        let Some(bet) = s.bet else { continue };
        let sign = if s.last_delta >= 0 { "+" } else { "" };
        ctx.screen.line(&format!(
            "  {:<10} {} on {}  →  {}  {}",
            s.name,
            theme.bold(&LETTERS[bet.letter].to_string()),
            bet.number,
            theme.paint(if s.last_delta >= 0 { ui::theme::GREEN } else { ui::theme::RED }, &format!("{sign}{} chips", s.last_delta)),
            theme.dim(&s.note)
        ));
    }
    if let Some(w) = sweep_winner {
        for line in widgets::banner(&theme, &format!("{} SWEEPS THE JACKPOT", seats[w].name), true) {
            ctx.screen.line(&format!("  {line}"));
        }
    }
    ctx.screen.blank();
    if turbo {
        ctx.screen.line(&theme.dim("··· press any key to continue ···"));
        ctx.screen.present();
        ui::wait_any_key();
    } else {
        ctx.screen.line(&widgets::footer(&theme, &[('y', "another round"), ('n', "leave the table")]));
        ctx.screen.present();
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
        assert_eq!(evaluate(&Bet { letter: 0, number: 6, stake: 10 }, &values, &counts), (true, true));
        assert_eq!(evaluate(&Bet { letter: 1, number: 6, stake: 10 }, &values, &counts), (false, false));
        assert_eq!(evaluate(&Bet { letter: 3, number: 3, stake: 10 }, &values, &counts), (true, false));
    }

    #[test]
    fn vip_improves_the_payout() {
        assert_eq!(payout_multiplier(false), 5);
        assert_eq!(payout_multiplier(true), 6);
    }
}
