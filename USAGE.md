# Using ModFoundry from an agent

ModFoundry gives an AI agent (or you) the DayZ modding toolchain as one command-line program, `mf`, and an MCP
server, `modfoundry-mcp-server`. Both run the same commands: anything the CLI does, an MCP tool does too, with the same
`MF-*` diagnostic codes.

This page is written for agents. Read it once, then use `mf <command> --help` for the exact flags of the build you
have. `mf changelog` (MCP `host.changelog`) says what changed between versions.

## First calls

1. `mf doctor` (MCP `host.doctor`): finds the DayZ client, DayZ Server, DayZ Tools and the Steam Workshop folder.
   Most commands fall back to what it finds when a path flag is left out.
2. `mf formats` (MCP `formats.list`): the file types ModFoundry reads and writes.
3. Work on a mod: lint it, validate its config, build it.

A mod is any folder. ModFoundry does not impose a layout: it reads Workshop mods, Community Framework style trees and
mods nested under any number of prefix folders.

## Everyday loop for a script mod

```sh
mf new MyMod                      # scaffold a new mod (config.cpp, mod.cpp, a starter script)
                                  # --patch-class and --required-addon set the CfgPatches class and its requiredAddons
mf lint MyMod                     # Enforce Script lint + the compile errors the game itself would print
mf validate MyMod                 # config.cpp checks (MF-C* codes)
mf keygen mykey --dir MyMod/keys  # once: a signing key pair (the private key never leaves your machine)
mf build MyMod --authority mykey  # pack the PBO, sign it, stage MyMod/builds/default/@MyMod
mf check-signatures MyMod/builds/default/@MyMod
mf run-diag MyMod/builds/default/@MyMod --source-root MyMod
                                  # start the Diag server and client with the mod, then map log errors back to files
mf errors <profiles folder>       # read RPT / script.log files and map errors to source lines
```

`mf lint` needs the mod's dependencies to resolve types and to run the game check. Save them once per mod:

```sh
mf deps add MyMod 1559212036          # a Workshop id (all digits), or a mod folder
mf deps list MyMod                    # each row ends with (file), (machine) or (defaults)
mf deps remove MyMod 1559212036       # a Workshop id, a folder path, or the name `list` prints
```

`add` and `remove` write `MyMod/.modfoundry/dependencies.json`, so the list is committed with the mod and every clone,
git worktree or moved folder has it. The file holds Workshop ids and folders relative to the mod:

```json
{ "dependencies": [ { "workshop": "1559212036" }, { "folder": "../MyDependency" } ] }
```

`--machine` changes this machine's own list instead (kept in the host database). A machine row for the same mod
replaces the file's row; the file's rows come first. `mf lint`, `mf layout check` and the game check use both lists
(MCP `project.dependencies` reads them, `project.edit_dependencies` changes one, `where: "machine"` for the machine
list). For one run, `--dependency <mod folder or .pbo>` (repeatable) or `--no-dependencies` overrides them.

Every command that changes files accepts `--preview`: it prints the change-set it would apply and writes nothing.
`mf undo` rolls back the newest recorded change-set.

## CLI commands

