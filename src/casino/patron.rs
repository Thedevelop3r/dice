//! The people who fill the casino.
//!
//! A patron is not a coin flip with a name on it, and — since the roster
//! phase — not a temporary either. Each one is a person the building knows:
//! minted once, given an id that is unique for the life of the casino, and
//! kept in [`Roster`](super::roster::Roster) whether or not they happen to
//! be sitting at a table. They arrive, choose somewhere to play, play, get
//! up, go away for a while, and come back to a floor that remembers what
//! they did last time.
//!
//! Two layers decide how one behaves:
//!
//! - An [`Archetype`] — the kind of gambler they are. It sets the *bands*
//!   their traits are rolled within, so a whale is reliably a whale without
//!   every whale being identical.
//! - Four traits inside those bands:
//!   - **nerve** — how much of their stack goes on a bet.
//!   - **appetite** — how far up the risk ladder they reach when a table
//!     offers a choice of bets. This is the trait that changes *which real
//!     bet* they place, so it changes their whole return profile.
//!   - **discipline** — whether they walk away ahead, or keep going.
//!   - **read** — how well they judge a table. A sharp patron leans toward
//!     the better-priced bets; a poor one is drawn to the long shots.
//!
//! What no archetype and no trait does is bend an outcome. Every round a
//! patron plays is resolved by the table's own audited maths, exactly as if
//! a person were sitting there. `luck` is therefore a *result*, not an
//! input: it reports how a patron has actually run against what their
//! staking says they should have. That is why [`Archetype::Lucky`] is a
//! *behaviour* — a hunch-player whose staking wanders — and not a thumb on
//! the scale. Letting a "lucky" patron win more often would quietly undo
//! the whole point of `games::audit`.

use std::time::Duration;

use super::config::Config;
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

/// The kind of gambler somebody is.
///
/// An archetype is a *disposition*, never a result. It decides how somebody
/// bets — the size, the boldness, how long they stay — and has no say
/// whatever in whether the bet comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Archetype {
    /// Small stakes, short prices, goes home when they are ahead.
    Conservative,
    /// The middle of the population: bets freely, no particular plan.
    Gambler,
    /// Reads a board well and takes the best-priced bet on it.
    Strategist,
    /// Bets bigger the further behind they get.
    Chaser,
    /// Enormous stakes and the patience to lose them slowly.
    Whale,
    /// New to all of this: timid, easily talked into a long shot.
    Beginner,
    /// Plays hunches — erratic staking and a wandering eye for the board.
    /// Named for how they *feel*, not for how they run.
    Lucky,
}

