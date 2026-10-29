# Ollama Notes → Rust: rewrite plan

Status: **implemented on the `rust` branch** (2026-10-29). The plan below is kept as written; this section records
where the implementation departed from it and what has been verified.

### Implementation notes

- **The page is plain `web-sys`, not Leptos.** `app.js` and `graph.js` were imperative DOM code, so they were ported
  1:1 to Rust over `web-sys` (`ui/src/*.rs`), keeping `index.html` and `style.css` unchanged apart from Trunk's tags
  and table styles. This gives the same DOM, a straight module-for-module port, fewer dependencies and a smaller wasm.
- **The browser-mode server was written straight against the new page**, and the interim step of hosting the old JS
  UI in Tauri (M1) was skipped: the whole rewrite landed at once, and the old code stays on `master`.
- `Node.kind` is a string, not an enum. Unknown values round-trip either way, and a string is simpler.
- Card heights are read from the DOM each frame, as the JS did, instead of being cached. That has been fine so far.
- The force layout matches the JS operation for operation: the golden tests compare positions after every tick for
  200 ticks (within 1e-6), and Tidy's settled positions exactly.
- The CSP is off (`csp: null`), as in the Swift app; the plan's CSP is still worth adding later.
- The menu bar has no "Paste and Match Style" and no Edit › Delete, because Tauri's menu API has no arbitrary
  selectors. Nothing auto-reloads the page if WebKit's content process crashes. Both are listed in §11.

### Verified

- `scripts/check.sh` passes: `cargo fmt --check`, clippy `-D warnings` natively and for wasm32, and 59 tests. Those
  are the unit tests, 8 golden tests against fixtures generated from the master branch's JS with `jsc`, the store and
  launcher tests, and the server and app tests.
- Spike S2 (CORS): Ollama 0.34.1 answers a preflight from `Origin: tauri://localhost` with 204 and
  `Access-Control-Allow-Origin: tauri://localhost`.
