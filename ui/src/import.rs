//! Importing documents: each file becomes a Document hub note, and its text is sent to the model section by
//! section to extract notes linked to the hub.

use crate::app::{App, el, html, input, plural};
use crate::documents::{self, Doc};
use crate::dom::{self, esc, locale_number, num_or};
use crate::ollama::{Abort, AbortReason};
use crate::platform::FileData;
use js_sys::Promise;
use notes_core::extract::{self, DOC_EXTRACT_SYSTEM, EXTRACT_TIMEOUT_MS};
use notes_core::model::{Node, Source};
use notes_core::ollama::ChatMessage;
use notes_core::text::{self, char_len, truncate_chars};
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlDialogElement;

/// The choices in the import dialog.
#[derive(Clone, Copy)]
pub struct ImportOptions {
    pub extract: bool,
    pub max_sections: usize,
    pub chunk_size: usize,
    pub keep_text: bool,
}

fn chunk_size_field() -> usize {
    num_or(&input("docChunkSize").value(), 2400.0).max(800.0) as usize
}

fn max_sections_field() -> usize {
    num_or(&input("docMaxSections").value(), 20.0).max(1.0) as usize
}

impl App {
    pub async fn import_files(self: &Rc<Self>, files: Vec<FileData>) {
        if files.is_empty() {
            return;
        }
        let mut docs = Vec::new();
        for f in files {
            let name = f.name.clone();
            self.toast(&format!("Reading {name}…"), 120_000);
            let (a, n) = (self.weak(), name.clone());
            let progress = move |page: u32, pages: u32| {
                if let Some(a) = a.upgrade() {
                    a.toast(&format!("Reading {n}: page {page} of {pages}…"), 120_000);
                }
            };
            match documents::parse(f, progress).await {
                Ok(d) if d.text.is_empty() => {
                    self.toast(&format!("{name}: no text found (a scanned PDF needs OCR first)"), 6000)
                }
                Ok(d) => docs.push(d),
                Err(e) => self.toast(&format!("{name}: {e}"), 6000),
            }
        }
        self.hide_toast();
        if docs.is_empty() {
            return;
        }
        let Some(opts) = self.ask_import_options(&docs).await else { return };
        for d in docs {
            self.import_document(d, opts).await;
        }
    }

    async fn ask_import_options(&self, docs: &[Doc]) -> Option<ImportOptions> {
        let dlg: HtmlDialogElement = el("docDialog");
        html("docTitle").set_text_content(Some(&if docs.len() == 1 {
            format!("Import \"{}\"", docs[0].name)
        } else {
            format!("Import {} documents", docs.len())
        }));
        let list: String = docs
            .iter()
            .map(|d| {
                let pages = d.pages.map(|p| format!("{p} pages · ")).unwrap_or_default();
                format!(
                    "<li><span>{}</span><span>{pages}{} chars</span></li>",
                    esc(&d.name),
                    locale_number(char_len(&d.text))
                )
            })
            .collect();
        html("docList").set_inner_html(&list);
        let model = self.st.borrow().settings.extraction_model().to_string();
        let texts: Rc<Vec<String>> = Rc::new(docs.iter().map(|d| d.text.clone()).collect());
        let update_hint = move || {
            let (size, max) = (chunk_size_field(), max_sections_field());
            let sections: Vec<usize> = texts.iter().map(|t| text::chunk_text(t, size).len()).collect();
            let total: usize = sections.iter().map(|&b| b.min(max)).sum();
            let skipped: usize = sections.iter().map(|&b| b.saturating_sub(max)).sum();
            let hint = if input("docExtract").checked() {
                let skipped = if skipped > 0 {
                    format!(" {} beyond the limit will be skipped.", plural(skipped, "section"))
                } else {
                    String::new()
                };
                format!(
                    "{} will be sent to {}, one call each (roughly {} min at ~8 s per section).{skipped}",
                    plural(total, "section"),
                    if model.is_empty() { "the model" } else { &model },
                    (total * 8).div_ceil(60)
                )
            } else {
                "Only a document note will be created; no notes are extracted.".to_string()
            };
            html("docHint").set_text_content(Some(&hint));
        };
        update_hint();
        let handler = Closure::<dyn FnMut()>::new(update_hint).into_js_value();
        for id in ["docExtract", "docMaxSections", "docChunkSize"] {
            input(id).set_oninput(Some(handler.unchecked_ref()));
        }
        let closed = Promise::new(&mut |resolve, _| {
            let dlg2 = dlg.clone();
            let on_close = Closure::once_into_js(move || {
                let _ = resolve.call1(&JsValue::NULL, &dlg2.return_value().into());
            });
            dlg.set_onclose(Some(on_close.unchecked_ref()));
        });
        dlg.set_return_value("");
        let _ = dlg.show_modal();
        let rv = JsFuture::from(closed).await.ok().and_then(|v| v.as_string()).unwrap_or_default();
        (rv == "ok").then(|| ImportOptions {
            extract: input("docExtract").checked(),
            max_sections: max_sections_field(),
            chunk_size: chunk_size_field(),
            keep_text: input("docKeepText").checked(),
        })
    }

