#!/bin/bash
# The quality gate: everything here must pass before a commit.
set -euo pipefail
cd "$(dirname "$0")/.."
(cd ui && trunk build) # the app and the server embed ui/dist
cargo fmt --all -- --check
cargo clippy --workspace --exclude notes-ui --all-targets -- -D warnings
cargo clippy -p notes-ui --target wasm32-unknown-unknown -- -D warnings
cargo test --workspace --exclude notes-ui
