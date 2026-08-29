//! Ultra Casino Dice — an unattended, self-playing table. Twelve dice
//! (lettered A..L) and eight computer-only seats play out ten rounds with
//! no input required at all: each round the table bids, rolls, pays out,
//! and asks every seat whether it's staying, entirely on its own clock.
//! Built to be glanced at like an idle screen rather than played — the
//! only key that does anything is `Q`, which lets the current round
//! finish and then returns to the floor instead of cutting it off mid-air.
//!
//! The win condition matches Luck Bet: each seat backs one die (a letter)
//! and calls a face (1-6), and only wins if that exact die lands on that
//! face. Unlike Luck Bet, there's no house — every seat's stake goes into
//! one pot for the round, and whoever called it right splits that pot
//! (proportional to their own stake); a miss just loses the stake. Every
//! round's result, and every session's final standings, are appended to a
//! plain-text log (`history.rs`) viewable later from the dashboard.

use super::Ctx;
use crate::history;
use crate::rng::Rng;
use crate::ui::{self, dice_art, theme, widgets};

pub const DICE: usize = 12;
pub const SEATS: usize = 8;
pub const ROUNDS: u32 = 10;
pub const LETTERS: [char; DICE] = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L'];

const BID_MS: u64 = 5_000;
// The roll itself takes its 7s/140-frames-at-20fps timing from
// dice_art::ULTRA_FRAME_DELAYS directly — there's no separate constant to
// keep in sync with it here.
const DECIDE_MS: u64 = 4_000;
const SETTLE_HOLD_MS: u64 = 15_000;
const LOOP_PAUSE_MS: u64 = 10_000;

const START_CHIPS_MIN: i64 = 200;
const START_CHIPS_STEPS: usize = 21; // 200, 250, ... 1200

const FIRST_NAMES: &[&str] = &[
    "Lucky", "Big", "Slick", "Wild", "Mad", "Silver", "Diamond", "Fast", "Cool", "Iron", "Golden", "Sharp", "Jazzy", "Reckless", "Smooth",
    "Velvet", "Midnight", "Crimson", "Silent", "Rowdy",
];
const LAST_NAMES: &[&str] = &[
    "Vega", "Suarez", "Nakamura", "Castellano", "Petrov", "Steele", "Fontaine", "Marlowe", "Sinclair", "Duval", "Okafor", "Kowalski",
    "Reyes", "Ashford", "Blackwood", "Devereux", "Hartley", "Monroe", "Delgado", "Whitfield",
];

#[derive(Clone, Copy)]
struct Bet {
    letter: usize,
    face: u32,
    stake: i64,
}

struct AiPlayer {
    name: String,
    chips: i64,
    start_chips: i64,
    rounds: u32,
    wins: u32,
}

struct Seat {
    player: AiPlayer,
    bet: Option<Bet>,
    last_delta: i64,
    note: String,
    leaving: bool,
}

/// A seat that cashed out (or busted out) mid-session — kept separately
/// from `Seat` since the seat itself gets a brand-new occupant afterward.
struct Departure {
    name: String,
    rounds: u32,
    wins: u32,
    net: i64,
    round_left: u32,
}

pub fn play(ctx: &mut Ctx) {
    loop {
        if play_session(ctx) || ui::quit_requested() {
            return;
        }
    }
}

/// Plays one full ten-round session. Returns `true` if the player asked to
/// leave (`Q` or Ctrl+C) — the caller should return to the floor instead
/// of looping into another session.
fn play_session(ctx: &mut Ctx) -> bool {
    let mut seats: Vec<Seat> = Vec::with_capacity(SEATS);
    for _ in 0..SEATS {
        let existing = seat_names(&seats);
        seats.push(new_seat(ctx.rng, 0, &existing));
    }
    let mut departures: Vec<Departure> = Vec::new();
    let mut quit = false;

    history::append(&format!("=== session start {} ===", history::stamp()));
    ctx.store.bump("ultra.sessions", 1);

    for round in 1..=ROUNDS {
        replace_busted(ctx.rng, &mut seats, &mut departures, round);

        bidding_phase(ctx, &mut seats, round, &mut quit);
        let values = roll_phase(ctx, &seats, round, &mut quit);
        settle_round(ctx, &mut seats, &values, round);
        hold(SETTLE_HOLD_MS, &mut quit);

        if round < ROUNDS && !quit {
            decide_phase(ctx, &mut seats, round, &mut departures, &mut quit);
        }
        if quit {
            break;
        }
    }

    log_session_summary(&seats, &departures);
    history::append(&format!("=== session end {} ===", history::stamp()));
    let _ = ctx.store.save();

    if quit {
        draw_session_summary(ctx, &seats, &departures, true, None);
        ui::pause(ctx.screen);
        true
    } else {
        idle_countdown(ctx, &seats, &departures, LOOP_PAUSE_MS)
    }
}

