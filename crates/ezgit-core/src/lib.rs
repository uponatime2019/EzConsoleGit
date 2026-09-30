//! GUI-free core services shared by the EzGitRs desktop shell.
//!
//! Everything that talks to `git`, the AI HTTP endpoint, the settings files or
//! the operating system lives here so it can be unit-tested without pulling in
//! Tauri or the webview.

pub mod ai_service;
pub mod git_service;
pub mod models;
pub mod platform_service;
pub mod settings_store;
