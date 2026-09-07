//! Every tunable number the casino simulation runs on, in one place.
//!
//! The rule this file exists to enforce: **no threshold is hard-coded at the
//! point that uses it.** A table limit, a VIP cut-off, the size of a win
//! that is worth telling somebody about, how fast the clock runs, what the
//! lights cost per hour — all of it lives here, is read through a `&Config`
//! held by the floor, and can therefore be changed in one edit or, later,
//! from a settings screen.
//!
//! Money is integers throughout, as everywhere else in this program. Rates
//! that want a fraction are held in permille (`1_000` = 1.0x) so a quarter
//! speed is `250` and no float ever touches a balance.
//!
//! Not every part of this module is called yet: it is the substrate the
//! later phases of the autonomous-simulation roadmap are built on, and the
//! pieces are written and tested together so that the phase that needs them
//! does not also have to invent them. Same justification as `games::cards`.
#![allow(dead_code)]

use std::time::Duration;

/// Speed is a permille multiplier on simulated time: `1_000` is real time.
pub const SPEED_UNIT: u32 = 1_000;

/// The speeds the `s` key cycles through — 0.25x up to 10x, as the roadmap
/// asks. Kept as a list rather than a formula so the ladder is legible and
/// so a user-facing settings screen can present exactly these rungs.
pub const SPEED_LADDER: [u32; 7] = [250, 500, 1_000, 2_000, 4_000, 6_000, 10_000];

/// What a patron has to have put through the tables, in chips, to hold each
/// tier. Lowest tier first; the list must stay ascending.
///
/// Deliberately measured on lifetime turnover rather than on the stack in
/// front of them: a whale who is having a bad night is still a whale, and a
/// grinder who spikes a jackpot is not.
pub const DEFAULT_TIERS: [(&str, i64); 4] =
    [("guest", 0), ("regular", 25_000), ("high roller", 150_000), ("whale", 750_000)];

#[derive(Debug, Clone)]
pub struct Config {
    // ---- clock -------------------------------------------------------
    /// Permille multiplier on simulated time.
    pub speed: u32,
    /// How often the simulation thread wakes. Rounds are paced by the
    /// simulated clock, so this bounds latency, not frequency.
    pub tick: Duration,
    /// A ceiling on rounds one table may catch up in a single tick, so a
    /// stalled process resyncs instead of replaying the night at once.
    pub max_catch_up: u32,