// ---------------------------------------------------------------- setup --

fn seat_names(seats: &[Seat]) -> Vec<String> {
    seats.iter().map(|s| s.player.name.clone()).collect()
}

fn random_name(rng: &mut Rng, existing: &[String]) -> String {
    for _ in 0..6 {
        let name = format!("{} {}", FIRST_NAMES[rng.below(FIRST_NAMES.len())], LAST_NAMES[rng.below(LAST_NAMES.len())]);
        if !existing.iter().any(|n| n == &name) {
            return name;
        }
    }
    format!("{} {}", FIRST_NAMES[rng.below(FIRST_NAMES.len())], LAST_NAMES[rng.below(LAST_NAMES.len())])
}

fn new_player(rng: &mut Rng, existing: &[String]) -> AiPlayer {
    let start = START_CHIPS_MIN + (rng.below(START_CHIPS_STEPS) as i64) * 50;
    AiPlayer { name: random_name(rng, existing), chips: start, start_chips: start, rounds: 0, wins: 0 }
}

fn new_seat(rng: &mut Rng, _round: u32, existing: &[String]) -> Seat {
    Seat { player: new_player(rng, existing), bet: None, last_delta: 0, note: String::new(), leaving: false }
}

/// Safety net: a seat that hit exactly 0 chips (an all-in miss) is
/// "busted out" and gets a fresh occupant before the next round's bidding,
/// same as a voluntary cash-out, just not one the player chose.
fn replace_busted(rng: &mut Rng, seats: &mut [Seat], departures: &mut Vec<Departure>, round: u32) {
    for i in 0..SEATS {
        if seats[i].player.chips > 0 {
            continue;
        }
        let p = &seats[i].player;
        departures.push(Departure { name: p.name.clone(), rounds: p.rounds, wins: p.wins, net: p.chips - p.start_chips, round_left: round.saturating_sub(1).max(1) });
        let existing = seat_names(seats);
        seats[i] = new_seat(rng, round, &existing);
    }
}

fn shuffled_order(rng: &mut Rng) -> Vec<usize> {
    let mut order: Vec<usize> = (0..SEATS).collect();
    for i in (1..order.len()).rev() {
        let j = rng.below(i + 1);
        order.swap(i, j);
    }
    order
}

/// Seats with a live bet, sorted by which letter they backed — keeps the
/// on-screen list grouped and stable regardless of bidding order.
fn bet_order(seats: &[Seat]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..SEATS).filter(|&i| seats[i].bet.is_some()).collect();
    order.sort_by_key(|&i| seats[i].bet.unwrap().letter);
    order
}

// --------------------------------------------------------- quiet timing --

/// Holds the current frame for `ms`, checking every beat for a leave
/// signal so an unattended screen still notices `Q`/Ctrl+C promptly
/// instead of only at the next `read_key()` — which never comes, since
/// nothing here ever blocks on one.
fn hold(ms: u64, quit: &mut bool) {
    let step = 50u64;
    let mut waited = 0u64;
    while waited < ms {
        if ui::poll_leave_signal() {
            *quit = true;
        }
        let d = step.min(ms - waited);
        ui::sleep_ms(d);
        waited += d;
    }
}

// -------------------------------------------------------------- bidding --

fn ai_bid(rng: &mut Rng, chips: i64) -> Bet {
    let letter = rng.below(DICE);
    let face = rng.roll(6);
    let stack = chips.max(1);
    let pct = 5 + rng.below(16) as i64; // 5%..20% of the stack
    let stake = ((stack * pct) / 100).max(1).min(stack);
    Bet { letter, face, stake }
}

