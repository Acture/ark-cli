//! Unity wire formats and execution of an owned Player. No solver verdicts are produced here.
pub mod assets;
pub mod build;
pub mod io;
pub mod player;
#[cfg(windows)]
mod windows_process;

pub const UNITY_VERSION: &str = "2021.3.39f1";
