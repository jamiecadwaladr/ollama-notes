//! Ollama Notes logic that doesn't touch the DOM or the file system: the saved data model, the notes map and its
//! force layout, keyword extraction and keyword links, text sectioning, the Ollama and cloud API protocols, Markdown,
//! and note extraction. It builds for the page (wasm32) and natively, so all of it is tested with `cargo test`.

pub mod extract;
pub mod graph;
pub mod host;
pub mod keywords;
pub mod layout;
pub mod markdown;
pub mod model;
pub mod ollama;
pub mod openai;
pub mod text;
pub mod view;
