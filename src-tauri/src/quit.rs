//! Quitting without losing the last edit: the page saves on a 250 ms debounce, so before the app exits it is asked
//! to write what is still waiting (the page answers with `flush_done`). It never hangs: after 3 s the app quits
//! anyway.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct Quit {
    started: AtomicBool,
    finished: AtomicBool,
}

/// Whether the app is past the page's last save and really exiting.
pub fn finished(app: &AppHandle) -> bool {
    app.state::<Quit>().finished.load(Ordering::SeqCst)
}

pub fn begin(app: &AppHandle) {
    if app.state::<Quit>().started.swap(true, Ordering::SeqCst) {
        return;
    }
    if app.emit_to("main", "flush", ()).is_err() {
        return finish(app);
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        finish(&app);
    });
}

fn finish(app: &AppHandle) {
    if !app.state::<Quit>().finished.swap(true, Ordering::SeqCst) {
        app.exit(0);
    }
}

/// The page has written its last save.
#[tauri::command]
pub fn flush_done(app: AppHandle) {
    finish(&app);
}
