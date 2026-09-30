use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

/// Result of a `git` command invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommandResult {
    pub exit_code: i32,
    pub output: String,
    pub error: String,
}

impl GitCommandResult {
    pub fn new(exit_code: i32, output: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            exit_code,
            output: output.into(),
            error: error.into(),
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            exit_code: -1,
            output: String::new(),
            error: message.into(),
        }
    }
}

/// A single changed file as reported by `git status --porcelain`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileStatus {
    pub status_code: String,
    pub file_path: String,
    pub display_status: String,
    pub display_path: String,
    pub clean_path: String,
    pub file_name: String,
    pub folder_path: String,
    pub is_untracked: bool,
    pub is_staged: bool,
    pub has_unstaged_changes: bool,
    pub is_modified: bool,
    pub is_added: bool,
    pub is_deleted: bool,
    pub is_conflicted: bool,
    pub is_image: bool,
    pub file_size_bytes: i64,
    pub has_serious_warning: bool,
    pub serious_warning_tooltip: String,
}

/// Working-tree status split into unstaged and staged files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusResult {
    pub unstaged: Vec<GitFileStatus>,
    pub staged: Vec<GitFileStatus>,
}

/// A single entry from the recent commit history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    /// Leading `[Tag]` section of the message, if any (e.g. `[Services\GitService]`).
    pub tag_section: String,
    pub message_section: String,
    pub date: String,
    pub author: String,
}

/// Before/after image payload used by the image diff preview. Bytes are
/// base64-encoded so they survive the JSON bridge to the webview.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDiffData {
    pub file_name: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub before_size: Option<usize>,
    pub after_size: Option<usize>,
    pub has_before: bool,
    pub has_after: bool,
    pub is_svg: bool,
}

/// How a generated commit message is prefixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitPrefixMode {
    AutoFolder,
    Custom,
    Both,
    None,
}

impl GitPrefixMode {
    pub fn wire(self) -> &'static str {
        match self {
            GitPrefixMode::AutoFolder => "AutoFolder",
            GitPrefixMode::Custom => "Custom",
            GitPrefixMode::Both => "Both",
            GitPrefixMode::None => "None",
        }
    }

    pub fn parse_str(raw: &str) -> Self {
        let lower = raw.to_lowercase();
        match lower.as_str() {
            "custom" => GitPrefixMode::Custom,
            "both" => GitPrefixMode::Both,
            "none" => GitPrefixMode::None,
            _ => GitPrefixMode::AutoFolder,
        }
    }

    pub fn parse(value: Option<&Value>) -> Self {
        match value {
            Some(Value::String(s)) => Self::parse_str(s),
            _ => GitPrefixMode::AutoFolder,
        }
    }
}

impl Serialize for GitPrefixMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire())
    }
}

impl<'de> Deserialize<'de> for GitPrefixMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::parse_str(&raw))
    }
}

/// Application colour theme selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppThemeMode {
    System,
    Dark,
    Light,
}

impl AppThemeMode {
    pub fn wire(self) -> &'static str {
        match self {
            AppThemeMode::System => "system",
            AppThemeMode::Dark => "dark",
            AppThemeMode::Light => "light",
        }
    }

    pub fn parse(value: Option<&Value>) -> Self {
        match value {
            Some(Value::String(s)) => match s.to_lowercase().as_str() {
                "dark" => AppThemeMode::Dark,
                "light" => AppThemeMode::Light,
                _ => AppThemeMode::System,
            },
            _ => AppThemeMode::System,
        }
    }
}

impl Serialize for AppThemeMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.wire())
    }
}

impl<'de> Deserialize<'de> for AppThemeMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.to_lowercase().as_str() {
            "dark" => Ok(AppThemeMode::Dark),
            "light" => Ok(AppThemeMode::Light),
            _ => Ok(AppThemeMode::System),
        }
    }
}

