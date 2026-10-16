//! Original-engine execution in an owned Unity Player. Reports observations, never solver verdicts.
#![deny(unsafe_code)]

mod bundle;
mod environment;
mod events;
mod maa;
mod pipeline;
mod probe;
mod program;
mod proof;
mod runtime;
mod squad;
mod tools;

pub mod comparison;
pub use environment::HostConfig;
pub use pipeline::{Host, LadderSearch, PlayerSource, RunOptions, RunReport};
pub use runtime::Presentation;
pub use squad::{SquadTraining, Training};
