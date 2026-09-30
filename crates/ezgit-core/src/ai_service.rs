use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

use crate::git_service::split_lines;
use crate::models::{AiSettings, GitPrefixMode};

pub const MAX_DIFF_CHARS_FOR_COMMIT: usize = 3500;
pub const COMPACT_LINES_PER_FILE: usize = 30;

static LEADING_QUOTES: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^[`'"]+"#).unwrap());
static TRAILING_QUOTES: Lazy<Regex> = Lazy::new(|| Regex::new(r#"[`'"]+$"#).unwrap());
static WHITESPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static LEADING_BRACKET_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\[[^\]]+\]\s*").unwrap());
static NON_ALPHANUMERIC: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^A-Za-z0-9]").unwrap());
static DIFF_PATH: Lazy<Regex> = Lazy::new(|| Regex::new(r"diff --git a/(.+?) b/").unwrap());

/// Splits a diff into per-file sections at each `diff --git ` line start.
/// Equivalent to Dart's `split(RegExp(r'(?=^diff --git )', multiLine: true))`,
/// which the Rust `regex` crate cannot express (no look-around).
fn split_diff_sections(text: &str) -> Vec<String> {
    let mut starts: Vec<usize> = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            starts.push(offset);
        }
        offset += line.len();
    }

    if starts.is_empty() {
        return vec![text.to_string()];
    }

    let mut sections = Vec::with_capacity(starts.len() + 1);
    if starts[0] > 0 {
        sections.push(text[..starts[0]].to_string());
    }
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(text.len());
        sections.push(text[*start..end].to_string());
    }
    sections
}

/// Returns the first `n` characters of `s` without panicking on UTF-8 bounds.
fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

fn joined_chars_len(parts: &[String]) -> usize {
    let total: usize = parts.iter().map(|p| p.chars().count()).sum();
    total + parts.len().saturating_sub(1)
}

/// Best-effort file path extraction from a `diff --git` section.
pub fn extract_diff_file_path(section: &str) -> String {
    if section.is_empty() {
        return "unknown".to_string();
    }

    let lines = split_lines(section);
    for line in &lines {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            return rest.trim().to_string();
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            return rest.trim().to_string();
        }
    }

    if let Some(caps) = DIFF_PATH.captures(section) {
        if let Some(group) = caps.get(1) {
            return group.as_str().trim().to_string();
        }
    }

    let first = lines.first().map(|l| l.trim().to_string()).unwrap_or_default();
    if first.is_empty() {
        "unknown".to_string()
    } else {
        first
    }
}

