#!/bin/sh
# Build the herdr-palette binary and put it at bin/herdr-palette.
#
# Herdr runs this as the plugin's [[build]] step on `herdr plugin install`
# (updating is reinstalling). `herdr plugin link` never runs it, so run it
# yourself after cloning or pulling a local checkout.
#
# Requires Rust (https://rustup.rs). Herdr may start this with a thin PATH, so
# the usual cargo locations are added first.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH

if ! command -v cargo >/dev/null 2>&1; then
    echo "herdr-palette: cargo not found. Install Rust from https://rustup.rs and reinstall the plugin." >&2
    exit 1
fi

cargo build --release --locked

# Move into place in one rename, so a popup that is open never sees half a binary.
mkdir -p bin
cp target/release/herdr-palette "bin/herdr-palette.tmp.$$"
mv -f "bin/herdr-palette.tmp.$$" bin/herdr-palette
echo "herdr-palette: installed $ROOT/bin/herdr-palette" >&2
