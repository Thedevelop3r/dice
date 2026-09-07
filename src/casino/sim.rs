//! The bundle of floor-owned services a running table needs.
//!
//! A table cannot own its random source, the books, the event feed or the
//! configuration — those belong to the floor, and there is exactly one of
//! each. So they are borrowed in for the length of a call, together, in
//! this struct.
//!
//! Why a struct rather than four parameters: every phase of the roadmap
//! adds another thing a table needs to reach (a transaction log, a demand
//! model, the patron roster). Threading each one through as a new argument
//! would churn every signature and every test on every phase. Adding a
//! field here changes one constructor.
//!
//! The borrows are all distinct fields of `Floor`, so Rust allows them to
//! be taken at once; the construction site is the only place that has to
//! know that.

use std::time::Duration;

use crate::rng::Rng;

use super::bank::Bank;
use super::config::Config;
use super::event::{Event, Feed, Weight};

pub struct Sim<'a> {
    pub rng: &'a mut Rng,
    pub bank: &'a mut Bank,
    pub feed: &'a mut Feed,
    pub cfg: &'a Config,
    /// Simulated time since the doors opened, sampled once per tick. Every
    /// deadline in the simulation is expressed against this.
    pub now: Duration,
    /// The next unused patron id. Identity is minted here rather than
    /// inside a table so that an id is unique across the whole floor and
    /// stays meaningful when a patron moves between tables.
    pub next_patron: &'a mut u64,
}

impl Sim<'_> {
    /// Mints a fresh patron identity.
    pub fn patron_id(&mut self) -> u64 {
        let id = *self.next_patron;
        *self.next_patron += 1;
        id
    }

    /// Publishes an event, stamped with the current simulated time.
    pub fn emit(&mut self, weight: Weight, event: Event) {
        self.feed.push(self.now, weight, event);
    }

    /// Publishes a settled bet at whatever weight its size earns, using the
    /// thresholds from configuration rather than any number written here.
    pub fn emit_settlement(&mut self, event: Event) {
        let weight = match &event {
            Event::Settled { staked, returned, .. } => super::event::settlement_weight(
                *staked,
                *returned,
                self.cfg.big_win,
                self.cfg.huge_win,
                self.cfg.jackpot_multiple,
            ),
            _ => Weight::Routine,
        };
        self.emit(weight, event);
    }
}
