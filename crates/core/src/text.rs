//! Plain-text helpers: whitespace clean-up, document sectioning, titles from file names, and turning pdf.js text
//! items into lines (documents.js).
//!
//! JS counts string lengths in UTF-16 code units; these functions count `char`s. The two only differ for characters
//! outside the Basic Multilingual Plane (emoji), and counting chars can never split one.

use regex::Regex;
use std::sync::LazyLock;

/// The file picker's filter: text-like files and PDFs.
pub const ACCEPT: &str =
    ".txt,.md,.markdown,.text,.csv,.json,.log,.rst,.html,.htm,.xml,.yaml,.yml,.toml,.pdf,text/plain,application/pdf";

static TEXT_EXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\.(txt|md|markdown|text|csv|json|log|rst|html?|xml|yaml|yml|toml|ini|cfg|tex|org)$").unwrap()
});
static PARAGRAPH_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n").unwrap());
static TRAILING_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+\n").unwrap());
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
static EXTENSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.[^.]+$").unwrap());
static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_\-]+").unwrap());
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// The first `n` characters of `s`.
pub fn truncate_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Runs of whitespace become one space, ends trimmed (app.js `clean`).
pub fn clean(s: &str) -> String {
    WHITESPACE.replace_all(s, " ").trim().to_string()
}

/// Unix line ends, no-break spaces as spaces, no trailing spaces, at most one blank line in a row, trimmed.
pub fn normalize(t: &str) -> String {
    let t = t.replace("\r\n", "\n").replace('\r', "\n").replace('\u{a0}', " ");
    let t = TRAILING_SPACE.replace_all(&t, "\n");
    BLANK_LINES.replace_all(&t, "\n\n").trim().to_string()
}

pub fn is_pdf(name: &str, mime: &str) -> bool {
    name.to_lowercase().ends_with(".pdf") || mime == "application/pdf"
}

/// Whether a file is read as text: a known text extension, a text MIME type, or no type at all.
pub fn is_text(name: &str, mime: &str) -> bool {
    TEXT_EXT.is_match(name) || mime.starts_with("text/") || mime == "application/json" || mime.is_empty()
}

pub fn is_html(name: &str, mime: &str) -> bool {
    let name = name.to_lowercase();
    name.ends_with(".html") || name.ends_with(".htm") || mime == "text/html"
}

/// Splits text into sections of roughly `size` characters on paragraph, then sentence, then word boundaries.
pub fn chunk_text(text: &str, size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let push = |cur: &mut String, chunks: &mut Vec<String>| {
        let t = cur.trim();
        if !t.is_empty() {
            chunks.push(t.to_string());
        }
        cur.clear();
    };
    let long = size as f64 * 1.5;
    let min_cut = size as f64 * 0.4;
    for para in PARAGRAPH_BREAK.split(text).map(str::trim).filter(|p| !p.is_empty()) {
        let mut p: Vec<char> = para.chars().collect();
        while p.len() as f64 > long {
            let mut cut = last_index_of(&p, &['.', ' '], size);
            if (cut as f64) < min_cut {
                cut = last_index_of(&p, &[' '], size);
            }
            if (cut as f64) < min_cut {
                cut = size as isize;
            }
            let cut = cut as usize; // < p.len(), since p is longer than 1.5 × size
            push(&mut cur, &mut chunks);
            chunks.push(p[..=cut].iter().collect::<String>().trim().to_string());
            p = p[cut + 1..].iter().collect::<String>().trim().chars().collect();
        }
        let p: String = p.into_iter().collect();
        if !cur.is_empty() && char_len(&cur) + char_len(&p) + 2 > size {
            push(&mut cur, &mut chunks);
        }
        if cur.is_empty() {
            cur = p;
        } else {
            cur.push_str("\n\n");
            cur.push_str(&p);
        }
    }
    push(&mut cur, &mut chunks);
    chunks
}

