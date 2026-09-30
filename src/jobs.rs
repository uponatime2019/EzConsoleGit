//! Background jobs. Every git invocation and AI request runs on its own thread
//! and reports back through an `mpsc` channel, so the event loop never blocks.

use std::fs;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use ezgit_core::ai_service;
use ezgit_core::git_service;
use ezgit_core::models::{
    AiSettings, GitCommandResult, GitCommitInfo, GitFileStatus, ImageDiffData,
};

use crate::grouping::group_files;

/// Everything a job needs that is not the repository itself.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub repo: String,
    pub ai: AiSettings,
    pub tree_depth: u32,
}

pub struct Snapshot {
    pub unstaged: Vec<GitFileStatus>,
    pub staged: Vec<GitFileStatus>,
    pub commits: Vec<GitCommitInfo>,
    pub status: String,
    pub is_error: bool,
    /// True when a commit succeeded and the commit box should be emptied.
    pub clear_message: bool,
}

pub struct DiffPayload {
    /// `None` means the unified working-tree diff.
    pub path: Option<String>,
    pub staged: bool,
    pub commit: Option<String>,
    pub diff: String,
    pub image: Option<ImageDiffData>,
}

pub enum Msg {
    Status { text: String, is_error: bool },
    Snapshot(Box<Snapshot>),
    Diff(Box<DiffPayload>),
    AiMessage(String),
    TestResult(Result<String, String>),
}

#[derive(Debug, Clone)]
pub struct ResetEntry {
    pub path: String,
    pub untracked: bool,
}

pub enum Job {
    Refresh,
    LoadDiff {
        path: Option<String>,
        staged: bool,
        commit: Option<String>,
    },
    Stage(Vec<String>),
    Unstage(Vec<String>),
    Reset(Vec<ResetEntry>),
    ResetAll,
    Ignore(String),
    CommitPush {
        paths: Option<Vec<String>>,
        message: String,
    },
    AiCommit {
        paths: Option<Vec<String>>,
        use_staged: bool,
    },
    AiCommitAllTrees,
    Pull,
    Push,
    Sync,
    TestConnection(AiSettings),
}

/// Spawns a job on a detached worker thread.
pub fn spawn(job: Job, ctx: Ctx, tx: Sender<Msg>) {
    std::thread::spawn(move || run(job, ctx, tx));
}

/// A shared single-threaded runtime for the async AI client.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build the tokio runtime")
    })
}

/// Collapses multi-line git/HTTP output into a single status-bar line.
fn one_line(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 400 {
        let head: String = collapsed.chars().take(400).collect();
        format!("{head}…")
    } else {
        collapsed
    }
}

fn error_text(res: &GitCommandResult) -> String {
    if !res.error.is_empty() {
        one_line(&res.error)
    } else if !res.output.is_empty() {
        one_line(&res.output)
    } else {
        "Unknown error".to_string()
    }
}

fn status(tx: &Sender<Msg>, text: impl Into<String>, is_error: bool) {
    let _ = tx.send(Msg::Status {
        text: text.into(),
        is_error,
    });
}

/// Re-reads status + history and reports a final status message.
fn finish(ctx: &Ctx, tx: &Sender<Msg>, message: impl Into<String>, is_error: bool) {
    finish_with(ctx, tx, message, is_error, false);
}

fn finish_with(
    ctx: &Ctx,
    tx: &Sender<Msg>,
    message: impl Into<String>,
    is_error: bool,
    clear_message: bool,
) {
    let status = git_service::get_git_status(&ctx.repo);
    let commits = git_service::get_recent_commits(&ctx.repo, 50);
    let _ = tx.send(Msg::Snapshot(Box::new(Snapshot {
        unstaged: status.unstaged,
        staged: status.staged,
        commits,
        status: message.into(),
        is_error,
        clear_message,
    })));
}

fn stage_paths(ctx: &Ctx, paths: &[String]) -> GitCommandResult {
    let mut args = vec!["add".to_string(), "--".to_string()];
    args.extend(paths.iter().cloned());
    git_service::run_git(&args, &ctx.repo, true)
}

