//! Best-effort start of a local Ollama server: Ollama.app if it's installed, otherwise `ollama serve`.

use notes_core::host::is_local;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Ollama.app's bundle identifier.
pub const OLLAMA_APP_ID: &str = "com.electron.ollama";

/// Starts Ollama if `host` is this machine. Returns what was done ("opened Ollama.app"), or None when the host is
/// another machine or Ollama isn't installed.
pub fn start(host: &str) -> Option<String> {
    if !is_local(host) {
        return None;
    }
    if open_app() {
        return Some("opened Ollama.app".into());
    }
    let exe = find_cli(std::env::var_os("PATH").as_deref().map(Path::new), dirs::home_dir().as_deref())?;
    serve(&exe).then(|| "ran `ollama serve`".into())
}

/// Opens Ollama.app in the background by its bundle id; false if it isn't installed.
#[cfg(target_os = "macos")]
fn open_app() -> bool {
    Command::new("/usr/bin/open")
        .args(["-g", "-b", OLLAMA_APP_ID])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(not(target_os = "macos"))]
fn open_app() -> bool {
    false
}

/// The `ollama` CLI: on PATH, or where the installers put it (apps opened from Finder get a minimal PATH).
pub fn find_cli(path: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = path.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    dirs.extend([PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")]);
    if let Some(home) = home {
        dirs.push(home.join(".local/bin"));
    }
    dirs.into_iter().map(|d| d.join(exe_name())).find(|p| is_executable(p))
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "ollama.exe" } else { "ollama" }
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Runs `ollama serve` detached from this process, with its output discarded.
fn serve(exe: &Path) -> bool {
    let mut cmd = Command::new(exe);
    cmd.arg("serve").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_hosts_are_left_alone() {
        assert_eq!(start("http://gpu-box:11434"), None);
    }

    #[cfg(unix)]
    #[test]
    fn finds_the_cli_on_path_first() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        for d in [&a, &b] {
            std::fs::create_dir(d).unwrap();
        }
        let exe = b.join("ollama");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        assert_eq!(find_cli(Some(&a), Some(dir.path())).filter(|p| p.starts_with(dir.path())), None);
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(find_cli(Some(Path::new(&path)), None), Some(exe));
    }
}
