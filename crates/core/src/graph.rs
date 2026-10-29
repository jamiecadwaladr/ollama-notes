//! The notes map as data: notes and links in insertion order (as the JS `Map`s were), lookups, and where to put a
//! new note. Drawing it is the page's job (ui/src/graph_view.rs).

use crate::model::{Edge, Extra, GraphData, KeywordLinks, Node, Physics, View};
use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use std::sync::LazyLock;

/// A note card's width; its height comes from its content.
pub const NODE_W: f64 = 220.0;
/// The height assumed for a card that hasn't been drawn yet.
pub const DEFAULT_H: f64 = 110.0;

static NON_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").unwrap());

/// Lower-case letters and digits separated by single spaces, for matching titles and filtering.
pub fn norm(s: &str) -> String {
    NON_WORD.replace_all(&s.to_lowercase(), " ").trim().to_string()
}

/// The order-independent key of a pair of notes ("a|b").
pub fn pair_key(a: &str, b: &str) -> String {
    if a < b { format!("{a}|{b}") } else { format!("{b}|{a}") }
}

/// A new id in the JS format: 8 random base-36 characters, then the time in base 36.
pub fn new_id(rng: &mut fastrand::Rng, now_ms: f64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut id: String = (0..8).map(|_| DIGITS[rng.usize(..36)] as char).collect();
    let mut t = now_ms.max(0.0) as u64;
    let mut time = Vec::new();
    loop {
        time.push(DIGITS[(t % 36) as usize]);
        t /= 36;
        if t == 0 {
            break;
        }
    }
    id.extend(time.iter().rev().map(|&b| b as char));
    id
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    pub nodes: IndexMap<String, Node>,
    pub edges: IndexMap<String, Edge>,
    pub view: View,
    pub physics: Physics,
    pub keyword_links: KeywordLinks,
    /// Pairs ("a|b") the user unlinked; keyword linking leaves them alone.
    pub kw_ignore: IndexSet<String>,
    pub extra: Extra,
}

