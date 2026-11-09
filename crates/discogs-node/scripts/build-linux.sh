#!/bin/sh
# Build and test the Linux binary for one napi target inside a Node image of
# the runner's own architecture, so the binary runs where it was built:
#
#   docker run --rm -v "$PWD:/w" -w /w node:22-bullseye \
#     sh crates/discogs-node/scripts/build-linux.sh aarch64-unknown-linux-gnu
#
# Debian bullseye sets the glibc floor at 2.31.
set -eu
TARGET="$1"
apt-get update -qq && apt-get install -y -qq build-essential curl >/dev/null
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
# shellcheck source=/dev/null
. "$HOME/.cargo/env"

corepack enable
COREPACK_ENABLE_DOWNLOAD_PROMPT=0 yarn install --immutable
cd crates/discogs-node
yarn exec napi build --platform --release --strip --js false --target "$TARGET"
yarn run test
