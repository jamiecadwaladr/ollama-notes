//! Draws the app icon, a small notes map: the ◎ hub linked to note cards in the map's kind colours, on the macOS
//! icon grid (an 824-point tile with a soft shadow in a 1024 square).
//!   make-icon <out.png>    then: cargo tauri icon <out.png>   (scripts/icons.sh does both)
//! Coordinates below are the original's, with the origin at the bottom left as in CoreGraphics.

use anyhow::{Context, Result};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point,
    SpreadMode, Stroke, StrokeDash, Transform,
};

const SIZE: f32 = 1024.0;

fn color(hex: u32, alpha: f32) -> Color {
    Color::from_rgba8(
        (hex >> 16 & 0xff) as u8,
        (hex >> 8 & 0xff) as u8,
        (hex & 0xff) as u8,
        (alpha * 255.0).round() as u8,
    )
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

/// A rounded rectangle given bottom-left-origin coordinates.
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let top = SIZE - y - h;
    let (l, t, rr, b) = (x, top, x + w, top + h);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(l + r, t);
    pb.line_to(rr - r, t);
    pb.cubic_to(rr - r + k, t, rr, t + r - k, rr, t + r);
    pb.line_to(rr, b - r);
    pb.cubic_to(rr, b - r + k, rr - r + k, b, rr - r, b);
    pb.line_to(l + r, b);
    pb.cubic_to(l + r - k, b, l, b - r + k, l, b - r);
    pb.line_to(l, t + r);
    pb.cubic_to(l, t + r - k, l + r - k, t, l + r, t);
    pb.close();
    pb.finish().expect("a rounded rectangle")
}

fn circle(cx: f32, cy: f32, r: f32) -> Path {
    PathBuilder::from_circle(cx, SIZE - cy, r).expect("a circle")
}

fn line(points: &[(f32, f32)]) -> Path {
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].0, SIZE - points[0].1);
    for &(x, y) in &points[1..] {
        pb.line_to(x, SIZE - y);
    }
    pb.finish().expect("a line")
}

/// A Gaussian-ish blur (three box blurs) of a premultiplied RGBA pixmap.
fn blur(p: &mut Pixmap, sigma: f32) {
    let (w, h) = (p.width() as usize, p.height() as usize);
    let n = 3.0;
    let ideal = ((12.0 * sigma * sigma / n) + 1.0).sqrt();
    let mut lo = ideal.floor() as usize;
    if lo.is_multiple_of(2) {
        lo -= 1;
    }
    let m = ((12.0 * sigma * sigma - n * (lo * lo) as f32 - 4.0 * n * lo as f32 - 3.0 * n) / (-4.0 * lo as f32 - 4.0))
        .round() as usize;
    let data = p.data_mut();
    let mut tmp = vec![0u8; data.len()];
    for pass in 0..3 {
        let size = if pass < m { lo } else { lo + 2 };
        let r = size / 2;
        box_pass(data, &mut tmp, w, h, r, true);
        box_pass(&tmp, data, w, h, r, false);
    }
}

fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| if horizontal { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let div = (2 * r + 1) as u32;
    for line in 0..lines {
        for c in 0..4 {
            let get =
                |i: isize| -> u32 { if i < 0 || i as usize >= len { 0 } else { src[at(line, i as usize) + c] as u32 } };
            let mut sum: u32 = (-(r as isize)..=r as isize).map(get).sum();
            for i in 0..len {
                dst[at(line, i) + c] = (sum / div) as u8;
                sum += get(i as isize + r as isize + 1);
                sum -= get(i as isize - r as isize);
            }
        }
    }
}

/// Draws `shape`'s shadow: the shape in `shadow` colour, blurred, offset down by `dy`.
fn shadow(canvas: &mut Pixmap, shape: impl Fn(&mut Pixmap), dy: f32, blur_radius: f32, clip: Option<&Mask>) {
    let mut layer = Pixmap::new(canvas.width(), canvas.height()).expect("a layer");
    shape(&mut layer);
    blur(&mut layer, blur_radius / 2.0);
    canvas.draw_pixmap(0, dy.round() as i32, layer.as_ref(), &PixmapPaint::default(), Transform::identity(), clip);
}

