//! The map's live force layout (graph.js `_tick`). All strengths scale with a note's number of links, its degree:
//! - repulsion: every note pushes every other away; well-connected notes push harder (more mass)
//! - tension: linked notes are joined by springs; stiffness and rest length grow with degree, so hubs pull their
//!   cluster in but keep enough room for their satellites
//! - gravity: notes drift toward the map's centre of mass, hubs strongest, so leaves orbit outside
//! - collision: overlapping cards are separated directly (no bouncing)
//! - labels: cards keep clear of the keyword labels drawn on links
//!
//! The simulation cools down (alpha -> 0) and stops; anything that changes the map re-heats it. The arithmetic
//! follows the JS operation for operation, so a map settles the same way it did there.

use crate::graph::{Graph, NODE_W};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Velocity {
    pub vx: f64,
    pub vy: f64,
}

/// The simulation's temperature: `alpha` eases toward `target` each step.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sim {
    pub alpha: f64,
    pub target: f64,
    /// The largest overlap correction in the last step.
    pub corrected: f64,
    /// Frames since the simulation cooled, while overlaps are still being cleared.
    pub tail: u32,
}

impl Sim {
    /// Re-heat to at least `alpha`.
    pub fn heat(&mut self, alpha: f64) {
        self.alpha = self.alpha.max(alpha);
    }

    /// Book-keeping after an animation frame's step: whether to run another frame. When it returns false the
    /// simulation has settled.
    pub fn next_frame(&mut self) -> bool {
        let hot = self.alpha > 0.004 || self.target > 0.0;
        self.tail = if hot { 0 } else { self.tail + 1 };
        if hot || (self.corrected > 0.4 && self.tail < 240) {
            true
        } else {
            self.alpha = 0.0;
            false
        }
    }
}

/// What the layout needs to know about the drawn map.
pub struct Measure<'a> {
    /// A card's rendered height.
    pub height: &'a dyn Fn(&str) -> f64,
    /// A link label's rendered width, once it has been drawn.
    pub label_width: &'a dyn Fn(&str) -> Option<f64>,
}

struct Body {
    x: f64,
    y: f64,
    cx: f64,
    cy: f64,
    hw: f64,
    hh: f64,
    d: f64,
    m: f64,
    v: Velocity,
    fx: f64,
    fy: f64,
    fixed: bool,
}

/// Mutable references to two different bodies.
fn pair(b: &mut [Body], i: usize, j: usize) -> (&mut Body, &mut Body) {
    debug_assert!(i < j);
    let (lo, hi) = b.split_at_mut(j);
    (&mut lo[i], &mut hi[0])
}

/// Moves a and c apart by a fraction `r` of their overlap (ox, oy) along the axis of least overlap; the lighter
/// note moves more. Unlike a spring force this never overshoots, so nothing bounces or shakes.
fn separate(a: &mut Body, c: &mut Body, ox: f64, oy: f64, r: f64, corrected: &mut f64) {
    if a.fixed && c.fixed {
        return;
    }
    let wa = if a.fixed {
        0.0
    } else if c.fixed {
        1.0
    } else {
        c.m / (a.m + c.m)
    };
    let wc = 1.0 - wa;
    if ox < oy {
        let sgn = if a.cx >= c.cx { 1.0 } else { -1.0 };
        let d = ox * r;
        a.x += sgn * d * wa;
        a.cx += sgn * d * wa;
        c.x -= sgn * d * wc;
        c.cx -= sgn * d * wc;
        *corrected = corrected.max(d);
    } else {
        let sgn = if a.cy >= c.cy { 1.0 } else { -1.0 };
        let d = oy * r;
        a.y += sgn * d * wa;
        a.cy += sgn * d * wa;
        c.y -= sgn * d * wc;
        c.cy -= sgn * d * wc;
        *corrected = corrected.max(d);
    }
}

