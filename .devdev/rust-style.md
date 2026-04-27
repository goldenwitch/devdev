# Rust style

- Edition 2024. Use `let ... else`, `if let && ...`, `?` over
  `match err`.
- Errors: `thiserror` for libraries, `anyhow` for binaries. No
  ad-hoc `String` errors at module boundaries.
- Tests live next to the code they cover — `#[cfg(test)] mod tests`
  for unit tests, `tests/` dir for integration tests.
- No `unwrap()` in non-test code unless the invariant is one line
  away.
- Small modules over giant ones. If `mod.rs` exceeds ~400 lines,
  it wants splitting.
- Public items get a one-line doc comment. Internal items only get
  comments when the *why* isn't obvious from the *what*.
