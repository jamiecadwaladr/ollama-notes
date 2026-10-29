//! The page's state and the parts of app.js that aren't chat, documents or the inspector: saving and loading,
//! the toast and status line, the model list, settings, and export/import. The other parts live in chat.rs,
//! import.rs, inspector.rs and wiring.rs as further `impl App` blocks.

use crate::dom::{self, esc, num, num_or, toggle_class};
use crate::graph_view::{Els, GraphEvent, GraphView};
use crate::ollama::{Abort, AbortReason, Ollama};
use crate::platform::Platform;
use notes_core::graph::new_id;
use notes_core::host::{DEFAULT_OLLAMA_HOST, normalize_api_base, normalize_host};
use notes_core::keywords::LinkChanges;
use notes_core::model::{AppState, Message, Provider};
use notes_core::ollama::ModelInfo;
use notes_core::openai;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlElement, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};

pub fn el<T: JsCast>(id: &str) -> T {
    dom::by_id(id)
}
pub fn html(id: &str) -> HtmlElement {
    el(id)
}
pub fn input(id: &str) -> HtmlInputElement {
    el(id)
}
pub fn textarea(id: &str) -> HtmlTextAreaElement {
    el(id)
}
pub fn select_el(id: &str) -> HtmlSelectElement {
    el(id)
}

/// "1 note", "2 notes".
pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

pub struct App {
    pub graph: Rc<GraphView>,
    pub platform: Platform,
    pub ollama: Ollama,
    pub st: RefCell<AppState>,
    pub models: RefCell<Vec<ModelInfo>>,
    /// The chat reply being streamed.
    pub chat_abort: RefCell<Option<Abort>>,
    pub extracting: Cell<u32>,
    pub extract_label: RefCell<String>,
    pub extract_aborts: RefCell<Vec<Abort>>,
    pub insp_id: RefCell<Option<String>>,
    /// The last model a toast said can't search the web, so it says so once.
    pub no_tools_toast: RefCell<String>,
    /// Nothing is saved before the saved state has been read, so it can't be overwritten.
    loaded: Cell<bool>,
    save_timer: dom::Timer,
    toast_timer: dom::Timer,
    models_timer: dom::Timer,
    pub kw_timer: dom::Timer,
    /// Set at launch and when the connection changes in Settings: if the models can't be listed, the app then starts
    /// a local Ollama, or shows the cloud API's error.
    reconnected: Cell<bool>,
    started_ollama_at: Cell<f64>,
    rng: RefCell<fastrand::Rng>,
    pub paint_frame: Cell<Option<i32>>,
    me: RefCell<Weak<App>>,
}

impl App {
    pub fn new() -> Rc<App> {
        let graph =
            GraphView::new(Els { canvas: el("canvas"), world: el("world"), edges: el("edges"), nodes: el("nodes") });
        let platform = Platform::detect();
        let seed = (dom::now() as u64) ^ ((js_sys::Math::random() * 1e15) as u64);
        let app = Rc::new(App {
            graph,
            platform,
            ollama: Ollama::default(),
            st: RefCell::new(AppState::default()),
            models: RefCell::default(),
            chat_abort: RefCell::default(),
            extracting: Cell::new(0),
            extract_label: RefCell::default(),
            extract_aborts: RefCell::default(),
            insp_id: RefCell::default(),
            no_tools_toast: RefCell::default(),
            loaded: Cell::new(false),
            save_timer: dom::Timer::default(),
            toast_timer: dom::Timer::default(),
            models_timer: dom::Timer::default(),
            kw_timer: dom::Timer::default(),
            reconnected: Cell::new(true),
            started_ollama_at: Cell::new(0.0),
            rng: RefCell::new(fastrand::Rng::with_seed(seed)),
            paint_frame: Cell::new(None),
            me: RefCell::new(Weak::new()),
        });
        *app.me.borrow_mut() = Rc::downgrade(&app);
        let weak = Rc::downgrade(&app);
        app.graph.set_handler(move |ev| {
            if let Some(a) = weak.upgrade() {
                a.on_graph(ev);
            }
        });
        app
    }

    pub fn rc(&self) -> Rc<App> {
        self.me.borrow().upgrade().expect("the app lives as long as the page")
    }

