//! Asking the model for notes and putting them in the map: the JSON schema, the prompts for chat replies and
//! document sections, the chat request with pinned context, and merging extracted notes into the map (app.js).

use crate::graph::Graph;
use crate::model::{Edge, Message, NOTE_KINDS, Node, Source};
use crate::ollama::ChatMessage;
use crate::text::{char_len, clean, truncate_chars};
use serde_json::{Value, json};

/// Plenty for ~8 short notes; stops runaway JSON generation.
pub const EXTRACT_MAX_TOKENS: u32 = 900;
pub const EXTRACT_TIMEOUT_MS: u32 = 90_000;
/// How much of a pinned document's text goes into the system prompt.
pub const DOC_CONTEXT_CHARS: usize = 6000;

pub const CHAT_EXTRACT_SYSTEM: &str = "You turn conversation exchanges into concise, reusable notes for a personal knowledge map. Respond with JSON only.";
pub const DOC_EXTRACT_SYSTEM: &str =
    "You turn document sections into concise, reusable notes for a personal knowledge map. Respond with JSON only.";

/// The shape note extraction asks for.
pub fn note_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "topic": { "type": "string" },
            "notes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "maxLength": 80 },
                        "summary": { "type": "string", "maxLength": 400 },
                        "kind": { "type": "string", "enum": NOTE_KINDS },
                        "related": { "type": "array", "items": { "type": "string", "maxLength": 80 }, "maxItems": 3 },
                        "keywords": { "type": "array", "items": { "type": "string", "maxLength": 40 }, "maxItems": 5 },
                    },
                    "required": ["title", "summary", "kind"],
                },
                "maxItems": 8,
            },
        },
        "required": ["topic", "notes"],
    })
}

/// Extraction request options: low temperature, a token cap, and a penalty against looping.
pub fn extract_options() -> Value {
    json!({ "temperature": 0.2, "num_predict": EXTRACT_MAX_TOKENS, "repeat_penalty": 1.15 })
}

/// The last 80 note titles, for the model to link to and avoid repeating.
pub fn existing_titles(g: &Graph) -> String {
    let titles: Vec<&str> = g.nodes.values().map(|n| n.title.as_str()).filter(|t| !t.is_empty()).collect();
    titles[titles.len().saturating_sub(80)..].join("; ")
}

fn or_none(s: &str) -> &str {
    if s.is_empty() { "(none)" } else { s }
}

/// The prompt that turns one chat exchange into notes.
pub fn chat_prompt(topic: Option<&str>, existing: &str, user: &str, assistant: &str, max: usize) -> String {
    format!(
        "Conversation topic: {topic}
Existing note titles: {existing}

USER:
{user}

ASSISTANT:
{assistant}

Extract 1 to {max} distinct notes from this exchange. Rules:
- \"title\": at most 6 words, specific and self-explanatory.
- \"summary\": 1–2 self-contained sentences someone could understand without the conversation.
- \"kind\": one of concept, fact, decision, question, task.
- \"related\": up to 3 exact existing note titles this note connects to (may be empty).
- \"keywords\": 2–5 important keywords or short phrases for this note (specific terms, not generic words).
- Skip anything already covered by an existing note unless it adds new information.
- \"topic\": a short label (at most 5 words) for the whole conversation.
Keep the JSON compact; no commentary before or after it.",
        topic = topic.unwrap_or("(not set yet — propose one)"),
        existing = or_none(existing),
        user = truncate_chars(user, 4000),
        assistant = truncate_chars(assistant, 6000),
    )
}

/// The prompt that turns one document section into notes.
pub fn doc_prompt(title: &str, section: usize, sections: usize, existing: &str, text: &str, max: usize) -> String {
    format!(
        "Document: {title} (section {section} of {sections})
Existing note titles: {existing}

TEXT:
{text}

Extract 1 to {max} distinct notes from this section. Rules:
- \"title\": at most 6 words, specific and self-explanatory.
- \"summary\": 1–2 self-contained sentences someone could understand without the document.
- \"kind\": one of concept, fact, decision, question, task.
- \"related\": up to 3 exact existing note titles this note connects to (may be empty).
- \"keywords\": 2–5 important keywords or short phrases for this note (specific terms, not generic words).
- Skip anything already covered by an existing note unless it adds new information.
- \"topic\": a short label (at most 5 words) for the document.
Keep the JSON compact; no commentary before or after it.",
        existing = or_none(existing),
    )
}

