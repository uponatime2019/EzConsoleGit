//! Diff text classification, ported from the Flutter `DiffViewer._parse`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Added,
    Removed,
    Hunk,
    Meta,
    Normal,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// Classifies every line of a unified diff. `+++`/`---` file headers are meta
/// rather than additions/deletions, matching the desktop apps.
pub fn parse(text: &str) -> Vec<DiffLine> {
    if text.is_empty() {
        return Vec::new();
    }

    text.split('\n')
        .map(|raw| {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let kind = if line.starts_with("+++")
                || line.starts_with("---")
                || line.starts_with("diff --git")
                || line.starts_with("index ")
                || line.starts_with("new file")
                || line.starts_with("deleted file")
                || line.starts_with("similarity index")
                || line.starts_with("rename ")
                || line.starts_with("Binary files ")
                || line.starts_with("old mode")
                || line.starts_with("new mode")
            {
                DiffLineKind::Meta
            } else if line.starts_with("@@") {
                DiffLineKind::Hunk
            } else if line.starts_with('+') {
                DiffLineKind::Added
            } else if line.starts_with('-') {
                DiffLineKind::Removed
            } else {
                DiffLineKind::Normal
            };

            DiffLine {
                kind,
                text: line.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_are_meta_not_changes() {
        let lines = parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n same");
        let kinds: Vec<_> = lines.iter().map(|l| l.kind).collect();
        let expected = [
            DiffLineKind::Meta,
            DiffLineKind::Meta,
            DiffLineKind::Meta,
            DiffLineKind::Hunk,
            DiffLineKind::Removed,
            DiffLineKind::Added,
            DiffLineKind::Normal,
        ];
        // Trailing element from the final split is absent: split always yields
        // one element per '\n'-separated segment.
        assert_eq!(&kinds[..expected.len()], &expected[..]);
    }

    #[test]
    fn empty_diff_has_no_lines() {
        assert!(parse("").is_empty());
    }
}