impl Archetype {
    pub const ALL: [Archetype; 7] = [
        Archetype::Conservative,
        Archetype::Gambler,
        Archetype::Strategist,
        Archetype::Chaser,
        Archetype::Whale,
        Archetype::Beginner,
        Archetype::Lucky,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Archetype::Conservative => "conservative",
            Archetype::Gambler => "gambler",
            Archetype::Strategist => "strategist",
            Archetype::Chaser => "chaser",
            Archetype::Whale => "whale",
            Archetype::Beginner => "beginner",
            Archetype::Lucky => "hunch player",
        }
    }

    /// How common each kind is, out of the whole population. Weighted so
    /// the floor is mostly ordinary people and the extremes stay rare
    /// enough to be worth noticing when one walks in.
    pub fn weight(self) -> usize {
        match self {
            Archetype::Conservative => 22,
            Archetype::Gambler => 28,
            Archetype::Strategist => 10,
            Archetype::Chaser => 15,
            Archetype::Whale => 3,
            Archetype::Beginner => 14,
            Archetype::Lucky => 8,
        }
    }

    /// The `(low, high)` band each trait is rolled inside, in the order
    /// nerve, appetite, discipline, read.
    pub fn bands(self) -> [(u8, u8); 4] {
        match self {
            //                          nerve      appetite   discipline  read
            Archetype::Conservative => [(5, 35), (5, 30), (60, 100), (35, 75)],
            Archetype::Gambler => [(30, 75), (30, 75), (25, 70), (25, 70)],
            Archetype::Strategist => [(25, 60), (10, 40), (55, 95), (75, 100)],
            Archetype::Chaser => [(45, 90), (45, 90), (0, 25), (15, 50)],
            Archetype::Whale => [(60, 100), (35, 80), (30, 75), (40, 85)],
            Archetype::Beginner => [(5, 30), (40, 85), (30, 70), (0, 25)],
            Archetype::Lucky => [(35, 85), (55, 100), (10, 45), (0, 40)],
        }
    }

    /// How much of an ordinary buy-in this kind of person walks in with, in
    /// hundredths. A whale's is the reason the top table limit exists.
    pub fn buy_in_scale(self) -> i64 {
        match self {
            Archetype::Conservative => 70,
            Archetype::Gambler => 100,
            Archetype::Strategist => 120,
            Archetype::Chaser => 90,
            Archetype::Whale => 1_200,
            Archetype::Beginner => 45,
            Archetype::Lucky => 85,
        }
    }

    /// The inverse of `label`, for reading a saved roster back.
    pub fn from_label(label: &str) -> Option<Archetype> {
        Archetype::ALL.iter().copied().find(|a| a.label() == label)
    }

    /// Rolls one, respecting how common each kind is.
    pub fn roll(rng: &mut Rng) -> Archetype {
        let total: usize = Archetype::ALL.iter().map(|a| a.weight()).sum();
        let mut pick = rng.below(total);
        for a in Archetype::ALL {
            if pick < a.weight() {
                return a;
            }
            pick -= a.weight();
        }
        Archetype::Gambler
    }
}

/// Where somebody is right now.
///
/// The point of this type is that "not at a table" is a real state rather
/// than non-existence. A patron who gets up is `Away`; the roster still
/// holds them, their record intact, until they come back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// Not in the building. Comes back at this simulated time.
    Away { back_at: Duration },
    /// In the building, deciding where to play.
    Looking,
    /// Sat at this table.
    Seated { table: u32 },
    /// In a tournament. In the building and busy, but not at a cash table
    /// — which is what stops the seating loop offering them a chair.
    InTournament { id: u32 },
}

impl Presence {
    pub fn table(self) -> Option<u32> {
        match self {
            Presence::Seated { table } => Some(table),
            _ => None,
        }
    }

    pub fn is_here(self) -> bool {
        !matches!(self, Presence::Away { .. })
    }

    /// Whether they are free to be offered a seat.
    pub fn is_free(self) -> bool {
        matches!(self, Presence::Looking)
    }
}

/// What the building remembers about somebody, across every visit they have
/// ever made.
///
/// Accumulated as it happens, never recomputed — the same rule the event
/// feed follows, and the reason a leaderboard can be drawn without walking
/// history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lifetime {
    pub visits: u32,
    pub rounds: u64,
    pub staked: i64,
    pub returned: i64,
    pub biggest_win: i64,
    /// The best and worst a single visit has ever finished, in chips.
    pub best_visit: i64,
    pub worst_visit: i64,
}

impl Lifetime {
    /// Chips won or lost over every visit.
    pub fn net(&self) -> i64 {
        self.returned - self.staked
    }
}

#[derive(Debug, Clone)]
pub struct Patron {
    /// Unique on the floor, for the whole life of the casino. Minted by the
    /// roster rather than by a table, so a patron keeps one identity
    /// wherever they sit and however long they are away.
    pub id: u64,
    pub name: String,
    pub archetype: Archetype,
    pub presence: Presence,

    /// Chips in front of them right now.
    pub chips: i64,
    /// What they bought in for on *this* visit, in chips — the baseline
    /// `net` and `luck` measure against.
    pub bought: i64,

