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
use super::roster::Roster;

pub struct Sim<'a> {
    pub rng: &'a mut Rng,
    pub bank: &'a mut Bank,
    pub feed: &'a mut Feed,
    pub cfg: &'a Config,
    /// Everyone the casino knows. Tables hold ids; the people themselves
    /// live here, so somebody can exist while not seated.
    pub roster: &'a mut Roster,
    /// Simulated time since the doors opened, sampled once per tick. Every
    /// deadline in the simulation is expressed against this.
    pub now: Duration,
}

impl Sim<'_> {
    /// Publishes an event, stamped with the current simulated time.
    pub fn emit(&mut self, weight: Weight, event: Event) {
        self.feed.push(self.now, weight, event);
    }
}
