//! One running table: its seats, its round clock, and its history.
//!
//! An instance owns everything about itself and nothing about anything else.
//! It never draws, never reads the keyboard, and never reaches for the
//! player's wallet — it advances when the manager tells it a round is due,
//! and reports what happened.
//!
//! Rounds resolve through each table's own `simulate` entry point, which is
//! the same code `games::audit` holds to its declared return. A background
//! table is therefore not an approximation of the real game: it *is* the
//! real game, with the animation and the keyboard taken away.

use super::event::{Event, Weight};
use super::sim::Sim;
use crate::games::{baccarat, bigsix, bingo, blackjack, chuck, crash, hilo, horses, keno, mines, plinko, roulette, scratch, slots, threecard, vidpoker, war};
use crate::rng::Rng;
use std::time::Duration;

/// Every table the floor can run unattended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Slots,
    Roulette,
    Blackjack,
    Baccarat,
    VidPoker,
    ThreeCard,
    War,
    HiLo,
    BigSix,
    Keno,
    Bingo,
    Plinko,
    Mines,
    Crash,
    Scratch,
    Horses,
    Chuck,
}

impl Kind {
    pub const ALL: [Kind; 17] = [
        Kind::Slots,
        Kind::Roulette,
        Kind::Blackjack,
        Kind::Baccarat,
        Kind::VidPoker,
        Kind::ThreeCard,
        Kind::War,
        Kind::HiLo,
        Kind::BigSix,
        Kind::Keno,
        Kind::Bingo,
        Kind::Plinko,
        Kind::Mines,
        Kind::Crash,
        Kind::Scratch,
        Kind::Horses,
        Kind::Chuck,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Slots => "Slots",
            Kind::Roulette => "Roulette",
            Kind::Blackjack => "Blackjack",
            Kind::Baccarat => "Baccarat",
            Kind::VidPoker => "Video Poker",
            Kind::ThreeCard => "Three Card Poker",
            Kind::War => "Casino War",
            Kind::HiLo => "Hi-Lo",
            Kind::BigSix => "Big Six",
            Kind::Keno => "Keno",
            Kind::Bingo => "Bingo",
            Kind::Plinko => "Plinko",
            Kind::Mines => "Mines",
            Kind::Crash => "Crash",
            Kind::Scratch => "Scratch Cards",
            Kind::Horses => "Horse Racing",
            Kind::Chuck => "Chuck-a-Luck",
        }
    }

    /// What a seat is called here — a machine has a player at it, a table
    /// has seats, a race has punters.
    pub fn seat_word(self) -> &'static str {
        match self {
            Kind::Slots | Kind::VidPoker | Kind::Scratch | Kind::Crash | Kind::Mines | Kind::Plinko => "player",
            Kind::Horses => "punter",
            _ => "seat",
        }
    }

    /// The stats key this table books under, shared with `economy::House`.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Slots => "slots",
            Kind::Roulette => "roulette",
            Kind::Blackjack => "blackjack",
            Kind::Baccarat => "baccarat",
            Kind::VidPoker => "vidpoker",
            Kind::ThreeCard => "threecard",
            Kind::War => "war",
            Kind::HiLo => "hilo",
            Kind::BigSix => "bigsix",
            Kind::Keno => "keno",
            Kind::Bingo => "bingo",
            Kind::Plinko => "plinko",
            Kind::Mines => "mines",
            Kind::Crash => "crash",
            Kind::Scratch => "scratch",
            Kind::Horses => "horses",
            Kind::Chuck => "chuck",
        }
    }

    /// How many can sit here at once, and how long a round takes. A slot
    /// machine seats one and spins constantly; a bingo hall seats several
    /// and takes its time.
    /// The inverse of `key`, for reading a saved floor back.
    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.key() == key)
    }

    pub fn seats(self) -> (usize, usize) {
        match self {
            Kind::Slots | Kind::VidPoker | Kind::Scratch | Kind::Crash | Kind::Mines | Kind::Plinko => (1, 1),
            Kind::HiLo => (1, 2),
            Kind::Blackjack | Kind::ThreeCard | Kind::War => (1, 5),
            Kind::Roulette | Kind::Chuck | Kind::Horses => (2, 7),
            Kind::Baccarat => (3, 8),
            Kind::Keno | Kind::Bingo => (2, 6),
            Kind::BigSix => (2, 6),
        }
    }

    pub fn pace(self) -> Duration {
        Duration::from_millis(match self {
            Kind::Slots => 1_400,
            Kind::Scratch => 2_600,
            Kind::VidPoker | Kind::Plinko | Kind::Mines => 3_000,
            Kind::Chuck | Kind::HiLo | Kind::War => 3_400,
            Kind::Blackjack | Kind::ThreeCard => 4_000,
            Kind::Baccarat | Kind::BigSix => 4_600,
            Kind::Roulette => 5_200,
            Kind::Crash | Kind::Horses => 6_000,
            Kind::Keno => 7_000,
            Kind::Bingo => 9_000,
        })
    }

    /// How many bets the table offers, shortest price first. Patrons pick
    /// along this ladder by appetite.
    pub fn variants(self) -> usize {
        match self {
            Kind::Roulette => roulette::BETS,
            Kind::Chuck => chuck::BETS,
            Kind::BigSix => bigsix::BETS,
            Kind::Baccarat => baccarat::BETS,
            Kind::Plinko => plinko::PROFILES,
            Kind::Horses => horses::FIELD,
            Kind::Keno => 10,
            _ => 1,
        }
    }

    /// What the chosen bet is called, for the table view.
    pub fn variant_name(self, i: usize) -> String {
        match self {
            Kind::Roulette => roulette::bet_name(i),
            Kind::Chuck => chuck::bet_name(i),
            Kind::BigSix => bigsix::bet_name(i).to_string(),
            Kind::Baccarat => baccarat::bet_name(i).to_string(),
            Kind::Plinko => format!("{} risk", plinko::profile_name(i)),
            Kind::Horses => format!("runner {}", i + 1),
            Kind::Keno => format!("{} spots", i + 1),
            _ => String::new(),
        }
    }

    /// Plays one round at this table and reports `(staked, returned)` for a
    /// single seat. Every arm here is the table's own audited maths.
    pub fn resolve(self, rng: &mut Rng, variant: usize) -> (i64, i64) {
        match self {
            Kind::Slots => slots::simulate(rng),
            Kind::Roulette => roulette::simulate_at(rng, variant),
            Kind::Blackjack => blackjack::simulate(rng),
            Kind::Baccarat => baccarat::simulate_at(rng, variant),
            Kind::VidPoker => vidpoker::simulate(rng),
            Kind::ThreeCard => threecard::simulate(rng),
            Kind::War => war::simulate(rng),
            Kind::HiLo => hilo::simulate(rng),
            Kind::BigSix => bigsix::simulate_at(rng, variant),
            Kind::Keno => keno::simulate_at(rng, variant + 1),
            Kind::Bingo => bingo::simulate(rng),
            Kind::Plinko => plinko::simulate_at(rng, variant),
            Kind::Mines => mines::simulate(rng),
            Kind::Crash => crash::simulate(rng),
            Kind::Scratch => scratch::simulate(rng),
            Kind::Horses => horses::simulate_at(rng, variant),
            Kind::Chuck => chuck::simulate_at(rng, variant),
        }
    }
}

