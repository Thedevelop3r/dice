//! The operations centre's read models.
//!
//! Nothing in this file is a source of truth. Every field is a *copy* of a
//! figure some other system already owns: the bank owns the money, the
//! roster owns the people, the feed owns what happened, the tournament
//! owns its own field. This is the shape those figures take on their way
//! to a screen, and nothing more.
//!
//! Why a dedicated set of structs rather than handing the floor's own view
//! to the dashboard: the dashboard asks a different question. The floor
//! screen wants one table in depth; the dashboard wants fifty tables at a
//! glance, the cage, the books and the tournaments *side by side*, and it
//! wants them small enough to copy under the simulation lock without
//! holding it across a render. Building that picture in one pass, from one
//! consistent moment, is the whole point — see
//! [`Manager::dashboard`](super::manager::Manager::dashboard), which is
//! the only thing that constructs these.
//!
//! The rule, stated once: **a dashboard view may be derived from state, and
//! may never be a second copy of it.** If a number here ever disagrees with
//! the bank, the bank is right and this is a bug.

use std::time::Duration;

use super::analytics::Tally;
use super::event::Record;
use super::instance::{Kind, Limit};
use super::reception::Visit;
use super::tournament::{Entrant, Payday, Stage};

/// The strip across the top: what the whole building amounts to right now.
#[derive(Debug, Clone, Default)]
pub struct GlobalView {
    pub money: i64,
    pub chips: i64,
    /// People in the building, and people the casino knows at all.
    pub crowd: usize,
    pub known: usize,
    pub tables: usize,
    pub tourneys: usize,
    /// Permille speed — `1_000` is real time.
    pub speed: u32,
    /// Wall time and simulated time since the doors opened. At anything
    /// but real time these are different numbers, and both are worth
    /// showing.
    pub uptime: Duration,
    pub sim_time: Duration,
    /// Whether the simulation thread is still turning.
    pub running: bool,
    /// The seed the floor's random source is currently running on.
    pub seed: u64,
}

/// One running table, as the dashboard's list needs it.
///
/// Deliberately shallow: no seats, no history, no patrons. A fifty-table
/// floor copies fifty of these, and the moment somebody wants depth the
/// dashboard hands them off to the existing table view rather than
/// carrying the depth around for every row all night.
#[derive(Debug, Clone)]
pub struct GameView {
    pub id: u32,
    pub kind: Kind,
    pub name: String,
    pub round: u64,
    pub seats: usize,
    /// What the table would like to seat — the denominator of its
    /// utilization.
    pub wanted: usize,
    pub status: &'static str,
    pub limit: Limit,
    /// The stake on the most recent settled round, and what the house made
    /// of it.
    pub pot: i64,
    pub house: i64,
    /// Everything ever staked here, and the house's running position.
    pub staked: i64,
    pub take: i64,
    /// How long since this table last settled a round, in simulated time.
    /// A table that has gone quiet is the one an operator wants to find.
    pub idle_for: Duration,
    /// How worth watching it is, in the floor's own words.
    pub why: &'static str,
    pub interest: i64,
}

impl GameView {
    /// Seats filled against seats offered, in permille — the same unit the
    /// demand model reports occupancy in.
    pub fn utilization(&self) -> i64 {
        if self.wanted == 0 { 0 } else { self.seats as i64 * 1_000 / self.wanted as i64 }
    }

    /// Whether this table is one of the house's high-limit ones.
    pub fn high_limit(&self) -> bool {
        self.limit == Limit::High
    }
}

/// Where a casino stands: profitable, losing, or neither.
///
/// Decided by the net cash position and nothing else. There is no
/// threshold here because the architecture has none to borrow — a casino
/// is ahead, behind, or exactly level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Profitable,
    Loss,
    BreakEven,
}

impl Standing {
    pub fn of(ngr: i64) -> Standing {
        match ngr {
            n if n > 0 => Standing::Profitable,
            n if n < 0 => Standing::Loss,
            _ => Standing::BreakEven,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Standing::Profitable => "PROFITABLE",
            Standing::Loss => "LOSS",
            Standing::BreakEven => "BREAK-EVEN",
        }
    }
}

