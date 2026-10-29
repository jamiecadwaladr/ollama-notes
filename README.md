# Ollama Notes

A Mac app for chatting with a locally hosted [Ollama](https://ollama.com) model, with a node-based notes map that
fills itself in as you talk. Native window, menu bar and file panels; no dependencies to install.

## Build and run

```bash
./build.sh             # builds build/Ollama Notes.app
./build.sh --open      # ...and launches it
./build.sh --install   # ...and copies it to /Applications
```

All it needs is the Xcode Command Line Tools (`xcode-select --install`); the app runs on macOS 14 or later, on Apple
silicon and Intel. It is signed ad hoc, which is all this Mac needs. If you copy it to another Mac, right-click it and
choose **Open** the first time.

The app talks to Ollama at `http://127.0.0.1:11434`. If Ollama isn't running there when the app starts, it opens
Ollama.app (or runs `ollama serve`) and connects once it's up. For Ollama on another machine, change
**Settings › Ollama server**. You need at least one model pulled, e.g. `ollama pull llama3.2`.

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
- Streams replies from the selected model; thinking-model reasoning is shown collapsed.
- After each reply, the model is asked (in a second, structured-JSON call) to extract 1–4 reusable notes:
  concepts, facts, decisions, questions and tasks. They appear in the map linked to a *topic* hub node for the
  conversation and to any related existing notes. Turn this off with the **Auto-notes** toggle.
- Each reply also has manual **Extract notes** and **Save as note** actions.
- Notes can be **pinned to chat context**: their content is injected into the system prompt.

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
- **Export**/**Import** round-trip the map and chat as JSON.

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

The UI is the web app in `web/`, shown in a native window by a WKWebView, which loads it from inside the app bundle
as `app://notes/`. Ollama accepts requests from `app://` pages by default, so the page streams from Ollama directly
and **Stop** cancels generation on the server. The Swift side adds what a browser tab can't do: the menu bar,
open/save panels, confirm sheets, the data file, and starting Ollama. Right-click › **Inspect Element** opens the
Web Inspector. After changing anything in `web/`, run `./build.sh` again: the folder is copied into the app.

```
build.sh                        builds build/Ollama Notes.app
macos/Sources/main.swift        entry point
macos/Sources/AppDelegate.swift window, menu bar, web view (links, file panels, confirm sheets), quitting
macos/Sources/Bridge.swift      what the page asks of the app: data file, export, starting Ollama
macos/Sources/WebAssets.swift   serves web/ to the web view as app://notes
macos/Info.plist                app bundle metadata
macos/make-icon.swift           draws the app icon
web/index.html    layout
web/style.css     styling (dark theme)
web/app.js        chat, note extraction, inspector, persistence, menu commands
web/platform.js   what differs between the app and browser mode (storage, saving files, reaching Ollama)
web/graph.js      pan/zoom node canvas, linking, placement, force layout
web/keywords.js   keyword extraction (TF-IDF) and keyword-based auto-linking
web/documents.js  text/PDF parsing (pdf.js in web/vendor) and sectioning
web/ollama.js     Ollama API client (streaming chat, JSON-schema chat)
web/markdown.js   small safe Markdown renderer
server.py, Ollama Notes.command   browser mode, below
```

## Browser mode (any OS)

The same UI also runs in a browser tab, with nothing but Python 3: double-click **`Ollama Notes.command`**, or run

```bash
./server.py                # or: python3 server.py
./server.py --no-browser   # don't open a tab
./server.py --port 9000    # custom port (default 8765; if busy, the next free port is used)
./server.py --no-ollama    # don't try to start Ollama
```

It starts Ollama if it isn't running, serves `web/` at http://localhost:8765, and proxies `/api/*` to Ollama at
`http://127.0.0.1:11434` (set `OLLAMA_HOST=http://host:11434` to point somewhere else). In this mode the map is
saved in the browser's `localStorage` and isn't shared with the app; Export/Import moves it between the two.
