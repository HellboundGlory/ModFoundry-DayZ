#!/usr/bin/env bash
# Builds the CLI, the MCP server and the Steam helper and packs them into
# dist/ModFoundry-<version>-linux-x86_64.tar.gz with its .sha256.
# Runs in the public checkout, where README.md and the other user docs sit at the top.
set -euo pipefail

command -v cargo-about >/dev/null || {
    echo "cargo-about is needed for the licence notices: cargo install --locked --features cli cargo-about@0.9.3" >&2
    exit 1
}

root="$(cd "$(dirname "$0")/../.." && pwd)"
here="$root/packaging/linux"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)"
[[ -n "$version" ]] || { echo "no version line in $root/Cargo.toml" >&2; exit 1; }

# Keeps the builder's home and source folder out of panic messages in the binaries.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~ --remap-path-prefix=$root=."
export CFLAGS="${CFLAGS:-} -ffile-prefix-map=$HOME=~ -ffile-prefix-map=$root=."
cargo build --locked --release --manifest-path "$root/Cargo.toml" \
    -p modfoundry-cli -p modfoundry-mcp-server -p modfoundry-steam

target="${CARGO_TARGET_DIR:-$root/target}/release"
name="ModFoundry-$version-linux-x86_64"
stage="$root/dist/$name"
rm -rf "$stage"
mkdir -p "$stage"
for program in mf modfoundry-mcp-server modfoundry-steam libsteam_api.so; do
    [[ -f "$target/$program" ]] || { echo "missing $program in $target" >&2; exit 1; }
    install -m755 "$target/$program" "$stage/$program"
done
install -m755 "$here/install.sh" "$stage/install.sh"
for doc in README.md INSTALL.md USAGE.md CHANGELOG.md THIRD-PARTY.md LICENSE; do
    install -m644 "$root/$doc" "$stage/$doc"
done
mkdir -p "$stage/licenses"
install -m644 "$root/vendor/tree-sitter-enforce/LICENSE" "$stage/licenses/tree-sitter-enforce.txt"
cargo about generate --locked --fail --workspace --manifest-path "$root/Cargo.toml" \
    -c "$here/about.toml" -o "$stage/licenses/rust-crates.txt" "$here/about.hbs"
mkdir -p "$stage/skill/modfoundry"
install -m644 "$root/packaging/skill/modfoundry/SKILL.md" "$stage/skill/modfoundry/SKILL.md"

tar -C "$root/dist" --owner=0 --group=0 --numeric-owner -czf "$root/dist/$name.tar.gz" "$name"
(cd "$root/dist" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "$root/dist/$name.tar.gz"