    pub fn weak(&self) -> Weak<App> {
        self.me.borrow().clone()
    }

    pub fn uid(&self) -> String {
        new_id(&mut self.rng.borrow_mut(), dom::now())
    }

    fn on_graph(&self, ev: GraphEvent) {
        match ev {
            GraphEvent::Select(id) => self.show_inspector(id.as_deref()),
            GraphEvent::Change => {
                self.save();
                self.update_map_count();
            }
            GraphEvent::Edit(id) => {
                self.show_inspector(Some(&id));
                let t = input("inspTitle");
                let _ = t.focus();
                t.select();
            }
        }
    }

    /// Runs a change to one chat message, if it is still in the conversation.
    pub fn with_message<R>(&self, id: &str, f: impl FnOnce(&mut Message) -> R) -> Option<R> {
        self.st.borrow_mut().chat.iter_mut().find(|m| m.id == id).map(f)
    }

    pub fn message(&self, id: &str) -> Option<Message> {
        self.st.borrow().chat.iter().find(|m| m.id == id).cloned()
    }

    // ---------- persistence (the app keeps a file in Application Support, a browser uses localStorage)

    pub fn save(&self) {
        if !self.loaded.get() {
            return;
        }
        let me = self.weak();
        self.save_timer.start(250, move || {
            if let Some(a) = me.upgrade() {
                spawn_local(async move { a.flush_save().await });
            }
        });
    }

    /// Whether a save is waiting on its debounce timer.
    pub fn save_pending(&self) -> bool {
        self.save_timer.pending()
    }

    pub async fn flush_save(&self) {
        self.save_timer.clear();
        let json = {
            let mut s = self.st.borrow().clone();
            s.graph = Some(self.graph.to_data());
            serde_json::to_string(&s).unwrap_or_default()
        };
        if let Err(e) = self.platform.save_state(&json).await {
            dom::warn(&format!("save failed: {e}"));
            self.toast(&format!("Saving failed: {e}"), 6000);
        }
    }

    pub async fn load(&self) {
        match self.platform.load_state().await {
            Ok(Some(raw)) => match AppState::from_saved(&raw) {
                Ok(mut s) => {
                    let g = s.graph.take();
                    *self.st.borrow_mut() = s;
                    if let Some(g) = g {
                        self.graph.load(g);
                    }
                }
                Err(e) => self.load_failed(&e.to_string()),
            },
            Ok(None) => {}
            Err(e) => self.load_failed(&e),
        }
        self.loaded.set(true);
    }

    fn load_failed(&self, e: &str) {
        dom::warn(&format!("load failed: {e}"));
        self.toast(&format!("Could not load your saved notes: {e}"), 8000);
    }

    // ---------- toast / status

    pub fn toast(&self, text: &str, ms: i32) {
        let t = html("toast");
        t.set_text_content(Some(text));
        t.set_hidden(false);
        self.toast_timer.start(ms, move || t.set_hidden(true));
    }

    pub fn hide_toast(&self) {
        self.toast_timer.clear();
        html("toast").set_hidden(true);
    }

    pub fn set_status(&self, ok: bool, text: &str) {
        let s = html("status");
        toggle_class(&s, "ok", ok);
        toggle_class(&s, "err", !ok);
        if let Some(t) = dom::query::<HtmlElement>(&s, ".status-text") {
            t.set_text_content(Some(text));
        }
        let host = self.ollama.host();
        s.set_title(&if ok && self.ollama.is_cloud() {
            format!("Connected to {host}")
        } else if ok {
            format!("Connected to Ollama{}", if host.is_empty() { String::new() } else { format!(" at {host}") })
        } else {
            "Start Ollama.app (or run `ollama serve`); Ollama Notes reconnects automatically.".to_string()
        });
    }

    pub fn update_extract_status(&self) {
        let el = html("extractStatus");
        let n = self.extracting.get();
        if n > 0 {
            let label = self.extract_label.borrow();
            let label = if label.is_empty() { "Extracting notes into the map…" } else { label.as_str() };
            el.set_inner_html(&format!(
                "{} <button class=\"link-btn\" id=\"cancelExtract\">cancel</button>",
                esc(label)
            ));
        } else {
            el.set_inner_html("");
        }
        toggle_class(&el, "busy", n > 0);
    }

