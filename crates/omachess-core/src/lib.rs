//! Domain core for OMACHESS: puzzles, scheduling and progress measurement.
//!
//! This crate is deliberately free of GUI and network dependencies so the
//! scheduling and grading logic — the part that carries the project's core
//! claim about measuring improvement — can be tested in isolation.

pub mod backup;
pub mod clock;
pub mod drill;
pub mod endgame;
pub mod engine;
pub mod game;
pub mod grade;
pub mod ingest;
pub mod openings;
pub mod paths;
pub mod pgn;
pub mod progress;
pub mod puzzle;
pub mod review;
pub mod session;
pub mod store;
pub mod study;

pub use grade::{grade, next_rating, Speed};
pub use puzzle::{Attempt, MoveOutcome, Puzzle};
pub use session::{Outcome, Session, Solve};
pub use store::Store;
