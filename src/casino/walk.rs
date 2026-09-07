//! The whole casino, walked end to end.
//!
//! Every other test in this module checks one thing in isolation, which is
//! how they should be written and is also their blind spot: a system can
//! pass its own tests and still be wired to nothing. This file is the
//! opposite shape — one casino, opened once, walked through in order, with
//! each numbered step asserting that what the last phase built is actually
//! reachable from what the next one needs.
//!
//! It is deliberately one long test rather than twenty-four short ones. The
//! steps share a floor precisely because that is the thing under test: not
//! "does the roster work" but "does the roster the *manager* is running
//! feed the leaderboard the *screen* would draw". Splitting it up would
//! give twenty-four more unit tests and lose the only thing this file adds.
//!
//! It runs the clock fast and takes the better part of a minute. That is
//! the price of walking a night.

use std::time::{Duration, Instant};

use super::analytics::Span;
use super::bank::Movement;
use super::config::Config;
use super::event::Weight;
use super::instance::{Kind, Limit};
use super::manager::Manager;
use super::save::{Save, VERSION};
use super::tournament::Stage;
use crate::ui::Badge;

/// Waits for something to become true, or gives up.
fn until(mut ready: impl FnMut() -> bool, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ready()
}

/// A configuration that makes a night's worth of things happen in a minute
/// of ours, and makes the rolled-for ones certain rather than likely.
fn brisk() -> Config {
    Config {
        speed: 10_000,
        roster_size: 60,
        arrivals_period: Duration::from_secs(2),
        arrivals_per_period: 4,
        away_for: (Duration::from_secs(20), Duration::from_secs(90)),
        demand_sample: Duration::from_secs(2),
        demand_period: Duration::from_secs(6),
        expense_period: Duration::from_secs(20),
        happening_chance: 100,
        happening_period: Duration::from_secs(4),
        happening_for: (Duration::from_secs(60), Duration::from_secs(180)),
        tourney_every: Duration::from_secs(45),
        tourney_registration: Duration::from_secs(20),
        tourney_round_every: Duration::from_secs(4),
        tourney_min_field: 4,
        ..Config::default()
    }
}

