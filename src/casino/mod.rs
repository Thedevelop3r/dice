//! The casino: a living floor that runs itself.
//!
//! The whole point of this module is the separation the rest of the app
//! does not have. A table here is not a screen you are looking at — it is a
//! simulation the manager is advancing on a clock, which you may or may not
//! happen to be watching. The five pieces map onto that idea directly:
//!
//! - [`bank`] — the economy manager. One set of books, money and chips,
//!   which every table reports into and nothing else keeps a copy of.
//! - [`patron`] — a simulated player, with traits that change how they bet
//!   rather than what the dice do.
//! - [`instance`] — one running table: its seats, its clock, its history.
//! - [`manager`] — the simulation thread. Owns every instance, advances
//!   them, and hands out read-only snapshots.
//! - [`ui`] — the screens. They draw snapshots. They never run anything.
//!
//! The rule that keeps it honest: **the UI is a viewer, not a driver.**
//! Nothing under `ui` may advance a simulation, and nothing above `manager`
//! may hold game state.

pub mod bank;
pub mod instance;
pub mod manager;
pub mod patron;
pub mod ui;

pub use manager::Manager;
