// NodeGraph: a pan/zoom canvas of draggable note nodes with links between them, laid out by a
// live force simulation. Nodes are HTML elements inside a transformed "world" div; edges are SVG
// lines beneath them.
//
// Physics (all strengths scale with a node's number of connections, its "degree"):
//   - repulsion: every node pushes every other away; well-connected nodes push harder (more mass)
//   - tension:   linked nodes are joined by springs; stiffness and rest length grow with degree, so
//                hubs pull their cluster in but keep enough room for their satellites
//   - gravity:   nodes drift toward the map's centre of mass, hubs strongest, so leaves orbit outside
//   - collision: overlapping note rectangles are separated directly (no bouncing)
//   - labels:    cards keep clear of the keyword annotations drawn on links
// The simulation cools down (alpha -> 0) and stops; anything that changes the graph re-heats it.

export const NODE_W = 220;
const DEFAULT_H = 110;
const uid = () => Math.random().toString(36).slice(2, 10) + Date.now().toString(36);
const norm = (s) => (s || '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const clamp = (v, a, b) => Math.min(b, Math.max(a, v));

export const PHYSICS_DEFAULTS = { enabled: true, tension: 1, gravity: 1, repulsion: 1 };

export class NodeGraph {
  constructor(els, opts = {}) {
    this.el = els; // { canvas, world, edges, nodes }
    this.opts = opts; // { onSelect(id|null), onChange(), onEdit(id), onSettle() }
    this.nodes = new Map();
    this.edges = new Map();
    this.view = { x: 0, y: 0, k: 1 };
    this.physics = { ...PHYSICS_DEFAULTS };
    this.keywordLinks = { enabled: true, threshold: 1.0 };
    this.kwIgnore = new Set(); // "idA|idB" pairs the user unlinked; keyword linking leaves them alone
    this.selected = null;
    this.selectedEdge = null;
    this.filter = '';
    this.padRight = 0; // space covered by an overlay (inspector) that fit() should avoid
    this._linking = null;
    this._els = new Map(); // node id -> element
    this._vel = new Map(); // node id -> {vx, vy}
    this._sim = { alpha: 0, target: 0, raf: 0, corrected: 0, tail: 0 };
    this._labelW = new Map(); // link label text -> rendered width (px, world units)
    this._bind();
    this.applyView();
  }

  // ---------- data
  toJSON() { return { nodes: [...this.nodes.values()].map(({ _drag, ...n }) => n), edges: [...this.edges.values()], view: { ...this.view }, physics: { ...this.physics }, keywordLinks: { ...this.keywordLinks }, kwIgnore: [...this.kwIgnore] }; }
  load(data) {
    this.stopSim();
    this.nodes = new Map((data.nodes || []).map((n) => [n.id, n]));
    this.edges = new Map((data.edges || []).filter((e) => this.nodes.has(e.from) && this.nodes.has(e.to)).map((e) => [e.id, e]));
    if (data.view && Number.isFinite(data.view.k)) this.view = { ...data.view };
    this.physics = { ...PHYSICS_DEFAULTS, ...(data.physics || {}) };
    this.keywordLinks = { enabled: true, threshold: 1.0, ...(data.keywordLinks || {}) };
    this.kwIgnore = new Set(data.kwIgnore || []);
    this.selected = null; this.selectedEdge = null;
    this.el.nodes.innerHTML = ''; this._els.clear(); this._vel.clear();
    this.applyView();
    this.render();
    this.kick(0.25); // gentle settle in case sizes changed since last save
  }
  clear() { this.stopSim(); this.nodes.clear(); this.edges.clear(); this._vel.clear(); this.kwIgnore.clear(); this.selected = null; this.selectedEdge = null; this.render(); this._emitSelect(null); this._change(); }
  getNode(id) { return this.nodes.get(id); }
  allNodes() { return [...this.nodes.values()]; }
  findByTitle(title) {
    const t = norm(title); if (!t) return null;
    for (const n of this.nodes.values()) if (norm(n.title) === t) return n;
    return null;
  }
  addNode(partial = {}) {
    const n = { id: uid(), title: 'Untitled', body: '', kind: 'note', x: 0, y: 0, createdAt: Date.now(), ...partial };
    this.nodes.set(n.id, n);
    this._renderNode(n);
    this._change();
    this.kick(0.7);
    return n;
  }
  updateNode(id, patch) {
    const n = this.nodes.get(id); if (!n) return;
    Object.assign(n, patch);
    this._renderNode(n);
    this._renderEdges();
    this._change();
    if ('locked' in patch) this.kick(0.2);
  }
  removeNode(id) {
    if (!this.nodes.delete(id)) return;
    this._vel.delete(id);
    for (const [eid, e] of this.edges) if (e.from === id || e.to === id) this.edges.delete(eid);
    if (this.selected === id) { this.selected = null; this._emitSelect(null); }
    this.render();
    this._change();
    this.kick(0.5);
  }
  hasEdge(a, b) {
    for (const e of this.edges.values()) if ((e.from === a && e.to === b) || (e.from === b && e.to === a)) return e;
    return null;
  }
  addEdge(from, to, label = '', extra = {}) {
    if (from === to || !this.nodes.has(from) || !this.nodes.has(to)) return null;
    const existing = this.hasEdge(from, to); if (existing) return existing;
    const e = { id: uid(), from, to, label, ...extra };
    this.edges.set(e.id, e);
    this._renderEdges();
    this._change();
    this.kick(0.6);
    return e;
  }
  removeEdge(id) {
    if (!this.edges.delete(id)) return;
    if (this.selectedEdge === id) this.selectedEdge = null;
    this._renderEdges();
    this._change();
    this.kick(0.4);
  }
  neighbors(id) {
    const out = [];
    for (const e of this.edges.values()) {
      if (e.from === id) out.push({ edge: e, node: this.nodes.get(e.to) });
      else if (e.to === id) out.push({ edge: e, node: this.nodes.get(e.from) });
    }
    return out.filter((x) => x.node);
  }
  degree(id) { let d = 0; for (const e of this.edges.values()) if (e.from === id || e.to === id) d++; return d; }
  select(id) {
    this.selected = id && this.nodes.has(id) ? id : null;
    this.selectedEdge = null;
    this._updateSelection();
    this._emitSelect(this.selected);
  }
  deleteSelected() {
    if (this.selected) this.removeNode(this.selected);
    else if (this.selectedEdge) this.removeEdge(this.selectedEdge);
  }

  // ---------- placement (initial position; the simulation takes it from there)
  _rect(n) { const el = this._els.get(n.id); return { x: n.x, y: n.y, w: NODE_W, h: el?.offsetHeight || DEFAULT_H }; }
  _free(x, y, w = NODE_W, h = DEFAULT_H, pad = 28) {
    for (const n of this.nodes.values()) {
      const r = this._rect(n);
      if (x < r.x + r.w + pad && x + w + pad > r.x && y < r.y + r.h + pad && y + h + pad > r.y) return false;
    }
    return true;
  }
  viewCenter() {
    const c = this.el.canvas.getBoundingClientRect();
    return { x: ((c.width - this.padRight) / 2 - this.view.x) / this.view.k, y: (c.height / 2 - this.view.y) / this.view.k };
  }
  placeNear(anchorId) {
    const a = anchorId && this.nodes.get(anchorId);
    let cx, cy;
    if (a) { const r = this._rect(a); cx = r.x + r.w / 2; cy = r.y + r.h / 2; }
    else {
      ({ x: cx, y: cy } = this.viewCenter());
      if (this._free(cx - NODE_W / 2, cy - DEFAULT_H / 2)) return { x: Math.round(cx - NODE_W / 2), y: Math.round(cy - DEFAULT_H / 2) };
    }
    const golden = 2.399963;
    const seed = this.nodes.size;
    for (let ring = 1; ring < 14; ring++) {
      const radius = 170 + ring * 120;
      const steps = 6 + ring * 4;
      const start = (seed * golden) % (Math.PI * 2);
      for (let s = 0; s < steps; s++) {
        const ang = start + s * (Math.PI * 2 / steps);
        const x = cx + Math.cos(ang) * radius - NODE_W / 2;
        const y = cy + Math.sin(ang) * radius * 0.7 - DEFAULT_H / 2;
        if (this._free(x, y)) return { x: Math.round(x), y: Math.round(y) };
      }
    }
    return { x: Math.round(cx + Math.random() * 1200 - 600), y: Math.round(cy + Math.random() * 1200 - 600) };
  }

  // ---------- physics
  setPhysics(patch) {
    const wasOn = this.physics.enabled;
    Object.assign(this.physics, patch);
    this._change();
    if (!this.physics.enabled) this.stopSim();
    else this.kick(wasOn ? 0.5 : 0.8);
  }
  setKeywordLinks(patch) { Object.assign(this.keywordLinks, patch); this._change(); }
  /** Re-heat the simulation. alpha in (0, 1]: how energetic the reflow is. */
  kick(alpha = 1) {
    if (!this.physics.enabled || this.nodes.size < 2) return;
    this._sim.alpha = Math.max(this._sim.alpha, alpha);
    if (!this._sim.raf) this._sim.raf = requestAnimationFrame(this._loop);
  }
  stopSim() { if (this._sim.raf) cancelAnimationFrame(this._sim.raf); this._sim.raf = 0; this._sim.alpha = 0; this._sim.target = 0; }
  get simulating() { return !!this._sim.raf; }
  _loop = () => {
    this._sim.raf = 0;
    if (!this.physics.enabled) return;
    this._tick();
    this._applyPositions();
    const hot = this._sim.alpha > 0.004 || this._sim.target > 0;
    this._sim.tail = hot ? 0 : this._sim.tail + 1;
    if (hot || (this._sim.corrected > 0.4 && this._sim.tail < 240)) this._sim.raf = requestAnimationFrame(this._loop);
    else { this._sim.alpha = 0; this._change(); this.opts.onSettle?.(); }
  };
  /** One simulation step. Also used synchronously by tidy() when physics is off. */
  _tick() {
    const s = this._sim;
    s.alpha += (s.target - s.alpha) * 0.028;
    const alpha = s.alpha;
    const P = this.physics;
    const nodes = [...this.nodes.values()];
    const N = nodes.length; if (N < 2) return;

    // per-node scratch: centre, half-size, degree, mass
    const deg = new Map();
    for (const e of this.edges.values()) { deg.set(e.from, (deg.get(e.from) || 0) + 1); deg.set(e.to, (deg.get(e.to) || 0) + 1); }
    const b = nodes.map((n) => {
      const el = this._els.get(n.id);
      const h = el?.offsetHeight || DEFAULT_H;
      const d = deg.get(n.id) || 0;
      const v = this._vel.get(n.id) || this._vel.set(n.id, { vx: 0, vy: 0 }).get(n.id);
      return { n, cx: n.x + NODE_W / 2, cy: n.y + h / 2, hw: NODE_W / 2, hh: h / 2, d, m: 1 + Math.min(4, 0.4 * d), v, fx: 0, fy: 0, fixed: !!(n.locked || n._drag) };
    });
    const idx = new Map(b.map((o, i) => [o.n.id, i]));

    // centre of mass (gravity target)
    let cmx = 0, cmy = 0, mt = 0;
    for (const o of b) { cmx += o.cx * o.m; cmy += o.cy * o.m; mt += o.m; }
    cmx /= mt; cmy /= mt;

    // repulsion + collision, all pairs (notes are wide, so vertical distances count for more)
    const R = 30000 * P.repulsion;
    const YS = 1.6;
    for (let i = 0; i < N; i++) {
      const a = b[i];
      for (let j = i + 1; j < N; j++) {
        const c = b[j];
        let dx = a.cx - c.cx, dy = (a.cy - c.cy) * YS;
        let d2 = dx * dx + dy * dy;
        if (d2 < 1) { dx = (Math.random() - 0.5) * 2; dy = (Math.random() - 0.5) * 2; d2 = dx * dx + dy * dy; }
        const d = Math.sqrt(d2);
        const f = (R * a.m * c.m) / Math.max(d2, 120 * 120);
        const fx = dx / d * f, fy = dy / d * f;
        a.fx += fx; a.fy += fy; c.fx -= fx; c.fy -= fy;
      }
    }

    // springs along edges: tension and rest length scale with the degree of the endpoints
    const T = 0.09 * P.tension;
    for (const e of this.edges.values()) {
      const a = b[idx.get(e.from)], c = b[idx.get(e.to)];
      if (!a || !c) continue;
      const hi = Math.max(a.d, c.d);
      const rest = Math.min(560, 250 + 28 * hi);           // hubs get longer arms
      const k = T * (0.6 + 0.45 * Math.log2(1 + hi));       // ...and pull harder
      const dx = c.cx - a.cx, dy = c.cy - a.cy;
      const d = Math.sqrt(dx * dx + dy * dy) || 1;
      const f = k * (d - rest);
      const fx = dx / d * f, fy = dy / d * f;
      a.fx += fx; a.fy += fy; c.fx -= fx; c.fy -= fy;
    }

    // gravity toward centre of mass, stronger for well-connected nodes
    const G = 0.018 * P.gravity;
    for (const o of b) { o.fx += (cmx - o.cx) * G * o.m; o.fy += (cmy - o.cy) * G * o.m; }

    // integrate: heavy velocity damping and a modest speed cap keep motion smooth rather than twitchy
    const damping = 0.5, vmax = 16, rest = 0.05;
    for (const o of b) {
      if (o.fixed) { o.v.vx = 0; o.v.vy = 0; continue; }
      o.v.vx = (o.v.vx + o.fx * alpha) * damping;
      o.v.vy = (o.v.vy + o.fy * alpha) * damping;
      const sp = Math.hypot(o.v.vx, o.v.vy);
      if (sp > vmax) { o.v.vx *= vmax / sp; o.v.vy *= vmax / sp; }
      else if (sp < rest) { o.v.vx = 0; o.v.vy = 0; continue; }
      o.n.x += o.v.vx; o.n.y += o.v.vy;
      o.cx += o.v.vx; o.cy += o.v.vy;
    }

    // collision: separate overlapping cards by moving them apart directly (a fraction of the overlap
    // per frame, split by mass). Unlike a spring force this never overshoots, so nothing bounces or
    // shakes, and it keeps going until no overlap is left.
    const pad = 26, relax = 0.3;
    let corrected = 0;
    // move a and c apart by a fraction of their overlap (ox, oy) along the axis of least overlap
    const separate = (a, c, ox, oy, r) => {
      if (a.fixed && c.fixed) return;
      const wa = a.fixed ? 0 : (c.fixed ? 1 : c.m / (a.m + c.m)); // lighter node moves more
      const wc = 1 - wa;
      if (ox < oy) {
        const sgn = a.cx >= c.cx ? 1 : -1, d = ox * r;
        a.n.x += sgn * d * wa; a.cx += sgn * d * wa; c.n.x -= sgn * d * wc; c.cx -= sgn * d * wc;
        corrected = Math.max(corrected, d);
      } else {
        const sgn = a.cy >= c.cy ? 1 : -1, d = oy * r;
        a.n.y += sgn * d * wa; a.cy += sgn * d * wa; c.n.y -= sgn * d * wc; c.cy -= sgn * d * wc;
        corrected = Math.max(corrected, d);
      }
    };
    for (let i = 0; i < N; i++) {
      const a = b[i];
      for (let j = i + 1; j < N; j++) {
        const c = b[j];
        const ox = a.hw + c.hw + pad - Math.abs(a.cx - c.cx);
        const oy = a.hh + c.hh + pad - Math.abs(a.cy - c.cy);
        if (ox > 0 && oy > 0) separate(a, c, ox, oy, relax);
      }
    }

    // link labels (keyword annotations) must stay readable: keep the two linked cards far enough
    // apart that the label fits between them, and move every other card off the label's box
    const LH = 18, lpad = 14;
    for (const e of this.edges.values()) {
      if (!e.label) continue;
      const a = b[idx.get(e.from)], c = b[idx.get(e.to)];
      if (!a || !c) continue;
      const LW = (this._labelW.get(e.label) || e.label.length * 6.2) + 12;
      // endpoints: the label sits at the midpoint, so each card must be at least half the label away
      const reqX = a.hw + c.hw + LW + 2 * lpad, reqY = 2 * Math.max(a.hh, c.hh) + LH + 2 * lpad;
      const ex = reqX - Math.abs(a.cx - c.cx), ey = reqY - Math.abs(a.cy - c.cy);
      if (ex > 0 && ey > 0) separate(a, c, ex, ey, 0.15);
      // everyone else: clear the label box
      const mx = (a.cx + c.cx) / 2, my = (a.cy + c.cy) / 2 - 7;
      for (const o of b) {
        if (o === a || o === c || o.fixed) continue;
        const ox = o.hw + LW / 2 + lpad - Math.abs(o.cx - mx);
        const oy = o.hh + LH / 2 + lpad - Math.abs(o.cy - my);
        if (ox <= 0 || oy <= 0) continue;
        if (ox < oy) { const sgn = o.cx >= mx ? 1 : -1, d = ox * relax; o.n.x += sgn * d; o.cx += sgn * d; corrected = Math.max(corrected, d); }
        else { const sgn = o.cy >= my ? 1 : -1, d = oy * relax; o.n.y += sgn * d; o.cy += sgn * d; corrected = Math.max(corrected, d); }
      }
    }
    s.corrected = corrected;
  }
  _applyPositions() {
    for (const n of this.nodes.values()) {
      const el = this._els.get(n.id);
      if (el) el.style.transform = `translate(${Math.round(n.x)}px, ${Math.round(n.y)}px)`;
    }
    this._renderEdges();
  }
  /** Re-heat fully (physics on) or run a synchronous settle (physics off), then fit the view. */
  tidy() {
    if (this.nodes.size < 2) return;
    if (this.physics.enabled) { this.kick(1); this._fitOnSettle = true; return; }
    this._sim.alpha = 1;
    for (let i = 0; i < 320 && this._sim.alpha > 0.004; i++) this._tick();
    this._sim.alpha = 0;
    for (const n of this.nodes.values()) { n.x = Math.round(n.x); n.y = Math.round(n.y); }
    this._applyPositions(); this._change(); this.fit({ animate: true });
  }

  // ---------- rendering
  render() {
    for (const n of this.nodes.values()) this._renderNode(n);
    for (const [id, el] of this._els) if (!this.nodes.has(id)) { el.remove(); this._els.delete(id); }
    this._renderEdges();
    this._updateSelection();
    this._applyFilter();
    this.el.canvas.classList.toggle('empty', this.nodes.size === 0);
  }
  _renderNode(n) {
    let el = this._els.get(n.id);
    if (!el) {
      el = document.createElement('div');
      el.className = 'node';
      el.dataset.id = n.id;
      el.innerHTML = '<div class="node-head"><span class="node-kind"></span><span class="node-title"></span></div><div class="node-body"></div><div class="node-tags"></div><div class="node-port" title="Drag onto another note to link"></div><span class="node-lock" title="Position locked">⌖</span>';
      this.el.nodes.appendChild(el);
      this._els.set(n.id, el);
    }
    for (const c of [...el.classList]) if (c.startsWith('kind-')) el.classList.remove(c);
    el.classList.add(`kind-${n.kind || 'note'}`);
    el.classList.toggle('locked', !!n.locked);
    el.style.transform = `translate(${Math.round(n.x)}px, ${Math.round(n.y)}px)`;
    el.querySelector('.node-kind').textContent = n.kind || 'note';
    el.querySelector('.node-title').textContent = n.title || 'Untitled';
    el.querySelector('.node-body').textContent = n.body || '';
    const tags = el.querySelector('.node-tags'); tags.innerHTML = '';
    for (const kw of (n.keywords || []).slice(0, 4)) { const t = document.createElement('span'); t.className = 'node-tag'; t.textContent = kw; tags.appendChild(t); }
    el.classList.toggle('dim', !!this.filter && !this._matches(n));
    this.el.canvas.classList.remove('empty');
  }
  _renderEdges() {
    let html = '';
    for (const e of this.edges.values()) {
      const a = this.nodes.get(e.from), b = this.nodes.get(e.to);
      if (!a || !b) continue;
      const ra = this._rect(a), rb = this._rect(b);
      const x1 = ra.x + ra.w / 2, y1 = ra.y + ra.h / 2, x2 = rb.x + rb.w / 2, y2 = rb.y + rb.h / 2;
      const cls = 'edge' + (e.auto === 'keyword' ? ' kw' : '') + (this.selectedEdge === e.id ? ' selected' : '') + (this.filter && !(this._matches(a) && this._matches(b)) ? ' dim' : '');
      html += `<g class="${cls}" data-id="${e.id}"><line class="hit" x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}"/><line class="vis" x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}"/>`;
      if (e.label) html += `<text x="${(x1 + x2) / 2}" y="${(y1 + y2) / 2 - 4}">${esc(e.label)}</text>`;
      html += '</g>';
    }
    if (this._linking) {
      const a = this.nodes.get(this._linking.from);
      if (a) { const r = this._rect(a); html += `<line class="temp" x1="${r.x + r.w / 2}" y1="${r.y + r.h / 2}" x2="${this._linking.x}" y2="${this._linking.y}"/>`; }
    }
    this.el.edges.innerHTML = html;
    for (const t of this.el.edges.querySelectorAll('text')) {
      if (!this._labelW.has(t.textContent)) { try { this._labelW.set(t.textContent, t.getComputedTextLength()); } catch { /* not rendered yet */ } }
    }
  }
  _updateSelection() {
    for (const [id, el] of this._els) el.classList.toggle('selected', id === this.selected);
    for (const g of this.el.edges.querySelectorAll('.edge')) g.classList.toggle('selected', g.dataset.id === this.selectedEdge);
  }
  _matches(n) { return !this.filter || norm(n.title + ' ' + n.body + ' ' + (n.keywords || []).join(' ')).includes(this.filter); }
  _applyFilter() { for (const n of this.nodes.values()) this._els.get(n.id)?.classList.toggle('dim', !!this.filter && !this._matches(n)); }
  setFilter(q) { this.filter = norm(q); this._applyFilter(); this._renderEdges(); }
  flash(id) {
    const el = this._els.get(id); if (!el) return;
    el.classList.remove('flash'); void el.offsetWidth; el.classList.add('flash');
    setTimeout(() => el.classList.remove('flash'), 1900);
  }

  // ---------- view
  applyView() { this.el.world.style.transform = `translate(${this.view.x}px, ${this.view.y}px) scale(${this.view.k})`; }
  zoomAt(mx, my, factor) {
    const k = clamp(this.view.k * factor, 0.12, 2.5);
    const f = k / this.view.k;
    this.view.x = mx - (mx - this.view.x) * f;
    this.view.y = my - (my - this.view.y) * f;
    this.view.k = k;
    this.applyView();
  }
  zoomBy(factor) { const r = this.el.canvas.getBoundingClientRect(); this.zoomAt((r.width - this.padRight) / 2, r.height / 2, factor); this._changeSoon(); }
  toWorld(clientX, clientY) {
    const r = this.el.canvas.getBoundingClientRect();
    return { x: (clientX - r.left - this.view.x) / this.view.k, y: (clientY - r.top - this.view.y) / this.view.k };
  }
  bounds() {
    if (!this.nodes.size) return null;
    let x1 = Infinity, y1 = Infinity, x2 = -Infinity, y2 = -Infinity;
    for (const n of this.nodes.values()) { const r = this._rect(n); x1 = Math.min(x1, r.x); y1 = Math.min(y1, r.y); x2 = Math.max(x2, r.x + r.w); y2 = Math.max(y2, r.y + r.h); }
    return { x1, y1, x2, y2 };
  }
  fit({ animate = false } = {}) {
    const b = this.bounds(); if (!b) return;
    const r = this.el.canvas.getBoundingClientRect();
    const W = Math.max(200, r.width - this.padRight), H = r.height, pad = 50;
    const w = b.x2 - b.x1, h = b.y2 - b.y1;
    const k = clamp(Math.min((W - pad * 2) / w, (H - pad * 2) / h), 0.12, 1.5);
    this._animateView({ k, x: (W - w * k) / 2 - b.x1 * k, y: (H - h * k) / 2 - b.y1 * k }, animate);
  }
  centerOn(id, animate = true) {
    const n = this.nodes.get(id); if (!n) return;
    const r = this.el.canvas.getBoundingClientRect();
    const rc = this._rect(n);
    const k = Math.max(this.view.k, 0.8);
    this._animateView({ k, x: (r.width - this.padRight) / 2 - (rc.x + rc.w / 2) * k, y: r.height / 2 - (rc.y + rc.h / 2) * k }, animate);
  }
  _animateView(target, animate) {
    if (!animate) { Object.assign(this.view, target); this.applyView(); this._changeSoon(); return; }
    const from = { ...this.view }, t0 = performance.now(), dur = 380;
    const step = (t) => {
      const p = Math.min(1, (t - t0) / dur), e = 1 - Math.pow(1 - p, 3);
      this.view.x = from.x + (target.x - from.x) * e;
      this.view.y = from.y + (target.y - from.y) * e;
      this.view.k = from.k + (target.k - from.k) * e;
      this.applyView();
      if (p < 1) requestAnimationFrame(step); else this._changeSoon();
    };
    requestAnimationFrame(step);
  }

  // ---------- events
  _emitSelect(id) { this.opts.onSelect?.(id); }
  _change() { this.opts.onChange?.(); }
  _changeSoon() { clearTimeout(this._ct); this._ct = setTimeout(() => this._change(), 300); }
  _bind() {
    const c = this.el.canvas;
    let drag = null;

    c.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0) return;
      const port = ev.target.closest('.node-port');
      const nodeEl = ev.target.closest('.node');
      const edgeEl = ev.target.closest('.edge');
      if (port && nodeEl) {
        const p = this.toWorld(ev.clientX, ev.clientY);
        this._linking = { from: nodeEl.dataset.id, x: p.x, y: p.y };
        drag = { type: 'link', from: nodeEl.dataset.id };
        this._renderEdges();
      } else if (nodeEl) {
        const n = this.nodes.get(nodeEl.dataset.id); if (!n) return;
        drag = { type: 'node', id: n.id, sx: ev.clientX, sy: ev.clientY, ox: n.x, oy: n.y, moved: false };
        nodeEl.classList.add('dragging');
      } else if (edgeEl) {
        this.selected = null; this.selectedEdge = edgeEl.dataset.id;
        this._updateSelection(); this._emitSelect(null);
        return;
      } else {
        drag = { type: 'pan', sx: ev.clientX, sy: ev.clientY, ox: this.view.x, oy: this.view.y, moved: false };
        c.classList.add('panning');
      }
      c.setPointerCapture(ev.pointerId);
      ev.preventDefault();
    });

    c.addEventListener('pointermove', (ev) => {
      if (!drag) return;
      const dx = ev.clientX - drag.sx, dy = ev.clientY - drag.sy;
      if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
      if (drag.type === 'pan') {
        this.view.x = drag.ox + dx; this.view.y = drag.oy + dy; this.applyView();
      } else if (drag.type === 'node') {
        const n = this.nodes.get(drag.id); if (!n) return;
        n.x = Math.round(drag.ox + dx / this.view.k); n.y = Math.round(drag.oy + dy / this.view.k);
        if (drag.moved && !n._drag) { n._drag = true; this._sim.target = 0.18; this.kick(0.18); } // neighbours reflow live, gently
        this._els.get(n.id).style.transform = `translate(${n.x}px, ${n.y}px)`;
        this._renderEdges();
      } else if (drag.type === 'link') {
        const p = this.toWorld(ev.clientX, ev.clientY);
        this._linking.x = p.x; this._linking.y = p.y;
        const t = this.el.edges.querySelector('.temp');
        if (t) { t.setAttribute('x2', p.x); t.setAttribute('y2', p.y); }
        const over = document.elementFromPoint(ev.clientX, ev.clientY)?.closest('.node');
        for (const el of this.el.nodes.querySelectorAll('.link-target')) el.classList.remove('link-target');
        if (over && over.dataset.id !== drag.from) over.classList.add('link-target');
      }
    });

    const end = (ev) => {
      if (!drag) return;
      const d = drag; drag = null;
      c.classList.remove('panning');
      if (d.type === 'node') {
        this._els.get(d.id)?.classList.remove('dragging');
        const n = this.nodes.get(d.id);
        if (n) { n._drag = false; this._sim.target = 0; }
        if (!d.moved) this.select(d.id);
        else { this._change(); this.kick(0.5); }
      } else if (d.type === 'pan') {
        if (!d.moved) this.select(null); else this._changeSoon();
      } else if (d.type === 'link') {
        this._linking = null;
        const over = document.elementFromPoint(ev.clientX, ev.clientY)?.closest('.node');
        for (const el of this.el.nodes.querySelectorAll('.link-target')) el.classList.remove('link-target');
        if (over && over.dataset.id !== d.from) { this.addEdge(d.from, over.dataset.id); this.select(over.dataset.id); }
        this._renderEdges();
      }
    };
    c.addEventListener('pointerup', end);
    c.addEventListener('pointercancel', end);

    // Scroll pans, ⌘/Ctrl+scroll (and trackpad pinch) zooms.
    let pinch = 0; // Safari and the desktop app (WebKit) report a pinch as gesture events, not as ctrl+wheel
    c.addEventListener('gesturestart', (ev) => { ev.preventDefault(); pinch = 1; });
    c.addEventListener('gesturechange', (ev) => {
      ev.preventDefault();
      const r = c.getBoundingClientRect();
      this.zoomAt(ev.clientX - r.left, ev.clientY - r.top, ev.scale / pinch); pinch = ev.scale;
    });
    c.addEventListener('gestureend', (ev) => { ev.preventDefault(); pinch = 0; this._changeSoon(); });
    c.addEventListener('wheel', (ev) => {
      ev.preventDefault();
      if (pinch && ev.ctrlKey) return;
      const s = ev.deltaMode === 1 ? 16 : 1;
      if (ev.ctrlKey || ev.metaKey) {
        const r = c.getBoundingClientRect();
        this.zoomAt(ev.clientX - r.left, ev.clientY - r.top, Math.exp(-ev.deltaY * s * 0.01));
      } else {
        this.view.x -= ev.deltaX * s; this.view.y -= ev.deltaY * s; this.applyView();
      }
      this._changeSoon();
    }, { passive: false });

    c.addEventListener('dblclick', (ev) => {
      const nodeEl = ev.target.closest('.node');
      if (nodeEl) { this.opts.onEdit?.(nodeEl.dataset.id); return; }
      if (ev.target.closest('.edge')) return;
      const p = this.toWorld(ev.clientX, ev.clientY);
      const n = this.addNode({ x: Math.round(p.x - NODE_W / 2), y: Math.round(p.y - 30) });
      this.select(n.id);
      this.flash(n.id);
      this.opts.onEdit?.(n.id);
    });

    document.addEventListener('keydown', (ev) => {
      const t = ev.target;
      if (t.closest?.('input, textarea, select, [contenteditable], dialog')) return;
      if (ev.key === 'Delete' || ev.key === 'Backspace') { if (this.selected || this.selectedEdge) { ev.preventDefault(); this.deleteSelected(); } }
      else if (ev.key === 'Escape') this.select(null);
    });
  }
}