/// Shrinks a large diff to a path + first-changed-lines-per-file summary.
pub fn build_compact_diff(
    full_diff: &str,
    max_total_chars: usize,
    lines_per_file: usize,
) -> String {
    if full_diff.is_empty() {
        return String::new();
    }
    if full_diff.chars().count() <= max_total_chars {
        return full_diff.to_string();
    }

    let sections: Vec<String> = split_diff_sections(full_diff)
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect();

    if sections.is_empty() {
        return format!("{}\n... [truncated]", take_chars(full_diff, max_total_chars));
    }

    let file_count = sections.len();
    let per_file_budget = (max_total_chars / file_count.max(1)).saturating_sub(120);
    let effective_per_file_budget = per_file_budget.max(300);
    let raw_lines_budget = effective_per_file_budget / 60;
    let lines_budget = 10.min(lines_per_file).min(raw_lines_budget);

    let header = format!(
        "[Full diff was {} chars across {} file(s); shortened to file paths + first {} lines per file]\n",
        full_diff.chars().count(),
        file_count,
        lines_budget
    );

    let mut lines_out: Vec<String> = vec![header];
    let mut current_length = lines_out[0].chars().count();

    for (i, section) in sections.iter().enumerate() {
        let file_path = extract_diff_file_path(section);
        let section_header = format!("\n--- File: {file_path} ---\n");
        let mut kept: Vec<String> = Vec::new();
        let mut changed_kept = 0usize;

        let raw_lines = split_lines(section);
        for raw_line in &raw_lines {
            let line = raw_line.trim_end().to_string();
            let is_header = line.starts_with("diff --git")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with("@@")
                || line.starts_with("Binary files ")
                || line.starts_with("new file")
                || line.starts_with("deleted file")
                || line.starts_with("similarity")
                || line.starts_with("rename ");

            if is_header {
                kept.push(line);
                continue;
            }

            if changed_kept >= lines_budget {
                continue;
            }

            let is_change = !line.is_empty() && matches!(line.as_bytes()[0], b'+' | b'-');
            if is_change {
                kept.push(line);
                changed_kept += 1;
            }
        }

        if changed_kept == 0 {
            let fallback: Vec<String> = raw_lines
                .iter()
                .map(|l| l.trim_end().to_string())
                .filter(|l| !l.trim().is_empty())
                .take(lines_budget)
                .collect();
            kept.extend(fallback);
        }

        let mut file_block = kept.join("\n");
        if file_block.chars().count() > effective_per_file_budget {
            file_block = format!(
                "{}\n... [file truncated]",
                take_chars(&file_block, effective_per_file_budget)
            );
        } else if raw_lines.len() > kept.len() + 1 {
            file_block.push_str(&format!(
                "\n... [truncated {} more lines for this file]",
                raw_lines.len() - kept.len()
            ));
        }

        if current_length + file_block.chars().count() + section_header.chars().count()
            > max_total_chars
        {
            let remaining = file_count - i;
            let room_raw = max_total_chars as i64 - current_length as i64 - 200;
            let room = if room_raw < 0 { 0 } else { room_raw as usize };
            lines_out.push(format!("{section_header}{}", take_chars(&file_block, room)));
            lines_out.push(format!(
                "\n... [omitted diff bodies for {} more file(s); paths listed below]",
                remaining - 1
            ));
            for rest in &sections[i + 1..] {
                lines_out.push(format!("- {}", extract_diff_file_path(rest)));
                if joined_chars_len(&lines_out) >= max_total_chars {
                    break;
                }
            }
            break;
        }

        lines_out.push(format!("{section_header}{file_block}"));
        current_length += section_header.chars().count() + file_block.chars().count();
    }

    let mut result = lines_out.join("\n");
    if result.chars().count() > max_total_chars {
        result = format!("{}\n... [truncated]", take_chars(&result, max_total_chars));
    }
    result
}

/// Derives a folder tag such as `Services\GitService` from changed file paths.
pub fn build_path_tag(changed_files: &[String]) -> String {
    if changed_files.is_empty() {
        return "others".to_string();
    }

    let mut counts: Vec<(String, u32)> = Vec::new();

    for raw in changed_files {
        let mut value = raw.replace('\\', "/").trim().to_string();
        if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
            value = value[1..value.len() - 1].to_string();
        }

        let parts: Vec<&str> = value.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() > 1 {
            let take = if parts.len() - 1 > 3 { 3 } else { parts.len() - 1 };
            let tag_parts: Vec<String> = parts[..take]
                .iter()
                .map(|part| NON_ALPHANUMERIC.replace_all(part, "").to_string())
                .filter(|part| !part.is_empty())
                .map(|part| if part.len() > 8 { part[..8].to_string() } else { part })
                .collect();

            if !tag_parts.is_empty() {
                let tag = tag_parts.join("\\");
                match counts.iter_mut().find(|(key, _)| key == &tag) {
                    Some((_, count)) => *count += 1,
                    None => counts.push((tag, 1)),
                }
            }
        }
    }

    let mut most_used_tag = "others".to_string();
    let mut max_count = 0u32;
    for (tag, count) in counts {
        if count > max_count {
            max_count = count;
            most_used_tag = tag;
        }
    }

    most_used_tag
}

pub fn get_formatted_prefix(path_tag: &str, settings: &AiSettings) -> String {
    let custom = settings.custom_prefix.trim();
    let tag = if path_tag.is_empty() { "others" } else { path_tag };

    match settings.prefix_mode {
        GitPrefixMode::AutoFolder => format!("[{tag}]"),
        GitPrefixMode::Custom => custom.to_string(),
        GitPrefixMode::Both => {
            if custom.is_empty() {
                format!("[{tag}]")
            } else {
                format!("[{tag}] {custom}")
            }
        }
        GitPrefixMode::None => String::new(),
    }
}

