//! The house auditor: plays every table a great many times with no
//! rendering and checks that each returns what it claims to.
//!
//! This app's central promise is that all fourteen paytables are honestly
//! priced rather than eyeballed. Each table proves its own arithmetic in
//! its own tests, but arithmetic that is individually right can still add
//! up to a game that pays 1.4x — so this runs the *real* settlement code
//! end to end and holds the realised return to a declared band.
//!
//! Every `simulate` it calls goes through the same payout functions the
//! table itself uses. Where a table's return depends on how the player
//! plays it, the strategy is fixed here and named in the band's comment,
//! because a return with no strategy attached means nothing.
//!
//! One rule about what belongs here: **simulate only what cannot be
//! computed.** A table with a small outcome space (slots' 9,261 lines,
//! Chuck-a-Luck's 216 rolls, roulette's 37 pockets, Big Six's 54 segments)
//! or a closed-form distribution (keno's hypergeometric odds, plinko's
//! binomial, the scratch print run) is priced exactly in its own file, and
//! that check is strictly better than anything this file could do. What is
//! left here is the tables whose returns really do need playing out.
//!
//! Sampling also cannot price a long tail: keno's nine-spot board carries a
//! 100,000x jackpot, and a single hit inside 400,000 rounds moves the
//! measured return by twenty-five points. That is why keno is audited by
//! its exact odds and not from here.
//!
//! If a band fails after a deliberate paytable change, re-measure with
//! `cargo test --release audit -- --nocapture` and move the band — do not
//! widen it to make the failure go away.

use super::{baccarat, bigsix, bingo, blackjack, chuck, crash, hilo, horses, mines, roulette, scratch, slots, threecard, vidpoker, war};
use crate::rng::Rng;

/// Rounds per measurement. Big enough that a half-percent drift shows,
/// small enough that the whole audit stays inside a second or two.
const ROUNDS: usize = 120_000;
/// Long-tailed tables (a 10,000x keno sweep, a 220x plinko edge) need more
/// rounds before their mean settles.
const LONG_TAIL: usize = 400_000;

/// Plays `rounds` rounds and reports chips back per chip staked.
fn measure(seed: u64, rounds: usize, mut round: impl FnMut(&mut Rng) -> (i64, i64)) -> f64 {
    let mut rng = Rng::from_seed(seed);
    let (mut staked, mut back) = (0i64, 0i64);
    for _ in 0..rounds {
        let (s, b) = round(&mut rng);
        staked += s;
        back += b;
    }
    assert!(staked > 0, "a table that stakes nothing cannot be audited");
    back as f64 / staked as f64
}

/// Measures one table and holds it to `lo..=hi`. Prints every result so
/// `--nocapture` gives the whole book at a glance.
fn audit(name: &str, lo: f64, hi: f64, rounds: usize, round: impl FnMut(&mut Rng) -> (i64, i64)) {
    // Seeded off the name so each table gets its own stream and a result
    // is reproducible run to run.
    let seed = name.bytes().fold(0xC0FFEEu64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
    let rtp = measure(seed, rounds, round);
    println!("  {name:<34} {:>7.3}%   band {:.0}-{:.0}%", rtp * 100.0, lo * 100.0, hi * 100.0);
    assert!(rtp >= lo && rtp <= hi, "{name} returned {rtp:.4}, outside its declared band of {lo}..={hi}");
}

#[test]
fn the_arcade_returns_what_it_claims() {
    println!("\nTHE ARCADE");
    audit("slots", 0.90, 0.98, LONG_TAIL, slots::simulate);
    audit("scratch cards", 0.93, 1.02, LONG_TAIL, scratch::simulate);
    audit("bingo", 0.90, 1.02, ROUNDS, bingo::simulate);
    // Mines and crash are both cash-out games: the strategy below is the
    // one the band was measured against.
    audit("mines · 3 mines, 5 tiles", 0.90, 1.00, ROUNDS, mines::simulate);
    audit("crash · out at 2.00x", 0.93, 1.00, ROUNDS, crash::simulate);
}

#[test]
fn the_wheels_return_what_they_claim() {
    println!("\nTHE WHEELS");
    // Both wheels price exactly in their own files; these runs confirm the
    // dealt outcomes actually follow those odds rather than the paytable
    // merely being right on paper.
    for i in 0..roulette::BETS {
        audit(&format!("roulette · {}", roulette::bet_name(i)), 0.93, 1.00, LONG_TAIL, move |r| roulette::simulate_at(r, i));
    }
    audit("big six · the 1", 0.85, 0.92, LONG_TAIL, |r| bigsix::simulate_at(r, 0));
}

#[test]
fn the_card_tables_return_what_they_claim() {
    println!("\nCARD TABLES");
    for i in 0..baccarat::BETS {
        audit(&format!("baccarat · {}", baccarat::bet_name(i)), 0.85, 1.00, LONG_TAIL, move |r| baccarat::simulate_at(r, i));
    }
    audit("casino war · always to war", 0.90, 1.00, ROUNDS, war::simulate);
    audit("three card poker · Q64+", 0.90, 1.02, ROUNDS, threecard::simulate);
    audit("hi-lo · out at 2.00x", 0.88, 1.00, ROUNDS, hilo::simulate);
    // Both of these are played below optimal on purpose — see their
    // `simulate` docs. The band is a floor, not the machine's headline.
    audit("video poker · demo holds", 0.70, 1.00, LONG_TAIL, vidpoker::simulate);
    audit("blackjack · hit below 17", 0.90, 1.00, ROUNDS, blackjack::simulate);
}

#[test]
fn the_dice_tables_return_what_they_claim() {
    println!("\nDICE TABLES");
    // Chuck-a-Luck prices exactly in its own file; this confirms the dice
    // actually land the way that pricing assumes.
    audit("chuck-a-luck · high", 0.94, 1.00, LONG_TAIL, |r| chuck::simulate_at(r, 0));
    for horse in 0..horses::FIELD {
        audit(&format!("horses · runner {}", horse + 1), 0.88, 1.00, LONG_TAIL, move |r| horses::simulate_at(r, horse));
    }
}

#[test]
fn no_table_anywhere_pays_more_than_it_takes() {
    // The one claim that holds across the whole building: play any table
    // long enough, under the strategies above, and the house is ahead.
    let checks: Vec<(&str, f64)> = vec![
        ("slots", measure(1, LONG_TAIL, slots::simulate)),
        ("scratch", measure(2, LONG_TAIL, scratch::simulate)),
        ("bingo", measure(3, ROUNDS, bingo::simulate)),
        ("mines", measure(4, ROUNDS, mines::simulate)),
        ("crash", measure(5, ROUNDS, crash::simulate)),
        ("roulette", measure(8, LONG_TAIL, roulette::simulate)),
        ("bigsix", measure(9, LONG_TAIL, bigsix::simulate)),
        ("baccarat", measure(10, LONG_TAIL, baccarat::simulate)),
        ("war", measure(11, ROUNDS, war::simulate)),
        ("threecard", measure(12, ROUNDS, threecard::simulate)),
        ("hilo", measure(13, ROUNDS, hilo::simulate)),
        ("vidpoker", measure(14, LONG_TAIL, vidpoker::simulate)),
        ("blackjack", measure(15, ROUNDS, blackjack::simulate)),
        ("chuck", measure(16, LONG_TAIL, chuck::simulate)),
        ("horses", measure(17, LONG_TAIL, horses::simulate)),
    ];
    for (name, rtp) in checks {
        assert!(rtp < 1.0, "{name} returns {rtp:.4} — the players own the building");
    }
}
