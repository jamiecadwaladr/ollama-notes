// Thin client for the Ollama HTTP API: same-origin /api in browser mode (server.py proxies it), or the Ollama server
// itself in the desktop app (Ollama accepts requests from app:// pages by default).

let base = '';

async function request(path, init) {
  try { return await fetch(base + path, init); } catch (e) {
    if (!base || init?.signal?.aborted) throw e;
    throw new Error(`Cannot reach Ollama at ${base}. Start Ollama.app or run \`ollama serve\`, or change the server in Settings.`);
  }
}

async function errText(r) {
  const t = await r.text();
  try { return JSON.parse(t).error || t; } catch { return t || `HTTP ${r.status}`; }
}

function parseLooseJSON(text) {
  try { return JSON.parse(text); } catch { /* fall through */ }
  const s = text.indexOf('{'), e = text.lastIndexOf('}');
  if (s >= 0 && e > s) { try { return JSON.parse(text.slice(s, e + 1)); } catch { /* fall through */ } }
  // Truncated or malformed (e.g. the model looped until the token cap): salvage what we can.
  const notes = [];
  for (const m of text.matchAll(/\{[^{}]*"title"[^{}]*\}/g)) { try { notes.push(JSON.parse(m[0])); } catch { /* skip */ } }
  if (!notes.length) {
    // No complete object: pull title/summary/kind triples out of the partial text instead.
    const str = '"((?:[^"\\\\]|\\\\.)*)"';
    const re = new RegExp(`"title"\\s*:\\s*${str}\\s*,\\s*"summary"\\s*:\\s*${str}(?:\\s*,\\s*"kind"\\s*:\\s*"(\\w+)")?`, 'g');
    for (const m of text.matchAll(re)) notes.push({ title: JSON.parse(`"${m[1]}"`), summary: JSON.parse(`"${m[2]}"`), kind: m[3] || 'concept', related: [] });
  }
  const topic = text.match(/"topic"\s*:\s*"((?:[^"\\]|\\.)*)"/);
  if (notes.length) return { topic: topic ? topic[1] : '', notes, salvaged: true };
  throw new Error('Model did not return valid JSON');
}

const capsCache = new Map();

export const Ollama = {
  /** Where Ollama is ('' = this page's server). Capabilities can differ between servers, so the cache is dropped. */
  get host() { return base; },
  setHost(url) { base = url; capsCache.clear(); },

  /** Capabilities of a model (e.g. ['completion','thinking','tools']), cached per model name. */
  async capabilities(model) {
    if (!capsCache.has(model)) {
      capsCache.set(model, (async () => {
        try {
          const r = await request('/api/show', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ model }) });
          if (!r.ok) return [];
          return (await r.json()).capabilities || [];
        } catch { return []; }
      })());
    }
    return capsCache.get(model);
  },

  async tags() {
    const r = await request('/api/tags', { signal: AbortSignal.timeout(5000) });
    if (!r.ok) throw new Error(await errText(r));
    return ((await r.json()).models || []).sort((a, b) => a.name.localeCompare(b.name));
  },

  // Streams /api/chat. onChunk({content, thinking, delta}) fires as tokens arrive.
  async chatStream({ model, messages, options, signal, onChunk }) {
    const r = await request('/api/chat', {
      method: 'POST', signal, headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model, messages, stream: true, options }),
    });
    if (!r.ok) throw new Error(await errText(r));
    const reader = r.body.getReader();
    const dec = new TextDecoder();
    let buf = '', content = '', thinking = '';
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (!line) continue;
        let j;
        try { j = JSON.parse(line); } catch { continue; }
        if (j.error) throw new Error(j.error);
        const m = j.message || {};
        if (m.content) content += m.content;
        if (m.thinking) thinking += m.thinking;
        onChunk?.({ content, thinking, delta: m.content || '' });
        if (j.done) return { content, thinking, stats: j };
      }
    }
    return { content, thinking };
  },

  // Non-streaming call that asks for JSON. `schema` is a JSON schema object (Ollama >= 0.5);
  // falls back to plain JSON mode if the server rejects the schema.
  async chatJSON({ model, messages, schema, signal, options, think }) {
    const body = { model, messages, stream: false, format: schema || 'json', options: { temperature: 0.2, ...(options || {}) } };
    if (think !== undefined) body.think = think; // thinking models must not spend the token budget on reasoning
    const post = () => request('/api/chat', { method: 'POST', signal, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    let r = await post();
    if (!r.ok && 'think' in body) { delete body.think; r = await post(); }
    if (!r.ok && schema) { body.format = 'json'; r = await post(); }
    if (!r.ok) throw new Error(await errText(r));
    const j = await r.json();
    if (j.error) throw new Error(j.error);
    return parseLooseJSON(j.message?.content || '');
  },
};
