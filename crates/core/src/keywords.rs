//! Local keyword extraction and keyword-based auto-linking between notes (keywords.js).
//!
//! Every note carries `keywords` (display strings). They come from the model when a note is extracted from a
//! conversation, from the user when edited in the inspector, and otherwise from this module's TF-IDF pass over the
//! note's own title and body (so a word that appears in every note of the map is never "important"). Two notes that
//! share rare keywords get a dashed link labelled with those keywords.

use crate::graph::{Graph, pair_key};
use crate::model::{Edge, Node};
use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

const STOP_WORDS: &str = "a an the and or but if then else of to in on at by for with from as is are was were be been
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
become becomes becoming remain remains remaining consider considers considering";

static STOP: LazyLock<HashSet<&'static str>> = LazyLock::new(|| STOP_WORDS.split_whitespace().collect());
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{L}[\p{L}\p{N}'’-]*").unwrap());
static SEGMENT_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[.,;:!?()\[\]{}"“”—–\n]+|\s[-–]\s"#).unwrap());
static PLURAL_ES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(s|x|z|ch|sh)es$").unwrap());

/// Lower-case words, with surrounding apostrophes and hyphens removed.
pub fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    TOKEN
        .find_iter(&lower)
        .map(|m| m.as_str().trim_matches(['\'', '’', '-']).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Phrase segments, so word pairs never straddle punctuation.
fn segments(text: &str) -> impl Iterator<Item = &str> {
    SEGMENT_BREAK.split(text).filter(|s| !s.trim().is_empty())
}

/// A light English stemmer: plural and -ing/-ed stripping, enough to match "hashes"/"hashing"/"hash".
pub fn stem(w: &str) -> String {
    let len = w.chars().count();
    let drop = |w: &str, n: usize| w.chars().take(w.chars().count() - n).collect::<String>();
    let mut w = if len > 4 && w.ends_with("ies") {
        drop(w, 3) + "y"
    } else if len > 4 && PLURAL_ES.is_match(w) {
        drop(w, 2)
    } else if len > 3 && w.ends_with('s') && !w.ends_with("ss") {
        drop(w, 1)
    } else {
        w.to_string()
    };
    let len = w.chars().count();
    if len > 6 && w.ends_with("ing") {
        w = drop(&w, 3);
    } else if len > 5 && w.ends_with("ed") {
        w = drop(&w, 2);
    }
    w
}

fn usable(t: &str) -> bool {
    t.chars().count() >= 3
        && !t.starts_with(|c: char| c.is_ascii_digit())
        && !STOP.contains(t)
        && !STOP.contains(stem(t).as_str())
}

/// The stems of a text's meaningful words (no stop words, numbers or very short words), to compare texts by.
pub fn word_stems(text: &str) -> HashSet<String> {
    tokenize(text).iter().filter(|t| usable(t)).map(|t| stem(t)).collect()
}

/// The canonical key of a keyword or phrase, so "Hash Sets" and "hash set" match.
pub fn key_of(display: &str) -> String {
    tokenize(display).iter().filter(|t| usable(t)).map(|t| stem(t)).collect::<Vec<_>>().join(" ")
}

/// A candidate term of a note: how often it occurs, how it is shown, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub count: u32,
    pub display: String,
    pub in_title: bool,
    pub bigram: bool,
}

/// A note's candidate terms (words and adjacent word pairs), keyed by stemmed key, in first-seen order.
pub fn terms_of(node: &Node) -> IndexMap<String, Term> {
    let mut terms: IndexMap<String, Term> = IndexMap::new();
    let mut add = |key: String, display: String, in_title: bool, bigram: bool| {
        let t = terms.entry(key).or_insert(Term { count: 0, display, in_title: false, bigram });
        t.count += 1;
        if in_title {
            t.in_title = true;
        }
    };
    for (text, in_title) in [(node.title.as_str(), true), (node.body.as_str(), false)] {
        for seg in segments(text) {
            let toks = tokenize(seg);
            let ok: Vec<bool> = toks.iter().map(|t| usable(t)).collect();
            for i in 0..toks.len() {
                if !ok[i] {
                    continue;
                }
                add(stem(&toks[i]), toks[i].clone(), in_title, false);
                if ok.get(i + 1) == Some(&true) {
                    add(
                        format!("{} {}", stem(&toks[i]), stem(&toks[i + 1])),
                        format!("{} {}", toks[i], toks[i + 1]),
                        in_title,
                        true,
                    );
                }
            }
        }
    }
    terms
}

/// Keywords for every note by TF-IDF over the whole map: model-supplied keywords (`kw_model`) first, then local
/// ones up to `per`; a phrase supersedes its own words ("hash functions" absorbs "hash" and "functions"). Notes
/// with manual keywords are left out of the result but still count for word rarity.
pub fn compute_auto_keywords(nodes: &[&Node], per: usize) -> IndexMap<String, Vec<String>> {
    let docs: Vec<IndexMap<String, Term>> = nodes.iter().map(|n| terms_of(n)).collect();
    let n_docs = nodes.len() as f64;
    let mut df: IndexMap<&str, f64> = IndexMap::new();
    for d in &docs {
        for k in d.keys() {
            *df.entry(k.as_str()).or_default() += 1.0;
        }
    }
    let idf = |k: &str| ((n_docs.max(6.0) + 1.0) / (df.get(k).copied().unwrap_or(0.0) + 0.5)).ln();
    let mut out = IndexMap::new();
    for (n, doc) in nodes.iter().zip(&docs) {
        if n.is_manual() {
            continue;
        }
        // a phrase only counts if it recurs or sits in the title; a one-off word pair is usually noise. A word used
        // once in the body is weak evidence, however rare it is across the map.
        let mut scored: Vec<(&str, &str, f64)> = doc
            .iter()
            .filter(|(_, t)| !t.bigram || t.count >= 2 || t.in_title)
            .map(|(k, t)| {
                let score = (t.count as f64 + if t.in_title { 2.0 } else { 0.0 })
                    * if t.bigram { 1.3 } else { 1.0 }
                    * if t.count == 1 && !t.in_title { 0.5 } else { 1.0 }
                    * idf(k);
                (k.as_str(), t.display.as_str(), score)
            })
            .filter(|x| x.2 > 0.35)
            .collect();
        scored.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut chosen: Vec<(String, String)> = Vec::new();
        let take = |chosen: &mut Vec<(String, String)>, k: String, display: &str| {
            if k.is_empty() || chosen.iter().any(|c| c.0 == k) {
                return;
            }
            if k.contains(' ') {
                let parts: Vec<&str> = k.split(' ').collect();
                chosen.retain(|c| !parts.contains(&c.0.as_str()));
            } else if chosen.iter().any(|c| c.0.contains(' ') && c.0.split(' ').any(|p| p == k)) {
                return;
            }
            chosen.push((k, display.to_string()));
        };
        for kw in n.kw_model.iter().flatten() {
            take(&mut chosen, key_of(kw), kw);
        }
        for (k, display, _) in &scored {
            if chosen.len() >= per {
                break;
            }
            take(&mut chosen, k.to_string(), display);
        }
        out.insert(n.id.clone(), chosen.into_iter().map(|c| c.1).collect());
    }
    out
}

/// Among shared keys, a phrase supersedes its own words so "bloom filter" isn't counted three times.
fn drop_shared_parts(keys: &[String]) -> Vec<&String> {
    let parts: HashSet<&str> = keys.iter().filter(|k| k.contains(' ')).flat_map(|k| k.split(' ')).collect();
    keys.iter().filter(|k| k.contains(' ') || !parts.contains(k.as_str())).collect()
}

/// What a keyword-linking run changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkChanges {
    pub added: usize,
    pub removed: usize,
    pub updated: usize,
}

impl LinkChanges {
    pub fn any(&self) -> bool {
        self.added + self.removed + self.updated > 0
    }
}

/// Creates, relabels and removes keyword links. A pair of notes is linked when the summed rarity (idf) of their
/// shared keywords reaches `threshold`. Manual links, and pairs the user unlinked, are left alone.
pub fn link_by_keywords(g: &mut Graph, threshold: f64, new_id: &mut dyn FnMut() -> String) -> LinkChanges {
    let n_docs = g.nodes.len() as f64;
    let mut keys_of: IndexMap<String, IndexMap<String, String>> = IndexMap::new(); // id -> key -> display
    for n in g.nodes.values() {
        let mut m = IndexMap::new();
        for kw in n.keywords() {
            let k = key_of(kw);
            if !k.is_empty() && !m.contains_key(&k) {
                m.insert(k, kw.clone());
            }
        }
        keys_of.insert(n.id.clone(), m);
    }
    let mut by_key: IndexMap<&str, Vec<&str>> = IndexMap::new();
    for (id, m) in &keys_of {
        for k in m.keys() {
            by_key.entry(k.as_str()).or_default().push(id.as_str());
        }
    }
    let idf = |k: &str| ((n_docs.max(6.0) + 1.0) / (by_key.get(k).map_or(0.0, |v| v.len() as f64) + 0.5)).ln();

    // candidate pairs: notes sharing at least one keyword that isn't near-universal
    let common = 3f64.max((n_docs * 0.5).ceil()) as usize;
    let mut pairs: IndexMap<String, (&str, &str, Vec<String>)> = IndexMap::new();
    for (k, ids) in &by_key {
        if ids.len() < 2 || ids.len() > common {
            continue;
        }
        for i in 0..ids.len() {
            for j in i + 1..ids.len() {
                pairs.entry(pair_key(ids[i], ids[j])).or_insert((ids[i], ids[j], Vec::new())).2.push(k.to_string());
            }
        }
    }

    let mut existing_kw: IndexMap<String, String> = IndexMap::new(); // pair key -> edge id
    for e in g.edges.values().filter(|e| e.is_keyword()) {
        existing_kw.insert(pair_key(&e.from, &e.to), e.id.clone());
    }

    let mut changes = LinkChanges::default();
    let mut keep: IndexSet<String> = IndexSet::new();
    let mut to_add: Vec<Edge> = Vec::new();
    let mut relabel: Vec<(String, String)> = Vec::new();
    for (pk, (a, b, shared)) in &pairs {
        if g.kw_ignore.contains(pk) {
            continue;
        }
        let mut shared: Vec<(&String, f64)> = drop_shared_parts(shared).into_iter().map(|k| (k, idf(k))).collect();
        shared.sort_by(|x, y| y.1.total_cmp(&x.1));
        let score: f64 = shared.iter().fold(0.0, |s, x| s + x.1);
        if score < threshold {
            continue;
        }
        let label = shared.iter().take(2).map(|x| keys_of[*a][x.0].as_str()).collect::<Vec<_>>().join(", ");
        if g.has_edge(a, b).is_some_and(|e| !e.is_keyword()) {
            continue;
        }
        keep.insert(pk.clone());
        match existing_kw.get(pk) {
            Some(id) => {
                if g.edges.get(id).is_some_and(|e| e.label != label) {
                    relabel.push((id.clone(), label));
                }
            }
            None => to_add.push(Edge {
                id: String::new(),
                from: a.to_string(),
                to: b.to_string(),
                label,
                auto: Some("keyword".into()),
                ..Edge::default()
            }),
        }
    }
    for (id, label) in relabel {
        if let Some(e) = g.edges.get_mut(&id) {
            e.label = label;
            changes.updated += 1;
        }
    }
    for mut e in to_add {
        e.id = new_id();
        if g.add_edge(e).is_some_and(|(_, new)| new) {
            changes.added += 1;
        }
    }
    for (pk, id) in &existing_kw {
        if !keep.contains(pk) && g.remove_edge(id) {
            changes.removed += 1;
        }
    }
    changes
}

/// The text a note's computed keywords depend on.
pub fn kw_text_of(n: &Node) -> String {
    format!("{}\u{1}{}\u{1}{}", n.title, n.body, n.kw_model.as_deref().unwrap_or(&[]).join(","))
}

/// What `refresh` did: how many notes got new keywords, and the keyword-link changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Refresh {
    pub stale: usize,
    pub links: LinkChanges,
}

