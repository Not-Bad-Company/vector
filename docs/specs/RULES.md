# Rules for every module

- Rust stable, edition 2024. `cargo clippy -p strata-core -- -D warnings` must be clean.
- No `unwrap()`/`expect()` on anything that touches the file system or user input. Tests may unwrap.
- Touch only the file named in your spec (plus tests inside it, `#[cfg(test)] mod tests`).
  Do not edit Cargo.toml, lib.rs or other modules; if you need a dependency that is not already
  in `crates/strata-core/Cargo.toml`, stop and report instead.
- Doc comments on public items, short and plain. Match the style of `src/sort.rs`.
- Implement the general logic. Never special-case a test input.

## Privacy (hard limits)

- Never list, open, read, search or index anything under `/home/daanh` except this source tree.
- Tests create their own files with `tempfile::tempdir()` (available as a dev-dependency).
- For manual experiments use only `/tmp/claude-1000/strata-test/` (a generated dummy tree).
