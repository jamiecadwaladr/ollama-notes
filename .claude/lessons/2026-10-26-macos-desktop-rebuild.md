---
title: Lessons from rebuilding Ollama Notes as a macOS desktop app
date: 2026-10-26
project: Ollama Notes (/Users/jamie/helper)
stack: Swift 6.2 (Swift 5 language mode), AppKit, WKWebView, vanilla JS ES modules, Ollama 0.34.1, pdf.js 4.10.38, macOS 26
tags: [macos, wkwebview, swift, appkit, ollama, cors, testing, electron-alternative, desktop-app, pdfjs]
---

# Lessons: Ollama Notes browser app → native macOS app (Swift + WKWebView)

Each section below stands alone. **Status** says whether the point was verified in this session or is reasoning
that was not tested directly.

## L01 — Choosing the desktop stack: Swift + WKWebView when there's no Node or Rust

**Context:** Ollama Notes was a vanilla-JS web UI served by a Python `server.py` that proxied `/api/*` to Ollama.
The task was "rebuild as a desktop application" on a Mac that had Swift (Command Line Tools) and Python 3, but no
Node/npm and no Rust.
**Lesson:** A native AppKit shell around WKWebView reuses the existing web UI unchanged and needs only
`swiftc`, so nothing new has to be installed. The app is 2.8 MB and universal (arm64 + x86_64). Electron would have
needed Node and ~200 MB; Tauri would have needed Rust. The cost is that it's macOS only, so browser mode via
`server.py` was kept as the cross-platform fallback.
**Apply:** Check the toolchains first (`node -v`, `rustc --version`, `swift --version`, `xcode-select -p`). If
the user is on a Mac without Node/Rust, prefer Swift + WKWebView.
**Status:** Verified (built, ran, and tested end to end).

## L02 — Ollama accepts `app://` origins by default, so a web view can call it without a proxy

**Context:** Ollama rejects cross-origin browser requests from unknown origins. The desktop app serves its page
from a custom scheme, `app://notes`.
**Lesson:** Ollama 0.34.1 allows `Origin: app://*` out of the box. A CORS preflight returns `204` with
`Access-Control-Allow-Origin: app://notes`, and a GET returns `200`, while `Origin: https://evil.example`
gets `403`. Naming the WKWebView custom scheme `app` therefore lets the page `fetch('http://127.0.0.1:11434/api/...')`
directly, and the Python proxy isn't needed.
**Apply:** Name the custom URL scheme `app` (Ollama's defaults also allow `file://`, `tauri://`,
`vscode-webview://`). Test with:
`curl -i -X OPTIONS -H "Origin: app://notes" -H "Access-Control-Request-Method: POST" -H "Access-Control-Request-Headers: content-type" http://127.0.0.1:11434/api/chat`
**Status:** Verified with curl, then in the app.

## L03 — Aborting a fetch in WKWebView cancels Ollama generation on the server

**Context:** The chat has a Stop button that aborts a streaming `/api/chat` fetch. With a proxy in between,
cancelling only the UI could leave Ollama generating, which wastes the GPU and queues later requests behind it.
**Lesson:** When the page talks to Ollama directly, `AbortController.abort()` closes the connection and Ollama
stops. In the test, Stop was pressed 5 s into a 600-word essay request, and Ollama's GIN log recorded that
request as `200 | 5.010855792s | POST "/api/chat"`. The UI updated in 262 ms.
**Apply:** To confirm cancellation reached the server, check `~/.ollama/logs/server.log` for the request
duration, e.g. `grep 'POST *"/api/chat"' ~/.ollama/logs/server*.log | tail`.
**Status:** Verified.

## L04 — Serve the web UI from a WKURLSchemeHandler (`app://`), not `file://`