    // ---- table limits (Phase 5) --------------------------------------
    /// Nobody sits down for less than this.
    pub min_bet: i64,
    /// The ceiling on a single bet at an ordinary table.
    pub max_bet: i64,
    /// The ceiling at a table reserved for the top tier. A high limit is
    /// the whole reason a tier is worth having.
    pub vip_max_bet: i64,
    /// Turnover thresholds for each tier, lowest first.
    pub tiers: [(&'static str, i64); 4],
    /// The tier at which the house starts treating somebody as a VIP: the
    /// high bet ceiling, and a seat in the high-limit room.
    ///
    /// Configurable rather than pinned to the top tier, because where the
    /// velvet rope goes is exactly the sort of decision a house changes its
    /// mind about, and pinning it to the top would leave the high-limit
    /// tables empty for most of a night.
    pub vip_tier: usize,

    // ---- the population (Phases 2-4) ---------------------------------
    /// How many named people the world holds. Once it is full, a seat is
    /// filled by somebody coming back rather than by a stranger being
    /// invented — which is what makes the floor a place with regulars, and
    /// what stops an all-night session leaking a person per seat.
    pub roster_size: usize,
    /// How long somebody stays away between visits, in simulated time.
    pub away_for: (Duration, Duration),
    /// How often people walk in, and how many at a time.
    ///
    /// This is what makes occupancy mean anything. If seats were simply
    /// filled the instant they emptied, every table would read as full for
    /// ever and there would be nothing for demand to measure — and no cost
    /// to opening more tables than the room can fill.
    pub arrivals_period: Duration,
    pub arrivals_per_period: usize,
    /// The chance in a hundred, per lifecycle pass, that somebody standing
    /// around with nowhere to sit gives up and goes home. Without this a
    /// room with too few tables silently fills with people who never play.
    pub gives_up: usize,

    // ---- what a patron walks in with ---------------------------------
    /// A buy-in is drawn uniformly from this range, in dollars.
    pub buy_in: (i64, i64),
    /// A top-tier patron buys in for this multiple of an ordinary one.
    pub vip_buy_in_multiple: i64,

    // ---- what the floor is in the mood for (Phase 8) ------------------
    /// How often the room's taste in games shifts.
    pub demand_period: Duration,
    /// How often table occupancy is sampled for the utilization figures.
    pub demand_sample: Duration,
    /// The largest single step the mood takes in one shift, in permille.
    pub appeal_step: i64,
    /// How far out of favour, and into favour, a game can get. Bounded so
    /// no single kind can end up owning the whole floor.
    pub appeal_floor: i64,
    pub appeal_ceiling: i64,

    // ---- event thresholds (Phases 9, 10) -----------------------------
    /// A single settlement returning at least this many chips is notable.
    pub big_win: i64,
    /// ...and at least this many is worth interrupting somebody for.
    pub huge_win: i64,
    /// A win of at least this multiple of the stake is notable regardless
    /// of its size, because that is what a jackpot looks like on a small
    /// bet.
    pub jackpot_multiple: i64,
    /// How many events the feed keeps. Old ones fall off the back.
    pub feed_capacity: usize,

    // ---- house costs (Phase 7) ---------------------------------------
    /// How often operating costs are charged. Periodic, never per frame.
    pub expense_period: Duration,
    /// Fixed cost per period, in dollars, whatever the floor is doing.
    pub overhead_per_period: i64,
    /// Additional cost per period for each open table — staffing it.
    pub table_cost_per_period: i64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            speed: SPEED_UNIT,
            tick: Duration::from_millis(40),
            max_catch_up: 4,

            min_bet: 5,
            max_bet: 2_500,
            vip_max_bet: 50_000,
            tiers: DEFAULT_TIERS,
            vip_tier: 2, // "high roller"


            // Small enough that a night produces regulars — somebody you
            // watched bust out turning up again two tables over is the
            // whole point of the roster, and a cap of several hundred
            // hides it behind an endless supply of strangers.
            roster_size: 120,
            away_for: (Duration::from_secs(60), Duration::from_secs(600)),
            arrivals_period: Duration::from_secs(4),
            arrivals_per_period: 3,
            gives_up: 6,

            buy_in: (20, 200),
            vip_buy_in_multiple: 12,

            demand_period: Duration::from_secs(180),
            demand_sample: Duration::from_secs(20),
            appeal_step: 60,
            appeal_floor: 550,
            appeal_ceiling: 1_600,

            big_win: 1_500,
            huge_win: 15_000,
            jackpot_multiple: 25,
            feed_capacity: 512,

            expense_period: Duration::from_secs(300),
            overhead_per_period: 400,
            table_cost_per_period: 45,
        }
    }
}

impl Config {
    /// The tier a patron's lifetime turnover earns them, as an index into
    /// `tiers`. Turnover, not bankroll — see `DEFAULT_TIERS`.
    pub fn tier_of(&self, lifetime_staked: i64) -> usize {
        let mut tier = 0;
        for (i, (_, need)) in self.tiers.iter().enumerate() {
            if lifetime_staked >= *need {
                tier = i;
            }
        }
        tier
    }

    pub fn tier_name(&self, tier: usize) -> &'static str {
        self.tiers.get(tier).map(|t| t.0).unwrap_or("guest")
    }

    /// The top tier's index.
    pub fn top_tier(&self) -> usize {
        self.tiers.len() - 1
    }

    /// Is somebody of this tier a VIP, as far as the house is concerned?
    pub fn is_vip(&self, tier: usize) -> bool {
        tier >= self.vip_tier.min(self.top_tier())
    }

    /// The bet ceiling for a patron of this tier.
    pub fn bet_ceiling(&self, tier: usize) -> i64 {
        if self.is_vip(tier) { self.vip_max_bet } else { self.max_bet }
    }

    /// The next rung up the speed ladder, wrapping at the top.
    pub fn next_speed(&self) -> u32 {
        let at = SPEED_LADDER.iter().position(|s| *s == self.speed).unwrap_or(2);
        SPEED_LADDER[(at + 1) % SPEED_LADDER.len()]
    }

    /// Speed as it should read on screen: `0.25x`, `1x`, `10x`.
    pub fn speed_label(&self) -> String {
        speed_label(self.speed)
    }
}

