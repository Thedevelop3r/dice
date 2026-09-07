//! The simulated players who fill the background tables.
//!
//! A patron is not a coin flip with a name on it. Four traits, rolled
//! independently at the door, decide how they actually behave:
//!
//! - **nerve** — how much of their stack goes on a bet.
//! - **appetite** — how far up the risk ladder they reach when a table
//!   offers a choice of bets. This is the trait that changes *which real
//!   bet* they place, so it changes their whole return profile.
//! - **discipline** — whether they walk away ahead, or keep going.
//! - **read** — how well they judge a table. A sharp patron leans toward the
//!   better-priced bets available; a poor one is drawn to the long shots.
//!
//! What no trait does is bend an outcome. Every round a patron plays is
//! resolved by the table's own audited maths, exactly as if a person were
//! sitting there. `luck` is therefore a *result*, not an input: it reports
//! how a patron has actually run against what their staking says they should
//! have. Letting a "lucky" patron win more often would quietly undo the
//! whole point of `games::audit`.

use super::event::Departure;
use crate::rng::Rng;

const FIRST: [&str; 32] = [
    "Ada", "Bruno", "Cleo", "Dot", "Emory", "Faye", "Gus", "Hattie", "Idris", "June", "Kit", "Lorne", "Mabel", "Nero", "Ozzie", "Prue", "Quill",
    "Rosa", "Silas", "Tess", "Ulla", "Vane", "Wren", "Xanth", "Yuki", "Zeb", "Marlow", "Odile", "Piet", "Rue", "Sable", "Thea",
];
const LAST: [&str; 24] = [
    "Ashcroft", "Bellweather", "Crane", "Doyle", "Eastwick", "Fairbairn", "Glass", "Halloran", "Ives", "Jarrow", "Keel", "Larkin", "Mordaunt",
    "Nightingale", "Orsini", "Peel", "Quarles", "Rooke", "Strand", "Thorne", "Underhill", "Vance", "Wexford", "Yarrow",
];

#[derive(Debug, Clone)]
pub struct Patron {
    /// Unique on the floor, for the whole life of the casino. Minted by
    /// the floor rather than by a table, so a patron keeps one identity
    /// wherever they sit and whatever they are doing.
    pub id: u64,
    pub name: String,
    /// Chips in front of them right now.
    pub chips: i64,
    /// What they bought in for, in chips — the baseline `luck` measures
    /// against.
    pub bought: i64,

    /// 0-100 each. See the module docs for what they actually do.
    pub nerve: u8,
    pub appetite: u8,
    pub discipline: u8,
    pub read: u8,

    pub rounds: u32,
    pub staked: i64,
    pub returned: i64,
    pub biggest_win: i64,
}

impl Patron {
    pub fn new(rng: &mut Rng, chips: i64, id: u64) -> Patron {
        // Traits are rolled as the average of two draws, which clusters them
        // toward the middle: most patrons are unremarkable and the extremes
        // are rare, which is what makes the rare ones worth watching.
        let trait_roll = |rng: &mut Rng| ((rng.below(101) + rng.below(101)) / 2) as u8;
        Patron {
            id,
            name: format!("{} {}", FIRST[rng.below(FIRST.len())], LAST[rng.below(LAST.len())]),
            chips,
            bought: chips,
            nerve: trait_roll(rng),
            appetite: trait_roll(rng),
            discipline: trait_roll(rng),
            read: trait_roll(rng),
            rounds: 0,
            staked: 0,
            returned: 0,
            biggest_win: 0,
        }
    }

