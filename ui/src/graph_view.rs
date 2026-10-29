//! The notes map on screen (graph.js): a pan/zoom canvas of draggable note cards with links between them, laid
//! out live by `notes_core::layout`. Cards are HTML elements inside a transformed "world" div; links are SVG lines
//! beneath them. The map data itself is a `notes_core::graph::Graph`.
//!
//! Callbacks (select, change, edit) run after this view's state is released, so they may call back into it.

use crate::dom::{self, closest, data, esc, num, on, on_active, request_frame, toggle_class};
use notes_core::extract::{self, Applied, Placement};
use notes_core::graph::{self, DEFAULT_H, Graph, NODE_W, new_id};
use notes_core::keywords::{self, Refresh};
use notes_core::layout::{self, Measure, Sim, Velocity};
use notes_core::model::{Edge, GraphData, KeywordLinks, Node, Physics, Source, View};
use notes_core::view;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, MouseEvent, PointerEvent, WheelEvent};

pub enum GraphEvent {
    Select(Option<String>),
    Change,
    Edit(String),
}

enum Drag {
    Link { from: String },
    Node { id: String, sx: f64, sy: f64, ox: f64, oy: f64, moved: bool },
    Pan { sx: f64, sy: f64, ox: f64, oy: f64, moved: bool },
}

pub struct Els {
    pub canvas: HtmlElement,
    pub world: HtmlElement,
    pub edges: Element,
    pub nodes: HtmlElement,
}

struct State {
    g: Graph,
    selected: Option<String>,
    selected_edge: Option<String>,
    filter: String,
    /// Space covered by an overlay (the inspector) that fit() should avoid.
    pad_right: f64,
    linking: Option<(String, f64, f64)>,
    els: HashMap<String, HtmlElement>,
    vel: HashMap<String, Velocity>,
    sim: Sim,
    frame: Option<i32>,
    /// Link label text -> rendered width (world units).
    label_w: HashMap<String, f64>,
    /// The note being dragged; the layout holds it in place.
    dragging: Option<String>,
    fit_on_settle: bool,
    rng: fastrand::Rng,
    drag: Option<Drag>,
    /// Safari and the desktop app (WebKit) report a pinch as gesture events, not as ctrl+wheel.
    pinch: f64,
}

type Handler = Rc<dyn Fn(GraphEvent)>;

pub struct GraphView {
    el: Els,
    s: RefCell<State>,
    handler: RefCell<Option<Handler>>,
    change_soon: dom::Timer,
    me: RefCell<Weak<GraphView>>,
}

fn height_of(els: &HashMap<String, HtmlElement>, id: &str) -> f64 {
    match els.get(id).map(|e| e.offset_height()) {
        Some(h) if h > 0 => h as f64,
        _ => DEFAULT_H,
    }
}

fn translate(n: &Node) -> String {
    format!("translate({}px, {}px)", n.x.round(), n.y.round())
}

impl GraphView {
    pub fn new(el: Els) -> Rc<GraphView> {
        let seed = (dom::now() as u64) ^ ((js_sys::Math::random() * 1e15) as u64);
        let v = Rc::new(GraphView {
            el,
            s: RefCell::new(State {
                g: Graph::default(),
                selected: None,
                selected_edge: None,
                filter: String::new(),
                pad_right: 0.0,
                linking: None,
                els: HashMap::new(),
                vel: HashMap::new(),
                sim: Sim::default(),
                frame: None,
                label_w: HashMap::new(),
                dragging: None,
                fit_on_settle: false,
                rng: fastrand::Rng::with_seed(seed),
                drag: None,
                pinch: 0.0,
            }),
            handler: RefCell::new(None),
            change_soon: dom::Timer::default(),
            me: RefCell::new(Weak::new()),
        });
        *v.me.borrow_mut() = Rc::downgrade(&v);
        v.bind();
        v.apply_view();
        v
    }

    pub fn set_handler(&self, f: impl Fn(GraphEvent) + 'static) {
        *self.handler.borrow_mut() = Some(Rc::new(f));
    }

    fn emit(&self, ev: GraphEvent) {
        let h = self.handler.borrow().clone();
        if let Some(h) = h {
            h(ev);
        }
    }

    fn change(&self) {
        self.emit(GraphEvent::Change);
    }

