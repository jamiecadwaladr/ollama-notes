//! Cloud APIs that speak OpenAI's chat completions protocol (OpenAI, Anthropic, Gemini, OpenRouter, Groq and others)
//! as data: the services Settings offers, request headers and bodies, the model list, and the fallbacks for
//! parameters a model rejects. Streamed replies are read by `ollama::ChatStream::sse`; sending requests is the
//! page's job (ui/src/ollama.rs).

use crate::host::host_name;
use crate::ollama::{ChatMessage, ModelInfo, sort_by_name};
use serde::Deserialize;
use serde_json::{Value, json};

/// The services Settings lists, with their API addresses. Any other OpenAI-compatible address works too. Each of
/// these answers requests from a web page (CORS), which the page needs, as it calls them directly.
pub const SERVICES: [(&str, &str); 9] = [
    ("OpenAI", "https://api.openai.com/v1"),
    ("Anthropic", "https://api.anthropic.com/v1"),
    ("Google Gemini", "https://generativelanguage.googleapis.com/v1beta/openai"),
    ("OpenRouter", "https://openrouter.ai/api/v1"),
    ("Groq", "https://api.groq.com/openai/v1"),
    ("Mistral", "https://api.mistral.ai/v1"),
    ("DeepSeek", "https://api.deepseek.com/v1"),
    ("Together AI", "https://api.together.xyz/v1"),
    ("xAI", "https://api.x.ai/v1"),
];

/// Words in the ids of models that can't chat: embeddings, speech, images, moderation, legacy completions.
const NOT_CHAT: [&str; 12] = [
    "embed",
    "tts",
    "whisper",
    "transcribe",
    "audio",
    "realtime",
    "dall-e",
    "image",
    "sora",
    "moderation",
    "babbage",
    "davinci",
];

/// The service's name from `SERVICES`, or the address's host name.
pub fn service_name(base: &str) -> String {
    match SERVICES.iter().find(|(_, url)| *url == base) {
        Some((name, _)) => name.to_string(),
        None => host_name(base).unwrap_or_else(|| base.to_string()),
    }
}

/// The headers that authorize a request. Anthropic also wants its own key header, its API version, and an opt-in
/// before it answers a web page.
pub fn headers(base: &str, key: &str) -> Vec<(&'static str, String)> {
    let mut h = Vec::new();
    if !key.is_empty() {
        h.push(("Authorization", format!("Bearer {key}")));
    }
    if host_name(base).as_deref() == Some("api.anthropic.com") {
        h.push(("anthropic-dangerous-direct-browser-access", "true".into()));
        h.push(("anthropic-version", "2023-06-01".into()));
        if !key.is_empty() {
            h.push(("x-api-key", key.into()));
        }
    }
    h
}

/// The chat models in a `/models` reply (`{"data": [{"id": …}, …]}`), sorted by name.
pub fn parse_models(body: &str) -> Result<Vec<ModelInfo>, serde_json::Error> {
    #[derive(Deserialize)]
    struct Model {
        #[serde(default)]
        id: String,
    }
    #[derive(Deserialize)]
    struct Models {
        #[serde(default)]
        data: Vec<Model>,
    }
    let mut models: Vec<ModelInfo> = serde_json::from_str::<Models>(body)?
        .data
        .into_iter()
        .filter(|m| !m.id.is_empty() && !NOT_CHAT.iter().any(|w| m.id.to_lowercase().contains(w)))
        .map(|m| ModelInfo { name: m.id, ..ModelInfo::default() })
        .collect();
    sort_by_name(&mut models);
    Ok(models)
}

/// The body of a streamed `/chat/completions` request.
pub fn chat_stream_body(model: &str, messages: &[ChatMessage], temperature: f64) -> Value {
    json!({ "model": model, "messages": messages, "stream": true, "temperature": temperature })
}

/// The body of a non-streamed `/chat/completions` request for JSON output constrained by `schema`.
pub fn chat_json_body(model: &str, messages: &[ChatMessage], schema: &Value, temperature: f64) -> Value {
    json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "response_format": { "type": "json_schema", "json_schema": { "name": "notes", "schema": schema } },
    })
}

/// The next request body after a 400 reply, as models reject different parameters: first without `temperature`
/// (reasoning models only take the default), then in plain JSON mode instead of a schema, then with no response
/// format. None when there is nothing left to try.
pub fn fallback(body: &Value) -> Option<Value> {
    let mut next = body.clone();
    let obj = next.as_object_mut()?;
    if obj.remove("temperature").is_some() {
        return Some(next);
    }
    match obj.get("response_format")?.get("type").and_then(Value::as_str) {
        Some("json_schema") => {
            obj.insert("response_format".into(), json!({ "type": "json_object" }));
        }
        _ => {
            obj.remove("response_format");
        }
    }
    Some(next)
}

/// The text of a non-streamed reply.
pub fn reply_content(reply: &Value) -> &str {
    reply.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_chat_models_only() {
        let body = r#"{"object":"list","data":[{"id":"gpt-5"},{"id":"text-embedding-3-small"},{"id":"Claude-x"},
            {"id":"gpt-image-1"},{"id":"whisper-1"},{"id":"babbage-002"},{"object":"model"}]}"#;
        let names: Vec<String> = parse_models(body).unwrap().into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["Claude-x", "gpt-5"]);
        assert!(parse_models("{}").unwrap().is_empty());
        assert!(parse_models("nope").is_err());
    }

    #[test]
    fn names_services() {
        assert_eq!(service_name("https://api.groq.com/openai/v1"), "Groq");
        assert_eq!(service_name("http://localhost:1234/v1"), "localhost");
    }

    #[test]
    fn authorizes_requests() {
        assert_eq!(headers("https://api.openai.com/v1", "sk-1"), [("Authorization", "Bearer sk-1".to_string())]);
        assert!(headers("http://localhost:1234/v1", "").is_empty());
        let anthropic = headers("https://api.anthropic.com/v1", "sk-ant");
        assert!(anthropic.contains(&("x-api-key", "sk-ant".to_string())));
        assert!(anthropic.contains(&("anthropic-dangerous-direct-browser-access", "true".to_string())));
    }

    #[test]
    fn requests_fall_back_step_by_step() {
        let schema = json!({ "type": "object" });
        let body = chat_json_body("m", &[ChatMessage::new("user", "hi")], &schema, 0.2);
        assert_eq!(body["response_format"]["json_schema"]["schema"], schema);
        let b2 = fallback(&body).unwrap();
        assert!(b2.get("temperature").is_none());
        let b3 = fallback(&b2).unwrap();
        assert_eq!(b3["response_format"], json!({ "type": "json_object" }));
        let b4 = fallback(&b3).unwrap();
        assert!(b4.get("response_format").is_none());
        assert!(fallback(&b4).is_none());
        let stream = chat_stream_body("m", &[], 0.7);
        assert!(fallback(&fallback(&stream).unwrap()).is_none());
    }

    #[test]
    fn reads_replies() {
        let reply = json!({ "choices": [{ "message": { "role": "assistant", "content": "{\"notes\":[]}" } }] });
        assert_eq!(reply_content(&reply), "{\"notes\":[]}");
        assert_eq!(reply_content(&json!({})), "");
    }
}