    /// A one-word read on the patron, for the table view.
    pub fn style(&self) -> &'static str {
        match (self.nerve >= 60, self.appetite >= 60, self.discipline >= 60) {
            (true, true, _) => "reckless",
            (true, false, true) => "bold",
            (true, false, false) => "heavy",
            (false, true, _) => "chancer",
            (false, false, true) => "careful",
            (false, false, false) => "grinder",
        }
    }

    /// What they put on the next bet. Nerve sets the share of the stack;
    /// a patron who is down chases a little, and a disciplined one chases
    /// less — but nobody bets more than they are holding.
    pub fn stake(&self, rng: &mut Rng, min: i64) -> i64 {
        if self.chips < min {
            return 0;
        }
        // Between a fortieth and a fifth of the stack, by nerve.
        let share = 40 - (self.nerve as i64 * 35 / 100);
        let mut bet = (self.chips / share.max(5)).max(min);
        if self.chips < self.bought {
            // Down on the visit: chase, scaled by how little discipline
            // they brought with them.
            let chase = 100 + (100 - self.discipline as i64) / 2;
            bet = bet * chase / 100;
        }
        // A little jitter so a table is not a metronome.
        let jitter = 90 + rng.below(21) as i64;
        (bet * jitter / 100).clamp(min, self.chips)
    }

    /// Which of a table's bets they take, where index 0 is the shortest
    /// price and the last is the longest shot. Appetite reaches up the
    /// ladder; a good read pulls back down it, because the long shots are
    /// where the house edge is thickest.
    pub fn pick_bet(&self, rng: &mut Rng, variants: usize) -> usize {
        if variants <= 1 {
            return 0;
        }
        let reach = self.appetite as i64 - (self.read as i64 / 3);
        let reach = reach.clamp(0, 100) as usize;
        // Map appetite onto the ladder, then wobble by one either way.
        let centre = reach * (variants - 1) / 100;
        let wobble = rng.below(3) as i64 - 1;
        ((centre as i64 + wobble).clamp(0, variants as i64 - 1)) as usize
    }

    /// Records one settled bet.
    pub fn settle(&mut self, staked: i64, returned: i64) {
        self.chips -= staked;
        self.chips += returned;
        self.staked += staked;
        self.returned += returned;
        self.rounds += 1;
        self.biggest_win = self.biggest_win.max(returned - staked);
    }

    /// Net chips against the buy-in.
    pub fn net(&self) -> i64 {
        self.chips - self.bought
    }

    /// How they have actually run, as a percentage of what they have put
    /// through the table. This is measured, never rolled — a patron cannot
    /// be born lucky here, only turn out to have been.
    pub fn luck(&self) -> i64 {
        if self.staked == 0 {
            return 0;
        }
        (self.returned - self.staked) * 100 / self.staked
    }

    /// Is this patron done, and if so why? They leave when they are
    /// cleaned out, when a disciplined player is well ahead, or when an
    /// undisciplined one has finally had enough of losing.
    ///
    /// The reason comes back with the answer because the event feed and
    /// the statistics both want to count departures by cause, and working
    /// it out again afterwards from the numbers would get it wrong — a
    /// patron can be both ahead and out of chips for the minimum.
    pub fn leaving(&self, rng: &mut Rng, min_bet: i64) -> Option<Departure> {
        if self.chips < min_bet {
            return Some(Departure::Busted);
        }
        let ahead = self.net() > self.bought / 2;
        let deep_down = self.net() < -(self.bought * 3 / 4);
        if ahead && rng.below(100) < self.discipline as usize {
            return Some(Departure::Ahead);
        }
        if deep_down && rng.below(100) < (40 + self.discipline as usize / 4) {
            return Some(Departure::Down);
        }
        // Everyone drifts off eventually.
        if self.rounds > 40 && rng.below(100) < 4 {
            return Some(Departure::Drifted);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> Rng {
        Rng::from_seed(88)
    }

    #[test]
    fn patrons_come_through_the_door_different_from_each_other() {
        let mut rng = probe();
        let crowd: Vec<Patron> = (0..200).map(|i| Patron::new(&mut rng, 1_000, i)).collect();
        // Not one uniform blob: traits must actually vary.
        let spread = |f: fn(&Patron) -> u8| {
            let vals: Vec<u8> = crowd.iter().map(f).collect();
            (*vals.iter().max().unwrap() as i64) - (*vals.iter().min().unwrap() as i64)
        };
        assert!(spread(|p| p.nerve) > 50, "nerve barely varies");
        assert!(spread(|p| p.appetite) > 50, "appetite barely varies");
        assert!(spread(|p| p.discipline) > 50, "discipline barely varies");
        assert!(spread(|p| p.read) > 50, "read barely varies");
        // And they are not all called the same thing.
        let names: std::collections::HashSet<&str> = crowd.iter().map(|p| p.name.as_str()).collect();
        assert!(names.len() > 100, "only {} distinct names in 200 patrons", names.len());
    }

    #[test]
    fn nerve_decides_how_much_goes_on_a_bet() {
        let mut rng = probe();
        let mut timid = Patron::new(&mut rng, 1_000, 1);
        let mut bold = timid.clone();
        timid.nerve = 0;
        bold.nerve = 100;
        // Averaged over the jitter so the comparison is about nerve.
        let avg = |p: &Patron, rng: &mut Rng| (0..200).map(|_| p.stake(rng, 1)).sum::<i64>() / 200;
        let t = avg(&timid, &mut rng);
        let b = avg(&bold, &mut rng);
        assert!(b > t * 3, "bold staked {b} against timid's {t} — not enough of a gap");
    }

    #[test]
    fn nobody_ever_bets_more_than_they_are_holding() {
        let mut rng = probe();
        for _ in 0..500 {
            let stack = 1 + rng.below(500) as i64;
            let mut p = Patron::new(&mut rng, stack, 1);
            p.nerve = 100;
            p.discipline = 0;
            p.chips = p.chips.min(30);
            let bet = p.stake(&mut rng, 1);
            assert!(bet <= p.chips, "staked {bet} holding {}", p.chips);
        }
    }

    #[test]
    fn a_patron_who_cannot_cover_the_minimum_stakes_nothing() {
        let mut rng = probe();
        let mut p = Patron::new(&mut rng, 100, 1);
        p.chips = 4;
        assert_eq!(p.stake(&mut rng, 5), 0);
        assert_eq!(p.leaving(&mut rng, 5), Some(Departure::Busted), "and they should be on their way out");
    }

    #[test]
    fn appetite_reaches_further_up_the_bet_ladder() {
        let mut rng = probe();
        let mut cautious = Patron::new(&mut rng, 1_000, 1);
        let mut chancer = cautious.clone();
        cautious.appetite = 0;
        cautious.read = 0;
        chancer.appetite = 100;
        chancer.read = 0;
        let avg = |p: &Patron, rng: &mut Rng| (0..400).map(|_| p.pick_bet(rng, 7) as i64).sum::<i64>() / 400;
        let c = avg(&cautious, &mut rng);
        let k = avg(&chancer, &mut rng);
        assert!(k > c + 2, "chancer averaged bet {k}, cautious {c} — appetite is not reaching");
    }

    #[test]
    fn a_good_read_pulls_back_from_the_long_shots() {
        let mut rng = probe();
        let mut mug = Patron::new(&mut rng, 1_000, 1);
        let mut sharp = mug.clone();
        mug.appetite = 90;
        mug.read = 0;
        sharp.appetite = 90;
        sharp.read = 100;
        let avg = |p: &Patron, rng: &mut Rng| (0..400).map(|_| p.pick_bet(rng, 7) as i64).sum::<i64>() / 400;
        assert!(avg(&sharp, &mut rng) < avg(&mug, &mut rng), "the sharp patron is not pulling back");
    }

    #[test]
    fn a_bet_choice_is_always_on_the_board() {
        let mut rng = probe();
        for variants in 1..=10usize {
            for _ in 0..200 {
                let p = Patron::new(&mut rng, 1_000, 1);
                assert!(p.pick_bet(&mut rng, variants) < variants);
            }
        }
    }

    #[test]
    fn luck_is_measured_rather_than_rolled() {
        let mut rng = probe();
        let mut p = Patron::new(&mut rng, 1_000, 1);
        assert_eq!(p.luck(), 0, "a patron who has not played has not run well or badly");
        p.settle(100, 150);
        assert_eq!(p.luck(), 50);
        p.settle(100, 50);
        assert_eq!(p.luck(), 0, "200 through the table, 200 back");
        assert_eq!(p.net(), 0);
    }

    #[test]
    fn settling_tracks_the_stack_and_the_best_hit() {
        let mut rng = probe();
        let mut p = Patron::new(&mut rng, 1_000, 1);
        p.settle(50, 0);
        assert_eq!(p.chips, 950);
        p.settle(50, 400);
        assert_eq!(p.chips, 1_300);
        assert_eq!(p.biggest_win, 350);
        assert_eq!(p.rounds, 2);
    }

    #[test]
    fn discipline_decides_who_walks_away_ahead() {
        let mut rng = probe();
        let leaves = |discipline: u8, rng: &mut Rng| {
            let mut left = 0;
            for _ in 0..400 {
                let mut p = Patron::new(rng, 1_000, 1);
                p.discipline = discipline;
                p.chips = 2_000; // well ahead
                if p.leaving(rng, 5).is_some() {
                    left += 1;
                }
            }
            left
        };
        let steady = leaves(95, &mut rng);
        let hooked = leaves(5, &mut rng);
        assert!(steady > hooked * 3, "disciplined left {steady}, undisciplined {hooked}");
    }
}
