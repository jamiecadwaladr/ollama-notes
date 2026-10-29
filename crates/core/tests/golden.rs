//! The Rust port against the original JS: fixtures/golden.json holds what web/*.js (master branch) computed for the
//! same inputs. Regenerate it with tools/golden/generate.sh.

use notes_core::graph::{DEFAULT_H, Graph};
use notes_core::keywords;
use notes_core::layout::{self, Measure, Sim};
use notes_core::model::{Edge, Node, Physics, View};
use notes_core::{host, ollama, text};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

static GOLDEN: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("fixtures/golden.json")).expect("golden.json parses"));

fn cases(name: &str) -> &'static Vec<Value> {
    GOLDEN[name].as_array().unwrap_or_else(|| panic!("no {name} cases"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn strings(v: &Value) -> Vec<String> {
    serde_json::from_value(v.clone()).unwrap()
}

fn nodes(v: &Value) -> Vec<Node> {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn tokenize_stem_and_keys() {
    for c in cases("tokenize") {
        assert_eq!(keywords::tokenize(s(&c[0])), strings(&c[1]), "tokenize {:?}", c[0]);
    }
    for c in cases("stem") {
        assert_eq!(keywords::stem(s(&c[0])), s(&c[1]), "stem {:?}", c[0]);
    }
    for c in cases("keyOf") {
        assert_eq!(keywords::key_of(s(&c[0])), s(&c[1]), "keyOf {:?}", c[0]);
    }
}

#[test]
fn candidate_terms() {
    for c in cases("terms") {
        let node: Node = serde_json::from_value(c["node"].clone()).unwrap();
        let got: Vec<Value> = keywords::terms_of(&node)
            .into_iter()
            .map(|(k, t)| {
                serde_json::json!([k, { "count": t.count, "display": t.display, "inTitle": t.in_title, "bigram": t.bigram }])
            })
            .collect();
        assert_eq!(Value::Array(got), c["terms"], "terms of {}", node.title);
    }
}

#[test]
fn auto_keywords() {
    for name in ["autoKeywords", "autoKeywordsSmall"] {
        let c = &GOLDEN[name];
        let ns = nodes(&c["nodes"]);
        let got = keywords::compute_auto_keywords(&ns.iter().collect::<Vec<_>>(), 6);
        let want: Vec<(String, Vec<String>)> = serde_json::from_value(c["out"].clone()).unwrap();
        assert_eq!(got.into_iter().collect::<Vec<_>>(), want, "{name}");
    }
}

#[test]
fn keyword_links() {
    for c in cases("linkByKeywords") {
        let mut g = Graph::default();
        for n in nodes(&c["nodes"]) {
            g.add_node(n);
        }
        for e in serde_json::from_value::<Vec<Edge>>(c["edges"].clone()).unwrap() {
            g.edges.insert(e.id.clone(), e);
        }
        g.kw_ignore = strings(&c["kwIgnore"]).into_iter().collect();
        let mut next = 0;
        let r = keywords::link_by_keywords(&mut g, c["threshold"].as_f64().unwrap(), &mut || {
            next += 1;
            format!("k{next}")
        });
        let want = &c["result"];
        assert_eq!(
            (r.added, r.removed, r.updated),
            (
                want["added"].as_u64().unwrap() as usize,
                want["removed"].as_u64().unwrap() as usize,
                want["updated"].as_u64().unwrap() as usize
            ),
            "threshold {}",
            c["threshold"]
        );
        let got: Vec<Value> = g.edges.values().map(|e| serde_json::json!([e.from, e.to, e.label, e.auto])).collect();
        assert_eq!(Value::Array(got), c["edgesAfter"], "edges after linking at {}", c["threshold"]);
    }
}

#[test]
fn chunks_titles_and_hosts() {
    for c in cases("chunkText") {
        let size = c["size"].as_u64().unwrap() as usize;
        assert_eq!(text::chunk_text(s(&c["text"]), size), strings(&c["out"]), "chunk {size} of {:.40?}", s(&c["text"]));
    }
    for c in cases("titleFromFilename") {
        assert_eq!(text::title_from_filename(s(&c[0])), s(&c[1]), "title of {:?}", c[0]);
    }
    for c in cases("normalizeHost") {
        assert_eq!(host::normalize_host(s(&c[0])), s(&c[1]), "host {:?}", c[0]);
    }
}

#[test]
fn loose_json() {
    for c in cases("parseLooseJSON") {
        let got = ollama::parse_loose_json(s(&c[0]));
        match c[1].get("error") {
            Some(e) if c[1].as_object().is_some_and(|o| o.len() == 1) => {
                assert_eq!(got.map_err(|e| e.to_string()), Err(s(e).to_string()), "{:?}", c[0])
            }
            _ => assert_eq!(got.as_ref().ok(), Some(&c[1]), "{:?}", c[0]),
        }
    }
}

/// A map and its measurements from a physics case.
fn sim_graph(c: &Value) -> (Graph, HashMap<String, f64>, Option<String>) {
    let mut g = Graph::default();
    let mut heights = HashMap::new();
    let mut dragging = None;
    for n in c["nodes"].as_array().unwrap() {
        let id = s(&n["id"]).to_string();
        if let Some(h) = n["h"].as_f64() {
            heights.insert(id.clone(), h);
        }
        if n["drag"].as_bool() == Some(true) {
            dragging = Some(id.clone());
        }
        g.add_node(Node {
            id,
            x: n["x"].as_f64().unwrap(),
            y: n["y"].as_f64().unwrap(),
            locked: n["locked"].as_bool(),
            ..Node::default()
        });
    }
    for (i, e) in c["edges"].as_array().unwrap().iter().enumerate() {
        let id = format!("e{i}");
        g.edges.insert(
            id.clone(),
            Edge {
                id,
                from: s(&e["from"]).into(),
                to: s(&e["to"]).into(),
                label: s(&e["label"]).into(),
                ..Edge::default()
            },
        );
    }
    let p = &c["physics"];
    let f = |k: &str| p.get(k).and_then(Value::as_f64).unwrap_or(1.0);
    g.physics =
        Physics { tension: f("tension"), gravity: f("gravity"), repulsion: f("repulsion"), ..Physics::default() };
    (g, heights, dragging)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * (1.0 + b.abs())
}

#[test]
fn force_layout_matches_step_for_step() {
    for c in cases("physics") {
        let name = s(&c["name"]);
        let (mut g, heights, dragging) = sim_graph(c);
        let labels: HashMap<String, f64> =
            serde_json::from_value(c.get("labelW").cloned().unwrap_or(Value::Null)).unwrap_or_default();
        let height = |id: &str| heights.get(id).copied().unwrap_or(DEFAULT_H);
        let label_width = |l: &str| labels.get(l).copied();
        let measure = Measure { height: &height, label_width: &label_width };
        let mut sim =
            Sim { alpha: c["alpha"].as_f64().unwrap(), target: c["target"].as_f64().unwrap(), ..Sim::default() };
        let mut vel = HashMap::new();
        let mut rng = fastrand::Rng::with_seed(0);
        let snapshots = c["snapshots"].as_array().unwrap();
        let mut next = snapshots.iter().peekable();
        for t in 1..=200 {
            layout::tick(&mut sim, &mut g, &mut vel, &measure, dragging.as_deref(), &mut rng);
            if let Some(snap) = next.next_if(|s| s["ticks"].as_u64() == Some(t)) {
                assert!(close(sim.alpha, snap["alpha"].as_f64().unwrap()), "{name} alpha at {t}");
                assert!(close(sim.corrected, snap["corrected"].as_f64().unwrap()), "{name} corrected at {t}");
                for (n, want) in g.nodes.values().zip(snap["nodes"].as_array().unwrap()) {
                    let (x, y) = (want[0].as_f64().unwrap(), want[1].as_f64().unwrap());
                    assert!(
                        close(n.x, x) && close(n.y, y),
                        "{name} {} at tick {t}: ({}, {}) vs ({x}, {y})",
                        n.id,
                        n.x,
                        n.y
                    );
                }
            }
        }
        let (mut g, ..) = sim_graph(c);
        layout::settle(&mut g, &mut HashMap::new(), &measure, &mut rng);
        let got: Vec<(f64, f64)> = g.nodes.values().map(|n| (n.x, n.y)).collect();
        let want: Vec<(f64, f64)> = serde_json::from_value(c["settled"].clone()).unwrap();
        assert_eq!(got, want, "{name} settled");
    }
}

#[test]
fn placement_of_new_notes() {
    for c in cases("placeNear") {
        let mut g = Graph::default();
        let mut heights = HashMap::new();
        for n in c["nodes"].as_array().unwrap() {
            let id = s(&n["id"]).to_string();
            if let Some(h) = n["h"].as_f64() {
                heights.insert(id.clone(), h);
            }
            g.add_node(Node { id, x: n["x"].as_f64().unwrap(), y: n["y"].as_f64().unwrap(), ..Node::default() });
        }
        let view: View = serde_json::from_value(c["view"].clone()).unwrap();
        let canvas = &c["canvas"];
        let (w, h, pad) = (
            canvas["width"].as_f64().unwrap(),
            canvas["height"].as_f64().unwrap(),
            canvas["padRight"].as_f64().unwrap(),
        );
        let center = (((w - pad) / 2.0 - view.x) / view.k, (h / 2.0 - view.y) / view.k);
        assert!(
            close(center.0, c["viewCenter"]["x"].as_f64().unwrap())
                && close(center.1, c["viewCenter"]["y"].as_f64().unwrap())
        );
        let height = |id: &str| heights.get(id).copied().unwrap_or(DEFAULT_H);
        let got = g.place_near(c["anchor"].as_str(), center, &height, &mut fastrand::Rng::with_seed(0));
        assert_eq!(got, (c["out"]["x"].as_f64().unwrap(), c["out"]["y"].as_f64().unwrap()), "anchor {:?}", c["anchor"]);
    }
}
