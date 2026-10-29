import { Ollama } from './ollama.js';
import { renderMarkdown } from './markdown.js';
import { NodeGraph } from './graph.js';
import { computeAutoKeywords, linkByKeywords } from './keywords.js';
import { parseFile, chunkText, titleFromFilename, ACCEPT as DOC_ACCEPT } from './documents.js';
import { isDesktop, loadState, saveState, saveFile, startOllama, normalizeHost, DEFAULT_OLLAMA_HOST } from './platform.js';

const $ = (s) => document.querySelector(s);
const STORE_KEY = 'ollama-notes.v1';
const uid = () => Math.random().toString(36).slice(2, 10) + Date.now().toString(36);
const escapeHtml = (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const clean = (s) => (typeof s === 'string' ? s.replace(/\s+/g, ' ').trim() : '');
const NOTE_KINDS = ['concept', 'fact', 'decision', 'question', 'task'];
const EXTRACT_MAX_TOKENS = 900; // plenty for ~8 short notes; stops runaway JSON generation
const EXTRACT_TIMEOUT_MS = 90000;
const DOC_CONTEXT_CHARS = 6000; // how much of a pinned document's text goes into the system prompt

const defaults = () => ({
  settings: {
    model: '', extractModel: '',
    system: 'You are a helpful, precise assistant. Answer directly and keep answers focused. Format with Markdown (headings, lists, fenced code); write math in plain text rather than LaTeX.',
    temperature: 0.7, maxNotes: 4, autoNotes: true, autoFit: true, chatWidth: 440,
    ollamaHost: DEFAULT_OLLAMA_HOST, // desktop app only; in browser mode server.py decides
  },
  chat: [], hubId: null, contextNodeIds: [], title: 'New conversation',
});

let state = defaults();
let models = [];
let abortCtl = null;
let extracting = 0;
let inspId = null;

const graph = new NodeGraph(
  { canvas: $('#canvas'), world: $('#world'), edges: $('#edges'), nodes: $('#nodes') },
  {
    onSelect: (id) => showInspector(id),
    onChange: () => { save(); updateMapCount(); },
    onEdit: (id) => { showInspector(id); $('#inspTitle').focus(); $('#inspTitle').select(); },
    onSettle: () => { if (graph._fitOnSettle) { graph._fitOnSettle = false; graph.fit({ animate: true }); } },
  },
);

// ---------- persistence (the app keeps a file in Application Support, a browser uses localStorage)
let loaded = false; // nothing is saved before the saved state has been read, so it can't be overwritten
function save() {
  if (!loaded) return;
  clearTimeout(save.t);
  save.t = setTimeout(flushSave, 250);
}
async function flushSave() {
  clearTimeout(save.t); save.t = 0;
  try { await saveState(STORE_KEY, JSON.stringify({ ...state, graph: graph.toJSON() })); }
  catch (e) { console.warn('save failed', e); toast('Saving failed: ' + (e.message || e), 6000); }
}
async function load() {
  try {
    const raw = await loadState(STORE_KEY); if (!raw) return;
    const data = JSON.parse(raw); const d = defaults();
    state = { ...d, ...data, settings: { ...d.settings, ...(data.settings || {}) } };
    delete state.graph;
    for (const m of state.chat) m.streaming = false;
    if (data.graph) graph.load(data.graph);
  } catch (e) { console.warn('load failed', e); toast('Could not load your saved notes: ' + (e.message || e), 8000); }
  finally { loaded = true; }
}

// ---------- toast / status
function toast(text, ms = 2800) {
  const t = $('#toast'); t.textContent = text; t.hidden = false;
  clearTimeout(toast.t); toast.t = setTimeout(() => { t.hidden = true; }, ms);
}
function hideToast() { clearTimeout(toast.t); $('#toast').hidden = true; }
let extractLabel = '';
function setStatus(ok, text) {
  const s = $('#status'); s.classList.toggle('ok', ok); s.classList.toggle('err', !ok);
  s.querySelector('.status-text').textContent = text;
  s.title = ok ? `Connected to Ollama${Ollama.host ? ' at ' + Ollama.host : ''}` : 'Start Ollama.app (or run `ollama serve`); Ollama Notes reconnects automatically.';
}
function updateExtractStatus() {
  const el = $('#extractStatus');
  el.innerHTML = extracting ? `${escapeHtml(extractLabel || 'Extracting notes into the map…')} <button class="link-btn" id="cancelExtract">cancel</button>` : '';
  el.classList.toggle('busy', extracting > 0);
}
const extractAborts = new Set();
function cancelExtractions() { for (const a of extractAborts) a.abort('cancelled'); }
function updateMapCount() {
  const n = graph.nodes.size, e = graph.edges.size;
  $('#mapCount').textContent = n ? `${n} note${n === 1 ? '' : 's'} · ${e} link${e === 1 ? '' : 's'}` : '';
}

// ---------- models
let mayStartOllama = isDesktop; // the app starts a local Ollama once: at launch, or when the server changes in Settings
let startedOllamaAt = 0;
const startingOllama = () => Date.now() - startedOllamaAt < 30000;
async function refreshModels() {
  try {
    models = await Ollama.tags();
    setStatus(true, models.length ? `Ollama · ${models.length} model${models.length === 1 ? '' : 's'}` : 'Ollama · no models pulled');
    if (!models.length) toast('No models found. Pull one first, e.g.  ollama pull llama3.2', 6000);
  } catch (e) {
    models = [];
    if (mayStartOllama && await startOllama(Ollama.host).catch(() => null)) startedOllamaAt = Date.now();
    setStatus(false, startingOllama() ? 'Starting Ollama…' : 'Ollama not reachable');
  }
  mayStartOllama = false;
  const names = models.map((m) => m.name);
  if (names.length && !names.includes(state.settings.model)) {
    state.settings.model = names.find((n) => !/embed/i.test(n)) || names[0];
    save();
  }
  fillModelSelects();
  clearTimeout(refreshModels.t);
  refreshModels.t = setTimeout(refreshModels, models.length ? 30000 : startingOllama() ? 1000 : 4000);
}
function fillModelSelects() {
  const opts = (selected, blankLabel) => {
    const list = models.map((m) => `<option value="${escapeHtml(m.name)}"${m.name === selected ? ' selected' : ''}>${escapeHtml(m.name)}${m.details?.parameter_size ? ` (${escapeHtml(m.details.parameter_size)})` : ''}</option>`);
    if (blankLabel !== undefined) list.unshift(`<option value=""${!selected ? ' selected' : ''}>${blankLabel}</option>`);
    if (!models.length && blankLabel === undefined) list.push('<option value="">no models available</option>');
    return list.join('');
  };
  $('#modelSelect').innerHTML = opts(state.settings.model);
  $('#setModel').innerHTML = opts(state.settings.model);
  $('#setExtractModel').innerHTML = opts(state.settings.extractModel, 'Same as chat model');
}

// ---------- chat rendering
function renderChat() {
  const box = $('#messages'); box.innerHTML = '';
  for (const m of state.chat) box.appendChild(messageEl(m));
  box.scrollTop = box.scrollHeight;
  $('#chatTitle').textContent = state.title;
}
function messageEl(m) {
  const el = document.createElement('article');
  el.className = `msg ${m.role}`; el.dataset.id = m.id;
  el.innerHTML = `<div class="msg-role">${m.role === 'user' ? 'You' : 'Model'}</div><div class="msg-body"></div><div class="msg-foot"></div>`;
  updateMessageEl(el, m);
  return el;
}
function updateMessageEl(el, m) {
  let html = '';
  if (m.thinking) html += `<details class="thinking"><summary>Thinking</summary><div>${renderMarkdown(m.thinking)}</div></details>`;
  if (m.error) html += `<div class="msg-error">${escapeHtml(m.error)}</div>`;
  html += m.role === 'assistant' ? renderMarkdown(m.content || '') : `<p>${escapeHtml(m.content).replace(/\n/g, '<br>')}</p>`;
  if (m.streaming) html += '<span class="cursor"></span>';
  el.querySelector('.msg-body').innerHTML = html;
  const foot = el.querySelector('.msg-foot'); foot.innerHTML = '';
  if (m.role === 'assistant' && !m.streaming && m.content) {
    const chips = (m.nodeIds || []).map((id) => graph.getNode(id)).filter(Boolean)
      .map((n) => `<button class="chip kind-${n.kind}" data-node="${n.id}" title="Show in map">${escapeHtml(n.title)}</button>`).join('');
    foot.innerHTML = `<div class="chips">${chips}</div><div class="msg-actions"><button class="link-btn" data-act="extract" title="Ask the model to pull notes from this reply">✦ Extract notes</button><button class="link-btn" data-act="note" title="Save this whole reply as one note">＋ Save as note</button></div>`;
  }
}
function scrollBottom(force = false) {
  const box = $('#messages');
  const near = box.scrollHeight - box.scrollTop - box.clientHeight < 120;
  if (force || near) box.scrollTop = box.scrollHeight;
}
function setBusy(busy) {
  $('#sendBtn').disabled = busy; $('#stopBtn').hidden = !busy; $('#input').disabled = false;
}

// ---------- chat logic
function buildMessages() {
  let sys = state.settings.system || '';
  const ctx = state.contextNodeIds.map((id) => graph.getNode(id)).filter(Boolean);
  if (ctx.length) {
    sys += '\n\nThe user has pinned these notes from their knowledge map as context for this conversation:\n' +
      ctx.map((n) => n.text
        ? `- ${n.title} (document${n.text.length > DOC_CONTEXT_CHARS ? ', first part' : ''}):\n${n.text.slice(0, DOC_CONTEXT_CHARS)}`
        : `- ${n.title}${n.body ? ': ' + n.body : ''}`).join('\n');
  }
  const history = state.chat.filter((m) => !m.streaming && m.content && !m.error).slice(-30).map((m) => ({ role: m.role, content: m.content }));
  return [{ role: 'system', content: sys }, ...history];
}

async function send() {
  const text = $('#input').value.trim();
  if (!text || abortCtl) return;
  if (!state.settings.model) { toast('No model selected. Is Ollama running with a model pulled?'); return; }
  $('#input').value = ''; autoresize();
  const user = { id: uid(), role: 'user', content: text, ts: Date.now() };
  const asst = { id: uid(), role: 'assistant', content: '', thinking: '', ts: Date.now(), streaming: true, nodeIds: [] };
  state.chat.push(user, asst);
  if (state.chat.length === 2 && state.title === 'New conversation') { state.title = text.slice(0, 60); $('#chatTitle').textContent = state.title; }
  const box = $('#messages');
  box.appendChild(messageEl(user));
  const asstEl = messageEl(asst); box.appendChild(asstEl);
  scrollBottom(true);
  abortCtl = new AbortController(); setBusy(true);
  let raf = 0;
  const paint = () => { raf = 0; updateMessageEl(asstEl, asst); scrollBottom(); };
  try {
    await Ollama.chatStream({
      model: state.settings.model, messages: buildMessages(),
      options: { temperature: +state.settings.temperature || 0 }, signal: abortCtl.signal,
      onChunk: ({ content, thinking }) => { asst.content = content; asst.thinking = thinking; if (!raf) raf = requestAnimationFrame(paint); },
    });
  } catch (e) {
    if (e.name === 'AbortError') { if (!asst.content) asst.error = 'Stopped.'; }
    else asst.error = e.message;
  } finally {
    if (raf) cancelAnimationFrame(raf);
    asst.streaming = false; abortCtl = null; setBusy(false);
    updateMessageEl(asstEl, asst); scrollBottom(); save();
  }
  if (asst.content && state.settings.autoNotes) {
    extractNotes(user, asst).catch((e) => toast(e.message.startsWith('Note extraction') ? e.message : 'Note extraction failed: ' + e.message, 6000));
  }
}

// ---------- automatic note extraction
const NOTE_SCHEMA = {
  type: 'object',
  properties: {
    topic: { type: 'string' },
    notes: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          title: { type: 'string', maxLength: 80 },
          summary: { type: 'string', maxLength: 400 },
          kind: { type: 'string', enum: NOTE_KINDS },
          related: { type: 'array', items: { type: 'string', maxLength: 80 }, maxItems: 3 },
          keywords: { type: 'array', items: { type: 'string', maxLength: 40 }, maxItems: 5 },
        },
        required: ['title', 'summary', 'kind'],
      },
      maxItems: 8,
    },
  },
  required: ['topic', 'notes'],
};

