//! The inspector (the selected note's details), the chat's pinned-context bar, and the map's physics and keyword
//! popovers.

use crate::app::{App, html, input, select_el, textarea};
use crate::dom::{self, esc, toggle_class};
use web_sys::HtmlElement;

impl App {
    pub fn show_inspector(&self, id: Option<&str>) {
        *self.insp_id.borrow_mut() = id.map(str::to_string);
        let insp = html("inspector");
        let Some(n) = id.and_then(|id| self.graph.node(id)) else {
            insp.set_hidden(true);
            self.graph.set_pad_right(0.0);
            return;
        };
        insp.set_hidden(false);
        self.graph.set_pad_right(340.0);
        input("inspTitle").set_value(&n.title);
        select_el("inspKind").set_value(&n.kind);
        textarea("inspBody").set_value(&n.body);
        let badge = html("inspKindBadge");
        badge.set_text_content(Some(&n.kind));
        badge.set_class_name(&format!("badge kind-{}", n.kind));
        let (pinned, has_source) = {
            let st = self.st.borrow();
            let src = n.source.as_ref().and_then(|s| s.msg_id.as_deref());
            (st.context_node_ids.contains(&n.id), src.is_some_and(|m| st.chat.iter().any(|c| c.id == m)))
        };
        let pin = html("inspPin");
        pin.set_text_content(Some(if pinned { "Unpin from chat" } else { "Pin to chat context" }));
        toggle_class(&pin, "active", pinned);
        html("inspSource").set_hidden(!has_source);
        let lock = html("inspLock");
        lock.set_text_content(Some(if n.is_locked() { "Unlock position" } else { "Lock position" }));
        toggle_class(&lock, "active", n.is_locked());
        let kw = input("inspKeywords");
        let focused = dom::document().active_element().is_some_and(|a| a.id() == "inspKeywords");
        if !focused {
            kw.set_value(&n.keywords().join(", "));
        }
        kw.set_placeholder(if n.is_manual() {
            "comma, separated, keywords"
        } else {
            "auto: comma, separated, keywords"
        });
        html("inspKwAuto").set_hidden(!n.is_manual());
        let links: String = self.graph.with(|g| {
            g.neighbors(&n.id)
                .into_iter()
                .map(|(e, other)| {
                    let title = if e.is_keyword() { format!("Keyword link: {}", esc(&e.label)) } else { "Link".into() };
                    format!(
                        "<span class=\"chip kind-{}{}\" data-node=\"{}\" title=\"{title}\">{}<button class=\"chip-x\" data-edge=\"{}\" title=\"Remove link\">×</button></span>",
                        esc(&other.kind),
                        if e.is_keyword() { " chip-kw" } else { "" },
                        esc(&other.id),
                        esc(&other.title),
                        esc(&e.id)
                    )
                })
                .collect()
        });
        html("inspLinks").set_inner_html(if links.is_empty() {
            "<span class=\"muted\">None yet. Drag the handle on a note's right edge onto another note to link them.</span>"
        } else {
            &links
        });
        let mut meta = format!("Created {}", dom::locale_date_time(n.created_at));
        if let Some(t) = n.doc_text() {
            meta.push_str(&format!(
                " · document text: {} characters (pin to use as chat context)",
                dom::locale_number(notes_core::text::char_len(t))
            ));
        }
        if let Some(file) = n.source.as_ref().and_then(|s| s.file.as_deref()) {
            let section = n
                .source
                .as_ref()
                .and_then(|s| s.section)
                .map(|s| format!(", section {}", dom::num(s)))
                .unwrap_or_default();
            meta.push_str(&format!(" · from {file}{section}"));
        }
        html("inspMeta").set_text_content(Some(&meta));
    }

    pub fn refresh_inspector(&self) {
        let id = self.insp_id.borrow().clone();
        if let Some(id) = id {
            self.show_inspector(Some(&id));
        }
    }

    pub fn render_context_bar(&self) {
        let bar = html("contextBar");
        let nodes: Vec<_> = {
            let ids = self.st.borrow().context_node_ids.clone();
            ids.iter().filter_map(|id| self.graph.node(id)).collect()
        };
        self.st.borrow_mut().context_node_ids = nodes.iter().map(|n| n.id.clone()).collect();
        bar.set_hidden(nodes.is_empty());
        let chips: String = nodes
            .iter()
            .map(|n| {
                format!(
                    "<span class=\"chip kind-{}\" data-node=\"{}\">{}<button class=\"chip-x\" data-unpin=\"{}\" title=\"Unpin\">×</button></span>",
                    esc(&n.kind),
                    esc(&n.id),
                    esc(&n.title),
                    esc(&n.id)
                )
            })
            .collect();
        bar.set_inner_html(&format!("<span class=\"label\">Context:</span>{chips}"));
    }

    pub fn toggle_pin(&self, id: &str) {
        {
            let mut st = self.st.borrow_mut();
            match st.context_node_ids.iter().position(|x| x == id) {
                Some(i) => {
                    st.context_node_ids.remove(i);
                }
                None => st.context_node_ids.push(id.to_string()),
            }
        }
        self.render_context_bar();
        if self.insp_id.borrow().as_deref() == Some(id) {
            self.show_inspector(Some(id));
        }
        self.save();
    }

    /// Keywords are recomputed a moment after typing stops.
    pub fn schedule_keywords(&self) {
        let me = self.weak();
        self.kw_timer.start(900, move || {
            if let Some(a) = me.upgrade() {
                a.kw_timer.forget();
                a.refresh_keywords(false);
                a.refresh_inspector();
            }
        });
    }

    pub fn sync_physics_ui(&self) {
        let p = self.graph.physics();
        input("physicsToggle").set_checked(p.enabled);
        for (id, v) in [("phTension", p.tension), ("phGravity", p.gravity), ("phRepulsion", p.repulsion)] {
            let el = input(id);
            el.set_value(&dom::num(v));
            set_slider_label(&el, v);
            el.set_disabled(!p.enabled);
        }
    }

    pub fn sync_keyword_ui(&self) {
        let k = self.graph.keyword_links();
        input("kwEnabled").set_checked(k.enabled);
        let t = input("kwThreshold");
        t.set_value(&dom::num(k.threshold));
        set_slider_label(&t, k.threshold);
        t.set_disabled(!k.enabled);
    }

    pub fn close_popovers(&self) {
        for p in dom::query_all(&dom::document().document_element().expect("html"), ".popover") {
            if let Ok(p) = wasm_bindgen::JsCast::dyn_into::<HtmlElement>(p) {
                p.set_hidden(true);
            }
        }
    }

    /// Opens a popover (closing the others), or closes it if it was open. Returns whether it is now open.
    pub fn toggle_popover(&self, id: &str) -> bool {
        let pop = html(id);
        let open = pop.hidden();
        self.close_popovers();
        pop.set_hidden(!open);
        open
    }
}

/// The value shown next to a slider ("1.0").
pub fn set_slider_label(el: &web_sys::HtmlInputElement, v: f64) {
    if let Some(label) = el.next_element_sibling() {
        label.set_text_content(Some(&format!("{v:.1}")));
    }
}