/// The books, summarised. Every figure comes off the bank.
#[derive(Debug, Clone, Default)]
pub struct FinancialView {
    pub money: i64,
    pub chips: i64,
    /// At the tables, in chips.
    pub handle: i64,
    pub payouts: i64,
    pub ggr: i64,
    /// Realised hold, in hundredths of a percent of the handle.
    pub hold: i64,
    pub bets: u64,
    pub rounds: u64,
    /// What the building cost, and the cash left after it.
    pub spent: i64,
    pub ngr: i64,
    pub by_expense: Vec<(&'static str, i64)>,
    /// At the cage, in cash.
    pub bought_in: i64,
    pub cashed_out: i64,
    pub table_profit: i64,
    pub cage_profit: i64,
    /// The games that made and lost the most, best first and worst first.
    pub best_game: Option<(&'static str, i64)>,
    pub worst_game: Option<(&'static str, i64)>,
    /// Every game measured the same way, and the night in slices.
    pub games: Vec<(&'static str, Tally)>,
    /// The handle in each of the last few buckets, oldest first — the
    /// shape of the night.
    pub shape: Vec<i64>,
}

impl FinancialView {
    pub fn standing(&self) -> Standing {
        Standing::of(self.ngr)
    }

    /// Cash in at the counter, less cash out. Positive means the cage is
    /// holding more than it started the flow with.
    pub fn net_cage_flow(&self) -> i64 {
        self.bought_in - self.cashed_out
    }

    /// The average bet, in chips. Derived from two counters, never from a
    /// walk over history.
    pub fn average_bet(&self) -> i64 {
        if self.bets == 0 { 0 } else { self.handle / self.bets as i64 }
    }
}

/// A tournament as the dashboard's list needs it: enough to rank and read,
/// with the field itself fetched only when somebody opens one.
#[derive(Debug, Clone)]
pub struct TournamentView {
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    pub stage: Stage,
    pub round: usize,
    /// Still standing, against everybody who ever entered.
    pub alive: usize,
    pub entered: usize,
    pub pool: i64,
    pub rake: i64,
    pub buy_in: i64,
    pub elapsed: Duration,
    /// Whoever is in front, if anybody is still playing.
    pub leader: Option<(String, i64)>,
    /// The standings, best first — a handful, not the whole field.
    pub board: Vec<Entrant>,
    /// Who got paid what, once it is over.
    pub paid: Vec<Payday>,
    /// Chip stacks across the field: the biggest, the smallest, and the
    /// average of those still in.
    pub biggest_stack: i64,
    pub smallest_stack: i64,
    pub average_stack: i64,
}

impl TournamentView {
    pub fn finished(&self) -> bool {
        self.stage == Stage::Done
    }

    /// How many are still to be knocked out before it is over.
    pub fn eliminated(&self) -> usize {
        self.entered.saturating_sub(self.alive)
    }
}

/// Somebody in the building, as the desk sees them.
#[derive(Debug, Clone)]
pub struct GuestView {
    pub id: u64,
    pub name: String,
    pub style: &'static str,
    pub tier: usize,
    /// Where they are: a table name, a tournament, or the floor.
    pub where_now: String,
    /// Chips they are holding, and the cash they brought to get them.
    pub chips: i64,
    pub bought: i64,
    /// This visit, and every visit they have ever made.
    pub net: i64,
    pub visits: u32,
    pub rounds: u32,
    pub staked: i64,
    pub lifetime_net: i64,
}

impl GuestView {
    /// What their cash stake is worth back at the counter, at the sell
    /// rate. Not a balance — an estimate of one, and labelled as such
    /// wherever it is shown.
    pub fn cash_value(&self) -> i64 {
        self.chips / crate::economy::CHIPS_PER_DOLLAR_SELL
    }
}

/// The front desk: who is in, what has crossed the counter, and the story
/// of the last few minutes of it.
#[derive(Debug, Clone, Default)]
pub struct ReceptionView {
    pub visitors: usize,
    pub entered: u64,
    pub left: u64,
    /// Cash over the counter both ways, and what stayed.
    pub bought_in: i64,
    pub cashed_out: i64,
    /// Chips issued and taken back.
    pub chips_out: i64,
    pub chips_in: i64,
    /// The chips currently out on the floor in people's hands.
    pub chips_in_play: i64,
    pub biggest_buy: (i64, String),
    pub biggest_cash: (i64, String),
    /// The desk's day book, newest last.
    pub activity: Vec<Visit>,
    /// Who is in the building, richest first, capped.
    pub guests: Vec<GuestView>,
}

impl ReceptionView {
    pub fn net_flow(&self) -> i64 {
        self.bought_in - self.cashed_out
    }
}

/// The whole operations centre, in one consistent picture.
///
/// Built in one pass under the simulation lock and drawn after it is
/// released — see [`Manager::dashboard`](super::manager::Manager::dashboard).
/// Nothing in here is fetched lazily, because a half-fetched dashboard
/// would show four sections from four different moments.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub global: GlobalView,
    pub games: Vec<GameView>,
    pub financials: FinancialView,
    /// The events worth a person's attention, oldest first.
    pub events: Vec<Record>,
    pub tournaments: Vec<TournamentView>,
    pub reception: ReceptionView,
    /// What the house calls its tiers, lowest first. Carried so a screen
    /// can name somebody's standing without writing a threshold — or a
    /// name — down itself.
    pub tiers: Vec<&'static str>,
}

impl Snapshot {
    /// The four sections, in the order the dashboard lays them out.
    pub const SECTIONS: [&'static str; 4] = ["LIVE GAMES", "FINANCIALS", "EVENTS & TOURNAMENTS", "RECEPTION / CAGE"];

    /// What the house calls a given standing.
    pub fn tier_name(&self, tier: usize) -> &'static str {
        self.tiers.get(tier).copied().unwrap_or("guest")
    }

    /// Whether a standing is the top one the house recognises.
    pub fn is_top_tier(&self, tier: usize) -> bool {
        !self.tiers.is_empty() && tier + 1 >= self.tiers.len()
    }
}

/// Which part of the dashboard the keyboard is pointed at.
///
/// An explicit enum rather than an index so a section can be added without
/// every match in the drawing code silently doing the wrong thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Games,
    Financials,
    Events,
    Reception,
}

impl Section {
    pub const ALL: [Section; 4] = [Section::Games, Section::Financials, Section::Events, Section::Reception];

    pub fn index(self) -> usize {
        Section::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn title(self) -> &'static str {
        Snapshot::SECTIONS[self.index()]
    }

    pub fn next(self) -> Section {
        Section::ALL[(self.index() + 1) % Section::ALL.len()]
    }

    pub fn prev(self) -> Section {
        Section::ALL[(self.index() + Section::ALL.len() - 1) % Section::ALL.len()]
    }

    /// The digit that jumps straight here, which is also the tab label on
    /// a terminal too small to show all four at once.
    pub fn key(self) -> char {
        char::from_digit(self.index() as u32 + 1, 10).unwrap_or('1')
    }
}

/// How the four sections are arranged, decided by the window rather than
/// assumed.
///
/// There are only two questions to answer — can two panels sit side by
/// side, and is there room for all four at once — so there are only three
/// answers. A terminal that can hold nothing gets tabs, and every section
/// is still reachable from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Two columns, two rows: the whole operations centre at once.
    Grid,
    /// One column, four sections down the page.
    Stacked,
    /// One section at a time, with a tab strip.
    Tabs,
}

impl Layout {
    /// How wide a window must be before two panels can sit beside each
    /// other and both stay readable, and how tall before four panels can
    /// each have enough lines to say anything.
    pub const WIDE: u16 = 104;
    pub const TALL: u16 = 30;
    /// Below this there is not room for four sections at all, however they
    /// are arranged.
    pub const CRAMPED: u16 = 22;