/// Renders a permille speed without a trailing `.00` on the whole numbers.
pub fn speed_label(speed: u32) -> String {
    let whole = speed / SPEED_UNIT;
    let frac = speed % SPEED_UNIT;
    if frac == 0 {
        format!("{whole}x")
    } else {
        format!("{}.{:02}x", whole, frac / 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tier_ladder_only_ever_climbs() {
        let cfg = Config::default();
        let mut last = i64::MIN;
        for (_, need) in cfg.tiers.iter() {
            assert!(*need > last || last == i64::MIN, "tiers must be listed in ascending order");
            last = *need;
        }
    }

    #[test]
    fn a_tier_is_earned_by_turnover_not_by_bankroll() {
        let cfg = Config::default();
        assert_eq!(cfg.tier_name(cfg.tier_of(0)), "guest");
        assert_eq!(cfg.tier_name(cfg.tier_of(24_999)), "guest");
        assert_eq!(cfg.tier_name(cfg.tier_of(25_000)), "regular");
        assert_eq!(cfg.tier_name(cfg.tier_of(1_000_000)), "whale");
        // Nothing about the current stack enters into it.
        assert_eq!(cfg.tier_of(750_000), cfg.top_tier());
    }

    #[test]
    fn the_high_limit_belongs_to_the_vip_tier_and_nobody_below_it() {
        let cfg = Config::default();
        assert!(cfg.vip_tier > 0, "everybody being a VIP makes the tier meaningless");
        assert!(cfg.vip_tier <= cfg.top_tier());
        for t in 0..cfg.vip_tier {
            assert!(!cfg.is_vip(t), "tier {t} should not be a VIP");
            assert_eq!(cfg.bet_ceiling(t), cfg.max_bet);
        }
        for t in cfg.vip_tier..=cfg.top_tier() {
            assert!(cfg.is_vip(t));
            assert_eq!(cfg.bet_ceiling(t), cfg.vip_max_bet);
        }
        assert!(cfg.vip_max_bet > cfg.max_bet);
    }

    #[test]
    fn moving_the_velvet_rope_moves_who_gets_the_high_limit() {
        // The whole reason this is configuration and not a constant.
        let cfg = Config { vip_tier: 1, ..Config::default() };
        assert!(cfg.is_vip(1), "a regular should be a VIP once the rope moves");
        assert!(!cfg.is_vip(0));
        let strict = Config { vip_tier: 99, ..Config::default() };
        assert!(strict.is_vip(strict.top_tier()), "the rope can never move past the top tier");
        assert!(!strict.is_vip(strict.top_tier() - 1));
    }

    #[test]
    fn the_speed_ladder_is_a_cycle_that_passes_through_real_time() {
        let mut cfg = Config::default();
        let mut seen = Vec::new();
        for _ in 0..SPEED_LADDER.len() {
            seen.push(cfg.speed);
            cfg.speed = cfg.next_speed();
        }
        assert_eq!(cfg.speed, SPEED_UNIT, "the ladder must come back round to where it started");
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SPEED_LADDER.len(), "every rung is distinct");
        assert!(seen.contains(&SPEED_UNIT));
        assert_eq!(*SPEED_LADDER.first().unwrap(), 250, "the roadmap asks for a quarter speed");
        assert_eq!(*SPEED_LADDER.last().unwrap(), 10_000, "...and for ten times");
    }

    #[test]
    fn speeds_read_the_way_a_person_would_write_them() {
        assert_eq!(speed_label(1_000), "1x");
        assert_eq!(speed_label(10_000), "10x");
        assert_eq!(speed_label(250), "0.25x");
        assert_eq!(speed_label(500), "0.50x");
    }

    #[test]
    fn the_defaults_are_internally_consistent() {
        let cfg = Config::default();
        assert!(cfg.min_bet > 0 && cfg.min_bet < cfg.max_bet);
        assert!(cfg.big_win < cfg.huge_win, "a huge win must clear the bar a big one sets");
        assert!(cfg.buy_in.0 > 0 && cfg.buy_in.0 < cfg.buy_in.1);
        assert!(cfg.roster_size > 0, "a casino with nobody in it is not a casino");
        assert!(cfg.away_for.0 < cfg.away_for.1, "the range somebody stays away must be a range");
        assert!(cfg.away_for.0 > Duration::ZERO, "nobody turns straight round at the door");
        assert!(cfg.arrivals_per_period > 0 && cfg.arrivals_period > Duration::ZERO, "nobody would ever come in");
        assert!(cfg.gives_up > 0 && cfg.gives_up < 100, "people must eventually give up, but not instantly");
        assert!(cfg.appeal_floor < 1_000 && cfg.appeal_ceiling > 1_000, "neutral must sit inside the band");
        assert!(cfg.appeal_step > 0 && cfg.appeal_step < cfg.appeal_ceiling - cfg.appeal_floor);
        assert!(cfg.demand_sample < cfg.demand_period, "occupancy should be sampled more often than the mood moves");
        assert!(cfg.feed_capacity > 0);
        assert!(cfg.expense_period > Duration::ZERO, "costs must be periodic, never per frame");
    }
}
