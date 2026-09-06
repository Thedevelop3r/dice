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

use super::bank::Bank;
use super::patron::Patron;
use crate::games::{baccarat, bigsix, bingo, blackjack, chuck, crash, hilo, horses, keno, mines, plinko, roulette, scratch, slots, threecard, vidpoker, war};
use crate::rng::Rng;
use std::time::{Duration, Instant};

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
const HISTORY: usize = 12;
/// Nobody sits down for less than this.
pub const MIN_BET: i64 = 5;

#[derive(Debug, Clone)]
pub struct Instance {
    pub id: u32,
    pub kind: Kind,
    pub name: String,
    pub patrons: Vec<Patron>,
    pub round: u64,
    pub paused: bool,
    /// When the next round falls due. The manager compares this against the
    /// clock rather than counting ticks, so a table keeps its own pace no
    /// matter how often the manager happens to run.
    pub next_at: Instant,
    pub history: Vec<RoundLog>,
    pub staked: i64,
    pub returned: i64,
    pub opened: Instant,
    /// Patrons who have come and gone since the table opened.
    pub seen: u32,
}

impl Instance {
    pub fn new(id: u32, kind: Kind, number: u32, rng: &mut Rng, bank: &mut Bank) -> Instance {
        let (lo, hi) = kind.seats();
        let seats = lo + rng.below(hi - lo + 1);
        let now = Instant::now();
        let mut inst = Instance {
            id,
            kind,
            name: format!("{} #{number}", kind.label()),
            patrons: Vec::new(),
            round: 0,
            paused: false,
            // Stagger the first round so a batch of new tables does not
            // resolve in lockstep for the rest of the night.
            next_at: now + Duration::from_millis(rng.below(kind.pace().as_millis() as usize) as u64),
            history: Vec::new(),
            staked: 0,
            returned: 0,
            opened: now,
            seen: 0,
        };
        for _ in 0..seats {
            inst.seat_one(rng, bank);
        }
        inst
    }

    /// Sits a fresh patron down, buying their chips at the cage.
    fn seat_one(&mut self, rng: &mut Rng, bank: &mut Bank) {
        let dollars = 20 + rng.below(180) as i64;
        let chips = bank.buy_in(dollars);
        self.patrons.push(Patron::new(rng, chips));
        self.seen += 1;
    }

    /// Fills empty seats and clears out anyone who is done. Called after a
    /// round so the table stays populated without ever being restarted.
    fn turn_over_seats(&mut self, rng: &mut Rng, bank: &mut Bank) {
        let (lo, hi) = self.kind.seats();
        let mut leaving: Vec<usize> = Vec::new();
        for (i, p) in self.patrons.iter().enumerate() {
            if p.leaving(rng, MIN_BET) {
                leaving.push(i);
            }
        }
        for i in leaving.into_iter().rev() {
            let p = self.patrons.remove(i);
            bank.cash_out(p.chips);
        }
        let want = lo + rng.below(hi - lo + 1);
        while self.patrons.len() < want.max(lo) {
            self.seat_one(rng, bank);
        }
    }