/// One seat's result in one round, kept for the table view.
#[derive(Debug, Clone)]
pub struct Seat {
    pub name: String,
    pub bet: String,
    pub staked: i64,
    pub returned: i64,
}

/// A finished round.
#[derive(Debug, Clone)]
pub struct RoundLog {
    pub number: u64,
    pub pot: i64,
    pub paid: i64,
    pub seats: Vec<Seat>,
}

impl RoundLog {
    /// What the house took, in chips.
    pub fn house(&self) -> i64 {
        self.pot - self.paid
    }
}

/// How many finished rounds a table remembers. Enough to fill the view,
/// bounded so a table left running all night cannot grow without limit.
/// What a table lets people bet, and therefore who is allowed to sit at it.
///
/// A high-limit table is the whole reason a tier is worth earning: it is
/// the one concrete privilege the house hands out, and it is the reason a
/// whale's stack has anywhere to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Open to anybody, at the house's ordinary ceiling.
    House,
    /// Reserved for the top tier, at the high ceiling.
    High,
}

impl Limit {
    pub fn label(self) -> &'static str {
        match self {
            Limit::House => "house",
            Limit::High => "high limit",
        }
    }

    pub fn from_label(label: &str) -> Option<Limit> {
        match label {
            "house" => Some(Limit::House),
            "high limit" => Some(Limit::High),
            _ => None,
        }
    }

    /// The bet ceiling this table imposes, whoever is sitting at it.
    pub fn ceiling(self, cfg: &super::config::Config) -> i64 {
        match self {
            Limit::House => cfg.max_bet,
            Limit::High => cfg.vip_max_bet,
        }
    }

    /// Whether somebody of this tier is welcome here.
    pub fn admits(self, tier: usize, cfg: &super::config::Config) -> bool {
        match self {
            Limit::House => true,
            Limit::High => cfg.is_vip(tier),
        }
    }
}