fn bidding_phase(ctx: &mut Ctx, seats: &mut [Seat], round: u32, quit: &mut bool) {
    for s in seats.iter_mut() {
        s.bet = None;
        s.last_delta = 0;
        s.note.clear();
    }
    draw_bidding_frame(ctx, seats, round, "the table opens — bids incoming...");
    hold(1_000, quit);

    let order = shuffled_order(ctx.rng);
    let step_ms = BID_MS / SEATS as u64;
    for &i in &order {
        let bet = ai_bid(ctx.rng, seats[i].player.chips);
        seats[i].player.chips -= bet.stake;
        seats[i].bet = Some(bet);
        draw_bidding_frame(ctx, seats, round, "bids are coming in...");
        hold(step_ms, quit);
    }
    draw_bidding_frame(ctx, seats, round, "bids are closed — the table is set.");
    hold(5_000, quit);
}

fn draw_bidding_frame(ctx: &mut Ctx, seats: &[Seat], round: u32, status: &str) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, &format!("ULTRA CASINO DICE — round {round}/{ROUNDS}"));
    ctx.screen.blank();
    ctx.screen.line(&theme.dim(&format!("  {DICE} dice on the table · {SEATS} players at the felt")));
    ctx.screen.blank();
    let order = bet_order(seats);
    if order.is_empty() {
        ctx.screen.line(&theme.dim("  ...every seat is still deciding..."));
    }
    for i in order {
        let s = &seats[i];
        let b = s.bet.unwrap();
        ctx.screen.line(&format!(
            "  {}  {:<20} backs face {} — {} chips",
            theme.paint(theme::GOLD, &format!("[{}]", LETTERS[b.letter])),
            s.player.name,
            b.face,
            b.stake
        ));
    }
    ctx.screen.blank();
    ctx.screen.line(&theme.dim(&format!("  {status}   ·   [Q] finish this round, then back to the floor")));
    ctx.screen.present();
}

// ----------------------------------------------------------------- roll --

fn roll_phase(ctx: &mut Ctx, seats: &[Seat], round: u32, quit: &mut bool) -> Vec<u32> {
    let no_hits = [false; DICE];
    dice_art::animate_table_roll(ctx.screen, ctx.rng, DICE, &dice_art::ULTRA_FRAME_DELAYS, |screen, frame| {
        if ui::poll_leave_signal() {
            *quit = true;
        }
        let theme = screen.theme;
        screen.begin();
        ui::header(screen, &format!("ULTRA CASINO DICE — round {round}/{ROUNDS}"));
        screen.blank();
        for line in dice_art::table_block(&theme, frame, &LETTERS, &no_hits, true, 6) {
            screen.line(&line);
        }
        screen.blank();
        for i in bet_order(seats) {
            let s = &seats[i];
            let b = s.bet.unwrap();
            screen.line(&theme.dim(&format!("  {}  {:<20} called {} on {}", LETTERS[b.letter], s.player.name, b.face, LETTERS[b.letter])));
        }
        screen.blank();
        screen.line(&theme.dim("  the table is rolling..."));
    })
}

// --------------------------------------------------------------- settle --

/// Pure payout math, kept separate from `Ctx`/`Screen` so it's directly
/// unit-testable: given each seat's bet (or none) and the dice that
/// landed, who won and what every seat's net change is. There's no house
/// here — the pot is exactly the sum of every stake, so `deltas` always
/// sums to zero: a miss loses exactly what a hit collects.
fn compute_payouts(bets: &[Option<Bet>], values: &[u32]) -> (Vec<usize>, Vec<i64>) {
    let pot: i64 = bets.iter().filter_map(|b| b.map(|b| b.stake)).sum();
    let winners: Vec<usize> = (0..bets.len()).filter(|&i| bets[i].is_some_and(|b| values[b.letter] == b.face)).collect();
    let winners_stake: i64 = winners.iter().map(|&i| bets[i].unwrap().stake).sum();

    let mut deltas = vec![0i64; bets.len()];
    if winners.is_empty() {
        for (i, b) in bets.iter().enumerate() {
            if let Some(b) = b {
                deltas[i] = -b.stake;
            }
        }
    } else {
        let mut distributed = 0i64;
        for (n, &i) in winners.iter().enumerate() {
            let b = bets[i].unwrap();
            let share = if n == winners.len() - 1 {
                pot - distributed
            } else {
                ((pot as i128) * (b.stake as i128) / (winners_stake as i128)) as i64
            };
            distributed += share;
            deltas[i] = share - b.stake;
        }
        for (i, b) in bets.iter().enumerate() {
            if winners.contains(&i) {
                continue;
            }
            if let Some(b) = b {
                deltas[i] = -b.stake;
            }
        }
    }
    (winners, deltas)
}

