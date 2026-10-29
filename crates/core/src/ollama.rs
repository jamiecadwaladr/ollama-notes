//! The Ollama HTTP API as data: request bodies, the streamed NDJSON chat reply, error messages, and salvaging JSON
//! from a model that didn't quite produce it (ollama.js). Sending requests is the page's job (ui/src/ollama.rs).

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        ChatMessage { role: role.into(), content: content.into() }
    }
}

/// A model from `/api/tags`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelInfo {
    pub name: String,
    pub details: ModelDetails,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelDetails {
    pub parameter_size: Option<String>,
}

/// The models in a `/api/tags` reply, sorted by name.
pub fn parse_tags(body: &str) -> Result<Vec<ModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Tags {
        #[serde(default)]
        models: Vec<ModelInfo>,
    }
    let mut models = serde_json::from_str::<Tags>(body)?.models;
    sort_by_name(&mut models);
    Ok(models)
}

pub(crate) fn sort_by_name(models: &mut [ModelInfo]) {
    models.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name)));
}

/// A model's capabilities from `/api/show` (e.g. completion, thinking, tools).
pub fn parse_capabilities(body: &str) -> Vec<String> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| serde_json::from_value(v.get("capabilities")?.clone()).ok())
        .unwrap_or_default()
}

/// An `error` from a reply: Ollama's string, or the `message` of a cloud API's error object.
pub fn error_message(e: &Value) -> String {
    match e.as_str().or_else(|| e.get("message")?.as_str()) {
        Some(m) => m.to_string(),
        None => e.to_string(),
    }
}

/// The text of an error reply: its JSON `error`, the body itself, or "HTTP <status>".
pub fn error_text(status: u16, body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => match v.get("error") {
            Some(Value::String(e)) if !e.is_empty() => e.clone(),
            Some(e) if !e.is_null() && e != &Value::Bool(false) => error_message(e),
            _ => body.to_string(),
        },
        Err(_) if body.is_empty() => format!("HTTP {status}"),
        Err(_) => body.to_string(),
    }
}

/// The body of a streamed `/api/chat` request.
pub fn chat_stream_body(model: &str, messages: &[ChatMessage], temperature: f64) -> Value {
    json!({ "model": model, "messages": messages, "stream": true, "options": { "temperature": temperature } })
}

/// The body of a non-streamed `/api/chat` request for JSON output: constrained by `schema` (Ollama >= 0.5), or plain
/// JSON mode. `think` is left out when None.
pub fn chat_json_body(
    model: &str,
    messages: &[ChatMessage],
    schema: Option<&Value>,
    options: &Value,
    think: Option<bool>,
) -> Value {
    let mut opts = json!({ "temperature": 0.2 });
    if let (Some(o), Some(extra)) = (opts.as_object_mut(), options.as_object()) {
        o.extend(extra.clone());
    }
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "format": schema.cloned().unwrap_or_else(|| "json".into()),
        "options": opts,
    });
    if let Some(t) = think {
        body["think"] = t.into();
    }
    body
}