/// A card's rectangle in map coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Graph {
    /// The map from saved data; links to missing notes are dropped and an unusable view is reset.
    pub fn from_data(data: GraphData) -> Graph {
        let nodes: IndexMap<String, Node> = data.nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        let edges = data
            .edges
            .into_iter()
            .filter(|e| nodes.contains_key(&e.from) && nodes.contains_key(&e.to))
            .map(|e| (e.id.clone(), e))
            .collect();
        let view = if data.view.k.is_finite() && data.view.k > 0.0 { data.view } else { View::default() };
        Graph {
            nodes,
            edges,
            view,
            physics: data.physics,
            keyword_links: data.keyword_links,
            kw_ignore: data.kw_ignore.into_iter().collect(),
            extra: data.extra,
        }
    }

    pub fn to_data(&self) -> GraphData {
        GraphData {
            nodes: self.nodes.values().cloned().collect(),
            edges: self.edges.values().cloned().collect(),
            view: self.view,
            physics: self.physics.clone(),
            keyword_links: self.keyword_links.clone(),
            kw_ignore: self.kw_ignore.iter().cloned().collect(),
            extra: self.extra.clone(),
        }
    }

    /// Notes, links and unlinked pairs removed; the view and settings stay.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.edges.clear();
        self.kw_ignore.clear();
    }

    /// The first note whose title matches, ignoring case and punctuation.
    pub fn find_by_title(&self, title: &str) -> Option<&Node> {
        let t = norm(title);
        if t.is_empty() {
            return None;
        }
        self.nodes.values().find(|n| norm(&n.title) == t)
    }

    pub fn add_node(&mut self, node: Node) -> String {
        let id = node.id.clone();
        self.nodes.insert(id.clone(), node);
        id
    }

    /// Removes a note and its links. Returns whether it existed.
    pub fn remove_node(&mut self, id: &str) -> bool {
        if self.nodes.shift_remove(id).is_none() {
            return false;
        }
        self.edges.retain(|_, e| e.from != id && e.to != id);
        true
    }

    /// The link between two notes, in either direction.
    pub fn has_edge(&self, a: &str, b: &str) -> Option<&Edge> {
        self.edges.values().find(|e| (e.from == a && e.to == b) || (e.from == b && e.to == a))
    }

    /// Links two notes, unless that's a loop, a note is missing, or they're linked already (then that link's id is
    /// returned). Returns the link's id and whether it is new.
    pub fn add_edge(&mut self, edge: Edge) -> Option<(String, bool)> {
        if edge.from == edge.to || !self.nodes.contains_key(&edge.from) || !self.nodes.contains_key(&edge.to) {
            return None;
        }
        if let Some(e) = self.has_edge(&edge.from, &edge.to) {
            return Some((e.id.clone(), false));
        }
        let id = edge.id.clone();
        self.edges.insert(id.clone(), edge);
        Some((id, true))
    }

    pub fn remove_edge(&mut self, id: &str) -> bool {
        self.edges.shift_remove(id).is_some()
    }

    /// Each link of a note with the note at its other end.
    pub fn neighbors(&self, id: &str) -> Vec<(&Edge, &Node)> {
        self.edges
            .values()
            .filter_map(|e| {
                let other = if e.from == id {
                    &e.to
                } else if e.to == id {
                    &e.from
                } else {
                    return None;
                };
                self.nodes.get(other).map(|n| (e, n))
            })
            .collect()
    }

    pub fn degree(&self, id: &str) -> usize {
        self.edges.values().filter(|e| e.from == id || e.to == id).count()
    }

    pub fn rect(&self, n: &Node, height: &dyn Fn(&str) -> f64) -> Rect {
        Rect { x: n.x, y: n.y, w: NODE_W, h: height(&n.id) }
    }

    /// Whether a card at (x, y) would keep `pad` clear of every other card.
    fn free(&self, x: f64, y: f64, height: &dyn Fn(&str) -> f64) -> bool {
        let (w, h, pad) = (NODE_W, DEFAULT_H, 28.0);
        self.nodes.values().all(|n| {
            let r = self.rect(n, height);
            !(x < r.x + r.w + pad && x + w + pad > r.x && y < r.y + r.h + pad && y + h + pad > r.y)
        })
    }

    /// Where a new note goes: the view's centre if that's free, otherwise the first free spot on rings around the
    /// anchor note (or the view centre). The force layout takes it from there.
    pub fn place_near(
        &self,
        anchor: Option<&str>,
        view_center: (f64, f64),
        height: &dyn Fn(&str) -> f64,
        rng: &mut fastrand::Rng,
    ) -> (f64, f64) {
        let (cx, cy) = match anchor.and_then(|id| self.nodes.get(id)) {
            Some(a) => {
                let r = self.rect(a, height);
                (r.x + r.w / 2.0, r.y + r.h / 2.0)
            }
            None => {
                let (cx, cy) = view_center;
                if self.free(cx - NODE_W / 2.0, cy - DEFAULT_H / 2.0, height) {
                    return ((cx - NODE_W / 2.0).round(), (cy - DEFAULT_H / 2.0).round());
                }
                (cx, cy)
            }
        };
        let golden = 2.399963;
        let seed = self.nodes.len() as f64;
        let tau = std::f64::consts::PI * 2.0;
        for ring in 1..14 {
            let radius = 170.0 + ring as f64 * 120.0;
            let steps = 6 + ring * 4;
            let start = (seed * golden) % tau;
            for s in 0..steps {
                let ang = start + s as f64 * (tau / steps as f64);
                let x = cx + ang.cos() * radius - NODE_W / 2.0;
                let y = cy + ang.sin() * radius * 0.7 - DEFAULT_H / 2.0;
                if self.free(x, y, height) {
                    return (x.round(), y.round());
                }
            }
        }
        ((cx + rng.f64() * 1200.0 - 600.0).round(), (cy + rng.f64() * 1200.0 - 600.0).round())
    }

    /// The smallest rectangle around every card, as (x1, y1, x2, y2).
    pub fn bounds(&self, height: &dyn Fn(&str) -> f64) -> Option<(f64, f64, f64, f64)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut b = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for n in self.nodes.values() {
            let r = self.rect(n, height);
            b = (b.0.min(r.x), b.1.min(r.y), b.2.max(r.x + r.w), b.3.max(r.y + r.h));
        }
        Some(b)
    }
}

