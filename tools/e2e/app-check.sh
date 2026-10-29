#!/bin/bash
# Launches the built app on a throwaway data folder and checks, without touching its window:
#   - the page loads in the app and saves through it (a seeded notes.json is read, laid out, and saved back, which
#     also makes the session's notes.backup.json)
#   - the map survives the round trip, unknown fields included
#   - quitting goes through the page's last save (exits well before the 3 s fallback)
# A window opens for a few seconds; don't click it.
#   tools/e2e/app-check.sh ["build/Ollama Notes.app"]
set -euo pipefail
cd "$(dirname "$0")/../.."
APP="${1:-build/Ollama Notes.app}"
BIN="$APP/Contents/MacOS/Ollama Notes"
DATA="$(mktemp -d)"
trap 'rm -rf "$DATA"' EXIT
fail() { echo "FAIL: $*"; exit 1; }

cat > "$DATA/notes.json" <<'JSON'
{"settings":{"model":"qwen3.5:2b","futureSetting":42},"chat":[],"hubId":null,"contextNodeIds":[],"title":"Seeded",
 "graph":{"nodes":[{"id":"a","title":"Bloom filters","body":"Set membership.","kind":"concept","x":0,"y":0,"createdAt":1},
 {"id":"b","title":"Hash functions","body":"Spread keys.","kind":"fact","x":5,"y":5,"createdAt":2,"color":"red"},
 {"id":"c","title":"Merkle trees","body":"Hash trees.","kind":"concept","x":10,"y":0,"createdAt":3}],
 "edges":[{"id":"e1","from":"a","to":"b","label":""}],"view":{"x":0,"y":0,"k":1}}}
JSON

"$BIN" -DataDirectory "$DATA" >/dev/null 2>&1 &
PID=$!
for _ in $(seq 1 100); do [[ -f "$DATA/notes.backup.json" ]] && break; sleep 0.2; done
[[ -f "$DATA/notes.backup.json" ]] || { kill "$PID" 2>/dev/null; fail "the app never saved (page not loaded, or invoke failing)"; }
echo "ok   the page loaded and saved through the app"

sleep 1.5 # let the layout settle and save
python3 - "$DATA/notes.json" <<'PY' || { kill "$PID" 2>/dev/null; exit 1; }
import json, sys
d = json.load(open(sys.argv[1]))
nodes = {n["id"]: n for n in d["graph"]["nodes"]}
assert set(nodes) == {"a", "b", "c"}, nodes.keys()
assert nodes["b"]["color"] == "red" and d["settings"]["futureSetting"] == 42, "unknown fields were dropped"
assert (nodes["a"]["x"], nodes["a"]["y"]) != (0, 0) or (nodes["b"]["x"], nodes["b"]["y"]) != (5, 5), "the layout didn't run"
assert all(n.get("keywords") for n in nodes.values()), "keywords weren't computed"
print("ok   map, keywords and unknown fields survived; the layout moved the stacked cards apart")
PY

START=$(python3 -c 'import time; print(time.time())')
osascript -e 'tell application id "local.ollama-notes" to quit' >/dev/null
for _ in $(seq 1 50); do kill -0 "$PID" 2>/dev/null || break; sleep 0.1; done
kill -0 "$PID" 2>/dev/null && { kill "$PID"; fail "the app didn't quit"; }
TOOK=$(python3 -c "import time; print(round(time.time() - $START, 2))")
python3 -c "import sys; sys.exit(0 if $TOOK < 2.5 else 1)" || fail "quitting took ${TOOK}s: the page's flush didn't answer (3 s fallback)"
echo "ok   quit through the page's last save in ${TOOK}s"