    /// 0-100 each, rolled inside the archetype's bands.
    pub nerve: u8,
    pub appetite: u8,
    pub discipline: u8,
    pub read: u8,

    /// This visit.
    pub rounds: u32,
    pub staked: i64,
    pub returned: i64,
    pub biggest_win: i64,

    /// Every visit, ever.
    pub lifetime: Lifetime,
}

impl Patron {
    /// Mints a new person. They start in the building with nothing in front
    /// of them — chips are dealt with at the cage, on arrival, not here.
    pub fn new(rng: &mut Rng, id: u64) -> Patron {
        let archetype = Archetype::roll(rng);
        Patron::of_kind(rng, id, archetype)
    }

    pub fn of_kind(rng: &mut Rng, id: u64, archetype: Archetype) -> Patron {
        let bands = archetype.bands();
        // Within a band, traits are the mean of two draws, which clusters
        // them toward the middle of the band: most people of a kind are
        // typical of it, and the extremes stay rare.
        let roll = |rng: &mut Rng, (lo, hi): (u8, u8)| {
            let span = (hi - lo) as usize + 1;
            lo + ((rng.below(span) + rng.below(span)) / 2) as u8
        };
        Patron {
            id,
            name: format!("{} {}", FIRST[rng.below(FIRST.len())], LAST[rng.below(LAST.len())]),
            archetype,
            presence: Presence::Looking,
            chips: 0,
            bought: 0,
            nerve: roll(rng, bands[0]),
            appetite: roll(rng, bands[1]),
            discipline: roll(rng, bands[2]),
            read: roll(rng, bands[3]),
            rounds: 0,
            staked: 0,
            returned: 0,
            biggest_win: 0,
            lifetime: Lifetime::default(),
        }
    }

    /// Starts a visit with `chips` bought at the cage. Clears the per-visit
    /// figures; the lifetime record is untouched, which is the whole point
    /// of having two.
    pub fn begin_visit(&mut self, chips: i64) {
        self.chips = chips;
        self.bought = chips;
        self.rounds = 0;
        self.staked = 0;
        self.returned = 0;
        self.biggest_win = 0;
        self.lifetime.visits += 1;
        self.presence = Presence::Looking;
    }

    /// Ends a visit, folding it into the lifetime record. Returns the chips
    /// handed back at the cage.
    pub fn end_visit(&mut self, back_at: Duration) -> i64 {
        let net = self.net();
        self.lifetime.best_visit = self.lifetime.best_visit.max(net);
        self.lifetime.worst_visit = self.lifetime.worst_visit.min(net);
        let chips = self.chips;
        self.chips = 0;
        self.bought = 0;
        self.presence = Presence::Away { back_at };
        chips
    }