    pub fn cancel_extractions(&self) {
        let aborts = self.extract_aborts.borrow().clone();
        for a in aborts {
            a.abort(AbortReason::Cancelled);
        }
    }

    pub fn update_map_count(&self) {
        let (n, e) = self.graph.counts();
        let text = if n > 0 { format!("{} · {}", plural(n, "note"), plural(e, "link")) } else { String::new() };
        html("mapCount").set_text_content(Some(&text));
    }

    /// Recomputes keywords and keyword links, then redraws and saves if anything changed.
    pub fn refresh_keywords(&self, force: bool) -> LinkChanges {
        let r = self.graph.refresh_keywords(force);
        if r.changed() {
            self.graph.render();
            self.save();
            self.update_map_count();
        }
        r.links
    }

    // ---------- models

    fn starting_ollama(&self) -> bool {
        dom::now() - self.started_ollama_at.get() < 30_000.0
    }

    pub fn refresh_models(&self) {
        let a = self.rc();
        spawn_local(async move { a.refresh_models_now().await });
    }

    async fn refresh_models_now(&self) {
        let cloud = self.ollama.is_cloud();
        match self.ollama.tags().await {
            Ok(models) => {
                let n = models.len();
                *self.models.borrow_mut() = models;
                let source = if cloud { openai::service_name(&self.ollama.host()) } else { "Ollama".into() };
                self.set_status(
                    true,
                    &match (n, cloud) {
                        (0, false) => "Ollama · no models pulled".into(),
                        (0, true) => format!("{source} · no models"),
                        _ => format!("{source} · {}", plural(n, "model")),
                    },
                );
                if n == 0 && cloud {
                    self.toast(&format!("{source} lists no chat models."), 6000);
                } else if n == 0 {
                    self.toast("No models found. Pull one first, e.g.  ollama pull llama3.2", 6000);
                }
            }
            Err(e) if cloud => {
                self.models.borrow_mut().clear();
                self.set_status(false, "Not connected");
                html("status").set_title(&e.to_string());
                if self.reconnected.get() {
                    self.toast(&e.to_string(), 8000);
                }
            }
            Err(_) => {
                self.models.borrow_mut().clear();
                if self.reconnected.get() && self.platform.start_ollama(&self.ollama.host()).await.is_some() {
                    self.started_ollama_at.set(dom::now());
                }
                self.set_status(
                    false,
                    if self.starting_ollama() { "Starting Ollama…" } else { "Ollama not reachable" },
                );
            }
        }
        self.reconnected.set(false);
        let names: Vec<String> = self.models.borrow().iter().map(|m| m.name.clone()).collect();
        if !names.is_empty() {
            let changed = {
                let s = &mut self.st.borrow_mut().settings;
                let model_gone = !names.contains(&s.model);
                if model_gone {
                    s.model = names.iter().find(|n| !n.to_lowercase().contains("embed")).unwrap_or(&names[0]).clone();
                }
                // e.g. a model of the previous provider: extract with the chat model instead
                let extract_gone = !s.extract_model.is_empty() && !names.contains(&s.extract_model);
                if extract_gone {
                    s.extract_model.clear();
                }
                model_gone || extract_gone
            };
            if changed {
                self.save();
            }
        }
        self.fill_model_selects();
        // a cloud API's list rarely changes, and a bad key won't fix itself
        let delay = match (names.is_empty(), cloud) {
            (false, false) => 30_000,
            (false, true) => 300_000,
            (true, true) => 60_000,
            (true, false) if self.starting_ollama() => 1000,
            (true, false) => 4000,
        };
        let me = self.weak();
        self.models_timer.start(delay, move || {
            if let Some(a) = me.upgrade() {
                a.refresh_models();
            }
        });
    }

    fn model_options(&self, selected: &str, blank: Option<&str>) -> String {
        let models = self.models.borrow();
        let mut list: Vec<String> = models
            .iter()
            .map(|m| {
                let size = m.details.parameter_size.as_deref().map(|p| format!(" ({})", esc(p))).unwrap_or_default();
                let sel = if m.name == selected { " selected" } else { "" };
                format!("<option value=\"{}\"{sel}>{}{size}</option>", esc(&m.name), esc(&m.name))
            })
            .collect();
        if let Some(b) = blank {
            list.insert(
                0,
                format!("<option value=\"\"{}>{b}</option>", if selected.is_empty() { " selected" } else { "" }),
            );
        }
        if models.is_empty() && blank.is_none() {
            list.push("<option value=\"\">no models available</option>".into());
        }
        list.concat()
    }