fn settle_round(ctx: &mut Ctx, seats: &mut [Seat], values: &[u32], round: u32) {
    let pot: i64 = seats.iter().filter_map(|s| s.bet.map(|b| b.stake)).sum();
    let bets: Vec<Option<Bet>> = seats.iter().map(|s| s.bet).collect();
    let (winners, deltas) = compute_payouts(&bets, values);

    let mut hits = [false; DICE];
    for &i in &winners {
        hits[seats[i].bet.unwrap().letter] = true;
    }

    // Each seat's stake was already deducted at bid time (`bidding_phase`),
    // so crediting `delta` here — the *net* change, win or lose — leaves
    // every chip stack exactly right: a winner's stake comes back as part
    // of their share, a loser's stays gone.
    for (i, seat) in seats.iter_mut().enumerate() {
        if seat.bet.is_none() {
            continue;
        }
        seat.last_delta = deltas[i];
        seat.player.chips += deltas[i];
        seat.player.rounds += 1;
        seat.note = if winners.contains(&i) { "HIT".into() } else { "miss".into() };
    }
    for &i in &winners {
        seats[i].player.wins += 1;
    }

    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, &format!("ULTRA CASINO DICE — round {round}/{ROUNDS}"));
    ctx.screen.blank();
    for line in dice_art::table_block(&theme, values, &LETTERS, &hits, true, 6) {
        ctx.screen.line(&line);
    }
    ctx.screen.blank();
    for i in bet_order(seats) {
        let s = &seats[i];
        let b = s.bet.unwrap();
        let delta = if s.last_delta >= 0 { theme.win(&format!("{:+} chips", s.last_delta)) } else { theme.lose(&format!("{:+} chips", s.last_delta)) };
        ctx.screen.line(&format!("  {:<20} {} on {}  →  {}  {}", s.player.name, theme.bold(&LETTERS[b.letter].to_string()), b.face, delta, theme.dim(&s.note)));
    }
    ctx.screen.blank();
    if winners.is_empty() {
        ctx.screen.line(&theme.dim(&format!("  no one called it — {pot} chips lost to the house.")));
    } else {
        let label = if winners.len() == 1 {
            format!("{} WINS {} CHIPS", seats[winners[0]].player.name.to_uppercase(), pot)
        } else {
            format!("{} PLAYERS SPLIT {} CHIPS", winners.len(), pot)
        };
        for line in widgets::banner(&theme, &label, true) {
            ctx.screen.line(&format!("  {line}"));
        }
    }
    ctx.screen.present();

    let dice_str = values.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let result = if winners.is_empty() {
        format!("no winners, {pot} chips lost")
    } else {
        let parts: Vec<String> = winners.iter().map(|&i| format!("{} {:+}", seats[i].player.name, seats[i].last_delta)).collect();
        format!("winners: {}", parts.join("; "))
    };
    history::append(&format!("round {round}/{ROUNDS} dice=[{dice_str}] pot={pot} {result}"));

    ctx.store.bump("ultra.rounds", 1);
    if !winners.is_empty() {
        ctx.store.bump("ultra.hits", winners.len() as i64);
    }
}

// --------------------------------------------------------------- decide --

fn will_leave(rng: &mut Rng, seat: &Seat) -> bool {
    let ratio = seat.player.chips as f64 / seat.player.start_chips.max(1) as f64;
    let mut chance = 0.18;
    if ratio < 0.4 {
        chance += 0.35; // badly down — bail before it gets worse
    } else if ratio < 0.75 {
        chance += 0.12;
    } else if ratio > 2.0 {
        chance += 0.25; // way up — cash out a winner
    }
    (rng.below(1000) as f64) < chance * 1000.0
}