pub fn normalize_commit_message(message: &str, path_tag: &str, settings: &AiSettings) -> String {
    if message.is_empty() {
        return String::new();
    }

    let mut normalized = message.trim().to_string();
    normalized = LEADING_QUOTES.replace_all(&normalized, "").to_string();
    normalized = TRAILING_QUOTES.replace_all(&normalized, "").to_string();
    normalized = WHITESPACE.replace_all(&normalized, " ").to_string();
    normalized = normalized.trim().to_string();
    normalized = LEADING_BRACKET_PREFIX.replace(&normalized, "").to_string();
    normalized = normalized.trim().to_string();

    let prefix = get_formatted_prefix(path_tag, settings);
    if !prefix.is_empty() {
        normalized = format!("{prefix} {normalized}");
    }

    if normalized.chars().count() > 120 {
        normalized = take_chars(&normalized, 120).trim_end().to_string();
    }

    normalized
}

/// Posts a chat-completion request to an OpenAI-compatible endpoint.
pub async fn generate_text(
    system_prompt: &str,
    user_prompt: &str,
    settings: &AiSettings,
    temperature: f64,
    max_tokens: u32,
) -> Result<String, String> {
    if settings.api_url.is_empty() {
        return Err("API URL is not configured. Please open AI Settings to enter a valid \
                    OpenAI API endpoint."
            .to_string());
    }

    let payload = json!({
        "model": if settings.model.is_empty() { "deepseek-chat" } else { settings.model.as_str() },
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_prompt },
        ],
        "max_tokens": max_tokens,
        "temperature": temperature,
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let mut request = client
        .post(settings.api_url.trim())
        .header("Content-Type", "application/json")
        .json(&payload);

    if !settings.api_key.trim().is_empty() {
        request = request.header("Authorization", format!("Bearer {}", settings.api_key.trim()));
    }

    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let response_text = response.text().await.map_err(|e| e.to_string())?;

    if !status.is_success() {
        return Err(format!(
            "API request failed ({}): {}\n{}",
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
            response_text
        ));
    }

    let data: Value = serde_json::from_str(&response_text).map_err(|e| e.to_string())?;
    let content = data
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|first| {
            first
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(|content| content.as_str())
                .or_else(|| first.get("text").and_then(|text| text.as_str()))
        })
        .map(|s| s.trim().to_string());

    match content {
        Some(text) if !text.is_empty() => Ok(text),
        _ => Err("AI service returned an empty response.".to_string()),
    }
}

pub async fn test_connection(settings: &AiSettings) -> Result<String, String> {
    let start = Instant::now();
    let reply = generate_text(
        "You are an API tester. Return only 'OK'.",
        "Ping",
        settings,
        0.1,
        10,
    )
    .await?;
    let elapsed = start.elapsed().as_millis();
    Ok(format!("Success ({elapsed} ms): {reply}"))
}