    /// A one-word read on the patron, for the table view.
    pub fn style(&self) -> &'static str {
        self.archetype.label()
    }

    /// Which tier the house has them in, on lifetime turnover.
    pub fn tier(&self, cfg: &Config) -> usize {
        cfg.tier_of(self.lifetime.staked)
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
        // A little jitter so a table is not a metronome. A hunch player's
        // is wider, because that is what playing hunches looks like from
        // the outside — the size wobbles, the odds do not.
        let swing: i64 = if self.archetype == Archetype::Lucky { 60 } else { 20 };
        let jitter = (100 - swing / 2) + rng.below(swing as usize + 1) as i64;
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

    /// How much this person fancies a table, given how many bets it offers
    /// and how quickly it deals. Higher is keener.
    ///
    /// This is the "choose a game" step of the lifecycle, and it is a
    /// preference and nothing more: it decides *where* somebody plays, and
    /// the table they land at then resolves their bets with its own audited
    /// maths exactly as it would for anyone else.
    pub fn taste(&self, variants: usize, pace_ms: u64) -> i64 {
        let mut score = 50;
        // A board with choices on it appeals to an appetite; a machine that
        // only does one thing appeals to people who do not want choices.
        if variants > 3 {
            score += self.appetite as i64 / 2;
            score += self.read as i64 / 4;
        } else {
            score += (100 - self.appetite as i64) / 3;
        }
        // Fast games suit the impatient; a slow table suits somebody who
        // came to sit down for the evening.
        let slow = pace_ms >= 4_000;
        score += match self.archetype {
            Archetype::Whale | Archetype::Strategist => {
                if slow {
                    30
                } else {
                    -10
                }
            }
            Archetype::Beginner => {
                if variants <= 1 {
                    35
                } else {
                    -5
                }
            }
            Archetype::Chaser | Archetype::Lucky => {
                if slow {
                    -15
                } else {
                    25
                }
            }
            _ => 0,
        };
        score.max(1)
    }

    /// What this person would buy in for, in dollars, given the house's
    /// ordinary range.
    pub fn buy_in(&self, rng: &mut Rng, cfg: &Config) -> i64 {
        let (lo, hi) = cfg.buy_in;
        let base = lo + rng.below((hi - lo + 1).max(1) as usize) as i64;
        (base * self.archetype.buy_in_scale() / 100).max(1)
    }

    pub fn settle(&mut self, staked: i64, returned: i64) {
        self.chips -= staked;
        self.chips += returned;
        self.staked += staked;
        self.returned += returned;
        self.rounds += 1;
        self.biggest_win = self.biggest_win.max(returned - staked);

        self.lifetime.rounds += 1;
        self.lifetime.staked += staked;
        self.lifetime.returned += returned;
        self.lifetime.biggest_win = self.lifetime.biggest_win.max(returned - staked);
    }

    /// Net chips against this visit's buy-in.
    pub fn net(&self) -> i64 {
        self.chips - self.bought
    }

    /// How they have actually run on this visit, as a percentage of what
    /// they have put through the table. This is measured, never rolled — a
    /// patron cannot be born lucky here, only turn out to have been.
    pub fn luck(&self) -> i64 {
        if self.staked == 0 {
            return 0;
        }
        (self.returned - self.staked) * 100 / self.staked
    }

    /// The same figure over every visit they have ever made, which is the
    /// one that actually means something.
    pub fn lifetime_luck(&self) -> i64 {
        if self.lifetime.staked == 0 {
            return 0;
        }
        self.lifetime.net() * 100 / self.lifetime.staked
    }

    /// Is this patron done at the table, and if so why? They leave when
    /// they are cleaned out, when a disciplined player is well ahead, or
    /// when an undisciplined one has finally had enough of losing.
    ///
    /// The reason comes back with the answer because the event feed and the
    /// statistics both want to count departures by cause, and working it
    /// out again afterwards from the numbers would get it wrong — a patron
    /// can be both ahead and out of chips for the minimum.
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
        // Everyone drifts off eventually. A whale has the patience to sit
        // far longer than a beginner does.
        let patience = match self.archetype {
            Archetype::Whale => 160,
            Archetype::Conservative | Archetype::Strategist => 70,
            Archetype::Beginner => 25,
            _ => 40,
        };
        if self.rounds > patience && rng.below(100) < 4 {
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

    /// Somebody at a table with chips in front of them, which is what most
    /// of these tests are actually about.
    fn seated(rng: &mut Rng, chips: i64) -> Patron {
        let mut p = Patron::new(rng, 1);
        p.begin_visit(chips);
        p
    }

    #[test]
    fn patrons_come_through_the_door_different_from_each_other() {
        let mut rng = probe();
        let crowd: Vec<Patron> = (0..300).map(|i| Patron::new(&mut rng, i)).collect();
        let spread = |f: fn(&Patron) -> u8| {
            let vals: Vec<u8> = crowd.iter().map(f).collect();
            (*vals.iter().max().unwrap() as i64) - (*vals.iter().min().unwrap() as i64)
        };
        assert!(spread(|p| p.nerve) > 50, "nerve barely varies");
        assert!(spread(|p| p.appetite) > 50, "appetite barely varies");
        assert!(spread(|p| p.discipline) > 50, "discipline barely varies");
        assert!(spread(|p| p.read) > 50, "read barely varies");
        let names: std::collections::HashSet<&str> = crowd.iter().map(|p| p.name.as_str()).collect();
        assert!(names.len() > 150, "only {} distinct names in 300 patrons", names.len());
    }

    #[test]
    fn every_archetype_actually_turns_up_and_the_rare_ones_stay_rare() {
        let mut rng = probe();
        let mut counts = [0usize; 7];
        for i in 0..4_000 {
            let p = Patron::new(&mut rng, i);
            let at = Archetype::ALL.iter().position(|a| *a == p.archetype).unwrap();
            counts[at] += 1;
        }
        for (i, a) in Archetype::ALL.iter().enumerate() {
            assert!(counts[i] > 0, "{} never walked in", a.label());
        }
        let whales = counts[Archetype::ALL.iter().position(|a| *a == Archetype::Whale).unwrap()];
        let gamblers = counts[Archetype::ALL.iter().position(|a| *a == Archetype::Gambler).unwrap()];
        assert!(whales * 4 < gamblers, "whales are meant to be rare: {whales} of 4000");
    }

    #[test]
    fn an_archetype_can_be_read_back_from_what_it_writes() {
        for a in Archetype::ALL {
            assert_eq!(Archetype::from_label(a.label()), Some(a), "{} does not survive a round trip", a.label());
        }
        assert_eq!(Archetype::from_label("card counter"), None);
    }

    #[test]
    fn an_archetype_is_a_disposition_and_traits_land_inside_its_bands() {
        let mut rng = probe();
        for a in Archetype::ALL {
            let bands = a.bands();
            for i in 0..400u64 {
                let p = Patron::of_kind(&mut rng, i, a);
                assert_eq!(p.archetype, a);
                for (t, (lo, hi)) in [p.nerve, p.appetite, p.discipline, p.read].iter().zip(bands) {
                    assert!(*t >= lo && *t <= hi, "{} rolled {t} outside {lo}..={hi}", a.label());
                }
            }
        }
    }

    #[test]
    fn the_archetypes_differ_where_they_are_supposed_to() {
        let mut rng = probe();
        let avg = |a: Archetype, f: fn(&Patron) -> u8, rng: &mut Rng| {
            (0..400).map(|i| f(&Patron::of_kind(rng, i, a)) as i64).sum::<i64>() / 400
        };
        // A strategist reads a board better than a beginner does.
        assert!(avg(Archetype::Strategist, |p| p.read, &mut rng) > avg(Archetype::Beginner, |p| p.read, &mut rng) + 40);
        // A chaser has less discipline than a conservative player.
        assert!(avg(Archetype::Conservative, |p| p.discipline, &mut rng) > avg(Archetype::Chaser, |p| p.discipline, &mut rng) + 40);
        // A whale bets a bigger share of a much bigger stack.
        assert!(avg(Archetype::Whale, |p| p.nerve, &mut rng) > avg(Archetype::Beginner, |p| p.nerve, &mut rng) + 30);
        assert!(Archetype::Whale.buy_in_scale() > Archetype::Beginner.buy_in_scale() * 5);
    }

    #[test]
    fn a_lucky_player_is_a_behaviour_and_never_a_thumb_on_the_scale() {
        // The one property of this archetype that must hold: it changes how
        // the stake wobbles and nothing at all about what comes back.
        let mut rng = probe();
        let mut hunch = Patron::of_kind(&mut rng, 1, Archetype::Lucky);
        let mut plain = Patron::of_kind(&mut rng, 2, Archetype::Gambler);
        // Same traits, same stack — the only difference left is the label.
        plain.nerve = hunch.nerve;
        plain.appetite = hunch.appetite;
        plain.discipline = hunch.discipline;
        plain.read = hunch.read;
        hunch.begin_visit(10_000);
        plain.begin_visit(10_000);

        let spread = |p: &Patron, rng: &mut Rng| {
            let bets: Vec<i64> = (0..600).map(|_| p.stake(rng, 5)).collect();
            bets.iter().max().unwrap() - bets.iter().min().unwrap()
        };
        assert!(spread(&hunch, &mut rng) > spread(&plain, &mut rng), "a hunch player's staking should wobble more");

        // And settlement is pure arithmetic on what the table returned.
        hunch.settle(100, 250);
        plain.settle(100, 250);
        assert_eq!(hunch.chips, plain.chips, "the archetype changed a settlement");
        assert_eq!(hunch.luck(), plain.luck());
    }

    #[test]
    fn nerve_decides_how_much_goes_on_a_bet() {
        let mut rng = probe();
        let mut timid = seated(&mut rng, 1_000);
        let mut bold = timid.clone();
        timid.nerve = 0;
        bold.nerve = 100;
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
            let mut p = seated(&mut rng, stack);
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
        let mut p = seated(&mut rng, 100);
        p.chips = 4;
        assert_eq!(p.stake(&mut rng, 5), 0);
        assert_eq!(p.leaving(&mut rng, 5), Some(Departure::Busted), "and they should be on their way out");
    }

    #[test]
    fn appetite_reaches_further_up_the_bet_ladder() {
        let mut rng = probe();
        let mut cautious = seated(&mut rng, 1_000);
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
        let mut mug = seated(&mut rng, 1_000);
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
                let p = seated(&mut rng, 1_000);
                assert!(p.pick_bet(&mut rng, variants) < variants);
            }
        }
    }

    #[test]
    fn luck_is_measured_rather_than_rolled() {
        let mut rng = probe();
        let mut p = seated(&mut rng, 1_000);
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
        let mut p = seated(&mut rng, 1_000);
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
                let mut p = seated(rng, 1_000);
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

    #[test]
    fn a_visit_ends_but_the_record_does_not() {
        // Rule 4, in one test: what a person did survives them getting up.
        let mut rng = probe();
        let mut p = Patron::new(&mut rng, 42);
        assert_eq!(p.lifetime.visits, 0);

        p.begin_visit(1_000);
        p.settle(200, 500);
        p.settle(200, 0);
        let first = p.net();
        let handed_back = p.end_visit(Duration::from_secs(600));
        assert_eq!(handed_back, 1_100);
        assert_eq!(p.chips, 0, "they took their chips to the cage");
        assert!(matches!(p.presence, Presence::Away { .. }));

        p.begin_visit(500);
        assert_eq!(p.staked, 0, "the visit's figures start clean");
        assert_eq!(p.net(), 0);
        assert_eq!(p.lifetime.visits, 2, "but the building has seen them twice");
        assert_eq!(p.lifetime.staked, 400, "and remembers everything they put through");
        assert_eq!(p.lifetime.returned, 500);
        assert_eq!(p.lifetime.biggest_win, 300);
        assert_eq!(p.lifetime.best_visit, first);
        assert_eq!(p.id, 42, "and it is the same person");
    }

    #[test]
    fn a_tier_is_earned_over_a_lifetime_not_over_a_night() {
        let cfg = Config::default();
        let mut rng = probe();
        let mut p = Patron::new(&mut rng, 1);
        p.begin_visit(1_000);
        assert_eq!(cfg.tier_name(p.tier(&cfg)), "guest");
        // One enormous night does not make a whale...
        p.settle(200_000, 0);
        assert_eq!(cfg.tier_name(p.tier(&cfg)), "high roller");
        // ...but a long history of them does.
        for _ in 0..4 {
            p.settle(200_000, 0);
        }
        assert_eq!(cfg.tier_name(p.tier(&cfg)), "whale");
        // And it is turnover that did it, not the stack: they are broke.
        assert!(p.net() < 0);
    }
}