| Command | What it does |
|---|---|
| `doctor` | Detect the DayZ game, DayZ Tools and Steam Workshop installs on this machine |
| `formats` | List the file formats ModFoundry recognises |
| `changelog` | Print what changed in each release |
| `new` | Scaffold a new DayZ script mod into a folder |
| `validate` | Validate a `config.cpp` (a file, or a folder holding one) |
| `lint` | Lint Enforce Script, then report the compile errors the game would print for the whole mod |
| `script outline` | Print the parsed outline of Enforce sources as JSON |
| `search` | Search the indexed game scripts, configs, layouts and docs; `search index` builds the index |
| `layout check` | Check `.layout` files against the game's widget classes, properties and files |
| `layout render` | Draw a `.layout` to a PNG without the game (`--size`, `--out`, `--json` for every widget's box) |
| `game-snapshot` | Save a snapshot of a DayZ install for `game-diff` |
| `game-diff` | Compare two DayZ builds and list where mods use what changed |
| `conflict-check` | List what several mods declare or modify in common: scripts, configs and packed files |
| `build` | Pack (and optionally sign) a mod into `builds/<profile>/@Mod`, or every local mod of a launch set (`--set`) |
| `publish` | Build, check and upload a mod to the Steam Workshop (`--preview`: check only and show the plan) |
| `publish-items` | List your published DayZ Workshop items |
| `check-signatures` | Check a built mod's signatures, and optionally a DayZServer's keys |
| `keygen` | Generate a `<authority>.bikey` + `.biprivatekey` signing key pair |
| `run` | Launch DayZServer with a built mod, capture its logs and report errors mapped to files |
| `run-client` | Launch the DayZ client under Proton with local mods staged, capture the logs, report errors |
| `run-diag` | Launch the Diag server (and by default the Diag client), capture both logs, report errors |
| `status` | Say whether a recorded run is still going, and how it ended |
| `errors` | Parse RPT / script.log files and print the errors mapped back to source files |
| `config-bin` | Binarize a `config.cpp` into `config.bin` |
| `derapify` | Convert a `config.bin` back into config text |
| `image` | Convert a PNG, or a folder of them, into `.edds` (UI art) or `.paa` textures |
| `imageset` | Pack a folder of PNGs into one texture and write the `.imageset` |
| `font` | Turn a TTF/OTF font into DayZ `.fnt` + `.edds` files, or one distance-field font |
| `pbo-unpack` | Unpack a PBO, or every PBO of a mod folder, into a folder (refuses to overwrite) |
| `deps` | List, add or remove the saved dependency mods of a mod (`list`, `add`, `remove`) |
| `undo` | Roll back the newest recorded change-set |
| `update` | Install the newest ModFoundry release over a release install; `--check` only asks |
| `agent` | Inspect and manage the agent policy and the audit trail (`audit`, `grant`, `revoke`) |

Most commands also take `--db <PATH>` (the agent database, default `$MODFOUNDRY_DB` or the data folder) and
`--preview`; `mf <command> --help` says which.

## The MCP server

Start it over stdio:

```sh
modfoundry-mcp-server                    # read-only: changing commands only answer with a preview
modfoundry-mcp-server --allow-mutations  # also lets this session change files and launch the game
```

Register it with Claude Code (the install script can do this for you, see [INSTALL.md](INSTALL.md)):

```sh
claude mcp add -s user modfoundry -- ~/.local/bin/modfoundry-mcp-server --allow-mutations
```

Any other MCP client: run `modfoundry-mcp-server` as a stdio server with the same arguments.

Server options:

| Option | Meaning |
|---|---|
| `--allow-mutations` | Let this session run file-changing and launch commands without a saved grant. Destructive, network and publish commands still need an `mf agent grant` rule |
| `--agent-id <ID>` | The agent id this session acts as; policy rules and audit rows use it (default `0`) |
| `--db <PATH>` | The agent database holding the policy and the audit trail |
| `--workspace <DIR>` | Confine `workspace.undo` to changes made under this folder |

Tool names use dots on the wire (`script.lint`). Some clients show them with underscores
(`mcp__modfoundry__script_lint` in Claude Code). Without `--allow-mutations`, a changing tool's description ends with
"(preview only: call it with `preview: true`)".

### MCP tools

| Area | Tools |
|---|---|
| Host | `host.doctor`, `host.changelog`, `formats.list` |
| Scripts | `script.lint` (what `mf lint` prints), `script.check` (the game's own compile errors for the retail server, Diag server and Diag client), `script.diagnostics` (fast MF-E/MF-W lint), `script.outline`, `script.index`, `script.index_status`, `script.find_class`, `script.definition`, `script.hover`, `script.complete`, `script.source` |
| Configs | `config.validate`, `config.get_tree`, `config.patches`, `config.index`, `config.resolve`, `config.source`, `config.edit`, `config.binarize`, `config.derapify` |
| Central Economy | `ce.validate`, `ce.get_tree`, `ce.index`, `ce.reference`, `ce.source`, `ce.coverage`, `ce.edit` |
| Search | `search.index`, `search.query` |
| Layouts and art | `layout.check`, `layout.render`, `image.convert`, `imageset.build`, `font.build` |
| Game updates | `game.snapshot`, `game.diff`, `conflict.check` |
| Files | `vfs.list`, `vfs.read`, `text.edit`, `pbo.unpack`, `workspace.undo` |
| Build and sign | `project.scaffold`, `project.dependencies`, `project.edit_dependencies`, `build.contents`, `build.run`, `sign.keygen`, `sign.check` |
| Run and logs | `diag.run`, `diag.run_client`, `diag.run_diag`, `diag.status`, `diag.stop`, `diag.errors`, `diag.learn_error` |
| Workshop | `publish.details`, `publish.save_details`, `publish.prepare`, `publish.status` |

That is 62 tools with `--allow-mutations`. Without it the server lists 59: `diag.stop`, `publish.prepare` and
`workspace.undo` are left out. Network and publish tools (`workshop.details`, `host.update_check`, `publish.items`,
`publish.upload`, `publish.cancel`) are listed only once a saved rule allows them. Installing an update is never an MCP
tool, whatever the rules say: only `mf update` does it. Each tool's input schema is in `tools/list`; read it there rather than guessing argument
names.

Prompts: `scaffold_mod`, `lint_module`, `diagnose_last_run`. Resources: `modfoundry://host` (detected installs) and
`modfoundry://formats`.

## What an agent is allowed to do

Every command has an effect class: `read`, `index_write`, `mutate`, `destructive`, `launch`, `network`, `publish`.
Reads are always allowed. For an agent, `mutate` and `launch` need `--allow-mutations` or a saved rule;
`destructive`, `network` and `publish` always need a saved rule. Rules live in the agent database and are managed with
`mf agent`:

```sh
mf agent grant 'agent:0' build.run                        # let agent 0 build
mf agent grant '*' '*' --effect publish --decision deny   # no agent may publish
mf agent revoke 'agent:0' build.run
mf agent audit                                            # what agents ran, newest first
```

A rule's decision is `allow` or `deny`. (`ask` needs the desktop app to answer it; the app is not in this release, so
an `ask` rule refuses.) A rule on a command also holds when another command runs it (a deny on
`script.check` also refuses `script.lint`). Every dispatched command is written to the audit trail.

## Diagnostics

Every finding has a namespaced code and the same shape in the CLI and over MCP: `MF-E*` and `MF-W*` script errors and
warnings, `MF-C*` config, `MF-X*` Central Economy, `MF-L*` layouts, `MF-M*` mod conflicts, `MF-P*` build and
publish. The game check (`mf lint`,
`script.check`) reports the errors DayZ itself prints when it compiles the mod, without starting the game.

## Environment variables

| Variable | Meaning |
|---|---|
| `MODFOUNDRY_DB` | The agent database (policy rules and audit trail) |
| `MODFOUNDRY_DATA_DIR` | Override the data folder (default `$XDG_DATA_HOME/modfoundry`) |
| `XDG_CACHE_HOME` | Caches: the search model, unpacked dependencies, game check results |
| `MF_NO_UPDATE_CHECK` | `1` turns off the once-a-day check for a newer release, and `mf update` |

## Good habits for agents

- Lint and validate after every edit; build only when both are clean.
- Use `--preview` (or `preview: true`) before any change you are unsure of, and show the user the change-set.
- Save a mod's dependencies with `mf deps add` before linting it; unknown-type warnings (`MF-W3002`) usually mean one is missing.
- Never publish without the user's word. Publishing needs a saved `publish` rule for exactly this reason.
- Launches start the real game or server; ask before starting one on someone's machine.
