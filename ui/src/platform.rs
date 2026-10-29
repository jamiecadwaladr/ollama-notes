//! What differs between the desktop app (a Tauri window, src-tauri) and browser mode (server/): where state is
//! kept, how files are opened and saved, confirmations, and starting Ollama.

use crate::dom::{self, js_error, window};
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Browser mode keeps the state in localStorage under the same key as before, so earlier data is still there.
pub const STORE_KEY: &str = "ollama-notes.v1";

/// A file the user picked or dropped.
pub struct FileData {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Text files and PDFs to turn into notes.
    Documents,
    /// An exported map (JSON).
    Map,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Desktop,
    Browser,
}

fn tauri() -> Option<JsValue> {
    Reflect::get(&window(), &"__TAURI__".into()).ok().filter(JsValue::is_object)
}

/// A plain JS object from key/value pairs.
pub fn obj(pairs: &[(&str, JsValue)]) -> JsValue {
    let o = Object::new();
    for (k, v) in pairs {
        let _ = Reflect::set(&o, &(*k).into(), v);
    }
    o.into()
}

fn method(namespace: &str, name: &str) -> Option<(JsValue, Function)> {
    let ns = Reflect::get(&tauri()?, &namespace.into()).ok()?;
    let f = Reflect::get(&ns, &name.into()).ok()?.dyn_into::<Function>().ok()?;
    Some((ns, f))
}

/// Calls a command of the desktop app (src-tauri/src/commands.rs).
pub async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, String> {
    let (ns, f) = method("core", "invoke").ok_or("not in the desktop app")?;
    let p: Promise =
        f.call2(&ns, &cmd.into(), &args).map_err(|e| js_error(&e))?.dyn_into().map_err(|e| js_error(&e))?;
    JsFuture::from(p).await.map_err(|e| js_error(&e))
}

impl Platform {
    pub fn detect() -> Platform {
        if tauri().is_some() { Platform::Desktop } else { Platform::Browser }
    }

    pub fn is_desktop(self) -> bool {
        self == Platform::Desktop
    }

    /// The saved state as JSON text, or None before the first save.
    pub async fn load_state(self) -> Result<Option<String>, String> {
        match self {
            Platform::Desktop => Ok(invoke("load_state", JsValue::UNDEFINED).await?.as_string()),
            Platform::Browser => Ok(storage()?.get_item(STORE_KEY).map_err(|e| js_error(&e))?),
        }
    }

    pub async fn save_state(self, json: &str) -> Result<(), String> {
        match self {
            Platform::Desktop => invoke("save_state", obj(&[("json", json.into())])).await.map(drop),
            Platform::Browser => storage()?.set_item(STORE_KEY, json).map_err(|e| js_error(&e)),
        }
    }

    /// Saves text to a file. The app asks where and returns the path (None if cancelled); a browser downloads it.
    pub async fn save_file(self, name: &str, text: &str) -> Result<Option<String>, String> {
        match self {
            Platform::Desktop => {
                Ok(invoke("save_file", obj(&[("name", name.into()), ("text", text.into())])).await?.as_string())
            }
            Platform::Browser => download(name, text).map(|()| None),
        }
    }

    /// Asks the app to start Ollama if `host` is this machine. Returns what it did ("opened Ollama.app"), or None.
    pub async fn start_ollama(self, host: &str) -> Option<String> {
        match self {
            Platform::Desktop => invoke("start_ollama", obj(&[("host", host.into())])).await.ok()?.as_string(),
            Platform::Browser => None,
        }
    }

    /// An OK/Cancel question: a sheet on the app's window, or the browser's confirm().
    pub async fn confirm(self, message: &str) -> bool {
        match self {
            Platform::Desktop => {
                invoke("confirm", obj(&[("message", message.into())])).await.ok().and_then(|v| v.as_bool())
                    == Some(true)
            }
            Platform::Browser => window().confirm_with_message(message).unwrap_or(false),
        }
    }

    /// The app's open panel. Browser mode uses the page's file inputs instead.
    pub async fn pick_files(self, pick: Pick) -> Vec<FileData> {
        let kind = if pick == Pick::Map { "map" } else { "documents" };
        let Ok(list) = invoke("pick_files", obj(&[("kind", kind.into())])).await else { return Vec::new() };
        let mut files = Vec::new();
        for item in Array::from(&list).iter() {
            let get = |k: &str| Reflect::get(&item, &k.into()).ok().and_then(|v| v.as_string()).unwrap_or_default();
            let (name, path) = (get("name"), get("path"));
            match invoke("read_file", obj(&[("path", path.as_str().into())])).await {
                Ok(buf) => {
                    files.push(FileData { mime: mime_for(&name).into(), name, bytes: Uint8Array::new(&buf).to_vec() })
                }
                Err(e) => dom::warn(&format!("reading {name}: {e}")),
            }
        }
        files
    }

    /// Opens a link from a reply in the default browser.
    pub fn open_url(self, url: &str) {
        match self {
            Platform::Desktop => {
                let url = url.to_string();
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = invoke("open_url", obj(&[("url", url.into())])).await;
                });
            }
            Platform::Browser => {
                let _ = window().open_with_url_and_target(url, "_blank");
            }
        }
    }

    /// Listens for an event the app sends to the page (menu commands, the flush before quitting).
    pub fn listen(self, event: &str, f: impl FnMut(JsValue) + 'static) {
        let Some((ns, listen)) = method("event", "listen") else { return };
        let cb = Closure::<dyn FnMut(JsValue)>::new(f);
        let _ = listen.call2(&ns, &event.into(), cb.as_ref());
        cb.forget();
    }
}

fn storage() -> Result<web_sys::Storage, String> {
    window().local_storage().ok().flatten().ok_or_else(|| "localStorage is not available".to_string())
}

fn download(name: &str, text: &str) -> Result<(), String> {
    let parts = Array::of1(&text.into());
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/json");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts).map_err(|e| js_error(&e))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|e| js_error(&e))?;
    let a: web_sys::HtmlAnchorElement = dom::document().create_element("a").map_err(|e| js_error(&e))?.unchecked_into();
    a.set_href(&url);
    a.set_download(name);
    a.click();
    dom::set_timeout(1000, move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    Ok(())
}

/// The MIME type a browser would give a file of this name (desktop picks come with paths only).
fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "txt" | "md" | "markdown" | "text" | "csv" | "log" | "rst" | "xml" | "yaml" | "yml" | "toml" | "ini"
        | "cfg" | "tex" | "org" => "text/plain",
        _ => "",
    }
}
