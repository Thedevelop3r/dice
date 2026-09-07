//! Everyone the casino knows, whether they are here tonight or not.
//!
//! Before this file, a patron was a row in a table's `Vec`: created when a
//! seat needed filling and dropped when they got up. Nothing about them
//! survived, so "the same person came back" was not a thing the program
//! could say. Rule 4 asks for the opposite, and this is where it lives.
//!
//! The roster owns every [`Patron`] by id. Instances own only *ids*. That
//! single change is what makes the rest possible:
//!
//! - Somebody can exist while not seated — [`Presence::Away`] is a state,
//!   not a deletion.
//! - Somebody can move between tables without being reconstructed.
//! - A lifetime record has somewhere to accumulate.
//! - A leaderboard, a VIP list and a "regulars" view are all queries over
//!   one collection rather than a scan of every table.
//!
//! **The population is bounded.** `Config::roster_size` caps how many named
//! people the world holds. Once it is full, a new arrival is somebody
//! coming *back* rather than somebody new, which is the whole point: a
//! casino that mints a fresh stranger for every seat is exactly the thing
//! Rule 4 forbids, and it also leaks memory all night.
//!
//! The query methods at the bottom — the leaderboards, the tier census —
//! are the substrate the analytics and leaderboard phases are built on, and
//! are written and tested here so that the phase which needs them does not
//! also have to invent them. Same justification as `games::cards`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use super::config::Config;
use super::patron::{Patron, Presence};
use crate::rng::Rng;

/// The building's memory of its customers.
#[derive(Debug, Clone, Default)]
pub struct Roster {
    people: BTreeMap<u64, Patron>,
    next_id: u64,
    /// How many have ever been minted, which is not the same as how many
    /// are held once the population is capped.
    minted: u64,
}

impl Roster {
    pub fn new() -> Roster {
        Roster { people: BTreeMap::new(), next_id: 1, minted: 0 }
    }