/// One simulation step. `dragging` is the note under the pointer, held in place like a locked one.
pub fn tick(
    sim: &mut Sim,
    g: &mut Graph,
    vel: &mut HashMap<String, Velocity>,
    measure: &Measure,
    dragging: Option<&str>,
    rng: &mut fastrand::Rng,
) {
    sim.alpha += (sim.target - sim.alpha) * 0.028;
    let alpha = sim.alpha;
    let p = &g.physics;
    let n = g.nodes.len();
    if n < 2 {
        return;
    }

    // per-node scratch: centre, half-size, degree, mass
    let mut deg: HashMap<&str, f64> = HashMap::new();
    for e in g.edges.values() {
        *deg.entry(&e.from).or_default() += 1.0;
        *deg.entry(&e.to).or_default() += 1.0;
    }
    let mut b: Vec<Body> = g
        .nodes
        .values()
        .map(|nd| {
            let h = (measure.height)(&nd.id);
            let d = deg.get(nd.id.as_str()).copied().unwrap_or(0.0);
            Body {
                x: nd.x,
                y: nd.y,
                cx: nd.x + NODE_W / 2.0,
                cy: nd.y + h / 2.0,
                hw: NODE_W / 2.0,
                hh: h / 2.0,
                d,
                m: 1.0 + 4f64.min(0.4 * d),
                v: vel.get(&nd.id).copied().unwrap_or_default(),
                fx: 0.0,
                fy: 0.0,
                fixed: nd.is_locked() || dragging == Some(nd.id.as_str()),
            }
        })
        .collect();
    let idx: HashMap<&str, usize> = g.nodes.keys().enumerate().map(|(i, id)| (id.as_str(), i)).collect();
    let links: Vec<(usize, usize, &str)> = g
        .edges
        .values()
        .filter_map(|e| Some((*idx.get(e.from.as_str())?, *idx.get(e.to.as_str())?, e.label.as_str())))
        .collect();

    // centre of mass (gravity target)
    let (mut cmx, mut cmy, mut mt) = (0.0, 0.0, 0.0);
    for o in &b {
        cmx += o.cx * o.m;
        cmy += o.cy * o.m;
        mt += o.m;
    }
    cmx /= mt;
    cmy /= mt;

    // repulsion, all pairs (notes are wide, so vertical distances count for more)
    let r = 30000.0 * p.repulsion;
    let ys = 1.6;
    for i in 0..n {
        for j in i + 1..n {
            let (a, c) = pair(&mut b, i, j);
            let mut dx = a.cx - c.cx;
            let mut dy = (a.cy - c.cy) * ys;
            let mut d2 = dx * dx + dy * dy;
            if d2 < 1.0 {
                dx = (rng.f64() - 0.5) * 2.0;
                dy = (rng.f64() - 0.5) * 2.0;
                d2 = dx * dx + dy * dy;
            }
            let d = d2.sqrt();
            let f = (r * a.m * c.m) / d2.max(120.0 * 120.0);
            let fx = dx / d * f;
            let fy = dy / d * f;
            a.fx += fx;
            a.fy += fy;
            c.fx -= fx;
            c.fy -= fy;
        }
    }

    // springs along links: tension and rest length scale with the degree of the endpoints
    let t = 0.09 * p.tension;
    for &(ia, ic, _) in &links {
        let (a, c) = (&b[ia], &b[ic]);
        let hi = a.d.max(c.d);
        let rest = 560f64.min(250.0 + 28.0 * hi); // hubs get longer arms
        let k = t * (0.6 + 0.45 * (1.0 + hi).log2()); // ...and pull harder
        let dx = c.cx - a.cx;
        let dy = c.cy - a.cy;
        let dist = (dx * dx + dy * dy).sqrt();
        let d = if dist == 0.0 { 1.0 } else { dist };
        let f = k * (d - rest);
        let fx = dx / d * f;
        let fy = dy / d * f;
        b[ia].fx += fx;
        b[ia].fy += fy;
        b[ic].fx -= fx;
        b[ic].fy -= fy;
    }

    // gravity toward the centre of mass, stronger for well-connected notes
    let gr = 0.018 * p.gravity;
    for o in &mut b {
        o.fx += (cmx - o.cx) * gr * o.m;
        o.fy += (cmy - o.cy) * gr * o.m;
    }

    // integrate: heavy velocity damping and a modest speed cap keep motion smooth rather than twitchy
    let (damping, vmax, rest) = (0.5, 16.0, 0.05);
    for o in &mut b {
        if o.fixed {
            o.v = Velocity::default();
            continue;
        }
        o.v.vx = (o.v.vx + o.fx * alpha) * damping;
        o.v.vy = (o.v.vy + o.fy * alpha) * damping;
        let sp = o.v.vx.hypot(o.v.vy);
        if sp > vmax {
            o.v.vx *= vmax / sp;
            o.v.vy *= vmax / sp;
        } else if sp < rest {
            o.v = Velocity::default();
            continue;
        }
        o.x += o.v.vx;
        o.y += o.v.vy;
        o.cx += o.v.vx;
        o.cy += o.v.vy;
    }

    // collision: separate overlapping cards by a fraction of the overlap per frame, until none is left
    let (pad, relax) = (26.0, 0.3);
    let mut corrected = 0.0;
    for i in 0..n {
        for j in i + 1..n {
            let (a, c) = pair(&mut b, i, j);
            let ox = a.hw + c.hw + pad - (a.cx - c.cx).abs();
            let oy = a.hh + c.hh + pad - (a.cy - c.cy).abs();
            if ox > 0.0 && oy > 0.0 {
                separate(a, c, ox, oy, relax, &mut corrected);
            }
        }
    }

    // link labels must stay readable: keep the two linked cards far enough apart that the label fits between
    // them, and move every other card off the label's box
    let (lh, lpad) = (18.0, 14.0);
    for &(ia, ic, label) in &links {
        if label.is_empty() || ia == ic {
            continue;
        }
        let measured = (measure.label_width)(label).filter(|w| *w != 0.0);
        let lw = measured.unwrap_or(label.chars().count() as f64 * 6.2) + 12.0;
        {
            let (a, c) = if ia < ic {
                pair(&mut b, ia, ic)
            } else {
                let (c, a) = pair(&mut b, ic, ia);
                (a, c)
            };
            let req_x = a.hw + c.hw + lw + 2.0 * lpad;
            let req_y = 2.0 * a.hh.max(c.hh) + lh + 2.0 * lpad;
            let ex = req_x - (a.cx - c.cx).abs();
            let ey = req_y - (a.cy - c.cy).abs();
            if ex > 0.0 && ey > 0.0 {
                separate(a, c, ex, ey, 0.15, &mut corrected);
            }
        }
        let mx = (b[ia].cx + b[ic].cx) / 2.0;
        let my = (b[ia].cy + b[ic].cy) / 2.0 - 7.0;
        for (i, o) in b.iter_mut().enumerate() {
            if i == ia || i == ic || o.fixed {
                continue;
            }
            let ox = o.hw + lw / 2.0 + lpad - (o.cx - mx).abs();
            let oy = o.hh + lh / 2.0 + lpad - (o.cy - my).abs();
            if ox <= 0.0 || oy <= 0.0 {
                continue;
            }
            if ox < oy {
                let sgn = if o.cx >= mx { 1.0 } else { -1.0 };
                let d = ox * relax;
                o.x += sgn * d;
                o.cx += sgn * d;
                corrected = f64::max(corrected, d);
            } else {
                let sgn = if o.cy >= my { 1.0 } else { -1.0 };
                let d = oy * relax;
                o.y += sgn * d;
                o.cy += sgn * d;
                corrected = f64::max(corrected, d);
            }
        }
    }
    sim.corrected = corrected;

    for (nd, o) in g.nodes.values_mut().zip(&b) {
        nd.x = o.x;
        nd.y = o.y;
        vel.insert(nd.id.clone(), o.v);
    }
}

