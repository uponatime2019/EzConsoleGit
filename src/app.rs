//! Application state and the whole input → action mapping.

use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ezgit_core::models::{
    AiSettings, AppThemeMode, DiffSettings, GitCommitInfo, GitFileStatus, GitPrefixMode,
    ImageDiffData,
};
use ezgit_core::platform_service::PlatformService;
use ezgit_core::{git_service, settings_store};

use crate::diff_lines::{self, DiffLine};
use crate::grouping::{self, Group};
use crate::jobs::{self, Ctx, Job, Msg, ResetEntry};
use crate::text_input::TextInput;

pub const SIDEBAR_MIN: u16 = 24;
pub const SIDEBAR_MAX: u16 = 80;
pub const SIDEBAR_DEFAULT: u16 = 40;
/// Collapsed groups still show this many files, matching the desktop apps.
pub const GROUP_PREVIEW: usize = 10;
pub const COMMITS_PER_PAGE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Changes,
    Diff,
    Commit,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Focus::Changes => Focus::Diff,
            Focus::Diff => Focus::Commit,
            Focus::Commit => Focus::Changes,
        }
    }

    fn prev(self) -> Self {
        match self {
            Focus::Changes => Focus::Commit,
            Focus::Diff => Focus::Changes,
            Focus::Commit => Focus::Diff,
        }
    }
}

/// One rendered line of the changes panel.
pub enum Row {
    Section {
        label: String,
        staged: bool,
    },
    Group {
        group: Group,
        staged: bool,
        expanded: bool,
    },
    File {
        file: GitFileStatus,
        staged: bool,
    },
    Empty(String),
}

impl Row {
    pub fn is_selectable(&self) -> bool {
        matches!(self, Row::Group { .. } | Row::File { .. })
    }

    pub fn staged(&self) -> bool {
        match self {
            Row::Section { staged, .. } | Row::Group { staged, .. } | Row::File { staged, .. } => {
                *staged
            }
            Row::Empty(_) => false,
        }
    }
}

/// Identity of the highlighted row, so it can survive a refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub staged: bool,
    pub is_group: bool,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTarget {
    pub path: Option<String>,
    pub staged: bool,
    pub commit: Option<String>,
}

pub struct ConfirmAction {
    pub title: String,
    pub message: String,
    pub status: String,
    pub job: Job,
}

pub enum PromptAction {
    CommitFiles { paths: Vec<String> },
}

pub enum Overlay {
    None,
    Help,
    Repos(ReposState),
    Commits(CommitsState),
    Settings(Box<SettingsForm>),
    Confirm(ConfirmAction),
    Prompt {
        title: String,
        hint: String,
        input: TextInput,
        action: PromptAction,
    },
}

#[derive(Default)]
pub struct ReposState {
    pub sel: usize,
    pub input: TextInput,
    pub typing: bool,
}

#[derive(Default)]
pub struct CommitsState {
    pub sel: usize,
    pub page: usize,
}

/// Editable mirror of `ai_settings.json`.
pub struct SettingsForm {
    pub focus: usize,
    pub api_url: TextInput,
    pub api_key: TextInput,
    pub model: TextInput,
    pub custom_prefix: TextInput,
    pub repo: TextInput,
    pub prefix_mode: GitPrefixMode,
    pub show_key: bool,
    pub testing: bool,
    pub test: Option<(bool, String)>,
}

impl SettingsForm {
    pub const FIELD_COUNT: usize = 9;
    pub const TEST: usize = 6;
    pub const SAVE: usize = 7;
    pub const CANCEL: usize = 8;

    pub fn new(ai: &AiSettings) -> Self {
        Self {
            focus: 0,
            api_url: TextInput::new(ai.api_url.clone()),
            api_key: TextInput::new(ai.api_key.clone()),
            model: TextInput::new(ai.model.clone()),
            custom_prefix: TextInput::new(ai.custom_prefix.clone()),
            repo: TextInput::new(ai.repository_path.clone()),
            prefix_mode: ai.prefix_mode,
            show_key: false,
            testing: false,
            test: None,
        }
    }

    pub fn is_text_field(&self) -> bool {
        matches!(self.focus, 0 | 1 | 2 | 4 | 5)
    }

    pub fn input_mut(&mut self) -> Option<&mut TextInput> {
        match self.focus {
            0 => Some(&mut self.api_url),
            1 => Some(&mut self.api_key),
            2 => Some(&mut self.model),
            4 => Some(&mut self.custom_prefix),
            5 => Some(&mut self.repo),
            _ => None,
        }
    }

    pub fn to_settings(&self, fallback: &AiSettings) -> AiSettings {
        let api_url = self.api_url.value().trim();
        AiSettings {
            api_url: if api_url.is_empty() {
                fallback.api_url.clone()
            } else {
                api_url.to_string()
            },
            api_key: self.api_key.value().to_string(),
            model: self.model.value().to_string(),
            prefix_mode: self.prefix_mode,
            custom_prefix: self.custom_prefix.value().to_string(),
            repository_path: self.repo.value().trim().to_string(),
        }
    }

    /// Mirrors the desktop dialog's live prefix preview.
    pub fn preview(&self) -> String {
        const TAG: &str = "[Services\\GitService]";
        const MESSAGE: &str = "Fix issue in login flow";
        let custom = self.custom_prefix.value().trim();
        match self.prefix_mode {
            GitPrefixMode::AutoFolder => format!("{TAG} {MESSAGE}"),
            GitPrefixMode::Custom => {
                if custom.is_empty() {
                    MESSAGE.to_string()
                } else {
                    format!("{custom} {MESSAGE}")
                }
            }
            GitPrefixMode::Both => {
                if custom.is_empty() {
                    format!("{TAG} {MESSAGE}")
                } else {
                    format!("{TAG} {custom} {MESSAGE}")
                }
            }
            GitPrefixMode::None => MESSAGE.to_string(),
        }
    }
}

