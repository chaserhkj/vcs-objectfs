//! Object file identity: a regular file whose name is a lowercase hex string
//! at least [`DEFAULT_MIN_NAME_LENGTH`] characters long, or of any length when
//! the minimum is set to 0, and whose path relative to the source directory
//! matches no exclusion pattern.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::exclude::Excludes;
/// Shortest name recognized as an object hash by default.
pub const DEFAULT_MIN_NAME_LENGTH: usize = 16;

#[derive(Debug, Clone)]
pub struct ObjectNameDef {
    source: PathBuf,
    min_length: usize,
    excludes: Arc<Excludes>,
}

impl ObjectNameDef {
    /// Recognizes object files below `source`.
    pub fn new(source: PathBuf) -> Self {
        Self {
            source,
            min_length: DEFAULT_MIN_NAME_LENGTH,
            excludes: Arc::default(),
        }
    }

    /// Recognizes lowercase hex names of at least `min_length` bytes; 0 accepts every length.
    pub fn with_min_length(mut self, min_length: usize) -> Self {
        self.min_length = min_length;
        self
    }

    /// Recognizes object files at none of the source-relative paths `excludes` matches.
    pub fn with_excludes(mut self, excludes: Arc<Excludes>) -> Self {
        self.excludes = excludes;
        self
    }

    /// Whether `name` is a lowercase hexadecimal string of the required length.
    pub fn is_name(&self, name: &OsStr) -> bool {
        // A name holds at least one byte, so a minimum of 0 recognizes one hex digit and up.
        let min_length = self.min_length.max(1);
        name.to_str().is_some_and(|name| {
            name.len() >= min_length
                && name
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    }

    /// Whether `path` is a regular file with an object file name.
    ///
    /// Directories and symlinks are excluded, so a fanout directory named after
    /// a hash prefix stays an ordinary directory.
    pub fn is_file(&self, path: &Path) -> bool {
        self.has_object_name(path)
            && !self.is_excluded(path)
            && fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
    }

    /// Whether an entry created at `path` may become an object file.
    ///
    /// Unlike [`Self::is_file`], a new entry is a regular file by definition, so
    /// only the name and the exclusion patterns decide.
    pub fn admits_creation(&self, path: &Path) -> bool {
        self.has_object_name(path) && !self.is_excluded(path)
    }

    /// Whether the file name of `path` is an object file name.
    fn has_object_name(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| self.is_name(name))
    }

    /// Whether an exclusion pattern matches `path`; a path outside the source matches none.
    fn is_excluded(&self, path: &Path) -> bool {
        path.strip_prefix(&self.source)
            .is_ok_and(|relative| self.excludes.matches(relative))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT: &str = "0123456789abcdef";

    /// Name-only rule, which needs no backing directory.
    fn names() -> ObjectNameDef {
        ObjectNameDef::new(PathBuf::from("/source"))
    }

    #[test]
    fn hex_names_of_sufficient_length_are_object_names() {
        let names = names();
        for name in [OBJECT, "0123456789abcdef0", "0000000000000000"] {
            assert!(names.is_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn short_hex_names_are_not_object_names() {
        let names = names();
        for name in ["", "0", "deadbeef", "0123456789abcde"] {
            assert!(!names.is_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn other_names_are_not_object_names() {
        let names = names();
        for name in [
            "DEADBEEFDEADBEEF",
            "DeadBeefDeadBeef",
            "dead beef dead ",
            "deadbeefdeadbeef.txt",
            "pack-abc123.pack",
            "gggggggggggggggg",
            "-1--------------",
        ] {
            assert!(!names.is_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn min_length_is_configurable() {
        let names = names().with_min_length(4);
        assert!(names.is_name(OsStr::new("deadbeef")));
        assert!(!names.is_name(OsStr::new("abc")));
    }

    #[test]
    fn zero_min_length_accepts_any_length() {
        let names = names().with_min_length(0);
        assert!(names.is_name(OsStr::new("0")));
        assert!(names.is_name(OsStr::new(OBJECT)));
        assert!(!names.is_name(OsStr::new("")));
        assert!(!names.is_name(OsStr::new("gg")));
    }

    #[test]
    fn file_name_decides_identity() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let object = root.join(OBJECT);
        let prefixed = root.join("prefix");
        let plain = root.join("notes.txt");
        let nested = root.join("fedcba9876543210");
        let short = root.join("abc");

        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&prefixed).unwrap();
        fs::write(&object, b"object").unwrap();
        fs::write(&plain, b"text").unwrap();
        fs::write(&short, b"short").unwrap();
        fs::write(prefixed.join(OBJECT), b"object").unwrap();

        let names = ObjectNameDef::new(root.to_path_buf());
        assert!(names.is_file(&object));
        assert!(names.is_file(&prefixed.join(OBJECT)));
        assert!(!names.is_file(&nested), "a directory is not an object file");
        assert!(!names.is_file(&short), "a short hex name is ordinary");
        assert!(!names.is_file(&plain));
        assert!(!names.is_file(&root.join("missing")));
        assert!(!names.is_file(&root.join("deadbeef")));
    }

    #[test]
    fn excluded_paths_are_not_object_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let excluded = root.join("info");
        fs::create_dir_all(&excluded).unwrap();
        let object = excluded.join(OBJECT);
        fs::write(&object, b"object").unwrap();

        let excludes = Arc::new(Excludes::new(&["info/*"]).unwrap());
        let names = ObjectNameDef::new(root.to_path_buf()).with_excludes(excludes);
        assert!(!names.is_file(&object));
        assert!(!names.admits_creation(&object));
        assert!(names.is_name(OsStr::new(OBJECT)), "the name stays hex");

        let plain = root.join(OBJECT);
        fs::write(&plain, b"object").unwrap();
        assert!(names.is_file(&plain), "a path outside the pattern");
        assert!(names.admits_creation(&plain));
    }

    #[test]
    fn creation_needs_a_name_and_no_exclusion() {
        let names = ObjectNameDef::new(PathBuf::from("/source"))
            .with_excludes(Arc::new(Excludes::new(&["ab/**"]).unwrap()));
        assert!(names.admits_creation(&Path::new("/source/cd").join(OBJECT)));
        assert!(!names.admits_creation(&Path::new("/source/ab").join(OBJECT)));
        assert!(!names.admits_creation(&Path::new("/source/cd").join("notes.txt")));
    }
}