**Context:** The UI uses ES modules and pdf.js, which starts a module Web Worker
(`new URL('./vendor/pdf.worker.min.mjs', import.meta.url)`).
**Lesson:** A `WKURLSchemeHandler` for `app` that maps paths to `Bundle.main.resourceURL/web/` works for module
scripts, dynamic `import()`, and the pdf.js 4.10.38 module worker. In the test, a dropped PDF was parsed in the
app. Set correct MIME types (`text/javascript` for `.js`/`.mjs`); module scripts need a JS MIME type. Guard
against path traversal by standardizing the file URL and checking it starts with the web root. With the scheme
registered, `window.isSecureContext` was `true` and `location.origin` was `app://notes`.
**Apply:** `config.setURLSchemeHandler(handler, forURLScheme: "app")`, then load `app://notes/index.html`.
**Status:** Verified for the custom scheme. The reasoning that `file://` would break modules was not tested.

## L05 — WKWebView needs WKUIDelegate methods for `confirm()`, `alert()` and `<input type=file>`

**Context:** The web UI uses `confirm()` (Clear map, Import map) and hidden file inputs (+ File, Import map).
**Lesson:** In a bare WKWebView these do nothing useful unless the `WKUIDelegate` implements
`runJavaScriptConfirmPanelWithMessage`, `runJavaScriptAlertPanelWithMessage` and `runOpenPanelWith`. Without
them, WebKit's defaults are confirm → `false` and no file picker. Implement them with `NSAlert`/`NSOpenPanel` as
sheets (`beginSheetModal(for:)`), and pass `parameters.allowsMultipleSelection` through. Note that
`WKOpenPanelParameters` doesn't expose the input's `accept` filter publicly.
**Apply:** In a WKWebView app, check every `confirm`/`alert`/`prompt`/file input in the web code and implement
the matching delegate method.
**Status:** Verified with the implementations (sheets appeared, multi-select was correct). The behaviour without
them comes from WebKit knowledge and was not tested.

## L06 — `evaluateJavaScript` from a native menu item counts as a user gesture for `input.click()`

**Context:** Menu items such as File › Import Files… (⌘O) run `window.desktopCommand('import-files')`, which calls
`.click()` on a hidden `<input type=file>`. Browsers normally block file pickers without user activation.
**Lesson:** In WKWebView, a script run through `webView.evaluateJavaScript(...)` from a menu action can
open the file picker: the delegate's `runOpenPanelWith` fired and the `NSOpenPanel` sheet attached. So native
menu commands can reuse the page's existing buttons and inputs.
**Apply:** Route menu commands to page buttons (`$(sel).click()`) instead of duplicating logic in Swift.
**Status:** Verified (`window.attachedSheet` was an `NSOpenPanel`).

## L07 — Export files through a native bridge (WKScriptMessageHandlerWithReply + NSSavePanel)

**Context:** The browser version exported JSON with `<a download>` and a blob URL revoked after 1 s.
**Lesson:** In WKWebView a blob download needs `WKDownload` plumbing, and revoking the blob while the save panel
is open is risky. Simpler and more reliable: `webkit.messageHandlers.desktop.postMessage({cmd:'saveFile', name, text})`
→ Swift shows `NSSavePanel` → writes atomically → replies with the path, which the JS shows in a toast.
`WKScriptMessageHandlerWithReply` turns `postMessage` into a Promise, and `replyHandler(nil, "message")` rejects it.
Check `message.frameInfo.securityOrigin.protocol == "app"` before acting on a message.
**Apply:** Keep the app-versus-browser differences in one JS module (here `web/platform.js`: `loadState`,
`saveState`, `saveFile`, `startOllama`), so the same UI still works in a browser.
**Status:** Verified (the file was written and the "Exported to …" toast appeared). The blob-revocation risk is
reasoning, not tested.

## L08 — NSSavePanel runs out of process: tests can't drive it, and it saves to its own default folder

**Context:** The test driver tried to finish an export's save panel programmatically.
**Lesson:** `NSSavePanel.ok(_:)` crashes the app with `NSGenericException: -[NSSavePanel ok:] : not implemented`.
`window.endSheet(panel, returnCode: .OK)` does complete it, but the remote panel ignores
`directoryURL`/`nameFieldStringValue` set programmatically while it's showing, and saves to its last-used or default
location. Here that was `~/Documents/ollama-notes-2026-10-26.json`, a real user folder, which had to be checked and
deleted.
**Apply:** In tests, cancel save panels (`panel.cancel(nil)`), or call the write function behind the bridge
directly. Never "OK" a real save panel in an automated test. If one was OK'd, find and remove what it wrote.
**Status:** Verified.

