# Changelog

What changed in each ModFoundry release, newest first. A release is a `## X.Y.Z - date` section (the date, or `unreleased`);
under it come `### Added`, `### Changed`, `### Fixed` and `### Removed`, one plain line per change, naming the command or
diagnostic code it touches.

## 2.3.1 - 2026-11-18

### Fixed

- `mf deps list` (MCP `project.dependencies`) lists every row of a mod's `.modfoundry/dependencies.json`. A mod the file
  names was hidden when this machine's list or the Defaults held the same mod as a test mod only, while `mf deps add`
  said it was already in the file and `mf deps remove` said it was not a dependency. Such a mod now counts as a
  dependency of the mod, so the script index sees it too.
- The `mf deps add` summary for a mod this machine already lists says which row takes its place (`this machine's row
  for X` or `the Defaults row for X`).

## 2.3.0 - 2026-11-18

### Added

- A mod's dependencies can live in the mod: `mf deps add` and `remove` now write `.modfoundry/dependencies.json`
  (Workshop ids, and folders relative to the mod), so every clone, git worktree or moved folder has them. `--machine`
  (MCP `where: "machine"` on `project.edit_dependencies`) keeps this machine's own list as before. `mf lint`, the game
  check, `mf layout check`, the launches and `mf deps list` read both lists; the file's rows come first and a machine
  row for the same mod replaces them. `mf deps list` ends each row with `(file)`, `(machine)` or `(defaults)`
  (`project.dependencies` returns an `origin`).
- `mf deps remove` takes the name `mf deps list` prints, a Workshop id, or a folder path from the working folder or
  relative to the mod.
- `mf layout render <file.layout>` (MCP `layout_render`) draws a `.layout` to a PNG without starting the game: boxes and flat
  colours, `ImageWidget` pictures from the game's imagesets and textures, and text in the game's own fonts, at any
  `--size`. It lists what it cannot draw (`MF-L2xx` hints, with line numbers); `--json` lists every widget's box. Text
  size and placement are approximations. It reads the game's BC7, DXT1 and DXT5 `.edds` textures (the 72-point fonts),
  named images inside imageset groups, a style's default colour, and keeps exact sizes under `fixwidth`; text widgets
  (plain, multiline and rich) are no longer stretched by `fixaspect`. Styled panels and boxes are drawn from the style's images (a tiled centre, edges and
  corners; border sizes are an assumption, `MF-L237`); a style with no image draws nothing, and a widget whose style
  has only other slots (such as `BarCenter`) or no style row keeps its `MF-L210` outline. Text widgets with `size to text` and grid, wrap and plain spacers with `Size To Content` are sized
  from their text and children (the game's exact rule is not measured, so each render with them says so, `MF-L238`). A spacer nested more than 8 deep inside other
  content-sized spacers keeps its written size (`MF-L240`). Spacers that script fills in the game are empty here
  and collapse, so a few vanilla layouts lose placeholder text.
- `MF-E4005`: when the game refuses a module and none of its findings lie on the linted path, `mf lint` and
  `script.lint` report one error naming the module, the builds, and the first compile error or the missing addons,
  instead of saying "ok".
- `mf new --patch-class <name>` sets the `CfgPatches` class, and `--required-addon <name>` (repeatable) adds to its
  `requiredAddons`.

### Changed

- `mf new` refuses a `--patch-class` or `--required-addon` that is not a safe class name.
- A shared dependency file that does not parse is an error in every command that reads it, and `mf deps remove`
  refuses a name two rows share (use `--machine` or the folder path).
- The shared dependency file is written outside the undo stack; the opposite `mf deps` command undoes it.
- `mf layout check` reads a mod's and the game's `.imageset` files with the same strict reader as `mf layout render`, so images inside imageset groups are known; a file the reader refuses is a warning naming the file and the line.
- `mf layout render` writes `<layout name>.render.png` in the current folder when `--out` is not given.
- `mf deps add` and `remove` write the mod's file unless `--machine` is given; before, they wrote this machine's list.
- A new mod's `CfgMods` entry carries a `version` line, and its starter script is `MissionServer.c`.

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
