//! The Ollama server and cloud API addresses: normalizing what the user types in Settings, and telling whether a
//! server is this machine.

use regex::Regex;
use std::sync::LazyLock;

pub const DEFAULT_OLLAMA_HOST: &str = "http://127.0.0.1:11434";
pub const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";

static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*://").unwrap());
static PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":[0-9]+$").unwrap());

/// "localhost" -> "http://localhost:11434", "box:8080" -> "http://box:8080"; a URL with a scheme is kept as it is.
pub fn normalize_host(s: &str) -> String {
    let s = s.trim().trim_end_matches('/');
    if s.is_empty() {
        DEFAULT_OLLAMA_HOST.to_string()
    } else if SCHEME.is_match(s) {
        s.to_string()
    } else if PORT.is_match(s) {
        format!("http://{s}")
    } else {
        format!("http://{s}:11434")
    }
}

/// "api.groq.com/openai/v1" -> "https://api.groq.com/openai/v1", but "localhost:1234/v1" (e.g. LM Studio) gets
/// http://; a pasted endpoint loses its "/chat/completions" or "/models", as requests add those.
pub fn normalize_api_base(s: &str) -> String {
    let mut s = s.trim().trim_end_matches('/');
    for endpoint in ["/chat/completions", "/models"] {
        s = s.strip_suffix(endpoint).unwrap_or(s).trim_end_matches('/');
    }
    if s.is_empty() {
        DEFAULT_API_BASE.to_string()
    } else if SCHEME.is_match(s) {
        s.to_string()
    } else if is_local(&format!("http://{s}")) {
        format!("http://{s}")
    } else {
        format!("https://{s}")
    }
}

/// The host name of a URL ("http://[::1]:11434/x" -> "::1"), lower-cased, or None without a scheme.
pub fn host_name(url: &str) -> Option<String> {
    let rest = &url[url.find("://")? + 3..];
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// Whether a server URL points at this machine, so starting Ollama here makes sense.
pub fn is_local(url: &str) -> bool {
    host_name(url).is_some_and(|h| matches!(h.as_str(), "127.0.0.1" | "localhost" | "0.0.0.0" | "::1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_hosts() {
        assert_eq!(normalize_host(""), DEFAULT_OLLAMA_HOST);
        assert_eq!(normalize_host("  localhost/ "), "http://localhost:11434");
        assert_eq!(normalize_host("box:8080"), "http://box:8080");
        assert_eq!(normalize_host("https://ollama.example///"), "https://ollama.example");
    }

    #[test]
    fn normalizes_api_bases() {
        assert_eq!(normalize_api_base(" "), DEFAULT_API_BASE);
        assert_eq!(normalize_api_base("api.groq.com/openai/v1/"), "https://api.groq.com/openai/v1");
        assert_eq!(normalize_api_base("https://api.openai.com/v1/chat/completions"), "https://api.openai.com/v1");
        assert_eq!(normalize_api_base("http://localhost:1234/v1/models/"), "http://localhost:1234/v1");
        assert_eq!(normalize_api_base("127.0.0.1:1234/v1"), "http://127.0.0.1:1234/v1");
    }

    #[test]
    fn finds_local_hosts() {
        assert!(is_local("http://127.0.0.1:11434"));
        assert!(is_local("http://LOCALHOST"));
        assert!(is_local("http://[::1]:11434/api"));
        assert!(is_local("http://user@0.0.0.0:1"));
        assert!(!is_local("http://gpu-box:11434"));
        assert!(!is_local("localhost:11434"));
        assert_eq!(host_name("http://[::1]:11434"), Some("::1".into()));
    }
}
