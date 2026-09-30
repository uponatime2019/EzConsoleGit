//! Folder-tree grouping of changed files, ported from the Flutter/Tauri
//! `groupKeyFor` helper.

use ezgit_core::models::GitFileStatus;

/// Folder group key for a changed file at the given tree depth.
///
/// `src/services/api/handlers.rs` at depth 2 becomes `src/services`, and a
/// top-level file such as `README.md` becomes `Root`.
pub fn group_key_for(file_path: &str, depth: u32) -> String {
    let normalized = file_path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();

    if parts.len() <= 1 {
        return "Root".to_string();
    }

    let depth = depth.max(1) as usize;
    let take = if depth < parts.len() - 1 {
        depth
    } else {
        parts.len() - 1
    };
    parts[..take].join("/")
}

/// A group of files sharing the same folder key.
#[derive(Debug, Clone)]
pub struct Group {
    pub key: String,
    pub items: Vec<GitFileStatus>,
    pub is_untracked_all: bool,
    pub has_serious_warning: bool,
}

/// Groups files by folder key, preserving first-seen ordering like the originals.
pub fn group_files(files: &[GitFileStatus], depth: u32) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();

    for file in files {
        let key = group_key_for(&file.clean_path, depth);
        match groups.iter_mut().find(|g| g.key == key) {
            Some(group) => group.items.push(file.clone()),
            None => groups.push(Group {
                key,
                items: vec![file.clone()],
                is_untracked_all: false,
                has_serious_warning: false,
            }),
        }
    }

    for group in &mut groups {
        group.is_untracked_all = group.items.iter().all(|f| f.is_untracked);
        group.has_serious_warning = group.items.iter().any(|f| f.has_serious_warning);
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_level_file_is_root() {
        assert_eq!(group_key_for("README.md", 2), "Root");
    }

    #[test]
    fn depth_limits_components() {
        assert_eq!(group_key_for("a/b/c/file.txt", 2), "a/b");
        assert_eq!(group_key_for("a/b/c/file.txt", 6), "a/b/c");
        assert_eq!(group_key_for("a/b/c/file.txt", 1), "a");
    }

    #[test]
    fn windows_separators_are_normalised() {
        assert_eq!(group_key_for("src\\utils\\a.dart", 2), "src/utils");
    }
}