/// Whether a note matches the filter (already `norm`ed): its title, body or keywords contain it.
pub fn matches(n: &Node, filter: &str) -> bool {
    filter.is_empty() || norm(&format!("{} {} {}", n.title, n.body, n.keywords().join(" "))).contains(filter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, title: &str, x: f64, y: f64) -> Node {
        Node { id: id.into(), title: title.into(), x, y, ..Node::default() }
    }

    fn edge(id: &str, from: &str, to: &str) -> Edge {
        Edge { id: id.into(), from: from.into(), to: to.into(), ..Edge::default() }
    }

    #[test]
    fn finds_titles_loosely() {
        let mut g = Graph::default();
        g.add_node(node("a", "Hash Sets!", 0.0, 0.0));
        assert_eq!(g.find_by_title("hash  sets").map(|n| n.id.as_str()), Some("a"));
        assert!(g.find_by_title("  ").is_none());
    }

    #[test]
    fn edges_are_undirected_and_unique() {
        let mut g = Graph::default();
        g.add_node(node("a", "A", 0.0, 0.0));
        g.add_node(node("b", "B", 0.0, 0.0));
        assert_eq!(g.add_edge(edge("e1", "a", "b")), Some(("e1".into(), true)));
        assert_eq!(g.add_edge(edge("e2", "b", "a")), Some(("e1".into(), false)));
        assert_eq!(g.add_edge(edge("e3", "a", "a")), None);
        assert_eq!(g.add_edge(edge("e4", "a", "zz")), None);
        assert_eq!(g.degree("a"), 1);
        assert_eq!(g.neighbors("b")[0].1.id, "a");
        assert!(g.remove_node("a"));
        assert!(g.edges.is_empty());
    }

    #[test]
    fn loading_drops_dangling_links_and_bad_views() {
        let data = GraphData {
            nodes: vec![node("a", "A", 0.0, 0.0)],
            edges: vec![edge("e", "a", "gone")],
            view: View { x: 1.0, y: 2.0, k: 0.0 },
            ..GraphData::default()
        };
        let g = Graph::from_data(data);
        assert!(g.edges.is_empty());
        assert_eq!(g.view, View::default());
    }

    #[test]
    fn places_new_notes_apart() {
        let mut g = Graph::default();
        let mut rng = fastrand::Rng::with_seed(1);
        let h = |_: &str| DEFAULT_H;
        assert_eq!(g.place_near(None, (500.0, 300.0), &h, &mut rng), (390.0, 245.0));
        g.add_node(node("a", "A", 390.0, 245.0));
        let (x, y) = g.place_near(Some("a"), (500.0, 300.0), &h, &mut rng);
        assert!((x - 390.0).abs() + (y - 245.0).abs() > 200.0);
    }

    #[test]
    fn ids_look_like_the_js_ones() {
        let id = new_id(&mut fastrand::Rng::with_seed(7), 1_761_700_000_000.0);
        assert_eq!(id.len(), 16);
        assert!(id.ends_with("mhbaozr4")); // (1761700000000).toString(36)
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn filter_matches_keywords() {
        let mut n = node("a", "Title", 0.0, 0.0);
        n.keywords = Some(vec!["Bloom filter".into()]);
        assert!(matches(&n, &norm("bloom")));
        assert!(!matches(&n, &norm("hash")));
        assert!(matches(&n, ""));
    }
}
