use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;

use crate::models::{
    GitCommandResult, GitCommitInfo, GitFileStatus, GitStatusResult, ImageDiffData,
};

const SERIOUS_WARNING_THRESHOLD_BYTES: u64 = 800 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const BINARY_TIMEOUT: Duration = Duration::from_secs(15);

const IMAGE_EXTENSIONS: [&str; 8] = [
    ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".ico", ".webp", ".svg",
];

fn to_posix(value: &str) -> String {
    value.replace('\\', "/")
}

/// Splits on `\n`, stripping a trailing `\r` from each piece. A trailing
/// newline yields a trailing empty element, matching the Dart/Python split.
pub(crate) fn split_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect()
}

fn posix_basename(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[i + 1..].to_string(),
        None => path.to_string(),
    }
}

fn posix_dirname(path: &str) -> String {
    match path.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
    }
}

/// Appends `pattern` to the repository's `.gitignore`.
pub fn add_to_git_ignore(repo_path: &str, pattern: &str) -> bool {
    let gitignore = Path::new(repo_path).join(".gitignore");
    let normalized = pattern.replace('\\', "/");
    let mut content = fs::read_to_string(&gitignore).unwrap_or_default();
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&normalized);
    content.push('\n');
    fs::write(&gitignore, content).is_ok()
}

/// Human readable byte size, kept local so the service has no UI dependency.
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Runs `git` and captures (exit code, stdout, stderr, timed_out).
fn run_process(args: &[String], cwd: &str, timeout: Duration) -> (i32, String, String, bool) {
    let spawned = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => return (-1, String::new(), err.to_string(), false),
    };

    let out_handle = child.stdout.take().map(|mut out| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            buf
        })
    });
    let err_handle = child.stderr.take().map(|mut err| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });

    let start = Instant::now();
    let mut timed_out = false;
    let mut exit_code = -1i32;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code().unwrap_or(-1);
                break;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }

    let out_bytes = out_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let err_bytes = err_handle.and_then(|h| h.join().ok()).unwrap_or_default();

    (
        exit_code,
        String::from_utf8_lossy(&out_bytes).to_string(),
        String::from_utf8_lossy(&err_bytes).to_string(),
        timed_out,
    )
}

/// Runs `git` and returns raw stdout bytes (used for image history blobs).
fn run_process_bytes(args: &[String], cwd: &str, timeout: Duration) -> Option<Vec<u8>> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let out_handle = child.stdout.take().map(|mut out| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            buf
        })
    });
    let err_handle = child.stderr.take().map(|mut err| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });

    let start = Instant::now();
    let mut exit_code = -1i32;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code().unwrap_or(-1);
                break;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }

    let bytes = out_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let _ = err_handle.and_then(|h| h.join().ok());

    if exit_code == 0 && !bytes.is_empty() {
        Some(bytes)
    } else {
        None
    }
}

/// Runs `git` with `args` in `repo_path`, retrying once on a stale index lock.
pub fn run_git(args: &[String], repo_path: &str, retry_index_lock: bool) -> GitCommandResult {
    if repo_path.is_empty() {
        return GitCommandResult::failure("No repository path specified");
    }

    let (exit_code, output, raw_error, timed_out) = run_process(args, repo_path, COMMAND_TIMEOUT);
    let error = if timed_out {
        "ERROR: Git process timed out".to_string()
    } else {
        raw_error
    };
    let exit_code = if timed_out { -1 } else { exit_code };

    if exit_code != 0 && retry_index_lock && is_git_index_lock_error(&output, &error) {
        thread::sleep(Duration::from_secs(2));
        return run_git(args, repo_path, false);
    }

    GitCommandResult::new(exit_code, output, error)
}

fn run_git_binary(args: &[String], repo_path: &str) -> Option<Vec<u8>> {
    if repo_path.is_empty() {
        return None;
    }
    run_process_bytes(args, repo_path, BINARY_TIMEOUT)
}