/// Scales a table's unit-stake payout to what the patron actually put down.
///
/// Rounded to the nearest chip, not truncated. Truncating looks harmless
/// and is not: it shaves a fraction off *every* winning settlement and
/// never off a losing one, so it quietly adds to the house edge on top of
/// the one `games::audit` measured — most at the small stakes where a whole
/// chip is a large share of the bet. Rounding is symmetric, so the audited
/// price is the price the patron actually gets.
fn scale_payout(unit_staked: i64, unit_returned: i64, stake: i64) -> i64 {
    if unit_staked <= 0 {
        return 0;
    }
    (unit_returned * stake + unit_staked / 2) / unit_staked
}

const HISTORY: usize = 12;

#[derive(Debug, Clone)]
pub struct Instance {
    pub id: u32,
    pub kind: Kind,
    pub name: String,
    /// Who is sitting here, by roster id. The people themselves belong to
    /// the floor's roster — a table holds seats, not persons, which is what
    /// lets somebody get up without ceasing to exist.
    pub patrons: Vec<u64>,
    pub round: u64,
    pub paused: bool,
    /// When the next round falls due, in *simulated* time since the doors
    /// opened. The manager compares this against the casino clock rather
    /// than counting ticks, so a table keeps its own pace no matter how
    /// often the manager happens to run — or how fast time is running.
    pub next_at: Duration,
    pub history: Vec<RoundLog>,
    pub staked: i64,
    pub returned: i64,
    /// Simulated time at which this table opened.
    pub opened: Duration,
    /// Simulated time at which this table last settled a round. Stamped
    /// when the round happens rather than worked out afterwards from
    /// `next_at`, so "quiet for a minute" means what it says even on a
    /// table that has been paused, resumed, or had its pace changed.
    pub last_at: Duration,
    /// Patrons who have come and gone since the table opened.
    pub seen: u32,
    /// How many seats the table currently wants filled.
    wanted: usize,
    /// Which one of its kind this is.
    number: u32,
    /// What this table lets people bet, and who it lets sit down.
    pub limit: Limit,
}

impl Instance {
    /// Opens a table with a stated limit. A high-limit table names itself
    /// as one, because the whole point is that people can see it.
    pub fn open_with(id: u32, kind: Kind, number: u32, limit: Limit, sim: &mut Sim) -> Instance {
        let (lo, hi) = kind.seats();
        let seats = lo + sim.rng.below(hi - lo + 1);
        let now = sim.now;
        let mut inst = Instance {
            id,
            kind,
            name: match limit {
                Limit::House => format!("{} #{number}", kind.label()),
                Limit::High => format!("{} #{number} ★", kind.label()),
            },
            patrons: Vec::new(),
            round: 0,
            paused: false,
            // Stagger the first round so a batch of new tables does not
            // resolve in lockstep for the rest of the night.
            next_at: now + Duration::from_millis(sim.rng.below(kind.pace().as_millis() as usize) as u64),
            history: Vec::new(),
            staked: 0,
            returned: 0,
            opened: now,
            last_at: now,
            seen: 0,
            wanted: seats,
            number,
            limit,
        };
        // Seats are filled by the floor, not by the table: who sits where
        // is a decision about people, and people are the floor's business.
        inst.wanted = seats;
        sim.emit(Weight::Notable, Event::TableOpened { table: id, name: inst.name.clone() });
        inst
    }