fn draw() -> Pixmap {
    let mut pm = Pixmap::new(SIZE as u32, SIZE as u32).expect("a canvas");
    let id = Transform::identity();
    let tile = rounded(100.0, 100.0, 824.0, 824.0, 186.0);
    shadow(&mut pm, |l| l.fill_path(&tile, &paint(color(0, 0.5)), FillRule::Winding, id, None), 10.0, 28.0, None);
    pm.fill_path(&tile, &paint(color(0x161923, 1.0)), FillRule::Winding, id, None);

    // everything else is clipped to the tile
    let mut clip = Mask::new(SIZE as u32, SIZE as u32).expect("a mask");
    clip.fill_path(&tile, FillRule::Winding, true, id);
    let clip = Some(&clip);
    let gradient = LinearGradient::new(
        Point::from_xy(512.0, SIZE - 924.0),
        Point::from_xy(512.0, SIZE - 100.0),
        vec![GradientStop::new(0.0, color(0x2a3047, 1.0)), GradientStop::new(1.0, color(0x0f1117, 1.0))],
        SpreadMode::Pad,
        id,
    )
    .expect("a gradient");
    let mut g = Paint { shader: gradient, ..Paint::default() };
    g.anti_alias = true;
    pm.fill_path(&tile, &g, FillRule::Winding, id, clip);
    let dot = paint(color(0x2a2f3f, 1.0)); // the canvas dot grid
    for x in (148..=880).step_by(61) {
        for y in (148..=880).step_by(61) {
            pm.fill_path(&circle(x as f32, y as f32, 4.0), &dot, FillRule::Winding, id, clip);
        }
    }

    let hub = (512.0, 500.0);
    let cards = [
        ((300.0, 730.0), 0x60a5fa),
        ((735.0, 742.0), 0x2dd4bf),
        ((752.0, 300.0), 0xfbbf24),
        ((278.0, 282.0), 0xf472b6),
    ];
    let link = paint(color(0x5b6690, 1.0));
    let mut stroke = Stroke { width: 16.0, line_cap: tiny_skia::LineCap::Round, ..Stroke::default() };
    for (p, _) in cards {
        pm.stroke_path(&line(&[hub, p]), &link, &stroke, id, clip);
    }
    stroke.width = 10.0; // a keyword link
    stroke.dash = StrokeDash::new(vec![26.0, 22.0], 0.0);
    pm.stroke_path(&line(&[cards[0].0, cards[1].0]), &link, &stroke, id, clip);

    for ((px, py), kind) in cards {
        let (x, y, w, h) = (px - 104.0, py - 62.0, 208.0, 124.0);
        let card = rounded(x, y, w, h, 24.0);
        shadow(&mut pm, |l| l.fill_path(&card, &paint(color(0, 0.45)), FillRule::Winding, id, None), 6.0, 16.0, clip);
        pm.fill_path(&card, &paint(color(0x1e2230, 1.0)), FillRule::Winding, id, clip);
        let mut card_clip = Mask::new(SIZE as u32, SIZE as u32).expect("a mask");
        card_clip.fill_path(&card, FillRule::Winding, true, id);
        pm.fill_path(&rounded(x, y, 18.0, h, 0.01), &paint(color(kind, 1.0)), FillRule::Winding, id, Some(&card_clip));
        pm.stroke_path(&card, &paint(color(0x343a50, 1.0)), &Stroke { width: 3.0, ..Stroke::default() }, id, clip);
        let top = y + h; // title line, then two body lines
        pm.fill_path(
            &rounded(x + 42.0, top - 46.0, 120.0, 18.0, 9.0),
            &paint(color(0xe6e8ef, 0.9)),
            FillRule::Winding,
            id,
            clip,
        );
        for (dy, lw) in [(80.0, 136.0), (106.0, 96.0)] {
            pm.fill_path(
                &rounded(x + 42.0, top - dy, lw, 12.0, 6.0),
                &paint(color(0x8b91a5, 0.8)),
                FillRule::Winding,
                id,
                clip,
            );
        }
    }

    // the hub: ◎ in the accent colour, with a glow
    pm.fill_path(&circle(hub.0, hub.1, 132.0), &paint(color(0x0f1117, 1.0)), FillRule::Winding, id, clip);
    let ring = circle(hub.0, hub.1, 104.0);
    let ring_stroke = Stroke { width: 34.0, ..Stroke::default() };
    let core = circle(hub.0, hub.1, 48.0);
    let glow = paint(color(0x7c8cff, 0.75));
    shadow(
        &mut pm,
        |l| {
            l.stroke_path(&ring, &glow, &ring_stroke, id, None);
            l.fill_path(&core, &glow, FillRule::Winding, id, None);
        },
        0.0,
        40.0,
        clip,
    );
    pm.stroke_path(&ring, &paint(color(0x7c8cff, 1.0)), &ring_stroke, id, clip);
    pm.fill_path(&core, &paint(color(0x9aa6ff, 1.0)), FillRule::Winding, id, clip);
    pm
}

fn main() -> Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "icon.png".into());
    draw().save_png(&out).with_context(|| format!("writing {out}"))?;
    println!("Wrote {out}");
    Ok(())
}
