# Third-party notices

ModFoundry's own code is licensed MIT. These parts come from others.

## In the source

- **tree-sitter-enforce** (`vendor/tree-sitter-enforce`): the Enforce Script grammar by Simone Vicinanza, MIT licence,
  kept in `vendor/tree-sitter-enforce/LICENSE`. It is built into the programs, so the release package carries the
  licence too, as `licenses/tree-sitter-enforce.txt`.
- **IBM Plex Mono** (`tests/fixtures/fonts/IBMPlexMono-Regular.ttf`), used only as a test input for `mf font`, SIL
  Open Font Licence 1.1, kept beside it in `OFL.txt`.
- **Rust crates** from crates.io, listed with their versions in `Cargo.lock`. Each keeps its own licence. The release
  package lists them with their licence texts in `licenses/rust-crates.txt`, made with `cargo about` from
  `packaging/linux/about.toml`.

## In the release package

- **Steamworks SDK redistributable** (`libsteam_api.so`), © Valve Corporation, distributed with ModFoundry's Steam
  helper under the [Steamworks SDK Access Agreement](https://partner.steamgames.com/documentation/sdk_access_agreement)
  §1.1(b). It is not covered by ModFoundry's licence. It reaches the build through the `steamworks-sys` crate.

## Downloaded when used

- **BAAI/bge-small-en-v1.5**, the text-embedding model `mf search` uses, downloaded from Hugging Face into your cache on
  first use (MIT licence, per its model card). It is not part of the package.

## The logo

The ModFoundry logo (`assets/logo.png`) is the project's mark. It is not covered by the MIT licence; please
don't reuse it for other projects.

## Not included

No DayZ game files, scripts, configs or art are part of ModFoundry or its releases. Commands that need them read your
own installed copy of the game.