    /// Which one of its kind this is — `Roulette #3` is number three.
    /// Kept so a reopened casino carries on numbering rather than starting
    /// again at one.
    pub fn number(&self) -> u32 {
        self.number
    }

    /// How many seats this table would like filled. Re-rolled as people
    /// come and go so a table breathes rather than sitting at a fixed size.
    pub fn wanted(&self) -> usize {
        self.wanted
    }

    pub fn reconsider_size(&mut self, rng: &mut Rng) {
        let (lo, hi) = self.kind.seats();
        self.wanted = lo + rng.below(hi - lo + 1);
    }

    /// Puts somebody in a seat. The caller has already bought their chips
    /// and told the roster where they are.
    pub fn sit(&mut self, id: u64) {
        if !self.patrons.contains(&id) {
            self.patrons.push(id);
            self.seen += 1;
        }
    }

    /// Takes somebody out of a seat.
    pub fn stand(&mut self, id: u64) {
        self.patrons.retain(|p| *p != id);
    }

    /// Plays one round. Every seat stakes, the table's own maths settles
    /// it, and the house books each bet separately.
    ///
    /// The people are borrowed out of the roster a seat at a time. A seat
    /// whose patron has gone missing from the roster is simply skipped —
    /// the floor's lifecycle pass will tidy the seat up — because a table
    /// must never be the thing that decides somebody has stopped existing.
    pub fn play_round(&mut self, sim: &mut Sim) {
        self.round += 1;
        let variants = self.kind.variants();
        let mut seats: Vec<Seat> = Vec::with_capacity(self.patrons.len());
        let (mut pot, mut paid) = (0i64, 0i64);
        let mut settlements: Vec<Event> = Vec::new();

        let Sim { rng, bank, feed, cfg, roster, now } = sim;
        for id in self.patrons.iter() {
            let Some(p) = roster.get_mut(*id) else { continue };
            // The table's limit is a configured ceiling, raised for the top
            // tier — which is what a tier is actually *for*. Lifetime
            // turnover, not the stack in front of them, decides the tier.
            // Two ceilings apply and the lower one wins: what the table
            // allows, and what the patron's own standing allows.
            let ceiling = self.limit.ceiling(cfg).min(cfg.bet_ceiling(p.tier(cfg)));
            let stake = p.stake(rng, cfg.min_bet).min(ceiling.max(cfg.min_bet));
            if stake <= 0 {
                continue;
            }
            let variant = p.pick_bet(rng, variants);
            let (unit_staked, unit_returned) = self.kind.resolve(rng, variant);
            let returned = scale_payout(unit_staked, unit_returned, stake);
            p.settle(stake, returned);
            bank.settle(self.kind.key(), stake, returned);
            pot += stake;
            paid += returned;
            let bet = self.kind.variant_name(variant);
            settlements.push(Event::Settled {
                patron: p.id,
                who: p.name.clone(),
                table: self.id,
                table_name: self.name.clone(),
                bet: bet.clone(),
                staked: stake,
                returned,
            });
            seats.push(Seat { name: p.name.clone(), bet, staked: stake, returned });
        }

        self.staked += pot;
        self.returned += paid;
        bank.count_round();
        // Each settlement is published at whatever weight its size earns,
        // so the ordinary ones stay counted-but-unseen and only the ones
        // worth reading reach the feed.
        for e in settlements {
            let weight = match &e {
                Event::Settled { staked, returned, .. } => super::event::settlement_weight(
                    *staked,
                    *returned,
                    cfg.big_win,
                    cfg.huge_win,
                    cfg.jackpot_multiple,
                ),
                _ => Weight::Routine,
            };
            feed.push(*now, weight, e);
        }
        feed.push(*now, Weight::Routine, Event::Round { table: self.id, number: self.round, pot, paid });
        self.history.push(RoundLog { number: self.round, pot, paid, seats });
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.last_at = *now;
        self.next_at += self.kind.pace();
    }