/** Extraction options for a model: turn thinking off where supported, so the whole budget goes to the JSON. */
async function extractionMode(model) {
  const caps = await Ollama.capabilities(model);
  return caps.includes('thinking') ? { think: false } : {};
}

async function extractNotes(user, asst) {
  const model = state.settings.extractModel || state.settings.model;
  if (!model) return;
  // Small models can loop forever in constrained-JSON mode, so cap the output and time out.
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort('timeout'), EXTRACT_TIMEOUT_MS);
  extractAborts.add(abort);
  extracting++; updateExtractStatus();
  try {
    const hub = state.hubId ? graph.getNode(state.hubId) : null;
    const existing = graph.allNodes().map((n) => n.title).filter(Boolean).slice(-80).join('; ');
    const max = Math.max(1, Math.min(8, +state.settings.maxNotes || 4));
    const result = await Ollama.chatJSON({
      model, schema: NOTE_SCHEMA, signal: abort.signal, ...(await extractionMode(model)),
      options: { temperature: 0.2, num_predict: EXTRACT_MAX_TOKENS, repeat_penalty: 1.15 },
      messages: [
        { role: 'system', content: 'You turn conversation exchanges into concise, reusable notes for a personal knowledge map. Respond with JSON only.' },
        { role: 'user', content:
`Conversation topic: ${hub ? hub.title : '(not set yet — propose one)'}
Existing note titles: ${existing || '(none)'}

USER:
${user.content.slice(0, 4000)}

ASSISTANT:
${asst.content.slice(0, 6000)}

Extract 1 to ${max} distinct notes from this exchange. Rules:
- "title": at most 6 words, specific and self-explanatory.
- "summary": 1–2 self-contained sentences someone could understand without the conversation.
- "kind": one of concept, fact, decision, question, task.
- "related": up to 3 exact existing note titles this note connects to (may be empty).
- "keywords": 2–5 important keywords or short phrases for this note (specific terms, not generic words).
- Skip anything already covered by an existing note unless it adds new information.
- "topic": a short label (at most 5 words) for the whole conversation.
Keep the JSON compact; no commentary before or after it.` },
      ],
    });
    applyExtraction(result, asst);
  } catch (e) {
    if (abort.signal.aborted) {
      const why = abort.signal.reason === 'timeout' ? 'timed out' : 'cancelled';
      throw new Error(`Note extraction ${why}. Try "Extract notes" again, or pick a smaller/other extraction model in Settings.`);
    }
    throw e;
  } finally { clearTimeout(timer); extractAborts.delete(abort); extracting--; updateExtractStatus(); }
}