/// The chat request: the system prompt with any pinned notes, then the last 30 finished messages.
pub fn build_messages(system: &str, pinned: &[&Node], chat: &[Message]) -> Vec<ChatMessage> {
    let mut sys = system.to_string();
    if !pinned.is_empty() {
        sys.push_str(
            "\n\nThe user has pinned these notes from their knowledge map as context for this conversation:\n",
        );
        let lines: Vec<String> = pinned
            .iter()
            .map(|n| match n.doc_text() {
                Some(text) => format!(
                    "- {} (document{}):\n{}",
                    n.title,
                    if char_len(text) > DOC_CONTEXT_CHARS { ", first part" } else { "" },
                    truncate_chars(text, DOC_CONTEXT_CHARS)
                ),
                None if n.body.is_empty() => format!("- {}", n.title),
                None => format!("- {}: {}", n.title, n.body),
            })
            .collect();
        sys.push_str(&lines.join("\n"));
    }
    let history: Vec<&Message> =
        chat.iter().filter(|m| !m.is_streaming() && !m.content.is_empty() && m.error.is_none()).collect();
    let mut out = vec![ChatMessage::new("system", sys)];
    out.extend(
        history[history.len().saturating_sub(30)..].iter().map(|m| ChatMessage::new(&m.role, m.content.clone())),
    );
    out
}

/// What merging an extraction did: the new notes, and every note it touched, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub created: Vec<String>,
    pub nodes: Vec<String>,
}

/// Where new things get their ids and positions from.
pub struct Placement<'a> {
    pub new_id: &'a mut dyn FnMut() -> String,
    pub now: f64,
    /// A free spot near a note, given the map as it is at that moment.
    pub place_near: &'a mut dyn FnMut(&Graph, &str) -> (f64, f64),
}

fn strings(v: Option<&Value>) -> Vec<&Value> {
    v.and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default()
}

fn clean_value(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).map(clean).unwrap_or_default()
}

/// Puts an extraction result's notes in the map, each linked to `hub_id` and to the existing notes it names. A
/// note whose title already exists updates that note instead.
pub fn apply_notes(g: &mut Graph, result: &Value, hub_id: &str, source: &Source, at: &mut Placement) -> Applied {
    let mut applied = Applied::default();
    for raw in strings(result.get("notes")).into_iter().take(8) {
        let title = truncate_chars(&clean_value(raw.get("title")), 80).to_string();
        if title.is_empty() {
            continue;
        }
        let summary = clean_value(raw.get("summary"));
        let kw_model: Vec<String> = strings(raw.get("keywords"))
            .into_iter()
            .map(|k| clean_value(Some(k)))
            .filter(|k| !k.is_empty() && char_len(k) <= 40)
            .take(5)
            .collect();
        let id = match g.find_by_title(&title).map(|n| n.id.clone()) {
            Some(id) => {
                let n = &mut g.nodes[&id];
                if !summary.is_empty() && !n.body.is_empty() && !n.body.contains(&summary) {
                    n.body = format!("{}\n\n{summary}", n.body);
                } else if !summary.is_empty() && n.body.is_empty() {
                    n.body = summary;
                }
                if !kw_model.is_empty() {
                    let mut merged = n.kw_model.take().unwrap_or_default();
                    for k in kw_model {
                        if !merged.contains(&k) {
                            merged.push(k);
                        }
                    }
                    merged.truncate(8);
                    n.kw_model = Some(merged);
                }
                id
            }
            None => {
                let kind =
                    raw.get("kind").and_then(Value::as_str).filter(|k| NOTE_KINDS.contains(k)).unwrap_or("concept");
                let (x, y) = (at.place_near)(g, hub_id);
                let node = Node {
                    title,
                    kind: kind.into(),
                    body: summary,
                    kw_model: Some(kw_model),
                    source: Some(source.clone()),
                    x,
                    y,
                    ..Node::new((at.new_id)(), at.now)
                };
                let id = g.add_node(node);
                applied.created.push(id.clone());
                id
            }
        };
        link(g, hub_id, &id, at);
        for rel in strings(raw.get("related")) {
            let other = g.find_by_title(&clean_value(Some(rel))).map(|n| n.id.clone());
            if let Some(other) = other.filter(|o| *o != id) {
                link(g, &other, &id, at);
            }
        }
        applied.nodes.push(id);
    }
    applied
}

