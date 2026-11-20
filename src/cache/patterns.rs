//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Include / exclude glob sets for `input:` / `output:` declarations.
//!
//! Patterns prefixed with `!` are excludes (see `vendor::parse::pattern`). A file
//! is selected when it matches at least one include and no exclude. Includes are
//! expanded with `glob`; excludes are matched against the workspace-relative path
//! (or the absolute path, for absolute exclude patterns).

use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};

use crate::vendor::parse::pattern::split_patterns;

/// `*` and `?` never cross `/`; only `**` matches across directories.
const MATCH_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// Compiled include/exclude set of one pattern list.
#[derive(Debug, Clone)]
pub struct PatternSet {
    includes: Vec<String>,
    excludes: Vec<Pattern>,
}

impl PatternSet {
    /// Compile a pattern list. Exclude patterns ending in `/` are shorthand for
    /// everything below that directory (`dist/` == `dist/**`).
    pub fn new(patterns: &[String]) -> Result<Self, String> {
        let split = split_patterns(patterns);
        let excludes = split
            .excludes
            .iter()
            .map(|p| {
                let normalized = match p.strip_suffix('/') {
                    Some(dir) => format!("{dir}/**"),
                    None => p.to_string(),
                };
                Pattern::new(&normalized)
                    .map_err(|e| format!("Invalid exclude pattern '!{p}': {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            includes: split.includes.iter().map(|p| p.to_string()).collect(),
            excludes,
        })
    }

    /// Include patterns, without the excludes.
    pub fn includes(&self) -> &[String] {
        &self.includes
    }

    /// Number of exclude patterns.
    pub fn exclude_count(&self) -> usize {
        self.excludes.len()
    }

    /// `true` if `path` (absolute or workspace-relative) matches an exclude.
    pub fn is_excluded(&self, workspace: &Path, path: &Path) -> bool {
        if self.excludes.is_empty() {
            return false;
        }
        let rel = path.strip_prefix(workspace).unwrap_or(path);
        let abs = workspace.join(rel);
        self.excludes.iter().any(|p| {
            if p.as_str().starts_with('/') {
                p.matches_path_with(&abs, MATCH_OPTIONS)
            } else {
                p.matches_path_with(rel, MATCH_OPTIONS)
            }
        })
    }

    /// Absolute glob pattern for an include, relative includes resolved against `workspace`.
    pub fn full_pattern(workspace: &Path, include: &str) -> String {
        if include.starts_with('/') {
            include.to_string()
        } else {
            workspace.join(include).to_string_lossy().to_string()
        }
    }

    /// Expand to the sorted, de-duplicated list of matching files (absolute paths).
    /// Invalid include patterns and unreadable entries are skipped.
    pub fn expand_files(&self, workspace: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self
            .includes
            .iter()
            .filter_map(|include| glob::glob(&Self::full_pattern(workspace, include)).ok())
            .flat_map(|paths| paths.filter_map(Result::ok))
            .filter(|p| p.is_file() && !self.is_excluded(workspace, p))
            .collect();
        files.sort();
        files.dedup();
        files
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|p| p.to_string()).collect()
    }

    fn workspace() -> TempDir {
        let tmp = TempDir::new().unwrap();
        for f in ["src/main.rs", "src/gen/out.rs", "firmware/a/b.bin", "firmware/c.rs", "README.md"] {
            let p = tmp.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, f).unwrap();
        }
        tmp
    }

    fn rel(ws: &Path, files: Vec<PathBuf>) -> Vec<String> {
        files
            .into_iter()
            .map(|p| p.strip_prefix(ws).unwrap().to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn test_split_into_includes() {
        let set = PatternSet::new(&s(&["src/**", "!src/gen/**", "README.md"])).unwrap();
        assert_eq!(set.includes(), &["src/**", "README.md"]);
        assert_eq!(set.exclude_count(), 1);
    }

    #[test]
    fn test_expand_applies_excludes() {
        let ws = workspace();
        let set = PatternSet::new(&s(&["**/*", "!firmware/**", "!src/gen/**"])).unwrap();
        assert_eq!(rel(ws.path(), set.expand_files(ws.path())), vec!["README.md", "src/main.rs"]);
    }

    #[test]
    fn test_exclude_star_does_not_cross_directories() {
        let ws = workspace();
        // `firmware/*` excludes only direct children, not `firmware/a/b.bin`.
        let set = PatternSet::new(&s(&["firmware/**/*", "!firmware/*"])).unwrap();
        assert_eq!(rel(ws.path(), set.expand_files(ws.path())), vec!["firmware/a/b.bin"]);
    }

    #[test]
    fn test_exclude_directory_shorthand() {
        let ws = workspace();
        let set = PatternSet::new(&s(&["**/*.rs", "!src/"])).unwrap();
        assert_eq!(rel(ws.path(), set.expand_files(ws.path())), vec!["firmware/c.rs"]);
    }

    #[test]
    fn test_is_excluded_relative_and_absolute_paths() {
        let ws = Path::new("/ws");
        let set = PatternSet::new(&s(&["**", "!firmware/**"])).unwrap();
        assert!(set.is_excluded(ws, Path::new("firmware/x/y.bin")));
        assert!(set.is_excluded(ws, Path::new("/ws/firmware/x/y.bin")));
        assert!(!set.is_excluded(ws, Path::new("/ws/src/firmware.rs")));
    }

    #[test]
    fn test_invalid_exclude_is_error() {
        // `**` must be a whole path component in glob syntax.
        let err = PatternSet::new(&s(&["**", "!firmware/**.bin"])).unwrap_err();
        assert!(err.contains("!firmware/**.bin"), "{err}");
    }
}
