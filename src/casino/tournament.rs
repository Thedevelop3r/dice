//! A tournament running itself in the corner of the room.
//!
//! Everything else on this floor is a cash game: you sit down with chips,
//! you get up with whatever you have left, and nothing connects one seat to
//! another. A tournament is the opposite shape — a fixed field, everybody
//! starting level, nobody leaving until they are knocked out, and the whole
//! thing narrowing to one person.
//!
//! The structure is a bracket by *survival*, not by pairings. Every
//! survivor plays the same number of hands each round, at the same stake;
//! whoever is short at the end of the round is out. The field halves each
//! time — 128, 64, 32, 16, 8 — until a final table, and then plays down to
//! a winner. That gives a real arc without pretending to model heads-up
//! play the game engine has never heard of.
//!
//! Two things it must not do, both learned the hard way elsewhere in this
//! module:
//!
//! - **It must not bend an outcome.** Every hand is resolved by the same
//!   `Kind::resolve` a cash table uses. A short stack does not get luckier
//!   because it would be a better story; it just goes out. Rule 5.
//! - **It must not invent or destroy money.** Buy-ins come out of patrons'
//!   own chips and go into a prize pool; the house takes a stated rake off
//!   the top; the pool is paid back out in full. Tournament chips are a
//!   separate scoring currency that never touches the tray, which is what
//!   keeps the conservation invariant true.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use super::config::Config;
use super::instance::Kind;
use super::roster::Roster;
use crate::rng::Rng;

/// Where a tournament has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Taking entries; not started.
    Registering,
    /// Playing down. The number is how many are left.
    Running(usize),
    /// Down to the last few.
    FinalTable,
    /// Finished, with a winner.
    Done,
}

impl Stage {
    pub fn label(self) -> String {
        match self {
            Stage::Registering => "taking entries".into(),
            Stage::Running(n) => format!("{n} left"),
            Stage::FinalTable => "the final table".into(),
            Stage::Done => "finished".into(),
        }
    }
}

/// One player's standing in a tournament.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entrant {
    pub patron: u64,
    pub name: String,
    /// Tournament chips. A score, not money — see the module docs.
    pub stack: i64,
    /// The round they went out in; `None` while they are still alive.
    pub out_in: Option<usize>,
}

/// What somebody won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payday {
    pub patron: u64,
    pub name: String,
    pub place: usize,
    pub prize: i64,
}

/// A tournament, from registration to a winner.
#[derive(Debug, Clone)]
pub struct Tournament {
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    pub stage: Stage,
    pub round: usize,
    /// Everybody who ever entered, in entry order.
    pub field: Vec<Entrant>,
    /// The prize pool, in real chips, after the house's rake.
    pub pool: i64,
    pub rake: i64,
    pub buy_in: i64,
    /// Paid out once it is over, best first.
    pub paid: Vec<Payday>,
    /// When the next round is played.
    next_at: Duration,
    started: Duration,
    /// When it finished, so a screen can keep showing it for a while
    /// afterwards. Measured from the *end*, not the start — a tournament
    /// that took an hour to play down should still be readable for a
    /// minute after somebody wins it.
    finished: Option<Duration>,
}

impl Tournament {
    /// Opens registration. Nobody is in it yet.
    pub fn new(id: u32, number: u32, kind: Kind, cfg: &Config, now: Duration) -> Tournament {
        Tournament {
            id,
            name: format!("{} Tournament #{number}", kind.label()),
            kind,
            stage: Stage::Registering,
            round: 0,
            field: Vec::new(),
            pool: 0,
            rake: 0,
            buy_in: cfg.tourney_buy_in,
            paid: Vec::new(),
            next_at: now + cfg.tourney_round_every,
            started: now,
            finished: None,
        }
    }

    pub fn alive(&self) -> usize {
        self.field.iter().filter(|e| e.out_in.is_none()).count()
    }

    pub fn entered(&self) -> usize {
        self.field.len()
    }

    pub fn running_for(&self, now: Duration) -> Duration {
        now.saturating_sub(self.started)
    }

    /// Takes somebody's entry. Their buy-in leaves their own stack and goes
    /// into the pool, less the house's rake — which is the only money the
    /// house makes here, and is taken once, at the door.
    ///
    /// Returns the chips the house raked, for the caller to book.
    pub fn enter(&mut self, patron: u64, name: String, chips: i64, cfg: &Config) -> Option<i64> {
        if self.stage != Stage::Registering || self.field.iter().any(|e| e.patron == patron) {
            return None;
        }
        if chips < self.buy_in {
            return None;
        }
        let rake = self.buy_in * cfg.tourney_rake / 100;
        self.pool += self.buy_in - rake;
        self.rake += rake;
        self.field.push(Entrant { patron, name, stack: cfg.tourney_stack, out_in: None });
        Some(rake)
    }