    pub fn get(&self, id: u64) -> Option<&Patron> {
        self.people.get(&id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Patron> {
        self.people.get_mut(&id)
    }

    pub fn len(&self) -> usize {
        self.people.len()
    }

    pub fn is_empty(&self) -> bool {
        self.people.is_empty()
    }

    pub fn minted(&self) -> u64 {
        self.minted
    }

    pub fn iter(&self) -> impl Iterator<Item = &Patron> {
        self.people.values()
    }

    /// How many are in the building at all, seated or still choosing.
    pub fn present(&self) -> usize {
        self.people.values().filter(|p| p.presence.is_here()).count()
    }

    /// Everyone in the building who is not yet at a table.
    pub fn looking(&self) -> Vec<u64> {
        self.people.iter().filter(|(_, p)| p.presence == Presence::Looking).map(|(id, _)| *id).collect()
    }

    /// Everyone whose time away is up, oldest booking first.
    pub fn due_back(&self, now: Duration) -> Vec<u64> {
        let mut out: Vec<(Duration, u64)> = self
            .people
            .iter()
            .filter_map(|(id, p)| match p.presence {
                Presence::Away { back_at } if back_at <= now => Some((back_at, *id)),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out.into_iter().map(|(_, id)| id).collect()
    }

    /// Mints somebody brand new and puts them in the building.
    pub fn mint(&mut self, rng: &mut Rng) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.minted += 1;
        self.people.insert(id, Patron::new(rng, id));
        id
    }

    /// Somebody already in the building and not yet playing, if there is
    /// anybody. Does **not** invent one: a seat is only filled if a person
    /// is actually standing there, which is what makes occupancy a real
    /// measurement rather than a tautology.
    pub fn waiting(&mut self, rng: &mut Rng) -> Option<u64> {
        self.pick_waiting(rng)
    }

    /// Somebody walks in through the front door: a familiar face if the
    /// population is already at its cap, otherwise a new one.
    ///
    /// This is the method that makes the floor feel like a place rather
    /// than a queue. The person arrives in [`Presence::Looking`]; finding
    /// them a table is the floor's job, and may not succeed.
    pub fn admit(&mut self, rng: &mut Rng, cfg: &Config, now: Duration) -> Option<u64> {
        if self.people.len() < cfg.roster_size {
            let id = self.mint(rng);
            return Some(id);
        }
        // The world is full, so somebody comes back early rather than a
        // stranger being invented. Whoever has been away longest is up.
        let soonest = self
            .people
            .iter()
            .filter_map(|(id, p)| match p.presence {
                Presence::Away { back_at } => Some((back_at, *id)),
                _ => None,
            })
            .min();
        let (_, id) = soonest?;
        if let Some(p) = self.people.get_mut(&id) {
            p.presence = Presence::Looking;
            let _ = now;
        }
        Some(id)
    }

    /// One of the people already in the building and not yet playing.
    fn pick_waiting(&mut self, rng: &mut Rng) -> Option<u64> {
        let waiting = self.looking();
        if waiting.is_empty() {
            return None;
        }
        Some(waiting[rng.below(waiting.len())])
    }

    /// Brings somebody back from a spell away, ready to choose a table.
    pub fn welcome_back(&mut self, id: u64) {
        if let Some(p) = self.people.get_mut(&id)
            && !p.presence.is_here()
        {
            p.presence = Presence::Looking;
        }
    }

    /// Sits somebody at a table.
    pub fn seat(&mut self, id: u64, table: u32) {
        if let Some(p) = self.people.get_mut(&id) {
            p.presence = Presence::Seated { table };
        }
    }

    /// The ids currently seated at a table.
    pub fn seated_at(&self, table: u32) -> Vec<u64> {
        self.people
            .iter()
            .filter(|(_, p)| p.presence.table() == Some(table))
            .map(|(id, _)| *id)
            .collect()
    }

    /// The `n` people with the most turnover, best first — the VIP list and
    /// the leaderboard's spine.
    pub fn by_turnover(&self, n: usize) -> Vec<&Patron> {
        let mut all: Vec<&Patron> = self.people.values().collect();
        all.sort_by(|a, b| b.lifetime.staked.cmp(&a.lifetime.staked).then(a.id.cmp(&b.id)));
        all.truncate(n);
        all
    }

    /// The `n` people most up on the house, best first.
    pub fn by_winnings(&self, n: usize) -> Vec<&Patron> {
        let mut all: Vec<&Patron> = self.people.values().collect();
        all.sort_by(|a, b| b.lifetime.net().cmp(&a.lifetime.net()).then(a.id.cmp(&b.id)));
        all.truncate(n);
        all
    }

    /// How the population breaks down by tier, lowest tier first.
    pub fn by_tier(&self, cfg: &Config) -> Vec<usize> {
        let mut counts = vec![0usize; cfg.tiers.len()];
        let top = counts.len() - 1;
        for p in self.people.values() {
            counts[p.tier(cfg).min(top)] += 1;
        }
        counts
    }

    /// Chips held by everyone in the building — the other half of the
    /// conservation sum, the house tray being the first.
    pub fn chips_in_play(&self) -> i64 {
        self.people.values().map(|p| p.chips).sum()
    }
}

/// How long somebody stays away between visits, drawn from the configured
/// range. Kept here rather than in `Patron` because it is a property of the
/// world's pacing, not of the person.
pub fn time_away(rng: &mut Rng, cfg: &Config, now: Duration) -> Duration {
    let (lo, hi) = cfg.away_for;
    let span = hi.saturating_sub(lo).as_millis().max(1) as usize;
    now + lo + Duration::from_millis(rng.below(span) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casino::patron::Archetype;

    fn probe() -> Rng {
        Rng::from_seed(7)
    }

    #[test]
    fn a_new_roster_mints_distinct_people_with_distinct_ids() {
        let mut rng = probe();
        let mut r = Roster::new();
        let ids: Vec<u64> = (0..50).map(|_| r.mint(&mut rng)).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "an id was reused");
        assert_eq!(r.len(), 50);
        assert_eq!(r.minted(), 50);
        for id in ids {
            assert_eq!(r.get(id).expect("held").id, id);
        }
    }

    #[test]
    fn somebody_who_gets_up_is_still_somebody() {
        let mut rng = probe();
        let mut r = Roster::new();
        let cfg = Config::default();
        let id = r.mint(&mut rng);
        r.get_mut(id).unwrap().begin_visit(1_000);
        r.seat(id, 4);
        assert_eq!(r.seated_at(4), vec![id]);

        let back = Duration::from_secs(300);
        r.get_mut(id).unwrap().end_visit(back);
        assert_eq!(r.len(), 1, "they were deleted rather than sent home");
        assert_eq!(r.seated_at(4), Vec::<u64>::new());
        assert_eq!(r.present(), 0);
        assert!(r.due_back(Duration::from_secs(299)).is_empty());
        assert_eq!(r.due_back(back), vec![id]);

        r.welcome_back(id);
        assert_eq!(r.present(), 1);
        assert_eq!(r.looking(), vec![id]);
        assert_eq!(r.get(id).unwrap().lifetime.visits, 1);
        let _ = cfg;
    }

    #[test]
    fn the_population_is_bounded_and_familiar_faces_come_back() {
        // The Rule 4 property: a busy night must not invent a thousand
        // strangers. Past the cap, the next seat is filled by a returner.
        let mut rng = probe();
        let cfg = Config { roster_size: 12, ..Config::default() };
        let mut r = Roster::new();
        let now = Duration::from_secs(0);

        // Seat everyone, then send them all home.
        for i in 0..40 {
            let Some(id) = r.admit(&mut rng, &cfg, now) else { panic!("nobody available") };
            r.get_mut(id).unwrap().begin_visit(100);
            r.seat(id, i as u32 % 3);
            // ...and immediately get them up again, so the next call has to
            // decide between minting and recalling.
            r.get_mut(id).unwrap().end_visit(Duration::from_secs(9_999));
        }
        assert_eq!(r.len(), 12, "the world grew past its cap: {} people", r.len());
        assert!(r.minted() <= 12, "{} people were minted for 40 seats", r.minted());
        // Everyone in it has been in more than once — they are regulars.
        let repeats = r.iter().filter(|p| p.lifetime.visits > 1).count();
        assert!(repeats > 0, "nobody ever came back");
    }

    #[test]
    fn a_seat_is_only_ever_filled_by_somebody_who_is_actually_here() {
        // The property that makes occupancy a measurement rather than a
        // tautology: asking for a seat-filler must never conjure one.
        let mut rng = probe();
        let mut r = Roster::new();
        assert_eq!(r.waiting(&mut rng), None, "somebody was invented out of an empty room");
        assert_eq!(r.minted(), 0);

        let known = r.mint(&mut rng);
        assert_eq!(r.waiting(&mut rng), Some(known));
        assert_eq!(r.minted(), 1, "the person already stood there should have been used");

        // Somebody who is seated is not available to fill another seat.
        r.seat(known, 3);
        assert_eq!(r.waiting(&mut rng), None);
    }

    #[test]
    fn the_leaderboards_rank_by_what_they_say_they_rank_by() {
        let mut rng = probe();
        let mut r = Roster::new();
        let ids: Vec<u64> = (0..5).map(|_| r.mint(&mut rng)).collect();
        for (i, id) in ids.iter().enumerate() {
            let p = r.get_mut(*id).unwrap();
            p.begin_visit(10_000);
            // Turnover climbs with i; winnings run the other way.
            p.settle(1_000 * (i as i64 + 1), 900 * (i as i64 + 1));
        }
        let turnover = r.by_turnover(3);
        assert_eq!(turnover.len(), 3);
        assert_eq!(turnover[0].id, ids[4], "the biggest bettor is not top");
        assert!(turnover[0].lifetime.staked >= turnover[1].lifetime.staked);

        let winners = r.by_winnings(5);
        assert_eq!(winners[0].id, ids[0], "the least-down player should head the winnings list");
        assert!(winners[0].lifetime.net() >= winners[4].lifetime.net());
    }

    #[test]
    fn tiers_partition_the_whole_population_exactly_once() {
        let mut rng = probe();
        let cfg = Config::default();
        let mut r = Roster::new();
        for i in 0..40 {
            let id = r.mint(&mut rng);
            let p = r.get_mut(id).unwrap();
            p.begin_visit(1_000_000);
            p.settle(i as i64 * 30_000, 0);
        }
        let counts = r.by_tier(&cfg);
        assert_eq!(counts.len(), cfg.tiers.len());
        assert_eq!(counts.iter().sum::<usize>(), r.len(), "somebody is in two tiers or none");
        assert!(counts[0] > 0 && counts.iter().skip(1).any(|c| *c > 0), "everybody landed in one tier");
    }

    #[test]
    fn time_away_lands_inside_the_configured_range() {
        let mut rng = probe();
        let cfg = Config::default();
        let now = Duration::from_secs(1_000);
        for _ in 0..500 {
            let back = time_away(&mut rng, &cfg, now);
            assert!(back >= now + cfg.away_for.0, "came back too soon");
            assert!(back <= now + cfg.away_for.1, "stayed away too long");
        }
    }

    #[test]
    fn chips_in_play_is_the_sum_of_every_stack_in_the_room() {
        let mut rng = probe();
        let mut r = Roster::new();
        for _ in 0..10 {
            let id = r.mint(&mut rng);
            r.get_mut(id).unwrap().begin_visit(250);
        }
        assert_eq!(r.chips_in_play(), 2_500);
        let one = r.looking()[0];
        r.get_mut(one).unwrap().end_visit(Duration::from_secs(60));
        assert_eq!(r.chips_in_play(), 2_250, "a departed patron is still holding chips");
    }

    #[test]
    fn an_archetype_survives_a_visit_ending() {
        let mut rng = probe();
        let mut r = Roster::new();
        let id = r.mint(&mut rng);
        let kind = r.get(id).unwrap().archetype;
        assert!(Archetype::ALL.contains(&kind));
        r.get_mut(id).unwrap().begin_visit(500);
        r.get_mut(id).unwrap().end_visit(Duration::from_secs(10));
        r.welcome_back(id);
        assert_eq!(r.get(id).unwrap().archetype, kind, "somebody changed personality overnight");
    }
}
