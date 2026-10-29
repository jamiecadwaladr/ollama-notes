//! The saved state (notes.json, exports and browser storage), read and written compatibly with the JS version.
//!
//! Every field falls back to its default when it is missing or has the wrong type, as the JS merged saved data over
//! defaults; a type mismatch must never make the whole file unreadable, because the next save would replace it.
//! Fields this version doesn't know are kept in `extra` and written back unchanged.

use crate::host::{DEFAULT_API_BASE, DEFAULT_OLLAMA_HOST};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub type Extra = serde_json::Map<String, Value>;

pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful, precise assistant. Answer directly and keep answers focused. Format with Markdown (headings, lists, fenced code); write math in plain text rather than LaTeX.";
pub const DEFAULT_TITLE: &str = "New conversation";
/// Keys an export adds to the state, removed again on import.
const EXPORT_KEYS: [&str; 3] = ["app", "version", "exportedAt"];

/// The field's value, or its default when it has another type.
fn lenient<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(d)?).unwrap_or_default())
}

/// Where the models are: an Ollama server, or a cloud API that speaks OpenAI's chat completions protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Ollama,
    OpenAi,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    #[serde(deserialize_with = "lenient")]
    pub model: String,
    #[serde(deserialize_with = "lenient")]
    pub extract_model: String,
    #[serde(deserialize_with = "lenient")]
    pub system: String,
    #[serde(deserialize_with = "lenient")]
    pub temperature: f64,
    #[serde(deserialize_with = "lenient")]
    pub max_notes: f64,
    #[serde(deserialize_with = "lenient")]
    pub auto_notes: bool,
    #[serde(deserialize_with = "lenient")]
    pub auto_fit: bool,
    #[serde(deserialize_with = "lenient")]
    pub chat_width: f64,
    /// Desktop app only; in browser mode the server decides.
    #[serde(deserialize_with = "lenient")]
    pub ollama_host: String,
    #[serde(deserialize_with = "lenient")]
    pub provider: Provider,
    /// The cloud API, for `Provider::OpenAi`. Exports leave the key out.
    #[serde(deserialize_with = "lenient")]
    pub api_base: String,
    #[serde(deserialize_with = "lenient")]
    pub api_key: String,
    /// Let Ollama models search the web and read pages before answering (`websearch`).
    #[serde(deserialize_with = "lenient")]
    pub web_search: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: String::new(),
            extract_model: String::new(),
            system: DEFAULT_SYSTEM_PROMPT.to_string(),
            temperature: 0.7,
            max_notes: 4.0,
            auto_notes: true,
            auto_fit: true,
            chat_width: 440.0,
            ollama_host: DEFAULT_OLLAMA_HOST.to_string(),
            provider: Provider::Ollama,
            api_base: DEFAULT_API_BASE.to_string(),
            api_key: String::new(),
            web_search: true,
            extra: Extra::new(),
        }
    }
}