pub async fn generate_commit_message(
    diff: &str,
    changed_files: &[String],
    settings: &AiSettings,
) -> Result<String, String> {
    const SYSTEM_PROMPT: &str =
        "Write one concise Git commit description. Do not add a tag, prefix, or square brackets; \
         the application adds the appropriate prefix. Return only that single line, with no quotes, \
         no Markdown, and no explanation. Keep the whole line under 110 characters.";

    let compact_diff = build_compact_diff(diff, MAX_DIFF_CHARS_FOR_COMMIT, COMPACT_LINES_PER_FILE);
    let user_prompt = format!("Create a commit message for this diff:\n\n{compact_diff}");

    let raw_message = generate_text(SYSTEM_PROMPT, &user_prompt, settings, 0.2, 120).await?;

    let path_tag = build_path_tag(changed_files);
    Ok(normalize_commit_message(&raw_message, &path_tag, settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_prefers_plus_header() {
        let section = "diff --git a/lib/main.dart b/lib/main.dart\n\
                       index 123..456 100644\n\
                       --- a/lib/main.dart\n\
                       +++ b/lib/main.dart\n\
                       @@ -1 +1 @@\n\
                       -a\n\
                       +b";
        assert_eq!(extract_diff_file_path(section), "lib/main.dart");
    }

    #[test]
    fn extract_falls_back_to_diff_git_path() {
        assert_eq!(
            extract_diff_file_path("diff --git a/src/app.ts b/src/app.ts\nsomebody"),
            "src/app.ts"
        );
    }

    #[test]
    fn extract_returns_unknown_for_empty() {
        assert_eq!(extract_diff_file_path(""), "unknown");
    }

    #[test]
    fn path_tag_picks_most_common_folder() {
        let files = vec![
            "src/utils/a.dart".to_string(),
            "src/utils/b.dart".to_string(),
            "lib/other.dart".to_string(),
        ];
        assert_eq!(build_path_tag(&files), "src\\utils");
    }

    #[test]
    fn path_tag_joins_up_to_three_parts() {
        assert_eq!(build_path_tag(&["a/b/c/file.txt".to_string()]), "a\\b\\c");
    }

    #[test]
    fn path_tag_defaults_to_others() {
        assert_eq!(build_path_tag(&[]), "others");
        assert_eq!(build_path_tag(&["README.md".to_string()]), "others");
    }

    #[test]
    fn compact_diff_passes_small_diffs_through() {
        let diff = "diff --git a/a.txt b/a.txt\n+hello";
        assert_eq!(build_compact_diff(diff, MAX_DIFF_CHARS_FOR_COMMIT, COMPACT_LINES_PER_FILE), diff);
    }

    #[test]
    fn compact_diff_shrinks_large_diffs() {
        let mut buffer = String::new();
        for f in 0..40 {
            buffer.push_str(&format!("diff --git a/file{f}.txt b/file{f}.txt\n"));
            buffer.push_str(&format!(
                "index 1..2 100644\n--- a/file{f}.txt\n+++ b/file{f}.txt\n"
            ));
            buffer.push_str("@@ -1,50 +1,50 @@\n");
            for l in 0..50 {
                buffer.push_str(&format!("+line {f}-{l} of some reasonably long content here\n"));
            }
        }
        assert!(buffer.chars().count() > MAX_DIFF_CHARS_FOR_COMMIT);

        let compact = build_compact_diff(&buffer, MAX_DIFF_CHARS_FOR_COMMIT, COMPACT_LINES_PER_FILE);
        assert!(
            compact.chars().count() <= MAX_DIFF_CHARS_FOR_COMMIT + 32,
            "length {} exceeds budget",
            compact.chars().count()
        );
        assert!(compact.contains("shortened to file paths"));
        assert!(compact.contains("[truncated]"), "missing truncation marker");
    }

    #[test]
    fn prefix_modes() {
        assert_eq!(get_formatted_prefix("src\\utils", &AiSettings::default()), "[src\\utils]");
        let custom = AiSettings {
            prefix_mode: GitPrefixMode::Custom,
            custom_prefix: "feat:".into(),
            ..AiSettings::default()
        };
        assert_eq!(get_formatted_prefix("src\\utils", &custom), "feat:");
        let both = AiSettings {
            prefix_mode: GitPrefixMode::Both,
            custom_prefix: "feat:".into(),
            ..AiSettings::default()
        };
        assert_eq!(get_formatted_prefix("src\\utils", &both), "[src\\utils] feat:");
        let none = AiSettings {
            prefix_mode: GitPrefixMode::None,
            ..AiSettings::default()
        };
        assert_eq!(get_formatted_prefix("src\\utils", &none), "");
    }

    #[test]
    fn normalize_adds_prefix_and_strips_ai_brackets() {
        assert_eq!(
            normalize_commit_message("\"[src\\utils] Fix the thing\"", "src\\utils", &AiSettings::default()),
            "[src\\utils] Fix the thing"
        );
    }

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(
            normalize_commit_message("  Fix   the\n  thing ", "src\\utils", &AiSettings::default()),
            "[src\\utils] Fix the thing"
        );
    }

    #[test]
    fn normalize_caps_at_120_chars() {
        let long = "x".repeat(400);
        let result = normalize_commit_message(&long, "src\\utils", &AiSettings::default());
        assert!(result.chars().count() <= 120);
    }
}
