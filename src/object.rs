//! Object file identity: a regular file whose name is a lowercase hex string
//! at least [`DEFAULT_MIN_NAME_LENGTH`] characters long.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

/// Shortest name recognized as an object hash by default.
pub const DEFAULT_MIN_NAME_LENGTH: usize = 16;

/// Recognition rule for object file names.
#[derive(Debug, Clone, Copy)]
pub struct ObjectNames {
    min_length: usize,
}

impl Default for ObjectNames {
    fn default() -> Self {
        Self {
            min_length: DEFAULT_MIN_NAME_LENGTH,
        }
    }
}

impl ObjectNames {
    /// Recognizes lowercase hex names of at least `min_length` bytes.
    pub fn new(min_length: usize) -> Self {
        Self { min_length }
    }

    /// Whether `name` is a lowercase hexadecimal string of the required length.
    pub fn is_name(&self, name: &OsStr) -> bool {
        name.to_str().is_some_and(|name| {
            name.len() >= self.min_length
                && name
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    }

    /// Whether `path` is a regular file with an object file name.
    ///
    /// Directories and symlinks are excluded, so a fanout directory named after a
    /// hash prefix stays an ordinary directory.
    pub fn is_file(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| self.is_name(name))
            && fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT: &str = "0123456789abcdef";

    #[test]
    fn hex_names_of_sufficient_length_are_object_names() {
        let names = ObjectNames::default();
        for name in [OBJECT, "0123456789abcdef0", "0000000000000000"] {
            assert!(names.is_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn short_hex_names_are_not_object_names() {
        let names = ObjectNames::default();
        for name in ["", "0", "deadbeef", "0123456789abcde"] {
            assert!(!names.is_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn other_names_are_not_object_names() {
        let names = ObjectNames::default();
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
        let names = ObjectNames::new(4);
        assert!(names.is_name(OsStr::new("deadbeef")));
        assert!(!names.is_name(OsStr::new("abc")));
    }

    #[test]
    fn file_name_decides_identity() {
        let directory = tempfile::tempdir().unwrap();
        let object = directory.path().join(OBJECT);
        let prefixed = directory.path().join("prefix");
        let plain = directory.path().join("notes.txt");
        let nested = directory.path().join("fedcba9876543210");
        let short = directory.path().join("abc");

        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&prefixed).unwrap();
        fs::write(&object, b"object").unwrap();
        fs::write(&plain, b"text").unwrap();
        fs::write(&short, b"short").unwrap();
        fs::write(prefixed.join(OBJECT), b"object").unwrap();

        let names = ObjectNames::default();
        assert!(names.is_file(&object));
        assert!(names.is_file(&prefixed.join(OBJECT)));
        assert!(!names.is_file(&nested), "a directory is not an object file");
        assert!(!names.is_file(&short), "a short hex name is ordinary");
        assert!(!names.is_file(&plain));
        assert!(!names.is_file(&directory.path().join("missing")));
        assert!(!names.is_file(&directory.path().join("deadbeef")));
    }
}
