//! How worth watching a table is, right now.
//!
//! "Follow the action" needs an answer to a question nobody has asked the
//! program before: of forty tables all running at once, which one would a
//! person actually want to be stood at? This file is that answer, and it is
//! a *score*, not a rule — the tables it ranks carry on exactly as they
//! were whether anyone watches them or not.
//!
//! Five things make a table interesting, and every one of them is weighted
//! from configuration rather than from a number written down here:
//!
//! - **Money on the table** — the size of the pots going through it.
//! - **Swings** — the biggest single hit anyone has taken off it lately. A
//!   quiet table with one enormous win beats a busy one with none.
//! - **A crowd** — how many people are sat at it.
//! - **Somebody worth watching** — a VIP in a seat.
//! - **The house losing** — a table the players are beating is more fun to
//!   watch than one grinding them down, and it is also the rarer event.
//!
//! Every input comes from the table's own bounded recent history, so the
//! score costs the same to compute whether the casino opened a minute ago
//! or has been running all night. Nothing here walks the whole night's
//! records, and nothing is recomputed per frame that could have been
//! accumulated — the score is worked out once per snapshot, under the lock,
//! alongside everything else the view carries.
//!
//! What this must never do is change the simulation. A table does not deal
//! faster because somebody is watching it, and it does not pay better
//! because it scored well here. Interest is a lens, and Rule 5 is why.

use super::config::Config;
use super::instance::Instance;
use super::roster::Roster;

/// A table's claim on somebody's attention, and the reason for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interest {
    /// Higher is more worth watching. Arbitrary units — only the ordering
    /// means anything, and only within one floor at one moment.
    pub score: i64,
    /// The single biggest contributor, so a screen can say *why* it sent
    /// you here instead of just sending you.
    pub why: Reason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Nothing much; it is just a table.
    Quiet,
    /// Serious money going through it.
    BigMoney,
    /// Somebody just took a lump off it.
    BigSwing,
    /// A full house.
    Crowded,
    /// A VIP in a seat.
    Somebody,
    /// The players are beating it.
    HouseLosing,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Quiet => "ticking over",
            Reason::BigMoney => "big money",
            Reason::BigSwing => "a big hit",
            Reason::Crowded => "packed",
            Reason::Somebody => "somebody worth watching",
            Reason::HouseLosing => "the players are winning",
        }
    }
}

impl Interest {
    pub const QUIET: Interest = Interest { score: 0, why: Reason::Quiet };
}