/// The next request body after a rejected one: first without `think`, then in plain JSON mode instead of a
/// schema. None when there is nothing left to try.
pub fn json_fallback(body: &Value) -> Option<Value> {
    let mut next = body.clone();
    let obj = next.as_object_mut()?;
    if obj.remove("think").is_some() {
        return Some(next);
    }
    if obj.get("format").is_some_and(Value::is_object) {
        obj.insert("format".into(), "json".into());
        return Some(next);
    }
    None
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StreamError {
    #[error("{0}")]
    Ollama(String),
}

/// Accumulates a streamed chat reply from its bytes: Ollama's NDJSON from `/api/chat`, or the server-sent events of
/// a cloud API's `/chat/completions`.
#[derive(Debug, Default)]
pub struct ChatStream {
    buf: Vec<u8>,
    sse: bool,
    pub content: String,
    pub thinking: String,
    /// The final line (with timing stats) once the reply is done.
    pub done: Option<Value>,
}

impl ChatStream {
    /// A stream of server-sent events (`data: {…}` lines, then `data: [DONE]`) from a chat completions API.
    pub fn sse() -> ChatStream {
        ChatStream { sse: true, ..ChatStream::default() }
    }

    /// Feeds bytes as they arrive. Returns whether any line was read, so the caller knows to repaint.
    pub fn push(&mut self, bytes: &[u8]) -> Result<bool, StreamError> {
        self.buf.extend_from_slice(bytes);
        let mut read = false;
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let mut line = line.trim();
            if self.sse {
                // event names, ids and comments carry nothing
                let Some(data) = line.strip_prefix("data:") else { continue };
                line = data.trim_start();
                if line == "[DONE]" {
                    self.done = Some(json!({ "done": true }));
                    return Ok(true);
                }
            }
            if line.is_empty() {
                continue;
            }
            let Ok(j) = serde_json::from_str::<Value>(line) else { continue };
            if let Some(e) = j.get("error").filter(|e| !e.is_null()) {
                return Err(StreamError::Ollama(error_message(e)));
            }
            let message = if self.sse { j.pointer("/choices/0/delta") } else { j.get("message") };
            if let Some(m) = message {
                if let Some(c) = m.get("content").and_then(Value::as_str) {
                    self.content.push_str(c);
                }
                // Ollama's "thinking"; "reasoning_content" (DeepSeek) or "reasoning" (OpenRouter, Groq) in a delta
                if let Some(t) = ["thinking", "reasoning_content", "reasoning"]
                    .iter()
                    .find_map(|k| m.get(*k).and_then(Value::as_str))
                {
                    self.thinking.push_str(t);
                }
            }
            read = true;
            if j.get("done").and_then(Value::as_bool) == Some(true) {
                self.done = Some(j);
                return Ok(true);
            }
        }
        Ok(read)
    }

    pub fn is_done(&self) -> bool {
        self.done.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LooseJsonError {
    #[error("Model did not return valid JSON")]
    Invalid,
}

const QUOTED: &str = r#""((?:[^"\\]|\\.)*)""#;
static NOTE_OBJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\{[^{}]*"title"[^{}]*\}"#).unwrap());
static NOTE_TRIPLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#""title"\s*:\s*{QUOTED}\s*,\s*"summary"\s*:\s*{QUOTED}(?:\s*,\s*"kind"\s*:\s*"([A-Za-z0-9_]+)")?"#
    ))
    .unwrap()
});
static TOPIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r#""topic"\s*:\s*{QUOTED}"#)).unwrap());

