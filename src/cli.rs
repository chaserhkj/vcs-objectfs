use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;

/// Command line arguments for the access-control filesystem.
#[derive(Debug, Parser)]
#[command(
    name = "vcs-objectfs",
    version,
    about = "Expose a VCS object directory as append-only storage"
)]
pub struct Args {
    /// Directory whose contents are served through the mount point
    pub source: PathBuf,

    /// Directory the filesystem is mounted on; created when missing
    pub mountpoint: PathBuf,

    /// Mount the filesystem read-only
    #[arg(short, long)]
    pub read_only: bool,

    /// Additional FUSE mount option, repeatable
    #[arg(short = 'o', long = "option", value_name = "OPTION")]
    pub mount_options: Vec<String>,

    /// Increase log verbosity; repeat for trace level
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl Args {
    /// Canonical path of the backing directory, which must exist.
    pub fn resolve_source(&self) -> Result<PathBuf, Box<dyn Error>> {
        let source = fs::canonicalize(&self.source)
            .map_err(|err| format!("cannot resolve source {}: {err}", self.source.display()))?;
        if !source.is_dir() {
            return Err(format!("source {} is not a directory", source.display()).into());
        }
        Ok(source)
    }

    /// Canonical mount point path, created when it does not exist.
    pub fn resolve_mountpoint(&self, source: &Path) -> Result<PathBuf, Box<dyn Error>> {
        if !self.mountpoint.exists() {
            fs::create_dir_all(&self.mountpoint)?;
        }
        let mountpoint = fs::canonicalize(&self.mountpoint).map_err(|err| {
            format!(
                "cannot resolve mount point {}: {err}",
                self.mountpoint.display()
            )
        })?;
        if mountpoint.starts_with(source) {
            return Err(format!(
                "mount point {} lies inside source {}",
                mountpoint.display(),
                source.display()
            )
            .into());
        }
        if source.starts_with(&mountpoint) {
            return Err(format!(
                "source {} lies inside mount point {}",
                source.display(),
                mountpoint.display()
            )
            .into());
        }
        Ok(mountpoint)
    }
}
