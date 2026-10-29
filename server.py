#!/usr/bin/env python3
"""Ollama Notes — one-file launcher: starts Ollama if needed, serves the web UI,
proxies /api/* to Ollama, and opens your browser.

Usage:  ./server.py  (or double-click "Ollama Notes.command")
Flags:  --no-browser   don't open a browser tab
        --no-ollama    don't try to start Ollama
        --port N       listen on N (default 8765, or the next free port)
Env:    PORT, OLLAMA_HOST (default http://127.0.0.1:11434)
"""
import argparse
import functools
import http.client
import json
import os
import platform
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.parse
import webbrowser
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.dirname(os.path.abspath(__file__))
WEB_DIR = os.path.join(ROOT, "web")
PORT = int(os.environ.get("PORT", "8765"))
OLLAMA = os.environ.get("OLLAMA_HOST", "http://127.0.0.1:11434")
if "://" not in OLLAMA:
    OLLAMA = "http://" + OLLAMA
_u = urllib.parse.urlparse(OLLAMA)
OLLAMA_HOST, OLLAMA_PORT = _u.hostname or "127.0.0.1", _u.port or 11434


class Handler(SimpleHTTPRequestHandler):
    def log_message(self, fmt, *args):
        # Keep the console quiet for static assets; show API traffic and errors.
        if self.path.startswith("/api/") or (args and str(args[-2:-1]) not in ("('200',)", "('304',)")):
            sys.stderr.write("%s - %s\n" % (self.log_date_time_string(), fmt % args))

    def do_GET(self):
        if self.path.startswith("/api/"):
            return self.proxy("GET")
        return super().do_GET()

    def do_POST(self):
        if self.path.startswith("/api/"):
            return self.proxy("POST")
        self.send_error(405)

    def do_DELETE(self):
        if self.path.startswith("/api/"):
            return self.proxy("DELETE")
        self.send_error(405)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def _send_json(self, status, obj):
        payload = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def proxy(self, method):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else None
        try:
            conn = http.client.HTTPConnection(OLLAMA_HOST, OLLAMA_PORT, timeout=600)
            conn.request(method, self.path, body=body, headers={"Content-Type": "application/json"})
            resp = conn.getresponse()
        except OSError as e:
            return self._send_json(502, {"error": f"Cannot reach Ollama at {OLLAMA} ({e}). Start Ollama.app or run `ollama serve`."})
        self.send_response(resp.status)
        self.send_header("Content-Type", resp.getheader("Content-Type", "application/json"))
        self.end_headers()
        try:
            # Stream chunks through as they arrive so token streaming works.
            while True:
                chunk = resp.read1(8192)
                if not chunk:
                    break
                self.wfile.write(chunk)
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass  # Browser aborted (Stop button); closing conn cancels generation.
        finally:
            conn.close()


def ollama_models():
    """Returns the list of model names, or None if Ollama is not reachable."""
    try:
        conn = http.client.HTTPConnection(OLLAMA_HOST, OLLAMA_PORT, timeout=2)
        conn.request("GET", "/api/tags")
        r = conn.getresponse()
        data = json.loads(r.read() or b"{}")
        conn.close()
        return [m.get("name") for m in data.get("models", [])]
    except Exception:
        return None


def start_ollama():
    """Best-effort launch of a local Ollama server. Returns a description of what was done, or None."""
    if OLLAMA_HOST not in ("127.0.0.1", "localhost", "0.0.0.0"):
        return None  # remote host: nothing to start here
    if platform.system() == "Darwin" and os.path.isdir("/Applications/Ollama.app"):
        subprocess.Popen(["open", "-g", "-a", "Ollama"])
        return "opened Ollama.app"
    exe = shutil.which("ollama")
    if exe:
        subprocess.Popen([exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                         start_new_session=True)
        return "ran `ollama serve`"
    return None


def wait_for_ollama(seconds):
    deadline = time.time() + seconds
    while time.time() < deadline:
        models = ollama_models()
        if models is not None:
            return models
        time.sleep(0.5)
    return None


def already_running(port):
    """True if an Ollama Notes server already answers on this port (so we just open the browser)."""
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
        conn.request("GET", "/")
        body = conn.getresponse().read(4096)
        conn.close()
        return b"<title>Ollama Notes</title>" in body
    except Exception:
        return False


def port_free(port):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        return s.connect_ex(("127.0.0.1", port)) != 0


def main():
    global PORT
    sys.stdout.reconfigure(line_buffering=True)  # logs show up promptly even when piped to a file
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=PORT)
    ap.add_argument("--no-browser", action="store_true")
    ap.add_argument("--no-ollama", action="store_true")
    args = ap.parse_args()
    PORT = args.port

    url = f"http://localhost:{PORT}"
    if already_running(PORT):
        print(f"Ollama Notes is already running at {url}")
        if not args.no_browser:
            webbrowser.open(url)
        return
    while not port_free(PORT):
        PORT += 1
        url = f"http://localhost:{PORT}"

    models = ollama_models()
    if models is None and not args.no_ollama:
        how = start_ollama()
        if how:
            print(f"Ollama not running — {how}, waiting for it…", flush=True)
            models = wait_for_ollama(25)
        else:
            print("Ollama not found. Install it from https://ollama.com and run `ollama serve`.")

    handler = functools.partial(Handler, directory=WEB_DIR)
    server = ThreadingHTTPServer(("127.0.0.1", PORT), handler)
    server.daemon_threads = True

    print(f"Ollama Notes  →  {url}")
    print(f"Ollama API    →  {OLLAMA}", end="  ")
    if models is None:
        print("(NOT reachable yet — the UI will reconnect automatically once Ollama is up)")
    elif not models:
        print("(running, but no models pulled — run e.g. `ollama pull llama3.2`)")
    else:
        print(f"({len(models)} model(s): {', '.join(models)})")
    print("Press Ctrl+C to stop.")

    if not args.no_browser:
        threading.Timer(0.6, lambda: webbrowser.open(url)).start()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nbye")


if __name__ == "__main__":
    main()