pub struct App {
    pub repo_path: String,
    pub recent_repos: Vec<String>,
    pub unstaged: Vec<GitFileStatus>,
    pub staged: Vec<GitFileStatus>,
    pub commits: Vec<GitCommitInfo>,
    pub commit_files: Vec<GitFileStatus>,
    pub inspecting: Option<GitCommitInfo>,

    pub rows: Vec<Row>,
    pub row_sel: usize,
    pub selection: Option<Selection>,
    pub expanded: HashSet<String>,
    pub show_all: bool,

    pub diff: String,
    pub diff_lines: Vec<DiffLine>,
    pub image_diff: Option<ImageDiffData>,
    pub diff_scroll: usize,
    pub diff_hscroll: usize,
    pub diff_viewport: usize,
    pub diff_target: Option<DiffTarget>,
    pub loading_diff: bool,

    pub ai_settings: AiSettings,
    pub diff_settings: DiffSettings,
    pub commit_message: TextInput,

    pub status: String,
    pub status_error: bool,
    pub is_operating: bool,
    pub spinner: u8,

    pub focus: Focus,
    pub sidebar_width: u16,
    pub overlay: Overlay,
    pub should_quit: bool,

    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let mut app = Self {
            repo_path: String::new(),
            recent_repos: Vec::new(),
            unstaged: Vec::new(),
            staged: Vec::new(),
            commits: Vec::new(),
            commit_files: Vec::new(),
            inspecting: None,
            rows: Vec::new(),
            row_sel: 0,
            selection: None,
            expanded: HashSet::new(),
            show_all: false,
            diff: String::new(),
            diff_lines: Vec::new(),
            image_diff: None,
            diff_scroll: 0,
            diff_hscroll: 0,
            diff_viewport: 10,
            diff_target: None,
            loading_diff: false,
            ai_settings: AiSettings::default(),
            diff_settings: DiffSettings::default(),
            commit_message: TextInput::default(),
            status: "Ready".to_string(),
            status_error: false,
            is_operating: false,
            spinner: 0,
            focus: Focus::Changes,
            sidebar_width: SIDEBAR_DEFAULT,
            overlay: Overlay::None,
            should_quit: false,
            tx,
            rx,
        };

        app.ai_settings = settings_store::load_ai_settings();
        app.diff_settings = settings_store::load_diff_settings();
        app.recent_repos = settings_store::load_recent_repos();
        app.sidebar_width = app
            .diff_settings
            .changes_files_width
            .clamp(SIDEBAR_MIN as f64, SIDEBAR_MAX as f64) as u16;

        let mut detected = app.ai_settings.repository_path.clone();
        if !git_service::is_git_repository(&detected) {
            detected =
                git_service::detect_default_repository_path(Some(&app.ai_settings.repository_path));
        }
        if !git_service::is_git_repository(&detected) {
            detected = String::new();
        }
        app.repo_path = detected;

        if app.repo_path.is_empty() {
            app.status = "No repository selected — press 'o' to pick one.".to_string();
        } else {
            let repo = app.repo_path.clone();
            app.add_recent_repo(&repo);
            app.refresh();
        }

