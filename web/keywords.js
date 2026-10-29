// keywords.js — local keyword extraction and keyword-based auto-linking between notes.
//
// Every note carries `keywords` (display strings). They come from the model when a note is
// extracted from a conversation, from the user when edited in the inspector, and otherwise from
// this file's TF-IDF pass over the note's own title and body (so a word that appears in every note
// of the map is never "important"). Two notes that share rare keywords get a dashed link labelled
// with those keywords.

const STOP = new Set(`a an the and or but if then else of to in on at by for with from as is are was were be been
being it its this that these those there here which who whom whose what when where why how not no nor so than too
very can could would should will shall may might must do does did done have has had having i you he she we they them
their our your my me him her us into over under about between across after before during through while also more
most less least much many some any each other another such only own same both either neither all few several via
per etc one two three first second new way ways use used uses using make makes made get gets got like just even
still yet already always often usually typically generally need needs needed allow allows allowing enable enables
enabling based basic basics example examples case cases thing things something note notes conversation started
introduction summary overview instead rather whether without within because since until unless among again further
however therefore thus hence means mean meaning provide provides providing include includes including e.g i.e
important different various certain specific general common particular possible available following given known
called known set sets data information system systems process processes method methods result results value
values number numbers large small high low good bad better best well check checks checking checked every single
unlike rely relies relying reduce reduces reducing make making allow allowing indicate indicates indicating
represent represents representing efficiently efficient quick quickly organize organizes organizing store stores
storing return returns returning ensure ensures ensuring help helps helping require requires requiring take takes
taking give gives giving keep keeps keeping put puts show shows showing work works working determine determines
determining typically often especially simply really actually come comes coming compare compares comparing
choose chooses choosing trade trades trading against toward towards along around itself themselves let lets
become becomes becoming remain remains remaining consider considers considering`.split(/\s+/));