/// JS `String.prototype.lastIndexOf(pattern, from)` over chars: the last match starting at or before `from`, or -1.
fn last_index_of(haystack: &[char], pattern: &[char], from: usize) -> isize {
    if pattern.len() > haystack.len() {
        return -1;
    }
    let start = from.min(haystack.len() - pattern.len());
    (0..=start).rev().find(|&i| haystack[i..i + pattern.len()] == *pattern).map_or(-1, |i| i as isize)
}

/// A readable title from a file name: "my_report-v2.pdf" -> "my report v2".
pub fn title_from_filename(name: &str) -> String {
    let name = if name.is_empty() { "Document" } else { name };
    let t = EXTENSION.replace(name, "");
    let t = SEPARATORS.replace_all(&t, " ");
    let t = WHITESPACE.replace_all(&t, " ");
    let t = truncate_chars(t.trim(), 80);
    if t.is_empty() { "Document".to_string() } else { t.to_string() }
}

/// One text run from pdf.js `getTextContent()`: its string, baseline position, width and end-of-line flag.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct PdfItem {
    pub s: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub eol: bool,
}

/// A page's text: a new line when the baseline moves, a space where runs are visibly apart.
pub fn pdf_page_text(items: &[PdfItem]) -> String {
    let mut text = String::new();
    let (mut last_x, mut last_y): (Option<f64>, Option<f64>) = (None, None);
    for it in items {
        if let Some(ly) = last_y
            && (it.y - ly).abs() > 2.0
        {
            text.push('\n');
        } else if let Some(lx) = last_x
            && it.x - lx > 1.5
            && !text.is_empty()
            && !text.ends_with(char::is_whitespace)
            && !it.s.starts_with(char::is_whitespace)
        {
            text.push(' ');
        }
        text.push_str(&it.s);
        if it.eol {
            text.push('\n');
        }
        last_x = Some(it.x + it.w);
        last_y = Some(it.y);
    }
    normalize(&text)
}

/// Pages joined with "[Page n]" markers; empty pages are left out.
pub fn join_pdf_pages(pages: &[String]) -> String {
    pages
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.is_empty())
        .map(|(i, t)| format!("[Page {}]\n{t}", i + 1))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn truncates_by_chars() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("hi", 5), "hi");
        assert_eq!(truncate_chars("🙂🙂🙂", 2), "🙂🙂");
    }

    #[test]
    fn normalizes_whitespace() {
        assert_eq!(normalize("a\r\nb\rc\u{a0}d  \n\n\n\ne"), "a\nb\nc d\n\ne");
    }

    #[test]
    fn titles_from_file_names() {
        assert_eq!(title_from_filename("my_report-v2.pdf"), "my report v2");
        assert_eq!(title_from_filename(""), "Document");
        assert_eq!(title_from_filename(".pdf"), "Document");
    }

    #[test]
    fn classifies_files() {
        assert!(is_pdf("A.PDF", ""));
        assert!(is_text("notes.Markdown", "application/octet-stream"));
        assert!(is_text("x", ""));
        assert!(!is_text("photo.png", "image/png"));
        assert!(is_html("page.HTM", ""));
    }

    #[test]
    fn pdf_items_become_lines_and_words() {
        let item = |s: &str, x, y, w, eol| PdfItem { s: s.into(), x, y, w, eol };
        let items = [
            item("Hello", 0.0, 100.0, 30.0, false),
            item("world", 34.0, 100.0, 30.0, false),
            item("Next", 0.0, 80.0, 20.0, true),
        ];
        assert_eq!(pdf_page_text(&items), "Hello world\nNext");
        assert_eq!(join_pdf_pages(&["a".into(), String::new(), "c".into()]), "[Page 1]\na\n\n[Page 3]\nc");
    }

    proptest! {
        #[test]
        fn chunking_keeps_every_word_in_order(text in "(\\PC{0,40}[ .\n]{0,3}){0,60}", size in 20usize..200) {
            let chunks = chunk_text(&text, size);
            let words = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
            let joined: Vec<String> = chunks.iter().flat_map(|c| words(c)).collect();
            // A hard cut can split a word in two, so compare the characters rather than the words.
            prop_assert_eq!(joined.concat(), words(&text).concat());
            for c in &chunks {
                prop_assert!(!c.is_empty());
            }
        }
    }
}
