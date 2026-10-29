//! The menu bar. Items with a `cmd:` id run the page's command of that name (`App::command` in ui/src/wiring.rs).

use notes_native::Store;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Runtime};

fn command<R: Runtime>(app: &AppHandle<R>, name: &str, title: &str, key: Option<&str>) -> tauri::Result<MenuItem<R>> {
    MenuItem::with_id(app, format!("cmd:{name}"), title, true, key)
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let sep = || PredefinedMenuItem::separator(app);
    let app_menu = SubmenuBuilder::new(app, "Ollama Notes")
        .item(&PredefinedMenuItem::about(app, Some("About Ollama Notes"), None)?)
        .item(&sep()?)
        .item(&command(app, "settings", "Settings…", Some("CmdOrCtrl+,"))?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::services(app, None)?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::hide(app, Some("Hide Ollama Notes"))?)
        .item(&PredefinedMenuItem::hide_others(app, None)?)
        .item(&PredefinedMenuItem::show_all(app, None)?)
        .item(&sep()?)
        // not the predefined Quit, so quitting goes through the page's last save
        .item(&MenuItem::with_id(app, "quit", "Quit Ollama Notes", true, Some("CmdOrCtrl+Q"))?)
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&command(app, "new-chat", "New Chat", Some("CmdOrCtrl+N"))?)
        .item(&command(app, "new-note", "New Note", Some("CmdOrCtrl+Shift+N"))?)
        .item(&sep()?)
        .item(&command(app, "import-files", "Import Files…", Some("CmdOrCtrl+O"))?)
        .item(&command(app, "import-map", "Import Map…", Some("CmdOrCtrl+Shift+O"))?)
        .item(&command(app, "export-map", "Export Map…", Some("CmdOrCtrl+Shift+S"))?)
        .item(&sep()?)
        .item(&MenuItem::with_id(app, "show-data", "Show Data in Finder", true, None::<&str>)?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::close_window(app, Some("Close Window"))?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&command(app, "fit", "Fit Map", Some("CmdOrCtrl+0"))?)
        .item(&command(app, "zoom-in", "Zoom In", Some("CmdOrCtrl+="))?)
        .item(&command(app, "zoom-out", "Zoom Out", Some("CmdOrCtrl+-"))?)
        .item(&command(app, "tidy", "Tidy Layout", None)?)
        .item(&sep()?)
        .item(&command(app, "focus-chat", "Focus Chat", Some("CmdOrCtrl+L"))?)
        .item(&command(app, "filter-notes", "Filter Notes", Some("CmdOrCtrl+F"))?)
        .item(&sep()?)
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .item(&PredefinedMenuItem::minimize(app, None)?)
        .item(&PredefinedMenuItem::maximize(app, Some("Zoom"))?)
        .build()?;
    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[&MenuItem::with_id(app, "model-library", "Ollama Model Library", true, None::<&str>)?],
    )?;
    #[cfg(target_os = "macos")]
    {
        window.set_as_windows_menu_for_nsapp()?;
        help.set_as_help_menu_for_nsapp()?;
    }
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window, &help])
}

pub fn on_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "quit" => crate::quit::begin(app),
        "show-data" => show_data(app),
        "model-library" => crate::commands::open_external("https://ollama.com/library"),
        id => {
            if let Some(name) = id.strip_prefix("cmd:") {
                let _ = app.emit_to("main", "menu", name);
            }
        }
    }
}

/// Reveals notes.json in Finder, or opens its folder before the first save.
fn show_data(app: &AppHandle) {
    let store = app.state::<Store>();
    let file = store.data_file();
    if file.exists() {
        let _ = tauri_plugin_opener::reveal_item_in_dir(&file);
    } else {
        let _ = std::fs::create_dir_all(store.dir());
        let _ = tauri_plugin_opener::open_path(store.dir().display().to_string(), None::<&str>);
    }
}