    /// Starts play, if enough people turned up. Returns whether it did.
    pub fn begin(&mut self, cfg: &Config) -> bool {
        if self.stage != Stage::Registering || self.field.len() < cfg.tourney_min_field {
            return false;
        }
        self.stage = Stage::Running(self.field.len());
        self.round = 1;
        true
    }

    /// Is a round due?
    pub fn due(&self, now: Duration) -> bool {
        matches!(self.stage, Stage::Running(_) | Stage::FinalTable) && now >= self.next_at
    }

    /// Plays one round: every survivor plays the same hands at the same
    /// stake, then the short stacks go out.
    ///
    /// Returns the entrants knocked out, worst-placed first.
    pub fn play_round(&mut self, rng: &mut Rng, cfg: &Config, now: Duration) -> Vec<Entrant> {
        self.next_at = now + cfg.tourney_round_every;
        let variants = self.kind.variants();
        let ante = cfg.tourney_ante.max(1);

        for e in self.field.iter_mut().filter(|e| e.out_in.is_none()) {
            for _ in 0..cfg.tourney_hands {
                if e.stack < ante {
                    break;
                }
                // The same maths a cash table uses, at a level stake. No
                // thumb on the scale for a short stack, however good a
                // story it would make.
                let pick = rng.below(variants);
                let (unit_staked, unit_returned) = self.kind.resolve(rng, pick);
                let back = if unit_staked > 0 { (unit_returned * ante + unit_staked / 2) / unit_staked } else { 0 };
                e.stack += back - ante;
            }
        }

        // Anybody with nothing left is out regardless; beyond that, the
        // field halves, shortest stacks first, until the final table.
        let alive = self.alive();
        let target = if alive <= cfg.tourney_final_table { 1.max(alive.saturating_sub(1)) } else { alive.div_ceil(2) };

        let mut standing: Vec<(i64, u64)> =
            self.field.iter().filter(|e| e.out_in.is_none()).map(|e| (e.stack, e.patron)).collect();
        // Shortest first; ties broken by id so a round is deterministic
        // given the same rolls.
        standing.sort_unstable();
        let cut = standing.len().saturating_sub(target);
        let doomed: Vec<u64> = standing
            .iter()
            .enumerate()
            .filter(|(i, (stack, _))| *i < cut || *stack <= 0)
            .map(|(_, (_, id))| *id)
            .collect();

        let round = self.round;
        let mut out = Vec::new();
        for e in self.field.iter_mut() {
            if e.out_in.is_none() && doomed.contains(&e.patron) {
                e.out_in = Some(round);
                out.push(e.clone());
            }
        }
        // Worst-placed first: the shortest stack of the round went out
        // lowest, which is the order `standing` is already in.
        out.sort_by_key(|e| e.stack);

        self.round += 1;
        let left = self.alive();
        if left <= 1 {
            self.finished = Some(now);
        }
        self.stage = if left <= 1 {
            Stage::Done
        } else if left <= cfg.tourney_final_table {
            Stage::FinalTable
        } else {
            Stage::Running(left)
        };
        out
    }

    /// Works out who gets what. Called once, when it is over.
    ///
    /// The pool is paid out **in full**: the last place paid absorbs any
    /// rounding, so what goes out is exactly what went in. A tournament
    /// that quietly kept a chip would be a tournament that invents money.
    pub fn settle(&mut self, cfg: &Config) -> &[Payday] {
        if self.stage != Stage::Done || !self.paid.is_empty() {
            return &self.paid;
        }
        // Finishing order: still standing first, then by how late they went
        // out, then by the stack they went out with.
        let mut order: Vec<&Entrant> = self.field.iter().collect();
        order.sort_by(|a, b| {
            let rank = |e: &Entrant| e.out_in.unwrap_or(usize::MAX);
            rank(b).cmp(&rank(a)).then(b.stack.cmp(&a.stack)).then(a.patron.cmp(&b.patron))
        });

        let places = cfg.tourney_prizes.len().min(order.len());
        let mut left = self.pool;
        let mut paid = Vec::new();
        for (i, e) in order.iter().take(places).enumerate() {
            let share = if i + 1 == places {
                // The last place paid takes whatever is left, so nothing
                // is lost to integer division.
                left
            } else {
                self.pool * cfg.tourney_prizes[i] / 100
            };
            left -= share;
            paid.push(Payday { patron: e.patron, name: e.name.clone(), place: i + 1, prize: share });
        }
        self.paid = paid;
        &self.paid
    }