## L09 — AppKit's automatic menu items: View › Enter Full Screen and hidden alternates

**Context:** A dump of the programmatic main menu seemed to show duplicates: two "Enter Full Screen", two
"Start Dictation…", three "Emoji & Symbols".
**Lesson:** AppKit recognises the View menu by a `toggleFullScreen:` item. When one exists, AppKit inserts its
own visible "Enter Full Screen" (🌐F) and hides yours (`isHidden = true`; its ⌃⌘F shortcut keeps working).
If you remove yours, AppKit adds **no** Full Screen item at all. The extra Edit items are hidden system alternates.
"Close All ⌥⌘W" is added as an Option-alternate of Close.
**Apply:** Keep an explicit `toggleFullScreen:` item in the View menu. When dumping menus, print `isHidden` and
`isAlternate` (and the `.function` modifier) before deciding something is a duplicate.
**Status:** Verified.

## L10 — Test GUI apps on the user's screen only after warning them; a click looked like a bug

**Context:** Test runs opened the app's window and confirm sheets on the user's display. `confirm()` kept
returning `true` with no sheet visible, so it looked as if the app wiped the map without asking.
**Lesson:** The user was clicking OK on the test sheets because they thought Claude was stuck. The evidence: the
sheet attached, then ended with `1000` (`.alertFirstButtonReturn`) while `NSApp.currentEvent` was `leftMouseUp`
(type 2). A plain native NSAlert in the same harness stayed up. Time went on instrumenting a working dialog.
**Apply:** Before a GUI test run, tell the user what will appear and for how long, and ask them not to interact.
If a UI result looks impossible, ask whether they clicked anything before adding debug code. Useful diagnostics:
log `NSApp.currentEvent?.type` in the handler, and add `NSEvent.addLocalMonitorForEvents(matching: .any)`.
**Status:** Verified (the user confirmed: "That was me clicking the alert").

## L11 — A test driver for a WKWebView app without computer use

**Context:** There was no way to click around the native app, but behaviour still had to be verified inside WebKit.
**Lesson:** What worked:
- Keep the app's entry point in a tiny `main.swift`. A test build compiles the same sources with a different
  `main.swift` (top-level code is only allowed in a file named `main.swift`).
- The driver creates the real `AppDelegate` and runs JSON steps:
  - `js`: `webView.callAsyncJavaScript`, which awaits Promises.
  - `snapshot`: `webView.takeSnapshot` to a PNG, which needs no Screen Recording permission.
  - `native`: run menu commands, inspect or end `window.attachedSheet`, dump menus, `NSApp.terminate`.
- Build the test copy as a real `.app` with a **different bundle id**, run its binary inside the bundle so
  `Bundle.main` finds the resources, and pass `-DataDirectory <tmp>`. UserDefaults reads `-Key value` launch
  arguments, so throwaway data needs no special test code in the app.
- Wait with JS polling loops (`until(() => cond, ms)`) for streaming and extraction to finish.
**Apply:** Reuse this pattern for any AppKit + WKWebView app. Clean up afterwards with `defaults delete <test bundle id>`.
**Status:** Verified.

## L12 — A JS `confirm()` blocks `callAsyncJavaScript`; trigger it with `setTimeout`

**Context:** The test driver clicked "Clear map", which calls `confirm()`.
**Lesson:** `confirm()` blocks the page's JS thread until the native sheet is answered. If the click runs inside
`callAsyncJavaScript`, that call doesn't complete, so a driver waiting on it before dismissing the sheet
deadlocks. Schedule the click with `setTimeout(() => el.click(), 0)`, return right away, then handle the sheet
in a native step.
**Status:** Applied as reasoning; the deadlock was avoided, not reproduced.

## L13 — Simulating a file drop in WebKit, and making a test PDF on macOS