    /// The table's own take since it opened, in chips.
    pub fn house_take(&self) -> i64 {
        self.staked - self.returned
    }

    pub fn last_round(&self) -> Option<&RoundLog> {
        self.history.last()
    }

    pub fn status(&self) -> &'static str {
        if self.paused {
            "paused"
        } else if self.patrons.is_empty() {
            "seating"
        } else {
            "running"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casino::bank::Bank;
    use crate::casino::config::Config;
    use crate::casino::event::Feed;
    use crate::casino::roster::Roster;

    fn probe() -> Rng {
        Rng::from_seed(404)
    }

    /// A floor's worth of services, standing in for the manager's, so a
    /// table can be driven in isolation exactly as the real one drives it.
    ///
    /// It also carries the small part of the floor's lifecycle these tests
    /// need — seating people — because since the roster phase a table does
    /// not seat anybody itself. That is the point being preserved: an
    /// instance plays rounds; the floor decides who is at it.
    struct Bench {
        rng: Rng,
        bank: Bank,
        feed: Feed,
        cfg: Config,
        roster: Roster,
        now: Duration,
    }

    impl Bench {
        fn new(chips: i64) -> Bench {
            Bench {
                rng: probe(),
                bank: Bank::new(1_000_000, chips),
                feed: Feed::new(4_096),
                cfg: Config::default(),
                roster: Roster::new(),
                now: Duration::ZERO,
            }
        }

        fn sim(&mut self) -> Sim<'_> {
            Sim {
                rng: &mut self.rng,
                bank: &mut self.bank,
                feed: &mut self.feed,
                cfg: &self.cfg,
                roster: &mut self.roster,
                now: self.now,
            }
        }

        /// Fills a table to the number of seats it wants.
        fn fill(&mut self, inst: &mut Instance) {
            while inst.patrons.len() < inst.wanted() {
                let id = self.roster.mint(&mut self.rng);
                let p = self.roster.get_mut(id).expect("just minted");
                let dollars = p.buy_in(&mut self.rng, &self.cfg);
                let chips = self.bank.buy_in(dollars);
                p.begin_visit(chips);
                self.roster.seat(id, inst.id);
                inst.sit(id);
            }
        }

        /// Opens a table and seats it, as the floor would.
        fn open(&mut self, id: u32, kind: Kind) -> Instance {
            let mut inst = Instance::open_with(id, kind, 1, Limit::House, &mut self.sim());
            self.fill(&mut inst);
            inst
        }

        /// Every chip in the building: the house tray plus every stack.
        fn all_chips(&self) -> i64 {
            self.bank.chips() + self.roster.chips_in_play()
        }
    }

    #[test]
    fn every_kind_names_and_keys_itself_distinctly() {
        let mut labels: Vec<&str> = Kind::ALL.iter().map(|k| k.label()).collect();
        let mut keys: Vec<&str> = Kind::ALL.iter().map(|k| k.key()).collect();
        labels.sort_unstable();
        keys.sort_unstable();
        let n = Kind::ALL.len();
        labels.dedup();
        keys.dedup();
        assert_eq!(labels.len(), n, "two kinds share a label");
        assert_eq!(keys.len(), n, "two kinds share a stats key");
    }

    #[test]
    fn every_kind_and_limit_can_be_read_back_from_what_it_writes() {
        // Persistence depends on this exactly: a saved floor is a list of
        // keys, and a key that cannot be parsed back is a table that
        // quietly vanishes when the casino reopens.
        for k in Kind::ALL {
            assert_eq!(Kind::from_key(k.key()), Some(k), "{} does not survive a round trip", k.label());
        }
        assert_eq!(Kind::from_key("not-a-game"), None);
        for l in [Limit::House, Limit::High] {
            assert_eq!(Limit::from_label(l.label()), Some(l));
        }
        assert_eq!(Limit::from_label("velvet"), None);
    }

    #[test]
    fn every_kind_can_actually_play_a_round() {
        let mut rng = probe();
        for kind in Kind::ALL {
            for v in 0..kind.variants() {
                let (staked, returned) = kind.resolve(&mut rng, v);
                assert!(staked > 0, "{} staked nothing on bet {v}", kind.label());
                assert!(returned >= 0, "{} returned {returned} on bet {v}", kind.label());
            }
        }
    }

