use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::models::{AiSettings, DiffSettings};

/// Root of per-user application data for the current platform. Mirrors the
/// Electron/C#/Flutter apps so existing configuration keeps working.
pub fn app_data_root() -> PathBuf {
    if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("AppData").join("Local"))
    } else if cfg!(target_os = "macos") {
        home_dir().join("Library").join("Application Support")
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"))
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Writes go to `<appData>/EzConsoleGit/settings/` with automatic fallback
/// to legacy EzGit/EleEzGit locations for backward compatibility.
pub fn settings_dir() -> PathBuf {
    app_data_root().join("EzConsoleGit").join("settings")
}

fn legacy_dirs() -> Vec<PathBuf> {
    let root = app_data_root();
    vec![
        root.join("EzGit").join("settings"),
        root.join("EzGit"),
        root.join("EleEzGit").join("settings"),
        root.join("EleEzGit"),
    ]
}

pub fn load_ai_settings() -> AiSettings {
    let mut candidates = vec![settings_dir().join("ai_settings.json")];
    candidates.extend(legacy_dirs().into_iter().map(|d| d.join("ai_settings.json")));
    match read_first_json(&candidates) {
        Some(value) => AiSettings::from_value(&value),
        None => AiSettings::default(),
    }
}

pub fn load_diff_settings() -> DiffSettings {
    let mut candidates = vec![settings_dir().join("diff_settings.json")];
    candidates.extend(legacy_dirs().into_iter().map(|d| d.join("diff_settings.json")));
    match read_first_json(&candidates) {
        Some(value) => DiffSettings::from_value(&value),
        None => DiffSettings::default(),
    }
}

pub fn save_ai_settings(settings: &AiSettings) -> Result<(), String> {
    let value = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    write_json(&settings_dir().join("ai_settings.json"), &value)
}

pub fn save_diff_settings(settings: &DiffSettings) -> Result<(), String> {
    let value = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    write_json(&settings_dir().join("diff_settings.json"), &value)
}

/// Recently opened repository paths, most recent first.
pub fn load_recent_repos() -> Vec<String> {
    let mut candidates = vec![settings_dir().join("recent_repos.json")];
    candidates.extend(legacy_dirs().into_iter().map(|d| d.join("recent_repos.json")));

    for path in candidates {
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(decoded) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if let Value::Array(items) = decoded {
            return items
                .iter()
                .map(|item| match item {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Vec::new()
}

pub fn save_recent_repos(repos: &[String]) -> Result<(), String> {
    let value = serde_json::to_value(repos).map_err(|e| e.to_string())?;
    write_json(&settings_dir().join("recent_repos.json"), &value)
}

fn read_first_json(candidates: &[PathBuf]) -> Option<Value> {
    for path in candidates {
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(decoded) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if decoded.is_object() {
            return Some(decoded);
        }
    }
    None
}

fn write_json(path: &Path, content: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let pretty = serde_json::to_string_pretty(content).map_err(|e| e.to_string())?;
    fs::write(path, pretty).map_err(|e| e.to_string())
}
