# Ollama Notes

A Mac app for chatting with a locally hosted [Ollama](https://ollama.com) model (or a cloud model, with your API key),
with a node-based notes map that fills itself in as you talk. Native window, menu bar and file panels. Written in
Rust: a Tauri desktop shell around a page compiled to WebAssembly.

## Build and run

```bash
./build.sh             # builds build/Ollama Notes.app
./build.sh --open      # ...and launches it
./build.sh --install   # ...and copies it to /Applications
```

Building needs [Rust](https://rustup.rs) plus two targets and two cargo tools (no Node):

```bash
rustup target add wasm32-unknown-unknown x86_64-apple-darwin
cargo install --locked trunk tauri-cli
```

The app runs on macOS 14 or later, on Apple silicon and Intel. It is signed ad hoc, which is all this Mac needs. If you
copy it to another Mac, right-click it and choose **Open** the first time.

The app talks to Ollama at `http://127.0.0.1:11434`. If Ollama isn't running there when the app starts, it opens
Ollama.app (or runs `ollama serve`) and connects once it's up. For Ollama on another machine, change
**Settings › Ollama server**. You need at least one model pulled, e.g. `ollama pull llama3.2`.

### Cloud models

To use models from a cloud service instead, pick it in **Settings › Models from** and paste an API key. The list has
OpenAI, Anthropic, Google Gemini, OpenRouter, Groq, Mistral, DeepSeek, Together AI and xAI; **Another
OpenAI-compatible API** takes any address that serves `/models` and `/chat/completions` and accepts requests from web
pages (CORS), such as LM Studio at `localhost:1234/v1`. The page calls the service directly, so your messages and any
pinned notes go to it. The key is saved with your settings in `notes.json` (or the browser's storage) and is left out
of exports. Ollama's own cloud models don't need a key here: run `ollama signin` and pull one, and it shows up under
Ollama.

Thinking is shown when the service streams it (DeepSeek, OpenRouter, Groq). If a model rejects the temperature or the
note-extraction JSON schema, the request is retried without it.

### Web search

With **Web search** on (the toggle next to Auto-notes), Ollama models that can call tools research before they
answer: the model searches the web, reads the pages it needs, and answers with links to its sources. The searches and
pages behind a reply are listed above it. `ollama show <model>` lists *tools* under Capabilities for models that can
(qwen3, llama3.1, gpt-oss and others); with other models a toast says so and they answer without searching.

The searches go through Ollama's own web search, which uses the ollama.com account you sign in with (`ollama signin`,
once), so there's no key to set up here. A free account has an hourly limit on searches; when it runs out, the model
answers without searching and the reply says why. Search queries, and the addresses of pages the model reads, go to
ollama.com. Cloud models from **Settings › Models from** don't get web search.

While web search is on, chats ask Ollama for a 16K-token context instead of the default 4K, so the results fit. Note
extraction with the chat model asks for the same, so Ollama doesn't reload the model between the two.

### Your data

Everything is saved as you go to `~/Library/Application Support/Ollama Notes/notes.json` (**File › Show Data in
Finder**). The first save of each session copies the previous file to `notes.backup.json` next to it, so a bad import
or **Clear map** can be undone by restoring that file while the app is closed. To keep the data somewhere else, such
as a synced folder:

```bash
defaults write local.ollama-notes DataDirectory "~/Dropbox/Ollama Notes"
```

**Coming from the browser version?** Your map is in that browser's storage, not on disk. Click **Export map**
there, then **File › Import Map…** in the app.

### Shortcuts

| | | | |
|---|---|---|---|
| ⌘N | New chat | ⌘O | Import files (text, PDF) as notes |
| ⇧⌘N | New note | ⇧⌘O | Import map (JSON) |
| ⌘L | Focus the chat box | ⇧⌘S | Export map (JSON) |
| ⌘F | Filter notes | ⌘, | Settings |
| ⌘0 | Fit map | ⌘= / ⌘− | Zoom map in / out |

## What it does

**Chat (left pane)**
- Streams replies from the selected model, rendered as Markdown (including tables); thinking-model reasoning is shown
  collapsed.
- After each reply, the model is asked (in a second, structured-JSON call) to extract 1–4 reusable notes:
  concepts, facts, decisions, questions and tasks. They appear in the map linked to a *topic* hub node for the
  conversation and to any related existing notes. Turn this off with the **Auto-notes** toggle.
- Each reply also has manual **Extract notes** and **Save as note** actions.
- Notes can be **pinned to chat context**: their content is injected into the system prompt.
- With **Web search** on, the model looks things up and reads pages before answering, and cites them (see
  [Web search](#web-search)).

**Map (right pane)**
- Drag to pan, scroll to pan, ⌘/Ctrl + scroll (or pinch) to zoom. Double-click empty space to add a note.
- Drag a note by its body. Drag the round handle on a note's right edge onto another note to link them.
- Click a note to edit it in the inspector (title, kind, body, links). Click a link line and press Delete to remove it.
- The map lays itself out with a live force simulation: notes repel each other, links act as springs, and gravity
  pulls everything toward the centre of mass. All three scale with a note's number of links, so well-connected
  notes pull harder, get more room, and settle near the middle while leaves orbit outside. Cards never overlap
  each other or the keyword labels on links: overlaps are resolved by moving cards directly, so nothing bounces
  or shakes, and two linked cards are kept far enough apart for their label to fit between them. Dragging a note
  reflows its neighbours; **Lock position** in the inspector pins a note in place. The **Physics** toggle turns the
  simulation off, **Forces ▾** tunes tension, gravity and spacing, **Tidy** re-runs the layout from scratch.
- **Keyword links.** Every note carries a few important keywords: the model supplies them when a note is
  extracted from a chat, and a local TF-IDF pass over the note's own text fills in the rest (words common to the
  whole map never count, and a phrase like "hash functions" absorbs its own words). Notes that share rare keywords
  get a dashed link labelled with them, created and updated automatically as notes are added or edited. The
  **Keywords ▾** menu toggles this, sets how strict it is, re-runs it, or removes all keyword links. Edit a note's
  keywords in the inspector (a curated list sticks; "auto" goes back to computed ones). Removing a keyword link
  from the inspector keeps that pair unlinked in future runs.
- **Fit** frames everything and the search box filters notes (keywords included).
- **Export**/**Import** round-trip the map and chat as JSON (the same format as before the Rust rewrite).

**Documents**
- **＋ File** in the map toolbar (or ⌘O, or drop files onto the map) imports text-like files (.txt, .md, .csv,
  .json, .html, …) and PDFs. PDFs are parsed locally with a vendored copy of pdf.js; scanned PDFs without a text
  layer come out empty.
- Each document becomes a *Document* hub note. Its text is split into sections of ~2,400 characters and each
  section is sent to the model to extract notes, which link to the hub and to related existing notes, and get
  keywords. You choose the section size and a cap on sections before it starts; a status line shows progress and
  can cancel.
- The document note keeps the full text. Pin it to chat context and the first 6,000 characters are given to the
  model with your questions.

**Settings (⌘,)**: Ollama server, chat model, a separate (smaller/faster) model for note extraction, system
prompt, temperature, max notes per exchange.

## How it's put together

The page in `ui/` is Rust compiled to WebAssembly (built with [Trunk](https://trunkrs.dev)), driving the same HTML
and CSS as before. The desktop app (`src-tauri/`, [Tauri 2](https://tauri.app)) shows it in a native window as
`tauri://localhost`. Ollama accepts requests from `tauri://` pages by default, so the page streams from Ollama
directly and **Stop** cancels generation on the server. The Rust side adds what a browser tab can't do: the menu bar,
open/save panels, confirm sheets, the data file, and starting Ollama. Right-click › **Inspect Element** opens the Web
Inspector. Everything that isn't drawing the page (the data model, keywords, the force layout, Markdown, prompts) is in
`crates/core`, which is tested natively.

```
Cargo.toml                    the workspace
build.sh                      builds build/Ollama Notes.app (or the browser-mode server)
crates/core/                  logic shared by everything, no I/O: tested with cargo test
  model.rs                      the saved state (notes.json, exports), read leniently, unknown fields kept
  graph.rs  layout.rs  view.rs  the notes map, its force layout, pan/zoom maths
  keywords.rs                   TF-IDF keywords and keyword links
  extract.rs  ollama.rs         note extraction prompts and merging; the Ollama API as data
  openai.rs                     cloud APIs (OpenAI-style chat completions) as data
  websearch.rs                  web search for the model: its tools, the research prompt, results cut to fit
  text.rs  markdown.rs          sectioning, file names, PDF text; safe Markdown (pulldown-cmark)
crates/native/                the data file (with its per-session backup) and starting Ollama
ui/                           the page: index.html, style.css, src/*.rs, vendored pdf.js (+ js/pdf_shim.js)
  app.rs chat.rs import.rs      state and saving, the conversation and its research, importing documents
  graph_view.rs inspector.rs    the map on screen, the inspector and popovers
  platform.rs ollama.rs         app vs browser differences; talking to Ollama or a cloud API
src-tauri/                    the desktop app: window, menu bar, commands for the page, quitting
server/                       browser mode: serves ui/dist, proxies /api/* to Ollama
tools/make-icon/              draws the app icon (scripts/icons.sh makes src-tauri/icons from it)
tools/golden/                 regenerates the tests that compare crates/core with the original JS
tools/e2e/                    drives the page in headless Chrome against the server (tools/e2e/smoke.json;
                              cloud.json against the stand-in API in mock_openai.py; research.json against the
                              stand-in Ollama in mock_ollama.py; research-live.json with a real model and search)
scripts/check.sh              the quality gate: fmt, clippy (native and wasm32), tests
```

For development, `cd ui && trunk serve` serves the page with live reload at http://127.0.0.1:1420 (run
`target/debug/ollama-notes-server` alongside it, or open it through `cargo tauri dev`, which starts Trunk itself).

## Browser mode (any OS)

The same UI also runs in a browser tab: double-click **`Ollama Notes.command`** (it builds the server the first time),
or run

```bash
./build.sh --server                              # once
target/release/ollama-notes-server               # opens http://localhost:8765
target/release/ollama-notes-server --no-browser  # don't open a tab
target/release/ollama-notes-server --port 9000   # custom port (default 8765; if busy, the next free port is used)
target/release/ollama-notes-server --no-ollama   # don't try to start Ollama
```

It starts Ollama if it isn't running, serves the page (built into the binary) at http://localhost:8765, and proxies
`/api/*` to Ollama at `http://127.0.0.1:11434` (set `OLLAMA_HOST=http://host:11434` to point somewhere else). In this
mode the map is saved in the browser's `localStorage` (the same place as before, so an existing map is still there)
and isn't shared with the app; Export/Import moves it between the two.
