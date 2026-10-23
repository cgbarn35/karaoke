#!/usr/bin/env bash
# Cross-build the Windows GUI (karaoke.exe) from WSL into dist/.
set -euo pipefail
cd "$(dirname "$0")/.."
# Building on the /mnt/c drvfs mount is very slow; keep the target dir on the Linux side.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/karaoke-target}"
cargo build --release --target x86_64-pc-windows-gnu -p karaoke-app
mkdir -p dist
cp "$CARGO_TARGET_DIR/x86_64-pc-windows-gnu/release/karaoke.exe" dist/
echo "built dist/karaoke.exe"
