use std::error::Error;
use std::fs;
use std::path::PathBuf;

use clap::Parser;

use crate::object::{DEFAULT_MIN_NAME_LENGTH, ObjectNameDef};

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

    /// Shortest lowercase hex file name recognized as an object, 0 to recognize any length
    #[arg(long, value_name = "LEN", default_value_t = DEFAULT_MIN_NAME_LENGTH)]
    pub min_object_name_length: usize,

    /// Increase log verbosity; repeat for trace level
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl Args {
    /// Object name recognition rule.
    pub fn object_name_def(&self) -> ObjectNameDef {
        ObjectNameDef::new(self.min_object_name_length)
    }

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
