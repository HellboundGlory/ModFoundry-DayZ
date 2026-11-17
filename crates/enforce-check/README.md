# enforce-check

Reports the compile errors DayZ prints for Enforce Script, without starting the game. Every rule was measured by
booting the real game on small probe mods (DayZ 1.30.164289.70); the probes need game data, so they are not published.

- **Formatting is off** (`rustfmt.toml`): the crate keeps its own layout and plain `//` comments.
- **Fixtures** live at the repo root in `tests/fixtures/`, because the tests reach them as `../../tests/fixtures`.
- **Slow checks** need local game data and sit behind `ENFORCE_CHECK_SLOW=1` and `#[ignore]`, for example
  `tests/probes.rs` against the recorded probes in `ENFORCE_CHECK_PROBES`.

After a game update the rules have to be measured again before the checker is trusted on the new version.
