#!/usr/bin/env bash
# Build dist/tent-setup: a static installer with the release tent binary embedded.
# Needs: zig 0.15.x on PATH (for libghostty-vt), zstd, rustup target x86_64-unknown-linux-musl.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release
mkdir -p target/dist dist
zstd -19 -T0 -q -f target/release/tent -o target/dist/tent.zst
TENT_PAYLOAD="$PWD/target/dist/tent.zst" cargo build --release \
  --manifest-path installer/Cargo.toml --target x86_64-unknown-linux-musl --target-dir target/installer
cp target/installer/x86_64-unknown-linux-musl/release/tent-setup dist/tent-setup
ls -lh dist/tent-setup