    pub fn fill_model_selects(&self) {
        let (model, extract) = {
            let s = &self.st.borrow().settings;
            (s.model.clone(), s.extract_model.clone())
        };
        select_el("modelSelect").set_inner_html(&self.model_options(&model, None));
        select_el("setModel").set_inner_html(&self.model_options(&model, None));
        select_el("setExtractModel").set_inner_html(&self.model_options(&extract, Some("Same as chat model")));
    }

    // ---------- settings

    pub fn set_chat_width(width: f64) {
        if let Some(root) = dom::document().document_element().and_then(|e| e.dyn_into::<HtmlElement>().ok()) {
            let _ = root.style().set_property("--chat-w", &format!("{}px", num(width)));
        }
    }

    pub fn apply_settings_to_ui(&self) {
        let s = self.st.borrow().settings.clone();
        input("autoNotes").set_checked(s.auto_notes);
        input("webSearch").set_checked(s.web_search);
        textarea("setSystem").set_value(&s.system);
        input("setTemp").set_value(&num(s.temperature));
        input("setMaxNotes").set_value(&num(s.max_notes));
        input("setAutoFit").set_checked(s.auto_fit);
        input("setHost").set_value(if s.ollama_host.is_empty() { DEFAULT_OLLAMA_HOST } else { &s.ollama_host });
        let api_base = normalize_api_base(&s.api_base);
        input("setApiBase").set_value(&api_base);
        input("setApiKey").set_value(&s.api_key);
        let choice = match s.provider {
            Provider::Ollama => "ollama",
            Provider::OpenAi if openai::SERVICES.iter().any(|(_, url)| *url == api_base) => &api_base,
            Provider::OpenAi => "other",
        };
        select_el("setProvider").set_value(choice);
        self.show_provider_fields();
        Self::set_chat_width(s.chat_width);
    }

    /// The "Models from" choices: Ollama, the cloud services by their API address, and any other address.
    fn fill_provider_select(&self) {
        let services: String = openai::SERVICES
            .iter()
            .map(|(name, url)| format!("<option value=\"{}\">{}</option>", esc(url), esc(name)))
            .collect();
        select_el("setProvider").set_inner_html(&format!(
            "<option value=\"ollama\">Ollama</option>{services}<option value=\"other\">Another OpenAI-compatible API</option>"
        ));
    }

    /// Shows the Settings fields for the chosen provider.
    fn show_provider_fields(&self) {
        let ollama = select_el("setProvider").value() == "ollama";
        html("setHostRow").set_hidden(!ollama || !self.platform.is_desktop());
        html("setProxyNote").set_hidden(!ollama || self.platform.is_desktop());
        html("setApiBaseRow").set_hidden(ollama);
        html("setApiKeyRow").set_hidden(ollama);
    }

    /// A choice in "Models from": a cloud service fills in its API address.
    pub fn provider_chosen(&self) {
        let choice = select_el("setProvider").value();
        if openai::SERVICES.iter().any(|(_, url)| *url == choice) {
            input("setApiBase").set_value(&choice);
        }
        self.show_provider_fields();
    }

    /// Points the client at the models chosen in Settings. In browser mode, Ollama is the page's own server. Web
    /// search goes through Ollama, so only its models get the toggle.
    fn connect(&self) {
        let s = self.st.borrow().settings.clone();
        html("webSearchToggle").set_hidden(s.provider != Provider::Ollama);
        match s.provider {
            Provider::OpenAi => self.ollama.set_host(&normalize_api_base(&s.api_base), Some(&s.api_key)),
            Provider::Ollama if self.platform.is_desktop() => {
                self.ollama.set_host(&normalize_host(&s.ollama_host), None)
            }
            Provider::Ollama => self.ollama.set_host("", None),
        }
    }