fn decide_phase(ctx: &mut Ctx, seats: &mut [Seat], round: u32, departures: &mut Vec<Departure>, quit: &mut bool) {
    let order = shuffled_order(ctx.rng);
    let step_ms = DECIDE_MS / SEATS as u64;
    let mut decided: Vec<usize> = Vec::with_capacity(SEATS);

    for &i in &order {
        seats[i].leaving = will_leave(ctx.rng, &seats[i]);
        decided.push(i);
        draw_decide_frame(ctx, seats, round, &decided);
        hold(step_ms, quit);
    }

    for i in 0..SEATS {
        if !seats[i].leaving {
            continue;
        }
        let p = &seats[i].player;
        departures.push(Departure { name: p.name.clone(), rounds: p.rounds, wins: p.wins, net: p.chips - p.start_chips, round_left: round });
        let existing = seat_names(seats);
        seats[i] = new_seat(ctx.rng, round, &existing);
    }
}

fn draw_decide_frame(ctx: &mut Ctx, seats: &[Seat], round: u32, decided: &[usize]) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, &format!("ULTRA CASINO DICE — round {round}/{ROUNDS}"));
    ctx.screen.blank();
    ctx.screen.line(&theme.dim("  the table decides whether to stay..."));
    ctx.screen.blank();
    for (i, s) in seats.iter().enumerate() {
        let net = s.player.chips - s.player.start_chips;
        if decided.contains(&i) {
            let verb = if s.leaving { "cashes out" } else { "stays at the table" };
            ctx.screen.line(&format!("  {:<20} {verb} — {} chips ({:+})", s.player.name, s.player.chips, net));
        } else {
            ctx.screen.line(&theme.dim(&format!("  {:<20} deciding...", s.player.name)));
        }
    }
    ctx.screen.blank();
    ctx.screen.line(&theme.dim(&format!("  round {}/{ROUNDS} begins shortly...   ·   [Q] finish this round, then back to the floor", round + 1)));
    ctx.screen.present();
}

// -------------------------------------------------------------- summary --

fn log_session_summary(seats: &[Seat], departures: &[Departure]) {
    let mut standing: Vec<&Seat> = seats.iter().collect();
    standing.sort_by_key(|s| std::cmp::Reverse(s.player.chips - s.player.start_chips));
    for s in standing {
        let net = s.player.chips - s.player.start_chips;
        history::append(&format!("  final: {} {} chips (net {net:+}) {} rounds {} wins — still seated", s.player.name, s.player.chips, s.player.rounds, s.player.wins));
    }
    let mut left: Vec<&Departure> = departures.iter().collect();
    left.sort_by_key(|d| std::cmp::Reverse(d.net));
    for d in left {
        history::append(&format!("  final: {} left after round {} (net {:+}) {} rounds {} wins — cashed out", d.name, d.round_left, d.net, d.rounds, d.wins));
    }
}

fn draw_session_summary(ctx: &mut Ctx, seats: &[Seat], departures: &[Departure], quitting: bool, countdown: Option<u64>) {
    let theme = ctx.theme();
    ctx.screen.begin();
    ui::header(ctx.screen, if quitting { "ULTRA CASINO DICE — SESSION ENDED" } else { "ULTRA CASINO DICE — SESSION COMPLETE" });
    ctx.screen.blank();
    ctx.screen.line(&theme.bold("  still at the table"));
    let mut standing: Vec<&Seat> = seats.iter().collect();
    standing.sort_by_key(|s| std::cmp::Reverse(s.player.chips - s.player.start_chips));
    for s in &standing {
        let net = s.player.chips - s.player.start_chips;
        let net_str = if net >= 0 { theme.win(&format!("{net:+}")) } else { theme.lose(&format!("{net:+}")) };
        ctx.screen.line(&format!("   {:<20} {:>5} chips   {:>8}   {} rounds, {} wins", s.player.name, s.player.chips, net_str, s.player.rounds, s.player.wins));
    }
    if !departures.is_empty() {
        ctx.screen.blank();
        ctx.screen.line(&theme.bold("  cashed out mid-session"));
        let mut left: Vec<&Departure> = departures.iter().collect();
        left.sort_by_key(|d| std::cmp::Reverse(d.net));
        for d in left {
            let net_str = if d.net >= 0 { theme.win(&format!("{:+}", d.net)) } else { theme.lose(&format!("{:+}", d.net)) };
            ctx.screen.line(&format!("   {:<20} left after round {:<2}  {:>8}   {} rounds, {} wins", d.name, d.round_left, net_str, d.rounds, d.wins));
        }
    }
    ctx.screen.blank();
    ctx.screen.line(&theme.dim(&format!("  full history: {}", history::path_display())));
    if let Some(n) = countdown {
        ctx.screen.blank();
        ctx.screen.line(&theme.dim(&format!("  a new table opens in {n}s — press Q to return to the floor instead")));
    }
    ctx.screen.present();
}