    #[test]
    fn every_seat_layout_is_sane() {
        for kind in Kind::ALL {
            let (lo, hi) = kind.seats();
            assert!(lo >= 1 && hi >= lo, "{} seats {lo}-{hi}", kind.label());
            assert!(kind.pace().as_millis() > 0);
        }
    }

    #[test]
    fn a_new_table_wants_a_sensible_number_of_seats_and_fills_them() {
        let mut b = Bench::new(10_000_000);
        for (i, kind) in Kind::ALL.iter().enumerate() {
            let inst = b.open(i as u32, *kind);
            let (lo, hi) = kind.seats();
            assert!(inst.wanted() >= lo && inst.wanted() <= hi, "{} wants {} seats", kind.label(), inst.wanted());
            assert_eq!(inst.patrons.len(), inst.wanted());
            assert_eq!(inst.round, 0);
            assert_eq!(inst.name, format!("{} #1", kind.label()));
        }
    }

    #[test]
    fn chips_are_only_ever_moved_never_created() {
        // The strongest thing that can be said about the whole simulation:
        // every buy-in, every settlement and every cash-out is a transfer,
        // so the tray plus every stack in the room is a constant. If this
        // ever fails, the casino is printing money.
        let mut b = Bench::new(10_000_000);
        let mut inst = b.open(1, Kind::Baccarat);
        let opening = b.all_chips();
        for _ in 0..400 {
            inst.play_round(&mut b.sim());
            assert_eq!(b.all_chips(), opening, "chips appeared or vanished at round {}", inst.round);
        }
    }

    #[test]
    fn a_rounds_log_adds_up() {
        let mut b = Bench::new(1_000_000);
        let mut inst = b.open(1, Kind::Slots);
        inst.play_round(&mut b.sim());
        assert_eq!(inst.round, 1);
        let log = inst.last_round().expect("a round was played");
        assert_eq!(log.house(), log.pot - log.paid);
        assert_eq!(log.pot, log.seats.iter().map(|s| s.staked).sum::<i64>());
        assert_eq!(log.paid, log.seats.iter().map(|s| s.returned).sum::<i64>());
    }

    #[test]
    fn a_table_keeps_only_a_bounded_history() {
        let mut b = Bench::new(10_000_000);
        let mut inst = b.open(1, Kind::Slots);
        for _ in 0..HISTORY * 3 {
            inst.play_round(&mut b.sim());
        }
        assert_eq!(inst.history.len(), HISTORY, "history grew without bound");
        assert_eq!(inst.history.last().unwrap().number, inst.round);
    }

    #[test]
    fn rounds_are_paced_by_the_clock_not_by_tick_count() {
        let mut b = Bench::new(10_000_000);
        let mut inst = b.open(1, Kind::Roulette);
        let first = inst.next_at;
        inst.play_round(&mut b.sim());
        assert_eq!(inst.next_at - first, Kind::Roulette.pace());
    }

    #[test]
    fn the_house_take_is_the_difference_across_every_round() {
        let mut b = Bench::new(10_000_000);
        let mut inst = b.open(1, Kind::Chuck);
        for _ in 0..300 {
            inst.play_round(&mut b.sim());
        }
        assert_eq!(inst.house_take(), inst.staked - inst.returned);
        assert!(inst.staked > 0);
    }

    #[test]
    fn a_table_announces_itself() {
        let mut b = Bench::new(10_000_000);
        let _ = Instance::open_with(7, Kind::Blackjack, 3, Limit::House, &mut b.sim());
        let feed = b.feed.recent(64, Weight::Notable);
        assert!(
            matches!(feed.first().map(|r| &r.event), Some(Event::TableOpened { table: 7, .. })),
            "the table must announce itself when it opens"
        );
    }

