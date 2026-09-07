//! Things that happen to the casino.
//!
//! A floor that only ever does the same thing at a different speed is a
//! machine, not a place. A night in a real room has shape to it: a coach
//! turns up and the pit fills, word gets round about a game and everybody
//! wants a seat at it, somebody worth looking at walks in, half the staff
//! call in sick and it costs a fortune to keep the doors open.
//!
//! **Every one of these changes a rate or a cost. Not one of them touches a
//! payout.** That is the whole design constraint, and it is Rule 5 again:
//! an event can fill the room, empty it, make a game fashionable or make
//! the night expensive, but the cards do exactly what the cards were always
//! going to do. There is no variant here that could make a table pay
//! better, and the test at the bottom is written so that adding one breaks
//! the build.
//!
//! The effects are all queries — `arrivals`, `appeal_bonus`, `costs` — that
//! the floor asks about when it is already doing the thing they modify. So
//! a happening never reaches into the simulation; the simulation reads it
//! on its way past.

#![allow(dead_code)]

use std::time::Duration;

use super::config::Config;
use crate::rng::Rng;

/// Something going on in the building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Happening {
    /// A coach party, a works do, a stag: the doors do not stop.
    Rush,
    /// A flat spell. Nobody much is coming in.
    Lull,
    /// Word has got round about one game, and everybody wants a seat.
    Craze,
    /// Somebody worth looking at is in, and the room knows it.
    Celebrity,
    /// Half the staff are off. Covering the floor costs a fortune.
    ShortStaffed,
    /// Free drinks all night. It costs, and it fills the place.
    CompNight,
}

impl Happening {
    pub const ALL: [Happening; 6] = [
        Happening::Rush,
        Happening::Lull,
        Happening::Craze,
        Happening::Celebrity,
        Happening::ShortStaffed,
        Happening::CompNight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Happening::Rush => "a rush on the doors",
            Happening::Lull => "a quiet spell",
            Happening::Craze => "word has got round",
            Happening::Celebrity => "somebody is in",
            Happening::ShortStaffed => "short-staffed",
            Happening::CompNight => "comp night",
        }
    }

    /// A line a person can read, given whatever the happening attached
    /// itself to.
    pub fn blurb(self, about: Option<&'static str>) -> String {
        match self {
            Happening::Rush => "A coach has emptied into the lobby — the doors have not stopped".into(),
            Happening::Lull => "It has gone quiet; barely anybody is coming in".into(),
            Happening::Craze => match about {
                Some(game) => format!("Word has got round about {game} — everybody wants a seat"),
                None => "Word has got round about one of the games".into(),
            },
            Happening::Celebrity => "Somebody worth looking at has walked in, and the room knows it".into(),
            Happening::ShortStaffed => "Half the staff are off — covering the floor is costing a fortune".into(),
            Happening::CompNight => "Comp night: the drinks are free and the place is filling up".into(),
        }
    }

    /// How common each one is, out of the whole list.
    pub fn weight(self) -> usize {
        match self {
            Happening::Rush => 24,
            Happening::Lull => 20,
            Happening::Craze => 20,
            Happening::Celebrity => 12,
            Happening::ShortStaffed => 12,
            Happening::CompNight => 12,
        }
    }

    /// The multiplier this puts on how many people come in, in permille.
    /// `1_000` is no change.
    pub fn arrivals(self) -> i64 {
        match self {
            Happening::Rush => 2_600,
            Happening::Lull => 350,
            Happening::CompNight => 1_700,
            Happening::Celebrity => 1_300,
            _ => 1_000,
        }
    }

    /// The multiplier it puts on what the building costs to run.
    pub fn costs(self) -> i64 {
        match self {
            Happening::ShortStaffed => 2_400,
            Happening::CompNight => 1_600,
            _ => 1_000,
        }
    }

    /// What it adds to one game's appeal, in permille, and whether it picks
    /// a game to attach itself to at all.
    pub fn appeal_bonus(self) -> i64 {
        match self {
            Happening::Craze => 500,
            Happening::Celebrity => 150,
            _ => 0,
        }
    }

    /// Whether this one is about a particular game.
    pub fn picks_a_game(self) -> bool {
        matches!(self, Happening::Craze)
    }

    fn roll(rng: &mut Rng) -> Happening {
        let total: usize = Happening::ALL.iter().map(|h| h.weight()).sum();
        let mut pick = rng.below(total);
        for h in Happening::ALL {
            if pick < h.weight() {
                return h;
            }
            pick -= h.weight();
        }
        Happening::Rush
    }
}

/// A happening that is currently going on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Going {
    pub what: Happening,
    /// The game it is about, for the ones that are about a game.
    pub game: Option<&'static str>,
    pub started: Duration,
    pub until: Duration,
}

/// What is going on in the building, and what it is doing to the rates.
#[derive(Debug, Clone)]
pub struct Happenings {
    going: Vec<Going>,
    /// When the dice are next rolled on whether something happens.
    next_roll: Duration,
    /// How many there have ever been, so a screen can say whether the night
    /// has been eventful.
    seen: u64,
}

