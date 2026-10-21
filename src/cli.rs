use std::error::Error;
use std::fs;
use std::path::PathBuf;

use clap::Parser;

/// Command line arguments for the object file filesystem.
#[derive(Debug, Parser)]
#[command(
    name = "vcs-objectfs",
    version,
    about = "Expose a VCS object directory with immutable object files"
)]
pub struct Args {
    /// Directory to serve and mount on; it stays the same path after mounting
    pub path: PathBuf,

    /// Additional FUSE mount option, repeatable
    #[arg(short = 'o', long = "option", value_name = "OPTION")]
    pub mount_options: Vec<String>,

    /// Increase log verbosity; repeat for trace level
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl Args {
    /// Canonical path of the directory, which must exist.
    pub fn resolve_path(&self) -> Result<PathBuf, Box<dyn Error>> {
        let path = fs::canonicalize(&self.path)
            .map_err(|err| format!("cannot resolve {}: {err}", self.path.display()))?;
        if !path.is_dir() {
            return Err(format!("{} is not a directory", path.display()).into());
        }
        Ok(path)
    }
}
