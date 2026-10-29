//! The map's pan/zoom transform: zooming about a point, framing every note, centring one, and easing between views.

use crate::graph::Rect;
use crate::model::View;

pub const MIN_ZOOM: f64 = 0.12;
pub const MAX_ZOOM: f64 = 2.5;

/// Zooms by `factor` keeping the canvas point (mx, my) where it is.
pub fn zoom_at(view: &mut View, mx: f64, my: f64, factor: f64) {
    let k = (view.k * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let f = k / view.k;
    view.x = mx - (mx - view.x) * f;
    view.y = my - (my - view.y) * f;
    view.k = k;
}

/// The canvas point (relative to the canvas's top left) in map coordinates.
pub fn to_world(view: &View, x: f64, y: f64) -> (f64, f64) {
    ((x - view.x) / view.k, (y - view.y) / view.k)
}

/// The view that frames `bounds` (x1, y1, x2, y2) in a canvas of `width` × `height`, leaving `pad_right` free
/// for an overlay.
pub fn fit(bounds: (f64, f64, f64, f64), width: f64, height: f64, pad_right: f64) -> View {
    let (x1, y1, x2, y2) = bounds;
    let w_avail = (width - pad_right).max(200.0);
    let pad = 50.0;
    let (w, h) = (x2 - x1, y2 - y1);
    let k = ((w_avail - pad * 2.0) / w).min((height - pad * 2.0) / h).clamp(MIN_ZOOM, 1.5);
    View { k, x: (w_avail - w * k) / 2.0 - x1 * k, y: (height - h * k) / 2.0 - y1 * k }
}

/// The view that centres a card, zoomed in to at least 0.8.
pub fn center_on(view: &View, r: Rect, width: f64, height: f64, pad_right: f64) -> View {
    let k = view.k.max(0.8);
    View { k, x: (width - pad_right) / 2.0 - (r.x + r.w / 2.0) * k, y: height / 2.0 - (r.y + r.h / 2.0) * k }
}

/// The view `t` of the way (0–1) from `from` to `to`, with a cubic ease-out.
pub fn ease(from: &View, to: &View, t: f64) -> View {
    let e = 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3);
    View { x: from.x + (to.x - from.x) * e, y: from.y + (to.y - from.y) * e, k: from.k + (to.k - from.k) * e }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zooming_keeps_the_point_under_the_cursor() {
        let mut v = View { x: 10.0, y: 20.0, k: 1.0 };
        let before = to_world(&v, 300.0, 200.0);
        zoom_at(&mut v, 300.0, 200.0, 2.0);
        let after = to_world(&v, 300.0, 200.0);
        assert!((before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9);
        zoom_at(&mut v, 0.0, 0.0, 100.0);
        assert_eq!(v.k, MAX_ZOOM);
    }

    #[test]
    fn fit_frames_the_bounds() {
        let v = fit((0.0, 0.0, 1000.0, 500.0), 1100.0, 700.0, 0.0);
        assert!((v.k - 1.0).abs() < 1e-9);
        assert_eq!((v.x, v.y), (50.0, 100.0));
        let v = fit((0.0, 0.0, 10.0, 10.0), 1100.0, 700.0, 0.0);
        assert_eq!(v.k, 1.5);
    }

    #[test]
    fn easing_ends_at_the_target() {
        let (a, b) = (View { x: 0.0, y: 0.0, k: 1.0 }, View { x: 10.0, y: -10.0, k: 2.0 });
        assert_eq!(ease(&a, &b, 1.0), b);
        assert_eq!(ease(&a, &b, 0.0), a);
        assert!(ease(&a, &b, 0.5).x > 5.0);
    }
}
