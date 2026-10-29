//! Ollama Notes in a browser tab: starts Ollama if needed, serves the page (ui/dist, built into this binary),
//! proxies /api/* to Ollama, and opens your browser.
//!
//! Usage:  ollama-notes-server  (or double-click "Ollama Notes.command")
//! Env:    PORT, OLLAMA_HOST (default http://127.0.0.1:11434)

use anyhow::{Context, Result};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clap::Parser;
use futures_util::TryStreamExt;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

#[derive(rust_embed::RustEmbed)]
#[folder = "../ui/dist"]
struct Page;

#[derive(Parser)]
#[command(about = "Ollama Notes in a browser tab: serves the UI, proxies /api/* to Ollama, starts Ollama if needed.")]
struct Args {
    /// Listen on this port (the next free one if it's busy)
    #[arg(long, env = "PORT", default_value_t = 8765)]
    port: u16,
    /// Don't open a browser tab
    #[arg(long)]
    no_browser: bool,
    /// Don't try to start Ollama
    #[arg(long)]
    no_ollama: bool,
}

#[derive(Clone)]
struct Ollama {
    /// e.g. http://127.0.0.1:11434
    base: String,
    client: reqwest::Client,
}

/// OLLAMA_HOST as a URL: "host:port" gets http://, and a trailing slash goes.
fn ollama_base() -> String {
    let host = std::env::var("OLLAMA_HOST").unwrap_or_default();
    let host = host.trim().trim_end_matches('/');
    match host {
        "" => notes_core::host::DEFAULT_OLLAMA_HOST.to_string(),
        h if h.contains("://") => h.to_string(),
        h if h.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) => format!("http://{h}"),
        h => format!("http://{h}:11434"),
    }
}

impl Ollama {
    /// The pulled models, or None if Ollama can't be reached.
    async fn models(&self) -> Option<Vec<String>> {
        let r = self.client.get(format!("{}/api/tags", self.base)).timeout(Duration::from_secs(2)).send().await.ok()?;
        let tags = notes_core::ollama::parse_tags(&r.text().await.ok()?).ok()?;
        Some(tags.into_iter().map(|m| m.name).collect())
    }

    async fn wait_for(&self, seconds: u64) -> Option<Vec<String>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        while tokio::time::Instant::now() < deadline {
            if let Some(m) = self.models().await {
                return Some(m);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        None
    }
}

/// Streams /api/* to Ollama and the reply back as it arrives, so token streaming works. When the browser goes away
/// (the Stop button), the reply stream is dropped, which closes the connection and cancels the generation.
async fn proxy(State(ollama): State<Ollama>, req: Request) -> Response {
    let method = req.method().clone();
    let path = req.uri().path_and_query().map_or("/", |p| p.as_str()).to_string();
    let body = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let upstream = ollama
        .client
        .request(method.clone(), format!("{}{path}", ollama.base))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await;
    let r = match upstream {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{method} {path} -> 502 ({e})");
            let msg = format!("Cannot reach Ollama at {} ({e}). Start Ollama.app or run `ollama serve`.", ollama.base);
            let mut resp = (StatusCode::BAD_GATEWAY, serde_json::json!({ "error": msg }).to_string()).into_response();
            resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            return resp;
        }
    };
    eprintln!("{method} {path} -> {}", r.status().as_u16());
    let status = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type =
        r.headers().get(header::CONTENT_TYPE).cloned().unwrap_or(HeaderValue::from_static("application/json"));
    let stream = r.bytes_stream().map_err(std::io::Error::other);
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = status;
    resp.headers_mut().insert(header::CONTENT_TYPE, content_type);
    resp
}

/// The page's files, from the binary (or from ui/dist itself in a debug build).
async fn page(req: Request) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Page::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let mut resp = Response::new(Body::from(Bytes::from(file.data.into_owned())));
            if let Ok(v) = HeaderValue::from_str(mime.as_ref()) {
                resp.headers_mut().insert(header::CONTENT_TYPE, v);
            }
            resp
        }
        None => {
            eprintln!("GET /{path} -> 404");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn no_store(req: Request, next: axum::middleware::Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn app(ollama: Ollama) -> Router {
    let api = axum::routing::get(proxy).post(proxy).delete(proxy);
    Router::new()
        .route("/api/{*rest}", api)
        .fallback(page)
        .layer(axum::middleware::from_fn(no_store))
        .with_state(ollama)
}

/// Whether an Ollama Notes server already answers on this port (then we just open the browser).
async fn already_running(port: u16) -> bool {
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(1)).build() else { return false };
    match client.get(format!("http://127.0.0.1:{port}/")).send().await {
        Ok(r) => r.text().await.is_ok_and(|t| t.contains("<title>Ollama Notes</title>")),
        Err(_) => false,
    }
}

async fn port_free(port: u16) -> bool {
    tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.is_err()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut port = args.port;
    if already_running(port).await {
        let url = format!("http://localhost:{port}");
        println!("Ollama Notes is already running at {url}");
        if !args.no_browser {
            let _ = open::that(&url);
        }
        return Ok(());
    }
    while !port_free(port).await {
        port = port.checked_add(1).context("no free port")?;
    }
    let url = format!("http://localhost:{port}");

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(600))
        .build()?;
    let ollama = Ollama { base: ollama_base(), client };
    let mut models = ollama.models().await;
    if models.is_none() && !args.no_ollama {
        match notes_native::launcher::start(&ollama.base) {
            Some(how) => {
                println!("Ollama not running — {how}, waiting for it…");
                models = ollama.wait_for(25).await;
            }
            None => println!("Ollama not found. Install it from https://ollama.com and run `ollama serve`."),
        }
    }

    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .await
        .with_context(|| format!("listening on port {port}"))?;
    println!("Ollama Notes  →  {url}");
    let status = match &models {
        None => "(NOT reachable yet — the UI will reconnect automatically once Ollama is up)".to_string(),
        Some(m) if m.is_empty() => "(running, but no models pulled — run e.g. `ollama pull llama3.2`)".to_string(),
        Some(m) => format!("({} model(s): {})", m.len(), m.join(", ")),
    };
    println!("Ollama API    →  {}  {status}", ollama.base);
    println!("Press Ctrl+C to stop.");
    if !args.no_browser {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            let _ = open::that(&url);
        });
    }
    axum::serve(listener, app(ollama))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            println!("\nbye");
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_host_from_the_environment() {
        // SAFETY: tests in this module run on one thread and only this test touches OLLAMA_HOST.
        let set = |v: &str| unsafe { std::env::set_var("OLLAMA_HOST", v) };
        set("");
        assert_eq!(ollama_base(), "http://127.0.0.1:11434");
        set("gpu-box:8080/");
        assert_eq!(ollama_base(), "http://gpu-box:8080");
        set("gpu-box");
        assert_eq!(ollama_base(), "http://gpu-box:11434");
        set("https://ollama.example");
        assert_eq!(ollama_base(), "https://ollama.example");
    }
}