    fn change_later(&self) {
        let me = self.me.borrow().clone();
        self.change_soon.start(300, move || {
            if let Some(v) = me.upgrade() {
                v.change_soon.forget();
                v.change();
            }
        });
    }

    /// Read access to the map data.
    pub fn with<R>(&self, f: impl FnOnce(&Graph) -> R) -> R {
        f(&self.s.borrow().g)
    }

    pub fn node(&self, id: &str) -> Option<Node> {
        self.s.borrow().g.nodes.get(id).cloned()
    }

    pub fn counts(&self) -> (usize, usize) {
        let s = self.s.borrow();
        (s.g.nodes.len(), s.g.edges.len())
    }

    pub fn view(&self) -> View {
        self.s.borrow().g.view
    }

    pub fn physics(&self) -> Physics {
        self.s.borrow().g.physics.clone()
    }

    pub fn keyword_links(&self) -> KeywordLinks {
        self.s.borrow().g.keyword_links.clone()
    }

    pub fn set_pad_right(&self, px: f64) {
        self.s.borrow_mut().pad_right = px;
    }

    pub fn set_fit_on_settle(&self) {
        self.s.borrow_mut().fit_on_settle = true;
    }

    fn uid(s: &mut State) -> String {
        new_id(&mut s.rng, dom::now())
    }

    // ---------- data

    pub fn to_data(&self) -> GraphData {
        self.s.borrow().g.to_data()
    }

    pub fn load(&self, data: GraphData) {
        self.stop_sim();
        {
            let mut s = self.s.borrow_mut();
            s.g = Graph::from_data(data);
            s.selected = None;
            s.selected_edge = None;
            s.els.clear();
            s.vel.clear();
        }
        self.el.nodes.set_inner_html("");
        self.apply_view();
        self.render();
        self.kick(0.25); // gentle settle in case sizes changed since the last save
    }

    pub fn clear(&self) {
        self.stop_sim();
        {
            let mut s = self.s.borrow_mut();
            s.g.clear();
            s.vel.clear();
            s.selected = None;
            s.selected_edge = None;
        }
        self.render();
        self.emit(GraphEvent::Select(None));
        self.change();
    }

    pub fn add_node(&self, node: Node) -> String {
        let id = {
            let mut s = self.s.borrow_mut();
            let id = s.g.add_node(node);
            self.render_node(&mut s, &id);
            id
        };
        self.change();
        self.kick(0.7);
        id
    }

    /// A new note with the map's defaults (and its own id), at (x, y).
    pub fn new_node(&self, x: f64, y: f64) -> Node {
        let mut s = self.s.borrow_mut();
        Node { x, y, ..Node::new(Self::uid(&mut s), dom::now()) }
    }

    pub fn update_node(&self, id: &str, patch: impl FnOnce(&mut Node)) {
        let relock = {
            let mut s = self.s.borrow_mut();
            let Some(n) = s.g.nodes.get_mut(id) else { return };
            let locked = n.locked;
            patch(n);
            let relock = n.locked != locked;
            self.render_node(&mut s, id);
            self.render_edges(&mut s);
            relock
        };
        self.change();
        if relock {
            self.kick(0.2);
        }
    }

    pub fn remove_node(&self, id: &str) {
        let was_selected = {
            let mut s = self.s.borrow_mut();
            if !s.g.remove_node(id) {
                return;
            }
            s.vel.remove(id);
            let sel = s.selected.as_deref() == Some(id);
            if sel {
                s.selected = None;
            }
            sel
        };
        if was_selected {
            self.emit(GraphEvent::Select(None));
        }
        self.render();
        self.change();
        self.kick(0.5);
    }

    pub fn add_edge(&self, from: &str, to: &str) -> Option<String> {
        let added = {
            let mut s = self.s.borrow_mut();
            let id = Self::uid(&mut s);
            let r = s.g.add_edge(Edge { id, from: from.into(), to: to.into(), ..Edge::default() });
            if r.as_ref().is_some_and(|(_, new)| *new) {
                self.render_edges(&mut s);
            }
            r
        };
        let (id, new) = added?;
        if new {
            self.change();
            self.kick(0.6);
        }
        Some(id)
    }

    pub fn remove_edge(&self, id: &str) {
        {
            let mut s = self.s.borrow_mut();
            if !s.g.remove_edge(id) {
                return;
            }
            if s.selected_edge.as_deref() == Some(id) {
                s.selected_edge = None;
            }
            self.render_edges(&mut s);
        }
        self.change();
        self.kick(0.4);
    }

