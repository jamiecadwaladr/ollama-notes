//! Web search for the chat model, through the Ollama server's web search (`/api/experimental/web_search` and
//! `web_fetch`, which use the ollama.com account of `ollama signin`): the tools the model is offered, the prompt that
//! has it research before answering, and the results as the text it reads. Search results carry whole pages, far
//! more than a small model's context holds, so each is cut down to the passages that match the question. Running the
//! tools is the page's job (ui/src/chat.rs).

use crate::host::host_name;
use crate::keywords::word_stems;
use crate::markdown::is_web_link;
use crate::model::{ResearchStep, WebLink};
use crate::ollama::{ChatMessage, error_text};
use crate::text::{char_len, clean, truncate_chars};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::LazyLock;

pub const SEARCH_PATH: &str = "/api/experimental/web_search";
pub const FETCH_PATH: &str = "/api/experimental/web_fetch";
/// Rounds of tool calls before the model has to answer.
pub const MAX_ROUNDS: usize = 4;
/// Tool calls run per round; the model is told any more were skipped.
pub const MAX_CALLS: usize = 3;
/// The context a chat gets while web search is on. Ollama's default of 4096 tokens overflows after a page or two,
/// and the model then loses the start of the conversation, instructions included.
pub const CONTEXT_TOKENS: u32 = 16384;
const SEARCH_RESULTS: u32 = 5;
/// Characters kept from each search result, from a page read with web_fetch, and from any one paragraph.
const RESULT_CHARS: usize = 700;
const PAGE_CHARS: usize = 4000;
const PARAGRAPH_CHARS: usize = 600;

const CITE: &str = "(Cite what you use from these as Markdown links: [title](address).)";
const LAST_ROUND: &str = "That was your last search: answer now, from the results above.";
const SEARCH_OFF: &str = "Web search isn't available for the rest of this answer. Answer from what you know, and say \
                          that you couldn't search the web.";

static PARAGRAPH_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n").unwrap());

/// The tools the chat model is offered.
pub fn tools() -> Value {
    let tool = |name: &str, description: &str, param: &str, about: &str| {
        json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": { param: { "type": "string", "description": about } },
                    "required": [param],
                },
            },
        })
    };
    json!([
        tool(
            "web_search",
            "Search the web. Returns the top results, each with its title, address and the passages most relevant to \
             the question.",
            "query",
            "A short, specific search query, as you'd type it into a search engine",
        ),
        tool(
            "web_fetch",
            "Read a web page, such as a search result whose passages don't answer the question. Returns the parts of \
             the page most relevant to the question.",
            "url",
            "The page's full address, starting with https://",
        ),
    ])
}

/// What the system prompt gains when the model can search: today's date, which the model can't know, and to research
/// instead of answering from memory, citing its sources. Without it, small models search with last year's date in
/// the query, and cite nothing.
pub fn prompt(date: &str) -> String {
    format!(
        "Today is {date}. You have web access: web_search finds pages and web_fetch reads one.

Research before you answer. Your own knowledge is out of date and you can misremember details, so for any question \
about facts, people, events, news, products, software, versions, prices, places or anything else that may have \
changed, call web_search first instead of answering from memory. Use 1–3 short, specific queries (different angles, \
not the same words again), and web_fetch the most relevant result when its passages don't settle the answer. Then \
answer from what you found, and cite your sources inline as Markdown links, like \
[Bloom filter – Wikipedia](https://en.wikipedia.org/wiki/Bloom_filter). If the results don't answer the question, say \
so.

Only skip searching for small talk, creative writing, or reasoning and code that need no outside facts."
    )
}

/// A tool call from the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Search(String),
    Fetch(String),
    /// A tool that doesn't exist, by name.
    Unknown(String),
}