    /// Plays one round. Every seat stakes, the table's own maths settles it,
    /// and the house books each bet separately.
    pub fn play_round(&mut self, rng: &mut Rng, bank: &mut Bank) {
        self.round += 1;
        let variants = self.kind.variants();
        let mut seats: Vec<Seat> = Vec::with_capacity(self.patrons.len());
        let (mut pot, mut paid) = (0i64, 0i64);

        for p in self.patrons.iter_mut() {
            let stake = p.stake(rng, MIN_BET);
            if stake <= 0 {
                continue;
            }
            let variant = p.pick_bet(rng, variants);
            let (unit_staked, unit_returned) = self.kind.resolve(rng, variant);
            // The table's maths is written against a unit stake; scale it to
            // what this patron actually put down.
            let returned = if unit_staked > 0 { unit_returned * stake / unit_staked } else { 0 };
            p.settle(stake, returned);
            bank.settle(self.kind.key(), stake - returned);
            pot += stake;
            paid += returned;
            seats.push(Seat {
                name: p.name.clone(),
                bet: self.kind.variant_name(variant),
                staked: stake,
                returned,
            });
        }

        self.staked += pot;
        self.returned += paid;
        bank.count_round();
        self.history.push(RoundLog { number: self.round, pot, paid, seats });
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.turn_over_seats(rng, bank);
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

    fn probe() -> Rng {
        Rng::from_seed(404)
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
    fn a_new_table_opens_with_someone_at_it() {
        let mut rng = probe();
        let mut bank = Bank::new(1_000_000, 10_000_000);
        for (i, kind) in Kind::ALL.iter().enumerate() {
            let inst = Instance::new(i as u32, *kind, 1, &mut rng, &mut bank);
            let (lo, hi) = kind.seats();
            assert!(inst.patrons.len() >= lo && inst.patrons.len() <= hi, "{} opened with {}", kind.label(), inst.patrons.len());
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
        let mut rng = probe();
        let mut bank = Bank::new(1_000_000, 10_000_000);
        let mut inst = Instance::new(1, Kind::Baccarat, 1, &mut rng, &mut bank);
        let total = |bank: &Bank, inst: &Instance| bank.chips() + inst.patrons.iter().map(|p| p.chips).sum::<i64>();
        let opening = total(&bank, &inst);
        for _ in 0..400 {
            inst.play_round(&mut rng, &mut bank);
            assert_eq!(total(&bank, &inst), opening, "chips appeared or vanished at round {}", inst.round);
        }
    }

    #[test]
    fn a_rounds_log_adds_up() {
        let mut rng = probe();
        let mut bank = Bank::new(100_000, 1_000_000);
        let mut inst = Instance::new(1, Kind::Slots, 1, &mut rng, &mut bank);
        inst.play_round(&mut rng, &mut bank);
        assert_eq!(inst.round, 1);
        let log = inst.last_round().expect("a round was played");
        assert_eq!(log.house(), log.pot - log.paid);
        assert_eq!(log.pot, log.seats.iter().map(|s| s.staked).sum::<i64>());
        assert_eq!(log.paid, log.seats.iter().map(|s| s.returned).sum::<i64>());
    }

    #[test]
    fn a_table_keeps_only_a_bounded_history() {
        let mut rng = probe();
        let mut bank = Bank::new(1_000_000, 10_000_000);
        let mut inst = Instance::new(1, Kind::Slots, 1, &mut rng, &mut bank);
        for _ in 0..HISTORY * 3 {
            inst.play_round(&mut rng, &mut bank);
        }
        assert_eq!(inst.history.len(), HISTORY, "history grew without bound");
        assert_eq!(inst.history.last().unwrap().number, inst.round);
    }

    #[test]
    fn a_table_refills_its_own_seats() {
        let mut rng = probe();
        let mut bank = Bank::new(1_000_000, 10_000_000);
        let mut inst = Instance::new(1, Kind::Baccarat, 1, &mut rng, &mut bank);
        let (lo, _) = Kind::Baccarat.seats();
        for _ in 0..200 {
            inst.play_round(&mut rng, &mut bank);
            assert!(inst.patrons.len() >= lo, "table emptied below its minimum");
        }
        assert!(inst.seen > lo as u32, "nobody ever came or went in 200 rounds");
    }

    #[test]
    fn rounds_are_paced_by_the_clock_not_by_tick_count() {
        let mut rng = probe();
        let mut bank = Bank::new(1_000_000, 10_000_000);
        let mut inst = Instance::new(1, Kind::Roulette, 1, &mut rng, &mut bank);
        let first = inst.next_at;
        inst.play_round(&mut rng, &mut bank);
        assert_eq!(inst.next_at - first, Kind::Roulette.pace());
    }

    #[test]
    fn the_house_take_is_the_difference_across_every_round() {
        let mut rng = probe();
        let mut bank = Bank::new(1_000_000, 10_000_000);
        let mut inst = Instance::new(1, Kind::Chuck, 1, &mut rng, &mut bank);
        for _ in 0..300 {
            inst.play_round(&mut rng, &mut bank);
        }
        assert_eq!(inst.house_take(), inst.staked - inst.returned);
        assert!(inst.staked > 0);
    }
}
