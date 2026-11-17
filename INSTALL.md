# Installing ModFoundry

Linux x86_64 is the only tested platform. You need your own DayZ install through Steam for anything that reads the
game (types, the game check, runs); building, signing and most checks work on a mod folder alone.

## From a release

1. Download `ModFoundry-<version>-linux-x86_64.tar.gz` and its `.sha256` from the
   [Releases page](https://github.com/HellboundGlory/ModFoundry-DayZ/releases).
2. Check and unpack it:

   ```sh
   sha256sum -c ModFoundry-<version>-linux-x86_64.tar.gz.sha256
   tar xzf ModFoundry-<version>-linux-x86_64.tar.gz
   cd ModFoundry-<version>-linux-x86_64
   ```

3. Install for your user:

   ```sh
   ./install.sh                     # in a terminal, asks where to register the MCP server
   ./install.sh --allow-mutations   # same, registered so agents may change files and launch the game
   ./install.sh --yes               # no questions: every agent found on PATH, plus the ~/.agents folder
   ./install.sh --uninstall         # removes the programs and the user-wide agent entries (your data folder stays)
   ```

   The programs go into `~/.local/lib/modfoundry`, with `mf` and `modfoundry-mcp-server` linked into
   `~/.local/bin`. Run without a terminal (by an agent or a script) and with neither `--yes` nor `--target`, it
   registers nothing and prints the MCP command line instead. Without `--allow-mutations` the server is registered read-only: an agent can look at everything
   and preview changes, but not apply them. `--target` (repeatable) picks the agents without asking; see
   [Connect an agent](#connect-an-agent).

4. Make sure `~/.local/bin` is on your `PATH`, then run `mf doctor`. It lists the DayZ, DayZ Server, DayZ Tools and
   Workshop folders it found.

The package holds `mf`, `modfoundry-mcp-server`, `modfoundry-steam` (the Workshop upload helper) and Valve's
`libsteam_api.so`, which the helper loads. Keep the four files together.

## Connect an agent

`install.sh` registers the MCP server as `modfoundry` with the agents you pick. Its targets, for `--target`:

| Target | What it does |
|---|---|
| `claude-user` | Claude Code, all projects (`claude mcp add -s user`) |
| `claude-project=<dir>` | Claude Code, one project folder only |
| `codex` | Codex |
| `command-code` | Command Code (`command-code mcp add -s user`) |
| `agents` | Adds the server to `~/.agents/mcp.json` and the skill to `~/.agents/skills/modfoundry/SKILL.md` |
| `print` | Prints the command line for any other MCP client |

It only changes a `modfoundry` entry that runs its own program, and `--uninstall` removes only those. A
`claude-project` entry stays: remove it with `claude mcp remove -s local modfoundry` in that folder.

By hand, for Claude Code:

```sh
claude mcp add -s user modfoundry -- ~/.local/bin/modfoundry-mcp-server --allow-mutations
```

Any other MCP client: add a stdio server whose command is `~/.local/bin/modfoundry-mcp-server`, with
`--allow-mutations` if the agent may change files. Clients started before an install keep the old server until they
restart.

Then point the agent at [USAGE.md](USAGE.md).

## Updates

`mf update` installs the newest release over a release install: it downloads the package, checks its sha256 and
swaps the files in `~/.local/lib/modfoundry`. Restart your MCP clients afterwards. `mf update --check` only asks
whether a newer release is out.

Once a day `mf` also checks in the background (it gives up after 2 seconds) and, once a newer release is known, prints one
line on stderr on each run until you update; `mf doctor` and the MCP server's instructions say it too. `MF_NO_UPDATE_CHECK=1` turns off the check and `mf update` both.
A build from source never checks or updates.

## Permissions

Reading is always allowed. Changing files and launching the game need `--allow-mutations` or a saved rule; deleting,
network calls and Workshop uploads always need a saved rule:

```sh
mf agent grant 'agent:0' build.run                        # agent 0 may build without --allow-mutations
mf agent grant '*' '*' --effect publish --decision deny   # no agent may upload to the Workshop
mf agent audit                                            # what agents have run
```

[USAGE.md](USAGE.md) §What an agent is allowed to do has the details.

## Where ModFoundry keeps things

| What | Where |
|---|---|
| Agent database (policy, audit trail, undo history) | `~/.local/share/modfoundry/` (`$MODFOUNDRY_DB` or `--db` to change it) |
| Caches (search model, unpacked dependencies, game check) | `~/.cache/modfoundry/` |
| Search model | `BAAI/bge-small-en-v1.5`, downloaded from Hugging Face on the first `mf search index` |

## Build from source

Needs Rust 1.98 or newer (`rustup update stable`) and a C compiler (SQLite and the Enforce grammar are built from
source).

```sh
git clone https://github.com/HellboundGlory/ModFoundry-DayZ.git
cd ModFoundry-DayZ
cargo build --release -p modfoundry-cli -p modfoundry-mcp-server -p modfoundry-steam
cargo test --workspace
```

The programs land in `target/release/` (`mf`, `modfoundry-mcp-server`, `modfoundry-steam`, and `libsteam_api.so` copied
there by the Steam helper's build). `packaging/linux/build-release.sh` builds the same `.tar.gz` a release ships.