/// OpenAI-compatible AI provider settings, persisted as `ai_settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    pub api_url: String,
    pub api_key: String,
    pub model: String,
    pub prefix_mode: GitPrefixMode,
    pub custom_prefix: String,
    pub repository_path: String,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            api_url: "https://api.deepseek.com/chat/completions".to_string(),
            api_key: String::new(),
            model: "deepseek-chat".to_string(),
            prefix_mode: GitPrefixMode::AutoFolder,
            custom_prefix: String::new(),
            repository_path: String::new(),
        }
    }
}

impl AiSettings {
    /// Reads camelCase keys first, then the legacy PascalCase ones.
    pub fn from_value(value: &Value) -> Self {
        let defaults = AiSettings::default();
        Self {
            api_url: str_of(value, "apiUrl", "ApiUrl", &defaults.api_url),
            api_key: str_of(value, "apiKey", "ApiKey", &defaults.api_key),
            model: str_of(value, "model", "Model", &defaults.model),
            prefix_mode: GitPrefixMode::parse(get(value, "prefixMode", "PrefixMode")),
            custom_prefix: str_of(value, "customPrefix", "CustomPrefix", &defaults.custom_prefix),
            repository_path: str_of(
                value,
                "repositoryPath",
                "RepositoryPath",
                &defaults.repository_path,
            ),
        }
    }
}

/// Diff viewer settings, persisted as `diff_settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSettings {
    pub font_size: f64,
    pub font_family: String,
    pub changes_files_width: f64,
    pub tree_depth: u32,
    pub hide_changes_when_staged: bool,
    pub is_single_diff_view: bool,
    pub theme: AppThemeMode,
    pub hide_ai_config: bool,
}

impl Default for DiffSettings {
    fn default() -> Self {
        Self {
            font_size: 12.0,
            font_family: "Cascadia Code".to_string(),
            changes_files_width: 280.0,
            tree_depth: 2,
            hide_changes_when_staged: true,
            is_single_diff_view: false,
            theme: AppThemeMode::System,
            hide_ai_config: true,
        }
    }
}

impl DiffSettings {
    pub fn from_value(value: &Value) -> Self {
        let defaults = DiffSettings::default();
        Self {
            font_size: f64_of(value, "fontSize", "FontSize", defaults.font_size),
            font_family: str_of(value, "fontFamily", "FontFamily", &defaults.font_family),
            changes_files_width: f64_of(
                value,
                "changesFilesWidth",
                "ChangesFilesWidth",
                defaults.changes_files_width,
            ),
            tree_depth: int_of(value, "treeDepth", "TreeDepth", defaults.tree_depth) as u32,
            hide_changes_when_staged: bool_of(
                value,
                "hideChangesWhenStaged",
                "HideChangesWhenStaged",
                defaults.hide_changes_when_staged,
            ),
            is_single_diff_view: bool_of(
                value,
                "isSingleDiffView",
                "IsSingleDiffView",
                defaults.is_single_diff_view,
            ),
            theme: AppThemeMode::parse(get(value, "theme", "Theme")),
            hide_ai_config: bool_of(value, "hideAiConfig", "HideAiConfig", defaults.hide_ai_config),
        }
    }
}

fn get<'a>(value: &'a Value, camel: &str, pascal: &str) -> Option<&'a Value> {
    value
        .get(camel)
        .or_else(|| value.get(pascal))
        .filter(|v| !v.is_null())
}

fn str_of(value: &Value, camel: &str, pascal: &str, fallback: &str) -> String {
    match get(value, camel, pascal) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => fallback.to_string(),
    }
}

fn int_of(value: &Value, camel: &str, pascal: &str, fallback: u32) -> i64 {
    match get(value, camel, pascal) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(fallback as i64),
        Some(other) => other
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(fallback as i64),
        None => fallback as i64,
    }
}

fn f64_of(value: &Value, camel: &str, pascal: &str, fallback: f64) -> f64 {
    match get(value, camel, pascal) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(fallback),
        Some(other) => other
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(fallback),
        None => fallback,
    }
}

fn bool_of(value: &Value, camel: &str, pascal: &str, fallback: bool) -> bool {
    match get(value, camel, pascal) {
        Some(Value::Bool(b)) => *b,
        Some(other) => other
            .as_str()
            .map(|s| s.to_lowercase() == "true")
            .unwrap_or(fallback),
        None => fallback,
    }
}
