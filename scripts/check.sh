#!/bin/sh
# Everything a change has to pass before it is committed.
set -eu
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