    pub fn for_size(cols: u16, rows: u16) -> Layout {
        if rows < Layout::CRAMPED {
            Layout::Tabs
        } else if cols >= Layout::WIDE && rows >= Layout::TALL {
            Layout::Grid
        } else {
            Layout::Stacked
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_casino_is_ahead_behind_or_exactly_level() {
        assert_eq!(Standing::of(1), Standing::Profitable);
        assert_eq!(Standing::of(-1), Standing::Loss);
        assert_eq!(Standing::of(0), Standing::BreakEven);
    }

    #[test]
    fn the_average_bet_is_two_counters_divided_never_a_walk() {
        let f = FinancialView { handle: 10_000, bets: 40, ..FinancialView::default() };
        assert_eq!(f.average_bet(), 250);
        assert_eq!(FinancialView::default().average_bet(), 0, "no bets must not divide by zero");
    }

    #[test]
    fn the_cage_flow_is_what_came_in_less_what_went_out() {
        let f = FinancialView { bought_in: 128_400, cashed_out: 94_200, ..FinancialView::default() };
        assert_eq!(f.net_cage_flow(), 34_200);
    }

    #[test]
    fn sections_cycle_both_ways_and_come_back_round() {
        assert_eq!(Section::Games.next(), Section::Financials);
        assert_eq!(Section::Reception.next(), Section::Games);
        assert_eq!(Section::Games.prev(), Section::Reception);
        for s in Section::ALL {
            assert_eq!(s.next().prev(), s);
            assert_eq!(s.title(), Snapshot::SECTIONS[s.index()]);
        }
    }

    #[test]
    fn a_table_with_no_seats_offered_is_not_a_division_by_zero() {
        let mut g = GameView {
            id: 1,
            kind: Kind::Slots,
            name: "Slots #1".into(),
            round: 0,
            seats: 0,
            wanted: 0,
            status: "idle",
            limit: Limit::House,
            pot: 0,
            house: 0,
            staked: 0,
            take: 0,
            idle_for: Duration::ZERO,
            why: "ticking over",
            interest: 0,
        };
        assert_eq!(g.utilization(), 0);
        g.wanted = 8;
        g.seats = 4;
        assert_eq!(g.utilization(), 500);
    }
}
