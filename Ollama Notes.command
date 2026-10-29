#!/bin/bash
# Double-click this file in Finder (or run it from a terminal) to start Ollama Notes in your browser.
# It starts Ollama if needed, serves the UI, and opens your browser. Ctrl+C (or close this window) stops it.
cd "$(dirname "$0")" || exit 1
BIN="target/release/ollama-notes-server"
if [[ ! -x "$BIN" ]]; then
  if ! command -v cargo >/dev/null; then
    echo "Building the server needs Rust: install it from https://rustup.rs, then run this again."
    read -r -p "Press Enter to close."
    exit 1
  fi
  ./build.sh --server || { read -r -p "The build failed. Press Enter to close."; exit 1; }
fi
exec "$BIN" "$@"
