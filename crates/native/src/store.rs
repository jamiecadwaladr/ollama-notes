//! notes.json: the app's saved state, in `~/Library/Application Support/Ollama Notes` unless set with
//! `defaults write local.ollama-notes DataDirectory <path>` (or the `-DataDirectory <path>` launch argument).
//! The first save of each session keeps the file it replaces as notes.backup.json, so a bad import or "Clear map"
//! can be undone.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const DATA_FILE: &str = "notes.json";
pub const BACKUP_FILE: &str = "notes.backup.json";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{action} {path}: {source}")]
    Io { action: &'static str, path: PathBuf, source: std::io::Error },
}

fn io(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + use<> {
    let path = path.to_path_buf();
    move |source| StoreError::Io { action, path, source }
}

#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    backed_up: AtomicBool,
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir, backed_up: AtomicBool::new(false) }
    }

    /// The store in the configured data directory.
    pub fn open_default() -> Store {
        Store::new(data_dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn data_file(&self) -> PathBuf {
        self.dir.join(DATA_FILE)
    }

    /// The saved state (JSON text), or None before the first save.
    pub fn load(&self) -> Result<Option<String>, StoreError> {
        let path = self.data_file();
        match std::fs::read_to_string(&path) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io("reading", &path)(e)),
        }
    }

    /// Writes the state atomically, keeping the session's starting file as the backup first.
    pub fn save(&self, json: &str) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.dir).map_err(io("creating", &self.dir))?;
        let path = self.data_file();
        if !self.backed_up.swap(true, Ordering::SeqCst) && path.exists() {
            // Best effort, as before: a failed backup must not stop the save.
            let backup = self.dir.join(BACKUP_FILE);
            let _ = std::fs::remove_file(&backup);
            let _ = std::fs::copy(&path, &backup);
        }
        write_atomic(&path, json.as_bytes())
    }
}

/// Writes `bytes` to `path` through a temporary file in the same folder, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(io("creating a temporary file in", dir))?;
    tmp.write_all(bytes).map_err(io("writing", path))?;
    tmp.as_file().sync_all().map_err(io("writing", path))?;
    tmp.persist(path).map_err(|e| io("replacing", path)(e.error))?;
    Ok(())
}

/// `~` and `~/…` expanded to the home folder.
pub fn expand_tilde(p: &str) -> PathBuf {
    match (p.strip_prefix('~'), dirs::home_dir()) {
        (Some(""), Some(home)) => home,
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(p),
    }
}

/// The data directory: the `DataDirectory` setting if there is one, else `<data dir>/Ollama Notes`.
pub fn data_dir() -> PathBuf {
    configured_data_dir()
        .filter(|s| !s.trim().is_empty())
        .map(|s| expand_tilde(s.trim()))
        .unwrap_or_else(|| dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("Ollama Notes"))
}

/// macOS: the app's `DataDirectory` user default, which also covers a `-DataDirectory <path>` launch argument.
#[cfg(target_os = "macos")]
fn configured_data_dir() -> Option<String> {
    use objc2_foundation::{NSString, NSUserDefaults};
    let defaults = NSUserDefaults::standardUserDefaults();
    defaults.stringForKey(&NSString::from_str("DataDirectory")).map(|s| s.to_string())
}

#[cfg(not(target_os = "macos"))]
fn configured_data_dir() -> Option<String> {
    std::env::var("OLLAMA_NOTES_DATA_DIR").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Store::new(dir.path().join("sub")).load().unwrap().is_none());
    }

    #[test]
    fn first_save_of_a_session_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(DATA_FILE), "old").unwrap();
        let store = Store::new(dir.path().to_path_buf());
        store.save("one").unwrap();
        store.save("two").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("two"));
        assert_eq!(std::fs::read_to_string(dir.path().join(BACKUP_FILE)).unwrap(), "old");
        // the next session backs up what this one left
        let next = Store::new(dir.path().to_path_buf());
        next.save("three").unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(BACKUP_FILE)).unwrap(), "two");
        // only the two files: no temporary files left behind
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn no_backup_without_an_earlier_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("new"));
        store.save("{}").unwrap();
        assert!(!dir.path().join("new").join(BACKUP_FILE).exists());
    }

    #[test]
    fn tilde_expands() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/Dropbox/Notes"), home.join("Dropbox/Notes"));
        assert_eq!(expand_tilde("/abs/~x"), PathBuf::from("/abs/~x"));
    }
}