    /// Removes every keyword link. Returns how many there were.
    pub fn remove_keyword_links(&self) -> usize {
        let ids: Vec<String> =
            self.with(|g| g.edges.values().filter(|e| e.is_keyword()).map(|e| e.id.clone()).collect());
        for id in &ids {
            self.remove_edge(id);
        }
        ids.len()
    }

    /// Unlinks a pair for keyword linking from now on.
    pub fn ignore_pair(&self, a: &str, b: &str) {
        self.s.borrow_mut().g.kw_ignore.insert(graph::pair_key(a, b));
    }

    pub fn select(&self, id: Option<&str>) {
        let sel = {
            let mut s = self.s.borrow_mut();
            let sel = id.filter(|id| s.g.nodes.contains_key(*id)).map(str::to_string);
            s.selected = sel;
            s.selected_edge = None;
            self.update_selection(&s);
            s.selected.clone()
        };
        self.emit(GraphEvent::Select(sel));
    }

    fn delete_selected(&self) {
        let (node, edge) = {
            let s = self.s.borrow();
            (s.selected.clone(), s.selected_edge.clone())
        };
        if let Some(id) = node {
            self.remove_node(&id);
        } else if let Some(id) = edge {
            self.remove_edge(&id);
        }
    }

    /// The canvas's size as (width, height).
    fn canvas_size(&self) -> (f64, f64) {
        let r = self.el.canvas.get_bounding_client_rect();
        (r.width(), r.height())
    }

    fn view_center(&self, s: &State) -> (f64, f64) {
        let (w, h) = self.canvas_size();
        let v = s.g.view;
        (((w - s.pad_right) / 2.0 - v.x) / v.k, (h / 2.0 - v.y) / v.k)
    }

    /// Where a new note goes (see `Graph::place_near`).
    pub fn place_near(&self, anchor: Option<&str>) -> (f64, f64) {
        let mut s = self.s.borrow_mut();
        let center = self.view_center(&s);
        let State { g, els, rng, .. } = &mut *s;
        g.place_near(anchor, center, &|id| height_of(els, id), rng)
    }

    /// Puts an extraction result's notes in the map around `hub` (see `notes_core::extract::apply_notes`).
    pub fn apply_notes(&self, result: &Value, hub: &str, source: &Source) -> Applied {
        let applied = {
            let mut s = self.s.borrow_mut();
            let center = self.view_center(&s);
            let mut id_rng = fastrand::Rng::with_seed(s.rng.u64(..));
            let State { g, els, rng, .. } = &mut *s;
            let now = dom::now();
            let mut next_id = || new_id(&mut id_rng, now);
            let mut place = |g: &Graph, anchor: &str| g.place_near(Some(anchor), center, &|id| height_of(els, id), rng);
            extract::apply_notes(
                g,
                result,
                hub,
                source,
                &mut Placement { new_id: &mut next_id, now, place_near: &mut place },
            )
        };
        self.render();
        self.change();
        self.kick(0.7);
        applied
    }

    /// Recomputes stale keywords and keyword links (see `notes_core::keywords::refresh`).
    pub fn refresh_keywords(&self, force: bool) -> Refresh {
        let r = {
            let mut s = self.s.borrow_mut();
            let mut id_rng = fastrand::Rng::with_seed(s.rng.u64(..));
            let now = dom::now();
            keywords::refresh(&mut s.g, force, &mut || new_id(&mut id_rng, now))
        };
        if r.links.added > 0 {
            self.kick(0.6);
        } else if r.links.removed > 0 {
            self.kick(0.4);
        }
        r
    }

    // ---------- physics

    pub fn set_physics(&self, patch: impl FnOnce(&mut Physics)) {
        let (was_on, on) = {
            let mut s = self.s.borrow_mut();
            let was = s.g.physics.enabled;
            patch(&mut s.g.physics);
            (was, s.g.physics.enabled)
        };
        self.change();
        if !on {
            self.stop_sim();
        } else {
            self.kick(if was_on { 0.5 } else { 0.8 });
        }
    }

    pub fn set_keyword_links(&self, patch: impl FnOnce(&mut KeywordLinks)) {
        patch(&mut self.s.borrow_mut().g.keyword_links);
        self.change();
    }

