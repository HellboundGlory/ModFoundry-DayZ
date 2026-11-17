# Changelog

What changed in each ModFoundry release, newest first. A release is a `## X.Y.Z - date` section (the date, or `unreleased`);
under it come `### Added`, `### Changed`, `### Fixed` and `### Removed`, one plain line per change, naming the command or
diagnostic code it touches.

## 2.2.0 - 2026-11-17

The first public release.

### Added

- `mf deps list|add|remove <mod> <dependency>`: save the dependency mods of a mod (a mod folder, or a Workshop id)
  that `mf lint`, `mf layout check` and the game check use. A Workshop item is listed under the name its `meta.cpp`
  declares, and `remove` takes it as listed (`workshop:<id>`). A saved list that cannot be read is refused, not
  shown as empty. Over MCP, `project.dependencies` lists them and `project.edit_dependencies` changes them.
- `mf update` installs the newest release over a release install: it downloads the package, checks its sha256 and
  swaps the files, keeping the old ones until the new `mf` starts. `mf update --check` (MCP `host.update_check`, after
  a saved `network` rule) only asks, and says so when no release is published yet. Over MCP, installing an update is
  never offered.
- A release install checks for a newer release once a day in the background, giving up after 2 seconds; a newer one
  is named in one stderr line, in `mf doctor` (`host.doctor`'s `update`) and in the MCP server's instructions.
  `MF_NO_UPDATE_CHECK=1` turns it and `mf update` off.
- Release packages for Linux x86_64: a `.tar.gz` with `mf`, `modfoundry-mcp-server`, the Workshop helper, the
  `modfoundry` agent skill and `install.sh`, which asks which agents to register the MCP server with (Claude Code for
  all projects or one, Codex, others, the `~/.agents` folder), or takes `--target` and `--yes`.

### Changed

- `mf lint --help` says dependencies are saved with `mf deps add`, and describes its path argument.