fn unstage_paths(ctx: &Ctx, paths: &[String]) {
    let mut args = vec![
        "restore".to_string(),
        "--staged".to_string(),
        "--".to_string(),
    ];
    args.extend(paths.iter().cloned());
    let res = git_service::run_git(&args, &ctx.repo, true);
    if res.exit_code != 0 {
        let mut fallback = vec!["reset".to_string(), "HEAD".to_string(), "--".to_string()];
        fallback.extend(paths.iter().cloned());
        git_service::run_git(&fallback, &ctx.repo, true);
    }
}

fn reset_entry(ctx: &Ctx, entry: &ResetEntry) {
    if entry.untracked {
        let full = Path::new(&ctx.repo).join(&entry.path);
        let removed = if full.is_dir() {
            fs::remove_dir_all(&full).is_ok()
        } else {
            fs::remove_file(&full).is_ok()
        };
        if !removed {
            git_service::run_git(
                &[
                    "clean".to_string(),
                    "-fd".to_string(),
                    "--".to_string(),
                    entry.path.clone(),
                ],
                &ctx.repo,
                true,
            );
        }
        return;
    }

    let res = git_service::run_git(
        &["restore".to_string(), "--".to_string(), entry.path.clone()],
        &ctx.repo,
        true,
    );
    if res.exit_code != 0 {
        git_service::run_git(
            &[
                "checkout".to_string(),
                "HEAD".to_string(),
                "--".to_string(),
                entry.path.clone(),
            ],
            &ctx.repo,
            true,
        );
    }
}

fn commit(ctx: &Ctx, message: &str) -> GitCommandResult {
    git_service::run_git(
        &["commit".to_string(), "-m".to_string(), message.to_string()],
        &ctx.repo,
        true,
    )
}

fn ai_message(ctx: &Ctx, tx: &Sender<Msg>, diff: &str, files: &[String]) -> Result<String, String> {
    let result = runtime().block_on(ai_service::generate_commit_message(diff, files, &ctx.ai));
    if let Ok(message) = &result {
        let _ = tx.send(Msg::AiMessage(message.clone()));
    }
    result
}

