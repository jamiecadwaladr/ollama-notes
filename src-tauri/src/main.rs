//! Ollama Notes for the desktop: one window showing the page (ui/, served as tauri://localhost), plus what a browser
//! tab can't do: a menu bar, open/save panels, confirm sheets, a data file in Application Support, and starting
//! Ollama.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod menu;
mod quit;

use notes_native::Store;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// Background of the window before the page paints: the page's top bar colour (--bg2).
const BACKGROUND: tauri::window::Color = tauri::window::Color(0x16, 0x19, 0x23, 0xff);

/// The window only ever shows the app's own page; links open in the default browser.
fn allow_navigation(url: &tauri::Url) -> bool {
    let own = match url.scheme() {
        "tauri" | "about" => true,
        // Windows serves the page from http://tauri.localhost; `cargo tauri dev` from Trunk's dev server
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || (cfg!(debug_assertions) && url.host_str() == Some("127.0.0.1") && url.port() == Some(1420))
        }
        _ => false,
    };
    if !own {
        commands::open_external(url.as_str());
    }
    own
}

fn main() -> anyhow::Result<()> {
    let app = tauri::Builder::default()
        // A second copy would fight the first over the data file, so hand over to the one already running.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Store::open_default())
        .manage(commands::Picked::default())
        .manage(quit::Quit::default())
        .invoke_handler(tauri::generate_handler![
            commands::load_state,
            commands::save_state,
            commands::save_file,
            commands::start_ollama,
            commands::confirm,
            commands::pick_files,
            commands::read_file,
            commands::open_url,
            quit::flush_done,
        ])
        .menu(menu::build)
        .on_menu_event(menu::on_event)
        // Closing the window quits, by way of the page's last save.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && !quit::finished(window.app_handle())
            {
                api.prevent_close();
                quit::begin(window.app_handle());
            }
        })
        .setup(|app| {
            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Ollama Notes")
                .inner_size(1280.0, 820.0)
                .min_inner_size(760.0, 480.0)
                .center()
                .background_color(BACKGROUND)
                .theme(Some(tauri::Theme::Dark))
                // so the page gets ordinary HTML drop events with the files' contents
                .disable_drag_drop_handler()
                .devtools(true)
                .on_navigation(allow_navigation);
            #[cfg(target_os = "macos")]
            let builder = builder.title_bar_style(tauri::TitleBarStyle::Transparent).hidden_title(true);
            builder.build()?;
            Ok(())
        })
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        if let RunEvent::ExitRequested { api, code: None, .. } = event
            && !quit::finished(app)
        {
            // ⌘Q, the Dock's Quit, logging out: let the page save first
            api.prevent_exit();
            quit::begin(app);
        }
    });
    Ok(())
}
