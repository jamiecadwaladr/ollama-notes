//! The conversation: rendering messages, streaming a reply, and turning exchanges into notes.

use crate::app::{App, html, plural, textarea};
use crate::dom::{self, esc};
use crate::ollama::{Abort, AbortReason, Error};
use notes_core::extract::{self, CHAT_EXTRACT_SYSTEM, EXTRACT_TIMEOUT_MS};
use notes_core::markdown;
use notes_core::model::{DEFAULT_TITLE, Message, Node, Source};
use notes_core::ollama::ChatMessage;
use notes_core::text::{clean, truncate_chars};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlButtonElement};

impl App {
    pub fn render_chat(&self) {
        let list = html("messages");
        list.set_inner_html("");
        let chat = self.st.borrow().chat.clone();
        for m in &chat {
            let _ = list.append_child(&self.message_el(m));
        }
        list.set_scroll_top(list.scroll_height());
        html("chatTitle").set_text_content(Some(&self.st.borrow().title));
    }

    fn message_el(&self, m: &Message) -> Element {
        let el = dom::document().create_element("article").expect("article");
        el.set_class_name(&format!("msg {}", m.role));
        let _ = el.set_attribute("data-id", &m.id);
        el.set_inner_html(&format!(
            "<div class=\"msg-role\">{}</div><div class=\"msg-body\"></div><div class=\"msg-foot\"></div>",
            if m.role == "user" { "You" } else { "Model" }
        ));
        self.update_message_el(&el, m);
        el
    }

    pub fn message_el_by_id(&self, id: &str) -> Option<Element> {
        dom::query(&html("messages"), &format!(".msg[data-id=\"{}\"]", esc(id)))
    }

    pub fn update_message_el(&self, el: &Element, m: &Message) {
        let mut body = String::new();
        if let Some(t) = m.thinking.as_deref().filter(|t| !t.is_empty()) {
            body.push_str(&format!(
                "<details class=\"thinking\"><summary>Thinking</summary><div>{}</div></details>",
                markdown::render(t)
            ));
        }
        if let Some(e) = m.error.as_deref().filter(|e| !e.is_empty()) {
            body.push_str(&format!("<div class=\"msg-error\">{}</div>", esc(e)));
        }
        if m.role == "assistant" {
            body.push_str(&markdown::render(&m.content));
        } else {
            body.push_str(&format!("<p>{}</p>", esc(&m.content).replace('\n', "<br>")));
        }
        if m.is_streaming() {
            body.push_str("<span class=\"cursor\"></span>");
        }
        if let Some(b) = dom::query::<Element>(el, ".msg-body") {
            b.set_inner_html(&body);
        }
        let Some(foot) = dom::query::<Element>(el, ".msg-foot") else { return };
        foot.set_inner_html("");
        if m.role == "assistant" && !m.is_streaming() && !m.content.is_empty() {
            let chips: String = m
                .node_ids
                .iter()
                .flatten()
                .filter_map(|id| self.graph.node(id))
                .map(|n| {
                    format!(
                        "<button class=\"chip kind-{}\" data-node=\"{}\" title=\"Show in map\">{}</button>",
                        esc(&n.kind),
                        esc(&n.id),
                        esc(&n.title)
                    )
                })
                .collect();
            foot.set_inner_html(&format!("<div class=\"chips\">{chips}</div><div class=\"msg-actions\"><button class=\"link-btn\" data-act=\"extract\" title=\"Ask the model to pull notes from this reply\">✦ Extract notes</button><button class=\"link-btn\" data-act=\"note\" title=\"Save this whole reply as one note\">＋ Save as note</button></div>"));
        }
    }

    pub fn refresh_message(&self, id: &str) {
        if let (Some(el), Some(m)) = (self.message_el_by_id(id), self.message(id)) {
            self.update_message_el(&el, &m);
        }
    }

    pub fn scroll_bottom(&self, force: bool) {
        let list = html("messages");
        let near = list.scroll_height() - list.scroll_top() - list.client_height() < 120;
        if force || near {
            list.set_scroll_top(list.scroll_height());
        }
    }