    /// Re-heats the simulation; `alpha` in (0, 1] is how energetic the reflow is.
    pub fn kick(&self, alpha: f64) {
        let mut s = self.s.borrow_mut();
        if !s.g.physics.enabled || s.g.nodes.len() < 2 {
            return;
        }
        s.sim.heat(alpha);
        if s.frame.is_none() {
            s.frame = Some(self.next_frame());
        }
    }

    fn next_frame(&self) -> i32 {
        let me = self.me.borrow().clone();
        request_frame(move |_| {
            if let Some(v) = me.upgrade() {
                v.frame();
            }
        })
    }

    pub fn stop_sim(&self) {
        let mut s = self.s.borrow_mut();
        if let Some(f) = s.frame.take() {
            dom::cancel_frame(f);
        }
        s.sim.alpha = 0.0;
        s.sim.target = 0.0;
    }

    fn step(s: &mut State) {
        let State { g, els, vel, sim, label_w, dragging, rng, .. } = s;
        let height = |id: &str| height_of(els, id);
        let label_width = |l: &str| label_w.get(l).copied();
        layout::tick(sim, g, vel, &Measure { height: &height, label_width: &label_width }, dragging.as_deref(), rng);
    }

    fn frame(&self) {
        let settled = {
            let mut s = self.s.borrow_mut();
            s.frame = None;
            if !s.g.physics.enabled {
                return;
            }
            Self::step(&mut s);
            self.apply_positions(&mut s);
            if s.sim.next_frame() {
                s.frame = Some(self.next_frame());
                false
            } else {
                true
            }
        };
        if settled {
            self.change();
            let fit = std::mem::take(&mut self.s.borrow_mut().fit_on_settle);
            if fit {
                self.fit(true);
            }
        }
    }

    fn apply_positions(&self, s: &mut State) {
        for n in s.g.nodes.values() {
            if let Some(el) = s.els.get(&n.id) {
                let _ = el.style().set_property("transform", &translate(n));
            }
        }
        self.render_edges(s);
    }

    /// Re-heats fully (physics on), or settles in one go (physics off), then fits the view.
    pub fn tidy(&self) {
        if self.with(|g| g.nodes.len()) < 2 {
            return;
        }
        if self.physics().enabled {
            self.kick(1.0);
            self.set_fit_on_settle();
            return;
        }
        {
            let mut s = self.s.borrow_mut();
            let State { g, els, vel, label_w, rng, .. } = &mut *s;
            let height = |id: &str| height_of(els, id);
            let label_width = |l: &str| label_w.get(l).copied();
            layout::settle(g, vel, &Measure { height: &height, label_width: &label_width }, rng);
            self.apply_positions(&mut s);
        }
        self.change();
        self.fit(true);
    }

    // ---------- rendering

    pub fn render(&self) {
        {
            let mut s = self.s.borrow_mut();
            let ids: Vec<String> = s.g.nodes.keys().cloned().collect();
            for id in &ids {
                self.render_node(&mut s, id);
            }
            let gone: Vec<String> = s.els.keys().filter(|id| !s.g.nodes.contains_key(*id)).cloned().collect();
            for id in gone {
                if let Some(el) = s.els.remove(&id) {
                    el.remove();
                }
            }
            self.render_edges(&mut s);
            self.update_selection(&s);
            self.apply_filter(&s);
            toggle_class(&self.el.canvas, "empty", s.g.nodes.is_empty());
        }
    }

