//! The Ollama Notes page, compiled to WebAssembly: a chat with a local Ollama model beside a notes map that fills
//! itself in as you talk. The same page runs in the desktop app and in browser mode (see platform.rs).

mod app;
mod chat;
mod documents;
mod dom;
mod graph_view;
mod import;
mod inspector;
mod ollama;
mod platform;
mod wiring;

fn main() {
    std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&info.to_string().into())));
    let app = app::App::new();
    app.bind();
    // Listeners hold weak references; the app itself lives as long as the page.
    std::mem::forget(app.clone());
    wasm_bindgen_futures::spawn_local(async move { app.start().await });
}