    #[test]
    fn a_table_holds_seats_rather_than_people() {
        // Rule 4 at the table level: standing somebody up removes the seat
        // and leaves the person exactly where they were — in the roster.
        let mut b = Bench::new(10_000_000);
        let mut inst = b.open(1, Kind::Baccarat);
        let who = inst.patrons[0];
        let before = b.roster.get(who).expect("held").clone();
        inst.stand(who);
        assert!(!inst.patrons.contains(&who), "the seat is empty");
        let after = b.roster.get(who).expect("still known to the building");
        assert_eq!(after.id, before.id);
        assert_eq!(after.name, before.name);
        assert_eq!(after.archetype, before.archetype);
        // And sitting them back down does not duplicate the seat.
        inst.sit(who);
        inst.sit(who);
        assert_eq!(inst.patrons.iter().filter(|p| **p == who).count(), 1);
    }

    #[test]
    fn a_round_plays_around_a_seat_whose_person_has_gone() {
        // A table must never be the thing that decides somebody has
        // stopped existing; it just skips the seat and plays on.
        let mut b = Bench::new(10_000_000);
        let mut inst = b.open(1, Kind::Slots);
        inst.sit(999_999); // an id the roster has never heard of
        inst.play_round(&mut b.sim());
        assert_eq!(inst.round, 1, "one missing person stopped the whole table");
        let log = inst.last_round().unwrap();
        assert!(log.seats.len() < inst.patrons.len(), "the phantom seat should have been skipped");
    }

    #[test]
    fn a_patrons_record_survives_every_round_they_play() {
        let mut b = Bench::new(10_000_000);
        let mut inst = b.open(1, Kind::Roulette);
        let who = inst.patrons[0];
        for _ in 0..40 {
            inst.play_round(&mut b.sim());
        }
        let p = b.roster.get(who).expect("still known");
        assert!(p.lifetime.rounds > 0, "nothing accumulated over forty rounds");
        assert_eq!(p.lifetime.staked, p.staked, "one visit's turnover should be the lifetime's so far");
        assert_eq!(p.lifetime.visits, 1);
    }

    #[test]
    fn the_round_traffic_never_crowds_out_the_readable_feed() {
        let mut b = Bench::new(10_000_000);
        let mut inst = b.open(1, Kind::Slots);
        for _ in 0..500 {
            inst.play_round(&mut b.sim());
        }
        assert!(b.feed.seen(Weight::Routine) >= 500);
        for r in b.feed.recent(4_096, Weight::Notable) {
            assert!(!matches!(r.event, Event::Round { .. }), "per-round traffic reached the feed");
        }
    }

    #[test]
    fn scaling_a_payout_rounds_rather_than_shaving() {
        // A table's maths is written at a unit stake and scaled to what the
        // patron actually bet. Truncating that scaling would shave a
        // fraction off every winning settlement and none off a losing one —
        // a house edge nobody audited and nobody wrote down.
        //
        // Exact where it divides evenly:
        assert_eq!(scale_payout(100, 200, 37), 74);
        assert_eq!(scale_payout(1, 2, 500), 1_000);
        assert_eq!(scale_payout(100, 0, 37), 0, "a losing bet returns nothing at any stake");

        // Nearest, not down, where it does not. Baccarat's banker bet pays
        // 1.95 on a unit of 100, so a stake of 7 is worth 13.65 chips.
        assert_eq!(scale_payout(100, 195, 7), 14);
        assert_eq!(scale_payout(100, 195, 3), 6, "5.85 rounds to 6, not down to 5");
        assert_eq!(scale_payout(100, 101, 5), 5, "5.05 rounds to 5");

        // And over a spread of stakes the rounding is symmetric: the total
        // paid tracks the exact figure instead of sitting under it.
        let (unit_staked, unit_returned) = (100i64, 96i64);
        let mut paid = 0i64;
        let mut exact = 0i64;
        for stake in 5..500 {
            paid += scale_payout(unit_staked, unit_returned, stake);
            exact += unit_returned * stake;
        }
        let drift = paid * unit_staked - exact;
        assert!(drift.abs() * 1_000 < exact, "the scaling drifts by {drift} against an exact {exact}");
        assert!(drift >= 0 || drift.abs() < exact / 1_000, "the drift is one-sided, which is a hidden edge");
    }