**Lesson:** In WKWebView, `new DataTransfer(); dt.items.add(new File([blob], 'x.pdf', {type: 'application/pdf'}))`
works. Create `new Event('drop', {bubbles: true, cancelable: true})`, attach the transfer with
`Object.defineProperty(ev, 'dataTransfer', {value: dt})`, and dispatch it. The page's drop handler (checking
`dataTransfer.types.includes('Files')`) treats it like a real drop. To make a PDF with a real text layer without
extra tools, run `cupsfilter -m application/pdf input.txt > out.pdf`. To get the PDF into the page, copy it into
the test bundle's web folder and `fetch('/test.pdf')`.
**Status:** Verified (pdf.js extracted 1,461 characters).

## L14 — Saving on quit in a WKWebView app: `terminateLater` plus a page flush

**Context:** The page saves on a 250 ms debounce, so an edit made just before ⌘Q could be lost.
**Lesson:** In `applicationShouldTerminate`, return `.terminateLater`. Then run
`webView.callAsyncJavaScript("await window.desktopFlush?.()")` and call `NSApp.reply(toApplicationShouldTerminate: true)`
when it completes, with a 3 s fallback so quitting can never hang. Make `windowShouldClose` call
`NSApp.terminate(nil)` and return `false`, so closing the window takes the same path. The page's flush clears the
debounce timer and awaits the bridge save, which writes atomically.
**Apply:** Also add a `loaded` guard, so nothing is saved before the async load finishes. Otherwise an early
save can overwrite the data file with empty state.
**Status:** Verified: a title typed immediately before quitting was in `notes.json` after exit.

## L15 — File storage with a per-session backup instead of WebKit localStorage

**Lesson:** WebKit caps `localStorage` at about 5 MB per origin, and documents that keep their full PDF text can
exceed that, which made the browser version's saves fail silently (`console.warn`). The app stores JSON in
`~/Library/Application Support/<App>/notes.json` through the bridge, written with `Data.write(options: .atomic)`.
On the first save of each session it copies the old file to `notes.backup.json`, so a bad import or "Clear map" is
recoverable. Allow overriding the folder with `defaults write <bundle id> DataDirectory <path>`.
Also show a toast when a save fails instead of failing silently.
**Status:** Backup creation and persistence across relaunch verified. The 5 MB figure is WebKit knowledge, not measured.

## L16 — Starting Ollama from a Finder-launched app