impl Settings {
    /// `{...self, ...imported}`: the fields present in `imported` replace this object's.
    pub fn overlay(&self, imported: &Value) -> Settings {
        let mut merged = serde_json::to_value(self).unwrap_or(Value::Null);
        if let (Value::Object(m), Value::Object(i)) = (&mut merged, imported) {
            for (k, v) in i {
                m.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(merged).unwrap_or_else(|_| self.clone())
    }

    /// The model used for note extraction: the extraction model, or the chat model.
    pub fn extraction_model(&self) -> &str {
        if self.extract_model.is_empty() { &self.model } else { &self.extract_model }
    }

    /// Max notes per exchange, as the extraction prompt uses it (1–8).
    pub fn max_notes_clamped(&self) -> usize {
        let n = if self.max_notes.is_finite() && self.max_notes != 0.0 { self.max_notes } else { 4.0 };
        n.clamp(1.0, 8.0) as usize
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    #[serde(deserialize_with = "lenient")]
    pub id: String,
    #[serde(deserialize_with = "lenient")]
    pub role: String,
    #[serde(deserialize_with = "lenient")]
    pub content: String,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub ts: f64,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub node_ids: Option<Vec<String>>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The web searches and page reads behind a reply.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub research: Option<Vec<ResearchStep>>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Message {
    pub fn is_streaming(&self) -> bool {
        self.streaming == Some(true)
    }
}

/// A web search or page read the model made while answering.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResearchStep {
    /// "search" or "fetch".
    #[serde(deserialize_with = "lenient")]
    pub tool: String,
    /// The search query, or the page's address.
    #[serde(deserialize_with = "lenient")]
    pub input: String,
    /// The results found, or the page read.
    #[serde(deserialize_with = "lenient")]
    pub links: Vec<WebLink>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Still running.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub pending: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl ResearchStep {
    pub fn is_fetch(&self) -> bool {
        self.tool == "fetch"
    }

    pub fn is_pending(&self) -> bool {
        self.pending == Some(true)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebLink {
    #[serde(deserialize_with = "lenient")]
    pub title: String,
    #[serde(deserialize_with = "lenient")]
    pub url: String,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Where a note came from: a chat message, or a section of an imported file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Source {
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub section: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Node {
    #[serde(deserialize_with = "lenient")]
    pub id: String,
    #[serde(deserialize_with = "lenient")]
    pub title: String,
    #[serde(deserialize_with = "lenient")]
    pub body: String,
    /// topic, source (a document), concept, fact, decision, question, task or note.
    #[serde(deserialize_with = "lenient")]
    pub kind: String,
    #[serde(deserialize_with = "lenient")]
    pub x: f64,
    #[serde(deserialize_with = "lenient")]
    pub y: f64,
    #[serde(deserialize_with = "lenient")]
    pub created_at: f64,
    /// Keywords the model gave when it extracted the note.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub kw_model: Option<Vec<String>>,
    /// The note's keywords: computed, or the user's own when `keywords_manual`.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub keywords_manual: Option<bool>,
    /// The text the computed keywords were made from, to tell when they are stale.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub kw_text: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    /// A document's full text.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(flatten)]
    pub extra: Extra,
}

pub const NOTE_KINDS: [&str; 5] = ["concept", "fact", "decision", "question", "task"];

impl Node {
    /// A new note as the map creates one: untitled, kind "note".
    pub fn new(id: String, created_at: f64) -> Node {
        Node { id, title: "Untitled".into(), kind: "note".into(), created_at, ..Node::default() }
    }

    pub fn kind_or_note(&self) -> &str {
        if self.kind.is_empty() { "note" } else { &self.kind }
    }

    pub fn is_locked(&self) -> bool {
        self.locked == Some(true)
    }

    pub fn is_manual(&self) -> bool {
        self.keywords_manual == Some(true)
    }

    pub fn keywords(&self) -> &[String] {
        self.keywords.as_deref().unwrap_or(&[])
    }

    pub fn doc_text(&self) -> Option<&str> {
        self.text.as_deref().filter(|t| !t.is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Edge {
    #[serde(deserialize_with = "lenient")]
    pub id: String,
    #[serde(deserialize_with = "lenient")]
    pub from: String,
    #[serde(deserialize_with = "lenient")]
    pub to: String,
    #[serde(deserialize_with = "lenient")]
    pub label: String,
    /// "keyword" for links made by keyword linking.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub auto: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Edge {
    pub fn is_keyword(&self) -> bool {
        self.auto.as_deref() == Some("keyword")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    #[serde(deserialize_with = "lenient")]
    pub x: f64,
    #[serde(deserialize_with = "lenient")]
    pub y: f64,
    #[serde(deserialize_with = "lenient")]
    pub k: f64,
}

impl Default for View {
    fn default() -> Self {
        View { x: 0.0, y: 0.0, k: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Physics {
    #[serde(deserialize_with = "lenient")]
    pub enabled: bool,
    #[serde(deserialize_with = "lenient")]
    pub tension: f64,
    #[serde(deserialize_with = "lenient")]
    pub gravity: f64,
    #[serde(deserialize_with = "lenient")]
    pub repulsion: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Physics {
    fn default() -> Self {
        Physics { enabled: true, tension: 1.0, gravity: 1.0, repulsion: 1.0, extra: Extra::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeywordLinks {
    #[serde(deserialize_with = "lenient")]
    pub enabled: bool,
    #[serde(deserialize_with = "lenient")]
    pub threshold: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for KeywordLinks {
    fn default() -> Self {
        KeywordLinks { enabled: true, threshold: 1.0, extra: Extra::new() }
    }
}

/// The map as saved: graph.js `toJSON()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphData {
    #[serde(deserialize_with = "lenient")]
    pub nodes: Vec<Node>,
    #[serde(deserialize_with = "lenient")]
    pub edges: Vec<Edge>,
    #[serde(deserialize_with = "lenient")]
    pub view: View,
    #[serde(deserialize_with = "lenient")]
    pub physics: Physics,
    #[serde(deserialize_with = "lenient")]
    pub keyword_links: KeywordLinks,
    #[serde(deserialize_with = "lenient")]
    pub kw_ignore: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Everything that is saved: settings, the conversation and the map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppState {
    #[serde(deserialize_with = "lenient")]
    pub settings: Settings,
    #[serde(deserialize_with = "lenient")]
    pub chat: Vec<Message>,
    #[serde(deserialize_with = "lenient")]
    pub hub_id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub context_node_ids: Vec<String>,
    #[serde(deserialize_with = "lenient")]
    pub title: String,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphData>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            settings: Settings::default(),
            chat: Vec::new(),
            hub_id: None,
            context_node_ids: Vec::new(),
            title: DEFAULT_TITLE.to_string(),
            graph: None,
            extra: Extra::new(),
        }
    }
}

impl AppState {
    /// Saved state from notes.json or browser storage. Nothing is streaming any more.
    pub fn from_saved(json: &str) -> Result<AppState, serde_json::Error> {
        let mut state: AppState = serde_json::from_str(json)?;
        state.stop_streaming();
        Ok(state)
    }

    /// An export file, imported over the current settings (`{...current, ...imported}`), as app.js did.
    pub fn from_export(json: &str, current: &Settings) -> Result<AppState, ImportError> {
        let value: Value = serde_json::from_str(json)?;
        if value.get("graph").is_none_or(Value::is_null) {
            return Err(ImportError::NotAnExport);
        }
        let mut state: AppState = serde_json::from_value(value.clone())?;
        state.settings = current.overlay(value.get("settings").unwrap_or(&Value::Null));
        for k in EXPORT_KEYS {
            state.extra.remove(k);
        }
        state.stop_streaming();
        Ok(state)
    }

    fn stop_streaming(&mut self) {
        for m in &mut self.chat {
            if m.streaming.is_some() {
                m.streaming = Some(false);
            }
            for step in m.research.iter_mut().flatten() {
                step.pending = None;
            }
        }
    }

    /// The export file's text: this state plus the export marker fields, pretty-printed. The API key stays here.
    pub fn to_export(&self, exported_at: &str) -> String {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(s) = v.get_mut("settings").and_then(Value::as_object_mut) {
            s.remove("apiKey");
        }
        if let Value::Object(m) = &mut v {
            let mut out = Extra::new();
            out.insert("app".into(), "ollama-notes".into());
            out.insert("version".into(), 1.into());
            out.insert("exportedAt".into(), exported_at.into());
            out.extend(std::mem::take(m));
            v = Value::Object(out);
        }
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("not an Ollama Notes export")]
    NotAnExport,
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_types_fall_back_to_defaults() {
        let s: AppState =
            serde_json::from_str(r#"{"settings":{"temperature":"hot","model":"m"},"chat":"x","title":5}"#).unwrap();
        assert_eq!(s.settings.temperature, 0.0);
        assert_eq!(s.settings.model, "m");
        assert_eq!(s.settings.system, DEFAULT_SYSTEM_PROMPT);
        assert!(s.chat.is_empty());
        assert_eq!(s.title, "");
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s = AppState::from_saved("{}").unwrap();
        assert_eq!(s, AppState::default());
    }

    #[test]
    fn unknown_fields_survive() {
        let json = r#"{"future":1,"settings":{"theme":"x"},"graph":{"nodes":[{"id":"a","color":"red"}],"edges":[]}}"#;
        let s = AppState::from_saved(json).unwrap();
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["future"], 1);
        assert_eq!(back["settings"]["theme"], "x");
        assert_eq!(back["graph"]["nodes"][0]["color"], "red");
    }

    #[test]
    fn import_overlays_settings_and_drops_export_keys() {
        let current = Settings { model: "mine".into(), temperature: 1.5, ..Settings::default() };
        let json = r#"{"app":"ollama-notes","version":1,"exportedAt":"t","settings":{"temperature":0.2},"graph":{}}"#;
        let s = AppState::from_export(json, &current).unwrap();
        assert_eq!(s.settings.model, "mine");
        assert_eq!(s.settings.temperature, 0.2);
        assert!(s.extra.is_empty());
        assert!(matches!(AppState::from_export("{}", &current), Err(ImportError::NotAnExport)));
    }

    #[test]
    fn export_starts_with_marker_fields() {
        let text = AppState::default().to_export("2026-10-29T00:00:00.000Z");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["app"], "ollama-notes");
        assert_eq!(v.as_object().unwrap().keys().next().unwrap(), "app");
        assert!(text.contains("\n  \"version\": 1"));
    }

    #[test]
    fn exports_leave_out_the_api_key_and_imports_keep_it() {
        let mut state = AppState::default();
        state.settings.provider = Provider::OpenAi;
        state.settings.api_key = "sk-secret".into();
        let text = state.to_export("t");
        assert!(!text.contains("sk-secret"));
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["settings"]["provider"], "openai");
        let back =
            AppState::from_export(&format!("{{\"graph\":{{}},\"settings\":{}}}", v["settings"]), &state.settings);
        assert_eq!(back.unwrap().settings.api_key, "sk-secret");
    }

    #[test]
    fn unknown_providers_fall_back_to_ollama() {
        let s: Settings = serde_json::from_str(r#"{"provider":"carrier-pigeon"}"#).unwrap();
        assert_eq!(s.provider, Provider::Ollama);
        let s: Settings = serde_json::from_str(r#"{"provider":"openai"}"#).unwrap();
        assert_eq!((s.provider, s.api_base.as_str()), (Provider::OpenAi, DEFAULT_API_BASE));
    }

    #[test]
    fn saved_streaming_flags_are_cleared() {
        let s = AppState::from_saved(r#"{"chat":[{"id":"1","streaming":true},{"id":"2"}]}"#).unwrap();
        assert_eq!(s.chat[0].streaming, Some(false));
        assert_eq!(s.chat[1].streaming, None);
    }

    #[test]
    fn web_search_is_on_unless_turned_off() {
        assert!(AppState::from_saved("{}").unwrap().settings.web_search);
        assert!(!AppState::from_saved(r#"{"settings":{"webSearch":false}}"#).unwrap().settings.web_search);
    }

    #[test]
    fn research_steps_round_trip() {
        let json = r#"{"chat":[{"id":"1","research":[{"tool":"search","input":"q","pending":true,"links":[{"title":"T","url":"https://x","rank":1}]},{"tool":"fetch","input":"https://y","error":"not found"}]}]}"#;
        let s = AppState::from_saved(json).unwrap();
        let steps = s.chat[0].research.as_deref().unwrap();
        assert_eq!((steps[0].links[0].url.as_str(), steps[0].pending), ("https://x", None));
        assert!(steps[1].is_fetch() && steps[1].error.is_some());
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["chat"][0]["research"][0]["links"][0]["rank"], 1);
        assert!(back["chat"][0]["research"][0].get("pending").is_none());
        let junk = AppState::from_saved(r#"{"chat":[{"id":"1","research":"x"}]}"#).unwrap();
        assert_eq!(junk.chat[0].research, None);
    }

    #[test]
    fn max_notes_is_clamped() {
        let s = |n| Settings { max_notes: n, ..Settings::default() };
        assert_eq!(s(0.0).max_notes_clamped(), 4);
        assert_eq!(s(20.0).max_notes_clamped(), 8);
        assert_eq!(s(-1.0).max_notes_clamped(), 1);
        assert_eq!(s(f64::NAN).max_notes_clamped(), 4);
    }
}
