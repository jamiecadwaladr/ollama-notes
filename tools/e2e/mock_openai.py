#!/usr/bin/env python3
"""A stand-in for an OpenAI-compatible cloud API, with CORS, for testing Ollama Notes' cloud provider.

GET  /v1/models            needs "Authorization: Bearer test-key"
POST /v1/chat/completions  streams SSE, or answers JSON notes; model "mock-reasoner" rejects `temperature`
                           and `json_schema` with 400, like reasoning models and some providers do
GET  /log                  the requests seen so far

    tools/e2e/mock_openai.py 18734 &  tools/e2e/drive.py tools/e2e/cloud.json;  kill %1
"""
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = []
NOTES = {"topic": "Bloom filters", "notes": [
    {"title": "Bloom filter", "summary": "A probabilistic set: false positives, no false negatives.", "kind": "concept", "related": []},
    {"title": "False positive rate", "summary": "Grows as the filter fills; tuned by bits and hash count.", "kind": "fact", "related": ["Bloom filter"]},
]}


class H(BaseHTTPRequestHandler):
    def cors(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Headers", "authorization, content-type")
        self.send_header("Access-Control-Allow-Methods", "GET, POST")

    def reply(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.cors()
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def authorized(self):
        if self.headers.get("Authorization") == "Bearer test-key":
            return True
        self.reply(401, {"error": {"message": "Incorrect API key provided", "type": "invalid_request_error"}})
        return False

    def do_OPTIONS(self):
        self.send_response(204)
        self.cors()
        self.end_headers()

    def do_GET(self):
        if self.path == "/log":
            return self.reply(200, LOG)
        LOG.append({"path": self.path, "auth": self.headers.get("Authorization")})
        if self.path == "/v1/models" and self.authorized():
            self.reply(200, {"object": "list", "data": [
                {"id": "mock-reasoner"}, {"id": "text-embedding-3-small"}, {"id": "mock-chat"}]})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        rf = body.get("response_format", {}).get("type")
        LOG.append({"path": self.path, "auth": self.headers.get("Authorization"), "model": body.get("model"),
                    "stream": body.get("stream", False), "temperature": body.get("temperature"), "format": rf})
        if not self.authorized():
            return
        if body.get("model") == "mock-reasoner" and ("temperature" in body or rf == "json_schema"):
            bad = "temperature" if "temperature" in body else "response_format"
            return self.reply(400, {"error": {"message": f"Unsupported parameter: '{bad}'", "param": bad}})
        if body.get("stream"):
            self.send_response(200)
            self.cors()
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            chunks = [{"role": "assistant", "reasoning": "Thinking about sets."},
                      {"content": "A **Bloom filter** is "}, {"content": "a probabilistic set."}]
            for d in chunks:
                self.wfile.write(f"data: {json.dumps({'choices': [{'index': 0, 'delta': d}]})}\n\n".encode())
                self.wfile.flush()
                time.sleep(0.2)
            self.wfile.write(b"data: [DONE]\n\n")
            return
        self.reply(200, {"choices": [{"index": 0, "message": {"role": "assistant", "content": json.dumps(NOTES)}}]})

    def log_message(self, *a):
        pass


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