    fn set_busy(&self, busy: bool) {
        dom::by_id::<HtmlButtonElement>("sendBtn").set_disabled(busy);
        html("stopBtn").set_hidden(!busy);
        textarea("input").set_disabled(false);
    }

    pub fn autoresize(&self) {
        let ta = textarea("input");
        let _ = ta.style().set_property("height", "auto");
        let _ = ta.style().set_property("height", &format!("{}px", ta.scroll_height().min(220)));
    }

    fn build_messages(&self) -> Vec<ChatMessage> {
        let st = self.st.borrow();
        let pinned: Vec<Node> = st.context_node_ids.iter().filter_map(|id| self.graph.node(id)).collect();
        extract::build_messages(&st.settings.system, &pinned.iter().collect::<Vec<_>>(), &st.chat)
    }

    /// Repaints the streaming reply at the next animation frame (at most once per frame).
    fn schedule_paint(&self, el: &Element, id: &str) {
        if self.paint_frame.get().is_some() {
            return;
        }
        let (a, el, id) = (self.rc(), el.clone(), id.to_string());
        self.paint_frame.set(Some(dom::request_frame(move |_| {
            a.paint_frame.set(None);
            if let Some(m) = a.message(&id) {
                a.update_message_el(&el, &m);
            }
            a.scroll_bottom(false);
        })));
    }

    pub async fn send(&self) {
        let ta = textarea("input");
        let text = ta.value().trim().to_string();
        if text.is_empty() || self.chat_abort.borrow().is_some() {
            return;
        }
        let model = self.st.borrow().settings.model.clone();
        if model.is_empty() {
            self.toast("No model selected. Is Ollama running with a model pulled?", 2800);
            return;
        }
        ta.set_value("");
        self.autoresize();
        let now = dom::now();
        let user =
            Message { id: self.uid(), role: "user".into(), content: text.clone(), ts: now, ..Message::default() };
        let asst = Message {
            id: self.uid(),
            role: "assistant".into(),
            thinking: Some(String::new()),
            ts: now,
            streaming: Some(true),
            node_ids: Some(Vec::new()),
            ..Message::default()
        };
        let asst_id = asst.id.clone();
        {
            let mut st = self.st.borrow_mut();
            st.chat.push(user.clone());
            st.chat.push(asst.clone());
            if st.chat.len() == 2 && st.title == DEFAULT_TITLE {
                st.title = truncate_chars(&text, 60).to_string();
            }
        }
        html("chatTitle").set_text_content(Some(&self.st.borrow().title));
        let list = html("messages");
        let _ = list.append_child(&self.message_el(&user));
        let asst_el = self.message_el(&asst);
        let _ = list.append_child(&asst_el);
        self.scroll_bottom(true);
        let abort = Abort::new();
        *self.chat_abort.borrow_mut() = Some(abort.clone());
        self.set_busy(true);

        let messages = self.build_messages();
        let temperature = Some(self.st.borrow().settings.temperature).filter(|t| t.is_finite()).unwrap_or(0.0);
        // the reply so far, kept here too: a new chat removes the message from the conversation mid-stream
        let reply = RefCell::new(String::new());
        let result = self
            .ollama
            .chat_stream(&model, &messages, temperature, &abort, |s| {
                reply.replace(s.content.clone());
                self.with_message(&asst_id, |m| {
                    m.content = s.content.clone();
                    m.thinking = Some(s.thinking.clone());
                });
                self.schedule_paint(&asst_el, &asst_id);
            })
            .await;
        let error = match result {
            Ok(_) => None,
            Err(Error::Aborted) => reply.borrow().is_empty().then(|| "Stopped.".to_string()),
            Err(Error::Failed(e)) => Some(e),
        };
        if let Some(f) = self.paint_frame.take() {
            dom::cancel_frame(f);
        }
        let done = self.with_message(&asst_id, |m| {
            m.streaming = Some(false);
            if error.is_some() {
                m.error = error.clone();
            }
            m.clone()
        });
        *self.chat_abort.borrow_mut() = None;
        self.set_busy(false);
        let done = done.unwrap_or(Message { streaming: Some(false), error, content: reply.borrow().clone(), ..asst });
        self.update_message_el(&asst_el, &done);
        self.scroll_bottom(false);
        self.save();
        let content = reply.into_inner();
        if !content.is_empty() && self.st.borrow().settings.auto_notes {
            self.spawn_extraction(user.content, asst_id, content);
        }
    }

