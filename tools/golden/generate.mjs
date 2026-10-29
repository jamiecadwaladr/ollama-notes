// Writes crates/core/tests/fixtures/golden.json: what the original JS (web/ on the master branch) computes for a
// fixed set of inputs. The Rust port is tested against it. Run through tools/golden/generate.sh, which exports the
// JS from git and runs this file with JavaScriptCore's shell (`jsc -m`), so no Node is needed.

globalThis.window = {}; // platform.js reads window.webkit when it loads
const dir = globalThis.arguments[0];
const kw = await import(`${dir}/keywords.js`);
const docs = await import(`${dir}/documents.js`);
const platform = await import(`${dir}/platform.js`);
const { NodeGraph } = await import(`${dir}/graph.js`);

// parseLooseJSON is private to ollama.js: evaluate its source on its own.
const ollamaSrc = read(`${dir}/ollama.js`);
const start = ollamaSrc.indexOf('function parseLooseJSON');
const end = ollamaSrc.indexOf('\nconst capsCache');
const parseLooseJSON = (0, eval)(`(${ollamaSrc.slice(start, end)})`);

// A small seeded PRNG (mulberry32), so the generated maps are the same on every run.
function rng(seed) {
  return () => {
    seed |= 0; seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
Math.random = () => { throw new Error('the fixtures must not depend on Math.random'); };

const out = {};

// ---------- keywords
const words = ['hashes', 'hashing', 'Hash', 'queries', 'boxes', 'churches', 'wishes', 'class', 'bus', 'gas', 'indexed',
  'indexing', 'sing', 'bring', 'red', 'seeded', 'Größe', 'naïve', 'x-ray', '’quoted’', "don't", '-dash-', '42abc',
  'data', 'systems', 'e.g', 'trees', 'B-trees', 'Ünïcödé', 'ÉTUDES', 'café', 'tries', 'fizzes', 'flies'];
out.tokenize = words.concat(['Bloom filters — fast; “quoted” (aside) x - y', 'naïve Größe-Schätzung 3D-scan’s']).map((w) => [w, kw.tokenize(w)]);
out.stem = words.map((w) => w.toLowerCase()).map((w) => [w, kw.stem(w)]);
out.keyOf = ['Hash Sets', 'hash set', 'The Bloom Filter', 'data', 'Binary search trees', 'x', 'Größe', 'B-trees indexing']
  .map((w) => [w, kw.keyOf(w)]);

const corpus = [
  ['Bloom filters', 'A Bloom filter is a probabilistic data structure for set membership. Bloom filters can report false positives but never false negatives.', ['bloom filter', 'false positives']],
  ['Hash functions', 'Hash functions map keys to buckets. Good hash functions spread keys evenly; hashing is used by hash tables and Bloom filters.', ['hash function']],
  ['Hash tables', 'A hash table stores key-value pairs in buckets chosen by a hash function. Collisions are resolved by chaining or open addressing.', null],
  ['Open addressing', 'Open addressing resolves collisions by probing other buckets: linear probing, quadratic probing or double hashing.', null],
  ['Binary search trees', 'Binary search trees keep keys ordered. Lookups take logarithmic time when the tree is balanced.', ['binary search tree', 'balanced tree']],
  ['B-trees', 'B-trees are balanced trees with many keys per node, used by databases and file systems for indexing on disk.', null],
  ['Database indexing', 'An index speeds up queries. Databases use B-trees or hash indexes; indexing trades write speed for read speed.', ['index']],
  ['Tea brewing', 'Green tea brews at 80 °C for two minutes; black tea at 95 °C for four minutes.', null],
  ['Café culture', 'Café culture in Vienna: coffee houses, newspapers, and a glass of water with every coffee.', null],
  ['Größe und Schätzung', 'Die Größe einer Stichprobe bestimmt die Genauigkeit der Schätzung.', null],
  ['Garbage collection', 'Tracing garbage collection finds reachable objects from roots. Generational collectors assume most objects die young.', ['garbage collection']],
  ['Reference counting', 'Reference counting frees objects when their count drops to zero, but cycles need a separate collector.', null],
  ['Rust ownership', 'Rust ownership rules let the compiler free memory without garbage collection; borrowing checks references at compile time.', ['ownership', 'borrow checker']],
  ['Untitled', '', null],
  ['Question: why tries?', 'Tries store strings by prefix; lookups take time proportional to the key length.', null],
  ['Task — benchmark hash tables', 'Benchmark open addressing against chaining for hash tables with 1M keys.', ['benchmark']],
];
const corpusNodes = corpus.map(([title, body, kwModel], i) => ({ id: `n${String(i).padStart(2, '0')}`, title, body, ...(kwModel ? { kwModel } : {}) }));
corpusNodes[13].keywordsManual = true;
corpusNodes[13].keywords = ['kept'];
out.terms = corpusNodes.slice(0, 4).map((n) => ({ node: n, terms: [...kw.termsOf(n)].map(([k, t]) => [k, t]) }));
out.autoKeywords = { nodes: corpusNodes, per: 6, out: [...kw.computeAutoKeywords(corpusNodes)] };
out.autoKeywordsSmall = { nodes: corpusNodes.slice(0, 3), per: 6, out: [...kw.computeAutoKeywords(corpusNodes.slice(0, 3))] };

// linkByKeywords against a stand-in for NodeGraph's data methods
function fakeGraph(nodes, edges, kwIgnore) {
  let next = 0;
  const g = {
    nodes: new Map(nodes.map((n) => [n.id, n])),
    edges: new Map(edges.map((e) => [e.id, e])),
    kwIgnore: new Set(kwIgnore),
    allNodes() { return [...this.nodes.values()]; },
    hasEdge(a, b) { for (const e of this.edges.values()) if ((e.from === a && e.to === b) || (e.from === b && e.to === a)) return e; return null; },
    addEdge(from, to, label = '', extra = {}) {
      if (from === to || !this.nodes.has(from) || !this.nodes.has(to)) return null;
      const existing = this.hasEdge(from, to); if (existing) return existing;
      const e = { id: `k${next++}`, from, to, label, ...extra }; this.edges.set(e.id, e); return e;
    },
    removeEdge(id) { this.edges.delete(id); },
  };
  return g;
}
const computed = kw.computeAutoKeywords(corpusNodes);
const withKeywords = corpusNodes.map((n) => ({ ...n, keywords: n.keywordsManual ? n.keywords : computed.get(n.id) }));
out.linkByKeywords = [];
for (const [threshold, edges, ignore] of [
  [1.0, [], []],
  [0.5, [], []],
  [2.5, [], []],
  [1.0, [{ id: 'm1', from: 'n01', to: 'n02', label: '' }, { id: 'old', from: 'n07', to: 'n08', label: 'stale', auto: 'keyword' }], ['n00|n01']],
]) {
  const g = fakeGraph(withKeywords.map((n) => ({ ...n })), edges.map((e) => ({ ...e })), ignore);
  const result = kw.linkByKeywords(g, { threshold });
  out.linkByKeywords.push({ nodes: withKeywords, edges, kwIgnore: ignore, threshold, result, edgesAfter: [...g.edges.values()].map((e) => [e.from, e.to, e.label, e.auto || null]) });
}

// ---------- documents and hosts
const R = rng(7);
const sentence = () => Array.from({ length: 4 + Math.floor(R() * 14) }, () => ['alpha', 'beta', 'gamma', 'délta', 'ε', 'zeta', 'eta', 'theta'][Math.floor(R() * 8)]).join(' ') + '.';
const paragraph = (n) => Array.from({ length: n }, sentence).join(' ');
const longWords = Array.from({ length: 900 }, () => 'word').join(' ');
const texts = [
  '', 'One short paragraph.', 'A.\n\nB.\n\n\n  \n\nC.',
  Array.from({ length: 12 }, () => paragraph(1 + Math.floor(R() * 6))).join('\n\n'),
  paragraph(80), longWords, 'x'.repeat(5000), 'Line one\r\nline two\n \t\nThird ' + 'é'.repeat(3000),
];
out.chunkText = [];
for (const text of texts) for (const size of [100, 800, 2400]) out.chunkText.push({ text, size, out: docs.chunkText(text, { size }) });
out.titleFromFilename = ['my_report-v2.pdf', 'notes.md', '', '.hidden', 'a__b--c.tar.gz', '  spaced   name .txt', 'x'.repeat(100) + '.pdf', 'Größe_Übersicht.pdf']
  .map((n) => [n, docs.titleFromFilename(n)]);
out.normalizeHost = ['', '   ', 'localhost', 'localhost:8080', 'http://box:11434/', 'https://x.example///', 'box', '10.0.0.5', 'ftp://h', 'HTTP://Upper', '[::1]:11434', 'gpu box ']
  .map((h) => [h, platform.normalizeHost(h)]);

// ---------- loose JSON
out.parseLooseJSON = [
  '{"topic":"T","notes":[]}',
  'Here you go:\n```json\n{"topic":"Hashing","notes":[{"title":"A","summary":"B","kind":"fact"}]}\n```',
  '{"topic": "Hashing", "notes": [{"title": "Hash \\"sets\\"", "summary": "Fast lookups.", "kind": "fact"}, {"title": "Bloom',
  '{"topic":"Cut","notes":[{"title":"One","summary":"S1","kind":"task","related":[',
  '{"notes":[{"title":"Esc\\u00e9","summary":"a\\nb"}, {"title":"Two","summary":"S2","kind":"concept"',
  '{"notes": [{"title": "A", "summary": "B',
  'no json at all',
  '{"topic":"x" "notes":[{"title":"T","summary":"S"}]}',
].map((t) => { try { return [t, parseLooseJSON(t)]; } catch (e) { return [t, { error: e.message }]; } });

// ---------- force layout (graph.js _tick on a stand-in object: no DOM involved)
function simGraph(spec) {
  const g = Object.create(NodeGraph.prototype);
  g.nodes = new Map(spec.nodes.map((n) => [n.id, { ...n }]));
  g.edges = new Map(spec.edges.map((e, i) => [`e${i}`, { id: `e${i}`, ...e }]));
  g.physics = { enabled: true, tension: 1, gravity: 1, repulsion: 1, ...spec.physics };
  g._els = new Map(spec.nodes.filter((n) => n.h).map((n) => [n.id, { offsetHeight: n.h }]));
  g._vel = new Map();
  g._sim = { alpha: spec.alpha, target: spec.target, raf: 0, corrected: 0, tail: 0 };
  g._labelW = new Map(Object.entries(spec.labelW || {}));
  for (const n of g.nodes.values()) { delete n.h; if (n.drag) { n._drag = true; delete n.drag; } }
  return g;
}
function randomMap(seed, n, links, opts = {}) {
  const r = rng(seed);
  const nodes = Array.from({ length: n }, (_, i) => ({
    id: `p${i}`, x: Math.round(r() * 900 - 450), y: Math.round(r() * 700 - 350),
    ...(r() < 0.6 ? { h: 80 + Math.round(r() * 90) } : {}),
    ...(opts.locked && i % 7 === 3 ? { locked: true } : {}),
    ...(opts.drag && i === 1 ? { drag: true } : {}),
  }));
  const edges = [];
  for (let i = 0; i < links; i++) {
    const a = Math.floor(r() * n), b = Math.floor(r() * n);
    if (a !== b) edges.push({ from: `p${a}`, to: `p${b}`, label: r() < 0.3 ? ['hash, bloom', 'trees', 'garbage collection', 'x'][Math.floor(r() * 4)] : '' });
  }
  return { nodes, edges };
}
const specs = [
  { name: 'pair', nodes: [{ id: 'a', x: 0, y: 0 }, { id: 'b', x: 400, y: 30 }], edges: [{ from: 'a', to: 'b', label: '' }], alpha: 1, target: 0 },
  { name: 'stacked', nodes: [{ id: 'a', x: 0, y: 0 }, { id: 'b', x: 10, y: 4, h: 140 }, { id: 'c', x: 25, y: 13 }, { id: 'd', x: 3, y: 50, locked: true }],
    edges: [{ from: 'a', to: 'b', label: 'shared words' }, { from: 'b', to: 'c', label: '' }], labelW: { 'shared words': 74.5 }, alpha: 1, target: 0 },
  { name: 'random12', ...randomMap(11, 12, 14), alpha: 1, target: 0 },
  { name: 'random30-locked-drag', ...randomMap(23, 30, 40, { locked: true, drag: true }), labelW: { trees: 30.25 }, alpha: 0.7, target: 0.18, physics: { tension: 1.7, gravity: 0.4, repulsion: 2.2 } },
  { name: 'random20-weak', ...randomMap(5, 20, 8), alpha: 0.25, target: 0, physics: { tension: 0.2, gravity: 3, repulsion: 0.2 } },
];
out.physics = specs.map((spec) => {
  const g = simGraph(spec);
  const snapshots = [];
  for (let t = 1; t <= 200; t++) {
    g._tick();
    if ([1, 2, 5, 20, 60, 200].includes(t)) snapshots.push({ ticks: t, alpha: g._sim.alpha, corrected: g._sim.corrected, nodes: [...g.nodes.values()].map((n) => [n.x, n.y]) });
  }
  // the physics-off Tidy: a synchronous settle from alpha 1, then whole pixels
  const t = simGraph({ ...spec, alpha: 1, target: 0 });
  for (const n of t.nodes.values()) n._drag = false;
  for (let i = 0; i < 320 && t._sim.alpha > 0.004; i++) t._tick();
  const settled = [...t.nodes.values()].map((n) => [Math.round(n.x), Math.round(n.y)]);
  return { ...spec, snapshots, settled };
});

// ---------- placement of new notes
function placeGraph(nodes, view, canvas) {
  const g = Object.create(NodeGraph.prototype);
  g.nodes = new Map(nodes.map((n) => [n.id, { ...n }]));
  g._els = new Map(nodes.filter((n) => n.h).map((n) => [n.id, { offsetHeight: n.h }]));
  g.view = view; g.padRight = canvas.padRight;
  g.el = { canvas: { getBoundingClientRect: () => ({ width: canvas.width, height: canvas.height }) } };
  return g;
}
out.placeNear = [];
const crowd = Array.from({ length: 40 }, (_, i) => ({ id: `c${i}`, x: (i % 8) * 250 - 1000, y: Math.floor(i / 8) * 140 - 350, ...(i % 3 ? {} : { h: 150 }) }));
for (const [nodes, anchor, view, canvas] of [
  [[], null, { x: 0, y: 0, k: 1 }, { width: 840, height: 700, padRight: 0 }],
  [[{ id: 'a', x: 300, y: 300 }], null, { x: 0, y: 0, k: 1 }, { width: 840, height: 700, padRight: 0 }],
  [[{ id: 'a', x: 300, y: 300 }], 'a', { x: 0, y: 0, k: 1 }, { width: 840, height: 700, padRight: 0 }],
  [crowd, 'c12', { x: 120, y: -40, k: 0.6 }, { width: 840, height: 700, padRight: 340 }],
  [crowd, null, { x: 120, y: -40, k: 0.6 }, { width: 840, height: 700, padRight: 340 }],
]) {
  const g = placeGraph(nodes, view, canvas);
  out.placeNear.push({ nodes, anchor, view, canvas, viewCenter: g.viewCenter(), out: g.placeNear(anchor) });
}

print(JSON.stringify(out));
