#!/usr/bin/env python3
"""Drives the page in headless Chrome (no window on screen) against the browser-mode server, over the DevTools
protocol. Each step is JavaScript run in the page (awaited) or a screenshot.

    tools/e2e/drive.py steps.json [--out DIR]

steps.json is a list of steps:
    {"js": "<expression or async function body>", "expect": <value>}   run it; compare the result if "expect" given
    {"shot": "name"}                                                   save DIR/name.png
    {"reload": true}                                                   reload the page
    {"dialogs": "accept" | "dismiss"}                                  how to answer confirm() from now on

console.info() in a step is printed at the end, e.g. to report what a real model did.

Needs: the server built (cargo build -p ollama-notes-server), Google Chrome, and Python's websockets package.
Ollama is used as it is: start it first for steps that chat.
"""
import argparse, asyncio, base64, json, os, shutil, socket, subprocess, sys, tempfile, time, urllib.request

import websockets

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_http(url, seconds=20):
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            return urllib.request.urlopen(url, timeout=1).read()
        except OSError:
            time.sleep(0.2)
    raise RuntimeError(f"{url} did not come up")


class Page:
    def __init__(self, ws):
        self.ws, self.next_id, self.pending, self.events = ws, 0, {}, []
        self.dialog_answer = True

    async def reader(self):
        async for raw in self.ws:
            msg = json.loads(raw)
            if "id" in msg and msg["id"] in self.pending:
                self.pending.pop(msg["id"]).set_result(msg)
            elif msg.get("method") == "Page.javascriptDialogOpening":
                asyncio.ensure_future(self.send("Page.handleJavaScriptDialog", accept=self.dialog_answer))
                self.events.append(("dialog", msg["params"].get("message")))
            elif msg.get("method") == "Runtime.exceptionThrown":
                d = msg["params"]["exceptionDetails"]
                self.events.append(("exception", d.get("exception", {}).get("description") or d.get("text")))
            elif msg.get("method") == "Runtime.consoleAPICalled":
                args = " ".join(str(a.get("value", a.get("description", ""))) for a in msg["params"]["args"])
                self.events.append((msg["params"]["type"], args))

    async def send(self, method, **params):
        self.next_id += 1
        fut = asyncio.get_running_loop().create_future()
        self.pending[self.next_id] = fut
        await self.ws.send(json.dumps({"id": self.next_id, "method": method, "params": params}))
        msg = await asyncio.wait_for(fut, 300)
        if "error" in msg:
            raise RuntimeError(f"{method}: {msg['error']}")
        return msg["result"]

    async def js(self, code):
        expr = code if "return" not in code and "await" not in code else f"(async () => {{ {code} }})()"
        r = await self.send("Runtime.evaluate", expression=expr, awaitPromise=True, returnByValue=True)
        if "exceptionDetails" in r:
            raise RuntimeError(r["exceptionDetails"].get("exception", {}).get("description") or r["exceptionDetails"])
        return r["result"].get("value")


# Helpers available to every step as window.t.*
HELPERS = r"""
window.t = {
  $: (s) => document.querySelector(s),
  sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
  async until(fn, ms = 60000, every = 100) {
    const end = Date.now() + ms;
    while (Date.now() < end) { const v = fn(); if (v) return v; await t.sleep(every); }
    throw new Error('timed out waiting for ' + fn);
  },
  type(sel, text) { const el = t.$(sel); el.value = text; el.dispatchEvent(new Event('input', { bubbles: true })); },
  files(sel, files) {
    const dt = new DataTransfer(); for (const f of files) dt.items.add(f);
    const el = t.$(sel); el.files = dt.files; el.dispatchEvent(new Event('change', { bubbles: true }));
  },
  drop(files) {
    const dt = new DataTransfer(); for (const f of files) dt.items.add(f);
    const ev = new Event('drop', { bubbles: true, cancelable: true });
    Object.defineProperty(ev, 'dataTransfer', { value: dt }); t.$('#canvas').dispatchEvent(ev);
  },
  state: () => JSON.parse(localStorage.getItem('ollama-notes.v1') || 'null'),
  count: () => t.$('#mapCount').textContent,
  toast: () => (t.$('#toast').hidden ? '' : t.$('#toast').textContent),
};
"""


async def run(steps, out):
    port = free_port()
    server = subprocess.Popen([os.path.join(ROOT, "target/debug/ollama-notes-server"), "--no-browser", "--no-ollama", "--port", str(port)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    profile = tempfile.mkdtemp()
    cdp = free_port()
    chrome = subprocess.Popen([CHROME, "--headless=new", f"--remote-debugging-port={cdp}", f"--user-data-dir={profile}",
                               "--window-size=1280,820", "--no-first-run", "--no-default-browser-check", "about:blank"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    failures = 0
    try:
        wait_http(f"http://127.0.0.1:{port}/")
        wait_http(f"http://127.0.0.1:{cdp}/json/version")
        targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{cdp}/json/list").read())
        target = next(t for t in targets if t.get("type") == "page")
        async with websockets.connect(target["webSocketDebuggerUrl"], max_size=None) as ws:
            page = Page(ws)
            reader = asyncio.ensure_future(page.reader())
            for m in ("Page.enable", "Runtime.enable"):
                await page.send(m)
            await page.send("Page.addScriptToEvaluateOnNewDocument", source=HELPERS)

            async def load():
                await page.send("Page.navigate", url=f"http://127.0.0.1:{port}/")
                await page.js("return await t.until(() => document.querySelector('#status .status-text').textContent !== 'connecting…', 15000)")

            await load()
            for i, step in enumerate(steps):
                label = step.get("name") or step.get("js", step.get("shot", ""))[:70].replace("\n", " ")
                try:
                    if "reload" in step:
                        await load()
                        result = None
                    elif "dialogs" in step:
                        page.dialog_answer = step["dialogs"] == "accept"
                        result = None
                    elif "shot" in step:
                        shot = await page.send("Page.captureScreenshot", format="png")
                        path = os.path.join(out, f"{step['shot']}.png")
                        with open(path, "wb") as f:
                            f.write(base64.b64decode(shot["data"]))
                        result = path
                    else:
                        result = await page.js(step["js"])
                    if "expect" in step and result != step["expect"]:
                        failures += 1
                        print(f"FAIL {i:2} {label}\n       got {json.dumps(result)[:300]}\n  expected {json.dumps(step['expect'])[:300]}")
                    else:
                        print(f"ok   {i:2} {label}" + (f"  -> {json.dumps(result)[:160]}" if result is not None else ""))
                except Exception as e:  # noqa: BLE001 - report and carry on with the next step
                    failures += 1
                    print(f"FAIL {i:2} {label}\n       {e}")
            problems = [e for e in page.events if e[0] in ("exception", "error")]
            for kind, text in page.events:
                if kind in ("exception", "error", "warning", "dialog", "info"):
                    print(f"  [{kind}] {text}")
            failures += len(problems)
            reader.cancel()
    finally:
        chrome.terminate()
        server.terminate()
        chrome.wait(10)
        shutil.rmtree(profile, ignore_errors=True)
    print(f"{len(steps)} steps, {failures} failure(s)")
    return failures


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("steps")
    ap.add_argument("--out", default=tempfile.gettempdir())
    args = ap.parse_args()
    with open(args.steps) as f:
        steps = json.load(f)
    os.makedirs(args.out, exist_ok=True)
    sys.exit(1 if asyncio.run(run(steps, args.out)) else 0)


if __name__ == "__main__":
    main()
