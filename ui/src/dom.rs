//! Small helpers over web-sys: finding elements, listening to events, timers, animation frames, and the browser's
//! locale formatting.

use js_sys::{Function, Reflect};
use wasm_bindgen::JsCast;
use wasm_bindgen::convert::FromWasmAbi;
use wasm_bindgen::prelude::*;
use web_sys::{Document, Element, EventTarget, Window};

pub fn window() -> Window {
    web_sys::window().expect("a window")
}

pub fn document() -> Document {
    window().document().expect("a document")
}

/// The element with this id, as `T`. The markup is the app's own, so a missing element is a bug.
pub fn by_id<T: JsCast>(id: &str) -> T {
    document()
        .get_element_by_id(id)
        .unwrap_or_else(|| panic!("#{id} is missing from index.html"))
        .dyn_into::<T>()
        .unwrap_or_else(|_| panic!("#{id} has an unexpected element type"))
}

pub fn query<T: JsCast>(root: &Element, selector: &str) -> Option<T> {
    root.query_selector(selector).ok().flatten().and_then(|e| e.dyn_into::<T>().ok())
}

pub fn query_all(root: &Element, selector: &str) -> Vec<Element> {
    let Ok(list) = root.query_selector_all(selector) else { return Vec::new() };
    (0..list.length()).filter_map(|i| list.item(i)?.dyn_into::<Element>().ok()).collect()
}

/// The nearest ancestor-or-self of an event target matching `selector`.
pub fn closest(target: Option<EventTarget>, selector: &str) -> Option<Element> {
    target?.dyn_into::<Element>().ok()?.closest(selector).ok().flatten()
}

/// The value of a `data-*` attribute.
pub fn data(el: &Element, name: &str) -> Option<String> {
    el.get_attribute(&format!("data-{name}"))
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn toggle_class(el: &Element, class: &str, on: bool) {
    let _ = el.class_list().toggle_with_force(class, on);
}

/// Listens for `ty` events for the life of the page.
pub fn on<E: FromWasmAbi + 'static>(target: &EventTarget, ty: &str, f: impl FnMut(E) + 'static) {
    let cb = Closure::<dyn FnMut(E)>::new(f);
    let _ = target.add_event_listener_with_callback(ty, cb.as_ref().unchecked_ref());
    cb.forget();
}

/// `on`, for a listener that calls `preventDefault()` on scroll-like events (passive: false).
pub fn on_active<E: FromWasmAbi + 'static>(target: &EventTarget, ty: &str, f: impl FnMut(E) + 'static) {
    let cb = Closure::<dyn FnMut(E)>::new(f);
    let opts = web_sys::AddEventListenerOptions::new();
    opts.set_passive(false);
    let _ =
        target.add_event_listener_with_callback_and_add_event_listener_options(ty, cb.as_ref().unchecked_ref(), &opts);
    cb.forget();
}

/// Runs `f` once after `ms` milliseconds. Returns the timer id for `clear_timeout`.
pub fn set_timeout(ms: i32, f: impl FnOnce() + 'static) -> i32 {
    let cb = Closure::once_into_js(f);
    window().set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms).unwrap_or(0)
}

pub fn clear_timeout(id: i32) {
    window().clear_timeout_with_handle(id);
}

/// A restartable one-shot timer, for debouncing.
#[derive(Default)]
pub struct Timer(std::cell::Cell<Option<i32>>);

impl Timer {
    pub fn start(&self, ms: i32, f: impl FnOnce() + 'static) {
        self.clear();
        self.0.set(Some(set_timeout(ms, f)));
    }

    pub fn clear(&self) {
        if let Some(id) = self.0.take() {
            clear_timeout(id);
        }
    }

    /// Whether it has been started and not cleared (it may have fired already; callers clear it when it does).
    pub fn pending(&self) -> bool {
        self.0.get().is_some()
    }

    pub fn forget(&self) {
        self.0.set(None);
    }
}

/// Runs `f` before the next repaint. Returns the request id for `cancel_frame`.
pub fn request_frame(f: impl FnOnce(f64) + 'static) -> i32 {
    let cb = Closure::once_into_js(f);
    window().request_animation_frame(cb.unchecked_ref()).unwrap_or(0)
}

pub fn cancel_frame(id: i32) {
    let _ = window().cancel_animation_frame(id);
}

/// Milliseconds since the epoch.
pub fn now() -> f64 {
    js_sys::Date::now()
}

pub fn perf_now() -> f64 {
    window().performance().map_or_else(now, |p| p.now())
}

/// Calls a method with no arguments and returns its string result: the browser's own locale formatting.
fn call_string(obj: &JsValue, method: &str) -> String {
    Reflect::get(obj, &method.into())
        .ok()
        .and_then(|f| f.dyn_into::<Function>().ok())
        .and_then(|f| f.call0(obj).ok())
        .and_then(|v| v.as_string())
        .unwrap_or_default()
}

/// `new Date(ms).toLocaleString()`.
pub fn locale_date_time(ms: f64) -> String {
    call_string(&js_sys::Date::new(&ms.into()).into(), "toLocaleString")
}

/// `new Date(ms).toLocaleDateString()`.
pub fn locale_date(ms: f64) -> String {
    call_string(&js_sys::Date::new(&ms.into()).into(), "toLocaleDateString")
}

/// Today's date in words, e.g. "Thursday 29 October 2026".
pub fn long_date() -> String {
    let opts = js_sys::Object::new();
    for (k, v) in [("weekday", "long"), ("day", "numeric"), ("month", "long"), ("year", "numeric")] {
        let _ = js_sys::Reflect::set(&opts, &k.into(), &v.into());
    }
    js_sys::Date::new_0().to_locale_date_string("en-GB", &opts).into()
}

/// `n.toLocaleString()`, e.g. 12,345.
pub fn locale_number(n: usize) -> String {
    let format = js_sys::Intl::NumberFormat::new(&js_sys::Array::new(), &js_sys::Object::new()).format();
    format
        .call1(&JsValue::NULL, &JsValue::from_f64(n as f64))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| n.to_string())
}

/// The current time as an ISO 8601 string (UTC).
pub fn iso_now() -> String {
    js_sys::Date::new_0().to_iso_string().into()
}

/// A JS error or rejection value as text.
pub fn js_error(e: &JsValue) -> String {
    if let Some(s) = e.as_string() {
        return s;
    }
    Reflect::get(e, &"message".into()).ok().and_then(|m| m.as_string()).unwrap_or_else(|| format!("{e:?}"))
}

/// A JS number as text the way JS prints it in templates (integers without ".0").
pub fn num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// `+s || default`: the number in a form field, or the default when it is empty, zero or not a number.
pub fn num_or(s: &str, default: f64) -> f64 {
    s.trim().parse::<f64>().ok().filter(|v| *v != 0.0 && v.is_finite()).unwrap_or(default)
}

pub fn warn(msg: &str) {
    web_sys::console::warn_1(&msg.into());
}
