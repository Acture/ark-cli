//! Asset and revision-pinned snapshot ingestion from user-supplied Arknights files.
//!
//! Typed workflow requests are public; command-line parsing and process exit policy live in
//! `ark-cli`. The legacy Android exporter requires a separately configured ArkUnpacker install.

mod battle_data;
mod extract;
mod fetch;
mod fetch_gamedata;
mod gamedata_snapshot;
mod identify_spine;
mod tooling;
mod walk;

pub use battle_data::{run as extract_battle_data, ExtractBattleDataArgs};
pub use extract::{run as extract, ExtractArgs};
pub use fetch::{run as fetch, FetchArgs};
pub use fetch_gamedata::{run as fetch_gamedata, FetchGameDataArgs, GameDataProfile};
pub use identify_spine::run as identify_spine;
