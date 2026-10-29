//! What the desktop app and the browser-mode server do on the machine itself: where the data file lives, saving it
//! safely, and starting a local Ollama.

pub mod launcher;
pub mod store;

pub use store::Store;