    pub fn read_settings_from_dialog(&self) {
        let host = normalize_host(&input("setHost").value());
        let provider = if select_el("setProvider").value() == "ollama" { Provider::Ollama } else { Provider::OpenAi };
        let api_base = normalize_api_base(&input("setApiBase").value());
        let api_key = input("setApiKey").value().trim().to_string();
        let before = self.connection();
        {
            let mut st = self.st.borrow_mut();
            let s = &mut st.settings;
            let model = select_el("setModel").value();
            if !model.is_empty() {
                s.model = model;
            }
            s.extract_model = select_el("setExtractModel").value();
            s.system = textarea("setSystem").value();
            s.temperature = num_or(&input("setTemp").value(), 0.0).clamp(0.0, 2.0);
            s.max_notes = num_or(&input("setMaxNotes").value(), 4.0).clamp(1.0, 8.0);
            s.auto_fit = input("setAutoFit").checked();
            if self.platform.is_desktop() {
                s.ollama_host = host;
            }
            s.provider = provider;
            s.api_base = api_base;
            s.api_key = api_key;
        }
        if self.connection() != before {
            self.reconnect();
        }
        self.fill_model_selects();
        self.save();
    }

    /// The settings `connect` uses, to tell when they change.
    fn connection(&self) -> (Provider, String, String, String) {
        let s = &self.st.borrow().settings;
        (s.provider, s.ollama_host.clone(), s.api_base.clone(), s.api_key.clone())
    }

    fn reconnect(&self) {
        self.connect();
        self.reconnected.set(true);
        self.set_status(false, "connecting…");
        self.refresh_models();
    }

    // ---------- export / import

    pub async fn export_json(&self) {
        let iso = dom::iso_now();
        let text = {
            let mut s = self.st.borrow().clone();
            s.graph = Some(self.graph.to_data());
            s.to_export(&iso)
        };
        let name = format!("ollama-notes-{}.json", &iso[..iso.len().min(10)]);
        match self.platform.save_file(&name, &text).await {
            Ok(Some(path)) => self.toast(&format!("Exported to {path}"), 2800),
            Ok(None) => {}
            Err(e) => self.toast(&format!("Export failed: {e}"), 5000),
        }
    }

    pub async fn import_json(&self, text: &str) {
        let current = self.st.borrow().settings.clone();
        let mut state = match AppState::from_export(text, &current) {
            Ok(s) => s,
            Err(e) => return self.toast(&format!("Import failed: {e}"), 5000),
        };
        if self.graph.counts().0 > 0
            && !self.platform.confirm("Replace the current map and conversation with the imported file?").await
        {
            return;
        }
        let graph = state.graph.take().unwrap_or_default();
        let before = self.connection();
        *self.st.borrow_mut() = state;
        if self.connection() != before {
            self.reconnect();
        }
        self.graph.load(graph);
        self.apply_settings_to_ui();
        self.fill_model_selects();
        self.render_chat();
        self.render_context_bar();
        self.update_map_count();
        self.show_inspector(None);
        self.sync_physics_ui();
        self.sync_keyword_ui();
        self.refresh_keywords(false);
        let g = self.graph.clone();
        dom::request_frame(move |_| g.fit(false));
        self.save();
        self.toast("Imported", 2800);
    }

    pub async fn clear_map(&self) {
        if self.graph.counts().0 == 0 {
            return;
        }
        if !self.platform.confirm("Delete every note and link in the map? (Export first if you want a backup.)").await {
            return;
        }
        self.graph.clear();
        {
            let mut st = self.st.borrow_mut();
            st.hub_id = None;
            st.context_node_ids.clear();
            for m in &mut st.chat {
                m.node_ids = Some(Vec::new());
            }
        }
        self.render_chat();
        self.render_context_bar();
        self.save();
    }

    // ---------- start-up

    pub async fn start(&self) {
        self.load().await;
        self.connect();
        self.fill_provider_select();
        self.apply_settings_to_ui();
        self.render_chat();
        self.render_context_bar();
        self.update_map_count();
        self.graph.render();
        self.sync_physics_ui();
        self.sync_keyword_ui();
        self.refresh_keywords(false);
        self.refresh_models();
        let g = self.graph.clone();
        dom::request_frame(move |_| {
            let v = g.view();
            if g.counts().0 > 0 && v.x == 0.0 && v.y == 0.0 {
                g.fit(false);
            }
        });
        let _ = textarea("input").focus();
    }
}