#[test]
fn the_whole_casino_end_to_end() {
    // ---- 1. The doors open ------------------------------------------
    let mut m = Manager::start(2_026, 1_000_000, 80_000_000, Badge::new());
    m.configure(brisk());
    assert!(m.is_running(), "1: the simulation thread never started");
    let opening = m.balances();
    assert_eq!(m.table_count(), 0, "1: a casino opened with tables already on the floor");
    assert!(
        m.feed(8, Weight::Notable).iter().any(|r| matches!(r.event, super::event::Event::CasinoOpened { .. })),
        "1: the doors opening was not announced"
    );

    // ---- 2. Tables open and deal themselves --------------------------
    m.open(Kind::Slots, 4);
    m.open(Kind::Roulette, 2);
    m.open(Kind::Blackjack, 2);
    m.open_at(Kind::Baccarat, 1, Limit::High);
    assert_eq!(m.table_count(), 9, "2: not every table opened");
    assert!(until(|| m.snapshot().rounds > 50, Duration::from_secs(15)), "2: the tables never dealt anything");

    // ---- 3. Opening a table seats nobody -----------------------------
    // People arrive at their own rate; a seat is only ever filled by
    // somebody actually standing there.
    let view = m.snapshot();
    assert!(view.known > 0, "3: nobody ever walked in");
    assert!(view.crowd <= view.known, "3: more people in the room than the casino knows of");

    // ---- 4. They buy in at the cage ----------------------------------
    assert_ne!(m.balances(), opening, "4: nobody ever bought a chip");
    let (_, _, bought, _) = m.snapshot().totals;
    assert!(bought > 0, "4: the cage never sold anything");

    // ---- 5. Rounds resolve through the real games --------------------
    let v = m.snapshot();
    assert!(v.handle > 0 && v.bets > 0, "5: bets were counted but nothing was staked");
    assert_eq!(v.ggr, v.handle - v.payouts, "5: the gross win is not the handle less the payouts");

    // ---- 6. Chips are moved, never made ------------------------------
    // The tray plus every stack in the room is a constant, once the money
    // in flight through the cage is accounted for. Checked as an identity
    // on the books rather than by counting, because the floor is moving.
    let v = m.snapshot();
    let (collected, paid, bought, cashed) = v.totals;
    assert_eq!(v.table_profit, collected - paid, "6: the tray's books do not add up");
    assert_eq!(v.cage_profit, bought - cashed, "6: the cage's books do not add up");

    // ---- 7. The feed publishes, and the traffic stays out of it ------
    assert!(!m.feed(64, Weight::Notable).is_empty(), "7: nothing readable was ever published");
    for r in m.snapshot().feed {
        assert!(r.weight >= Weight::Notable, "7: per-round traffic reached a screen: {:?}", r.event);
    }

    // ---- 8. A reader is never handed the same thing twice ------------
    let mut cursor = m.feed_cursor();
    let mut announced = Vec::new();
    for _ in 0..10 {
        let (fresh, next) = m.feed_since(cursor, Weight::Notable);
        for r in fresh {
            assert!(r.seq > cursor, "8: an event was announced twice");
            announced.push(r.seq);
        }
        cursor = next;
        std::thread::sleep(Duration::from_millis(30));
    }
    let mut unique = announced.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), announced.len(), "8: the same event was announced more than once");

    // ---- 9. Looking at a table does not touch it ---------------------
    let watched = m.ids()[0];
    let before = m.table(watched).expect("9: the table is open");
    for _ in 0..50 {
        let _ = m.table(watched);
        let _ = m.most_interesting();
    }
    let after = m.table(watched).expect("9: the table is still open");
    assert_eq!(after.name, before.name);
    assert!(after.round >= before.round, "9: watching a table wound it back");
    assert_eq!(m.table_count(), 9, "9: watching closed something");

    // ---- 10. Pausing stops one table and only one --------------------
    let paused = m.ids()[1];
    m.toggle_pause(paused);
    let others: Vec<u32> = m.ids().into_iter().filter(|id| *id != paused).collect();
    let marks: Vec<u64> = others.iter().filter_map(|id| m.table(*id)).map(|t| t.round).collect();
    let held = m.table(paused).expect("10: the paused table is still there").round;
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(m.table(paused).unwrap().round, held, "10: a paused table played on");
    let moved = others.iter().filter_map(|id| m.table(*id)).zip(marks.iter()).filter(|(t, was)| t.round > **was).count();
    assert!(moved > 0, "10: pausing one table stopped the others");
    m.toggle_pause(paused);

    // ---- 11. Closing a table leaves the rest, and its people live ----
    let doomed = m.ids()[2];
    let known_before = m.snapshot().known;
    m.close(doomed);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(m.table_count(), 8, "11: closing one table took others with it");
    assert!(m.snapshot().known >= known_before, "11: closing a table deleted people");
    assert!(!m.ids().contains(&doomed));

    // ---- 12. The clock changes rate without restarting anything ------
    let id = m.ids()[0];
    let before = m.table(id).expect("12: the table is open");
    m.set_speed(250);
    std::thread::sleep(Duration::from_millis(200));
    m.set_speed(10_000);
    let after = m.table(id).expect("12: the table survived the speed change");
    assert!(after.round >= before.round, "12: the round counter went backwards");
    assert!(after.staked >= before.staked, "12: a table's turnover was reset");
    assert_eq!(m.config().speed, 10_000);

    // ---- 13. Limits bind, and the high-limit room is exclusive -------
    let cfg = m.config();
    for t in m.snapshot().tables {
        let full = m.table(t.id).expect("13: the table is open");
        for round in full.history.iter() {
            for seat in round.seats.iter() {
                assert!(seat.staked <= cfg.vip_max_bet, "13: {} bet over every ceiling there is", seat.name);
            }
        }
        if full.limit == Limit::High {
            for p in full.patrons.iter() {
                assert!(cfg.is_vip(p.tier(&cfg)), "13: {} got a seat in the high-limit room", p.name);
            }
        }
    }

    // ---- 14. A tier is earned by turnover, not by bankroll -----------
    for t in m.snapshot().tables {
        for p in m.table(t.id).map(|t| t.patrons).unwrap_or_default() {
            assert_eq!(p.tier(&cfg), cfg.tier_of(p.lifetime.staked), "14: {}'s tier is not their turnover", p.name);
        }
    }

    // ---- 15. Occupancy is measured, and is a real fraction -----------
    assert!(until(|| !m.snapshot().demand.is_empty(), Duration::from_secs(10)), "15: the room never took a reading");
    let v = m.snapshot();
    assert!(v.occupancy > 0, "15: a floor with people at it read as empty");
    assert!(v.occupancy <= 1_000, "15: a floor cannot be {} permille full", v.occupancy);
    for (game, st) in v.demand.iter() {
        assert!(st.utilization() <= 1_000, "15: {game} read as {} permille full", st.utilization());
    }

    // ---- 16. The room's taste moves, within bounds -------------------
    for (game, st) in m.snapshot().demand.iter() {
        assert!(st.appeal >= cfg.appeal_floor && st.appeal <= cfg.appeal_ceiling, "16: {game} drifted to {}", st.appeal);
    }

    // ---- 17. The building charges its costs, out of the cage ---------
    assert!(until(|| m.snapshot().spent > 0, Duration::from_secs(15)), "17: the bills never came");
    let v = m.snapshot();
    assert!(v.by_expense.iter().any(|(k, _)| *k == "overhead"), "17: nothing was billed for the building");
    assert!(v.by_expense.iter().any(|(k, _)| *k == "staffing"), "17: nothing was billed for the staff");
    assert_eq!(v.spent, v.by_expense.iter().map(|(_, n)| *n).sum::<i64>(), "17: the costs do not add up");

    // ---- 18. The books balance ---------------------------------------
    let v = m.snapshot();
    assert_eq!(v.ggr, v.handle - v.payouts, "18: the gaming win");
    assert_eq!(v.hold, v.ggr * 10_000 / v.handle, "18: the hold");
    assert_eq!(v.ngr, v.cage_profit - v.spent, "18: the bottom line");
    let wagered = v.movements.iter().find(|(k, _, _)| *k == Movement::Wager).unwrap().2;
    assert_eq!(wagered, v.handle, "18: the movement ledger disagrees with the handle");

    // ---- 19. Analytics accumulate, and the spans are bounded ---------
    let v = m.snapshot();
    let all = v.spans.iter().find(|(s, _)| *s == Span::AllNight).map(|(_, t)| *t).expect("19: all night is missing");
    let minute = v.spans.iter().find(|(s, _)| *s == Span::LastMinute).map(|(_, t)| *t).expect("19: the last minute is missing");
    assert!(all.handle > 0, "19: the night's figures are empty");
    assert!(minute.handle <= all.handle, "19: a minute held more than the whole night");
    assert!(minute.bets <= all.bets);
    assert_eq!(all.ggr(), all.handle - all.payouts);

    // ---- 20. Every game is measured the same way ---------------------
    assert!(!v.games.is_empty(), "20: no game has any figures");
    for (game, g) in v.games.iter() {
        // A table with nobody at it still deals — the high-limit room with
        // no VIPs in it yet is exactly that — so rounds without bets is a
        // real state, and a handle without bets is not.
        assert!(g.rounds > 0, "20: {game} has figures without having been dealt");
        assert!(g.handle == 0 || g.bets > 0, "20: {game} has a handle without any bets");
        assert_eq!(g.ggr(), g.handle - g.payouts, "20: {game}'s win does not add up");
        assert!(Kind::from_key(game).is_some() || *game == "tourney", "20: a book for {game}, which is not a game");
    }

    // ---- 21. The leaderboards rank ------------------------------------
    assert!(until(|| !m.snapshot().top_turnover.is_empty(), Duration::from_secs(10)), "21: nobody made the boards");
    let v = m.snapshot();
    for pair in v.top_turnover.windows(2) {
        assert!(pair[0].1 >= pair[1].1, "21: the spenders are out of order");
    }
    for pair in v.top_regulars.windows(2) {
        assert!(pair[0].1 >= pair[1].1, "21: the regulars are out of order");
    }
    assert_eq!(v.by_tier.iter().sum::<usize>(), v.known, "21: the tier census does not cover everybody");

    // ---- 22. Things happen, and they never touch a payout ------------
    assert!(until(|| !m.snapshot().weather.is_empty(), Duration::from_secs(15)), "22: nothing ever happened");
    let v = m.snapshot();
    for g in v.weather.iter() {
        assert!(!g.what.label().is_empty());
        assert!(g.until > g.started, "22: something ended before it began");
    }
    // The books still add up with the weather in — which is the whole
    // claim: a happening changes traffic, not maths.
    assert_eq!(v.ggr, v.handle - v.payouts, "22: the weather reached the books");

    // ---- 23. A tournament runs itself, and pays its pool out whole ---
    let mut finished = None;
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline && finished.is_none() {
        for t in m.snapshot().tourneys {
            if t.stage == Stage::Done && t.entered() >= 4 && !t.paid.is_empty() {
                finished = Some(t);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let t = finished.expect("23: no tournament ever played down to a winner");
    assert_eq!(t.alive(), 1, "23: it finished with {} still in", t.alive());
    assert_eq!(t.paid.iter().map(|p| p.prize).sum::<i64>(), t.pool, "23: the pool did not pay out whole");
    assert_eq!(t.paid[0].place, 1);
    assert_eq!(t.paid[0].patron, t.winner().expect("23: somebody won").patron);

    // ---- 24. It survives being closed and opened again ---------------
    let before = m.snapshot();
    let save = m.saved();
    assert_eq!(save.version, VERSION);
    m.stop();

    let at = std::env::temp_dir().join("dice_arena_walk.save");
    let _ = std::fs::remove_file(&at);
    save.write(&at).expect("24: the save would not write");
    let read_back = Save::load(&at).expect("24: the save would not load");
    assert_eq!(read_back, save, "24: the casino changed on its way through the file");

    let reopened = Manager::resume(2_027, &read_back, Badge::new());
    std::thread::sleep(Duration::from_millis(300));
    let after = reopened.snapshot();
    assert!(after.handle >= before.handle, "24: the night's turnover was forgotten");
    assert!(after.known >= before.known, "24: the regulars were forgotten");
    assert_eq!(after.tables.len(), before.tables.len(), "24: the floor plan changed");
    assert!(after.tables.iter().any(|t| t.limit == Limit::High), "24: the high-limit room did not reopen");
    assert!(!after.tables.iter().any(|t| t.name.ends_with("#1")), "24: table numbering started again");
    // And it is a casino, not a photograph of one.
    assert!(until(|| reopened.snapshot().rounds > after.rounds, Duration::from_secs(15)), "24: the reopened floor is dead");
    assert!(reopened.is_running());

    let _ = std::fs::remove_file(&at);
}
