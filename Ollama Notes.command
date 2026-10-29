#!/bin/bash
# Double-click this file in Finder (or run it from a terminal) to start Ollama Notes.
# It starts Ollama if needed, serves the UI, and opens your browser. Ctrl+C (or close this window) stops it.
cd "$(dirname "$0")" || exit 1
PY="$(command -v python3 || echo /usr/bin/python3)"
if ! "$PY" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)' 2>/dev/null; then
  echo "Python 3.8+ is required. Install it from https://www.python.org or with: xcode-select --install"
  read -r -p "Press Enter to close."
  exit 1
fi
exec "$PY" server.py "$@"