impl Call {
    /// A tool call as the model sent it. Arguments come as an object, or from some models as a JSON string.
    pub fn parse(call: &Value) -> Call {
        let f = call.get("function").unwrap_or(&Value::Null);
        let args = match f.get("arguments") {
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_default(),
            Some(v) => v.clone(),
            None => Value::Null,
        };
        let arg = |k: &str| args.get(k).and_then(Value::as_str).map_or(String::new(), |s| s.trim().to_string());
        match f.get("name").and_then(Value::as_str).unwrap_or("") {
            "web_search" => Call::Search(arg("query")),
            "web_fetch" => Call::Fetch(page_address(&arg("url"))),
            name => Call::Unknown(name.to_string()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Call::Search(_) => "web_search",
            Call::Fetch(_) => "web_fetch",
            Call::Unknown(name) => name,
        }
    }

    /// The step the reply shows for this call while it runs.
    pub fn step(&self) -> ResearchStep {
        let (tool, input) = match self {
            Call::Search(q) => ("search", q.as_str()),
            Call::Fetch(url) => ("fetch", url.as_str()),
            Call::Unknown(name) => (name.as_str(), ""),
        };
        ResearchStep { tool: tool.into(), input: input.into(), pending: Some(true), ..ResearchStep::default() }
    }

    /// The Ollama endpoint and request body that run this call, or why it can't run.
    pub fn request(&self) -> Result<(&'static str, Value), Failure> {
        match self {
            Call::Search(q) if q.is_empty() => Err(Failure::new("web_search needs a query")),
            Call::Search(q) => Ok((SEARCH_PATH, json!({ "query": q, "max_results": SEARCH_RESULTS }))),
            Call::Fetch(url) if !is_web_link(url) => {
                Err(Failure::new("web_fetch needs a page's address, starting with https://"))
            }
            Call::Fetch(url) => Ok((FETCH_PATH, json!({ "url": url }))),
            Call::Unknown(name) => {
                Err(Failure::new(&format!("there is no tool called \"{name}\"; use web_search or web_fetch")))
            }
        }
    }

    /// Ollama's reply to `request`, cut down to the passages about `focus` (the stems of the question and searches).
    pub fn found(&self, reply: &str, focus: &HashSet<String>) -> Result<Found, Failure> {
        let bad = |e: serde_json::Error| Failure::new(&format!("unexpected reply from Ollama ({e})"));
        match self {
            Call::Search(_) => {
                let pages: Vec<Page> = serde_json::from_str::<SearchReply>(reply).map_err(bad)?.results;
                let pages: Vec<Page> = pages.into_iter().filter(|p| is_web_link(&p.url)).collect();
                if pages.is_empty() {
                    return Ok(Found { text: "No results. Try other words.".into(), links: Vec::new() });
                }
                let text: Vec<String> = pages
                    .iter()
                    .map(|p| format!("## {}\n{}", markdown_link(&p.link()), excerpt(&p.content, focus, RESULT_CHARS)))
                    .collect();
                Ok(Found {
                    text: format!("{}\n\n{CITE}", text.join("\n\n")),
                    links: pages.iter().map(Page::link).collect(),
                })
            }
            Call::Fetch(url) => {
                let page = Page { url: url.clone(), ..serde_json::from_str(reply).map_err(bad)? };
                let text = excerpt(&page.content, focus, PAGE_CHARS);
                let text = if text.is_empty() { "The page has no readable text.".into() } else { text };
                Ok(Found {
                    text: format!("## {}\n{text}\n\n{CITE}", markdown_link(&page.link())),
                    links: vec![page.link()],
                })
            }
            Call::Unknown(_) => self.request().map(|_| Found::default()),
        }
    }
}

/// "example.com/a" -> "https://example.com/a": models sometimes leave out the scheme.
fn page_address(url: &str) -> String {
    if url.is_empty() || url.contains("://") { url.to_string() } else { format!("https://{url}") }
}

/// A search result, or a page read with web_fetch.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Page {
    title: String,
    url: String,
    content: String,
}