    pub fn stop_reply(&self) {
        let abort = self.chat_abort.borrow().clone();
        if let Some(a) = abort {
            a.abort(AbortReason::Stopped);
        }
    }

    /// Note extraction in the background, with failures as a toast.
    pub fn spawn_extraction(&self, user: String, asst_id: String, reply: String) {
        let a = self.rc();
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(m) = a.extract_notes(&user, &asst_id, &reply).await {
                let text = if m.starts_with("Note extraction") { m } else { format!("Note extraction failed: {m}") };
                a.toast(&text, 6000);
            }
        });
    }

    /// Extraction options for a model: thinking off where supported, so the whole budget goes to the JSON.
    pub async fn extraction_think(&self, model: &str) -> Option<bool> {
        self.ollama.capabilities(model).await.iter().any(|c| c == "thinking").then_some(false)
    }

    async fn extract_notes(self: &Rc<Self>, user: &str, asst_id: &str, reply: &str) -> Result<(), String> {
        let model = self.st.borrow().settings.extraction_model().to_string();
        if model.is_empty() {
            return Ok(());
        }
        // Small models can loop forever in constrained-JSON mode, so cap the output and time out.
        let abort = Abort::new();
        let timer = {
            let a = abort.clone();
            dom::set_timeout(EXTRACT_TIMEOUT_MS as i32, move || a.abort(AbortReason::Timeout))
        };
        self.extract_aborts.borrow_mut().push(abort.clone());
        self.extracting.set(self.extracting.get() + 1);
        self.update_extract_status();
        let result = async {
            let (hub_title, max) = {
                let st = self.st.borrow();
                (
                    st.hub_id.as_deref().and_then(|id| self.graph.node(id)).map(|n| n.title),
                    st.settings.max_notes_clamped(),
                )
            };
            let existing = self.graph.with(extract::existing_titles);
            let think = self.extraction_think(&model).await;
            let messages = [
                ChatMessage::new("system", CHAT_EXTRACT_SYSTEM),
                ChatMessage::new("user", extract::chat_prompt(hub_title.as_deref(), &existing, user, reply, max)),
            ];
            let v = self
                .ollama
                .chat_json(&model, &messages, &extract::note_schema(), &extract::extract_options(), think, &abort)
                .await?;
            self.apply_extraction(&v, asst_id);
            Ok::<(), Error>(())
        }
        .await;
        dom::clear_timeout(timer);
        self.extract_aborts.borrow_mut().retain(|a| !a.same(&abort));
        self.extracting.set(self.extracting.get() - 1);
        self.update_extract_status();
        result.map_err(|e| match abort.reason() {
            Some(r) => format!(
                "Note extraction {}. Try \"Extract notes\" again, or pick a smaller/other extraction model in Settings.",
                if r == AbortReason::Timeout { "timed out" } else { "cancelled" }
            ),
            None => e.to_string(),
        })
    }

    fn apply_extraction(&self, result: &Value, asst_id: &str) {
        let mut created = Vec::new();
        let hub = self.st.borrow().hub_id.clone().filter(|id| self.graph.node(id).is_some());
        let hub = match hub {
            Some(h) => h,
            None => {
                let topic = {
                    let st = self.st.borrow();
                    let t = clean(result.get("topic").and_then(Value::as_str).unwrap_or(""));
                    if !t.is_empty() {
                        t
                    } else if !st.title.is_empty() {
                        st.title.clone()
                    } else {
                        "Conversation".into()
                    }
                };
                let (x, y) = self.graph.place_near(None);
                let node = Node {
                    title: topic.clone(),
                    kind: "topic".into(),
                    body: format!("Conversation started {}", dom::locale_date_time(dom::now())),
                    source: Some(Source { msg_id: Some(asst_id.into()), ..Source::default() }),
                    ..self.graph.new_node(x, y)
                };
                let id = self.graph.add_node(node);
                {
                    let mut st = self.st.borrow_mut();
                    st.hub_id = Some(id.clone());
                    st.title = topic.clone();
                }
                html("chatTitle").set_text_content(Some(&topic));
                created.push(id.clone());
                id
            }
        };
        let r = self.graph.apply_notes(result, &hub, &Source { msg_id: Some(asst_id.into()), ..Source::default() });
        created.extend(r.created);
        self.with_message(asst_id, |m| {
            let ids = m.node_ids.get_or_insert_with(Vec::new);
            for id in &r.nodes {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        });
        let kw = self.refresh_keywords(false);
        self.graph.render();
        for id in &created {
            self.graph.flash(id);
        }
        self.refresh_message(asst_id);
        self.save();
        self.update_map_count();
        if !created.is_empty() {
            let links = if kw.added > 0 { format!(", {}", plural(kw.added, "keyword link")) } else { String::new() };
            self.toast(&format!("{} added to the map{links}", plural(created.len(), "note")), 2800);
            if self.st.borrow().settings.auto_fit {
                self.graph.fit(true);
                self.graph.set_fit_on_settle();
            }
        } else {
            self.toast("No new notes from this exchange", 2800);
        }
    }

    /// "Save as note": the whole reply as one note.
    pub fn note_from_message(&self, msg_id: &str) {
        let Some(m) = self.message(msg_id) else { return };
        let hub = self.st.borrow().hub_id.clone();
        let (x, y) = self.graph.place_near(hub.as_deref());
        let node = Node {
            title: extract::note_title_from_reply(&m.content),
            kind: "note".into(),
            body: truncate_chars(&m.content, 2000).to_string(),
            source: Some(Source { msg_id: Some(m.id.clone()), ..Source::default() }),
            ..self.graph.new_node(x, y)
        };
        let id = self.graph.add_node(node);
        if let Some(h) = &hub {
            self.graph.add_edge(h, &id);
        }
        self.with_message(msg_id, |m| m.node_ids.get_or_insert_with(Vec::new).push(id.clone()));
        self.refresh_keywords(false);
        self.graph.render();
        self.graph.flash(&id);
        self.graph.select(Some(&id));
        self.graph.center_on(&id);
        self.refresh_message(msg_id);
        self.save();
    }

    /// The click on a reply's "Extract notes": the reply with the user message before it.
    pub fn extract_from_message(&self, msg_id: &str) {
        let (user, reply) = {
            let st = self.st.borrow();
            let Some(i) = st.chat.iter().position(|m| m.id == msg_id) else { return };
            let user =
                st.chat[..i].iter().rev().find(|m| m.role == "user").map(|m| m.content.clone()).unwrap_or_default();
            (user, st.chat[i].content.clone())
        };
        self.spawn_extraction(user, msg_id.to_string(), reply);
    }

    pub fn new_chat(&self) {
        self.stop_reply();
        self.cancel_extractions();
        {
            let mut st = self.st.borrow_mut();
            st.chat.clear();
            st.hub_id = None;
            st.title = DEFAULT_TITLE.into();
        }
        self.render_chat();
        self.save();
        let _ = textarea("input").focus();
    }

    /// Scrolls to a note's source message and highlights it.
    pub fn show_source(&self, msg_id: &str) {
        let Some(el) = self.message_el_by_id(msg_id) else { return };
        let opts = web_sys::ScrollIntoViewOptions::new();
        opts.set_behavior(web_sys::ScrollBehavior::Smooth);
        opts.set_block(web_sys::ScrollLogicalPosition::Center);
        el.scroll_into_view_with_scroll_into_view_options(&opts);
        let _ = el.class_list().add_1("highlight");
        dom::set_timeout(1600, move || {
            let _ = el.class_list().remove_1("highlight");
        });
    }

    /// Links in replies open outside the app.
    pub fn open_link(&self, a: &Element) -> bool {
        let Some(href) = a.dyn_ref::<web_sys::HtmlAnchorElement>().map(|a| a.href()) else { return false };
        self.platform.open_url(&href);
        true
    }
}
