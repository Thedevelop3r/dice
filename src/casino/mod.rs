//! The casino: a living floor that runs itself.
//!
//! The whole point of this module is the separation the rest of the app
//! does not have. A table here is not a screen you are looking at — it is a
//! simulation the manager is advancing on a clock, which you may or may not
//! happen to be watching. The five pieces map onto that idea directly:
//!
//! - [`config`] — every tunable number: table limits, tier thresholds,
//!   event thresholds, clock speed, what the lights cost. Nothing is
//!   hard-coded at the point that uses it.
//! - [`analytics`] — what the night has looked like: bucketed time ranges
//!   and per-game figures, all of it accumulated, none of it recomputed.
//! - [`clock`] — simulated time. Everything paced reads it, so one speed
//!   setting moves the whole casino together.
//! - [`demand`] — what the floor is in the mood for, and how full its
//!   tables have actually been. Nudges where people sit, never what they
//!   win.
//! - [`event`] — the bus. The simulation announces; screens read. There is
//!   no path by which a reader's code runs on the simulation thread.
//! - [`interest`] — how worth watching a table is. A lens on the floor,
//!   never a hand on it.
//! - [`sim`] — the bundle of floor-owned services a table borrows for the
//!   length of one call.
//! - [`bank`] — the economy manager. One set of books, money and chips,
//!   which every table reports into and nothing else keeps a copy of.
//! - [`patron`] — a simulated player, with an archetype and traits that
//!   change how they bet rather than what the dice do.
//! - [`roster`] — everyone the casino knows, here tonight or not. Tables
//!   hold seat ids; the people themselves live here and outlive any table
//!   they sit at.
//! - [`happening`] — things that happen to the room. Every one of them
//!   moves a rate or a cost; not one of them touches a payout.
//! - [`instance`] — one running table: its seats, its clock, its history.
//! - [`manager`] — the simulation thread. Owns every instance, advances
//!   them, and hands out read-only snapshots.
//! - [`tournament`] — a fixed field playing down to one winner, on its own
//!   clock, using the same maths the cash tables use.
//! - [`ui`] — the screens. They draw snapshots. They never run anything.
//!
//! The rule that keeps it honest: **the UI is a viewer, not a driver.**
//! Nothing under `ui` may advance a simulation, and nothing above `manager`
//! may hold game state.

pub mod analytics;
pub mod bank;
pub mod clock;
pub mod config;
pub mod demand;
pub mod event;
pub mod happening;
pub mod instance;
pub mod interest;
pub mod manager;
pub mod patron;
pub mod roster;
pub mod sim;
pub mod tournament;
pub mod ui;

pub use manager::Manager;