impl Page {
    fn link(&self) -> WebLink {
        let title = clean(&self.title);
        let title = if title.is_empty() { host_name(&self.url).unwrap_or_else(|| self.url.clone()) } else { title };
        WebLink { title, url: self.url.clone(), ..WebLink::default() }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SearchReply {
    results: Vec<Page>,
}

/// A link that stays one link with brackets in its title, or spaces or parentheses in its address.
fn markdown_link(l: &WebLink) -> String {
    let title = l.title.replace('[', "(").replace(']', ")");
    let url = l.url.replace(' ', "%20").replace('(', "%28").replace(')', "%29");
    format!("[{title}]({url})")
}

/// A tool's result: what the model reads, and the pages the reply shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub text: String,
    pub links: Vec<WebLink>,
}

/// Why a call failed.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub message: String,
    /// No search can work again during this reply: not signed in, the hourly limit, or no web search in this Ollama.
    pub stops_search: bool,
}

impl Failure {
    pub fn new(message: &str) -> Failure {
        Failure { message: message.into(), stops_search: false }
    }

    /// Ollama can't be reached.
    pub fn unreachable(message: &str) -> Failure {
        Failure { message: message.into(), stops_search: true }
    }

    /// An error reply from Ollama, with what to do about it where that's known.
    pub fn from_reply(status: u16, body: &str) -> Failure {
        let text = error_text(status, body);
        match status {
            401 | 403 => Failure::unreachable(&format!(
                "Web search needs an ollama.com account: run `ollama signin`, then try again. ({text})"
            )),
            // Ollama's router rather than a JSON error from the tool: this version has no web search
            404 if serde_json::from_str::<Value>(body).is_err() => {
                Failure::unreachable("This version of Ollama has no web search. Update Ollama to use it.")
            }
            429 => Failure::unreachable(&text),
            _ => Failure::new(&text),
        }
    }

    /// What the model reads in place of results.
    pub fn for_model(&self, call: &Call) -> String {
        let text = format!("{} failed: {}", call.name(), self.message);
        if self.stops_search { format!("{text}\n\n{SEARCH_OFF}") } else { text }
    }
}

