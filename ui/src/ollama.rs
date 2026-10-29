//! Talking to the models from the page: same-origin /api in browser mode (the server proxies it), or the Ollama
//! server itself in the desktop app (Ollama accepts requests from tauri:// pages by default); or, in both, a cloud
//! API with an OpenAI-style /chat/completions, called directly with the key from Settings. Aborting a request closes
//! its connection, so Stop also stops generation on the server.

use crate::dom::{self, js_error, window};
use notes_core::ollama::{self as api, ChatMessage, ChatStream, ModelInfo};
use notes_core::openai;
use notes_core::websearch::Failure;
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortController, ReadableStreamDefaultReader, RequestInit, Response};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    /// The Stop button, or a new chat.
    Stopped,
    Timeout,
    Cancelled,
}

/// An AbortController that remembers why it was aborted.
#[derive(Clone)]
pub struct Abort {
    ctl: AbortController,
    reason: Rc<Cell<Option<AbortReason>>>,
}

impl Abort {
    pub fn new() -> Abort {
        Abort { ctl: AbortController::new().expect("AbortController"), reason: Rc::default() }
    }

    pub fn abort(&self, reason: AbortReason) {
        if self.reason.get().is_none() {
            self.reason.set(Some(reason));
        }
        self.ctl.abort();
    }

    pub fn reason(&self) -> Option<AbortReason> {
        self.reason.get()
    }

    pub fn is_aborted(&self) -> bool {
        self.reason.get().is_some()
    }

    pub fn same(&self, other: &Abort) -> bool {
        Rc::ptr_eq(&self.reason, &other.reason)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Aborted,
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Aborted => f.write_str("Aborted"),
            Error::Failed(m) => f.write_str(m),
        }
    }
}

/// A chat request's settings besides its messages. A cloud API only takes the temperature.
#[derive(Default)]
pub struct ChatOptions<'a> {
    pub temperature: f64,
    /// The context in tokens, instead of the model's default.
    pub num_ctx: Option<u32>,
    /// Tools the model may call.
    pub tools: Option<&'a Value>,
}

#[derive(Default)]
pub struct Ollama {
    /// Where Ollama is ('' = this page's server), or the cloud API's base URL.
    base: RefCell<String>,
    /// The cloud API's key, or None for Ollama.
    api_key: RefCell<Option<String>>,
    /// Capabilities per model; they can differ between servers, so changing the host clears it.
    caps: RefCell<HashMap<String, Vec<String>>>,
}