    #[test]
    fn a_background_table_holds_what_a_casino_table_holds() {
        // Not a check on any one game's price — `games::audit` does that
        // properly, over hundreds of thousands of rounds. This is the
        // cruder property that matters here: a table run by the simulation,
        // with people choosing their own bets, ends up holding a few
        // percent of the handle rather than half of it or none of it.
        let mut b = Bench::new(1_000_000_000);
        b.cfg.max_bet = 1_000_000;
        b.cfg.vip_max_bet = 1_000_000;
        let mut inst = b.open(1, Kind::Baccarat);
        for _ in 0..20_000 {
            // Keep the seats solvent, so this is about pricing rather than
            // about people busting out and being replaced.
            for id in inst.patrons.clone() {
                if let Some(p) = b.roster.get_mut(id) {
                    p.chips = 10_000;
                    p.bought = 10_000;
                }
            }
            inst.play_round(&mut b.sim());
        }
        let hold = (inst.staked - inst.returned) * 10_000 / inst.staked;
        assert!(inst.staked > 100_000, "not enough turnover to say anything: {}", inst.staked);
        assert!((0..1_000).contains(&hold), "a realised hold of {hold} hundredths of a percent is not a casino");
    }

    #[test]
    fn a_high_limit_table_says_so_and_lets_people_bet_like_it() {
        let cfg = Config::default();
        assert_eq!(Limit::House.ceiling(&cfg), cfg.max_bet);
        assert_eq!(Limit::High.ceiling(&cfg), cfg.vip_max_bet);
        assert!(Limit::High.ceiling(&cfg) > Limit::House.ceiling(&cfg));

        // ...and it only seats the top tier, which is the one concrete
        // thing a tier actually buys.
        for tier in 0..cfg.vip_tier {
            assert!(Limit::House.admits(tier, &cfg), "the house table turned somebody away");
            assert!(!Limit::High.admits(tier, &cfg), "tier {tier} got into the high-limit room");
        }
        for tier in cfg.vip_tier..=cfg.top_tier() {
            assert!(Limit::High.admits(tier, &cfg), "tier {tier} was turned away from its own room");
        }

        let mut b = Bench::new(10_000_000);
        let inst = Instance::open_with(1, Kind::Baccarat, 2, Limit::High, &mut b.sim());
        assert!(inst.name.contains('★'), "a high-limit table should be visible as one: {}", inst.name);
        assert_eq!(inst.limit, Limit::High);
    }

    #[test]
    fn a_table_holds_whichever_limit_is_lower_its_own_or_the_patrons() {
        // A guest sitting at a high-limit table — which the floor would not
        // normally allow — still only gets the guest ceiling, and a whale
        // at an ordinary table only gets the house one. The lower of the
        // two always wins, so neither can be used to smuggle the other up.
        let mut b = Bench::new(1_000_000_000);
        b.cfg.max_bet = 50;
        b.cfg.vip_max_bet = 100_000;
        let mut inst = Instance::open_with(1, Kind::Roulette, 1, Limit::High, &mut b.sim());
        b.fill(&mut inst);
        for id in inst.patrons.clone() {
            let p = b.roster.get_mut(id).unwrap();
            p.chips = 10_000_000;
            p.nerve = 100;
            // A guest: no lifetime turnover at all.
            p.lifetime.staked = 0;
        }
        for _ in 0..30 {
            inst.play_round(&mut b.sim());
            for seat in inst.last_round().unwrap().seats.iter() {
                assert!(seat.staked <= 50, "{} bet {} at a guest's ceiling of 50", seat.name, seat.staked);
            }
        }
    }

    #[test]
    fn nobody_bets_over_the_house_limit() {
        let mut b = Bench::new(100_000_000);
        b.cfg.max_bet = 40;
        b.cfg.vip_max_bet = 40;
        let mut inst = b.open(1, Kind::Roulette);
        for id in inst.patrons.clone() {
            let p = b.roster.get_mut(id).unwrap();
            p.chips = 10_000_000;
            p.nerve = 100;
        }
        for _ in 0..50 {
            inst.play_round(&mut b.sim());
            for s in inst.last_round().unwrap().seats.iter() {
                assert!(s.staked <= 40, "{} got {} down over a limit of 40", s.name, s.staked);
            }
        }
    }
}