/// The passages of a page that matter for `focus` (the stems of the question and searches), in page order, up to
/// `budget` characters: its first paragraph, then the paragraphs sharing the most words with `focus`. When none
/// does, the page from the top.
pub fn excerpt(content: &str, focus: &HashSet<String>, budget: usize) -> String {
    let paragraphs: Vec<&str> = PARAGRAPH_BREAK.split(content).map(str::trim).filter(|p| !p.is_empty()).collect();
    let mut ranked: Vec<(usize, usize)> =
        paragraphs.iter().enumerate().skip(1).map(|(i, p)| (word_stems(p).intersection(focus).count(), i)).collect();
    if ranked.iter().any(|(score, _)| *score > 0) {
        ranked.retain(|(score, _)| *score > 0);
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    let (mut kept, mut left) = (Vec::new(), budget);
    for i in paragraphs.first().map(|_| 0).into_iter().chain(ranked.into_iter().map(|(_, i)| i)) {
        if left < 40 {
            break; // no room for anything useful
        }
        let p = shorten(paragraphs[i], PARAGRAPH_CHARS.min(left));
        left = left.saturating_sub(char_len(&p) + 1);
        kept.push((i, p));
    }
    kept.sort_unstable();
    kept.into_iter().map(|(_, p)| p).collect::<Vec<_>>().join("\n")
}

/// `text` cut to `n` characters, marked with "…" where it was cut.
fn shorten(text: &str, n: usize) -> String {
    if char_len(text) <= n { text.to_string() } else { format!("{}…", truncate_chars(text, n - 1).trim_end()) }
}

/// The message that gives the model a call's result.
pub fn tool_message(call: &Call, content: String) -> ChatMessage {
    ChatMessage { tool_name: Some(call.name().to_string()), ..ChatMessage::new("tool", content) }
}

/// What the model reads for a call beyond `MAX_CALLS` in one round.
pub fn skipped() -> String {
    format!("Not run: at most {MAX_CALLS} searches or page reads at a time.")
}

/// Tells the model, at the end of the last results before its final round, to answer now.
pub fn last_round(messages: &mut [ChatMessage]) {
    if let Some(m) = messages.last_mut() {
        m.content.push_str("\n\n");
        m.content.push_str(LAST_ROUND);
    }
}

/// The text of one reply's rounds (the model may write before it searches, then answer) as one.
pub fn join_rounds(earlier: &str, next: &str) -> String {
    match (earlier.trim().is_empty(), next.trim().is_empty()) {
        (true, _) => next.to_string(),
        (false, true) => earlier.to_string(),
        (false, false) => format!("{}\n\n{}", earlier.trim_end(), next.trim_start()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Value) -> Value {
        json!({ "id": "call_1", "function": { "index": 0, "name": name, "arguments": args } })
    }

    #[test]
    fn offers_two_tools() {
        let t = tools();
        let names: Vec<&str> = t.as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["web_search", "web_fetch"]);
        assert_eq!(t[0]["function"]["parameters"]["required"], json!(["query"]));
        assert!(prompt("Thursday 29 October 2026").starts_with("Today is Thursday 29 October 2026."));
    }

    #[test]
    fn parses_calls() {
        assert_eq!(
            Call::parse(&call("web_search", json!({ "query": " rust 2024 " }))),
            Call::Search("rust 2024".into())
        );
        assert_eq!(Call::parse(&call("web_search", json!("{\"query\":\"q\"}"))), Call::Search("q".into()));
        assert_eq!(Call::parse(&call("web_search", json!("not json"))), Call::Search(String::new()));
        assert_eq!(
            Call::parse(&call("web_fetch", json!({ "url": "ollama.com/blog" }))),
            Call::Fetch("https://ollama.com/blog".into())
        );
        assert_eq!(Call::parse(&call("calculator", json!({}))), Call::Unknown("calculator".into()));
        assert_eq!(Call::parse(&json!({})), Call::Unknown(String::new()));
    }

    #[test]
    fn builds_requests() {
        let (path, body) = Call::Search("q".into()).request().unwrap();
        assert_eq!((path, body), (SEARCH_PATH, json!({ "query": "q", "max_results": 5 })));
        assert_eq!(Call::Fetch("https://x.org".into()).request().unwrap().1, json!({ "url": "https://x.org" }));
        assert!(Call::Search(String::new()).request().is_err());
        assert!(Call::Fetch("ftp://x".into()).request().is_err());
        let unknown = Call::Unknown("calc".into());
        assert!(!unknown.request().unwrap_err().stops_search);
        assert_eq!(unknown.step().tool, "calc");
        let step = Call::Fetch("https://x.org".into()).step();
        assert!(step.is_fetch() && step.is_pending() && step.input == "https://x.org");
    }

    #[test]
    fn failures_say_what_to_do() {
        let limit = r#"{"error":"you have reached your web search hourly request limit, upgrade for higher limits"}"#;
        let f = Failure::from_reply(429, limit);
        assert!(f.stops_search && f.message.starts_with("you have reached"));
        let model_text = f.for_model(&Call::Search("q".into()));
        assert!(model_text.starts_with("web_search failed: you have") && model_text.ends_with(SEARCH_OFF));
        assert!(Failure::from_reply(401, r#"{"error":"unauthorized"}"#).message.contains("ollama signin"));
        let old = Failure::from_reply(404, "404 page not found");
        assert!(old.stops_search && old.message.contains("Update Ollama"));
        let missing = Failure::from_reply(404, r#"{"error":"not found"}"#);
        assert_eq!(missing, Failure::new("not found"));
        assert!(!missing.for_model(&Call::Fetch("u".into())).contains(SEARCH_OFF));
        assert!(!Failure::from_reply(500, "").stops_search);
    }

    #[test]
    fn excerpts_keep_the_passages_about_the_question() {
        let filler = "Unrelated text about gardening and weather. ".repeat(20);
        let page = format!(
            "Bloom filter\n\nA Bloom filter is a probabilistic set.\n\n{filler}\n\nThe false positive rate grows as the filter fills up.\n\n{filler}"
        );
        let focus = word_stems("bloom filter false positive rate");
        let e = excerpt(&page, &focus, 300);
        assert!(e.starts_with("Bloom filter\nA Bloom filter is a probabilistic set.\nThe false positive rate grows"));
        assert!(char_len(&e) <= 300);
        // with nothing matching, the page from the top
        let e = excerpt(&page, &HashSet::new(), 100);
        assert_eq!(
            e,
            "Bloom filter\nA Bloom filter is a probabilistic set.\nUnrelated text about gardening and weather. Unr…"
        );
        assert_eq!(char_len(&excerpt(&"word ".repeat(400), &focus, 5000)), PARAGRAPH_CHARS);
        assert_eq!(excerpt(" \n\n ", &focus, 100), "");
    }

    #[test]
    fn search_results_for_the_model() {
        let reply = json!({ "results": [
            { "title": "Bloom [filter]", "url": "https://en.wikipedia.org/wiki/Bloom_filter_(data)", "content": "A Bloom filter is a set.\n\nMore." },
            { "title": "", "url": "https://example.com/a b", "content": "" },
            { "title": "Not a page", "url": "javascript:alert(1)", "content": "x" },
        ] });
        let found = Call::Search("bloom".into()).found(&reply.to_string(), &word_stems("bloom")).unwrap();
        assert_eq!(
            found.text,
            format!(
                "## [Bloom (filter)](https://en.wikipedia.org/wiki/Bloom_filter_%28data%29)\nA Bloom filter is a set.\nMore.\n\n## [example.com](https://example.com/a%20b)\n\n\n{CITE}"
            )
        );
        let urls: Vec<&str> = found.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["https://en.wikipedia.org/wiki/Bloom_filter_(data)", "https://example.com/a b"]);
        assert_eq!(found.links[0].title, "Bloom [filter]");
        let none = Call::Search("q".into()).found(r#"{"results":[]}"#, &HashSet::new()).unwrap();
        assert!(none.links.is_empty() && none.text.starts_with("No results"));
        assert!(Call::Search("q".into()).found("<html>", &HashSet::new()).is_err());
    }

    #[test]
    fn pages_for_the_model() {
        let reply = json!({ "title": "Release notes", "content": "v1 adds search.", "links": ["https://x.org/a"] });
        let found = Call::Fetch("https://x.org/r".into()).found(&reply.to_string(), &HashSet::new()).unwrap();
        assert_eq!(found.text, format!("## [Release notes](https://x.org/r)\nv1 adds search.\n\n{CITE}"));
        assert_eq!(
            found.links,
            [WebLink { title: "Release notes".into(), url: "https://x.org/r".into(), ..WebLink::default() }]
        );
        let empty = Call::Fetch("https://x.org/r".into()).found("{}", &HashSet::new()).unwrap();
        assert!(empty.text.contains("no readable text") && empty.links[0].title == "x.org");
    }

    #[test]
    fn messages_between_rounds() {
        let call = Call::Search("q".into());
        let mut msgs = vec![tool_message(&call, "results".into())];
        assert_eq!((msgs[0].role.as_str(), msgs[0].tool_name.as_deref()), ("tool", Some("web_search")));
        last_round(&mut msgs);
        assert_eq!(msgs[0].content, format!("results\n\n{LAST_ROUND}"));
        assert!(skipped().contains("at most 3"));
        assert_eq!(join_rounds("", "Answer"), "Answer");
        assert_eq!(join_rounds("Let me look.\n", "\nAnswer"), "Let me look.\n\nAnswer");
        assert_eq!(join_rounds("Answer", " "), "Answer");
    }
}
