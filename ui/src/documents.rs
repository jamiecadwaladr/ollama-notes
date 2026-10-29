//! Imported and dropped files as plain text (documents.js): text-like files are decoded directly, HTML through the
//! browser's own parser, PDFs through the vendored pdf.js (works offline).

use crate::dom::{self, js_error};
use crate::platform::FileData;
use js_sys::{Array, Promise, Uint8Array};
use notes_core::text::{self, PdfItem};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{DomParser, SupportedType};

#[wasm_bindgen(module = "/js/pdf_shim.js")]
extern "C" {
    #[wasm_bindgen(js_name = pdfPages)]
    fn pdf_pages(data: &Uint8Array, on_progress: &JsValue) -> Promise;
}

/// A parsed file: its name, its text, and its page count if it was a PDF.
pub struct Doc {
    pub name: String,
    pub text: String,
    pub pages: Option<u32>,
}

/// Parses a file. `on_progress(page, pages)` fires per PDF page.
pub async fn parse(file: FileData, on_progress: impl FnMut(u32, u32) + 'static) -> Result<Doc, String> {
    let FileData { name, mime, bytes } = file;
    if text::is_pdf(&name, &mime) {
        let (text, pages) = parse_pdf(&bytes, on_progress).await?;
        return Ok(Doc { name, text, pages: Some(pages) });
    }
    if text::is_text(&name, &mime) {
        let raw = String::from_utf8_lossy(&bytes);
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        let raw = if text::is_html(&name, &mime) { html_to_text(raw) } else { raw.to_string() };
        return Ok(Doc { name, text: text::normalize(&raw), pages: None });
    }
    Err(format!("Unsupported file type: {name}"))
}

/// The visible text of an HTML document, without scripts and styles.
fn html_to_text(html: &str) -> String {
    let Ok(parser) = DomParser::new() else { return html.to_string() };
    let Ok(doc) = parser.parse_from_string(html, SupportedType::TextHtml) else { return html.to_string() };
    if let Ok(list) = doc.query_selector_all("script, style, noscript, template") {
        for i in 0..list.length() {
            if let Some(el) = list.item(i).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) {
                el.remove();
            }
        }
    }
    let Some(body) = doc.body() else { return String::new() };
    let inner = body.inner_text();
    if inner.is_empty() { body.text_content().unwrap_or_default() } else { inner }
}

async fn parse_pdf(bytes: &[u8], on_progress: impl FnMut(u32, u32) + 'static) -> Result<(String, u32), String> {
    let progress = Closure::<dyn FnMut(u32, u32)>::new(on_progress);
    let data = Uint8Array::from(bytes);
    let result = JsFuture::from(pdf_pages(&data, progress.as_ref())).await.map_err(|e| js_error(&e))?;
    drop(progress);
    let pages = Array::from(&result);
    let mut texts = Vec::with_capacity(pages.length() as usize);
    for page in pages.iter() {
        let items: Vec<PdfItem> = serde_wasm_bindgen::from_value(page).map_err(|e| e.to_string())?;
        texts.push(text::pdf_page_text(&items));
    }
    Ok((text::join_pdf_pages(&texts), texts.len() as u32))
}

/// The files of a `<input type=file>` or a drop.
pub async fn read_file_list(list: &web_sys::FileList) -> Vec<FileData> {
    let mut out = Vec::new();
    for i in 0..list.length() {
        let Some(f) = list.item(i) else { continue };
        match JsFuture::from(f.array_buffer()).await {
            Ok(buf) => out.push(FileData { name: f.name(), mime: f.type_(), bytes: Uint8Array::new(&buf).to_vec() }),
            Err(e) => dom::warn(&format!("reading {}: {}", f.name(), js_error(&e))),
        }
    }
    out
}
