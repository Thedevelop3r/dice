//! The casino's own clock.
//!
//! Everything the simulation paces itself by — when a table's next round
//! falls due, how long a patron has been in the building, when the lights
//! bill comes round — is measured against *simulated* time, not wall time.
//! The clock is the only place the two are related, and it relates them
//! through one number: the permille speed from [`Config`](super::config).
//!
//! Two properties matter and are pinned by tests below:
//!
//! - **Simulated time never goes backwards**, whatever the speed is changed
//!   to mid-run. Changing speed changes the *rate* from that instant on; it
//!   never rescales the past. A table that was due in four seconds of
//!   simulated time is still due then.
//! - **A stall does not become a stampede.** If the process is suspended,
//!   the clock still advances by the whole gap — the catch-up ceiling that
//!   keeps rounds sane lives in the manager, where it belongs, not here.
//!
//! Simulated time is a [`Duration`] since the doors opened rather than an
//! [`Instant`], because an `Instant` cannot be made to run at 0.25x and
//! cannot be written to a save file.
//!
//! Not every part of this module is called yet: it is the substrate the
//! later phases of the autonomous-simulation roadmap are built on, and the
//! pieces are written and tested together so that the phase that needs them
//! does not also have to invent them. Same justification as `games::cards`.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use super::config::SPEED_UNIT;

#[derive(Debug, Clone)]
pub struct Clock {
    /// Wall time of the last `advance`.
    last: Instant,
    /// Simulated time since the doors opened.
    elapsed: Duration,
    /// Wall time since the doors opened, for "you have been running for"
    /// figures that should read in real minutes.
    real: Duration,
    speed: u32,
}

impl Clock {
    pub fn new(speed: u32) -> Clock {
        Clock::started_at(Instant::now(), speed)
    }

    /// A clock whose origin is an instant the caller chooses. Taking the
    /// origin as an argument rather than sampling it here is what lets a
    /// test drive the clock from a fixed timeline instead of racing the
    /// machine it is running on.
    pub fn started_at(now: Instant, speed: u32) -> Clock {
        Clock { last: now, elapsed: Duration::ZERO, real: Duration::ZERO, speed: speed.max(1) }
    }

    /// Resumes a saved clock at a given simulated age.
    pub fn resume(elapsed: Duration, speed: u32) -> Clock {
        Clock { last: Instant::now(), elapsed, real: Duration::ZERO, speed: speed.max(1) }
    }

    /// Rolls the clock forward to `now`. Returns the simulated time that
    /// just passed, which is what a periodic system accumulates against.
    pub fn advance(&mut self, now: Instant) -> Duration {
        let real = now.saturating_duration_since(self.last);
        self.last = now;
        self.real += real;
        let simulated = scale(real, self.speed);
        self.elapsed += simulated;
        simulated
    }

    /// Simulated time since the doors opened. This is the value every
    /// deadline in the simulation is expressed against.
    pub fn now(&self) -> Duration {
        self.elapsed
    }

    /// Wall time since the doors opened.
    pub fn real(&self) -> Duration {
        self.real
    }

    pub fn speed(&self) -> u32 {
        self.speed
    }

    /// Changes the rate from `now` on. The past is not rescaled.
    ///
    /// The instant is a parameter for the same reason `started_at` takes
    /// one: the clock must never read the machine's time on its own account
    /// in the middle of an operation a test is trying to pin down.
    pub fn set_speed(&mut self, now: Instant, speed: u32) {
        // Roll the clock to this moment first, so the time already served
        // is credited at the old rate rather than the new one.
        self.advance(now);
        self.speed = speed.max(1);
    }
}

/// `d * speed / SPEED_UNIT`, computed in nanoseconds so a quarter speed is
/// exact rather than rounded to whole milliseconds.
fn scale(d: Duration, speed: u32) -> Duration {
    let nanos = d.as_nanos() * speed as u128 / SPEED_UNIT as u128;
    Duration::from_nanos(nanos.min(u64::MAX as u128) as u64)
}

/// A recurring deadline in simulated time — what "every five minutes of
/// casino time" is made of.
///
/// It answers `due(now)` as many times as periods have actually elapsed, so
/// a caller draining it in a `while` loop charges every period it owes even
/// if it was away for six of them, and charges nothing at all if it was
/// away for none. That is the shape Phase 7 needs: periodic, never per
/// frame, and never silently skipped.
#[derive(Debug, Clone)]
pub struct Every {
    period: Duration,
    next: Duration,
}

impl Every {
    pub fn new(period: Duration, from: Duration) -> Every {
        Every { period: period.max(Duration::from_millis(1)), next: from + period }
    }

