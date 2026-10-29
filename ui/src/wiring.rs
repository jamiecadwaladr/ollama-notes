//! Every listener on the page, the desktop app's menu commands, and the last save before quitting.

use crate::app::{App, html, input, select_el, textarea};
use crate::documents;
use crate::dom::{self, closest, data, on, toggle_class};
use crate::inspector::set_slider_label;
use crate::platform::{self, Pick};
use notes_core::text::ACCEPT;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{DragEvent, Event, HtmlDialogElement, HtmlInputElement, KeyboardEvent, MouseEvent, PointerEvent};

/// Runs an async method of the app without waiting for it.
fn spawn<F: std::future::Future<Output = ()> + 'static>(f: F) {
    spawn_local(f);
}

fn click(app: &Rc<App>, id: &str, f: impl Fn(&Rc<App>) + 'static) {
    let a = Rc::downgrade(app);
    on(&html(id), "click", move |_: MouseEvent| {
        if let Some(a) = a.upgrade() {
            f(&a);
        }
    });
}

fn listen<E: wasm_bindgen::convert::FromWasmAbi + 'static>(
    app: &Rc<App>,
    id: &str,
    ty: &str,
    f: impl Fn(&Rc<App>, E) + 'static,
) {
    let a = Rc::downgrade(app);
    on(&html(id), ty, move |ev: E| {
        if let Some(a) = a.upgrade() {
            f(&a, ev);
        }
    });
}

impl App {
    /// The page's buttons, so menu commands and shortcuts reuse them.
    pub fn command(self: &Rc<Self>, name: &str) {
        let press = |id: &str| html(id).click();
        match name {
            "new-chat" => press("newChatBtn"),
            "new-note" => press("addNodeBtn"),
            "settings" => press("settingsBtn"),
            "import-files" => press("importDocBtn"),
            "import-map" => press("importBtn"),
            "export-map" => press("exportBtn"),
            "fit" => press("fitBtn"),
            "tidy" => press("tidyBtn"),
            "zoom-in" => press("zoomInBtn"),
            "zoom-out" => press("zoomOutBtn"),
            "focus-chat" => {
                let _ = textarea("input").focus();
            }
            "filter-notes" => {
                let s = input("search");
                let _ = s.focus();
                s.select();
            }
            _ => {}
        }
    }

    pub fn bind(self: &Rc<Self>) {
        // ---------- chat
        let a = Rc::downgrade(self);
        on(&html("composer"), "submit", move |ev: Event| {
            ev.prevent_default();
            if let Some(a) = a.upgrade() {
                spawn(async move { a.send().await });
            }
        });
        listen(self, "input", "keydown", |a, ev: KeyboardEvent| {
            if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
                ev.prevent_default();
                let a = a.clone();
                spawn(async move { a.send().await });
            }
        });
        listen(self, "input", "input", |a, _: Event| a.autoresize());
        click(self, "stopBtn", |a| a.stop_reply());
        listen(self, "extractStatus", "click", |a, ev: MouseEvent| {
            if closest(ev.target(), "#cancelExtract").is_some() {
                a.cancel_extractions();
            }
        });
        click(self, "newChatBtn", |a| a.new_chat());
        listen(self, "modelSelect", "change", |a, _: Event| {
            a.st.borrow_mut().settings.model = select_el("modelSelect").value();
            a.fill_model_selects();
            a.save();
        });
        listen(self, "autoNotes", "change", |a, _: Event| {
            a.st.borrow_mut().settings.auto_notes = input("autoNotes").checked();
            a.save();
        });
        click(self, "settingsBtn", |a| {
            a.apply_settings_to_ui();
            a.fill_model_selects();
            let _ = dom::by_id::<HtmlDialogElement>("settingsDialog").show_modal();
        });
        listen(self, "settingsDialog", "close", |a, _: Event| a.read_settings_from_dialog());
        listen(self, "messages", "click", |a, ev: MouseEvent| {
            if let Some(link) = closest(ev.target(), "a[href]") {
                if a.platform.is_desktop() && a.open_link(&link) {
                    ev.prevent_default();
                }
                return;
            }
            if let Some(chip) = closest(ev.target(), "[data-node]") {
                let id = data(&chip, "node").unwrap_or_default();
                a.graph.select(Some(&id));
                a.graph.center_on(&id);
                return;
            }
            let Some(act) = closest(ev.target(), "[data-act]") else { return };
            let Some(msg) = act.closest(".msg").ok().flatten().and_then(|m| data(&m, "id")) else { return };
            match data(&act, "act").as_deref() {
                Some("note") => a.note_from_message(&msg),
                Some("extract") => a.extract_from_message(&msg),
                _ => {}
            }
        });
        listen(self, "contextBar", "click", |a, ev: MouseEvent| {
            if let Some(un) = closest(ev.target(), "[data-unpin]") {
                a.toggle_pin(&data(&un, "unpin").unwrap_or_default());
            } else if let Some(chip) = closest(ev.target(), "[data-node]") {
                let id = data(&chip, "node").unwrap_or_default();
                a.graph.select(Some(&id));
                a.graph.center_on(&id);
            }
        });