fn link(g: &mut Graph, from: &str, to: &str, at: &mut Placement) {
    if from != to && g.nodes.contains_key(from) && g.nodes.contains_key(to) && g.has_edge(from, to).is_none() {
        g.add_edge(Edge { id: (at.new_id)(), from: from.into(), to: to.into(), ..Edge::default() });
    }
}

/// The text for "Save as note": the reply's first line, without Markdown, as the title.
pub fn note_title_from_reply(content: &str) -> String {
    let first = content.split('\n').find(|l| !l.trim().is_empty()).unwrap_or("Note");
    let t = first.trim_start_matches('#').trim_start();
    let t: String = t.chars().filter(|c| !matches!(c, '*' | '`' | '_' | '>')).collect();
    truncate_chars(t.trim(), 70).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement<'a>(
        ids: &'a mut dyn FnMut() -> String,
        place: &'a mut dyn FnMut(&Graph, &str) -> (f64, f64),
    ) -> Placement<'a> {
        Placement { new_id: ids, now: 1.0, place_near: place }
    }

    #[test]
    fn applies_and_merges_notes() {
        let mut g = Graph::default();
        g.add_node(Node { title: "Hub".into(), ..Node::new("hub".into(), 0.0) });
        g.add_node(Node { title: "Hash sets".into(), body: "Old.".into(), ..Node::new("hs".into(), 0.0) });
        let result = json!({ "notes": [
            { "title": "  Hash   Sets ", "summary": "Fast lookups.", "keywords": ["hashing", ""] },
            { "title": "Bloom filters", "summary": "Probabilistic.", "kind": "weird", "related": ["hash sets", "nope"], "keywords": ["bloom"] },
            { "title": "" },
        ]});
        let mut n = 0;
        let mut ids = || {
            n += 1;
            format!("id{n}")
        };
        let mut place = |_: &Graph, _: &str| (5.0, 6.0);
        let r = apply_notes(&mut g, &result, "hub", &Source::default(), &mut placement(&mut ids, &mut place));
        assert_eq!(r.created.len(), 1);
        assert_eq!(r.nodes, vec!["hs".to_string(), r.created[0].clone()]);
        assert_eq!(g.nodes["hs"].body, "Old.\n\nFast lookups.");
        assert_eq!(g.nodes["hs"].kw_model, Some(vec!["hashing".to_string()]));
        let bloom = &g.nodes[&r.created[0]];
        assert_eq!((bloom.kind.as_str(), bloom.x, bloom.y), ("concept", 5.0, 6.0));
        assert_eq!(g.degree("hub"), 2);
        assert!(g.has_edge("hs", &r.created[0]).is_some());
    }

    #[test]
    fn pinned_context_and_history() {
        let doc = Node { title: "Paper".into(), text: Some("x".repeat(7000)), ..Node::default() };
        let note = Node { title: "Idea".into(), body: "Details".into(), ..Node::default() };
        let msg =
            |role: &str, content: &str| Message { role: role.into(), content: content.into(), ..Message::default() };
        let mut chat: Vec<Message> = (0..40).map(|i| msg("user", &format!("m{i}"))).collect();
        chat.push(Message { error: Some("x".into()), ..msg("assistant", "bad") });
        chat.push(Message { streaming: Some(true), ..msg("assistant", "") });
        let out = build_messages("Sys", &[&doc, &note], &chat);
        assert!(out[0].content.starts_with("Sys\n\nThe user has pinned"));
        assert!(out[0].content.contains("- Paper (document, first part):\nxxx"));
        assert!(out[0].content.ends_with("\n- Idea: Details"));
        assert_eq!(out.len(), 31);
        assert_eq!(out[1].content, "m10");
    }

    #[test]
    fn prompts_include_limits() {
        let p = chat_prompt(None, "", "u", "a", 4);
        assert!(p.starts_with("Conversation topic: (not set yet — propose one)\nExisting note titles: (none)\n"));
        assert!(p.contains("Extract 1 to 4 distinct notes from this exchange."));
        let p = doc_prompt("Doc", 2, 5, "A; B", "TEXT", 3);
        assert!(p.starts_with("Document: Doc (section 2 of 5)\nExisting note titles: A; B\n\nTEXT:\nTEXT\n"));
    }

    #[test]
    fn reply_titles() {
        assert_eq!(note_title_from_reply("\n## **Big** `idea`\nmore"), "Big idea");
        assert_eq!(note_title_from_reply(""), "Note");
    }
}