impl Refresh {
    pub fn changed(&self) -> bool {
        self.stale > 0 || self.links.any()
    }
}

/// Recomputes keywords for notes whose text changed (all notes with `force`), then runs keyword linking if it is
/// on (app.js `refreshKeywords`).
pub fn refresh(g: &mut Graph, force: bool, new_id: &mut dyn FnMut() -> String) -> Refresh {
    let stale: Vec<String> = g
        .nodes
        .values()
        .filter(|n| {
            !n.is_manual() && (force || n.kw_text.as_deref() != Some(kw_text_of(n).as_str()) || n.keywords.is_none())
        })
        .map(|n| n.id.clone())
        .collect();
    if !stale.is_empty() || force {
        let computed = compute_auto_keywords(&g.nodes.values().collect::<Vec<_>>(), 6);
        for id in &stale {
            if let Some(n) = g.nodes.get_mut(id) {
                n.keywords = Some(computed.get(id).cloned().unwrap_or_default());
                n.kw_text = Some(kw_text_of(n));
            }
        }
    }
    let links = if g.keyword_links.enabled {
        let threshold = g.keyword_links.threshold;
        link_by_keywords(g, threshold, new_id)
    } else {
        LinkChanges::default()
    };
    Refresh { stale: stale.len(), links }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, title: &str, body: &str) -> Node {
        Node { id: id.into(), title: title.into(), body: body.into(), ..Node::default() }
    }

    #[test]
    fn stems_plurals_and_endings() {
        assert_eq!(stem("hashes"), "hash");
        assert_eq!(stem("queries"), "query");
        assert_eq!(stem("hashing"), "hash");
        assert_eq!(stem("indexed"), "index");
        assert_eq!(stem("class"), "class");
        assert_eq!(key_of("Hash Sets"), key_of("hash set"));
    }

    #[test]
    fn tokenizes_unicode_words() {
        assert_eq!(tokenize("Größe -- naïve ’quoted’ 42abc x-ray"), vec!["größe", "naïve", "quoted", "abc", "x-ray"]);
    }

    #[test]
    fn links_notes_that_share_rare_keywords() {
        let mut g = Graph::default();
        for (id, t, b) in [
            ("a", "Bloom filters", "A bloom filter answers set membership."),
            ("b", "Bloom filter sizing", "Sizing a bloom filter."),
            ("c", "Tea", "Green tea."),
            ("d", "Coffee", "Espresso."),
        ] {
            g.add_node(note(id, t, b));
        }
        let mut n = 0;
        let r = refresh(&mut g, false, &mut || {
            n += 1;
            format!("k{n}")
        });
        assert_eq!(r.stale, 4);
        assert_eq!(r.links.added, 1);
        let e = g.edges.values().next().unwrap();
        assert!(e.is_keyword() && e.label.contains("bloom"));
        let id = e.id.clone();
        // nothing changed, so nothing to do
        assert_eq!(refresh(&mut g, false, &mut || unreachable!()), Refresh::default());
        // an unlinked pair stays unlinked
        g.kw_ignore.insert(pair_key("a", "b"));
        let r = refresh(&mut g, true, &mut || unreachable!());
        assert_eq!(r.links.removed, 1);
        assert!(!g.edges.contains_key(&id));
    }
}