- Browser mode, end to end in headless Chrome (`tools/e2e/drive.py tools/e2e/smoke.json`, 31 steps, all passing):
  - notes, keywords and saving, including a reload
  - a streamed chat with automatic note extraction
  - Stop, which also stops generation on the server (Ollama's log shows the request ending ~2.6 s in)
  - documents: a text file, and a dropped PDF parsed by pdf.js with notes extracted
  - keyword links, the filter, Tidy with no overlapping cards, pinning
  - importing an export, Clear map, New chat, Settings
- The server: MIME types (including `.mjs` and `.wasm`), `no-store`, 404/405, streaming `/api/chat`, and detecting
  an already-running copy.
- The icon drawn by `tools/make-icon` matches the Swift-drawn one.
- The desktop app (`./build.sh`): a 14 MB universal, ad-hoc-signed bundle with the same bundle id, and the ATS and
  local-network keys merged into Info.plist. `tools/e2e/app-check.sh` launches it on a temporary data folder and
  checks four things:
  - the wasm page runs in the app's WKWebView from `tauri://localhost`, and loads and saves through the Rust commands,
    which also writes the session backup
  - the `-DataDirectory` launch argument is honoured
  - a seeded map round-trips with unknown fields kept
  - a normal quit (`osascript … quit`) goes through the page's last save in 0.28 s; spike S4 for the AppleEvent path
  Ollama's log shows the app's `GET /api/tags` from its `tauri://` page succeeding.

### Not yet checked by hand in the app

These need someone at the keyboard (Tauri's WebDriver doesn't support macOS):
- the menu bar and its shortcuts
- the confirm, open and save sheets (spike S3)
- dropping files onto the map in the app window
- trackpad pinch
- ⌘Q, the close button and Dock › Quit (spike S4)
- single instance, and the remembered window position
- opening reply links in the browser
- starting Ollama when it isn't running

A short manual pass through §10's "Shell" list covers them.

## 1. Summary

Rebuild Ollama Notes as one Cargo workspace:

- **Desktop shell: Tauri 2** (`src-tauri/`), in place of the Swift AppKit + WKWebView app (`macos/`).
- **UI: Leptos, client-side rendered, compiled to WebAssembly** (`ui/`), in place of the vanilla-JS UI (`web/*.js`).
  `style.css` and the DOM structure carry over, so the app looks the same.
- **Logic: a pure-Rust core crate** (`crates/core`). It holds everything that is logic rather than DOM: the data
  model, TF-IDF keywords and auto-linking, the force layout, text chunking, Ollama protocol parsing, Markdown and the
  extraction prompts. It builds for both native and wasm32, so `cargo test` can test it natively.
- **Browser mode: a small axum server** (`server/`), in place of `server.py`.

Two kinds of non-Rust code remain. One is the vendored pdf.js, plus a ~20-line JS shim that calls it. The other is
the HTML/CSS.

**Why this shape.** The architecture stays as it is today. The page owns the state and talks to Ollama directly.
The native side does only what a page can't: menus, panels, the data file and starting Ollama. That means the port
goes module by module, and the data file, export format, shortcuts and look all stay the same. On macOS, Tauri uses
WKWebView, so the WebKit lessons (L02–L07, L12–L14, L18) still apply. No Node is needed: Rust 1.99.1 is installed,
and Trunk and the Tauri CLI both install through `cargo`.

**Order.** The work goes in this order:

1. The Tauri shell, still running the existing JS UI. This retires the Swift code and proves data compatibility early.
2. The Rust browser-mode server. This retires `server.py`.
3. The core crate, pinned by golden tests generated from the current JS.
4. The Leptos UI port.

The checkpoint after step 2 is itself a usable result. It gives "native side in Rust, UI still JS" if you want to
stop there.

## 2. What exists today

| Area | Files | Lines | Role |
|---|---|---:|---|
| UI logic | `web/app.js` | 764 | state, chat, extraction, documents, inspector, settings, import/export, menu commands |
| | `web/graph.js` | 583 | pan/zoom canvas, nodes and edges in the DOM/SVG, drag, linking, placement, force layout |
| | `web/keywords.js` | 176 | tokenizer, stemmer, stop words, TF-IDF keywords, keyword auto-links |
| | `web/ollama.js` | 109 | fetch client: streaming chat, JSON-schema chat with fallbacks, loose JSON salvage |
| | `web/documents.js` | 91 | text/HTML/PDF to text (pdf.js), chunking, title from file name |
| | `web/markdown.js` | 45 | small escaping Markdown renderer |
| | `web/platform.js` | 31 | app vs browser differences (storage, saving files, starting Ollama), `normalizeHost` |
| UI markup/style | `web/index.html`, `web/style.css` | 196 + 208 | layout, dialogs, dark theme |
| Vendored | `web/vendor/pdf.min.mjs`, `pdf.worker.min.mjs` | — | pdf.js 4.10.38 |
| macOS shell | `macos/Sources/AppDelegate.swift` | 224 | window, menu bar, navigation policy, open panel, confirm/alert sheets, quit flush |
| | `macos/Sources/Bridge.swift` | 97 | `load`/`save` (with per-session backup), `saveFile`, `startOllama` |
| | `macos/Sources/WebAssets.swift` | 33 | serves `web/` as `app://notes` |
| | `macos/Sources/main.swift`, `Info.plist`, `make-icon.swift` | 5 + — + 109 | entry point, bundle metadata, icon drawing |
| Build | `build.sh` | 44 | universal swiftc build, icon, bundle, ad-hoc sign, `--open`/`--install` |
| Browser mode | `server.py`, `Ollama Notes.command` | 203 + 11 | static server, streaming `/api` proxy, starts Ollama, opens a browser |

That is about 1,800 lines of JS, 470 of Swift and 200 of Python, with no automated tests. The live data file
(`~/Library/Application Support/Ollama Notes/notes.json`) currently has settings and an empty map.

## 3. Goals and non-goals

**Goals**
- Feature parity with the README. The checklist is in §10.
- Read and write the existing `notes.json` and export files unchanged. Keep the same location, the same
  `notes.backup.json` behaviour, the same `defaults write local.ollama-notes DataDirectory …` override and the same
  bundle id `local.ollama-notes`.
- macOS 14+, a universal binary (arm64 + x86_64), ad-hoc signed, still built with `./build.sh` (`--open`, `--install`).
- No Node/npm in the toolchain.
- Automated tests for all the logic.

**Non-goals for this rewrite**
- New features or a redesign. Improvements wait until parity (M6).
- Notarization or the App Store.
- Windows/Linux packaging. Tauri makes it possible later (see §12).

## 4. Target architecture

```
 Ollama Notes.app (Tauri 2)
 ┌────────────────────────────┐        ┌──────────────────────────────────────────────┐
 │ src-tauri  (native Rust)   │        │ ui  (Leptos → wasm32, inside WKWebView)       │
 │  menu bar, sheets/panels,  │◀─IPC──▶│  owns all state; chat, map, inspector,        │
 │  quit flush, single        │        │  dialogs; runs the layout loop; uses          │── fetch ──▶ Ollama :11434
 │  instance, external links  │        │  notes-core for every non-DOM decision        │  (streamed,
 └─────────────┬──────────────┘        └──────────────────────────────────────────────┘   abortable)
               │ uses
        crates/native  (data dir, notes.json store + backup, Ollama launcher)
               ▲ uses
 server/  (browser mode: serves ui/dist, proxies /api/* → Ollama, starts Ollama, opens the browser)
```

The rule stays the same as today: the page owns the state and talks to Ollama, and the native side does only what a
page can't.

### Workspace layout

```
Cargo.toml                 [workspace] resolver = "2"; shared [workspace.dependencies]; release profile
crates/core/               notes-core   pure logic, no I/O; builds for native and wasm32
crates/native/             notes-native data dir, store, Ollama launcher (used by src-tauri and server)
ui/                        notes-ui     Leptos CSR app (wasm32), built by Trunk
  index.html  Trunk.toml  style.css  vendor/pdf*.mjs  js/pdf_shim.js  src/…
src-tauri/                 ollama-notes Tauri 2 app: tauri.conf.json, Info.plist, icons/, src/…
server/                    ollama-notes-server  browser-mode binary
tools/make-icon/           draws the 1024-px icon PNG (tiny-skia)
tools/golden/              jsc scripts that generate test fixtures from the current JS
scripts/check.sh           quality gate (§8)
build.sh                   wraps `cargo tauri build`, keeps --open / --install
Ollama Notes.command       runs the server binary
```

The directory is named `src-tauri/` by convention so the Tauri CLI finds it without extra flags.

## 5. Key decisions

### 5.1 UI: Leptos (CSR) in the Tauri web view
- Leptos with the `csr` feature, mounted with `mount_to_body`, built by **Trunk**. Trunk handles wasm-bindgen and
  wasm-opt, and copies `vendor/`.
- The DOM and `style.css` stay, along with WebKit's text layout, editing, IME and accessibility. The same `ui/dist`
  serves the desktop app and browser mode.
- Fine-grained signals suit streamed tokens: each chat message gets its own signal, so a token re-renders only that
  message. The repaint stays throttled to `requestAnimationFrame`, as in `app.js`.
- Release profile: `opt-level = "z"`, `lto = true`, `codegen-units = 1` for the wasm. Trunk runs `wasm-opt -Oz`.

### 5.2 The graph canvas is imperative, not reactive
The layout moves every card every frame. Pushing a few hundred transforms through reactive signals per frame costs
something and buys nothing. So `ui::graph_view::GraphView` is a plain struct over `web-sys` that mirrors `graph.js`:
- It owns the card elements and rebuilds the edge SVG.
- It runs the `requestAnimationFrame` loop and handles pointer, wheel and gesture events.
- Leptos mounts the canvas container and hands over the `NodeRef`s.

Everything else (chat, inspector, toolbar, popovers, dialogs) is ordinary Leptos components.

The **model** (`core::graph::Graph`) and the **physics** (`core::layout`) know nothing about the DOM. `GraphView`
passes in the measured card heights and label widths. Card heights are cached: they are refreshed when a card
re-renders and through a `ResizeObserver`, instead of reading `offsetHeight` for every node on every tick.

### 5.3 Ollama calls stay in the page
- **Transport** (`ui::ollama`): `web-sys` `fetch` + `AbortController`, reading the `ReadableStream` (via
  `wasm-streams` or a manual reader). This matches `ollama.js` exactly. Stop still closes the connection, so Ollama
  stops generating (L03). The extraction timeout still aborts with a `timeout` reason, kept in a Rust enum next to the
  controller.
- **Protocol** (`core::ollama`, tested natively): the request/response types, an NDJSON accumulator (content,
  thinking, `done`, `error`), `parse_loose_json` with its salvage passes, error-text extraction, `NOTE_SCHEMA`, and
  the think → no-think → `format: "json"` fallback decision.
- Base URL: the desktop app uses the configured host. The page's origin is `tauri://localhost`, which Ollama accepts
  by default (L02); spike S2 confirms this. Browser mode uses `''`, i.e. same-origin `/api` through the server proxy.
- Routing through the Rust backend with reqwest was rejected. It would need streaming channels and cancellation
  plumbing over IPC, and browser mode would still need the page path.

### 5.4 PDFs: keep pdf.js
pdf.js gives the best text extraction available, and it works today. A `#[wasm_bindgen(module = "/js/pdf_shim.js")]`
binding exposes `extract_pages(bytes, on_progress) -> Vec<String>`. The Rust side keeps the line/space heuristics
(baseline moves > 2 → newline, x gap > 1.5 → space), so they are tested. A Rust-only extractor (`pdf-extract`/`lopdf`)
is noticeably weaker on real PDFs. It's an M6 evaluation, not part of the port (§12).

HTML files keep going through the web view's `DOMParser` (script/style/noscript/template removed, then `innerText`),
so the output matches today's.

### 5.5 Markdown: pulldown-cmark, sanitized
Porting `markdown.js` 1:1 would need `fancy-regex`, because it uses `(?!\w)` lookahead and Rust's `regex` has no
lookaround. It would also keep that renderer's limitations. Instead, `core::markdown` wraps **pulldown-cmark**:
- `Html`/`InlineHtml` events become escaped text.
- Links are kept only for `http`/`https`, rendered with `target="_blank" rel="noopener"`.
- Images become plain links, so replies never load remote content.
- Heading levels shift by +2, capped at h6, as today.
- Fenced code keeps `class="lang-…"`.
- Tables and strikethrough are enabled, so `.msg-body table` CSS gets added.

This is a deliberate, visible improvement: model replies use tables a lot. If exact output parity matters more, see §12.

### 5.6 Persistence and data compatibility
- **Location:** `dirs::data_dir()/Ollama Notes/notes.json`. This is *not* Tauri's `app_data_dir()`, which would be
  `…/Application Support/local.ollama-notes/`.
- **Override:** read `DataDirectory` through `NSUserDefaults` (`objc2-foundation`). That covers both
  `defaults write local.ollama-notes DataDirectory …` and the `-DataDirectory <path>` launch argument used for test
  isolation (L11). Expand `~`. This depends on the bundle id staying `local.ollama-notes`. Other OSes would use
  `OLLAMA_NOTES_DATA_DIR`.
- **Save:** create the directory. On the first save of each session, replace `notes.backup.json` with a copy of the
  existing file. Write atomically: a temp file in the same directory, then rename
  (`tempfile::NamedTempFile::persist`). **Load:** returns `None` when the file is missing. The page keeps its `loaded`
  guard, so nothing is saved before the load finishes (L14).
- **Schema** (`core::model`): serde structs with `rename_all = "camelCase"`.
  - Every field is `#[serde(default)]`, because the JS merges saved data over defaults.
  - Every object (state, settings, message, node, edge) has `#[serde(flatten)] extra: Map<String, Value>`, so fields
    this version doesn't know about survive a round trip.
  - Every JS number becomes `f64`.
  - Collections the JS keeps in a `Map`/`Set` become `IndexMap`/`IndexSet`, because insertion order affects JSON
    output, layout iteration and keyword-link order.
  - `Node.kind` is an enum that keeps unknown strings.
  - `kwText` may be `null`, and `text` may be absent.
  - `_drag` is runtime-only and never serialized.
  - Exports add `app: "ollama-notes"`, `version: 1` and `exportedAt`, pretty-printed with two spaces (as
    `serde_json::to_string_pretty` does). Import keeps today's merge rule: current settings, overlaid by the imported
    ones.
- **Browser mode** keeps the `localStorage` key `ollama-notes.v1` on `http://localhost:8765`, so existing browser-mode
  data carries over without an import.

### 5.7 Dialogs, file picking and drops
- **Confirm** (Clear map, Import map): a `confirm(message)` command uses `tauri-plugin-dialog` `ask`, with the window
  as parent so it shows as a sheet. It keeps today's split: the question up to the first `?` in bold, the rest as
  informative text. If S3 shows it isn't a sheet, fall back to `NSAlert beginSheetModal` via `objc2-app-kit`.
- **Open** (+ File, ⌘O, Import map ⇧⌘O): a `pick_files(kind)` command shows the native open panel and returns names
  and paths. The page reads each file with `read_file(path)`, which returns raw bytes through `tauri::ipc::Response`.
  The backend reads **only paths that came out of a panel this session**, so the page can't read arbitrary files.
- **Drops:** set `dragDropEnabled: false` on the window so WKWebView delivers ordinary HTML5 `drop` events with
  `File` objects, as today.
- **Save** (Export map): `save_file(name, text)` shows the save panel as a sheet, writes atomically and returns the
  path for the toast. Never press OK on a real panel in tests (L08).
- Pickers and drops feed one pipeline, `(name, mime, bytes)` → `documents::parse`.
- `ui::platform::Platform` is an enum (`Desktop` | `Browser`), not a trait object. Desktop calls `invoke`/`listen` on
  `window.__TAURI__`, via `app.withGlobalTauri: true` and a small wasm-bindgen extern block. Browser mode uses
  `localStorage`, `<input type=file>`, `window.confirm` and a blob download, as today. The app is detected by the
  presence of `window.__TAURI_INTERNALS__`.

### 5.8 Quitting without losing the last edit
Today, `applicationShouldTerminate` returns `.terminateLater` and awaits the page's `desktopFlush()`, with a 3 s
fallback (L14). In Tauri, three exits all go to one `begin_quit()`:
- ⌘Q, as a **custom** Quit menu item rather than the predefined one, so the app controls it.
- The close button: `WindowEvent::CloseRequested` → `prevent_close`.
- `RunEvent::ExitRequested` → `prevent_exit`.

`begin_quit()` emits `flush` to the page. The page clears its debounce, awaits `save_state`, then invokes
`flush_done`, and the backend calls `app.exit(0)`. A 3 s timer is the fallback, and a re-entrancy guard covers
repeated requests. Spike S4 checks that Dock › Quit and logout take this path too. If one of them doesn't, the
fallback design moves the debounce into the backend: the page sends every change, and the backend writes
synchronously in `RunEvent::Exit`.

### 5.9 Browser mode in Rust
`server/` uses axum + tokio + reqwest + clap + rust-embed:
- It embeds `ui/dist`. In debug builds, rust-embed reads from disk instead, so edits show up without a rebuild.
- It keeps all of `server.py`'s behaviour (§10).
- The proxy streams reqwest's body into an axum `Body`. When the browser disconnects, the response stream is dropped,
  which closes the upstream connection, so Ollama stops (checked in M2).
- The Ollama launcher is shared with the app (`notes-native`), using Swift's better version: bundle id lookup
  (`open -g -b com.electron.ollama`), then the `ollama` CLI on `PATH`, `/opt/homebrew/bin`, `/usr/local/bin` and
  `~/.local/bin`, run detached with null stdio in a new process group. It only runs for local hosts.

Trade-off: browser mode stops being "nothing but Python 3". It needs a built binary, or `cargo` to build one (§12).

### 5.10 Icon
`tools/make-icon` ports `make-icon.swift` to **tiny-skia** with the same geometry: a 1024 canvas, an 824 tile at 100
with radius 186, the gradient, the dot grid, the hub and the four cards. tiny-skia has no shadow, so the drop shadow
is a blurred alpha layer. It writes `src-tauri/icons/icon.png`. `cargo tauri icon` generates the `.icns` and the
other sizes. The outputs are committed and regenerated only when the tool changes.

### 5.11 Conventions
- Libraries (`core`, `native`) use `thiserror` error enums and contain no `unwrap()`/`expect()` outside tests.
  Binaries (`src-tauri`, `server`, tools) use `anyhow` with `.context(…)`.
- Shared dependency versions live in `[workspace.dependencies]`. Each crate is pinned when it's added, not in this plan.
- Regexes are compiled once with `std::sync::LazyLock`.

## 6. Where each piece goes

| Today | Goes to |
|---|---|
| `app.js` state, defaults, persistence | `core::model` (types, defaults, merge rules) + `ui::state` (signals, debounce, `loaded` guard) |
| `app.js` chat | `core::extract::build_messages` (system prompt, pinned context, last 30) + `ui::chat` |
| `app.js` extraction | `core::extract` (schema, prompt builders, `apply_notes`, `apply_extraction`) + `ui::extract` (timeouts, cancel, status) |
| `app.js` documents | `core::text` + `ui::documents` (pipeline, import dialog, sectioned extraction) |
| `app.js` inspector, settings, export/import, wiring | `ui::{inspector, settings, transfer, toolbar, commands}` |
| `graph.js` model and placement | `core::graph` (`Graph`, `add/update/remove`, `find_by_title`, `neighbors`, `degree`, `place_near`) |
| `graph.js` physics | `core::layout` (`Sim`, `tick`, `tidy`) |
| `graph.js` DOM, events, view | `ui::graph_view` (render, pan/zoom/pinch, drag, link handle, fit/centerOn animation) |
| `keywords.js` | `core::keywords` |
| `documents.js` | `core::text` (`normalize`, `chunk_text`, `title_from_filename`, PDF line heuristics) + `ui::documents` (`DOMParser`, pdf.js shim) |
| `ollama.js` | `core::ollama` (protocol) + `ui::ollama` (transport, capabilities cache, tags) |
| `markdown.js` | `core::markdown` |
| `platform.js` | `ui::platform` + `core::host::normalize_host` |
| `index.html`, `style.css` | `ui/index.html` (Trunk), `ui/style.css` (unchanged + table rules); markup moves into `view!` |
| `AppDelegate.swift` | `src-tauri/src/{main, menu, window, quit, links}.rs` + `tauri.conf.json` |
| `Bridge.swift` | `src-tauri/src/commands.rs` + `notes-native::{store, data_dir, launcher}` |
| `WebAssets.swift` | dropped: Tauri serves `ui/dist` itself |
| `Info.plist` | `tauri.conf.json` (identifier, product name, min macOS 14, category, copyright) + `src-tauri/Info.plist` (merged: `NSAllowsArbitraryLoadsInWebContent`, `NSLocalNetworkUsageDescription`) |
| `make-icon.swift` | `tools/make-icon` |
| `build.sh` | `build.sh` → `cargo tauri build --target universal-apple-darwin --bundles app`, then the same `--open`/`--install` |
| `server.py`, `Ollama Notes.command` | `server/` + the updated `.command` |

## 7. Porting notes

### 7.1 JS → Rust traps
- **Iteration order.** JS `Map`/`Set` iterate in insertion order, so use `IndexMap`/`IndexSet`. Delete with
  `shift_remove`, never `swap_remove`, which reorders.
- **String lengths.** JS `.length` and `.slice(0, n)` count UTF-16 code units. Rust `&s[..n]` counts bytes and panics
  mid-character. Add `truncate_chars` and use it everywhere the JS slices: titles 80/70, chat title 60, user text
  4,000, assistant 6,000, document context 6,000, note body 2,000, document preview 500. `chunk_text` works on char
  indices. Results differ from JS only around characters outside the BMP (emoji), which is acceptable. The property
  tests check that nothing panics.
- **Regex.** Rust's `regex` has no lookaround. The only use is in `markdown.js`, which pulldown-cmark replaces. JS
  `\w`, `\d` and `\b` are ASCII-only, while Rust's are Unicode, so write `[A-Za-z0-9_]` and `[0-9]` where the JS used
  them: `TEXT_EXT`, `^\d` in `usable`, `:\d+$` in `normalizeHost`. `\p{L}` and `\p{N}` work as they are.
- **Sorting.** Both JS `sort` and Rust `sort_by` are stable. Compare floats with `total_cmp`. For model names,
  `localeCompare` becomes `js_sys::JsString::locale_compare` in the UI.
- **Randomness** (ids, the placement fallback, jitter for coincident nodes): inject a `fastrand::Rng`, seeded from
  `Date.now()` in the page and fixed in tests. Keep `getrandom` out of the wasm build, or enable its `wasm_js`
  backend. Ids stay opaque strings in the same base-36 shape, and existing ids load unchanged.
- **Number coercion.** `+value || default` followed by a clamp (temperature, max notes, section size, max sections)
  becomes "parse, fall back, clamp", with the same bounds.
- **Locale formatting.** `toLocaleString`/`toLocaleDateString` for dates and character counts use `js_sys` in the UI.
  Core functions take preformatted strings, such as the hub body "Conversation started …" and the document metadata
  line.
- **Safari gesture events** (`gesturestart/change/end`) have no `web-sys` type. Listen for a plain `Event` and read
  `scale`, `clientX` and `clientY` through `js_sys::Reflect`. The `wheel` listener needs `passive: false`.
- **Closures.** Keep event-listener and rAF `Closure`s in `GraphView`, or `forget` the app-lifetime ones. A dropped
  closure throws "closure invoked recursively or after being dropped".
- **Raw HTML** reaches the DOM in exactly two places: rendered Markdown and the edge SVG string. The SVG builder
  escapes labels. Everything else is a text node, which Leptos escapes by default.
- **Tauri details:**
  - Command arguments arrive camelCase from the page and map to snake_case in Rust.
  - `generate_context!` embeds `ui/dist` at compile time, so building or linting `src-tauri` fails if `ui/dist` is
    missing. `scripts/check.sh` runs `trunk build` first.
  - Release builds need the `devtools` feature to keep Inspect Element.
  - The CSP must allow wasm and arbitrary Ollama hosts: `default-src 'self'; script-src 'self' 'wasm-unsafe-eval';
    connect-src 'self' ipc: http://ipc.localhost http: https:; worker-src 'self'; style-src 'self' 'unsafe-inline';
    img-src 'self' data:`.
- **pdf.js path.** wasm-bindgen copies the shim to `dist/snippets/<crate>-<hash>/`, which breaks
  `import.meta.url`-relative paths. Load pdf.js and its worker from absolute `/vendor/…` instead. This works under
  both `tauri://localhost/` and `http://localhost:8765/`.
- **Local-host check.** `url::Url::host_str()` returns `[::1]` with brackets, but Swift compared against `::1`.
  Accept both, alongside `localhost`, `127.0.0.1` and `0.0.0.0`.

### 7.2 Constants and behaviour to carry over verbatim
Don't retune anything during the port. The golden tests pin most of these.
- **Extraction:**
  - Request: `num_predict` 900, temperature 0.2, `repeat_penalty` 1.15, a 90 s timeout per call or section, and
    `think: false` when the model reports the `thinking` capability.
  - On a non-OK response, retry without `think`, then with `format: "json"`.
  - Limits: max notes 1–8 (default 4), at most 8 applied. The schema limits are 80/400/80/40 characters and 3/5/8
    items.
  - Prompt input: the last 80 existing titles joined with `"; "`, the user's text truncated to 4,000 characters and
    the assistant's to 6,000. Both prompts and system messages are copied verbatim.
  - `apply_notes` merge rules: append the summary when it's new; model keywords are capped at 5, the union at 8.
    Hub creation follows the same rules as today.
- **Chat:** the default system prompt verbatim, temperature 0.7, history of the last 30 complete, error-free
  messages, pinned documents truncated to 6,000 characters with the "first part" marker, the title taken from the
  first message (60 characters), and "Stopped." for an empty aborted reply.
- **Models:** the tags request times out after 5 s. Re-poll after 30 s when models are present, 1 s within 30 s of
  starting Ollama, and 4 s otherwise. Start Ollama once per launch or server change. Auto-select the first model
  whose name doesn't contain `embed`.
- **Timers:** save debounce 250 ms, view-change save 300 ms, keyword refresh 900 ms after edits.
- **Map view:** card width 220, default height 110, zoom 0.12–2.5, fit 0.12–1.5 with padding 50, `centerOn` minimum
  zoom 0.8, view animation 380 ms with cubic ease-out, 340 px kept clear for the inspector, drag threshold 3 px.
- **Placement:** check the view centre first, then 14 rings. Ring radius is 170 + 120·ring, with 6 + 4·ring steps,
  starting at a golden-angle offset seeded by the node count. The y offset is scaled by 0.7, with 28 px padding.
  Fall back to a random ±600.
- **Physics:**
  - Forces: repulsion 30000·repulsion·mᵃ·mᶜ / max(d², 120²) with y scaled by 1.6. Springs have k = 0.09·tension·
    (0.6 + 0.45·log₂(1+deg)) and rest length min(560, 250 + 28·deg). Gravity is 0.018·gravity·m, with mass
    m = 1 + min(4, 0.4·deg).
  - Integration: damping 0.5, maximum speed 16, rest speed 0.05.
  - Collision: padding 26, relax 0.3.
  - Label clearance: height 18, padding 14, relax 0.15, width = measured or 6.2 px per character, + 12.
  - Cooling: alpha eases by 0.028. The loop stops below 0.004, with a 240-frame tail while corrections exceed 0.4.
    Dragging sets target 0.18.
  - Kick strengths per operation: add 0.7, edge 0.6, remove 0.5, edge remove 0.4, lock 0.2, load 0.25.
  - `tidy`: 320 synchronous steps when physics is off.
- **Keywords:**
  - Selection: 6 per note, model keywords first. The score is (count + 2·inTitle) × (1.3 if a phrase) × (0.5 if a
    single body occurrence) × idf, kept above 0.35. Phrases absorb their own words.
  - Linking: idf = ln((max(N,6)+1)/(df+0.5)). Keys shared by more than max(3, ⌈N/2⌉) notes are skipped. The label is
    the top two shared keys. Default threshold 1.0, slider 0.5–3.
  - Edits: `kwIgnore` pairs are respected, and a manual keyword list is capped at 12.
- **Documents:** the file types in `ACCEPT`/`TEXT_EXT`, default section size 2,400 (800–8,000), default max sections
  20 (1–200), the hint's "~8 s per section" estimate, and the preview and metadata line formats.

## 8. Testing

- **Golden fixtures from the current JS, without Node.** macOS ships JavaScriptCore's shell at
  `/System/Library/Frameworks/JavaScriptCore.framework/Versions/A/Helpers/jsc` (present on this Mac).
  `tools/golden/*.mjs` run under `jsc -m`. They import `keywords.js`, `documents.js` (`chunkText`,
  `titleFromFilename`) and `platform.js` (`normalizeHost`). For module-private code, they evaluate source text:
  `parseLooseJSON`, and `NodeGraph.prototype._tick` on a stub object with `nodes`, `edges`, `_sim`, `_vel`, `_els`
  and `_labelW`. They write JSON to `crates/core/tests/fixtures/`. The fixtures are committed, so they outlive the JS.
  - Inputs: a hand-written corpus of ~40 notes (phrases, plurals, -ing/-ed, stop words, non-ASCII, digits), randomly
    generated maps with fixed seeds and no coincident nodes, chunking texts (long paragraphs, no sentence breaks,
    CRLF, NBSP), and loose-JSON samples (prose-wrapped, truncated, only partial triples).
  - Assertions: exact equality for strings and sets. Floats within 1e-9. Layout positions within 1e-6 after 200
    ticks.
  - If `jsc -m` module loading turns out not to work, concatenate the sources with `export` stripped.
- **Unit tests:**
  - Store: backup happens once per session, writes are atomic, a missing file returns `None`, `~` expands.
  - Launcher: local-host detection, CLI search order against a fake `PATH` directory.
  - `normalize_host`.
  - `chunk_text` properties with `proptest`: never panics on arbitrary Unicode, keeps every word in order, and
    respects the size bounds.
  - Markdown safety: `<script>`, raw HTML, `javascript:` links and images must not produce live tags.
  - Prompt builders, as snapshot tests.
- **Data compatibility:** round-trip deserialize → serialize → compare as `serde_json::Value` for three files:
  - a copy of the live `notes.json`,
  - a real export from the current app with chat, notes, keyword links, a locked node, `kwIgnore` and a document
    (made in M0),
  - a file with unknown extra fields.
- **Ollama:** a mock NDJSON server built with axum in the tests covers streaming, mid-stream errors, a 400 on the
  schema (to drive the fallback chain) and truncated JSON. Plus one manual run against real Ollama, checking that
  Stop cancels on the server (L03: a short request duration in `~/.ollama/logs/server.log`).
- **Server:** integration tests cover static files with the right MIME types (including `.mjs`) and `no-store`,
  proxy streaming, a 502 JSON response when Ollama is down, a 405 for a non-API POST, already-running detection and
  moving to the next free port.
- **UI:** `wasm-bindgen-test` for the parts that touch the DOM: `toWorld`/`zoomAt`/`fit` math, event-to-action
  mapping, and HTML → text. It runs headless in Chrome/Firefox if installed, or in Safari through `safaridriver`
  (a one-time `safaridriver --enable` needs admin).
- **End to end on macOS:** `tauri-driver` (WebDriver) doesn't support macOS, so the parity checklist (§10) is run by
  hand. Test builds use a separate identifier (a `tauri.test.conf.json` overlay → `local.ollama-notes.test`) and
  `-DataDirectory <tmp>`, and are cleaned up afterwards with `defaults delete local.ollama-notes.test`. If manual runs
  get slow, add a debug-only `--script steps.json` hook, as in L11.
- **Before any run that opens windows or sheets on your screen,** I'll say what will appear and for how long, and ask
  you not to click it (L10).
- **Quality gate** (`scripts/check.sh`, must pass before each commit):
  `trunk build` → `cargo fmt --all -- --check` → `cargo clippy --workspace --all-targets -- -D warnings` →
  `cargo clippy -p notes-ui --target wasm32-unknown-unknown -- -D warnings` → `cargo test --workspace`.

## 9. Milestones

Sizes are relative (S/M/L). The expected result is roughly 5k lines of Rust: core ~1.5k, UI ~2.5–3k, shell and
native ~600, server ~300, icon ~150. That compares with ~2.7k lines of JS, Swift and Python today. These are
estimates.

### M0. Groundwork and spikes (S)
1. `git init` with a `.gitignore` (`build/`, `target/`, `ui/dist/`, `.DS_Store`). Commit the current tree as the
   baseline. There is no history today, and the old code is the specification.
2. Copy `notes.json` aside. Make a non-trivial export from the current app to use as the compatibility fixture.
3. Toolchain: `rustup target add wasm32-unknown-unknown x86_64-apple-darwin`, then
   `cargo install --locked trunk tauri-cli`.
4. Workspace skeleton, `scripts/check.sh`, an empty crate in each slot.
5. **Spikes**, each throwaway, with results written back into this file:
   - **S1:** a Tauri window loads a Trunk-built Leptos page from `tauri://localhost` under the CSP above. A pdf.js
     module worker starts from `/vendor/`, and `.mjs` is served with a JS MIME type.
   - **S2:** from that page, a streamed `fetch` to `127.0.0.1:11434` works. Check with the L02 preflight `curl` using
     `Origin: tauri://localhost`. Aborting mid-stream stops Ollama (L03 log check).
   - **S3:** record which of `confirm()`, `alert()`, `<input type=file>` and HTML5 file drop (with
     `dragDropEnabled: false`) work in Tauri's WKWebView. Check that the dialog plugin's `ask` and save panel show as
     sheets.
   - **S4:** custom ⌘Q, the close button, Dock › Quit and `osascript -e 'quit app "Ollama Notes"'` all reach
     `begin_quit`. A title typed just before quitting is in `notes.json`.
   - **S5:** `NSUserDefaults` returns `DataDirectory` from `defaults write` and from `-DataDirectory /tmp/x`.
   - **S6:** a `GraphView` prototype with 300 cards, 400 edges and physics on holds a steady frame rate in the web
     view. Pinch gesture events arrive.
   - **S7:** `cargo tauri build --target universal-apple-darwin` with ad-hoc signing launches natively and under
     `arch -x86_64`. Record bundle size and first-build time.

   **Go/no-go:** if S2 or S6 fails, revisit §5.3 or §5.2 before M1.

Exit: spike results recorded, and the skeleton passes the gate.

### M1. Tauri shell around the existing JS UI (M)
- `notes-native`: data directory, store with backup, launcher (ports `Bridge.swift`).
- `src-tauri`:
  - Window: 1280×820, minimum 760×480, dark theme, transparent title bar, hidden title, background `#161923`.
    Window-state plugin to remember the frame.
  - The full menu tree from `AppDelegate.swift`, with the README shortcuts. Custom items emit `menu` events carrying
    today's command names. Window and Help menus are registered with AppKit. Keep an explicit Enter Full Screen item
    (L09).
  - Commands (§5.7), quit flush (§5.8), the single-instance plugin.
  - External links: `on_navigation` allows only the app's own origin, and `http`/`https`/`mailto` go to the default
    browser via the opener plugin. Show Data in Finder uses reveal-in-Finder, or opens the directory.
  - `devtools`, the Info.plist additions, ad-hoc signing (`signingIdentity: "-"`), minimum macOS 14.
- `frontendDist` points at `web/` for now. `platform.js` gets a Tauri branch (`invoke`/`listen` instead of
  `webkit.messageHandlers`, a `flush` listener and a `menu` listener). Confirm and the file pickers go through the new
  commands wherever S3 showed the WKWebView defaults don't work. This JS is throwaway, about 40 lines.
- `build.sh` builds through `cargo tauri build` and keeps `--open`/`--install`.

Exit: the "Shell" and "Data" sections of §10 pass with the old UI, and the app opens the existing `notes.json`.
`macos/Sources` is deleted. `make-icon.swift` stays until M5.

### M2. Browser-mode server (S)
- `server/` covers all of `server.py`'s behaviour:
  - Flags `--port`, `--no-browser` and `--no-ollama`; environment variables `PORT` and `OLLAMA_HOST` (adding
    `http://` if it's missing).
  - Already-running detection via `<title>Ollama Notes</title>`, then the next free port.
  - Start Ollama and wait up to 25 s, polling every 0.5 s. Print the banner with the model list.
  - Serve static files with `Cache-Control: no-store`.
  - Stream `/api/*` for GET, POST and DELETE to Ollama, with a 600 s timeout, a 502 JSON message when Ollama is
    unreachable and a 405 for other methods outside `/api`.
  - Log quietly for static assets. Open the browser after 0.6 s. Ctrl+C prints "bye".
- `Ollama Notes.command` runs the release binary, building it with `cargo` first if it's missing.

Exit: browser mode works with the old UI, `server.py` is deleted, and the server tests pass.

### M3. notes-core with golden tests (M)
- `model`, `graph`, `layout`, `keywords`, `text`, `host`, `ollama` (protocol), `extract`, `markdown`, as in §6 and §7.
- `tools/golden` and its fixtures, then the unit, property and compatibility tests from §8.

Exit: all core tests pass, and `cargo build -p notes-core --target wasm32-unknown-unknown` succeeds.

### M4. Leptos UI (L)
Each step ends with something you can run:
1. **Shell:** Trunk `index.html`, `style.css` verbatim, layout as `view!`, toast, status pill, splitter (chat width
   saved), toolbar wrap with the inspector offset through `ResizeObserver` (L22), empty-map hint.
2. **State and persistence** through `Platform`: load guard, 250 ms debounce, flush on request, failure toasts.
3. **Ollama:** transport, tags polling, status, start Ollama once. Settings dialog: host normalization, host row only
   in the app, proxy note only in a browser.
4. **Chat:**
   - Streaming with rAF-throttled repaint, collapsed Thinking, Stop.
   - Error and "Stopped." handling, title from the first message.
   - New chat, which aborts the stream and extractions.
   - Note chips under replies, Extract notes and Save as note.
   - Markdown, including the table CSS.
5. **GraphView:**
   - Nodes and edges, including dashed keyword links with labels.
   - Pan, wheel, ⌘/Ctrl + wheel and pinch zoom.
   - Dragging with live reflow, the link handle, edge selection with Delete/Backspace, Escape, double-click to add.
   - Filter (keywords included), flash, fit and `centerOn` animation, the physics loop, Tidy, lock, the Forces
     popover.
6. **Extraction**, automatic and manual: timeout and cancel, the status line with a cancel link, toasts, auto-fit
   when the layout settles.
7. **Inspector:**
   - Title, kind, body, keywords (manual list and "auto").
   - Links, where removing a keyword link adds the pair to `kwIgnore`.
   - Pin to context and the context bar, Ask about this, Show source (scroll and highlight), Lock, Delete.
   - Keywords popover: toggle, strictness, Link now, Remove keyword links.
8. **Documents:**
   - Input: pickers and drops (with the drop highlight) → parse, with per-page progress for PDFs and the "no text
     found" toast.
   - Import dialog with the live hint.
   - Sectioned extraction: progress, cancel, failure count, the final summary toast.
9. **Export/import, Clear map, menu commands, flush.** Then point `frontendDist` and the server's embedded directory
   at `ui/dist`.

Exit: the whole §10 checklist passes in the app and in browser mode.

### M5. Cut-over and cleanup (S)
- `tools/make-icon` (§5.10), then `cargo tauri icon`. Compare by eye with the current `AppIcon.icns`.
- Delete `web/` (pdf.js moves to `ui/vendor/`), `macos/` and `build/obj`.
- Rewrite the README's "Build and run", "Browser mode" and "How it's put together". Write
  `.claude/lessons/<date>-rust-rewrite.md`.
- Record release bundle size, launch time and build time next to today's: 2.8 MB, and about 80 s for the first build.

Exit: `./build.sh --install` produces the app, and you sign off on the checklist.

### M6. Optional, after parity
Windows/Linux bundles, Barnes–Hut repulsion for large maps, an evaluation of a Rust PDF extractor against a corpus,
keeping documents' full text outside `notes.json`, a "Paste and Match Style" item via objc2.

## 10. Parity checklist

**Shell**
- [ ] One window at 1280×820, minimum 760×480; dark, transparent title bar; no white flash; position remembered.
- [ ] Menus and shortcuts: ⌘N, ⇧⌘N, ⌘O, ⇧⌘O, ⇧⌘S, ⌘L, ⌘F, ⌘,, ⌘0, ⌘=, ⌘−; Edit menu (undo/redo/cut/copy/paste/
      select all); View › Enter Full Screen; Window menu; Help › Ollama Model Library; About/Hide/Hide Others/Show All/Quit.
- [ ] Show Data in Finder (reveals the file, or opens the folder if there's no file yet).
- [ ] A second launch activates the running copy.
- [ ] Reply links open in the default browser, and the window never navigates away.
- [ ] Right-click › Inspect Element.
- [ ] Ollama is started if it isn't running: Ollama.app by bundle id, otherwise `ollama serve` from the known
      paths, with "Starting Ollama…" and then a connection.
- [ ] Confirm sheets for Clear map and Import map; open panels with multiple selection; save panel for export
      with a toast showing the path.

**Data**
- [ ] Opens the existing `notes.json` and earlier exports unchanged. Round trips keep unknown fields.
- [ ] A `notes.backup.json` is made once per session. Writes are atomic. `DataDirectory` works via `defaults`
      and the launch argument.
- [ ] An edit made just before ⌘Q, closing the window or Dock › Quit is saved.
- [ ] A failed save shows a toast. Nothing is saved before the load finishes.

**Chat**
- [ ] Model picker (with parameter sizes), status pill, reconnect polling, "no models" toast.
- [ ] Streaming, collapsed Thinking, Stop cancels on the server, errors shown inline.
- [ ] Pinned notes and documents go into the system prompt. History is capped at 30 messages.
- [ ] Auto-notes toggle, Extract notes, Save as note, chips that center the map on a note, New chat.

**Map**
- [ ] Pan by drag and scroll; zoom with ⌘/Ctrl + scroll, pinch and the buttons; double-click to add; drag
      notes; link by handle; select an edge and delete it.
- [ ] The force layout matches today's feel: no overlaps with cards or labels, no bounce. Physics toggle,
      Forces sliders and reset, Tidy, Lock position.
- [ ] Keyword links: automatic on add/edit, strictness, Link now, Remove keyword links, `kwIgnore` respected,
      manual keyword lists stick and "auto" reverts.
- [ ] Fit, auto-fit setting, filter (keywords included), note/link count, flash on new notes, empty-map hint.

**Documents**
- [ ] ＋ File, ⌘O and drag-and-drop for text-like files, HTML (to text) and PDFs (with page progress); a
      scanned PDF gets the "no text found" toast.
- [ ] Import dialog: per-file stats, extract toggle, section size, max sections, keep full text, live
      estimate.
- [ ] Section-by-section extraction with progress, cancel, timeout handling and a summary toast. The hub
      note keeps the full text, and pinning it gives the model the first 6,000 characters.

**Settings and transfer**
- [ ] Ollama server (app only, normalized), chat model, extraction model, system prompt, temperature, max
      notes, auto-fit.
- [ ] Export/Import of the map and chat as JSON, compatible with the old app and browser mode.

**Browser mode**
- [ ] `./server` flags and environment variables, already-running detection, next free port, starts Ollama,
      banner, opens the browser.
- [ ] Streaming proxy (Stop still cancels), 502 message, `localStorage` data from the Python version still
      there.

## 11. Risks and known regressions

| Risk / regression | Impact | Mitigation |
|---|---|---|
| Ollama rejects the `tauri://localhost` origin, or abort doesn't cancel generation | Chat broken or wasteful | Spike S2. Fallback: stream through the Rust backend with reqwest over a Tauri `Channel`, cancelling by dropping the future. |
| A quit path skips the flush | Last edit lost | Spike S4. Fallback: the backend owns the debounce and writes in `RunEvent::Exit` (§5.8). |
| Layout loop too slow in wasm or through `web-sys` calls | Janky map | Spike S6. Cached heights, one SVG rebuild per frame, Barnes–Hut later. |
| Dialog plugin sheets aren't window-modal | Small UX change | objc2 `NSAlert`/`NSSavePanel beginSheetModal` fallback. |
| Leptos API churn between releases | Rework | Pin exact versions once M0 settles; the UI is the only crate that depends on it. |
| Markdown output changes (pulldown-cmark) | Replies render a bit differently, mostly better | Deliberate; see §12. |
| No auto-reload when the web content process crashes | App needs a relaunch after a WebKit crash | Tauri doesn't expose the hook. Accept for now. |
| "Paste and Match Style" menu item | Missing item | Tauri's menu API has no arbitrary selectors. M6 via objc2, if wanted. |
| Bigger app and slower first build than today's 2.8 MB / ~80 s | Disk use and build time | Measured in S7. A release-profile size pass. |
| Browser mode needs a Rust-built binary instead of system Python | Harder to run on another machine | Ship release binaries, or decide to drop browser mode (§12). |
| Saved window position from the Swift app isn't migrated | Opens centered once | Accept. |

## 12. Open questions for you

These are the choices where your preference changes the plan. Each has a default the plan already assumes.

1. **Keep browser mode?** The Tauri app can also be built for Windows and Linux, so browser mode is less necessary
   than it was. *Default: keep it (M2), since it's small.*
2. **Markdown:** is it fine to switch to pulldown-cmark, with tables and slightly different output? Or do you want
   an exact port of today's renderer? *Default: pulldown-cmark.*
3. **pdf.js:** is it OK to keep it as the one vendored JS library? Or must the app be 100% Rust, accepting weaker PDF
   text extraction? *Default: keep pdf.js.*
4. **Platforms:** macOS only for now, or Windows/Linux builds too? *Default: macOS only; Windows/Linux in M6.*