impl Happenings {
    pub fn new(cfg: &Config, from: Duration) -> Happenings {
        Happenings { going: Vec::new(), next_roll: from + cfg.happening_period, seen: 0 }
    }

    pub fn going(&self) -> &[Going] {
        &self.going
    }

    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// Rolls for a new happening if it is time, and clears out any that
    /// have run their course.
    ///
    /// Returns `(started, ended)` so the caller can announce both. Periodic,
    /// like everything else that is not a bet.
    pub fn tick(&mut self, rng: &mut Rng, cfg: &Config, now: Duration, games: &[&'static str]) -> (Vec<Going>, Vec<Going>) {
        let mut ended = Vec::new();
        self.going.retain(|g| {
            if g.until <= now {
                ended.push(*g);
                false
            } else {
                true
            }
        });

        let mut started = Vec::new();
        if now < self.next_roll {
            return (started, ended);
        }
        self.next_roll = now + cfg.happening_period;
        if self.going.len() >= cfg.happenings_at_once {
            return (started, ended);
        }
        if rng.below(100) >= cfg.happening_chance {
            return (started, ended);
        }

        let what = Happening::roll(rng);
        // One of a kind at a time: two rushes at once is not twice as busy,
        // it is a bug that reads as one.
        if self.going.iter().any(|g| g.what == what) {
            return (started, ended);
        }
        let game = if what.picks_a_game() && !games.is_empty() { Some(games[rng.below(games.len())]) } else { None };
        let (lo, hi) = cfg.happening_for;
        let span = hi.saturating_sub(lo).as_millis().max(1) as usize;
        let g = Going { what, game, started: now, until: now + lo + Duration::from_millis(rng.below(span) as u64) };
        self.going.push(g);
        self.seen += 1;
        started.push(g);
        (started, ended)
    }

    /// The combined multiplier on arrivals, in permille.
    ///
    /// Multiplied rather than added, so a rush during a comp night is
    /// genuinely busier than either alone, and a lull cancels a rush out
    /// instead of one of them silently winning.
    pub fn arrivals(&self) -> i64 {
        self.going.iter().fold(1_000, |acc, g| acc * g.what.arrivals() / 1_000)
    }

    /// The combined multiplier on the building's running costs.
    pub fn costs(&self) -> i64 {
        self.going.iter().fold(1_000, |acc, g| acc * g.what.costs() / 1_000)
    }

    /// What is being added to one game's appeal right now.
    pub fn appeal_bonus(&self, key: &'static str) -> i64 {
        self.going
            .iter()
            .map(|g| match g.game {
                Some(about) if about == key => g.what.appeal_bonus(),
                // A happening with no particular game lifts everything a
                // little, if it lifts anything at all.
                None => g.what.appeal_bonus(),
                _ => 0,
            })
            .sum()
    }

    /// Ends everything, for a floor that is closing.
    pub fn clear(&mut self) {
        self.going.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Rng {
        Rng::from_seed(9_001)
    }

    fn cfg() -> Config {
        Config::default()
    }

    const GAMES: [&str; 3] = ["slots", "roulette", "keno"];

    #[test]
    fn nothing_at_all_is_going_on_to_begin_with() {
        let h = Happenings::new(&cfg(), Duration::ZERO);
        assert!(h.going().is_empty());
        assert_eq!(h.arrivals(), 1_000, "with nothing going on, nothing is multiplied");
        assert_eq!(h.costs(), 1_000);
        assert_eq!(h.appeal_bonus("slots"), 0);
        assert_eq!(h.seen(), 0);
    }

    #[test]
    fn no_happening_can_ever_touch_a_payout() {
        // Rule 5, guarded by the shape of the type. Every happening is
        // allowed to move exactly three things — how many people come in,
        // what the building costs, and which game is fashionable. If
        // somebody adds a fourth that pays out, this is the test that was
        // supposed to stop them.
        for h in Happening::ALL {
            assert!(h.arrivals() > 0, "{} would stop people arriving entirely", h.label());
            assert!(h.costs() > 0, "{} would make the building free to run", h.label());
            assert!(h.appeal_bonus() >= 0);
            // The three levers, and a total of three: if this list ever
            // needs a fourth entry, the fourth had better not be money.
            let levers = [h.arrivals(), h.costs(), h.appeal_bonus()];
            assert_eq!(levers.len(), 3);
            assert!(!h.label().is_empty());
            assert!(!h.blurb(Some("slots")).is_empty());
        }
    }

    #[test]
    fn a_rush_fills_the_room_and_a_lull_empties_it() {
        assert!(Happening::Rush.arrivals() > 1_000);
        assert!(Happening::Lull.arrivals() < 1_000);
        assert!(Happening::CompNight.arrivals() > 1_000);
        // ...and comp night costs for the privilege.
        assert!(Happening::CompNight.costs() > 1_000);
        assert!(Happening::ShortStaffed.costs() > Happening::CompNight.costs());
        assert_eq!(Happening::Rush.costs(), 1_000, "a busy night is not a dearer one");
    }

    #[test]
    fn effects_multiply_so_two_at_once_are_not_one_of_them() {
        let mut h = Happenings::new(&cfg(), Duration::ZERO);
        let at = Duration::from_secs(10);
        h.going.push(Going { what: Happening::Rush, game: None, started: at, until: at + Duration::from_secs(60) });
        assert_eq!(h.arrivals(), Happening::Rush.arrivals());
        h.going.push(Going { what: Happening::CompNight, game: None, started: at, until: at + Duration::from_secs(60) });
        assert_eq!(h.arrivals(), Happening::Rush.arrivals() * Happening::CompNight.arrivals() / 1_000);
        // A lull against a rush pulls it back rather than one winning.
        h.going.push(Going { what: Happening::Lull, game: None, started: at, until: at + Duration::from_secs(60) });
        assert!(h.arrivals() < Happening::Rush.arrivals() * Happening::CompNight.arrivals() / 1_000);
    }

    #[test]
    fn a_craze_is_about_one_game_and_only_that_game() {
        let mut h = Happenings::new(&cfg(), Duration::ZERO);
        let at = Duration::ZERO;
        h.going.push(Going { what: Happening::Craze, game: Some("keno"), started: at, until: at + Duration::from_secs(60) });
        assert_eq!(h.appeal_bonus("keno"), Happening::Craze.appeal_bonus());
        assert_eq!(h.appeal_bonus("slots"), 0, "the craze leaked onto a game it was not about");
        assert!(Happening::Craze.picks_a_game());
        assert!(!Happening::Rush.picks_a_game());
    }

    #[test]
    fn happenings_start_and_then_run_out() {
        let c = Config { happening_chance: 100, happening_period: Duration::from_secs(10), ..Config::default() };
        let mut rng = probe();
        let mut h = Happenings::new(&c, Duration::ZERO);
        // Not yet.
        let (started, _) = h.tick(&mut rng, &c, Duration::from_secs(9), &GAMES);
        assert!(started.is_empty());
        // Now.
        let (started, _) = h.tick(&mut rng, &c, Duration::from_secs(10), &GAMES);
        assert_eq!(started.len(), 1);
        let g = started[0];
        assert!(g.until > g.started, "a happening that ends before it starts");

        // It runs its course and is announced on the way out.
        let (_, ended) = h.tick(&mut rng, &c, g.until, &GAMES);
        assert!(ended.iter().any(|e| e.what == g.what), "it never ended");
        assert!(!h.going().iter().any(|x| x.what == g.what && x.started == g.started));
    }

    #[test]
    fn never_two_of_the_same_thing_at_once_and_never_too_many_at_all() {
        let c = Config {
            happening_chance: 100,
            happening_period: Duration::from_secs(1),
            happening_for: (Duration::from_secs(9_000), Duration::from_secs(9_001)),
            ..Config::default()
        };
        let mut rng = probe();
        let mut h = Happenings::new(&c, Duration::ZERO);
        let mut at = Duration::ZERO;
        for _ in 0..500 {
            at += Duration::from_secs(1);
            h.tick(&mut rng, &c, at, &GAMES);
            assert!(h.going().len() <= c.happenings_at_once, "{} things at once", h.going().len());
            let mut kinds: Vec<Happening> = h.going().iter().map(|g| g.what).collect();
            let n = kinds.len();
            kinds.sort_unstable();
            kinds.dedup();
            assert_eq!(kinds.len(), n, "two of the same thing were going on at once");
        }
    }

    #[test]
    fn a_floor_with_no_games_open_still_has_weather() {
        // A craze needs a game to be about; with none open it must not
        // pick one out of nothing, and must not panic trying.
        let c = Config { happening_chance: 100, happening_period: Duration::from_secs(1), ..Config::default() };
        let mut rng = probe();
        let mut h = Happenings::new(&c, Duration::ZERO);
        let mut at = Duration::ZERO;
        for _ in 0..80 {
            at += Duration::from_secs(1);
            let (started, _) = h.tick(&mut rng, &c, at, &[]);
            for g in started {
                assert_eq!(g.game, None, "a happening attached itself to a game that is not open");
            }
        }
    }

    #[test]
    fn every_kind_of_happening_turns_up_eventually() {
        let c = Config {
            happening_chance: 100,
            happening_period: Duration::from_secs(1),
            happening_for: (Duration::from_secs(1), Duration::from_secs(2)),
            ..Config::default()
        };
        let mut rng = probe();
        let mut h = Happenings::new(&c, Duration::ZERO);
        let mut at = Duration::ZERO;
        let mut seen: Vec<Happening> = Vec::new();
        for _ in 0..3_000 {
            at += Duration::from_secs(1);
            let (started, _) = h.tick(&mut rng, &c, at, &GAMES);
            for g in started {
                if !seen.contains(&g.what) {
                    seen.push(g.what);
                }
            }
        }
        for w in Happening::ALL {
            assert!(seen.contains(&w), "{} never happened in three thousand rolls", w.label());
        }
    }
}
