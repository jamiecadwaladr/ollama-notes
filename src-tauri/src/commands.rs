//! What the page asks of the app (ui/src/platform.rs): the data file, file panels, confirm sheets, starting Ollama
//! and opening links.

use notes_native::{Store, launcher, store::write_atomic};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{State, WebviewWindow};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

/// What can be imported as documents: text-like files and PDFs.
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "text", "csv", "json", "log", "rst", "html", "htm", "xml", "yaml", "yml", "toml", "ini",
    "cfg", "tex", "org", "pdf",
];

/// Files the user picked in an open panel this session: the only ones the page may read.
#[derive(Default)]
pub struct Picked(Mutex<HashSet<PathBuf>>);

#[derive(Serialize)]
pub struct PickedFile {
    name: String,
    path: String,
}

/// The saved state (JSON text), or None before the first save.
#[tauri::command]
pub fn load_state(store: State<Store>) -> Result<Option<String>, String> {
    store.load().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_state(store: State<Store>, json: String) -> Result<(), String> {
    store.save(&json).map_err(|e| e.to_string())
}

/// Asks where to save `text`; returns the path, or None if cancelled.
#[tauri::command]
pub async fn save_file(window: WebviewWindow, name: String, text: String) -> Result<Option<String>, String> {
    let mut dialog = window.dialog().file().set_parent(&window).set_file_name(&name);
    if name.to_lowercase().ends_with(".json") {
        dialog = dialog.add_filter("JSON", &["json"]);
    }
    let Some(path) = dialog.blocking_save_file() else { return Ok(None) };
    let path = path.into_path().map_err(|e| e.to_string())?;
    write_atomic(&path, text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(Some(path.display().to_string()))
}

/// Starts Ollama if `host` is this machine; returns what was done.
#[tauri::command]
pub fn start_ollama(host: String) -> Option<String> {
    launcher::start(&host)
}

/// "Delete everything? (Export first.)" -> the question in bold, with the rest as the explanation below it.
fn split_question(message: &str) -> (String, String) {
    match message.split_once('?') {
        Some((q, rest)) => (format!("{q}?"), rest.trim().to_string()),
        None => (message.to_string(), String::new()),
    }
}

/// An OK/Cancel sheet on the window.
#[tauri::command]
pub async fn confirm(window: WebviewWindow, message: String) -> bool {
    let (question, detail) = split_question(&message);
    window
        .dialog()
        .message(detail)
        .title(question)
        .buttons(MessageDialogButtons::OkCancel)
        .parent(&window)
        .blocking_show()
}

/// The open panel: several documents, or one exported map.
#[tauri::command]
pub async fn pick_files(
    window: WebviewWindow,
    picked: State<'_, Picked>,
    kind: String,
) -> Result<Vec<PickedFile>, String> {
    let dialog = window.dialog().file().set_parent(&window);
    let paths = if kind == "map" {
        dialog.add_filter("Ollama Notes export", &["json"]).blocking_pick_file().map(|p| vec![p])
    } else {
        dialog.add_filter("Text files and PDFs", DOCUMENT_EXTENSIONS).blocking_pick_files()
    };
    let mut files = Vec::new();
    for p in paths.unwrap_or_default() {
        let path = p.into_path().map_err(|e| e.to_string())?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        picked.0.lock().map_err(|e| e.to_string())?.insert(path.clone());
        files.push(PickedFile { name, path: path.display().to_string() });
    }
    Ok(files)
}

/// A picked file's bytes.
#[tauri::command]
pub fn read_file(picked: State<Picked>, path: String) -> Result<tauri::ipc::Response, String> {
    let path = PathBuf::from(path);
    if !picked.0.lock().map_err(|e| e.to_string())?.contains(&path) {
        return Err("only files chosen in the open panel can be read".into());
    }
    std::fs::read(&path).map(tauri::ipc::Response::new).map_err(|e| format!("{}: {e}", path.display()))
}

/// Opens web and mail links in the default browser or mail app; anything else is ignored.
pub fn open_external(url: &str) {
    let lower = url.to_ascii_lowercase();
    if ["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s)) {
        let _ = tauri_plugin_opener::open_url(url, None::<&str>);
    }
}

#[tauri::command]
pub fn open_url(url: String) {
    open_external(&url);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_split_into_title_and_detail() {
        assert_eq!(
            split_question("Delete every note and link in the map? (Export first if you want a backup.)"),
            ("Delete every note and link in the map?".into(), "(Export first if you want a backup.)".into())
        );
        assert_eq!(split_question("Sure"), ("Sure".into(), String::new()));
    }
}