    /// Calls it off, because not enough people turned up.
    ///
    /// Returns everybody who had entered and the total the house raked, so
    /// the caller can hand back every chip and un-take its cut. A
    /// tournament that does not run must cost nobody anything.
    pub fn abandon(&mut self) -> (Vec<Entrant>, i64, i64) {
        let entrants = std::mem::take(&mut self.field);
        let (buy_in, rake) = (self.buy_in, self.rake);
        self.pool = 0;
        self.rake = 0;
        self.stage = Stage::Done;
        self.finished = Some(self.started);
        (entrants, buy_in, rake)
    }

    /// How long it has been over, if it is over.
    pub fn over_for(&self, now: Duration) -> Option<Duration> {
        self.finished.map(|at| now.saturating_sub(at))
    }

    /// The standings, best first — for a screen to draw.
    pub fn standings(&self, n: usize) -> Vec<Entrant> {
        let mut alive: Vec<Entrant> = self.field.iter().filter(|e| e.out_in.is_none()).cloned().collect();
        alive.sort_by(|a, b| b.stack.cmp(&a.stack).then(a.patron.cmp(&b.patron)));
        alive.truncate(n);
        alive
    }

    /// The winner, once there is one.
    pub fn winner(&self) -> Option<&Entrant> {
        if self.stage != Stage::Done {
            return None;
        }
        self.field.iter().find(|e| e.out_in.is_none())
    }
}

/// Signs somebody up if they can afford it and fancy it.
///
/// A tournament is a commitment: you cannot get up when you feel like it.
/// So the people who enter are the ones with the patience for it — high
/// discipline, or enough of a stack that the buy-in is nothing.
pub fn would_enter(roster: &Roster, patron: u64, buy_in: i64, rng: &mut Rng) -> bool {
    let Some(p) = roster.get(patron) else { return false };
    if p.chips < buy_in * 2 {
        return false;
    }
    let keen = 20 + p.discipline as usize / 3;
    rng.below(100) < keen
}