pub fn is_git_index_lock_error(output: &str, error: &str) -> bool {
    format!("{output} {error}")
        .to_lowercase()
        .contains(".git/index.lock")
}

pub fn is_image_file(file_path: &str) -> bool {
    Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&format!(".{}", ext.to_lowercase()).as_str()))
        .unwrap_or(false)
}

/// Whether `path` is the root of a Git repository. A `.git` entry can be a
/// directory (normal clone) or a file (worktree / submodule).
pub fn is_git_repository(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    matches!(fs::metadata(Path::new(path).join(".git")), Ok(meta) if meta.is_dir() || meta.is_file())
}

pub fn unquote_git_path(git_path: &str) -> String {
    let value = git_path.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

/// Picks the most likely repository: explicit setting, then walking up from
/// the current directory, then common project locations.
pub fn detect_default_repository_path(settings_repository_path: Option<&str>) -> String {
    let configured = settings_repository_path.unwrap_or("");
    if !configured.is_empty() && is_git_repository(configured) {
        return configured.to_string();
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut current = cwd.clone();
        loop {
            let candidate = current.to_string_lossy().to_string();
            if is_git_repository(&candidate) {
                return candidate;
            }
            match current.parent() {
                Some(parent) if parent != current.as_path() => current = parent.to_path_buf(),
                _ => break,
            }
        }
    }

    let home = home_dir();
    let known: Vec<String> = vec![
        home.join("Documents").join("Sources").to_string_lossy().to_string(),
        home.join("Projects").to_string_lossy().to_string(),
        home.join("Documents").join("Projects").to_string_lossy().to_string(),
        home.join("source").join("repos").to_string_lossy().to_string(),
        "D:\\Projects".to_string(),
        "C:\\Projects".to_string(),
    ];

    for candidate in &known {
        if is_git_repository(candidate) {
            return candidate.clone();
        }
    }
    for candidate in &known {
        if Path::new(candidate).is_dir() {
            return candidate.clone();
        }
    }

    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn file_size(repo_path: &str, clean_path: &str) -> u64 {
    let full = Path::new(repo_path).join(clean_path);
    fs::metadata(&full)
        .map(|meta| if meta.is_file() { meta.len() } else { 0 })
        .unwrap_or(0)
}

fn warning_tooltip(has_warning: bool, size: u64) -> String {
    if has_warning {
        format!(
            "Serious Warning: File size is {} (> 800KB limit for non-images)",
            format_bytes(size)
        )
    } else {
        String::new()
    }
}

pub fn get_git_status(repo_path: &str) -> GitStatusResult {
    let res = run_git(
        &[
            "status".to_string(),
            "--porcelain=v1".to_string(),
            "-uall".to_string(),
        ],
        repo_path,
        true,
    );

    if res.exit_code != 0 && res.output.is_empty() {
        return GitStatusResult {
            unstaged: Vec::new(),
            staged: Vec::new(),
        };
    }

    let mut unstaged = Vec::new();
    let mut staged = Vec::new();

    for line in split_lines(&res.output) {
        if line.len() < 3 {
            continue;
        }
        let bytes = line.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        let mut raw_path = line[3..].trim().to_string();
        if let Some(idx) = raw_path.rfind(" -> ") {
            raw_path = raw_path[idx + 4..].to_string();
        }
        let clean_path = to_posix(&unquote_git_path(&raw_path));

        let size = file_size(repo_path, &clean_path);
        let is_image = is_image_file(&clean_path);
        let has_serious_warning = !is_image && size > SERIOUS_WARNING_THRESHOLD_BYTES;

        let file_name = posix_basename(&clean_path);
        let parent = posix_dirname(&clean_path);
        let folder_path = if parent == "." || parent.is_empty() {
            String::new()
        } else {
            parent
        };

        let is_untracked = x == '?' && y == '?';
        let is_conflicted = !is_untracked
            && (x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D'));
        let is_staged = !is_untracked && matches!(x, 'A' | 'M' | 'D' | 'R' | 'C');
        let has_unstaged_changes = is_untracked || y != ' ';
        let is_modified = !is_untracked && (x == 'M' || y == 'M');
        let is_added = !is_untracked && x == 'A';

        if has_unstaged_changes {
            let display = if is_untracked {
                "??".to_string()
            } else if y != ' ' {
                y.to_string()
            } else {
                x.to_string()
            };
            unstaged.push(GitFileStatus {
                status_code: display.clone(),
                file_path: clean_path.clone(),
                display_status: display,
                display_path: clean_path.clone(),
                clean_path: clean_path.clone(),
                file_name: file_name.clone(),
                folder_path: folder_path.clone(),
                is_untracked,
                is_staged: false,
                has_unstaged_changes: true,
                is_modified,
                is_added,
                is_deleted: y == 'D',
                is_conflicted,
                is_image,
                file_size_bytes: size as i64,
                has_serious_warning,
                serious_warning_tooltip: warning_tooltip(has_serious_warning, size),
            });
        }

        if is_staged {
            staged.push(GitFileStatus {
                status_code: x.to_string(),
                file_path: clean_path.clone(),
                display_status: x.to_string(),
                display_path: clean_path.clone(),
                clean_path,
                file_name,
                folder_path,
                is_untracked: false,
                is_staged: true,
                has_unstaged_changes: false,
                is_modified: x == 'M',
                is_added: x == 'A',
                is_deleted: x == 'D',
                is_conflicted: false,
                is_image,
                file_size_bytes: size as i64,
                has_serious_warning,
                serious_warning_tooltip: warning_tooltip(has_serious_warning, size),
            });
        }
    }

    GitStatusResult { unstaged, staged }
}

fn synthetize_untracked_diff(repo_path: &str, norm_path: &str) -> Option<String> {
    let full_path = Path::new(repo_path).join(norm_path);
    let content = fs::read_to_string(&full_path).ok()?;
    let lines: Vec<String> = split_lines(&content)
        .into_iter()
        .map(|line| format!("+{line}"))
        .collect();
    let joined = lines.join("\n");
    let line_count = joined.split('\n').count();
    Some(format!(
        "diff --git a/{norm_path} b/{norm_path}\n\
         new file mode 100644\n\
         --- /dev/null\n\
         +++ b/{norm_path}\n\
         @@ -0,0 +1,{line_count} @@\n\
         {joined}"
    ))
}

pub fn get_file_diff(repo_path: &str, file_path: &str, staged: bool) -> String {
    let norm_path = to_posix(file_path);

    if staged {
        let res = run_git(
            &[
                "diff".to_string(),
                "--cached".to_string(),
                "--".to_string(),
                norm_path,
            ],
            repo_path,
            true,
        );
        return first_non_empty(&res.output, &res.error, "(No changes in staged diff)");
    }

    let check_untracked = run_git(
        &[
            "status".to_string(),
            "--porcelain".to_string(),
            "--".to_string(),
            norm_path.clone(),
        ],
        repo_path,
        true,
    );
    if check_untracked.output.starts_with("??") {
        if let Some(diff) = synthetize_untracked_diff(repo_path, &norm_path) {
            return diff;
        }
        return "(Error reading untracked file)".to_string();
    }

    let res = run_git(
        &["diff".to_string(), "--".to_string(), norm_path],
        repo_path,
        true,
    );
    first_non_empty(&res.output, &res.error, "(No changes in working tree)")
}

pub fn get_all_diff(repo_path: &str, staged: bool) -> String {
    if staged {
        let res = run_git(
            &["diff".to_string(), "--cached".to_string()],
            repo_path,
            true,
        );
        return first_non_empty(&res.output, "", "(No staged changes)");
    }

    let res = run_git(&["diff".to_string()], repo_path, true);
    let mut buffer = res.output;

    let status_res = run_git(
        &[
            "status".to_string(),
            "--porcelain".to_string(),
            "-uall".to_string(),
        ],
        repo_path,
        true,
    );

    for line in split_lines(&status_res.output) {
        if !line.starts_with("??") {
            continue;
        }
        let raw = unquote_git_path(line[3..].trim());
        if is_image_file(&raw) {
            continue;
        }
        if let Some(diff) = synthetize_untracked_diff(repo_path, &to_posix(&raw)) {
            buffer.push('\n');
            buffer.push_str(&diff);
        }
    }

    if buffer.is_empty() {
        "(No changes)".to_string()
    } else {
        buffer
    }
}

pub fn get_commit_diff(repo_path: &str, commit_hash: &str) -> String {
    let res = run_git(
        &[
            "show".to_string(),
            "--format=".to_string(),
            commit_hash.to_string(),
        ],
        repo_path,
        true,
    );
    first_non_empty(&res.output, &res.error, "(No diff for commit)")
}

pub fn get_commit_file_diff(repo_path: &str, commit_hash: &str, file_path: &str) -> String {
    let norm_path = to_posix(file_path);
    let res = run_git(
        &[
            "show".to_string(),
            commit_hash.to_string(),
            "--".to_string(),
            norm_path,
        ],
        repo_path,
        true,
    );
    first_non_empty(&res.output, &res.error, "(No diff for this file in commit)")
}

pub fn get_image_diff(
    repo_path: &str,
    file_path: &str,
    staged: bool,
    commit_hash: Option<&str>,
) -> ImageDiffData {
    let norm_path = to_posix(file_path);
    let file_name = posix_basename(&norm_path);

    let before: Option<Vec<u8>>;
    let after: Option<Vec<u8>>;

    if let Some(hash) = commit_hash {
        before = run_git_binary(
            &["show".to_string(), format!("{hash}^:{norm_path}")],
            repo_path,
        );
        after = run_git_binary(&["show".to_string(), format!("{hash}:{norm_path}")], repo_path);
    } else if staged {
        before = run_git_binary(&["show".to_string(), format!("HEAD:{norm_path}")], repo_path);
        after = run_git_binary(&["show".to_string(), format!(":{norm_path}")], repo_path);
    } else {
        before = run_git_binary(&["show".to_string(), format!("HEAD:{norm_path}")], repo_path);
        let full_path = Path::new(repo_path).join(&norm_path);
        after = fs::read(&full_path).ok().filter(|bytes| !bytes.is_empty());
    }

    let has_before = before.as_ref().map(|b| !b.is_empty()).unwrap_or(false);
    let has_after = after.as_ref().map(|a| !a.is_empty()).unwrap_or(false);
    let engine = base64::engine::general_purpose::STANDARD;

    ImageDiffData {
        file_name: file_name.clone(),
        before: before.as_ref().map(|b| engine.encode(b)),
        after: after.as_ref().map(|a| engine.encode(a)),
        before_size: before.as_ref().map(|b| b.len()),
        after_size: after.as_ref().map(|a| a.len()),
        has_before,
        has_after,
        is_svg: file_name.to_lowercase().ends_with(".svg"),
    }
}

pub fn is_merge_in_progress(repo_path: &str) -> bool {
    let git_dir = Path::new(repo_path).join(".git");
    if !git_dir.is_dir() {
        return false;
    }
    git_dir.join("MERGE_HEAD").is_file()
        || git_dir.join("rebase-merge").is_dir()
        || git_dir.join("rebase-apply").is_dir()
        || git_dir.join("CHERRY_PICK_HEAD").is_file()
        || git_dir.join("REVERT_HEAD").is_file()
}

/// Pushes, and on a rejected push (remote ahead) pulls and retries once.
pub fn execute_push_with_auto_pull(repo_path: &str) -> GitCommandResult {
    let push_res = run_git(&["push".to_string()], repo_path, true);
    let combined = format!("{} {}", push_res.error, push_res.output).to_lowercase();

    let rejected = push_res.exit_code != 0
        && (combined.contains("[rejected]")
            || combined.contains("updates were rejected")
            || combined.contains("fetch first"));

    if !rejected {
        return push_res;
    }

    if is_merge_in_progress(repo_path) {
        return GitCommandResult::new(
            push_res.exit_code,
            push_res.output,
            "Push rejected: remote has updates, and a merge is in progress. \
             Manual resolution required.",
        );
    }

    let pull_res = run_git(
        &["pull".to_string(), "--no-edit".to_string()],
        repo_path,
        true,
    );
    if pull_res.exit_code == 0 {
        return run_git(&["push".to_string()], repo_path, true);
    }

    let fallback = first_non_empty(&pull_res.error, &pull_res.output, "");
    GitCommandResult::new(
        pull_res.exit_code,
        pull_res.output,
        format!("Auto-pull failed (manual resolution required): {fallback}"),
    )
}

pub fn get_recent_commits(repo_path: &str, count: u32) -> Vec<GitCommitInfo> {
    let res = run_git(
        &[
            "log".to_string(),
            format!("-n{count}"),
            "--pretty=format:%H|%h|%an|%ad|%s".to_string(),
            "--date=relative".to_string(),
        ],
        repo_path,
        true,
    );

    if res.exit_code != 0 || res.output.is_empty() {
        return Vec::new();
    }

    let mut commits = Vec::new();
    for line in split_lines(&res.output) {
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 5 {
            continue;
        }
        let message = parts[4..].join("|");
        let mut tag_section = String::new();
        let mut message_section = message.clone();

        let trimmed = message.trim_start();
        if trimmed.starts_with('[') {
            if let Some(end_idx) = trimmed.find(']') {
                tag_section = trimmed[..=end_idx].to_string();
                message_section = trimmed[end_idx + 1..].trim_start().to_string();
            }
        }

        commits.push(GitCommitInfo {
            hash: parts[0].to_string(),
            short_hash: parts[1].to_string(),
            author: parts[2].to_string(),
            date: parts[3].to_string(),
            message,
            tag_section,
            message_section,
        });
    }

    commits
}

pub fn get_commit_files(repo_path: &str, commit_hash: &str) -> Vec<GitFileStatus> {
    let res = run_git(
        &[
            "diff-tree".to_string(),
            "--no-commit-id".to_string(),
            "--name-status".to_string(),
            "-r".to_string(),
            commit_hash.to_string(),
        ],
        repo_path,
        true,
    );

    if res.exit_code != 0 || res.output.is_empty() {
        return Vec::new();
    }

    let mut files = Vec::new();
    for line in split_lines(&res.output) {
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').filter(|p| !p.is_empty()).collect();
        if parts.len() < 2 {
            continue;
        }

        let status_char = parts[0].trim().to_string();
        let clean_path = to_posix(&unquote_git_path(parts[1].trim()));
        let is_image = is_image_file(&clean_path);
        let file_name = posix_basename(&clean_path);
        let parent = posix_dirname(&clean_path);
        let folder_path = if parent == "." || parent.is_empty() {
            String::new()
        } else {
            parent
        };

        files.push(GitFileStatus {
            status_code: status_char.clone(),
            file_path: clean_path.clone(),
            display_status: status_char.clone(),
            display_path: clean_path.clone(),
            clean_path,
            file_name,
            folder_path,
            is_untracked: false,
            is_staged: false,
            has_unstaged_changes: false,
            is_modified: status_char == "M",
            is_added: status_char == "A",
            is_deleted: status_char == "D",
            is_conflicted: false,
            is_image,
            file_size_bytes: 0,
            has_serious_warning: false,
            serious_warning_tooltip: String::new(),
        });
    }

    files
}

fn first_non_empty(a: &str, b: &str, fallback: &str) -> String {
    if !a.is_empty() {
        a.to_string()
    } else if !b.is_empty() {
        b.to_string()
    } else {
        fallback.to_string()
    }
}
