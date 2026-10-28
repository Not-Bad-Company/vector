//! strata-core: everything in strata that is not drawing.

pub mod command;
pub mod edit;
pub mod entry;
pub mod git;
pub mod icons;
pub mod model;
pub mod search;
pub mod selection;
pub mod sort;
pub mod theme;

#[cfg(test)]
mod test_support;

/// Program name, shown in titles and used for config paths. Rename here and in Cargo.toml.
pub const APP_NAME: &str = "strata";