/// The tournaments a floor has going, keyed by id.
pub type Running = BTreeMap<u32, Tournament>;

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Rng {
        Rng::from_seed(2_026)
    }

    fn cfg() -> Config {
        Config::default()
    }

    /// A tournament with `n` entrants already signed up.
    fn with_field(n: usize, c: &Config) -> Tournament {
        let mut t = Tournament::new(1, 1, Kind::Blackjack, c, Duration::ZERO);
        for i in 0..n {
            t.enter(i as u64 + 1, format!("Player {i}"), 1_000_000, c);
        }
        t
    }

    #[test]
    fn a_new_tournament_is_taking_entries_and_has_nobody_in_it() {
        let c = cfg();
        let t = Tournament::new(1, 3, Kind::Blackjack, &c, Duration::ZERO);
        assert_eq!(t.stage, Stage::Registering);
        assert_eq!(t.entered(), 0);
        assert_eq!(t.alive(), 0);
        assert_eq!(t.pool, 0);
        assert!(t.name.contains("#3"));
        assert!(t.winner().is_none());
    }

    #[test]
    fn the_pool_is_the_buy_ins_less_the_rake_and_the_rake_is_the_houses_only_cut() {
        let c = cfg();
        let mut t = with_field(10, &c);
        let expected_rake = c.tourney_buy_in * c.tourney_rake / 100;
        assert_eq!(t.rake, expected_rake * 10);
        assert_eq!(t.pool, (c.tourney_buy_in - expected_rake) * 10);
        assert_eq!(t.pool + t.rake, c.tourney_buy_in * 10, "money appeared or vanished at the door");
        // And it is taken once, at entry: playing does not rake again.
        let mut rng = probe();
        t.begin(&c);
        let before = t.rake;
        t.play_round(&mut rng, &c, Duration::from_secs(60));
        assert_eq!(t.rake, before, "the house raked a second time");
    }

    #[test]
    fn nobody_gets_in_twice_or_without_the_money() {
        let c = cfg();
        let mut t = Tournament::new(1, 1, Kind::Blackjack, &c, Duration::ZERO);
        assert!(t.enter(7, "Ada".into(), 1_000_000, &c).is_some());
        assert!(t.enter(7, "Ada".into(), 1_000_000, &c).is_none(), "the same person entered twice");
        assert!(t.enter(8, "Bruno".into(), c.tourney_buy_in - 1, &c).is_none(), "somebody entered they could not afford");
        assert_eq!(t.entered(), 1);
    }

    #[test]
    fn it_will_not_start_without_a_field() {
        let c = cfg();
        let mut thin = with_field(c.tourney_min_field - 1, &c);
        assert!(!thin.begin(&c));
        assert_eq!(thin.stage, Stage::Registering);
        let mut proper = with_field(c.tourney_min_field, &c);
        assert!(proper.begin(&c));
        assert!(matches!(proper.stage, Stage::Running(_)));
        assert!(!proper.begin(&c), "it started twice");
    }

    #[test]
    fn everybody_starts_level() {
        let c = cfg();
        let t = with_field(32, &c);
        assert!(t.field.iter().all(|e| e.stack == c.tourney_stack), "somebody started with a different stack");
    }

    #[test]
    fn the_field_halves_until_a_final_table_and_then_plays_down_to_one() {
        let c = cfg();
        let mut rng = probe();
        let mut t = with_field(128, &c);
        assert!(t.begin(&c));
        let mut at = Duration::ZERO;
        let mut sizes = vec![t.alive()];
        for _ in 0..40 {
            if t.stage == Stage::Done {
                break;
            }
            at += c.tourney_round_every;
            t.play_round(&mut rng, &c, at);
            sizes.push(t.alive());
        }
        assert_eq!(t.stage, Stage::Done, "it never finished: {sizes:?}");
        assert_eq!(t.alive(), 1, "it finished with {} people still in", t.alive());
        // It only ever gets smaller, and it halves while the field is big.
        for pair in sizes.windows(2) {
            assert!(pair[1] <= pair[0], "the field grew: {sizes:?}");
        }
        assert!(sizes.contains(&64) && sizes.contains(&32), "the field did not halve: {sizes:?}");
        assert!(t.winner().is_some());
    }

    #[test]
    fn a_small_field_still_finishes() {
        let c = cfg();
        let mut rng = probe();
        let mut t = with_field(c.tourney_min_field, &c);
        t.begin(&c);
        let mut at = Duration::ZERO;
        for _ in 0..50 {
            if t.stage == Stage::Done {
                break;
            }
            at += c.tourney_round_every;
            t.play_round(&mut rng, &c, at);
        }
        assert_eq!(t.stage, Stage::Done);
        assert_eq!(t.alive(), 1);
    }

    #[test]
    fn the_whole_pool_is_paid_out_and_not_a_chip_more_or_less() {
        // The property that stops a tournament being a hole in the books.
        let c = cfg();
        let mut rng = probe();
        for field in [7usize, 13, 40, 128] {
            let mut t = with_field(field, &c);
            t.begin(&c);
            let mut at = Duration::ZERO;
            while t.stage != Stage::Done {
                at += c.tourney_round_every;
                t.play_round(&mut rng, &c, at);
            }
            let pool = t.pool;
            let paid: i64 = t.settle(&c).iter().map(|p| p.prize).sum();
            assert_eq!(paid, pool, "a field of {field} paid out {paid} from a pool of {pool}");
            assert!(t.paid.iter().all(|p| p.prize >= 0), "somebody was paid a negative prize");
        }
    }

    #[test]
    fn the_winner_is_paid_first_and_best() {
        let c = cfg();
        let mut rng = probe();
        let mut t = with_field(64, &c);
        t.begin(&c);
        let mut at = Duration::ZERO;
        while t.stage != Stage::Done {
            at += c.tourney_round_every;
            t.play_round(&mut rng, &c, at);
        }
        let winner = t.winner().expect("somebody won").patron;
        let paid = t.settle(&c).to_vec();
        assert_eq!(paid[0].patron, winner, "the winner is not first in the money");
        assert_eq!(paid[0].place, 1);
        for pair in paid.windows(2) {
            assert!(pair[0].prize >= pair[1].prize, "a lower place was paid more");
        }
        // Settling twice does not pay twice.
        let again: i64 = t.settle(&c).iter().map(|p| p.prize).sum();
        assert_eq!(again, t.pool);
    }

    #[test]
    fn nobody_is_carried_and_nobody_is_dropped() {
        // Every entrant ends the tournament either the winner or knocked
        // out in a numbered round. Nobody just disappears.
        let c = cfg();
        let mut rng = probe();
        let mut t = with_field(50, &c);
        t.begin(&c);
        let mut at = Duration::ZERO;
        let mut knocked = 0;
        while t.stage != Stage::Done {
            at += c.tourney_round_every;
            knocked += t.play_round(&mut rng, &c, at).len();
        }
        assert_eq!(knocked, 49, "{knocked} of 50 were knocked out");
        assert_eq!(t.field.len(), 50, "the field changed size");
        assert!(t.field.iter().filter(|e| e.out_in.is_none()).count() == 1);
    }

    #[test]
    fn a_round_is_played_by_the_games_own_maths() {
        // Rule 5. The stacks after a round must be the result of real hands
        // — so with a house edge in the game, the total tournament chips in
        // play must go *down*, not stay level and not go up.
        let c = cfg();
        let mut rng = probe();
        let mut t = with_field(64, &c);
        t.begin(&c);
        let before: i64 = t.field.iter().map(|e| e.stack).sum();
        t.play_round(&mut rng, &c, c.tourney_round_every);
        let after: i64 = t.field.iter().map(|e| e.stack).sum();
        assert_ne!(after, before, "a round of play changed nothing, so no hands were dealt");
        assert!(after < before, "the field came out ahead of the house's own maths");
    }

    #[test]
    fn a_tournament_that_does_not_run_costs_nobody_anything() {
        let c = cfg();
        let mut t = with_field(3, &c);
        assert!(!t.begin(&c), "it ran on a field of three");
        let pool_and_rake = t.pool + t.rake;
        let (entrants, buy_in, rake) = t.abandon();
        assert_eq!(entrants.len(), 3);
        assert_eq!(buy_in, c.tourney_buy_in);
        assert_eq!(buy_in * 3, pool_and_rake, "the entries do not add up to what was taken");
        assert_eq!(rake * 3 / 3, rake);
        assert_eq!(t.pool, 0);
        assert_eq!(t.rake, 0, "the house kept a cut of a tournament that never happened");
        assert_eq!(t.stage, Stage::Done);
    }

    #[test]
    fn a_finished_tournament_knows_when_it_finished_and_not_when_it_started() {
        // The retention window is measured from the end. Measured from the
        // start, a tournament that took a long time to play down would be
        // forgotten the instant somebody won it — which is exactly the
        // moment anybody would want to look at it.
        let c = cfg();
        let mut rng = probe();
        let mut t = with_field(32, &c);
        t.begin(&c);
        assert!(t.over_for(Duration::from_secs(1)).is_none(), "it is not over yet");
        let mut at = Duration::ZERO;
        while t.stage != Stage::Done {
            at += c.tourney_round_every;
            t.play_round(&mut rng, &c, at);
        }
        assert!(at > c.tourney_round_every * 3, "the test needs a tournament that took a while");
        assert_eq!(t.over_for(at), Some(Duration::ZERO), "it should have just this moment finished");
        assert_eq!(t.over_for(at + Duration::from_secs(30)), Some(Duration::from_secs(30)));
    }

    #[test]
    fn standings_read_best_first() {
        let c = cfg();
        let mut rng = probe();
        let mut t = with_field(30, &c);
        t.begin(&c);
        t.play_round(&mut rng, &c, c.tourney_round_every);
        let top = t.standings(5);
        assert!(top.len() <= 5);
        for pair in top.windows(2) {
            assert!(pair[0].stack >= pair[1].stack, "the standings are not in order");
        }
        assert!(top.iter().all(|e| e.out_in.is_none()), "somebody knocked out is in the standings");
    }

    #[test]
    fn a_round_is_only_due_when_it_is_due_and_never_once_it_is_over() {
        let c = cfg();
        let mut rng = probe();
        let mut t = with_field(8, &c);
        assert!(!t.due(Duration::from_secs(9_999)), "a tournament that has not started is not due a round");
        t.begin(&c);
        assert!(!t.due(Duration::ZERO));
        assert!(t.due(c.tourney_round_every));
        let mut at = Duration::ZERO;
        while t.stage != Stage::Done {
            at += c.tourney_round_every;
            t.play_round(&mut rng, &c, at);
        }
        assert!(!t.due(at + Duration::from_secs(9_999)), "a finished tournament wanted another round");
    }
}