fn run(job: Job, ctx: Ctx, tx: Sender<Msg>) {
    if ctx.repo.is_empty() && !matches!(job, Job::TestConnection(_)) {
        status(&tx, "No repository selected.", true);
        return;
    }

    match job {
        Job::Refresh => {
            let status_result = git_service::get_git_status(&ctx.repo);
            let commits = git_service::get_recent_commits(&ctx.repo, 50);
            let _ = tx.send(Msg::Snapshot(Box::new(Snapshot {
                unstaged: status_result.unstaged,
                staged: status_result.staged,
                commits,
                status: "Ready".to_string(),
                is_error: false,
                clear_message: false,
            })));
        }

        Job::LoadDiff {
            path,
            staged,
            commit,
        } => {
            let payload = match &path {
                Some(path) if git_service::is_image_file(path) => DiffPayload {
                    path: Some(path.clone()),
                    staged,
                    commit: commit.clone(),
                    diff: String::new(),
                    image: Some(git_service::get_image_diff(
                        &ctx.repo,
                        path,
                        staged,
                        commit.as_deref(),
                    )),
                },
                Some(path) => DiffPayload {
                    path: Some(path.clone()),
                    staged,
                    commit: commit.clone(),
                    diff: match &commit {
                        Some(hash) => git_service::get_commit_file_diff(&ctx.repo, hash, path),
                        None => git_service::get_file_diff(&ctx.repo, path, staged),
                    },
                    image: None,
                },
                None => DiffPayload {
                    path: None,
                    staged,
                    commit: commit.clone(),
                    diff: match &commit {
                        Some(hash) => git_service::get_commit_diff(&ctx.repo, hash),
                        None => git_service::get_all_diff(&ctx.repo, staged),
                    },
                    image: None,
                },
            };
            let _ = tx.send(Msg::Diff(Box::new(payload)));
        }

        Job::Stage(paths) => {
            let label = paths.len();
            let res = stage_paths(&ctx, &paths);
            let (message, is_error) = if res.exit_code == 0 {
                (format!("Staged {label} file(s)."), false)
            } else {
                (format!("Staging failed: {}", error_text(&res)), true)
            };
            finish(&ctx, &tx, message, is_error);
        }

        Job::Unstage(paths) => {
            let label = paths.len();
            unstage_paths(&ctx, &paths);
            finish(&ctx, &tx, format!("Unstaged {label} file(s)."), false);
        }

        Job::Reset(entries) => {
            let label = entries.len();
            for entry in &entries {
                reset_entry(&ctx, entry);
            }
            finish(&ctx, &tx, format!("Reset {label} file(s)."), false);
        }

        Job::ResetAll => {
            git_service::run_git(
                &[
                    "reset".to_string(),
                    "--hard".to_string(),
                    "HEAD".to_string(),
                ],
                &ctx.repo,
                true,
            );
            git_service::run_git(&["clean".to_string(), "-fd".to_string()], &ctx.repo, true);
            finish(&ctx, &tx, "All changes reset successfully.", false);
        }

        Job::Ignore(pattern) => {
            let ok = git_service::add_to_git_ignore(&ctx.repo, &pattern);
            let (message, is_error) = if ok {
                (format!("Added \"{pattern}\" to .gitignore."), false)
            } else {
                (format!("Failed to add \"{pattern}\" to .gitignore."), true)
            };
            finish(&ctx, &tx, message, is_error);
        }

        Job::CommitPush { paths, message } => {
            let message = message.trim().to_string();
            if message.is_empty() {
                status(&tx, "Commit message cannot be empty.", true);
                return;
            }

            match &paths {
                Some(paths) => {
                    status(&tx, "Staging & committing…", false);
                    stage_paths(&ctx, paths);
                }
                None => {
                    let current = git_service::get_git_status(&ctx.repo);
                    if current.staged.is_empty() && !current.unstaged.is_empty() {
                        status(&tx, "Staging all files…", false);
                        let all: Vec<String> = current
                            .unstaged
                            .iter()
                            .map(|f| f.file_path.clone())
                            .collect();
                        stage_paths(&ctx, &all);
                    }
                }
            }

            status(&tx, "Committing changes…", false);
            let res = commit(&ctx, &message);
            if res.exit_code != 0 {
                finish(
                    &ctx,
                    &tx,
                    format!("Commit failed: {}", error_text(&res)),
                    true,
                );
                return;
            }

            status(&tx, "Pushing to remote…", false);
            let push = git_service::execute_push_with_auto_pull(&ctx.repo);
            let (text, is_error) = if push.exit_code == 0 {
                ("Commit & Push succeeded!".to_string(), false)
            } else {
                (format!("Push failed: {}", error_text(&push)), true)
            };
            finish_with(&ctx, &tx, text, is_error, true);
        }

        Job::AiCommit { paths, use_staged } => {
            let (paths, files, diff) = match &paths {
                Some(paths) => {
                    status(&tx, "Staging files…", false);
                    stage_paths(&ctx, paths);
                    let mut buffer = String::new();
                    for path in paths {
                        buffer.push('\n');
                        buffer.push_str(&git_service::get_file_diff(&ctx.repo, path, true));
                    }
                    (paths.clone(), paths.clone(), buffer)
                }
                None => {
                    status(&tx, "Analyzing changes for AI commit…", false);
                    let diff = git_service::get_all_diff(&ctx.repo, use_staged);
                    let current = git_service::get_git_status(&ctx.repo);
                    let files: Vec<String> = current
                        .staged
                        .iter()
                        .chain(current.unstaged.iter())
                        .map(|f| f.file_path.clone())
                        .collect();
                    (Vec::new(), files, diff)
                }
            };

            status(&tx, "Generating AI commit message…", false);
            let message = match ai_message(&ctx, &tx, &diff, &files) {
                Ok(message) => message,
                Err(err) => {
                    status(&tx, format!("AI Commit failed: {err}"), true);
                    return;
                }
            };

            if paths.is_empty() {
                let current = git_service::get_git_status(&ctx.repo);
                if current.staged.is_empty() && !current.unstaged.is_empty() {
                    let all: Vec<String> = current
                        .unstaged
                        .iter()
                        .map(|f| f.file_path.clone())
                        .collect();
                    stage_paths(&ctx, &all);
                }
            }

            let res = commit(&ctx, &message);
            if res.exit_code != 0 {
                finish(
                    &ctx,
                    &tx,
                    format!("Commit failed: {}", error_text(&res)),
                    true,
                );
                return;
            }

            status(&tx, "Pushing to remote…", false);
            let push = git_service::execute_push_with_auto_pull(&ctx.repo);
            let (text, is_error) = if push.exit_code == 0 {
                ("AI Commit & Push succeeded!".to_string(), false)
            } else {
                (format!("Push failed: {}", error_text(&push)), true)
            };
            finish_with(&ctx, &tx, text, is_error, true);
        }

        Job::AiCommitAllTrees => {
            let current = git_service::get_git_status(&ctx.repo);
            let all: Vec<GitFileStatus> = current
                .unstaged
                .iter()
                .chain(current.staged.iter())
                .cloned()
                .collect();
            let groups = group_files(&all, ctx.tree_depth);
            if groups.is_empty() {
                status(&tx, "No changes to commit.", false);
                return;
            }

            let total = groups.len();
            for (index, group) in groups.iter().enumerate() {
                let paths: Vec<String> = group.items.iter().map(|f| f.file_path.clone()).collect();

                status(
                    &tx,
                    format!("Processing tree {}/{}: [{}]…", index + 1, total, group.key),
                    false,
                );
                stage_paths(&ctx, &paths);

                let mut buffer = String::new();
                for path in &paths {
                    buffer.push('\n');
                    buffer.push_str(&git_service::get_file_diff(&ctx.repo, path, true));
                }

                let message = match ai_message(&ctx, &tx, &buffer, &paths) {
                    Ok(message) => message,
                    Err(err) => {
                        finish(
                            &ctx,
                            &tx,
                            format!("AI commit failed on [{}]: {err}", group.key),
                            true,
                        );
                        return;
                    }
                };

                let res = commit(&ctx, &message);
                if res.exit_code != 0 {
                    finish(
                        &ctx,
                        &tx,
                        format!("Commit failed on [{}]: {}", group.key, error_text(&res)),
                        true,
                    );
                    return;
                }

                let push = git_service::execute_push_with_auto_pull(&ctx.repo);
                if push.exit_code != 0 {
                    finish(
                        &ctx,
                        &tx,
                        format!("Push failed on [{}]: {}", group.key, error_text(&push)),
                        true,
                    );
                    return;
                }
            }

            finish(
                &ctx,
                &tx,
                "All trees committed and pushed successfully!",
                false,
            );
        }

        Job::Pull => {
            let res = git_service::run_git(
                &["pull".to_string(), "--no-edit".to_string()],
                &ctx.repo,
                true,
            );
            let (text, is_error) = if res.exit_code == 0 {
                ("Git Pull completed successfully.".to_string(), false)
            } else {
                (format!("Git Pull failed: {}", error_text(&res)), true)
            };
            finish(&ctx, &tx, text, is_error);
        }

        Job::Push => {
            let res = git_service::execute_push_with_auto_pull(&ctx.repo);
            let (text, is_error) = if res.exit_code == 0 {
                ("Git Push completed successfully.".to_string(), false)
            } else {
                (format!("Git Push failed: {}", error_text(&res)), true)
            };
            finish(&ctx, &tx, text, is_error);
        }

        Job::Sync => {
            let pull = git_service::run_git(
                &["pull".to_string(), "--no-edit".to_string()],
                &ctx.repo,
                true,
            );
            if pull.exit_code != 0 {
                finish(
                    &ctx,
                    &tx,
                    format!("Sync failed on Pull: {}", error_text(&pull)),
                    true,
                );
                return;
            }
            let push = git_service::run_git(&["push".to_string()], &ctx.repo, true);
            let (text, is_error) = if push.exit_code == 0 {
                ("Git Sync completed successfully!".to_string(), false)
            } else {
                (format!("Sync failed on Push: {}", error_text(&push)), true)
            };
            finish(&ctx, &tx, text, is_error);
        }

        Job::TestConnection(settings) => {
            let result = runtime().block_on(ai_service::test_connection(&settings));
            let _ = tx.send(Msg::TestResult(result));
        }
    }
}
