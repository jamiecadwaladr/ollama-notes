#!/bin/bash
# Regenerates crates/core/tests/fixtures/golden.json from the original JS implementation (web/ at the git revision
# given, default: master), using the JavaScriptCore shell that ships with macOS.
#   tools/golden/generate.sh [revision]
set -euo pipefail
cd "$(dirname "$0")/../.."
REV="${1:-master}"
JSC=/System/Library/Frameworks/JavaScriptCore.framework/Versions/A/Helpers/jsc
WEB="$(mktemp -d)"
trap 'rm -rf "$WEB"' EXIT
git archive "$REV" web | tar -x -C "$WEB"
mkdir -p crates/core/tests/fixtures
"$JSC" -m tools/golden/generate.mjs -- "$WEB/web" | python3 -m json.tool --indent 1 > crates/core/tests/fixtures/golden.json
echo "Wrote crates/core/tests/fixtures/golden.json from web/ at $REV"
