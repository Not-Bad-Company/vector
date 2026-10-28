//! strata-core: everything in strata that is not drawing.

pub mod command;
pub mod edit;
pub mod git;
pub mod icons;
pub mod search;
pub mod sort;
pub mod theme;

/// Program name, shown in titles and used for config paths. Rename here and in Cargo.toml.
pub const APP_NAME: &str = "strata";