/// Scores a table on how worth watching it is.
///
/// A paused table scores nothing at all: whatever else it is, it is not
/// action.
pub fn score(t: &Instance, roster: &Roster, cfg: &Config) -> Interest {
    if t.paused || t.history.is_empty() {
        return Interest::QUIET;
    }

    let rounds = t.history.len() as i64;
    let pot: i64 = t.history.iter().map(|r| r.pot).sum::<i64>() / rounds.max(1);
    let swing = t
        .history
        .iter()
        .flat_map(|r| r.seats.iter())
        .map(|s| s.returned - s.staked)
        .max()
        .unwrap_or(0)
        .max(0);
    let house: i64 = t.history.iter().map(|r| r.house()).sum();
    let seats = t.patrons.len() as i64;
    let vips = t
        .patrons
        .iter()
        .filter_map(|id| roster.get(*id))
        .filter(|p| cfg.is_vip(p.tier(cfg)))
        .count() as i64;

    // Each part is scaled against the configured yardstick for it, so the
    // weights compare like with like rather than "chips" against "people".
    let money = pot * cfg.interest_money / cfg.interest_pot_yardstick.max(1);
    let hit = swing * cfg.interest_swing / cfg.big_win.max(1);
    let crowd = seats * cfg.interest_crowd;
    let star = vips * cfg.interest_vip;
    // Only a *losing* house counts. A table grinding players down is the
    // normal state of affairs and not, in itself, a spectacle.
    let against = if house < 0 { (-house) * cfg.interest_upset / cfg.interest_pot_yardstick.max(1) } else { 0 };

    let parts = [
        (money, Reason::BigMoney),
        (hit, Reason::BigSwing),
        (crowd, Reason::Crowded),
        (star, Reason::Somebody),
        (against, Reason::HouseLosing),
    ];
    let score: i64 = parts.iter().map(|(v, _)| *v).sum();
    let why = parts.iter().max_by_key(|(v, _)| *v).map(|(_, r)| *r).unwrap_or(Reason::Quiet);
    // A reason is only worth naming if it is actually carrying the score.
    let why = if score <= 0 { Reason::Quiet } else { why };
    Interest { score, why }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casino::bank::Bank;
    use crate::casino::event::Feed;
    use crate::casino::instance::{Kind, Limit, RoundLog, Seat};
    use crate::casino::sim::Sim;
    use crate::rng::Rng;
    use std::time::Duration;

    struct Bench {
        rng: Rng,
        bank: Bank,
        feed: Feed,
        cfg: Config,
        roster: Roster,
    }

    impl Bench {
        fn new() -> Bench {
            Bench {
                rng: Rng::from_seed(1234),
                bank: Bank::new(1_000_000, 100_000_000),
                feed: Feed::new(64),
                cfg: Config::default(),
                roster: Roster::new(),
            }
        }

        fn sim(&mut self) -> Sim<'_> {
            Sim {
                rng: &mut self.rng,
                bank: &mut self.bank,
                feed: &mut self.feed,
                cfg: &self.cfg,
                roster: &mut self.roster,
                now: Duration::ZERO,
            }
        }

        fn table(&mut self) -> Instance {
            Instance::open_with(1, Kind::Baccarat, 1, Limit::House, &mut self.sim())
        }

        /// Sits `n` people at the table, so the crowd and VIP parts of the
        /// score have something real to read.
        fn seat(&mut self, t: &mut Instance, n: usize, vip: bool) {
            for _ in 0..n {
                let id = self.roster.mint(&mut self.rng);
                let p = self.roster.get_mut(id).unwrap();
                p.begin_visit(1_000);
                if vip {
                    // Turnover is what buys a tier, so give them some.
                    p.lifetime.staked = self.cfg.tiers[self.cfg.vip_tier].1;
                }
                self.roster.seat(id, t.id);
                t.sit(id);
            }
        }
    }

    fn round(number: u64, pot: i64, paid: i64, best_win: i64) -> RoundLog {
        RoundLog {
            number,
            pot,
            paid,
            seats: vec![Seat { name: "probe".into(), bet: "b".into(), staked: 10, returned: 10 + best_win }],
        }
    }

    #[test]
    fn a_table_nothing_has_happened_at_is_not_worth_watching() {
        let mut b = Bench::new();
        let t = b.table();
        assert_eq!(score(&t, &b.roster, &b.cfg), Interest::QUIET);
        assert_eq!(Interest::QUIET.why.label(), "ticking over");
    }

    #[test]
    fn a_paused_table_is_not_action_whatever_else_it_is() {
        let mut b = Bench::new();
        let mut t = b.table();
        t.history.push(round(1, 500_000, 0, 400_000));
        let busy = score(&t, &b.roster, &b.cfg);
        assert!(busy.score > 0);
        t.paused = true;
        assert_eq!(score(&t, &b.roster, &b.cfg), Interest::QUIET, "a paused table scored as action");
    }

    #[test]
    fn bigger_pots_beat_smaller_ones() {
        let mut b = Bench::new();
        let mut quiet = b.table();
        let mut loud = b.table();
        for i in 1..=6 {
            quiet.history.push(round(i, 50, 45, 0));
            loud.history.push(round(i, 50_000, 45_000, 0));
        }
        let (q, l) = (score(&quiet, &b.roster, &b.cfg), score(&loud, &b.roster, &b.cfg));
        assert!(l.score > q.score, "a table with a thousand times the money scored {} against {}", l.score, q.score);
        assert_eq!(l.why, Reason::BigMoney);
    }

    #[test]
    fn one_enormous_hit_beats_a_lot_of_small_ones() {
        let mut b = Bench::new();
        let mut steady = b.table();
        let mut spike = b.table();
        for i in 1..=6 {
            steady.history.push(round(i, 600, 600, 10));
            spike.history.push(round(i, 600, 600, 0));
        }
        spike.history.push(round(7, 600, 600, b.cfg.huge_win * 4));
        let (s, k) = (score(&steady, &b.roster, &b.cfg), score(&spike, &b.roster, &b.cfg));
        assert!(k.score > s.score, "the table somebody just cleaned out scored {} against {}", k.score, s.score);
        assert_eq!(k.why, Reason::BigSwing);
    }

    #[test]
    fn a_crowd_counts_for_something() {
        let mut b = Bench::new();
        let mut empty = b.table();
        let mut full = b.table();
        for i in 1..=4 {
            empty.history.push(round(i, 100, 100, 0));
            full.history.push(round(i, 100, 100, 0));
        }
        b.seat(&mut full, 7, false);
        assert!(score(&full, &b.roster, &b.cfg).score > score(&empty, &b.roster, &b.cfg).score);
    }

    #[test]
    fn a_vip_in_a_seat_is_worth_watching() {
        let mut b = Bench::new();
        let mut ordinary = b.table();
        let mut starry = b.table();
        for i in 1..=4 {
            ordinary.history.push(round(i, 100, 100, 0));
            starry.history.push(round(i, 100, 100, 0));
        }
        b.seat(&mut ordinary, 3, false);
        b.seat(&mut starry, 3, true);
        let (o, s) = (score(&ordinary, &b.roster, &b.cfg), score(&starry, &b.roster, &b.cfg));
        assert!(s.score > o.score, "a table with three VIPs at it scored {} against {}", s.score, o.score);
        assert_eq!(s.why, Reason::Somebody);
    }

    #[test]
    fn the_house_losing_is_a_spectacle_and_the_house_winning_is_not() {
        let mut b = Bench::new();
        let mut winning = b.table();
        let mut losing = b.table();
        for i in 1..=6 {
            // Same money through both; only the direction differs.
            winning.history.push(round(i, 5_000, 1_000, 0));
            losing.history.push(round(i, 5_000, 9_000, 0));
        }
        let (w, l) = (score(&winning, &b.roster, &b.cfg), score(&losing, &b.roster, &b.cfg));
        assert!(l.score > w.score, "the table being beaten scored {} against {}", l.score, w.score);
        assert_eq!(l.why, Reason::HouseLosing);
    }

    #[test]
    fn the_weights_are_configuration_and_actually_do_something() {
        // Rule 7: none of this is written down at the point that uses it.
        let mut b = Bench::new();
        let mut t = b.table();
        for i in 1..=4 {
            t.history.push(round(i, 1_000, 1_000, 0));
        }
        b.seat(&mut t, 5, false);

        let with_crowd = Config { interest_crowd: 1_000, ..Config::default() };
        let without = Config { interest_crowd: 0, ..Config::default() };
        assert!(score(&t, &b.roster, &with_crowd).score > score(&t, &b.roster, &without).score);
        assert_eq!(score(&t, &b.roster, &with_crowd).why, Reason::Crowded);
    }

    #[test]
    fn the_reason_given_is_the_one_actually_carrying_the_score() {
        let mut b = Bench::new();
        let mut t = b.table();
        for i in 1..=6 {
            t.history.push(round(i, 200_000, 200_000, 0));
        }
        let big = score(&t, &b.roster, &b.cfg);
        assert_eq!(big.why, Reason::BigMoney);
        for r in Reason::BigMoney.label().chars() {
            assert!(!r.is_uppercase(), "reasons read as prose, not as labels");
        }
    }

    #[test]
    fn scoring_a_table_never_touches_it() {
        // Interest is a lens. If reading it could change a table, the
        // act of watching would change what you were watching.
        let mut b = Bench::new();
        let mut t = b.table();
        b.seat(&mut t, 3, false);
        for i in 1..=5 {
            t.history.push(round(i, 900, 400, 50));
        }
        let before = (t.round, t.staked, t.returned, t.patrons.clone(), t.history.len());
        for _ in 0..50 {
            let _ = score(&t, &b.roster, &b.cfg);
        }
        assert_eq!((t.round, t.staked, t.returned, t.patrons.clone(), t.history.len()), before);
    }
}