    async fn import_document(self: &Rc<Self>, d: Doc, opts: ImportOptions) {
        let title = text::title_from_filename(&d.name);
        let pages = d.pages.map(|p| format!("{p} pages · ")).unwrap_or_default();
        let meta = format!(
            "{pages}{} characters · imported {}",
            locale_number(char_len(&d.text)),
            dom::locale_date(dom::now())
        );
        let existing = self.graph.with(|g| g.find_by_title(&title).map(|n| n.id.clone()));
        let hub = match existing {
            Some(id) => {
                let (text, meta) = (opts.keep_text.then(|| d.text.clone()), meta.clone());
                self.graph.update_node(&id, |n| {
                    if text.is_some() {
                        n.text = text;
                    }
                    if n.body.is_empty() {
                        n.body = meta;
                    }
                });
                id
            }
            None => {
                let (x, y) = self.graph.place_near(None);
                let preview =
                    format!("{}{}", truncate_chars(&d.text, 500), if char_len(&d.text) > 500 { "…" } else { "" });
                let node = Node {
                    title: title.clone(),
                    kind: "source".into(),
                    body: format!("{meta}\n\n{preview}"),
                    text: opts.keep_text.then(|| d.text.clone()),
                    source: Some(Source { file: Some(d.name.clone()), ..Source::default() }),
                    ..self.graph.new_node(x, y)
                };
                let id = self.graph.add_node(node);
                self.graph.flash(&id);
                id
            }
        };
        self.save();
        self.update_map_count();
        if !opts.extract {
            self.refresh_keywords(false);
            self.graph.render();
            self.toast(&format!("Added document note \"{title}\""), 2800);
            return;
        }

        let model = self.st.borrow().settings.extraction_model().to_string();
        if model.is_empty() {
            self.toast(
                "No model selected, so no notes were extracted. Pick a model and use \"Extract notes\" later.",
                6000,
            );
            return;
        }
        let mut sections = text::chunk_text(&d.text, opts.chunk_size);
        sections.truncate(opts.max_sections);
        let existing_before = self.graph.counts().0;
        let abort = Abort::new();
        self.extract_aborts.borrow_mut().push(abort.clone());
        self.extracting.set(self.extracting.get() + 1);
        let (mut failed, mut created_total) = (0, 0);
        let think = self.extraction_think(&model).await;
        let options = self.extraction_options(&model);
        for (i, section) in sections.iter().enumerate() {
            if abort.is_aborted() {
                break;
            }
            *self.extract_label.borrow_mut() =
                format!("Extracting notes from {}: section {} of {}…", d.name, i + 1, sections.len());
            self.update_extract_status();
            let timer = {
                let a = abort.clone();
                dom::set_timeout(EXTRACT_TIMEOUT_MS as i32, move || a.abort(AbortReason::Timeout))
            };
            let existing = self.graph.with(extract::existing_titles);
            let max = self.st.borrow().settings.max_notes_clamped();
            let messages = [
                ChatMessage::new("system", DOC_EXTRACT_SYSTEM),
                ChatMessage::new("user", extract::doc_prompt(&title, i + 1, sections.len(), &existing, section, max)),
            ];
            let result =
                self.ollama.chat_json(&model, &messages, &extract::note_schema(), &options, think, &abort).await;
            dom::clear_timeout(timer);
            match result {
                Ok(v) => {
                    let source =
                        Source { file: Some(d.name.clone()), section: Some((i + 1) as f64), ..Source::default() };
                    let r = self.graph.apply_notes(&v, &hub, &source);
                    created_total += r.created.len();
                    self.graph.render();
                    for id in &r.created {
                        self.graph.flash(id);
                    }
                    self.save();
                    self.update_map_count();
                }
                Err(_) if abort.is_aborted() => break,
                Err(e) => {
                    failed += 1;
                    dom::warn(&format!("section {} of {}: {e}", i + 1, d.name));
                }
            }
        }
        self.extract_aborts.borrow_mut().retain(|a| !a.same(&abort));
        self.extracting.set(self.extracting.get() - 1);
        self.extract_label.borrow_mut().clear();
        self.update_extract_status();
        let kw = self.refresh_keywords(false);
        self.graph.render();
        self.save();
        self.update_map_count();
        let why = match abort.reason() {
            Some(AbortReason::Timeout) => " (stopped: a section timed out)",
            Some(_) => " (cancelled)",
            None => "",
        };
        let failed = if failed > 0 { format!(", {failed} failed") } else { String::new() };
        let links = if kw.added > 0 { format!(", {}", plural(kw.added, "keyword link")) } else { String::new() };
        self.toast(
            &format!(
                "{}: {} added from {}{failed}{links}{why}",
                d.name,
                plural(created_total, "note"),
                plural(sections.len(), "section")
            ),
            7000,
        );
        if self.st.borrow().settings.auto_fit && self.graph.counts().0 > existing_before {
            self.graph.fit(true);
            self.graph.set_fit_on_settle();
        }
    }
}