    /// Consumes one due period if there is one.
    pub fn due(&mut self, now: Duration) -> bool {
        if now >= self.next {
            self.next += self.period;
            true
        } else {
            false
        }
    }

    /// Drops any backlog, so a long stall does not charge twelve hours of
    /// rent in one tick. Callers that *want* the backlog just don't call it.
    pub fn resync(&mut self, now: Duration) {
        if self.next < now {
            self.next = now + self.period;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock driven by a fake wall time, so the tests are deterministic
    /// rather than dependent on how long the test itself took to run.
    fn tick(c: &mut Clock, at: Instant, ms: u64) -> (Instant, Duration) {
        let next = at + Duration::from_millis(ms);
        let moved = c.advance(next);
        (next, moved)
    }

    #[test]
    fn at_real_time_a_second_of_wall_clock_is_a_second_of_casino() {
        let start = Instant::now();
        let mut c = Clock::started_at(start, SPEED_UNIT);
        let (_, moved) = tick(&mut c, start, 1_000);
        assert_eq!(moved, Duration::from_millis(1_000));
        assert_eq!(c.now(), Duration::from_millis(1_000));
        assert_eq!(c.real(), Duration::from_millis(1_000));
    }

    #[test]
    fn a_quarter_speed_is_exact_rather_than_rounded_away() {
        let start = Instant::now();
        let mut c = Clock::started_at(start, 250);
        let (at, moved) = tick(&mut c, start, 1_000);
        assert_eq!(moved, Duration::from_millis(250));
        // The point of nanosecond scaling: a 40ms tick at 0.25x is 10ms,
        // and a hundred of them add up to exactly a second of wall time.
        let mut at = at;
        for _ in 0..100 {
            let (next, moved) = tick(&mut c, at, 40);
            assert_eq!(moved, Duration::from_millis(10));
            at = next;
        }
        assert_eq!(c.now(), Duration::from_millis(250 + 1_000));
        assert_eq!(c.real(), Duration::from_millis(1_000 + 4_000));
    }

    #[test]
    fn ten_times_speed_is_ten_times_as_much_casino_per_second() {
        let start = Instant::now();
        let mut c = Clock::started_at(start, 10_000);
        let (_, moved) = tick(&mut c, start, 1_000);
        assert_eq!(moved, Duration::from_secs(10));
    }

    #[test]
    fn changing_speed_never_rescales_the_past() {
        let start = Instant::now();
        let mut c = Clock::started_at(start, SPEED_UNIT);
        let (at, _) = tick(&mut c, start, 1_000);
        let before = c.now();
        c.set_speed(at, 10_000);
        assert!(c.now() >= before, "simulated time must never go backwards");
        // The second already served stays worth a second, not ten.
        assert_eq!(c.now(), Duration::from_millis(1_000), "the second already served stays worth a second");
        let (_, moved) = tick(&mut c, at, 1_000);
        assert_eq!(moved, Duration::from_secs(10), "the new rate applies from here on");
    }

    #[test]
    fn simulated_time_only_ever_climbs() {
        let mut at = Instant::now();
        let mut c = Clock::started_at(at, SPEED_UNIT);
        let mut last = c.now();
        for (i, speed) in super::super::config::SPEED_LADDER.iter().cycle().take(30).enumerate() {
            c.set_speed(at, *speed);
            let (next, _) = tick(&mut c, at, 10 + i as u64);
            at = next;
            assert!(c.now() >= last, "the clock ran backwards at {speed}");
            last = c.now();
        }
    }

    #[test]
    fn a_period_fires_once_for_each_period_that_actually_passed() {
        let mut e = Every::new(Duration::from_secs(60), Duration::ZERO);
        assert!(!e.due(Duration::from_secs(59)));
        assert!(e.due(Duration::from_secs(60)));
        assert!(!e.due(Duration::from_secs(60)), "and only once for that one");

        // Away for three and a half periods: three charges owed, then stop.
        let now = Duration::from_secs(60 * 4 + 30);
        let mut fired = 0;
        while e.due(now) {
            fired += 1;
        }
        assert_eq!(fired, 3);
        assert!(!e.due(now));
    }

    #[test]
    fn resync_drops_a_backlog_instead_of_charging_it_all_at_once() {
        let mut e = Every::new(Duration::from_secs(60), Duration::ZERO);
        let now = Duration::from_secs(60 * 100);
        e.resync(now);
        assert!(!e.due(now), "the backlog is gone");
        assert!(e.due(now + Duration::from_secs(60)), "but the period still runs");
    }
}