fn idle_countdown(ctx: &mut Ctx, seats: &[Seat], departures: &[Departure], total_ms: u64) -> bool {
    let mut quit = false;
    let steps = (total_ms / 1000).max(1);
    for remaining in (1..=steps).rev() {
        draw_session_summary(ctx, seats, departures, false, Some(remaining));
        hold(1000, &mut quit);
        if quit {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bet(letter: usize, face: u32, stake: i64) -> Option<Bet> {
        Some(Bet { letter, face, stake })
    }

    #[test]
    fn letters_cover_every_die() {
        assert_eq!(LETTERS.len(), DICE);
        assert_eq!(LETTERS[11], 'L');
    }

    #[test]
    fn no_winners_loses_every_stake_to_nobody() {
        let values = [1, 2, 3, 4, 5, 6, 1, 2, 3, 4, 5, 6];
        let bets = vec![bet(0, 6, 10), bet(1, 6, 25), None];
        let (winners, deltas) = compute_payouts(&bets, &values);
        assert!(winners.is_empty());
        assert_eq!(deltas, vec![-10, -25, 0]);
    }

    #[test]
    fn single_winner_takes_the_whole_pot() {
        let values = [6, 2, 3, 4, 5, 6, 1, 2, 3, 4, 5, 6];
        // seat 0 calls letter A (die value 6) on face 6 — a hit; seat 1 misses.
        let bets = vec![bet(0, 6, 20), bet(1, 6, 30)];
        let (winners, deltas) = compute_payouts(&bets, &values);
        assert_eq!(winners, vec![0]);
        // the winner's stake comes back plus the loser's whole stake.
        assert_eq!(deltas, vec![30, -30]);
    }

    #[test]
    fn tied_winners_split_the_pot_proportional_to_stake() {
        let values = [6, 6, 3, 4, 5, 6, 1, 2, 3, 4, 5, 6];
        // seats 0 and 1 both call a die that lands on their face; seat 2 misses.
        let bets = vec![bet(0, 6, 30), bet(1, 6, 10), bet(2, 1, 20)];
        let (winners, deltas) = compute_payouts(&bets, &values);
        assert_eq!(winners, vec![0, 1]);
        // pot is 60; winners_stake is 40; seat 0 backed 3/4 of that, seat 1 backed 1/4.
        assert_eq!(deltas[0], 15); // (60 * 30/40) - 30
        assert_eq!(deltas[1], 5); // (60 * 10/40) - 10
        assert_eq!(deltas[2], -20);
    }

    #[test]
    fn payouts_are_always_zero_sum() {
        // Whatever the outcome, nobody's stake goes anywhere but into other
        // players' pockets — there's no house to skim a cut here.
        let values = [3, 6, 6, 1, 2, 4, 6, 5, 3, 2, 1, 6];
        let bets = vec![bet(0, 3, 12), bet(1, 6, 47), bet(2, 4, 8), None, bet(4, 6, 19)];
        let (_, deltas) = compute_payouts(&bets, &values);
        assert_eq!(deltas.iter().sum::<i64>(), 0);
    }

    #[test]
    fn will_leave_never_panics_across_the_chip_range() {
        let mut rng = Rng::from_seed(7);
        for chips in [0i64, 1, 50, 200, 1200, 5000] {
            let player = AiPlayer { name: "Test".into(), chips, start_chips: 200, rounds: 0, wins: 0 };
            let seat = Seat { player, bet: None, last_delta: 0, note: String::new(), leaving: false };
            let _ = will_leave(&mut rng, &seat);
        }
    }

    #[test]
    fn ai_bid_stake_never_exceeds_the_stack() {
        let mut rng = Rng::from_seed(99);
        for chips in [1i64, 2, 5, 50, 1200] {
            for _ in 0..20 {
                let b = ai_bid(&mut rng, chips);
                assert!(b.stake >= 1 && b.stake <= chips.max(1));
                assert!(b.letter < DICE);
                assert!((1..=6).contains(&b.face));
            }
        }
    }
}