export function tokenize(text) {
  return (String(text || '').toLowerCase().match(/[\p{L}][\p{L}\p{N}'’-]*/gu) || [])
    .map((t) => t.replace(/^['’-]+|['’-]+$/g, ''))
    .filter(Boolean);
}
/** Split text into phrase segments so bigrams never straddle punctuation. */
function segments(text) {
  return String(text || '').split(/[.,;:!?()[\]{}"“”—–\n]+|\s[-–]\s/).filter((s) => s.trim());
}

// Light English stemmer: plural and -ing/-ed stripping, enough to match "hashes"/"hashing"/"hash".
export function stem(w) {
  if (w.length > 4 && w.endsWith('ies')) w = w.slice(0, -3) + 'y';
  else if (w.length > 4 && /(s|x|z|ch|sh)es$/.test(w)) w = w.slice(0, -2);
  else if (w.length > 3 && w.endsWith('s') && !w.endsWith('ss')) w = w.slice(0, -1);
  if (w.length > 6 && w.endsWith('ing')) w = w.slice(0, -3);
  else if (w.length > 5 && w.endsWith('ed')) w = w.slice(0, -2);
  return w;
}

const usable = (t) => t.length >= 3 && !/^\d/.test(t) && !STOP.has(t) && !STOP.has(stem(t));

/** Canonical key for a keyword/phrase, so "Hash Sets" and "hash set" match. */
export function keyOf(display) {
  return tokenize(display).filter(usable).map(stem).join(' ');
}

/** Candidate terms (unigrams + adjacent bigrams) of a note with raw counts. key -> {count, display, inTitle, bigram} */
export function termsOf(node) {
  const terms = new Map();
  const add = (key, display, inTitle, bigram) => {
    const t = terms.get(key) || { count: 0, display, inTitle: false, bigram };
    t.count += 1; if (inTitle) t.inTitle = true;
    terms.set(key, t);
  };
  const scan = (text, inTitle) => {
    for (const seg of segments(text)) {
      const toks = tokenize(seg);
      const ok = toks.map(usable);
      for (let i = 0; i < toks.length; i++) {
        if (!ok[i]) continue;
        add(stem(toks[i]), toks[i], inTitle, false);
        if (ok[i + 1]) add(stem(toks[i]) + ' ' + stem(toks[i + 1]), toks[i] + ' ' + toks[i + 1], inTitle, true);
      }
    }
  };
  scan(node.title, true);
  scan(node.body, false);
  return terms;
}

/**
 * Compute keywords for every node with TF-IDF over the whole map.
 * Returns Map<nodeId, string[]> of display keywords (max `per` each). Model-supplied keywords
 * (`node.kwModel`) come first; local ones top them up. Nodes with `keywordsManual` are skipped.
 */
export function computeAutoKeywords(nodes, { per = 6 } = {}) {
  const docs = nodes.map((n) => termsOf(n));
  const N = nodes.length;
  const df = new Map();
  for (const d of docs) for (const k of d.keys()) df.set(k, (df.get(k) || 0) + 1);
  const idf = (k) => Math.log((Math.max(N, 6) + 1) / ((df.get(k) || 0) + 0.5));
  const out = new Map();
  nodes.forEach((n, i) => {
    if (n.keywordsManual) return;
    // a phrase only counts if it recurs or sits in the title; a one-off word pair is usually noise
    const scored = [...docs[i]].filter(([, t]) => !t.bigram || t.count >= 2 || t.inTitle)
      // a word used once in the body is weak evidence, however rare it is across the map
      .map(([k, t]) => ({ k, display: t.display, score: (t.count + (t.inTitle ? 2 : 0)) * (t.bigram ? 1.3 : 1) * (t.count === 1 && !t.inTitle ? 0.5 : 1) * idf(k) }))
      .filter((x) => x.score > 0.35)
      .sort((a, b) => b.score - a.score);
    // pick the best terms; a phrase supersedes its own words ("hash functions" absorbs "hash" and "functions")
    let chosen = []; // [{k, display}]
    const has = (k) => chosen.some((c) => c.k === k);
    const partOfChosenPhrase = (k) => chosen.some((c) => c.k.includes(' ') && c.k.split(' ').includes(k));
    const take = (k, display) => {
      if (!k || has(k)) return;
      if (k.includes(' ')) { const parts = k.split(' '); chosen = chosen.filter((c) => !parts.includes(c.k)); }
      else if (partOfChosenPhrase(k)) return;
      chosen.push({ k, display });
    };
    for (const kw of (n.kwModel || [])) take(keyOf(kw), kw);
    for (const x of scored) { if (chosen.length >= per) break; take(x.k, x.display); }
    out.set(n.id, chosen.map((c) => c.display));
  });
  return out;
}

/** Among shared keys, a phrase supersedes its own words so "bloom filter" isn't counted three times. */
function dropSharedParts(keys) {
  const parts = new Set();
  for (const k of keys) if (k.includes(' ')) for (const p of k.split(' ')) parts.add(p);
  return keys.filter((k) => k.includes(' ') || !parts.has(k));
}

const pairKey = (a, b) => (a < b ? `${a}|${b}` : `${b}|${a}`);

/**
 * Create/update/remove keyword links on the graph. A pair of notes is linked when the summed
 * rarity (idf) of their shared keywords reaches `threshold`. Existing manual links are left alone.
 * Returns { added, removed, updated }.
 */
export function linkByKeywords(graph, { threshold = 1.0 } = {}) {
  const nodes = graph.allNodes();
  const N = nodes.length;
  const keysOf = new Map(); // id -> Map(key -> display)
  for (const n of nodes) {
    const m = new Map();
    for (const kw of (n.keywords || [])) { const k = keyOf(kw); if (k && !m.has(k)) m.set(k, kw); }
    keysOf.set(n.id, m);
  }
  const byKey = new Map();
  for (const [id, m] of keysOf) for (const k of m.keys()) (byKey.get(k) || byKey.set(k, []).get(k)).push(id);
  const idf = (k) => Math.log((Math.max(N, 6) + 1) / ((byKey.get(k)?.length || 0) + 0.5));

  // candidate pairs: notes sharing at least one keyword that isn't near-universal
  const pairs = new Map();
  for (const [k, ids] of byKey) {
    if (ids.length < 2 || ids.length > Math.max(3, Math.ceil(N * 0.5))) continue;
    for (let i = 0; i < ids.length; i++) for (let j = i + 1; j < ids.length; j++) {
      const pk = pairKey(ids[i], ids[j]);
      const p = pairs.get(pk) || pairs.set(pk, { a: ids[i], b: ids[j], shared: [] }).get(pk);
      p.shared.push(k);
    }
  }

  const existingKw = new Map(); // pairKey -> edge
  for (const e of graph.edges.values()) if (e.auto === 'keyword') existingKw.set(pairKey(e.from, e.to), e);

  let added = 0, removed = 0, updated = 0;
  const keep = new Set();
  for (const [pk, p] of pairs) {
    if (graph.kwIgnore.has(pk)) continue;
    const shared = dropSharedParts(p.shared).map((k) => ({ k, idf: idf(k) })).sort((x, y) => y.idf - x.idf);
    const score = shared.reduce((s, x) => s + x.idf, 0);
    if (score < threshold) continue;
    const label = shared.slice(0, 2).map((x) => keysOf.get(p.a).get(x.k)).join(', ');
    const manual = graph.hasEdge(p.a, p.b);
    if (manual && manual.auto !== 'keyword') continue;
    keep.add(pk);
    const e = existingKw.get(pk);
    if (e) { if (e.label !== label) { e.label = label; updated++; } }
    else { graph.addEdge(p.a, p.b, label, { auto: 'keyword' }); added++; }
  }
  for (const [pk, e] of existingKw) if (!keep.has(pk)) { graph.removeEdge(e.id); removed++; }
  return { added, removed, updated };
}
