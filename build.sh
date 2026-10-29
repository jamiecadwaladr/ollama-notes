#!/bin/bash
# Builds "Ollama Notes.app" into ./build: the Rust desktop app (src-tauri) with the page (ui) inside.
#   ./build.sh            build
#   ./build.sh --open     build, then launch it
#   ./build.sh --install  build, then copy it to /Applications
#   ./build.sh --server   build only the browser-mode server (target/release/ollama-notes-server)
# Needs Rust (https://rustup.rs) with the wasm32-unknown-unknown and x86_64-apple-darwin targets, and Trunk and the
# Tauri CLI: rustup target add wasm32-unknown-unknown x86_64-apple-darwin; cargo install --locked trunk tauri-cli
set -euo pipefail
cd "$(dirname "$0")"

NAME="Ollama Notes"
APP="build/$NAME.app"

echo "Building the page…"
(cd ui && trunk build --release)

if [[ "${1:-}" == "--server" ]]; then
  cargo build --release -p ollama-notes-server
  echo "Built target/release/ollama-notes-server"
  exit 0
fi

echo "Building the app…"
# a universal binary, so the app runs on any Mac with macOS 14 or later; ad-hoc signed (tauri.conf.json)
cargo tauri build --target universal-apple-darwin --bundles app
mkdir -p build
rm -rf "$APP"
ditto "target/universal-apple-darwin/release/bundle/macos/$NAME.app" "$APP"
echo "Built $APP"

case "${1:-}" in
  --open) open "$APP" ;;
  --install)
    rm -rf "/Applications/$NAME.app"
    ditto "$APP" "/Applications/$NAME.app"
    echo "Installed /Applications/$NAME.app"
    ;;
esac
