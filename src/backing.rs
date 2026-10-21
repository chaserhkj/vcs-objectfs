use std::error::Error;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// Open handle to the directory an in-place mount covers.
///
/// Mounting over a directory hides its path, so the handler reaches the
/// underlying directory through this descriptor: `/proc/self/fd/N` keeps
/// naming the mounted-over directory.
pub struct Backing {
    _directory: File,
    path: PathBuf,
}

impl Backing {
    /// Opens `directory` and records the descriptor path that names it.
    pub fn open(directory: &Path) -> Result<Self, Box<dyn Error>> {
        let directory = File::open(directory)
            .map_err(|err| format!("cannot open {}: {err}", directory.display()))?;
        let path = PathBuf::from("/proc/self/fd").join(directory.as_raw_fd().to_string());
        if !fs::metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            return Err(format!(
                "{} does not name a directory; mount procfs at /proc",
                path.display()
            )
            .into());
        }
        Ok(Self {
            _directory: directory,
            path,
        })
    }

    /// Path the filesystem handler joins request paths to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}
