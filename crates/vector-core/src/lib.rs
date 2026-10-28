//! vector-core: everything in vector that is not drawing.

pub mod bookmarks;
pub mod command;
pub mod details;
pub mod edit;
pub mod entry;
pub mod filter;
pub mod git;
pub mod icons;
pub mod keymap;
pub mod listing;
pub mod model;
pub mod ops;
pub mod search;
pub mod selection;
pub mod sort;
pub mod theme;

#[cfg(test)]
mod test_support;

/// Program name, shown in titles and used for config paths. Rename here and in Cargo.toml.
pub const APP_NAME: &str = "vector";
