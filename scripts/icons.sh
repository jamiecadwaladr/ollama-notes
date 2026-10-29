#!/bin/bash
# Redraws the app icon (tools/make-icon) and regenerates src-tauri/icons from it.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo run -q --release -p make-icon -- src-tauri/icons/icon.png
cargo tauri icon src-tauri/icons/icon.png -o src-tauri/icons
# keep the desktop sizes only
rm -rf src-tauri/icons/android src-tauri/icons/ios src-tauri/icons/Square*.png src-tauri/icons/StoreLogo.png src-tauri/icons/64x64.png
