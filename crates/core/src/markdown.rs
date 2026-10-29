//! Markdown for model replies, rendered with pulldown-cmark and made safe: raw HTML in the source is shown as text,
//! only http(s) links survive (opening outside the app), and images become plain links so a reply never loads
//! anything. Headings start at h3, as the chat's styles expect.

use pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn is_web_link(url: &str) -> bool {
    let u = url.trim_start().to_ascii_lowercase();
    u.starts_with("http://") || u.starts_with("https://")
}

fn shift(level: HeadingLevel) -> HeadingLevel {
    match level {
        HeadingLevel::H1 => HeadingLevel::H3,
        HeadingLevel::H2 => HeadingLevel::H4,
        HeadingLevel::H3 => HeadingLevel::H5,
        _ => HeadingLevel::H6,
    }
}

/// "• item" lines (common in model output) as Markdown list items.
fn bullets(src: &str) -> String {
    src.split('\n')
        .map(|l| {
            let t = l.trim_start();
            match t.strip_prefix('•') {
                Some(rest) if rest.starts_with([' ', '\t']) => format!("{}-{rest}", &l[..l.len() - t.len()]),
                _ => l.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render(src: &str) -> String {
    if src.is_empty() {
        return String::new();
    }
    let src = bullets(src);
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut links: Vec<bool> = Vec::new(); // per open link/image: whether an <a> was written for it
    let events = Parser::new_ext(&src, opts).filter_map(|ev| match ev {
        Event::Html(s) | Event::InlineHtml(s) => Some(Event::Text(s)),
        Event::Start(Tag::Heading { level, id, classes, attrs }) => {
            Some(Event::Start(Tag::Heading { level: shift(level), id, classes, attrs }))
        }
        Event::End(TagEnd::Heading(level)) => Some(Event::End(TagEnd::Heading(shift(level)))),
        Event::Start(Tag::Link { dest_url, .. }) | Event::Start(Tag::Image { dest_url, .. }) => {
            let ok = is_web_link(&dest_url);
            links.push(ok);
            ok.then(|| {
                Event::Html(CowStr::from(format!(
                    "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">",
                    escape_attr(&dest_url)
                )))
            })
        }
        Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => {
            links.pop().unwrap_or(false).then(|| Event::Html(CowStr::from("</a>")))
        }
        other => Some(other),
    });
    let mut out = String::with_capacity(src.len() * 3 / 2);
    html::push_html(&mut out, events);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_html_is_shown_as_text() {
        let out = render("Hi <script>alert(1)</script>\n\n<div onclick=x>block</div>");
        assert!(!out.contains("<script") && !out.contains("<div"));
        assert!(out.contains("&lt;script&gt;"));
    }

    #[test]
    fn only_web_links_survive() {
        let out = render(
            "[ok](https://example.com/a?b=\"c\") [bad](javascript:alert(1)) <mailto:a@b.c> ![img](http://x/i.png)",
        );
        assert!(
            out.contains("<a href=\"https://example.com/a?b=&quot;c&quot;\" target=\"_blank\" rel=\"noopener\">ok</a>")
        );
        assert!(!out.contains("javascript:"));
        assert!(out.contains("bad"));
        assert!(!out.contains("<img"));
        assert!(out.contains("<a href=\"http://x/i.png\" target=\"_blank\" rel=\"noopener\">img</a>"));
        assert!(!out.contains("href=\"mailto") && out.contains("mailto:a@b.c"));
    }

    #[test]
    fn headings_code_lists_and_tables() {
        let out = render("# Title\n\n• one\n• two\n\n```rust\nlet x = 1 < 2;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(out.contains("<h3>Title</h3>"));
        assert!(out.contains("<ul>\n<li>one</li>\n<li>two</li>\n</ul>"));
        assert!(out.contains("<pre><code class=\"language-rust\">let x = 1 &lt; 2;\n</code></pre>"));
        assert!(out.contains("<table>"));
        assert_eq!(render(""), "");
    }
}