    fn render_node(&self, s: &mut State, id: &str) {
        let Some(n) = s.g.nodes.get(id) else { return };
        let el = match s.els.get(id) {
            Some(el) => el.clone(),
            None => {
                let el: HtmlElement = dom::document().create_element("div").expect("div").unchecked_into();
                el.set_class_name("node");
                let _ = el.set_attribute("data-id", id);
                el.set_inner_html("<div class=\"node-head\"><span class=\"node-kind\"></span><span class=\"node-title\"></span></div><div class=\"node-body\"></div><div class=\"node-tags\"></div><div class=\"node-port\" title=\"Drag onto another note to link\"></div><span class=\"node-lock\" title=\"Position locked\">⌖</span>");
                let _ = self.el.nodes.append_child(&el);
                s.els.insert(id.to_string(), el.clone());
                el
            }
        };
        let classes = el.class_list();
        for i in (0..classes.length()).rev() {
            if let Some(c) = classes.item(i).filter(|c| c.starts_with("kind-")) {
                let _ = classes.remove_1(&c);
            }
        }
        let _ = classes.add_1(&format!("kind-{}", n.kind_or_note()));
        toggle_class(&el, "locked", n.is_locked());
        let _ = el.style().set_property("transform", &translate(n));
        let set = |sel: &str, text: &str| {
            if let Some(e) = dom::query::<HtmlElement>(&el, sel) {
                e.set_text_content(Some(text));
            }
        };
        set(".node-kind", n.kind_or_note());
        set(".node-title", if n.title.is_empty() { "Untitled" } else { &n.title });
        set(".node-body", &n.body);
        if let Some(tags) = dom::query::<HtmlElement>(&el, ".node-tags") {
            tags.set_inner_html("");
            for kw in n.keywords().iter().take(4) {
                if let Ok(t) = dom::document().create_element("span") {
                    t.set_class_name("node-tag");
                    t.set_text_content(Some(kw));
                    let _ = tags.append_child(&t);
                }
            }
        }
        toggle_class(&el, "dim", !s.filter.is_empty() && !graph::matches(n, &s.filter));
        toggle_class(&self.el.canvas, "empty", false);
    }

