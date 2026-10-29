#!/bin/bash
# Builds "Ollama Notes.app" into ./build with nothing but the Xcode Command Line Tools (xcode-select --install).
#   ./build.sh            build
#   ./build.sh --open     build, then launch it
#   ./build.sh --install  build, then copy it to /Applications
set -euo pipefail
cd "$(dirname "$0")"

NAME="Ollama Notes"
APP="build/$NAME.app"
OBJ="build/obj"
mkdir -p "$OBJ"

echo "Compiling…"
for arch in arm64 x86_64; do # a universal binary, so the app runs on any Mac with macOS 14 or later
  swiftc -O -swift-version 5 -target "$arch-apple-macos14" -o "$OBJ/$arch" macos/Sources/*.swift
done

# The icon is drawn by macos/make-icon.swift, and only redrawn when that changes.
if [[ ! -f "$OBJ/AppIcon.icns" || macos/make-icon.swift -nt "$OBJ/AppIcon.icns" ]]; then
  echo "Drawing the icon…"
  swiftc -O -o "$OBJ/make-icon" macos/make-icon.swift
  rm -rf "$OBJ/AppIcon.iconset"
  "$OBJ/make-icon" "$OBJ/AppIcon.iconset"
  iconutil -c icns -o "$OBJ/AppIcon.icns" "$OBJ/AppIcon.iconset"
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/$NAME" "$OBJ/arm64" "$OBJ/x86_64"
cp macos/Info.plist "$APP/Contents/"
cp "$OBJ/AppIcon.icns" "$APP/Contents/Resources/"
cp -R web "$APP/Contents/Resources/web"
codesign --force --sign - "$APP" # ad-hoc signature: enough to run on this Mac
echo "Built $APP"

case "${1:-}" in
  --open) open "$APP" ;;
  --install)
    rm -rf "/Applications/$NAME.app"
    ditto "$APP" "/Applications/$NAME.app"
    echo "Installed /Applications/$NAME.app"
    ;;
esac