/** Turn an extraction result into notes linked to `hub`. Returns { created: ids, nodes: ids }. */
function applyNotes(result, { hub, source }) {
  const notes = Array.isArray(result?.notes) ? result.notes : [];
  const created = [], touched = [];
  for (const raw of notes.slice(0, 8)) {
    const title = clean(raw?.title).slice(0, 80); if (!title) continue;
    const summary = clean(raw?.summary);
    const kwModel = (Array.isArray(raw?.keywords) ? raw.keywords : []).map(clean).filter((k) => k && k.length <= 40).slice(0, 5);
    let node = graph.findByTitle(title);
    if (node) {
      if (summary && node.body && !node.body.includes(summary)) graph.updateNode(node.id, { body: node.body + '\n\n' + summary });
      else if (summary && !node.body) graph.updateNode(node.id, { body: summary });
      if (kwModel.length) node.kwModel = [...new Set([...(node.kwModel || []), ...kwModel])].slice(0, 8);
    } else {
      const kind = NOTE_KINDS.includes(raw?.kind) ? raw.kind : 'concept';
      node = graph.addNode({ title, kind, body: summary, kwModel, source, ...graph.placeNear(hub.id) });
      created.push(node.id);
    }
    graph.addEdge(hub.id, node.id);
    for (const rel of (Array.isArray(raw?.related) ? raw.related : [])) {
      const other = graph.findByTitle(clean(rel));
      if (other && other.id !== node.id) graph.addEdge(other.id, node.id);
    }
    touched.push(node.id);
  }
  return { created, nodes: touched };
}

function applyExtraction(result, asst) {
  let hub = state.hubId ? graph.getNode(state.hubId) : null;
  const created = [];
  if (!hub) {
    const topic = clean(result?.topic) || state.title || 'Conversation';
    hub = graph.addNode({ title: topic, kind: 'topic', body: `Conversation started ${new Date().toLocaleString()}`, source: { msgId: asst.id }, ...graph.placeNear(null) });
    state.hubId = hub.id; state.title = topic; $('#chatTitle').textContent = topic;
    created.push(hub.id);
  }
  const r = applyNotes(result, { hub, source: { msgId: asst.id } });
  created.push(...r.created);
  for (const id of r.nodes) if (!asst.nodeIds.includes(id)) asst.nodeIds.push(id);
  const kw = refreshKeywords();
  graph.render();
  for (const id of created) graph.flash(id);
  const el = $(`.msg[data-id="${asst.id}"]`); if (el) updateMessageEl(el, asst);
  save(); updateMapCount();
  if (created.length) {
    toast(`${created.length} note${created.length > 1 ? 's' : ''} added to the map` + (kw.added ? `, ${kw.added} keyword link${kw.added > 1 ? 's' : ''}` : ''));
    if (state.settings.autoFit) { graph.fit({ animate: true }); graph._fitOnSettle = true; }
  } else toast('No new notes from this exchange');
}