    fn render_edges(&self, s: &mut State) {
        let mut html = String::new();
        for e in s.g.edges.values() {
            let (Some(a), Some(b)) = (s.g.nodes.get(&e.from), s.g.nodes.get(&e.to)) else { continue };
            let (ha, hb) = (height_of(&s.els, &a.id), height_of(&s.els, &b.id));
            let (x1, y1) = (a.x + NODE_W / 2.0, a.y + ha / 2.0);
            let (x2, y2) = (b.x + NODE_W / 2.0, b.y + hb / 2.0);
            let dim = !s.filter.is_empty() && !(graph::matches(a, &s.filter) && graph::matches(b, &s.filter));
            let cls = format!(
                "edge{}{}{}",
                if e.is_keyword() { " kw" } else { "" },
                if s.selected_edge.as_deref() == Some(e.id.as_str()) { " selected" } else { "" },
                if dim { " dim" } else { "" }
            );
            let line = format!("x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"", num(x1), num(y1), num(x2), num(y2));
            html.push_str(&format!(
                "<g class=\"{cls}\" data-id=\"{}\"><line class=\"hit\" {line}/><line class=\"vis\" {line}/>",
                esc(&e.id)
            ));
            if !e.label.is_empty() {
                html.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\">{}</text>",
                    num((x1 + x2) / 2.0),
                    num((y1 + y2) / 2.0 - 4.0),
                    esc(&e.label)
                ));
            }
            html.push_str("</g>");
        }
        if let Some((from, x, y)) = &s.linking
            && let Some(a) = s.g.nodes.get(from)
        {
            let h = height_of(&s.els, &a.id);
            html.push_str(&format!(
                "<line class=\"temp\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>",
                num(a.x + NODE_W / 2.0),
                num(a.y + h / 2.0),
                num(*x),
                num(*y)
            ));
        }
        self.el.edges.set_inner_html(&html);
        for t in dom::query_all(&self.el.edges, "text") {
            let text = t.text_content().unwrap_or_default();
            if !s.label_w.contains_key(&text)
                && let Some(t) = t.dyn_ref::<web_sys::SvgTextContentElement>()
            {
                let w = t.get_computed_text_length() as f64;
                if w > 0.0 {
                    s.label_w.insert(text, w);
                }
            }
        }
    }

    fn update_selection(&self, s: &State) {
        for (id, el) in &s.els {
            toggle_class(el, "selected", s.selected.as_deref() == Some(id.as_str()));
        }
        for g in dom::query_all(&self.el.edges, ".edge") {
            toggle_class(&g, "selected", data(&g, "id").is_some() && data(&g, "id") == s.selected_edge);
        }
    }

    fn apply_filter(&self, s: &State) {
        for n in s.g.nodes.values() {
            if let Some(el) = s.els.get(&n.id) {
                toggle_class(el, "dim", !s.filter.is_empty() && !graph::matches(n, &s.filter));
            }
        }
    }

    pub fn set_filter(&self, q: &str) {
        let mut s = self.s.borrow_mut();
        s.filter = graph::norm(q);
        self.apply_filter(&s);
        self.render_edges(&mut s);
    }

    /// Briefly highlights a card.
    pub fn flash(&self, id: &str) {
        let Some(el) = self.s.borrow().els.get(id).cloned() else { return };
        let _ = el.class_list().remove_1("flash");
        let _ = el.offset_width(); // restart the animation
        let _ = el.class_list().add_1("flash");
        dom::set_timeout(1900, move || {
            let _ = el.class_list().remove_1("flash");
        });
    }

    // ---------- view

    fn apply_view(&self) {
        let v = self.view();
        let _ =
            self.el.world.style().set_property("transform", &format!("translate({}px, {}px) scale({})", v.x, v.y, v.k));
    }

    fn zoom_at(&self, mx: f64, my: f64, factor: f64) {
        view::zoom_at(&mut self.s.borrow_mut().g.view, mx, my, factor);
        self.apply_view();
    }

    pub fn zoom_by(&self, factor: f64) {
        let (w, h) = self.canvas_size();
        let pad = self.s.borrow().pad_right;
        self.zoom_at((w - pad) / 2.0, h / 2.0, factor);
        self.change_later();
    }

    fn to_world(&self, client_x: f64, client_y: f64) -> (f64, f64) {
        let r = self.el.canvas.get_bounding_client_rect();
        view::to_world(&self.view(), client_x - r.left(), client_y - r.top())
    }

    /// Frames every note.
    pub fn fit(&self, animate: bool) {
        let (w, h) = self.canvas_size();
        let target = {
            let s = self.s.borrow();
            let Some(b) = s.g.bounds(&|id| height_of(&s.els, id)) else { return };
            view::fit(b, w, h, s.pad_right)
        };
        self.animate_view(target, animate);
    }

    /// Centres a note, zoomed in to at least 0.8.
    pub fn center_on(&self, id: &str) {
        let (w, h) = self.canvas_size();
        let target = {
            let s = self.s.borrow();
            let Some(n) = s.g.nodes.get(id) else { return };
            view::center_on(&s.g.view, s.g.rect(n, &|id| height_of(&s.els, id)), w, h, s.pad_right)
        };
        self.animate_view(target, true);
    }

    fn animate_view(&self, target: View, animate: bool) {
        if !animate {
            self.s.borrow_mut().g.view = target;
            self.apply_view();
            self.change_later();
            return;
        }
        let from = self.view();
        let t0 = dom::perf_now();
        fn ease_step(me: Weak<GraphView>, from: View, target: View, t0: f64) {
            request_frame(move |_| {
                let Some(v) = me.upgrade() else { return };
                let p = ((dom::perf_now() - t0) / 380.0).min(1.0);
                v.s.borrow_mut().g.view = view::ease(&from, &target, p);
                v.apply_view();
                if p < 1.0 {
                    ease_step(me, from, target, t0);
                } else {
                    v.change_later();
                }
            });
        }
        ease_step(self.me.borrow().clone(), from, target, t0);
    }

    // ---------- events

    fn bind(self: &Rc<Self>) {
        let c = self.el.canvas.clone();

        let me = Rc::downgrade(self);
        on(&c, "pointerdown", move |ev: PointerEvent| {
            let Some(v) = me.upgrade() else { return };
            if ev.button() != 0 {
                return;
            }
            let port = closest(ev.target(), ".node-port");
            let node_el = closest(ev.target(), ".node");
            let edge_el = closest(ev.target(), ".edge");
            let (cx, cy) = (ev.client_x() as f64, ev.client_y() as f64);
            if let (Some(_), Some(n)) = (&port, &node_el) {
                let from = data(n, "id").unwrap_or_default();
                let (x, y) = v.to_world(cx, cy);
                let mut s = v.s.borrow_mut();
                s.linking = Some((from.clone(), x, y));
                s.drag = Some(Drag::Link { from });
                v.render_edges(&mut s);
            } else if let Some(n) = &node_el {
                let id = data(n, "id").unwrap_or_default();
                let mut s = v.s.borrow_mut();
                let Some(node) = s.g.nodes.get(&id) else { return };
                let (ox, oy) = (node.x, node.y);
                s.drag = Some(Drag::Node { id, sx: cx, sy: cy, ox, oy, moved: false });
                let _ = n.class_list().add_1("dragging");
            } else if let Some(e) = &edge_el {
                {
                    let mut s = v.s.borrow_mut();
                    s.selected = None;
                    s.selected_edge = data(e, "id");
                    v.update_selection(&s);
                }
                v.emit(GraphEvent::Select(None));
                return;
            } else {
                let mut s = v.s.borrow_mut();
                let view = s.g.view;
                s.drag = Some(Drag::Pan { sx: cx, sy: cy, ox: view.x, oy: view.y, moved: false });
                let _ = v.el.canvas.class_list().add_1("panning");
            }
            let _ = v.el.canvas.set_pointer_capture(ev.pointer_id());
            ev.prevent_default();
        });

        let me = Rc::downgrade(self);
        on(&c, "pointermove", move |ev: PointerEvent| {
            let Some(v) = me.upgrade() else { return };
            let (cx, cy) = (ev.client_x() as f64, ev.client_y() as f64);
            let world = v.to_world(cx, cy);
            let mut guard = v.s.borrow_mut();
            let s = &mut *guard;
            let k = s.g.view.k;
            match &mut s.drag {
                None => {}
                Some(Drag::Pan { sx, sy, ox, oy, moved }) => {
                    let (dx, dy) = (cx - *sx, cy - *sy);
                    *moved |= dx.abs() + dy.abs() > 3.0;
                    s.g.view.x = *ox + dx;
                    s.g.view.y = *oy + dy;
                    drop(guard);
                    v.apply_view();
                }
                Some(Drag::Node { id, sx, sy, ox, oy, moved }) => {
                    let (dx, dy) = (cx - *sx, cy - *sy);
                    *moved |= dx.abs() + dy.abs() > 3.0;
                    let (id, x, y, moved) = (id.clone(), (*ox + dx / k).round(), (*oy + dy / k).round(), *moved);
                    if let Some(n) = s.g.nodes.get_mut(&id) {
                        n.x = x;
                        n.y = y;
                    }
                    let start = moved && s.dragging.as_deref() != Some(id.as_str());
                    if start {
                        // neighbours reflow live, gently
                        s.dragging = Some(id.clone());
                        s.sim.target = 0.18;
                    }
                    if let Some(el) = s.els.get(&id) {
                        let _ = el.style().set_property("transform", &format!("translate({x}px, {y}px)"));
                    }
                    v.render_edges(s);
                    drop(guard);
                    if start {
                        v.kick(0.18);
                    }
                }
                Some(Drag::Link { from }) => {
                    let from = from.clone();
                    s.linking = Some((from.clone(), world.0, world.1));
                    drop(guard);
                    if let Some(t) = dom::query::<Element>(&v.el.edges, ".temp") {
                        let _ = t.set_attribute("x2", &num(world.0));
                        let _ = t.set_attribute("y2", &num(world.1));
                    }
                    let over = dom::document()
                        .element_from_point(cx as f32, cy as f32)
                        .and_then(|e| e.closest(".node").ok().flatten());
                    for el in dom::query_all(&v.el.nodes, ".link-target") {
                        let _ = el.class_list().remove_1("link-target");
                    }
                    if let Some(o) = over.filter(|o| data(o, "id").as_deref() != Some(from.as_str())) {
                        let _ = o.class_list().add_1("link-target");
                    }
                }
            }
        });

        let me = Rc::downgrade(self);
        let end = move |ev: PointerEvent| {
            let Some(v) = me.upgrade() else { return };
            let Some(d) = v.s.borrow_mut().drag.take() else { return };
            let _ = v.el.canvas.class_list().remove_1("panning");
            match d {
                Drag::Node { id, moved, .. } => {
                    {
                        let mut s = v.s.borrow_mut();
                        if let Some(el) = s.els.get(&id) {
                            let _ = el.class_list().remove_1("dragging");
                        }
                        s.dragging = None;
                        s.sim.target = 0.0;
                    }
                    if !moved {
                        v.select(Some(&id));
                    } else {
                        v.change();
                        v.kick(0.5);
                    }
                }
                Drag::Pan { moved, .. } => {
                    if !moved {
                        v.select(None);
                    } else {
                        v.change_later();
                    }
                }
                Drag::Link { from } => {
                    v.s.borrow_mut().linking = None;
                    let over = dom::document()
                        .element_from_point(ev.client_x() as f32, ev.client_y() as f32)
                        .and_then(|e| e.closest(".node").ok().flatten());
                    for el in dom::query_all(&v.el.nodes, ".link-target") {
                        let _ = el.class_list().remove_1("link-target");
                    }
                    if let Some(to) = over.and_then(|o| data(&o, "id")).filter(|to| *to != from) {
                        v.add_edge(&from, &to);
                        v.select(Some(&to));
                    }
                    v.render_edges(&mut v.s.borrow_mut());
                }
            }
        };
        let end2 = end.clone();
        on(&c, "pointerup", end);
        on(&c, "pointercancel", end2);

        // Scroll pans, ⌘/Ctrl+scroll (and trackpad pinch) zooms.
        let me = Rc::downgrade(self);
        on_active(&c, "gesturestart", move |ev: web_sys::Event| {
            ev.prevent_default();
            if let Some(v) = me.upgrade() {
                v.s.borrow_mut().pinch = 1.0;
            }
        });
        let me = Rc::downgrade(self);
        on_active(&c, "gesturechange", move |ev: web_sys::Event| {
            ev.prevent_default();
            let Some(v) = me.upgrade() else { return };
            let get = |k: &str| js_sys::Reflect::get(&ev, &k.into()).ok().and_then(|x| x.as_f64()).unwrap_or(0.0);
            let (scale, x, y) = (get("scale"), get("clientX"), get("clientY"));
            let pinch = v.s.borrow().pinch;
            if pinch > 0.0 && scale > 0.0 {
                let r = v.el.canvas.get_bounding_client_rect();
                v.zoom_at(x - r.left(), y - r.top(), scale / pinch);
                v.s.borrow_mut().pinch = scale;
            }
        });
        let me = Rc::downgrade(self);
        on_active(&c, "gestureend", move |ev: web_sys::Event| {
            ev.prevent_default();
            if let Some(v) = me.upgrade() {
                v.s.borrow_mut().pinch = 0.0;
                v.change_later();
            }
        });
        let me = Rc::downgrade(self);
        on_active(&c, "wheel", move |ev: WheelEvent| {
            ev.prevent_default();
            let Some(v) = me.upgrade() else { return };
            if v.s.borrow().pinch > 0.0 && ev.ctrl_key() {
                return;
            }
            let s = if ev.delta_mode() == 1 { 16.0 } else { 1.0 };
            if ev.ctrl_key() || ev.meta_key() {
                let r = v.el.canvas.get_bounding_client_rect();
                v.zoom_at(
                    ev.client_x() as f64 - r.left(),
                    ev.client_y() as f64 - r.top(),
                    (-ev.delta_y() * s * 0.01).exp(),
                );
            } else {
                {
                    let mut st = v.s.borrow_mut();
                    st.g.view.x -= ev.delta_x() * s;
                    st.g.view.y -= ev.delta_y() * s;
                }
                v.apply_view();
            }
            v.change_later();
        });

        let me = Rc::downgrade(self);
        on(&c, "dblclick", move |ev: MouseEvent| {
            let Some(v) = me.upgrade() else { return };
            if let Some(n) = closest(ev.target(), ".node") {
                v.emit(GraphEvent::Edit(data(&n, "id").unwrap_or_default()));
                return;
            }
            if closest(ev.target(), ".edge").is_some() {
                return;
            }
            let (x, y) = v.to_world(ev.client_x() as f64, ev.client_y() as f64);
            let node = v.new_node((x - NODE_W / 2.0).round(), (y - 30.0).round());
            let id = v.add_node(node);
            v.select(Some(&id));
            v.flash(&id);
            v.emit(GraphEvent::Edit(id));
        });

        let me = Rc::downgrade(self);
        on(&dom::document(), "keydown", move |ev: web_sys::KeyboardEvent| {
            let Some(v) = me.upgrade() else { return };
            if closest(ev.target(), "input, textarea, select, [contenteditable], dialog").is_some() {
                return;
            }
            match ev.key().as_str() {
                "Delete" | "Backspace" => {
                    let any = {
                        let s = v.s.borrow();
                        s.selected.is_some() || s.selected_edge.is_some()
                    };
                    if any {
                        ev.prevent_default();
                        v.delete_selected();
                    }
                }
                "Escape" => v.select(None),
                _ => {}
            }
        });
    }
}
