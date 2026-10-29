// platform.js — what differs between the macOS app (a WKWebView with a native bridge, macos/Sources/Bridge.swift)
// and browser mode (server.py): where state is kept, how files are saved, and how Ollama is reached and started.

const bridge = window.webkit?.messageHandlers?.desktop;
const call = (cmd, args) => bridge.postMessage({ cmd, ...args });

export const isDesktop = !!bridge;
export const DEFAULT_OLLAMA_HOST = 'http://127.0.0.1:11434';

/** The saved state as JSON text, or null. App: notes.json in Application Support; browser: localStorage. */
export async function loadState(key) { return isDesktop ? call('load') : localStorage.getItem(key); }
export async function saveState(key, json) { if (isDesktop) await call('save', { json }); else localStorage.setItem(key, json); }

/** Save text to a file. The app asks where and resolves to the path (null if cancelled); a browser downloads it. */
export async function saveFile(name, text) {
  if (isDesktop) return call('saveFile', { name, text });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(new Blob([text], { type: 'application/json' })); a.download = name;
  a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  return null;
}

/** Ask the app to start Ollama if `host` is this Mac. Resolves to what it did ("opened Ollama.app"), or null. */
export async function startOllama(host) { return isDesktop ? call('startOllama', { host }) : null; }

/** "localhost" -> "http://localhost:11434", "box:8080" -> "http://box:8080"; a URL with a scheme is kept as it is. */
export function normalizeHost(s) {
  s = String(s || '').trim().replace(/\/+$/, '');
  if (!s) return DEFAULT_OLLAMA_HOST;
  return /^[a-z][a-z\d+.-]*:\/\//i.test(s) ? s : `http://${/:\d+$/.test(s) ? s : s + ':11434'}`;
}