async fn text_of(r: &Response) -> String {
    match r.text() {
        Ok(p) => JsFuture::from(p).await.ok().and_then(|v| v.as_string()).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

impl Ollama {
    pub fn host(&self) -> String {
        self.base.borrow().clone()
    }

    /// Whether this talks to a cloud API rather than Ollama.
    pub fn is_cloud(&self) -> bool {
        self.api_key.borrow().is_some()
    }

    /// Points at an Ollama server (`api_key` None), or a cloud API.
    pub fn set_host(&self, url: &str, api_key: Option<&str>) {
        *self.base.borrow_mut() = url.to_string();
        *self.api_key.borrow_mut() = api_key.map(str::to_string);
        self.caps.borrow_mut().clear();
    }

    /// The path of an endpoint: Ollama's, or the cloud API's.
    fn path(&self, ollama: &'static str, cloud: &'static str) -> &'static str {
        if self.is_cloud() { cloud } else { ollama }
    }

    async fn request(&self, path: &str, body: Option<&Value>, abort: &Abort) -> Result<Response, Error> {
        let base = self.host();
        let init = RequestInit::new();
        init.set_signal(Some(&abort.ctl.signal()));
        let headers = web_sys::Headers::new().map_err(|e| Error::Failed(js_error(&e)))?;
        if let Some(key) = self.api_key.borrow().as_deref() {
            for (name, value) in openai::headers(&base, key) {
                let _ = headers.set(name, &value);
            }
        }
        if let Some(b) = body {
            init.set_method("POST");
            let _ = headers.set("Content-Type", "application/json");
            init.set_body(&JsValue::from_str(&b.to_string()));
        }
        init.set_headers(&headers);
        let result = JsFuture::from(window().fetch_with_str_and_init(&format!("{base}{path}"), &init)).await;
        match result {
            Ok(r) => Ok(r.unchecked_into()),
            Err(_) if abort.is_aborted() => Err(Error::Aborted),
            Err(e) if base.is_empty() => Err(Error::Failed(js_error(&e))),
            Err(_) if self.is_cloud() => Err(Error::Failed(format!(
                "Cannot reach {base}. Check that you're online, and the API address in Settings."
            ))),
            Err(_) => Err(Error::Failed(format!(
                "Cannot reach Ollama at {base}. Start Ollama.app or run `ollama serve`, or change the server in Settings."
            ))),
        }
    }

    /// POSTs `body`, retrying with the fallbacks for parameters the server or model rejects: for Ollama
    /// `notes_core::ollama::json_fallback`, for a cloud API `notes_core::openai::fallback` after a 400.
    async fn post(&self, path: &str, mut body: Value, abort: &Abort) -> Result<Response, Error> {
        loop {
            let r = self.request(path, Some(&body), abort).await?;
            let next = if r.ok() {
                None
            } else if self.is_cloud() {
                (r.status() == 400).then(|| openai::fallback(&body)).flatten()
            } else {
                api::json_fallback(&body)
            };
            match next {
                Some(b) => body = b,
                None => return Ok(r),
            }
        }
    }

    async fn check(r: Response) -> Result<Response, Error> {
        if r.ok() { Ok(r) } else { Err(Error::Failed(api::error_text(r.status(), &text_of(&r).await))) }
    }

    /// The pulled models (or the cloud API's chat models), sorted by name. Gives up after 5 s, so an unroutable host
    /// doesn't hang the status.
    pub async fn tags(&self) -> Result<Vec<ModelInfo>, Error> {
        let abort = Abort::new();
        let timer = {
            let a = abort.clone();
            dom::set_timeout(5000, move || a.abort(AbortReason::Timeout))
        };
        let result = async {
            let r = Self::check(self.request(self.path("/api/tags", "/models"), None, &abort).await?).await?;
            let text = text_of(&r).await;
            let models = if self.is_cloud() { openai::parse_models(&text) } else { api::parse_tags(&text) };
            models.map_err(|e| Error::Failed(e.to_string()))
        }
        .await;
        dom::clear_timeout(timer);
        result.map_err(|e| match e {
            Error::Aborted => Error::Failed("timed out".into()),
            e => e,
        })
    }

    /// A model's capabilities (e.g. completion, thinking, tools), cached per model name. Cloud APIs don't say.
    pub async fn capabilities(&self, model: &str) -> Vec<String> {
        if self.is_cloud() {
            return Vec::new();
        }
        if let Some(c) = self.caps.borrow().get(model) {
            return c.clone();
        }
        let body = serde_json::json!({ "model": model });
        let caps = match self.request("/api/show", Some(&body), &Abort::new()).await {
            Ok(r) if r.ok() => api::parse_capabilities(&text_of(&r).await),
            _ => Vec::new(),
        };
        self.caps.borrow_mut().insert(model.to_string(), caps.clone());
        caps
    }

    /// Streams /api/chat (or /chat/completions). `on_chunk` sees the reply so far each time a line arrives.
    pub async fn chat_stream(
        &self,
        model: &str,
        messages: &[ChatMessage],
        opts: &ChatOptions<'_>,
        abort: &Abort,
        mut on_chunk: impl FnMut(&ChatStream),
    ) -> Result<ChatStream, Error> {
        let (body, mut stream) = if self.is_cloud() {
            (openai::chat_stream_body(model, messages, opts.temperature), ChatStream::sse())
        } else {
            (api::chat_stream_body(model, messages, opts.temperature, opts.num_ctx, opts.tools), ChatStream::default())
        };
        let r = Self::check(self.post(self.path("/api/chat", "/chat/completions"), body, abort).await?).await?;
        let Some(body) = r.body() else { return Ok(stream) };
        let reader: ReadableStreamDefaultReader = body.get_reader().unchecked_into();
        let failed = |e: JsValue| if abort.is_aborted() { Error::Aborted } else { Error::Failed(js_error(&e)) };
        loop {
            let chunk = JsFuture::from(reader.read()).await.map_err(failed)?;
            if js_sys::Reflect::get(&chunk, &"done".into()).ok().and_then(|d| d.as_bool()) == Some(true) {
                break;
            }
            let value = js_sys::Reflect::get(&chunk, &"value".into()).unwrap_or(JsValue::UNDEFINED);
            let bytes = js_sys::Uint8Array::new(&value).to_vec();
            if stream.push(&bytes).map_err(|e| Error::Failed(e.to_string()))? {
                on_chunk(&stream);
            }
            if stream.is_done() {
                let _ = reader.cancel();
                break;
            }
        }
        Ok(stream)
    }

    /// A web search or page read through Ollama (`notes_core::websearch`): the reply's text, or why it failed. The
    /// caller checks `abort` for a Stop.
    pub async fn web(&self, path: &str, body: &Value, abort: &Abort) -> Result<String, Failure> {
        let r = self.request(path, Some(body), abort).await.map_err(|e| Failure::unreachable(&e.to_string()))?;
        let text = text_of(&r).await;
        if r.ok() { Ok(text) } else { Err(Failure::from_reply(r.status(), &text)) }
    }

    /// A non-streamed call that asks for JSON, constrained by `schema`, with fallbacks when the server rejects the
    /// request (see `post`). A cloud API only takes the temperature from `options`.
    pub async fn chat_json(
        &self,
        model: &str,
        messages: &[ChatMessage],
        schema: &Value,
        options: &Value,
        think: Option<bool>,
        abort: &Abort,
    ) -> Result<Value, Error> {
        let body = if self.is_cloud() {
            let temperature = options.get("temperature").and_then(Value::as_f64).unwrap_or(0.2);
            openai::chat_json_body(model, messages, schema, temperature)
        } else {
            api::chat_json_body(model, messages, Some(schema), options, think)
        };
        let r = self.post(self.path("/api/chat", "/chat/completions"), body, abort).await?;
        let text = text_of(&Self::check(r).await?).await;
        if abort.is_aborted() {
            return Err(Error::Aborted);
        }
        let j: Value = serde_json::from_str(&text).map_err(|e| Error::Failed(e.to_string()))?;
        if let Some(e) = j.get("error").filter(|e| !e.is_null()) {
            return Err(Error::Failed(api::error_message(e)));
        }
        let content = if self.is_cloud() {
            openai::reply_content(&j)
        } else {
            j.pointer("/message/content").and_then(Value::as_str).unwrap_or("")
        };
        api::parse_loose_json(content).map_err(|e| Error::Failed(e.to_string()))
    }
}
