use std::path::Path;
use std::process::Command;

/// Platform integrations: open/reveal paths and clipboard access.
pub struct PlatformService;

impl PlatformService {
    /// Opens `path` with the OS default application. Returns true on success.
    pub fn open_path(path: &str) -> bool {
        if !Path::new(path).exists() {
            return false;
        }
        open_external(path)
    }

    /// Reveals `path` in the OS file manager, falling back to opening the folder.
    pub fn reveal_in_folder(path: &str, fallback_dir: &str) -> bool {
        if Path::new(path).exists() && reveal(path) {
            return true;
        }
        if Path::new(fallback_dir).exists() {
            return open_external(fallback_dir);
        }
        false
    }

    /// Copies `text` to the system clipboard.
    pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        clipboard.set_text(text.to_string()).map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "windows")]
fn open_external(path: &str) -> bool {
    // `start` treats the first quoted argument as the window title, hence the "".
    Command::new("cmd")
        .args(["/c", "start", "", path])
        .spawn()
        .is_ok()
}

#[cfg(target_os = "macos")]
fn open_external(path: &str) -> bool {
    Command::new("open").arg(path).spawn().is_ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_external(path: &str) -> bool {
    Command::new("xdg-open").arg(path).spawn().is_ok()
}

#[cfg(target_os = "windows")]
fn reveal(path: &str) -> bool {
    // explorer.exe returns a non-zero exit code even on success, so only the
    // ability to spawn it matters.
    Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn()
        .is_ok()
}

#[cfg(target_os = "macos")]
fn reveal(path: &str) -> bool {
    Command::new("open").args(["-R", path]).spawn().is_ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal(path: &str) -> bool {
    let parent = Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    Command::new("xdg-open").arg(parent).spawn().is_ok()
}
