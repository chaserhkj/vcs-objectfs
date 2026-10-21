//! Object file identity: a regular file whose name is a lowercase hex string.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

/// Whether `name` is a non-empty lowercase hexadecimal string.
pub fn is_object_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Whether `path` is a regular file with an object file name.
///
/// Directories and symlinks are excluded, so a fanout directory named after a
/// hash prefix stays an ordinary directory.
pub fn is_object_file(path: &Path) -> bool {
    path.file_name().is_some_and(is_object_name)
        && fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_names_are_object_names() {
        for name in ["0", "deadbeef", "0123456789abcdef", "f"] {
            assert!(is_object_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn other_names_are_not_object_names() {
        for name in [
            "",
            "DEADBEEF",
            "DeadBeef",
            "dead beef",
            "deadbeef.txt",
            "pack-abc123.pack",
            "g",
            "-1",
        ] {
            assert!(!is_object_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn file_name_decides_identity() {
        let directory = tempfile::tempdir().unwrap();
        let object = directory.path().join("abc");
        let prefixed = directory.path().join("prefix");
        let plain = directory.path().join("notes.txt");
        let nested = directory.path().join("def");

        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&prefixed).unwrap();
        fs::write(&object, b"object").unwrap();
        fs::write(&plain, b"text").unwrap();
        fs::write(prefixed.join("abc"), b"object").unwrap();

        assert!(is_object_file(&object));
        assert!(is_object_file(&prefixed.join("abc")));
        assert!(
            !is_object_file(&nested),
            "a directory is not an object file"
        );
        assert!(!is_object_file(&plain));
        assert!(!is_object_file(&directory.path().join("missing")));
    }
}
