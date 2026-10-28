//! vector-core: everything in vector that is not drawing.

pub mod archive;
pub mod bookmarks;
pub mod command;
pub mod details;
pub mod du;
pub mod edit;
pub mod entry;
pub mod filter;
pub mod git;
pub mod help;
pub mod icons;
pub mod image;
pub mod keymap;
pub mod listing;
pub mod model;
pub mod ops;
pub mod pins;
pub mod pool;
pub mod rename;
#[cfg(feature = "script")]
pub mod script;
pub mod search;
pub mod selection;
pub mod sort;
pub mod style;
pub mod theme;
pub mod trash;

#[cfg(test)]
mod test_support;

/// Test runs (`VECTOR_NO_LAUNCH`): no other app, terminal or file manager is started, since it
/// would open a window on the user's screen; the app says what it would have opened instead.
pub fn no_launch() -> bool {
    std::env::var_os("VECTOR_NO_LAUNCH").is_some()
}

/// Program name, shown in titles and used for config paths. Rename here and in Cargo.toml.
pub const APP_NAME: &str = "vector";