        // ---------- inspector
        click(self, "inspClose", |a| a.graph.select(None));
        listen(self, "inspTitle", "input", |a, _: Event| {
            let Some(id) = a.insp_id.borrow().clone() else { return };
            let title = input("inspTitle").value();
            a.graph.update_node(&id, |n| n.title = title);
            a.render_context_bar();
            a.schedule_keywords();
        });
        listen(self, "inspBody", "input", |a, _: Event| {
            let Some(id) = a.insp_id.borrow().clone() else { return };
            let body = textarea("inspBody").value();
            a.graph.update_node(&id, |n| n.body = body);
            a.schedule_keywords();
        });
        listen(self, "inspKeywords", "change", |a, _: Event| {
            let Some(id) = a.insp_id.borrow().clone() else { return };
            let mut list: Vec<String> = Vec::new();
            for k in input("inspKeywords").value().split([',', ';', '\n']).map(str::trim).filter(|k| !k.is_empty()) {
                if !list.iter().any(|x| x == k) {
                    list.push(k.to_string());
                }
            }
            list.truncate(12);
            a.graph.update_node(&id, |n| {
                n.keywords = Some(list);
                n.keywords_manual = Some(true);
            });
            a.refresh_keywords(false);
            a.show_inspector(Some(&id));
        });
        click(self, "inspKwAuto", |a| {
            let Some(id) = a.insp_id.borrow().clone() else { return };
            a.graph.update_node(&id, |n| {
                n.keywords_manual = Some(false);
                n.kw_text = None;
            });
            a.refresh_keywords(false);
            a.show_inspector(Some(&id));
        });
        listen(self, "inspKind", "change", |a, _: Event| {
            let Some(id) = a.insp_id.borrow().clone() else { return };
            let kind = select_el("inspKind").value();
            a.graph.update_node(&id, |n| n.kind = kind);
            a.show_inspector(Some(&id));
        });
        click(self, "inspPin", |a| {
            let Some(id) = a.insp_id.borrow().clone() else { return };
            a.toggle_pin(&id);
        });
        click(self, "inspAsk", |a| {
            let Some(n) = a.insp_id.borrow().clone().and_then(|id| a.graph.node(&id)) else { return };
            let mut text = format!("Tell me more about \"{}\".", n.title);
            if !n.body.is_empty() {
                text.push_str(&format!("\n\nWhat I have so far: {}", n.body));
            }
            textarea("input").set_value(&text);
            a.autoresize();
            let _ = textarea("input").focus();
        });
        click(self, "inspSource", |a| {
            let msg = a.insp_id.borrow().clone().and_then(|id| a.graph.node(&id)).and_then(|n| n.source?.msg_id);
            if let Some(m) = msg {
                a.show_source(&m);
            }
        });
        click(self, "inspLock", |a| {
            let Some(id) = a.insp_id.borrow().clone() else { return };
            a.graph.update_node(&id, |n| n.locked = Some(!n.is_locked()));
            a.show_inspector(Some(&id));
        });
        click(self, "inspDelete", |a| {
            let Some(id) = a.insp_id.borrow().clone() else { return };
            if a.graph.node(&id).is_none() {
                return;
            }
            {
                let mut st = a.st.borrow_mut();
                if st.hub_id.as_deref() == Some(id.as_str()) {
                    st.hub_id = None;
                }
            }
            a.graph.remove_node(&id);
            a.render_context_bar();
            a.save();
        });
        listen(self, "inspLinks", "click", |a, ev: MouseEvent| {
            if let Some(x) = closest(ev.target(), "[data-edge]") {
                let id = data(&x, "edge").unwrap_or_default();
                let edge = a.graph.with(|g| g.edges.get(&id).cloned());
                if let Some(e) = edge.filter(|e| e.is_keyword()) {
                    a.graph.ignore_pair(&e.from, &e.to);
                }
                a.graph.remove_edge(&id);
                a.refresh_inspector();
                return;
            }
            if let Some(chip) = closest(ev.target(), "[data-node]") {
                let id = data(&chip, "node").unwrap_or_default();
                a.graph.select(Some(&id));
                a.graph.center_on(&id);
            }
        });

