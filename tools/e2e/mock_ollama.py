#!/usr/bin/env python3
"""A stand-in for Ollama with web search, for testing Ollama Notes' research loop (tools/e2e/research.json).

GET  /api/tags                       models "mock-researcher" (calls tools) and "mock-plain" (no tools capability)
POST /api/show                       their capabilities
POST /api/chat                       streams NDJSON; mock-researcher searches, then reads a page, then answers citing
                                     it. A question with "limit" hits the hourly search limit, "loop" searches until
                                     it's no longer offered tools, "slow" waits on a slow search. With "format" set
                                     (note extraction) it answers JSON notes.
POST /api/experimental/web_search    results whose content is a whole page, as Ollama's are; 429 for "limit test"
POST /api/experimental/web_fetch     a page
GET  /api/log                        the requests seen so far (through the server's /api proxy, for the page)

    tools/e2e/mock_ollama.py 18735 &
    OLLAMA_HOST=http://127.0.0.1:18735 tools/e2e/drive.py tools/e2e/research.json;  kill %1
"""
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = []
FILLER = "Unrelated text about gardening, weather and the history of the town. " * 12
PAGE = "\n\n".join([
    "Bloom filter",
    "A Bloom filter is a space-efficient probabilistic data structure.",
    FILLER,
    "False negatives are impossible: an added element always sets its bits, so a lookup never misses it.",
    FILLER,
])
NOTES = {"topic": "Bloom filters", "notes": [
    {"title": "No false negatives", "summary": "A Bloom filter lookup never misses an added element.", "kind": "fact", "related": []},
]}
LIMIT = "you have reached your web search hourly request limit, upgrade for higher limits: https://ollama.com/upgrade"


def call(name, args):
    return {"id": f"call_{name}", "function": {"index": 0, "name": name, "arguments": args}}


class H(BaseHTTPRequestHandler):
    def reply(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def stream(self, lines):
        self.send_response(200)
        self.send_header("Content-Type", "application/x-ndjson")
        self.end_headers()
        for line in lines:
            self.wfile.write((json.dumps(line) + "\n").encode())
            self.wfile.flush()
            time.sleep(0.15)

    def do_GET(self):
        if self.path == "/api/log":
            return self.reply(200, LOG)
        if self.path == "/api/tags":
            return self.reply(200, {"models": [{"name": "mock-researcher"}, {"name": "mock-plain"}]})
        self.reply(404, {"error": "not found"})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")
        if self.path == "/api/show":
            caps = ["completion", "tools"] if body.get("model") == "mock-researcher" else ["completion"]
            return self.reply(200, {"capabilities": caps})
        if self.path == "/api/experimental/web_search":
            LOG.append({"path": self.path, "query": body.get("query")})
            if body.get("query") == "limit test":
                return self.reply(429, {"error": LIMIT})
            if body.get("query") == "slow test":
                time.sleep(5)
            return self.reply(200, {"results": [
                {"title": "Bloom filter - Wikipedia", "url": "https://en.wikipedia.org/wiki/Bloom_filter", "content": PAGE},
                {"title": "Probabilistic data structures", "url": "https://example.com/pds", "content": "Sketches.\n\n" + FILLER},
            ]})
        if self.path == "/api/experimental/web_fetch":
            LOG.append({"path": self.path, "url": body.get("url")})
            return self.reply(200, {"title": "Bloom filter - Wikipedia", "content": PAGE, "links": []})
        if self.path != "/api/chat":
            return self.reply(404, {"error": "not found"})
        msgs = body.get("messages", [])
        last_user = max(i for i, m in enumerate(msgs) if m["role"] == "user")
        turn = msgs[last_user + 1:]
        tool_msgs = [m for m in turn if m["role"] == "tool"]
        LOG.append({
            "path": self.path, "model": body.get("model"), "format": "format" in body, "tools": "tools" in body,
            "num_ctx": body.get("options", {}).get("num_ctx"), "system": msgs[0]["content"] if msgs else "",
            "turn": [{k: m.get(k) for k in ("role", "content", "tool_name", "tool_calls") if m.get(k)} for m in turn],
        })
        if "format" in body:
            return self.reply(200, {"message": {"role": "assistant", "content": json.dumps(NOTES)}, "done": True})
        question = msgs[last_user]["content"]
        done = {"message": {"role": "assistant", "content": ""}, "done": True}
        if "tools" not in body:
            text = {"limit": "I couldn't search the web, so from memory: no false negatives.",
                    "loop": "Done looping."}.get(question.split()[0], "No tools here.")
            return self.stream([{"message": {"role": "assistant", "content": text}}, done])
        if question.startswith("loop"):
            return self.stream([{"message": {"role": "assistant", "content": "", "tool_calls": [
                call("web_search", {"query": f"loop {len(tool_msgs) + 1}"})]}}, done])
        if question.startswith(("limit", "slow")):
            q = question.split()[0] + " test"
            return self.stream([{"message": {"role": "assistant", "content": "Let me check.",
                                              "tool_calls": [call("web_search", {"query": q})]}}, done])
        if not tool_msgs:
            return self.stream([
                {"message": {"role": "assistant", "content": "", "thinking": "Need to search."}},
                {"message": {"role": "assistant", "content": "Let me check."}},
                {"message": {"role": "assistant", "content": "", "tool_calls": [
                    call("web_search", {"query": "bloom filter false negatives"})]}},
                done,
            ])
        if len(tool_msgs) == 1:
            # arguments as a JSON string, and an address without its scheme, as some models send them
            return self.stream([{"message": {"role": "assistant", "content": "", "tool_calls": [
                call("web_fetch", json.dumps({"url": "en.wikipedia.org/wiki/Bloom_filter"}))]}}, done])
        return self.stream([
            {"message": {"role": "assistant", "content": "", "thinking": "Found it."}},
            {"message": {"role": "assistant", "content": "A Bloom filter has no false negatives "}},
            {"message": {"role": "assistant", "content": "([Bloom filter - Wikipedia](https://en.wikipedia.org/wiki/Bloom_filter))."}},
            done,
        ])

    def log_message(self, *a):
        pass


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
