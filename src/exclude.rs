//! Path patterns whose entries keep their ordinary, mutable behavior.
//!
//! A pattern is a glob matched against a path relative to the source directory,
//! with `/` as separator and `*`/`?` stopping at a `/`, so `*.lock` matches a
//! top-level file only and `**` spans directories.

use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// Globs of paths, relative to the source directory, excluded from object recognition.
#[derive(Debug, Clone, Default)]
pub struct Excludes {
    patterns: GlobSet,
}

impl Excludes {
    /// Compiles `patterns`; an invalid pattern names the offending glob.
    pub fn new<S: AsRef<str>>(patterns: &[S]) -> Result<Self, String> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            let pattern = pattern.as_ref();
            let glob = GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|err| format!("invalid exclude pattern {pattern:?}: {err}"))?;
            builder.add(glob);
        }
        let patterns = builder
            .build()
            .map_err(|err| format!("invalid exclude patterns: {err}"))?;
        Ok(Self { patterns })
    }

    /// Whether `relative`, a path below the source directory, matches a pattern.
    pub fn matches(&self, relative: &Path) -> bool {
        !self.patterns.is_empty() && self.patterns.is_match(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn excludes(patterns: &[&str]) -> Excludes {
        Excludes::new(patterns).unwrap()
    }

    #[test]
    fn patterns_match_relative_paths() {
        let excludes = excludes(&["info/*"]);
        assert!(excludes.matches(Path::new("info/refs")));
        assert!(excludes.matches(Path::new("info/0123456789abcdef")));
        assert!(!excludes.matches(Path::new("0123456789abcdef")));
        assert!(!excludes.matches(Path::new("info")));
    }

    #[test]
    fn a_separator_ends_a_wildcard() {
        let excludes = excludes(&["*.lock"]);
        assert!(excludes.matches(Path::new("config.lock")));
        assert!(!excludes.matches(Path::new("ab/config.lock")));
        assert!(!excludes.matches(Path::new("config.txt")));
    }

    #[test]
    fn double_star_spans_directories() {
        let excludes = excludes(&["**/tmp_*"]);
        assert!(excludes.matches(Path::new("tmp_file")));
        assert!(excludes.matches(Path::new("ab/tmp_file")));
        assert!(!excludes.matches(Path::new("ab/file")));
    }

    #[test]
    fn braced_alternatives_match() {
        let excludes = excludes(&["{ab,cd}/**"]);
        assert!(excludes.matches(Path::new("ab/0123456789abcdef")));
        assert!(excludes.matches(Path::new("cd/ef/x")));
        assert!(!excludes.matches(Path::new("ef/0123456789abcdef")));
    }

    #[test]
    fn no_patterns_match_nothing() {
        let excludes = Excludes::default();
        assert!(!excludes.matches(Path::new("0123456789abcdef")));
        assert!(!excludes.matches(Path::new("info/refs")));
    }

    #[test]
    fn an_invalid_pattern_names_the_glob() {
        let err = Excludes::new(&["[ab"]).unwrap_err();
        assert!(err.contains("[ab"), "{err}");
    }
}
