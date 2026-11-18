---
name: "modfoundry"
description: "DayZ modding with ModFoundry: lint Enforce Script, validate config.cpp, build, sign and run mods through the mf CLI or the modfoundry MCP tools. Use for any DayZ mod, PBO, Enforce Script, config.cpp or DayZ server task."
license: "MIT"
---
# ModFoundry

ModFoundry gives you the DayZ modding toolchain as one program, `mf`, and an MCP server, `modfoundry-mcp-server`.
Both run the same commands and report the same `MF-*` diagnostic codes. Prefer the MCP tools when they are
connected; otherwise run `mf`.

## Use when

- Writing, linting or checking Enforce Script (`.c`) for a DayZ mod
- Validating a `config.cpp`, the Central Economy files or `.layout` files, and drawing a `.layout` to a PNG (`mf layout render`)
- Packing, signing, publishing or unpacking PBOs
- Starting a Diag server and client with a mod and mapping its log errors back to source lines

## First calls

1. `mf doctor` (MCP `host.doctor`): finds DayZ, DayZ Server, DayZ Tools and the Workshop folder, and says when a
   newer ModFoundry is out.
2. `mf <command> --help` for the exact flags of the installed build; `mf changelog` for what changed.

## Everyday loop

```sh
mf deps add MyMod <dependency mod folder or Workshop id>   # once per dependency
mf lint MyMod          # lint plus the compile errors the game itself would print
mf validate MyMod      # config.cpp checks
mf keygen mykey --dir MyMod/keys   # once: a signing key pair
mf build MyMod --authority mykey   # pack, sign, stage MyMod/builds/default/@MyMod
mf run-diag MyMod/builds/default/@MyMod --source-root MyMod   # Diag server and client, errors mapped to files
```

- Commands that change files take `--preview` (MCP `{"preview": true}`) and are undone with `mf undo` (not `mf deps add` and `remove`: they write `.modfoundry/dependencies.json`, and the opposite command undoes them).
- `mf lint` needs the mod's dependencies: saved with `mf deps`, or `--dependency` / `--no-dependencies`.
- Launches (`run-diag`, `run-server`, `run-client`) start the real game: only run them when the user asks.

## Rules

- Never print, copy or move a `.biprivatekey`.
- Trust the `MF-*` codes over guesses about the game; `mf lint` reports what the game's compiler would say.
- `mf update --check` says whether a newer release exists; `mf update` installs it (MCP clients need a restart).