// ---------- documents (text files, PDFs)
async function importFiles(fileList) {
  const files = [...fileList].filter(Boolean);
  if (!files.length) return;
  const docs = [];
  for (const f of files) {
    try {
      toast(`Reading ${f.name}…`, 120000);
      const d = await parseFile(f, ({ page, pages }) => toast(`Reading ${f.name}: page ${page} of ${pages}…`, 120000));
      if (!d.text) { toast(`${f.name}: no text found (a scanned PDF needs OCR first)`, 6000); continue; }
      docs.push(d);
    } catch (e) { toast(`${f.name}: ${e.message}`, 6000); }
  }
  hideToast();
  if (!docs.length) return;
  const opts = await askImportOptions(docs);
  if (!opts) return;
  for (const d of docs) await importDocument(d, opts);
}

function askImportOptions(docs) {
  const dlg = $('#docDialog');
  $('#docTitle').textContent = docs.length === 1 ? `Import "${docs[0].name}"` : `Import ${docs.length} documents`;
  $('#docList').innerHTML = docs.map((d) => `<li><span>${escapeHtml(d.name)}</span><span>${d.pages ? d.pages + ' pages · ' : ''}${d.text.length.toLocaleString()} chars</span></li>`).join('');
  const model = state.settings.extractModel || state.settings.model;
  const updateHint = () => {
    const size = Math.max(800, +$('#docChunkSize').value || 2400), max = Math.max(1, +$('#docMaxSections').value || 20);
    const sections = docs.map((d) => chunkText(d.text, { size }).length);
    const total = sections.reduce((a, b) => a + Math.min(b, max), 0);
    const skipped = sections.reduce((a, b) => a + Math.max(0, b - max), 0);
    $('#docHint').textContent = $('#docExtract').checked
      ? `${total} section${total === 1 ? '' : 's'} will be sent to ${model || 'the model'}, one call each (roughly ${Math.ceil(total * 8 / 60)} min at ~8 s per section).` + (skipped ? ` ${skipped} section${skipped === 1 ? '' : 's'} beyond the limit will be skipped.` : '')
      : 'Only a document note will be created; no notes are extracted.';
  };
  for (const id of ['#docExtract', '#docMaxSections', '#docChunkSize']) $(id).oninput = updateHint;
  updateHint();
  return new Promise((resolve) => {
    dlg.onclose = () => {
      if (dlg.returnValue !== 'ok') return resolve(null);
      resolve({ extract: $('#docExtract').checked, maxSections: Math.max(1, +$('#docMaxSections').value || 20), chunkSize: Math.max(800, +$('#docChunkSize').value || 2400), keepText: $('#docKeepText').checked });
    };
    dlg.showModal();
  });
}

async function importDocument(d, { extract, maxSections, chunkSize, keepText }) {
  const title = titleFromFilename(d.name);
  const meta = `${d.pages ? d.pages + ' pages · ' : ''}${d.text.length.toLocaleString()} characters · imported ${new Date().toLocaleDateString()}`;
  let hub = graph.findByTitle(title);
  if (!hub) {
    hub = graph.addNode({ title, kind: 'source', body: meta + '\n\n' + d.text.slice(0, 500) + (d.text.length > 500 ? '…' : ''), text: keepText ? d.text : undefined, source: { file: d.name }, ...graph.placeNear(null) });
    graph.flash(hub.id);
  } else graph.updateNode(hub.id, { text: keepText ? d.text : hub.text, body: hub.body || meta });
  save(); updateMapCount();
  if (!extract) { refreshKeywords(); graph.render(); toast(`Added document note "${title}"`); return; }

  const model = state.settings.extractModel || state.settings.model;
  if (!model) { toast('No model selected, so no notes were extracted. Pick a model and use "Extract notes" later.', 6000); return; }
  const sections = chunkText(d.text, { size: chunkSize }).slice(0, maxSections);
  const existingBefore = graph.nodes.size;
  const abort = new AbortController();
  extractAborts.add(abort);
  extracting++;
  let failed = 0, createdTotal = 0;
  try {
    for (let i = 0; i < sections.length; i++) {
      if (abort.signal.aborted) break;
      extractLabel = `Extracting notes from ${d.name}: section ${i + 1} of ${sections.length}…`; updateExtractStatus();
      const timer = setTimeout(() => abort.abort('timeout'), EXTRACT_TIMEOUT_MS);
      try {
        const existing = graph.allNodes().map((n) => n.title).filter(Boolean).slice(-80).join('; ');
        const max = Math.max(1, Math.min(8, +state.settings.maxNotes || 4));
        const result = await Ollama.chatJSON({
          model, schema: NOTE_SCHEMA, signal: abort.signal, ...(await extractionMode(model)),
          options: { temperature: 0.2, num_predict: EXTRACT_MAX_TOKENS, repeat_penalty: 1.15 },
          messages: [
            { role: 'system', content: 'You turn document sections into concise, reusable notes for a personal knowledge map. Respond with JSON only.' },
            { role: 'user', content:
`Document: ${title} (section ${i + 1} of ${sections.length})
Existing note titles: ${existing || '(none)'}

TEXT:
${sections[i]}

Extract 1 to ${max} distinct notes from this section. Rules:
- "title": at most 6 words, specific and self-explanatory.
- "summary": 1–2 self-contained sentences someone could understand without the document.
- "kind": one of concept, fact, decision, question, task.
- "related": up to 3 exact existing note titles this note connects to (may be empty).
- "keywords": 2–5 important keywords or short phrases for this note (specific terms, not generic words).
- Skip anything already covered by an existing note unless it adds new information.
- "topic": a short label (at most 5 words) for the document.
Keep the JSON compact; no commentary before or after it.` },
          ],
        });
        const r = applyNotes(result, { hub, source: { file: d.name, section: i + 1 } });
        createdTotal += r.created.length;
        graph.render(); for (const id of r.created) graph.flash(id);
        save(); updateMapCount();
      } catch (e) {
        if (abort.signal.aborted) break;
        failed++; console.warn(`section ${i + 1} of ${d.name}:`, e.message);
      } finally { clearTimeout(timer); }
    }
  } finally {
    extractAborts.delete(abort); extracting--; extractLabel = ''; updateExtractStatus();
  }
  const kw = refreshKeywords();
  graph.render(); save(); updateMapCount();
  const why = abort.signal.aborted ? (abort.signal.reason === 'timeout' ? ' (stopped: a section timed out)' : ' (cancelled)') : '';
  toast(`${d.name}: ${createdTotal} note${createdTotal === 1 ? '' : 's'} added from ${sections.length} section${sections.length === 1 ? '' : 's'}` + (failed ? `, ${failed} failed` : '') + (kw.added ? `, ${kw.added} keyword link${kw.added === 1 ? '' : 's'}` : '') + why, 7000);
  if (state.settings.autoFit && graph.nodes.size > existingBefore) { graph.fit({ animate: true }); graph._fitOnSettle = true; }
}