**Lesson:** Apps launched from Finder get a minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), so
`which ollama` fails. Look for Ollama.app by bundle id `com.electron.ollama` (checked with
`defaults read /Applications/Ollama.app/Contents/Info CFBundleIdentifier`) through
`NSWorkspace.shared.urlForApplication(withBundleIdentifier:)`, and open it with `OpenConfiguration.activates = false`.
Otherwise check `/opt/homebrew/bin`, `/usr/local/bin` and `~/.local/bin` for the `ollama` CLI and run `ollama serve`.
On that machine `/usr/local/bin/ollama` is a symlink into Ollama.app. Only try to start it for local hosts, only
once per launch or server change (so it doesn't relaunch Ollama after the user quits it), and poll every 1 s
for 30 s while showing "Starting Ollama…".
**Status:** Bundle id and paths verified. The launch path was exercised only for the "already running" case.

## L17 — Friendly network errors and timeouts in the Ollama client

**Lesson:** When the page calls Ollama directly, a refused or failed connection rejects `fetch` with a generic
`TypeError` (WebKit's text is "Load failed"). Wrap `fetch` and rethrow a message naming the server and what to do,
for example: "Cannot reach Ollama at http://localhost:1. Start Ollama.app or run 'ollama serve', or change the server in Settings."
Pass aborts through untouched: check `init.signal.aborted`, not `e.name`, because `abort('timeout')` rejects with
the reason value itself. Give the model-list probe `AbortSignal.timeout(5000)`, so an unroutable host doesn't
leave the status stuck on "connecting…" for ~75 s.
**Status:** Message verified in the app. The exact WebKit error text and the TCP timeout figure are knowledge.

## L18 — Trackpad pinch in WebKit fires gesture events, not ctrl+wheel

**Lesson:** Chrome reports a trackpad pinch as `wheel` events with `ctrlKey`. Safari and WKWebView report
`gesturestart` / `gesturechange` (with `ev.scale`, `clientX/Y`) / `gestureend`. Zoom by `ev.scale / lastScale`
on each change, `preventDefault()` all three, and ignore ctrl+wheel while a gesture is active in case both arrive.
**Status:** Implemented from WebKit knowledge; not tested with a real trackpad.

## L19 — Swift compile gotchas from building without Xcode

**Lessons:**
- `NSMenu.addItem(_:)` returns `Void`. Use `menu.addItem(withTitle:action:keyEquivalent:)`, which returns the item,
  when you need to set `.submenu`.
- `Substring + "?"` is a `Substring`, not a `String`; wrap it as `String(sub) + "?"`.
- Top-level statements are only allowed in a file named `main.swift` ("expressions are not allowed at the top level").
- Build universal: `swiftc -O -swift-version 5 -target arm64-apple-macos14 …` and `-target x86_64-apple-macos14 …`,
  then `lipo -create`. Sign ad hoc with `codesign --force --sign - App.app`, which is enough to run on the building
  Mac. A copy on another Mac needs right-click › Open the first time.
- The first full build, including drawing the icon, took about 80 s.
**Status:** Verified.

## L20 — macOS app icon from code: CoreGraphics + iconutil

**Lesson:** Without design tools, draw the icon in a small Swift script on a 1024-point canvas. Use the macOS grid:
an 824×824 rounded tile at offset 100 with a corner radius of about 186, plus a soft drop shadow. Render each
iconset size (16–512 at @1x and @2x) with `CGContext` + `CGImageDestination`, then run
`iconutil -c icns -o AppIcon.icns AppIcon.iconset`. Have the build script redraw only when the script is newer
than the `.icns`.
**Status:** Verified that it renders. Whether macOS 26 shows a grid-following legacy icon without its gray
container is reasoning.

## L21 — WKWebView window polish details

**Lessons:**
- `webView.setValue(false, forKey: "drawsBackground")` plus a window `backgroundColor` avoids a white flash
  before the page paints.
- `titlebarAppearsTransparent = true`, `titleVisibility = .hidden` and a window background matching the page's
  top bar make the title bar blend in, without the hard window-dragging problem of `fullSizeContentView`
  (WKWebView has no `-webkit-app-region`).
- `window.setFrameUsingName("Main")`, then `setFrameAutosaveName("Main")`, remembers size and position.
- `NSWindow.allowsAutomaticWindowTabbing = false` keeps tab items out of the View menu.
- `webView.isInspectable = true` enables right-click › Inspect Element.
- Decide navigation in `decidePolicyFor`: allow `app:`/`about:` and open `http(s)`/`mailto` in the default
  browser. Handle `target=_blank` in `createWebViewWith`.
- Reload in `webViewWebContentProcessDidTerminate`.
- Single instance: if `NSRunningApplication.runningApplications(withBundleIdentifier:)` finds another process,
  activate it and quit, so two copies never write the same data file.
**Status:** Window and title bar verified visually in snapshots. The single-instance and crash-reload paths
were not exercised.

## L22 — Existing UI bugs show up at a desktop app's fixed default window size

**Context:** At the default 1280-pt window (chat pane 440 px), the map toolbar was wider than its pane, so
Export map, Import map and Clear map were cut off and the note count wrapped onto three lines. The same happened
in a browser at that width.
**Lesson:** A desktop app opens at one known size, so snapshot it at that size and fix overflow. The fix was
`flex-wrap: wrap` on the toolbar and `white-space: nowrap` on the count. The absolutely positioned inspector
follows the toolbar through a CSS variable set by a `ResizeObserver`:
`top: calc(var(--toolbar-h, 38px) + 12px)`.
**Status:** Verified (the toolbar wrapped to 69 px and the inspector top was 81 px).

## L23 — Keep browser mode working when wrapping a web app natively

**Lesson:** The web UI decides it's inside the app by checking `window.webkit?.messageHandlers?.desktop`. A
bare WKWebView pointed at `http://localhost:8799` (served by `server.py --no-browser --no-ollama --port 8799`)
is a cheap way to test browser mode without a real browser. It confirmed the `localStorage` save, the
same-origin `/api` proxy (`GET /api/tags` logged), and that app-only settings stay hidden.
**Status:** Verified.