/// The physics-off "Tidy": a full settle in one go, then whole-pixel positions.
pub fn settle(g: &mut Graph, vel: &mut HashMap<String, Velocity>, measure: &Measure, rng: &mut fastrand::Rng) {
    let mut sim = Sim { alpha: 1.0, ..Sim::default() };
    for _ in 0..320 {
        if sim.alpha <= 0.004 {
            break;
        }
        tick(&mut sim, g, vel, measure, None, rng);
    }
    for n in g.nodes.values_mut() {
        n.x = n.x.round();
        n.y = n.y.round();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::DEFAULT_H;
    use crate::model::{Edge, Node};

    fn graph(points: &[(f64, f64)], links: &[(usize, usize)]) -> Graph {
        let mut g = Graph::default();
        for (i, &(x, y)) in points.iter().enumerate() {
            g.add_node(Node { id: format!("n{i}"), x, y, ..Node::default() });
        }
        for &(a, c) in links {
            g.add_edge(Edge { id: format!("e{a}-{c}"), from: format!("n{a}"), to: format!("n{c}"), ..Edge::default() });
        }
        g
    }

    fn overlaps(g: &Graph) -> bool {
        let r: Vec<_> = g.nodes.values().map(|n| (n.x, n.y)).collect();
        r.iter()
            .enumerate()
            .any(|(i, a)| r[i + 1..].iter().any(|b| (a.0 - b.0).abs() < NODE_W && (a.1 - b.1).abs() < DEFAULT_H))
    }

    #[test]
    fn settling_separates_stacked_cards() {
        let mut g = graph(&[(0.0, 0.0), (10.0, 5.0), (20.0, 10.0), (5.0, 30.0)], &[(0, 1), (1, 2)]);
        let h = |_: &str| DEFAULT_H;
        let lw = |_: &str| None;
        settle(
            &mut g,
            &mut HashMap::new(),
            &Measure { height: &h, label_width: &lw },
            &mut fastrand::Rng::with_seed(3),
        );
        assert!(!overlaps(&g));
        assert!(g.nodes.values().all(|n| n.x.fract() == 0.0 && n.y.fract() == 0.0));
    }

    #[test]
    fn locked_and_dragged_notes_stay_put() {
        let mut g = graph(&[(0.0, 0.0), (30.0, 0.0), (60.0, 0.0)], &[(0, 1)]);
        g.nodes[0].locked = Some(true);
        let (h, lw) = (|_: &str| DEFAULT_H, |_: &str| None);
        let mut sim = Sim { alpha: 1.0, ..Sim::default() };
        let mut vel = HashMap::new();
        for _ in 0..50 {
            tick(
                &mut sim,
                &mut g,
                &mut vel,
                &Measure { height: &h, label_width: &lw },
                Some("n2"),
                &mut fastrand::Rng::with_seed(1),
            );
        }
        assert_eq!((g.nodes[0].x, g.nodes[0].y), (0.0, 0.0));
        assert_eq!((g.nodes[2].x, g.nodes[2].y), (60.0, 0.0));
        assert_ne!(g.nodes[1].x, 30.0);
    }

    #[test]
    fn cooling_stops_the_frames() {
        let mut sim = Sim { alpha: 0.003, corrected: 0.0, ..Sim::default() };
        assert!(!sim.next_frame());
        assert_eq!(sim.alpha, 0.0);
        let mut sim = Sim { alpha: 0.003, corrected: 1.0, ..Sim::default() };
        assert!(sim.next_frame());
        sim.tail = 240;
        assert!(!sim.next_frame());
    }
}