        app
    }

    // ---------------------------------------------------------------- plumbing

    fn ctx(&self) -> Ctx {
        Ctx {
            repo: self.repo_path.clone(),
            ai: self.ai_settings.clone(),
            tree_depth: self.diff_settings.tree_depth,
        }
    }

    fn dispatch(&mut self, job: Job) {
        jobs::spawn(job, self.ctx(), self.tx.clone());
    }

    pub fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.on_msg(msg);
        }
    }

    pub fn tick(&mut self) {
        if self.is_operating {
            self.spinner = self.spinner.wrapping_add(1);
        }
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Status { text, is_error } => {
                self.status = text;
                self.status_error = is_error;
            }
            Msg::Snapshot(snapshot) => {
                self.unstaged = snapshot.unstaged;
                self.staged = snapshot.staged;
                self.commits = snapshot.commits;
                self.status = snapshot.status;
                self.status_error = snapshot.is_error;
                self.is_operating = false;
                if snapshot.clear_message {
                    self.commit_message.clear();
                }
                self.rebuild_rows();
            }
            Msg::Diff(payload) => {
                let target = DiffTarget {
                    path: payload.path.clone(),
                    staged: payload.staged,
                    commit: payload.commit.clone(),
                };
                if self.diff_target.as_ref() != Some(&target) {
                    return; // superseded by a newer request
                }
                self.diff = payload.diff;
                self.diff_lines = diff_lines::parse(&self.diff);
                self.image_diff = payload.image;
                self.loading_diff = false;
                self.diff_scroll = 0;
                self.diff_hscroll = 0;
            }
            Msg::AiMessage(message) => {
                self.commit_message.set(message);
            }
            Msg::TestResult(result) => {
                if let Overlay::Settings(form) = &mut self.overlay {
                    form.testing = false;
                    form.test = Some(match result {
                        Ok(text) => (false, text),
                        Err(err) => (true, err),
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------ actions

    pub fn refresh(&mut self) {
        if self.repo_path.is_empty() {
            self.status = "No repository selected.".to_string();
            self.status_error = true;
            return;
        }
        // Mirrors the desktop apps, which disable refresh while an operation runs
        // (a refresh snapshot would otherwise clear the busy flag mid-operation).
        if self.is_operating {
            return;
        }
        self.status = "Refreshing repository status…".to_string();
        self.status_error = false;
        self.dispatch(Job::Refresh);
    }

    fn add_recent_repo(&mut self, path: &str) {
        let normalized = normalize_path(path);
        if normalized.is_empty() || !git_service::is_git_repository(&normalized) {
            return;
        }
        self.recent_repos.retain(|r| !same_path(r, &normalized));
        self.recent_repos.insert(0, normalized);
        self.recent_repos.truncate(15);
        let _ = settings_store::save_recent_repos(&self.recent_repos);
    }

    fn remove_recent_repo(&mut self, path: &str) {
        self.recent_repos.retain(|r| !same_path(r, path));
        let _ = settings_store::save_recent_repos(&self.recent_repos);
    }

    pub fn switch_repo(&mut self, path: &str) {
        let normalized = normalize_path(path);
        if normalized.is_empty() {
            return;
        }
        if same_path(&normalized, &self.repo_path) {
            self.refresh();
            return;
        }
        if !git_service::is_git_repository(&normalized) {
            let name = Path::new(&normalized)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| normalized.clone());
            self.status = format!(
                "\"{name}\" is not a Git repository (no .git found). Pick a repository folder."
            );
            self.status_error = true;
            return;
        }

        self.repo_path = normalized;
        self.selection = None;
        self.inspecting = None;
        self.commit_files.clear();
        self.commit_message.clear();
        self.diff.clear();
        self.diff_lines.clear();
        self.image_diff = None;
        self.diff_target = None;
        let repo = self.repo_path.clone();
        self.add_recent_repo(&repo);
        self.ai_settings.repository_path = repo;
        let _ = settings_store::save_ai_settings(&self.ai_settings);
        self.refresh();
    }

    fn save_diff_settings(&self) {
        let _ = settings_store::save_diff_settings(&self.diff_settings);
    }

    fn set_single_diff_view(&mut self, value: bool) {
        self.diff_settings.is_single_diff_view = value;
        self.save_diff_settings();
        self.load_diff();
    }

    fn change_tree_depth(&mut self, delta: i32) {
        let current = self.diff_settings.tree_depth as i32 + delta;
        self.diff_settings.tree_depth = current.clamp(1, 6) as u32;
        self.save_diff_settings();
        self.rebuild_rows();
    }

    fn cycle_theme(&mut self) {
        self.diff_settings.theme = match self.diff_settings.theme {
            AppThemeMode::System => AppThemeMode::Dark,
            AppThemeMode::Dark => AppThemeMode::Light,
            AppThemeMode::Light => AppThemeMode::System,
        };
        self.save_diff_settings();
    }

    fn resize_sidebar(&mut self, delta: i32) {
        let next =
            (self.sidebar_width as i32 + delta).clamp(SIDEBAR_MIN as i32, SIDEBAR_MAX as i32);
        self.sidebar_width = next as u16;
        self.diff_settings.changes_files_width = self.sidebar_width as f64;
        self.save_diff_settings();
    }

    // ------------------------------------------------------------- row building

    pub fn rebuild_rows(&mut self) {
        let depth = self.diff_settings.tree_depth;
        let mut rows = Vec::new();

        if self.inspecting.is_some() {
            let groups = grouping::group_files(&self.commit_files, depth);
            if groups.is_empty() {
                rows.push(Row::Empty("No files modified in this commit".to_string()));
            } else {
                for group in groups {
                    push_group(&mut rows, group, false, &self.expanded, self.show_all);
                }
            }
        } else if self.unstaged.is_empty() && self.staged.is_empty() {
            rows.push(Row::Empty("Working tree is clean.".to_string()));
        } else {
            if !self.unstaged.is_empty() {
                rows.push(Row::Section {
                    label: format!("UNSTAGED CHANGES ({})", self.unstaged.len()),
                    staged: false,
                });
                for group in grouping::group_files(&self.unstaged, depth) {
                    push_group(&mut rows, group, false, &self.expanded, self.show_all);
                }
            }
            if !self.staged.is_empty() {
                rows.push(Row::Section {
                    label: format!("STAGED CHANGES ({})", self.staged.len()),
                    staged: true,
                });
                for group in grouping::group_files(&self.staged, depth) {
                    push_group(&mut rows, group, true, &self.expanded, self.show_all);
                }
            }
        }

        self.rows = rows;
        self.restore_selection();
    }

    fn find_row(&self, selection: &Selection) -> Option<usize> {
        self.rows.iter().position(|row| match row {
            Row::File { file, staged } => {
                !selection.is_group
                    && *staged == selection.staged
                    && file.file_path == selection.key
            }
            Row::Group { group, staged, .. } => {
                selection.is_group && *staged == selection.staged && group.key == selection.key
            }
            _ => false,
        })
    }

    /// A file is preferred so the diff pane has something to show.
    fn first_selectable(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::File { .. }))
            .or_else(|| self.rows.iter().position(|row| row.is_selectable()))
    }

    fn set_selection_from_row(&mut self, index: usize) {
        self.row_sel = index;
        self.selection = self.rows.get(index).map(|row| match row {
            Row::File { file, staged } => Selection {
                staged: *staged,
                is_group: false,
                key: file.file_path.clone(),
            },
            Row::Group { group, staged, .. } => Selection {
                staged: *staged,
                is_group: true,
                key: group.key.clone(),
            },
            _ => Selection {
                staged: row.staged(),
                is_group: false,
                key: String::new(),
            },
        });
    }

    fn restore_selection(&mut self) {
        let previous = self.selection.clone();
        let index = previous
            .as_ref()
            .and_then(|selection| self.find_row(selection))
            .or_else(|| self.first_selectable());

        match index {
            Some(index) => {
                self.set_selection_from_row(index);
                let selected_is_file = self
                    .selection
                    .as_ref()
                    .is_some_and(|selection| !selection.is_group && !selection.key.is_empty());
                if selected_is_file {
                    self.load_diff();
                }
            }
            None => {
                self.selection = None;
                self.diff.clear();
                self.diff_lines.clear();
                self.image_diff = None;
                self.diff_target = None;
                self.loading_diff = false;
            }
        }
    }

    // ----------------------------------------------------------------- diff load

    pub fn load_diff(&mut self) {
        if self.repo_path.is_empty() {
            return;
        }
        let commit = self.inspecting.as_ref().map(|c| c.hash.clone());

        let file_target = self
            .selection
            .as_ref()
            .filter(|selection| !selection.is_group && !selection.key.is_empty())
            .map(|selection| (selection.key.clone(), selection.staged));

        let (path, staged) = match file_target {
            Some((path, staged)) => (Some(path), staged),
            None => {
                if commit.is_some() && !self.commit_files.is_empty() {
                    return; // a file row is always selected in commit view
                }
                if self.diff_settings.is_single_diff_view || commit.is_some() {
                    (None, false)
                } else {
                    self.clear_diff();
                    return;
                }
            }
        };

        self.diff_target = Some(DiffTarget {
            path: path.clone(),
            staged,
            commit: commit.clone(),
        });
        self.loading_diff = true;
        self.dispatch(Job::LoadDiff {
            path,
            staged,
            commit,
        });
    }

    fn clear_diff(&mut self) {
        self.diff.clear();
        self.diff_lines.clear();
        self.image_diff = None;
        self.diff_target = None;
        self.loading_diff = false;
        self.diff_scroll = 0;
        self.diff_hscroll = 0;
    }

    // --------------------------------------------------------------- selection

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let mut index = self.row_sel as isize + delta;
        while index >= 0 && (index as usize) < self.rows.len() {
            if self.rows[index as usize].is_selectable() {
                self.select_index(index as usize);
                return;
            }
            index += delta;
        }
    }

    fn select_index(&mut self, index: usize) {
        self.set_selection_from_row(index);
        let selected_is_file = self
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.is_group && !selection.key.is_empty());
        if selected_is_file {
            self.load_diff();
        }
    }

    fn jump_selection(&mut self, from_end: bool) {
        let index = if from_end {
            self.rows.iter().rposition(|row| row.is_selectable())
        } else {
            self.rows.iter().position(|row| row.is_selectable())
        };
        if let Some(index) = index {
            self.select_index(index);
        }
    }

    fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.row_sel)
    }

    fn toggle_group(&mut self) {
        if let Some(Row::Group { group, .. }) = self.selected_row() {
            let key = group.key.clone();
            if !self.expanded.remove(&key) {
                self.expanded.insert(key);
            }
            self.rebuild_rows();
        }
    }

    fn collapse_group(&mut self) {
        if let Some(Row::Group { group, .. }) = self.selected_row() {
            let key = group.key.clone();
            if self.expanded.remove(&key) {
                self.rebuild_rows();
            }
        }
    }

    fn selected_paths(&self) -> Vec<String> {
        match self.selected_row() {
            Some(Row::File { file, .. }) => vec![file.file_path.clone()],
            Some(Row::Group { group, .. }) => {
                group.items.iter().map(|f| f.file_path.clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    fn selected_reset_entries(&self) -> Vec<ResetEntry> {
        match self.selected_row() {
            Some(Row::File { file, .. }) => vec![ResetEntry {
                path: file.file_path.clone(),
                untracked: file.is_untracked,
            }],
            Some(Row::Group { group, .. }) => group
                .items
                .iter()
                .map(|f| ResetEntry {
                    path: f.file_path.clone(),
                    untracked: f.is_untracked,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    // ------------------------------------------------------------ main actions

    fn start(&mut self, status: &str) {
        self.is_operating = true;
        self.status = status.to_string();
        self.status_error = false;
    }

    fn toggle_stage(&mut self) {
        if self.inspecting.is_some() || self.is_operating {
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        if self.selected_row().is_some_and(|row| row.staged()) {
            self.start(&format!("Unstaging {} file(s)…", paths.len()));
            self.dispatch(Job::Unstage(paths));
        } else {
            self.start(&format!("Staging {} file(s)…", paths.len()));
            self.dispatch(Job::Stage(paths));
        }
    }

    fn unstage_selected(&mut self) {
        if self.inspecting.is_some() || self.is_operating {
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() || !self.selected_row().is_some_and(|row| row.staged()) {
            return;
        }
        self.start(&format!("Unstaging {} file(s)…", paths.len()));
        self.dispatch(Job::Unstage(paths));
    }

    fn discard_selected(&mut self) {
        if self.inspecting.is_some() || self.is_operating {
            return;
        }
        let entries = self.selected_reset_entries();
        if entries.is_empty() {
            return;
        }
        let (title, message, status) = match self.selected_row() {
            Some(Row::File { file, .. }) => {
                let verb = if file.is_untracked {
                    "permanently delete"
                } else {
                    "reset"
                };
                (
                    if file.is_untracked {
                        "Confirm Delete Untracked File".to_string()
                    } else {
                        "Confirm Reset File".to_string()
                    },
                    format!(
                        "Are you sure you want to {verb} \"{}\"? This cannot be undone.",
                        file.clean_path
                    ),
                    format!("Resetting {}…", file.file_name),
                )
            }
            Some(Row::Group { group, .. }) => (
                format!("Reset All in [{}]", group.key),
                format!(
                    "Are you sure you want to reset and discard all {} file(s) in \"{}\"? \
                     This cannot be undone.",
                    group.items.len(),
                    group.key
                ),
                format!("Resetting files in [{}]…", group.key),
            ),
            _ => return,
        };

        self.overlay = Overlay::Confirm(ConfirmAction {
            title,
            message,
            status,
            job: Job::Reset(entries),
        });
    }

    fn reset_all(&mut self) {
        if self.is_operating {
            return;
        }
        self.overlay = Overlay::Confirm(ConfirmAction {
            title: "Reset All Changes".to_string(),
            message: "Are you sure you want to discard ALL working tree changes and untracked \
                      files? This operation is permanent."
                .to_string(),
            status: "Resetting all changes…".to_string(),
            job: Job::ResetAll,
        });
    }

    fn ignore_selected(&mut self) {
        if self.inspecting.is_some() || self.is_operating {
            return;
        }
        let pattern = match self.selected_row() {
            Some(Row::File { file, .. }) if file.is_untracked => file.clean_path.clone(),
            Some(Row::Group { group, .. }) if group.is_untracked_all => format!("{}/", group.key),
            _ => return,
        };
        self.start(&format!("Adding \"{pattern}\" to .gitignore…"));
        self.dispatch(Job::Ignore(pattern));
    }

    fn instant_ai_selected(&mut self) {
        if self.inspecting.is_some() || self.is_operating {
            return;
        }
        let Some(Row::File {
            file,
            staged: false,
        }) = self.selected_row()
        else {
            return;
        };
        let file = file.clone();
        self.overlay = Overlay::Confirm(ConfirmAction {
            title: format!("Instant AI Commit: {}", file.file_name),
            message: format!(
                "Generate an AI commit message for \"{}\", then commit and push it now?",
                file.file_path
            ),
            status: format!("Instant AI committing {}…", file.file_name),
            job: Job::AiCommit {
                paths: Some(vec![file.file_path.clone()]),
                use_staged: false,
            },
        });
    }

    fn ai_commit_selected_group(&mut self) {
        if self.inspecting.is_some() || self.is_operating {
            return;
        }
        let Some(Row::Group {
            group,
            staged: false,
            ..
        }) = self.selected_row()
        else {
            return;
        };
        let key = group.key.clone();
        let paths: Vec<String> = group.items.iter().map(|f| f.file_path.clone()).collect();
        self.start(&format!("AI committing [{key}]…"));
        self.dispatch(Job::AiCommit {
            paths: Some(paths),
            use_staged: false,
        });
    }

    fn prompt_commit_selected(&mut self) {
        if self.inspecting.is_some() || self.is_operating {
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let title = match self.selected_row() {
            Some(Row::File { file, .. }) => format!("Commit & Push: {}", file.file_name),
            Some(Row::Group { group, .. }) => format!("Commit & Push: [{}]", group.key),
            _ => return,
        };
        self.overlay = Overlay::Prompt {
            title,
            hint: "Enter commit message — Enter to stage, commit & push".to_string(),
            input: TextInput::default(),
            action: PromptAction::CommitFiles { paths },
        };
    }

    fn commit_and_push_from_box(&mut self) {
        if self.is_operating || self.repo_path.is_empty() {
            return;
        }
        let message = self.commit_message.value().trim().to_string();
        if message.is_empty() {
            self.status = "Commit message cannot be empty.".to_string();
            self.status_error = true;
            return;
        }
        self.start("Preparing commit…");
        self.dispatch(Job::CommitPush {
            paths: None,
            message,
        });
    }

    fn ai_commit_and_push(&mut self) {
        if self.is_operating || self.repo_path.is_empty() {
            return;
        }
        self.start("Analyzing changes for AI commit…");
        self.dispatch(Job::AiCommit {
            paths: None,
            use_staged: !self.staged.is_empty(),
        });
    }

    fn ai_commit_all_trees(&mut self) {
        if self.is_operating {
            return;
        }
        if self.unstaged.is_empty() && self.staged.is_empty() {
            self.status = "No changes to commit.".to_string();
            self.status_error = false;
            return;
        }

        let all: Vec<GitFileStatus> = self
            .unstaged
            .iter()
            .chain(self.staged.iter())
            .cloned()
            .collect();
        let count = grouping::group_files(&all, self.diff_settings.tree_depth).len();

        self.overlay = Overlay::Confirm(ConfirmAction {
            title: "Commit All Trees with AI".to_string(),
            message: format!(
                "This will sequentially generate AI commit messages, commit and push each of the \
                 {count} folder tree group(s) one by one. Continue?"
            ),
            status: "Preparing to AI commit all folder trees…".to_string(),
            job: Job::AiCommitAllTrees,
        });
    }

    fn git_pull(&mut self) {
        if self.is_operating {
            return;
        }
        self.start("Running Git Pull…");
        self.dispatch(Job::Pull);
    }

    fn git_push(&mut self) {
        if self.is_operating {
            return;
        }
        self.start("Running Git Push…");
        self.dispatch(Job::Push);
    }

    fn git_sync(&mut self) {
        if self.is_operating {
            return;
        }
        self.start("Running Git Sync (Pull & Push)…");
        self.dispatch(Job::Sync);
    }

    fn open_selected(&mut self, reveal: bool) {
        let paths = self.selected_paths();
        let Some(path) = paths.first() else {
            return;
        };
        let full = Path::new(&self.repo_path)
            .join(path)
            .to_string_lossy()
            .to_string();
        let ok = if reveal {
            PlatformService::reveal_in_folder(&full, &self.repo_path)
        } else {
            PlatformService::open_path(&full)
        };
        if !ok {
            self.status = format!("Could not open \"{path}\".");
            self.status_error = true;
        }
    }

    fn copy_selected_path(&mut self) {
        let text = match self.selected_row() {
            Some(Row::File { file, .. }) => file.clean_path.clone(),
            Some(Row::Group { group, .. }) => group.key.clone(),
            _ => return,
        };
        match PlatformService::copy_to_clipboard(&text) {
            Ok(()) => {
                self.status = format!("Copied path: {text}");
                self.status_error = false;
            }
            Err(err) => {
                self.status = format!("Clipboard error: {err}");
                self.status_error = true;
            }
        }
    }

    fn enter_commit_inspect(&mut self, commit: GitCommitInfo) {
        let hash = commit.hash.clone();
        let short = commit.short_hash.clone();
        self.inspecting = Some(commit);
        self.status = format!("Inspecting commit {short}…");
        self.status_error = false;
        self.focus = Focus::Changes;
        self.expanded.clear();
        self.show_all = false;

        // Commit files are read synchronously: `git diff-tree` on a single
        // commit is fast and keeps the worker API small.
        self.commit_files = git_service::get_commit_files(&self.repo_path, &hash);
        self.selection = None;
        self.rebuild_rows();
        if self.commit_files.is_empty() {
            self.load_diff();
        }
    }

    fn exit_commit_inspect(&mut self) {
        if self.inspecting.take().is_some() {
            self.commit_files.clear();
            self.selection = None;
            self.clear_diff();
            self.rebuild_rows();
            self.refresh();
        }
    }

    fn save_settings(&mut self) {
        let (updated, repo_changed) = {
            let Overlay::Settings(form) = &self.overlay else {
                return;
            };
            let updated = form.to_settings(&self.ai_settings);
            let repo_changed = !updated.repository_path.is_empty()
                && !same_path(&updated.repository_path, &self.repo_path);
            (updated, repo_changed)
        };

        self.ai_settings = updated;
        let _ = settings_store::save_ai_settings(&self.ai_settings);
        self.overlay = Overlay::None;

        if repo_changed {
            let repo = self.ai_settings.repository_path.clone();
            self.switch_repo(&repo);
        }
    }

    // ------------------------------------------------------------------- input

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if is_ctrl(&key, 'c') {
            self.should_quit = true;
            return;
        }

        let overlay = std::mem::replace(&mut self.overlay, Overlay::None);
        if matches!(overlay, Overlay::None) {
            self.handle_main_key(key);
            return;
        }
        if let Some(overlay) = self.handle_overlay_key(overlay, key) {
            self.overlay = overlay;
        }
    }

    fn handle_overlay_key(&mut self, overlay: Overlay, key: KeyEvent) -> Option<Overlay> {
        match overlay {
            Overlay::None => None,
            Overlay::Help => None, // any key dismisses
            Overlay::Confirm(action) => self.handle_confirm_key(action, key),
            Overlay::Prompt {
                title,
                hint,
                mut input,
                action,
            } => {
                match key.code {
                    KeyCode::Esc => return None,
                    KeyCode::Enter => {
                        let PromptAction::CommitFiles { paths } = &action;
                        let message = input.value().trim().to_string();
                        if message.is_empty() {
                            self.status = "Commit message cannot be empty.".to_string();
                            self.status_error = true;
                            return Some(Overlay::Prompt {
                                title,
                                hint,
                                input,
                                action,
                            });
                        }
                        self.start("Staging & committing…");
                        self.dispatch(Job::CommitPush {
                            paths: Some(paths.clone()),
                            message,
                        });
                        return None;
                    }
                    _ => edit_input(&mut input, key),
                }
                Some(Overlay::Prompt {
                    title,
                    hint,
                    input,
                    action,
                })
            }
            Overlay::Repos(mut state) => {
                if self.handle_repos_key(&mut state, key) {
                    Some(Overlay::Repos(state))
                } else {
                    None
                }
            }
            Overlay::Commits(mut state) => {
                match key.code {
                    KeyCode::Esc => return None,
                    KeyCode::Down | KeyCode::Char('j') => {
                        if state.sel + 1 < COMMITS_PER_PAGE {
                            state.sel += 1;
                        } else if (state.page + 1) * COMMITS_PER_PAGE < self.commits.len() {
                            state.page += 1;
                            state.sel = 0;
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if state.sel > 0 {
                            state.sel -= 1;
                        } else if state.page > 0 {
                            state.page -= 1;
                            state.sel = COMMITS_PER_PAGE - 1;
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') | KeyCode::PageDown => {
                        if state.page + 1 < self.commit_page_count() {
                            state.page += 1;
                            state.sel = 0;
                        }
                    }
                    KeyCode::Left | KeyCode::Char('h') | KeyCode::PageUp => {
                        if state.page > 0 {
                            state.page -= 1;
                            state.sel = 0;
                        }
                    }
                    KeyCode::Enter => {
                        let index = state.page * COMMITS_PER_PAGE + state.sel;
                        if let Some(commit) = self.commits.get(index).cloned() {
                            self.enter_commit_inspect(commit);
                            return None;
                        }
                    }
                    _ => {}
                }
                Some(Overlay::Commits(state))
            }
            Overlay::Settings(mut form) => {
                self.handle_settings_key(&mut form, key)?;
                Some(Overlay::Settings(form))
            }
        }
    }

    fn handle_confirm_key(&mut self, action: ConfirmAction, key: KeyEvent) -> Option<Overlay> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => None,
            KeyCode::Enter | KeyCode::Char('y') => {
                self.start(&action.status);
                self.dispatch(action.job);
                None
            }
            _ => Some(Overlay::Confirm(action)),
        }
    }

    /// Returns false when the dialog should close.
    fn handle_repos_key(&mut self, state: &mut ReposState, key: KeyEvent) -> bool {
        if state.typing {
            match key.code {
                KeyCode::Esc => state.typing = false,
                KeyCode::Enter => {
                    let path = state.input.value().trim().to_string();
                    if !path.is_empty() {
                        self.switch_repo(&path);
                        if same_path(&path, &self.repo_path) {
                            return false;
                        }
                        state.typing = false;
                    }
                }
                _ => edit_input(&mut state.input, key),
            }
            return true;
        }

        match key.code {
            KeyCode::Esc => return false,
            KeyCode::Up | KeyCode::Char('k') => {
                state.sel = state.sel.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if state.sel + 1 < self.recent_repos.len() {
                    state.sel += 1;
                }
            }
            KeyCode::Char('d') => {
                if let Some(repo) = self.recent_repos.get(state.sel).cloned() {
                    self.remove_recent_repo(&repo);
                    state.sel = state.sel.min(self.recent_repos.len().saturating_sub(1));
                }
            }
            KeyCode::Char('i') => {
                state.typing = true;
                state.input.clear();
            }
            KeyCode::Enter => {
                if let Some(repo) = self.recent_repos.get(state.sel).cloned() {
                    self.switch_repo(&repo);
                    return false;
                }
            }
            _ => {}
        }
        true
    }

    /// Returns `None` to close the settings dialog.
    fn handle_settings_key(&mut self, form: &mut SettingsForm, key: KeyEvent) -> Option<()> {
        match key.code {
            KeyCode::Esc => return None,
            KeyCode::Tab | KeyCode::Down => {
                form.focus = (form.focus + 1) % SettingsForm::FIELD_COUNT;
            }
            KeyCode::BackTab | KeyCode::Up => {
                form.focus =
                    (form.focus + SettingsForm::FIELD_COUNT - 1) % SettingsForm::FIELD_COUNT;
            }
            KeyCode::Char('s') if is_ctrl(&key, 's') => {
                self.save_settings();
                return None;
            }
            KeyCode::Left | KeyCode::Right if form.focus == 3 => {
                form.prefix_mode = cycle_prefix(form.prefix_mode, key.code == KeyCode::Right);
            }
            KeyCode::Enter => match form.focus {
                SettingsForm::TEST => {
                    if !form.testing {
                        form.testing = true;
                        form.test = None;
                        let settings = form.to_settings(&self.ai_settings);
                        self.dispatch(Job::TestConnection(settings));
                    }
                }
                SettingsForm::SAVE => {
                    self.save_settings();
                    return None;
                }
                SettingsForm::CANCEL => return None,
                1 => form.show_key = !form.show_key,
                _ => {}
            },
            KeyCode::F(2) => form.show_key = !form.show_key,
            _ => {
                if form.is_text_field() {
                    if let Some(input) = form.input_mut() {
                        edit_input(input, key);
                    }
                }
            }
        }
        Some(())
    }

    pub fn commit_page_count(&self) -> usize {
        self.commits.len().div_ceil(COMMITS_PER_PAGE).max(1)
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        if is_ctrl(&key, 'i') {
            self.ai_commit_and_push();
            return;
        }
        if is_ctrl(&key, 'r') {
            self.refresh();
            return;
        }
        if is_ctrl(&key, 'g') {
            self.focus = Focus::Commit;
            self.commit_message.move_end();
            return;
        }
        if is_ctrl(&key, 'p') {
            self.git_push();
            return;
        }
        if is_ctrl(&key, 'u') {
            self.git_pull();
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Enter) {
            self.commit_and_push_from_box();
            return;
        }

        let handled = match self.focus {
            Focus::Commit => {
                self.handle_commit_key(key);
                true
            }
            Focus::Changes => self.handle_changes_key(key),
            Focus::Diff => self.handle_diff_key(key),
        };

        if !handled {
            self.handle_global_key(key);
        }
    }

    fn handle_commit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.focus = Focus::Changes,
            KeyCode::Enter => self.commit_and_push_from_box(),
            KeyCode::Tab => self.focus = Focus::Changes,
            KeyCode::BackTab => self.focus = Focus::Diff,
            _ => edit_input(&mut self.commit_message, key),
        }
    }

    fn handle_changes_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Char('g') => self.jump_selection(false),
            KeyCode::Char('G') => self.jump_selection(true),
            KeyCode::Enter => {
                if matches!(self.selected_row(), Some(Row::Group { .. })) {
                    self.toggle_group();
                } else {
                    self.focus = Focus::Diff;
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.toggle_group(),
            KeyCode::Left | KeyCode::Char('h') => self.collapse_group(),
            KeyCode::Char(' ') | KeyCode::Char('s') => self.toggle_stage(),
            KeyCode::Char('u') => self.unstage_selected(),
            KeyCode::Char('d') => self.discard_selected(),
            KeyCode::Char('D') => self.reset_all(),
            KeyCode::Char('i') => self.instant_ai_selected(),
            KeyCode::Char('a') => self.ai_commit_selected_group(),
            KeyCode::Char('c') => self.prompt_commit_selected(),
            KeyCode::Char('n') => self.ignore_selected(),
            KeyCode::Char('y') => self.copy_selected_path(),
            KeyCode::Char('e') => self.open_selected(false),
            KeyCode::Char('E') => self.open_selected(true),
            _ => return false,
        }
        true
    }

    fn handle_diff_key(&mut self, key: KeyEvent) -> bool {
        let page = self.diff_viewport.max(1) as isize;
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.scroll_diff(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_diff(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_diff(page),
            KeyCode::PageUp => self.scroll_diff(-page),
            KeyCode::Home | KeyCode::Char('g') => self.diff_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.diff_scroll = self.diff_lines.len().saturating_sub(1)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.diff_hscroll = self.diff_hscroll.saturating_add(4)
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.diff_hscroll = self.diff_hscroll.saturating_sub(4)
            }
            KeyCode::Char('0') => self.diff_hscroll = 0,
            _ => return false,
        }
        true
    }

    fn handle_global_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('r') | KeyCode::F(5) => self.refresh(),
            KeyCode::Char('o') => self.overlay = Overlay::Repos(ReposState::default()),
            KeyCode::Char('H') => self.overlay = Overlay::Commits(CommitsState::default()),
            KeyCode::Char('S') => {
                self.overlay = Overlay::Settings(Box::new(SettingsForm::new(&self.ai_settings)))
            }
            KeyCode::Char('P') => self.git_push(),
            KeyCode::Char('L') => self.git_pull(),
            KeyCode::Char('Y') => self.git_sync(),
            KeyCode::Char('A') => self.ai_commit_all_trees(),
            // `Ctrl+I` is what the desktop apps use, but a terminal delivers it as
            // a plain Tab (0x09), so `I` is the binding that actually works here.
            KeyCode::Char('I') => self.ai_commit_and_push(),
            KeyCode::Char('D') => self.reset_all(),
            KeyCode::Char('x') => {
                self.show_all = !self.show_all;
                self.rebuild_rows();
            }
            KeyCode::Char('v') => {
                let next = !self.diff_settings.is_single_diff_view;
                self.set_single_diff_view(next);
            }
            KeyCode::Char('t') => self.cycle_theme(),
            KeyCode::Char('[') => self.resize_sidebar(-4),
            KeyCode::Char(']') => self.resize_sidebar(4),
            KeyCode::Char('\\') => {
                self.sidebar_width = SIDEBAR_DEFAULT;
                self.diff_settings.changes_files_width = SIDEBAR_DEFAULT as f64;
                self.save_diff_settings();
            }
            KeyCode::Char('-') => self.change_tree_depth(-1),
            KeyCode::Char('+') | KeyCode::Char('=') => self.change_tree_depth(1),
            KeyCode::Tab => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.prev(),
            KeyCode::Char('1') => self.focus = Focus::Changes,
            KeyCode::Char('2') => self.focus = Focus::Diff,
            KeyCode::Char('3') => self.focus = Focus::Commit,
            KeyCode::Esc => {
                if self.inspecting.is_some() {
                    self.exit_commit_inspect();
                }
            }
            _ => return false,
        }
        true
    }

    pub fn handle_paste(&mut self, text: &str) {
        match &mut self.overlay {
            Overlay::None => {
                if self.focus == Focus::Commit {
                    self.commit_message.insert_str(text);
                }
            }
            Overlay::Settings(form) => {
                if form.is_text_field() {
                    if let Some(input) = form.input_mut() {
                        input.insert_str(text);
                    }
                }
            }
            Overlay::Prompt { input, .. } => input.insert_str(text),
            Overlay::Repos(state) if state.typing => state.input.insert_str(text),
            _ => {}
        }
    }

    fn scroll_diff(&mut self, delta: isize) {
        let len = self.diff_lines.len() as isize;
        if len == 0 {
            return;
        }
        self.diff_scroll = (self.diff_scroll as isize + delta).clamp(0, len - 1) as usize;
    }

    /// Whether a text cursor should be visible somewhere on screen.
    pub fn wants_cursor(&self) -> bool {
        match &self.overlay {
            Overlay::None => self.focus == Focus::Commit,
            Overlay::Prompt { .. } => true,
            Overlay::Repos(state) => state.typing,
            Overlay::Settings(form) => form.is_text_field(),
            _ => false,
        }
    }

    pub fn context_hint(&self) -> &'static str {
        if self.inspecting.is_some() {
            "Esc exit commit · r refresh · ? help"
        } else {
            match self.focus {
                Focus::Changes => "j/k move · space stage · d discard · c commit · ? help",
                Focus::Diff => "j/k scroll · h/l pan · v single diff · ? help",
                Focus::Commit => "type message · Enter commit & push · Ctrl+I AI · Esc done",
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn push_group(
    rows: &mut Vec<Row>,
    group: Group,
    staged: bool,
    expanded_set: &HashSet<String>,
    show_all: bool,
) {
    let expanded = show_all || expanded_set.contains(&group.key);
    let preview: Vec<GitFileStatus> = if expanded {
        group.items.clone()
    } else {
        group.items.iter().take(GROUP_PREVIEW).cloned().collect()
    };
    rows.push(Row::Group {
        group,
        staged,
        expanded,
    });
    for file in preview {
        rows.push(Row::File { file, staged });
    }
}

fn cycle_prefix(mode: GitPrefixMode, forward: bool) -> GitPrefixMode {
    let order = [
        GitPrefixMode::AutoFolder,
        GitPrefixMode::Custom,
        GitPrefixMode::Both,
        GitPrefixMode::None,
    ];
    let index = order.iter().position(|m| *m == mode).unwrap_or(0);
    let next = if forward {
        (index + 1) % order.len()
    } else {
        (index + order.len() - 1) % order.len()
    };
    order[next]
}

fn is_ctrl(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&c))
}

/// Shared single-line editing keys.
fn edit_input(input: &mut TextInput, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(c) if !ctrl => input.insert_char(c),
        KeyCode::Backspace if ctrl => input.delete_word_left(),
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete(),
        KeyCode::Left if ctrl => input.move_word_left(),
        KeyCode::Left => input.move_left(),
        KeyCode::Right if ctrl => input.move_word_right(),
        KeyCode::Right => input.move_right(),
        KeyCode::Home => input.move_home(),
        KeyCode::End => input.move_end(),
        _ => {}
    }
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut value = trimmed.replace('/', std::path::MAIN_SEPARATOR_STR);
    let root_suffix = format!(":{}", std::path::MAIN_SEPARATOR);
    while value.len() > 1
        && value.ends_with(std::path::MAIN_SEPARATOR)
        && !value.ends_with(&root_suffix)
    {
        value.pop();
    }
    value
}

/// Compares two repository paths, case-insensitively on Windows.
pub fn same_path(a: &str, b: &str) -> bool {
    let a = normalize_path(a);
    let b = normalize_path(b);
    if cfg!(windows) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

pub fn prefix_label(ai: &AiSettings) -> String {
    match ai.prefix_mode {
        GitPrefixMode::AutoFolder => "Auto Folder Tag".to_string(),
        GitPrefixMode::Custom => {
            if ai.custom_prefix.is_empty() {
                "Custom Prefix".to_string()
            } else {
                format!("Prefix: {}", ai.custom_prefix)
            }
        }
        GitPrefixMode::Both => "Both (Folder + Custom)".to_string(),
        GitPrefixMode::None => "No Prefix".to_string(),
    }
}