function noteFromMessage(m) {
  const firstLine = (m.content.split('\n').find((l) => l.trim()) || 'Note').replace(/^#+\s*/, '').replace(/[*`_>]/g, '').trim();
  const node = graph.addNode({ title: firstLine.slice(0, 70), kind: 'note', body: m.content.slice(0, 2000), source: { msgId: m.id }, ...graph.placeNear(state.hubId) });
  if (state.hubId) graph.addEdge(state.hubId, node.id);
  m.nodeIds = m.nodeIds || []; m.nodeIds.push(node.id);
  refreshKeywords();
  graph.render(); graph.flash(node.id); graph.select(node.id); graph.centerOn(node.id);
  const el = $(`.msg[data-id="${m.id}"]`); if (el) updateMessageEl(el, m);
  save();
}

// ---------- keywords & keyword links
const kwTextOf = (n) => (n.title || '') + '\u0001' + (n.body || '') + '\u0001' + (n.kwModel || []).join(',');
/** Recompute auto keywords for notes whose text changed (or all with force), then run keyword linking if enabled. */
function refreshKeywords({ force = false } = {}) {
  const nodes = graph.allNodes();
  const stale = nodes.filter((n) => !n.keywordsManual && (force || n.kwText !== kwTextOf(n) || !n.keywords));
  if (stale.length || force) {
    const computed = computeAutoKeywords(nodes);
    for (const n of stale) { n.keywords = computed.get(n.id) || []; n.kwText = kwTextOf(n); }
    if (force) for (const n of nodes) if (!n.keywordsManual) { n.keywords = computed.get(n.id) || n.keywords; n.kwText = kwTextOf(n); }
  }
  let result = { added: 0, removed: 0, updated: 0 };
  if (graph.keywordLinks.enabled) result = linkByKeywords(graph, { threshold: graph.keywordLinks.threshold });
  if (stale.length || result.added || result.removed || result.updated) { graph.render(); save(); updateMapCount(); }
  return result;
}
function scheduleKeywords() { clearTimeout(scheduleKeywords.t); scheduleKeywords.t = setTimeout(() => { refreshKeywords(); if (inspId) showInspector(inspId); }, 900); }
function syncKeywordUI() {
  const k = graph.keywordLinks;
  $('#kwEnabled').checked = !!k.enabled;
  $('#kwThreshold').value = k.threshold; $('#kwThreshold').nextElementSibling.textContent = Number(k.threshold).toFixed(1);
  $('#kwThreshold').disabled = !k.enabled;
}

// ---------- inspector
function showInspector(id) {
  inspId = id;
  const n = id && graph.getNode(id);
  const insp = $('#inspector');
  if (!n) { insp.hidden = true; graph.padRight = 0; return; }
  insp.hidden = false; graph.padRight = 340;
  $('#inspTitle').value = n.title; $('#inspKind').value = n.kind; $('#inspBody').value = n.body || '';
  $('#inspKindBadge').textContent = n.kind; $('#inspKindBadge').className = `badge kind-${n.kind}`;
  const pinned = state.contextNodeIds.includes(n.id);
  $('#inspPin').textContent = pinned ? 'Unpin from chat' : 'Pin to chat context';
  $('#inspPin').classList.toggle('active', pinned);
  $('#inspSource').hidden = !(n.source?.msgId && state.chat.some((m) => m.id === n.source.msgId));
  $('#inspLock').textContent = n.locked ? 'Unlock position' : 'Lock position';
  $('#inspLock').classList.toggle('active', !!n.locked);
  if (document.activeElement !== $('#inspKeywords')) $('#inspKeywords').value = (n.keywords || []).join(', ');
  $('#inspKeywords').placeholder = n.keywordsManual ? 'comma, separated, keywords' : 'auto: comma, separated, keywords';
  $('#inspKwAuto').hidden = !n.keywordsManual;
  const links = graph.neighbors(n.id);
  $('#inspLinks').innerHTML = links.length
    ? links.map(({ edge, node }) => `<span class="chip kind-${node.kind}${edge.auto === 'keyword' ? ' chip-kw' : ''}" data-node="${node.id}" title="${edge.auto === 'keyword' ? 'Keyword link: ' + escapeHtml(edge.label) : 'Link'}">${escapeHtml(node.title)}<button class="chip-x" data-edge="${edge.id}" title="Remove link">×</button></span>`).join('')
    : '<span class="muted">None yet. Drag the handle on a note\'s right edge onto another note to link them.</span>';
  $('#inspMeta').textContent = `Created ${new Date(n.createdAt).toLocaleString()}` + (n.text ? ` · document text: ${n.text.length.toLocaleString()} characters (pin to use as chat context)` : '') + (n.source?.file ? ` · from ${n.source.file}${n.source.section ? ', section ' + n.source.section : ''}` : '');
}
function renderContextBar() {
  const bar = $('#contextBar');
  const nodes = state.contextNodeIds.map((id) => graph.getNode(id)).filter(Boolean);
  state.contextNodeIds = nodes.map((n) => n.id);
  bar.hidden = !nodes.length;
  bar.innerHTML = '<span class="label">Context:</span>' + nodes.map((n) => `<span class="chip kind-${n.kind}" data-node="${n.id}">${escapeHtml(n.title)}<button class="chip-x" data-unpin="${n.id}" title="Unpin">×</button></span>`).join('');
}
function togglePin(id) {
  const i = state.contextNodeIds.indexOf(id);
  if (i >= 0) state.contextNodeIds.splice(i, 1); else state.contextNodeIds.push(id);
  renderContextBar(); if (inspId === id) showInspector(id); save();
}
function showSource(msgId) {
  const el = $(`.msg[data-id="${msgId}"]`); if (!el) return;
  el.scrollIntoView({ behavior: 'smooth', block: 'center' });
  el.classList.add('highlight'); setTimeout(() => el.classList.remove('highlight'), 1600);
}

// ---------- settings
function applySettingsToUI() {
  const s = state.settings;
  $('#autoNotes').checked = !!s.autoNotes;
  $('#setSystem').value = s.system; $('#setTemp').value = s.temperature; $('#setMaxNotes').value = s.maxNotes; $('#setAutoFit').checked = !!s.autoFit;
  $('#setHost').value = s.ollamaHost || DEFAULT_OLLAMA_HOST;
  document.documentElement.style.setProperty('--chat-w', `${s.chatWidth}px`);
}
function readSettingsFromDialog() {
  const s = state.settings;
  s.model = $('#setModel').value || s.model;
  s.extractModel = $('#setExtractModel').value;
  s.system = $('#setSystem').value;
  s.temperature = Math.min(2, Math.max(0, +$('#setTemp').value || 0));
  s.maxNotes = Math.min(8, Math.max(1, +$('#setMaxNotes').value || 4));
  s.autoFit = $('#setAutoFit').checked;
  const host = normalizeHost($('#setHost').value);
  if (isDesktop && host !== s.ollamaHost) { s.ollamaHost = host; Ollama.setHost(host); mayStartOllama = true; setStatus(false, 'connecting…'); refreshModels(); }
  fillModelSelects(); save();
}

// ---------- export / import
async function exportJSON() {
  const data = { app: 'ollama-notes', version: 1, exportedAt: new Date().toISOString(), ...state, graph: graph.toJSON() };
  try {
    const path = await saveFile(`ollama-notes-${new Date().toISOString().slice(0, 10)}.json`, JSON.stringify(data, null, 2));
    if (path) toast(`Exported to ${path}`);
  } catch (e) { toast('Export failed: ' + (e.message || e), 5000); }
}
function importJSON(file) {
  const rd = new FileReader();
  rd.onload = () => {
    try {
      const data = JSON.parse(rd.result);
      if (!data.graph) throw new Error('not an Ollama Notes export');
      if (graph.nodes.size && !confirm('Replace the current map and conversation with the imported file?')) return;
      const d = defaults();
      state = { ...d, ...data, settings: { ...state.settings, ...(data.settings || {}) } };
      delete state.graph; delete state.app; delete state.version; delete state.exportedAt;
      for (const m of state.chat) m.streaming = false;
      graph.load(data.graph);
      applySettingsToUI(); fillModelSelects(); renderChat(); renderContextBar(); updateMapCount(); showInspector(null); syncPhysicsUI(); syncKeywordUI(); refreshKeywords();
      requestAnimationFrame(() => graph.fit());
      save(); toast('Imported');
    } catch (e) { toast('Import failed: ' + e.message, 5000); }
  };
  rd.readAsText(file);
}

// ---------- wiring
function autoresize() {
  const ta = $('#input'); ta.style.height = 'auto'; ta.style.height = Math.min(220, ta.scrollHeight) + 'px';
}

$('#composer').addEventListener('submit', (ev) => { ev.preventDefault(); send(); });
$('#input').addEventListener('keydown', (ev) => { if (ev.key === 'Enter' && !ev.shiftKey && !ev.isComposing) { ev.preventDefault(); send(); } });
$('#input').addEventListener('input', autoresize);
$('#stopBtn').addEventListener('click', () => abortCtl?.abort());
$('#extractStatus').addEventListener('click', (ev) => { if (ev.target.id === 'cancelExtract') cancelExtractions(); });
$('#newChatBtn').addEventListener('click', () => {
  if (abortCtl) abortCtl.abort();
  cancelExtractions();
  state.chat = []; state.hubId = null; state.title = 'New conversation';
  renderChat(); save(); $('#input').focus();
});
$('#modelSelect').addEventListener('change', (ev) => { state.settings.model = ev.target.value; fillModelSelects(); save(); });
$('#autoNotes').addEventListener('change', (ev) => { state.settings.autoNotes = ev.target.checked; save(); });
$('#settingsBtn').addEventListener('click', () => { applySettingsToUI(); fillModelSelects(); $('#settingsDialog').showModal(); });
$('#settingsDialog').addEventListener('close', readSettingsFromDialog);

$('#messages').addEventListener('click', (ev) => {
  const chip = ev.target.closest('[data-node]');
  if (chip) { graph.select(chip.dataset.node); graph.centerOn(chip.dataset.node); return; }
  const act = ev.target.closest('[data-act]'); if (!act) return;
  const m = state.chat.find((x) => x.id === act.closest('.msg')?.dataset.id); if (!m) return;
  if (act.dataset.act === 'note') noteFromMessage(m);
  if (act.dataset.act === 'extract') {
    const i = state.chat.indexOf(m);
    const user = state.chat.slice(0, i).reverse().find((x) => x.role === 'user') || { content: '' };
    extractNotes(user, m).catch((e) => toast(e.message.startsWith('Note extraction') ? e.message : 'Note extraction failed: ' + e.message, 6000));
  }
});
$('#contextBar').addEventListener('click', (ev) => {
  const un = ev.target.closest('[data-unpin]'); if (un) { togglePin(un.dataset.unpin); return; }
  const chip = ev.target.closest('[data-node]'); if (chip) { graph.select(chip.dataset.node); graph.centerOn(chip.dataset.node); }
});

// inspector
$('#inspClose').addEventListener('click', () => graph.select(null));
$('#inspTitle').addEventListener('input', (ev) => { if (inspId) { graph.updateNode(inspId, { title: ev.target.value }); renderContextBar(); scheduleKeywords(); } });
$('#inspBody').addEventListener('input', (ev) => { if (inspId) { graph.updateNode(inspId, { body: ev.target.value }); scheduleKeywords(); } });
$('#inspKeywords').addEventListener('change', (ev) => {
  const n = inspId && graph.getNode(inspId); if (!n) return;
  const list = [...new Set(ev.target.value.split(/[,;\n]/).map((s) => s.trim()).filter(Boolean))].slice(0, 12);
  graph.updateNode(n.id, { keywords: list, keywordsManual: true });
  refreshKeywords(); showInspector(n.id);
});
$('#inspKwAuto').addEventListener('click', () => {
  const n = inspId && graph.getNode(inspId); if (!n) return;
  graph.updateNode(n.id, { keywordsManual: false, kwText: null });
  refreshKeywords(); showInspector(n.id);
});
$('#inspKind').addEventListener('change', (ev) => { if (inspId) { graph.updateNode(inspId, { kind: ev.target.value }); showInspector(inspId); } });
$('#inspPin').addEventListener('click', () => inspId && togglePin(inspId));
$('#inspAsk').addEventListener('click', () => {
  const n = inspId && graph.getNode(inspId); if (!n) return;
  $('#input').value = `Tell me more about "${n.title}".` + (n.body ? `\n\nWhat I have so far: ${n.body}` : '');
  autoresize(); $('#input').focus();
});
$('#inspSource').addEventListener('click', () => { const n = inspId && graph.getNode(inspId); if (n?.source?.msgId) showSource(n.source.msgId); });
$('#inspLock').addEventListener('click', () => { const n = inspId && graph.getNode(inspId); if (!n) return; graph.updateNode(n.id, { locked: !n.locked }); showInspector(n.id); });
$('#inspDelete').addEventListener('click', () => {
  const n = inspId && graph.getNode(inspId); if (!n) return;
  if (state.hubId === n.id) state.hubId = null;
  graph.removeNode(n.id); renderContextBar(); save();
});
$('#inspLinks').addEventListener('click', (ev) => {
  const x = ev.target.closest('[data-edge]');
  if (x) {
    const e = graph.edges.get(x.dataset.edge);
    if (e?.auto === 'keyword') graph.kwIgnore.add(e.from < e.to ? `${e.from}|${e.to}` : `${e.to}|${e.from}`);
    graph.removeEdge(x.dataset.edge); showInspector(inspId); return;
  }
  const chip = ev.target.closest('[data-node]'); if (chip) { graph.select(chip.dataset.node); graph.centerOn(chip.dataset.node); }
});

// map toolbar
$('#addNodeBtn').addEventListener('click', () => {
  const n = graph.addNode({ ...graph.placeNear(null) });
  graph.select(n.id); graph.flash(n.id); graph.centerOn(n.id);
  $('#inspTitle').focus(); $('#inspTitle').select();
});
$('#fitBtn').addEventListener('click', () => graph.fit({ animate: true }));

// physics toggle + force sliders
function syncPhysicsUI() {
  const p = graph.physics;
  $('#physicsToggle').checked = !!p.enabled;
  for (const [id, key] of [['#phTension', 'tension'], ['#phGravity', 'gravity'], ['#phRepulsion', 'repulsion']]) {
    $(id).value = p[key]; $(id).nextElementSibling.textContent = Number(p[key]).toFixed(1);
    $(id).disabled = !p.enabled;
  }
}
$('#physicsToggle').addEventListener('change', (ev) => { graph.setPhysics({ enabled: ev.target.checked }); syncPhysicsUI(); });
$('#physicsBtn').addEventListener('click', () => { const pop = $('#physicsPop'); const open = pop.hidden; closePopovers(); pop.hidden = !open; if (open) syncPhysicsUI(); });
function closePopovers() { for (const p of document.querySelectorAll('.popover')) p.hidden = true; }
document.addEventListener('pointerdown', (ev) => { if (!ev.target.closest('.popover-wrap')) closePopovers(); });

// keyword links
$('#kwBtn').addEventListener('click', () => { const pop = $('#kwPop'); const open = pop.hidden; closePopovers(); pop.hidden = !open; if (open) syncKeywordUI(); });
$('#kwEnabled').addEventListener('change', (ev) => { graph.setKeywordLinks({ enabled: ev.target.checked }); syncKeywordUI(); if (ev.target.checked) { const r = refreshKeywords({ force: true }); toast(`${r.added} keyword link${r.added === 1 ? '' : 's'} added`); } });
$('#kwThreshold').addEventListener('input', (ev) => { graph.setKeywordLinks({ threshold: +ev.target.value }); ev.target.nextElementSibling.textContent = (+ev.target.value).toFixed(1); scheduleKeywords(); });
$('#kwRun').addEventListener('click', () => { const r = refreshKeywords({ force: true }); toast(`Keyword links: ${r.added} added, ${r.removed} removed` + (graph.keywordLinks.enabled ? '' : ' (auto-link is off)')); });
$('#kwClear').addEventListener('click', () => {
  let n = 0; for (const e of [...graph.edges.values()]) if (e.auto === 'keyword') { graph.removeEdge(e.id); n++; }
  graph.setKeywordLinks({ enabled: false }); syncKeywordUI(); if (inspId) showInspector(inspId);
  toast(`${n} keyword link${n === 1 ? '' : 's'} removed; auto-link turned off`);
});
for (const [id, key] of [['#phTension', 'tension'], ['#phGravity', 'gravity'], ['#phRepulsion', 'repulsion']]) {
  $(id).addEventListener('input', (ev) => { graph.setPhysics({ [key]: +ev.target.value }); ev.target.nextElementSibling.textContent = (+ev.target.value).toFixed(1); });
}
$('#phReset').addEventListener('click', () => { graph.setPhysics({ tension: 1, gravity: 1, repulsion: 1 }); syncPhysicsUI(); });
$('#tidyBtn').addEventListener('click', () => graph.tidy());
$('#zoomInBtn').addEventListener('click', () => graph.zoomBy(1.25));
$('#zoomOutBtn').addEventListener('click', () => graph.zoomBy(0.8));
$('#search').addEventListener('input', (ev) => graph.setFilter(ev.target.value));
$('#docFile').accept = DOC_ACCEPT;
$('#importDocBtn').addEventListener('click', () => $('#docFile').click());
$('#docFile').addEventListener('change', (ev) => { importFiles(ev.target.files); ev.target.value = ''; });
(() => {
  const canvas = $('#canvas'); let depth = 0;
  const hasFiles = (ev) => [...(ev.dataTransfer?.types || [])].includes('Files');
  document.addEventListener('dragenter', (ev) => { if (!hasFiles(ev)) return; ev.preventDefault(); depth++; canvas.classList.add('dropping'); });
  document.addEventListener('dragover', (ev) => { if (hasFiles(ev)) { ev.preventDefault(); ev.dataTransfer.dropEffect = 'copy'; } });
  document.addEventListener('dragleave', (ev) => { if (!hasFiles(ev)) return; depth = Math.max(0, depth - 1); if (!depth) canvas.classList.remove('dropping'); });
  document.addEventListener('drop', (ev) => { if (!hasFiles(ev)) return; ev.preventDefault(); depth = 0; canvas.classList.remove('dropping'); importFiles(ev.dataTransfer.files); });
})();
$('#exportBtn').addEventListener('click', exportJSON);
$('#importBtn').addEventListener('click', () => $('#importFile').click());
$('#importFile').addEventListener('change', (ev) => { const f = ev.target.files[0]; if (f) importJSON(f); ev.target.value = ''; });
$('#clearMapBtn').addEventListener('click', () => {
  if (!graph.nodes.size) return;
  if (!confirm('Delete every note and link in the map? (Export first if you want a backup.)')) return;
  graph.clear(); state.hubId = null; state.contextNodeIds = [];
  for (const m of state.chat) m.nodeIds = [];
  renderChat(); renderContextBar(); save();
});

// the map toolbar wraps onto a second row in a narrow window; keep the inspector below it
new ResizeObserver(([e]) => $('#mapPane').style.setProperty('--toolbar-h', `${e.target.offsetHeight}px`)).observe($('.map-toolbar'));

// splitter
(() => {
  const sp = $('#splitter'); let dragging = false;
  sp.addEventListener('pointerdown', (ev) => { dragging = true; sp.classList.add('active'); sp.setPointerCapture(ev.pointerId); });
  sp.addEventListener('pointermove', (ev) => {
    if (!dragging) return;
    const w = Math.min(window.innerWidth - 360, Math.max(300, ev.clientX));
    state.settings.chatWidth = Math.round(w);
    document.documentElement.style.setProperty('--chat-w', `${state.settings.chatWidth}px`);
  });
  const stop = () => { if (dragging) { dragging = false; sp.classList.remove('active'); save(); } };
  sp.addEventListener('pointerup', stop); sp.addEventListener('pointercancel', stop);
})();

// ---------- desktop app: menu bar commands, and the last save before quitting (macos/Sources/AppDelegate.swift)
const press = (sel) => () => $(sel).click();
const commands = {
  'new-chat': press('#newChatBtn'), 'new-note': press('#addNodeBtn'), settings: press('#settingsBtn'),
  'import-files': press('#importDocBtn'), 'import-map': press('#importBtn'), 'export-map': press('#exportBtn'),
  fit: press('#fitBtn'), tidy: press('#tidyBtn'), 'zoom-in': press('#zoomInBtn'), 'zoom-out': press('#zoomOutBtn'),
  'focus-chat': () => $('#input').focus(), 'filter-notes': () => { $('#search').focus(); $('#search').select(); },
};
window.desktopCommand = (name) => commands[name]?.();
window.desktopFlush = () => (save.t ? flushSave() : null);

// ---------- init
await load();
if (isDesktop) Ollama.setHost(normalizeHost(state.settings.ollamaHost));
$('#setHostRow').hidden = !isDesktop; $('#setProxyNote').hidden = isDesktop;
applySettingsToUI();
renderChat();
renderContextBar();
updateMapCount();
graph.render();
syncPhysicsUI();
syncKeywordUI();
refreshKeywords();
refreshModels();
requestAnimationFrame(() => { if (graph.nodes.size && !(graph.view.x || graph.view.y)) graph.fit(); });
$('#input').focus();