/// JSON from a model's reply: as is, the outermost `{…}`, or, when the reply was truncated or malformed (e.g. the
/// model looped until the token cap), whatever notes can be salvaged from it, marked `salvaged: true`.
pub fn parse_loose_json(text: &str) -> Result<Value, LooseJsonError> {
    if let Ok(v) = serde_json::from_str(text) {
        return Ok(v);
    }
    if let (Some(s), Some(e)) = (text.find('{'), text.rfind('}'))
        && e > s
        && let Ok(v) = serde_json::from_str(&text[s..=e])
    {
        return Ok(v);
    }
    let mut notes: Vec<Value> =
        NOTE_OBJECT.find_iter(text).filter_map(|m| serde_json::from_str(m.as_str()).ok()).collect();
    if notes.is_empty() {
        // no complete object: pull title/summary/kind triples out of the partial text instead
        let unquote = |s: &str| serde_json::from_str::<String>(&format!("\"{s}\"")).ok();
        for c in NOTE_TRIPLE.captures_iter(text) {
            let (Some(title), Some(summary)) = (unquote(&c[1]), unquote(&c[2])) else { continue };
            let kind = c.get(3).map_or("concept", |m| m.as_str());
            notes.push(json!({ "title": title, "summary": summary, "kind": kind, "related": [] }));
        }
    }
    if notes.is_empty() {
        return Err(LooseJsonError::Invalid);
    }
    let topic = TOPIC.captures(text).map_or(String::new(), |c| c[1].to_string());
    Ok(json!({ "topic": topic, "notes": notes, "salvaged": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_ndjson_across_chunk_boundaries() {
        let mut s = ChatStream::default();
        let reply = "{\"message\":{\"content\":\"Hé\"}}\n{\"message\":{\"thinking\":\"hm\",\"content\":\"llo\"}}\n\n{\"done\":true,\"eval_count\":3}\n";
        let bytes = reply.as_bytes();
        assert!(!s.push(&bytes[..20]).unwrap()); // inside the first line, and inside "é"
        assert!(s.push(&bytes[20..]).unwrap());
        assert_eq!((s.content.as_str(), s.thinking.as_str()), ("Héllo", "hm"));
        assert_eq!(s.done.as_ref().unwrap()["eval_count"], 3);
    }

    #[test]
    fn streams_server_sent_events() {
        let mut s = ChatStream::sse();
        let reply = ": keep-alive\r\nevent: x\ndata: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"reasoning\":\"hm\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hé\"}}]}\ndata:{\"choices\":[{\"delta\":{\"content\":\"llo\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n";
        let bytes = reply.as_bytes();
        assert!(!s.push(&bytes[..30]).unwrap());
        assert!(s.push(&bytes[30..]).unwrap());
        assert_eq!((s.content.as_str(), s.thinking.as_str()), ("Héllo", "hm"));
        assert!(s.is_done());
        let mut s = ChatStream::sse();
        let err = s.push(b"data: {\"error\":{\"message\":\"Rate limit reached\",\"code\":429}}\n");
        assert_eq!(err, Err(StreamError::Ollama("Rate limit reached".into())));
    }

    #[test]
    fn stream_errors_and_junk_lines() {
        let mut s = ChatStream::default();
        assert!(!s.push(b"not json\n").unwrap());
        assert_eq!(s.push(b"{\"error\":\"model not found\"}\n"), Err(StreamError::Ollama("model not found".into())));
    }

    #[test]
    fn error_texts() {
        assert_eq!(error_text(404, r#"{"error":"model 'x' not found"}"#), "model 'x' not found");
        assert_eq!(error_text(500, ""), "HTTP 500");
        assert_eq!(error_text(500, "boom"), "boom");
        assert_eq!(error_text(400, r#"{"other":1}"#), r#"{"other":1}"#);
        assert_eq!(
            error_text(401, r#"{"error":{"message":"Incorrect API key provided","code":null}}"#),
            "Incorrect API key provided"
        );
        assert_eq!(error_text(400, r#"{"error":{"code":7}}"#), r#"{"code":7}"#);
    }

    #[test]
    fn json_requests_fall_back_step_by_step() {
        let schema = json!({ "type": "object" });
        let body = chat_json_body("m", &[], Some(&schema), &json!({ "num_predict": 900 }), Some(false));
        assert_eq!(body["options"], json!({ "temperature": 0.2, "num_predict": 900 }));
        let b2 = json_fallback(&body).unwrap();
        assert!(b2.get("think").is_none());
        assert_eq!(b2["format"], schema);
        let b3 = json_fallback(&b2).unwrap();
        assert_eq!(b3["format"], "json");
        assert!(json_fallback(&b3).is_none());
    }

    #[test]
    fn salvages_loose_json() {
        assert_eq!(parse_loose_json("Sure! {\"a\": 1} hope that helps").unwrap(), json!({ "a": 1 }));
        let truncated = r#"{"topic": "Hashing", "notes": [{"title": "Hash \"sets\"", "summary": "Fast lookups.", "kind": "fact"}, {"title": "Bloom"#;
        let v = parse_loose_json(truncated).unwrap();
        assert_eq!(v["topic"], "Hashing");
        assert_eq!(v["salvaged"], true);
        assert_eq!(v["notes"][0]["title"], "Hash \"sets\"");
        let partial = r#"{"notes": [{"title": "A", "summary": "B"#;
        assert!(parse_loose_json(partial).is_err());
        let triple = r#"{"notes": [{"title": "A", "summary": "B", "kind": "task", "related": ["#;
        assert_eq!(
            parse_loose_json(triple).unwrap()["notes"][0],
            json!({ "title": "A", "summary": "B", "kind": "task", "related": [] })
        );
    }

    #[test]
    fn tags_are_sorted() {
        let models =
            parse_tags(r#"{"models":[{"name":"qwen"},{"name":"Llama","details":{"parameter_size":"3B"}}]}"#).unwrap();
        assert_eq!(models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Llama", "qwen"]);
        assert_eq!(models[0].details.parameter_size.as_deref(), Some("3B"));
        assert_eq!(parse_capabilities(r#"{"capabilities":["completion","thinking"]}"#), ["completion", "thinking"]);
    }
}