        // ---------- map toolbar
        click(self, "addNodeBtn", |a| {
            let (x, y) = a.graph.place_near(None);
            let node = a.graph.new_node(x, y);
            let id = a.graph.add_node(node);
            a.graph.select(Some(&id));
            a.graph.flash(&id);
            a.graph.center_on(&id);
            let t = input("inspTitle");
            let _ = t.focus();
            t.select();
        });
        click(self, "fitBtn", |a| a.graph.fit(true));
        listen(self, "physicsToggle", "change", |a, _: Event| {
            let on = input("physicsToggle").checked();
            a.graph.set_physics(|p| p.enabled = on);
            a.sync_physics_ui();
        });
        click(self, "physicsBtn", |a| {
            if a.toggle_popover("physicsPop") {
                a.sync_physics_ui();
            }
        });
        let a = Rc::downgrade(self);
        on(&dom::document(), "pointerdown", move |ev: PointerEvent| {
            if closest(ev.target(), ".popover-wrap").is_none()
                && let Some(a) = a.upgrade()
            {
                a.close_popovers();
            }
        });
        click(self, "kwBtn", |a| {
            if a.toggle_popover("kwPop") {
                a.sync_keyword_ui();
            }
        });
        listen(self, "kwEnabled", "change", |a, _: Event| {
            let on = input("kwEnabled").checked();
            a.graph.set_keyword_links(|k| k.enabled = on);
            a.sync_keyword_ui();
            if on {
                let r = a.refresh_keywords(true);
                a.toast(&format!("{} added", crate::app::plural(r.added, "keyword link")), 2800);
            }
        });
        listen(self, "kwThreshold", "input", |a, _: Event| {
            let el = input("kwThreshold");
            let v = el.value_as_number();
            a.graph.set_keyword_links(|k| k.threshold = v);
            set_slider_label(&el, v);
            a.schedule_keywords();
        });
        click(self, "kwRun", |a| {
            let r = a.refresh_keywords(true);
            let off = if a.graph.keyword_links().enabled { "" } else { " (auto-link is off)" };
            a.toast(&format!("Keyword links: {} added, {} removed{off}", r.added, r.removed), 2800);
        });
        click(self, "kwClear", |a| {
            let n = a.graph.remove_keyword_links();
            a.graph.set_keyword_links(|k| k.enabled = false);
            a.sync_keyword_ui();
            a.refresh_inspector();
            a.toast(&format!("{} removed; auto-link turned off", crate::app::plural(n, "keyword link")), 2800);
        });
        for (id, key) in [("phTension", 0), ("phGravity", 1), ("phRepulsion", 2)] {
            listen(self, id, "input", move |a, ev: Event| {
                let Some(el) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) else { return };
                let v = el.value_as_number();
                a.graph.set_physics(|p| match key {
                    0 => p.tension = v,
                    1 => p.gravity = v,
                    _ => p.repulsion = v,
                });
                set_slider_label(&el, v);
            });
        }
        click(self, "phReset", |a| {
            a.graph.set_physics(|p| {
                p.tension = 1.0;
                p.gravity = 1.0;
                p.repulsion = 1.0;
            });
            a.sync_physics_ui();
        });
        click(self, "tidyBtn", |a| a.graph.tidy());
        click(self, "zoomInBtn", |a| a.graph.zoom_by(1.25));
        click(self, "zoomOutBtn", |a| a.graph.zoom_by(0.8));
        listen(self, "search", "input", |a, _: Event| a.graph.set_filter(&input("search").value()));

        // ---------- files: the app's open panel, or the page's file inputs in a browser
        input("docFile").set_accept(ACCEPT);
        click(self, "importDocBtn", |a| {
            if a.platform.is_desktop() {
                let a = a.clone();
                spawn(async move {
                    let files = a.platform.pick_files(Pick::Documents).await;
                    a.import_files(files).await;
                });
            } else {
                html("docFile").click();
            }
        });
        listen(self, "docFile", "change", |a, _: Event| {
            let el = input("docFile");
            let Some(list) = el.files() else { return };
            let a = a.clone();
            spawn(async move {
                let files = documents::read_file_list(&list).await;
                input("docFile").set_value("");
                a.import_files(files).await;
            });
        });
        self.bind_drops();
        click(self, "exportBtn", |a| {
            let a = a.clone();
            spawn(async move { a.export_json().await });
        });
        click(self, "importBtn", |a| {
            if a.platform.is_desktop() {
                let a = a.clone();
                spawn(async move {
                    if let Some(f) = a.platform.pick_files(Pick::Map).await.into_iter().next() {
                        a.import_json(&String::from_utf8_lossy(&f.bytes)).await;
                    }
                });
            } else {
                html("importFile").click();
            }
        });
        listen(self, "importFile", "change", |a, _: Event| {
            let el = input("importFile");
            let Some(list) = el.files() else { return };
            let a = a.clone();
            spawn(async move {
                let files = documents::read_file_list(&list).await;
                input("importFile").set_value("");
                if let Some(f) = files.into_iter().next() {
                    a.import_json(&String::from_utf8_lossy(&f.bytes)).await;
                }
            });
        });
        click(self, "clearMapBtn", |a| {
            let a = a.clone();
            spawn(async move { a.clear_map().await });
        });

        // the map toolbar wraps onto a second row in a narrow window; keep the inspector below it
        let observer = Closure::<dyn FnMut(js_sys::Array)>::new(|entries: js_sys::Array| {
            let Some(e) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>().ok() else { return };
            let Some(t) = e.target().dyn_into::<web_sys::HtmlElement>().ok() else { return };
            let _ = html("mapPane").style().set_property("--toolbar-h", &format!("{}px", t.offset_height()));
        });
        if let Ok(ro) = web_sys::ResizeObserver::new(observer.as_ref().unchecked_ref())
            && let Some(bar) = dom::document().query_selector(".map-toolbar").ok().flatten()
        {
            ro.observe(&bar);
        }
        observer.forget();

        self.bind_splitter();

        // ---------- desktop app: menu bar commands, and the last save before quitting (src-tauri/src/main.rs)
        if self.platform.is_desktop() {
            let a = Rc::downgrade(self);
            self.platform.listen("menu", move |ev| {
                let name = js_sys::Reflect::get(&ev, &"payload".into()).ok().and_then(|p| p.as_string());
                if let (Some(a), Some(name)) = (a.upgrade(), name) {
                    a.command(&name);
                }
            });
            let a = Rc::downgrade(self);
            self.platform.listen("flush", move |_| {
                let Some(a) = a.upgrade() else { return };
                spawn(async move {
                    if a.save_pending() {
                        a.flush_save().await;
                    }
                    let _ = platform::invoke("flush_done", JsValue::UNDEFINED).await;
                });
            });
        }
    }

    fn bind_drops(self: &Rc<Self>) {
        let canvas = html("canvas");
        let depth = Rc::new(std::cell::Cell::new(0u32));
        let has_files = |ev: &DragEvent| {
            ev.data_transfer().is_some_and(|dt| dt.types().iter().any(|t| t.as_string().as_deref() == Some("Files")))
        };
        let doc = dom::document();
        let (c, d) = (canvas.clone(), depth.clone());
        on(&doc, "dragenter", move |ev: DragEvent| {
            if !has_files(&ev) {
                return;
            }
            ev.prevent_default();
            d.set(d.get() + 1);
            let _ = c.class_list().add_1("dropping");
        });
        on(&doc, "dragover", move |ev: DragEvent| {
            if has_files(&ev) {
                ev.prevent_default();
                if let Some(dt) = ev.data_transfer() {
                    dt.set_drop_effect("copy");
                }
            }
        });
        let (c, d) = (canvas.clone(), depth.clone());
        on(&doc, "dragleave", move |ev: DragEvent| {
            if !has_files(&ev) {
                return;
            }
            d.set(d.get().saturating_sub(1));
            if d.get() == 0 {
                toggle_class(&c, "dropping", false);
            }
        });
        let a = Rc::downgrade(self);
        on(&doc, "drop", move |ev: DragEvent| {
            if !has_files(&ev) {
                return;
            }
            ev.prevent_default();
            depth.set(0);
            toggle_class(&canvas, "dropping", false);
            let (Some(a), Some(list)) = (a.upgrade(), ev.data_transfer().and_then(|dt| dt.files())) else { return };
            spawn(async move {
                let files = documents::read_file_list(&list).await;
                a.import_files(files).await;
            });
        });
    }

    fn bind_splitter(self: &Rc<Self>) {
        let sp = html("splitter");
        let dragging = Rc::new(std::cell::Cell::new(false));
        let (s, d) = (sp.clone(), dragging.clone());
        on(&sp, "pointerdown", move |ev: PointerEvent| {
            d.set(true);
            let _ = s.class_list().add_1("active");
            let _ = s.set_pointer_capture(ev.pointer_id());
        });
        let (a, d) = (Rc::downgrade(self), dragging.clone());
        on(&sp, "pointermove", move |ev: PointerEvent| {
            let Some(a) = a.upgrade().filter(|_| d.get()) else { return };
            let inner = dom::window().inner_width().ok().and_then(|w| w.as_f64()).unwrap_or(1280.0);
            let w = (ev.client_x() as f64).max(300.0).min(inner - 360.0).round();
            a.st.borrow_mut().settings.chat_width = w;
            App::set_chat_width(w);
        });
        let (a, s) = (Rc::downgrade(self), sp.clone());
        let stop = move |_: PointerEvent| {
            if dragging.replace(false) {
                let _ = s.class_list().remove_1("active");
                if let Some(a) = a.upgrade() {
                    a.save();
                }
            }
        };
        on(&sp, "pointerup", stop.clone());
        on(&sp, "pointercancel", stop);
    }
}
