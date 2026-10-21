use std::error::Error;
use std::io;
use std::path::Path;
use std::process::Command;

use easy_fuser::fuse_parallel::prelude::*;
use log::{info, warn};

use crate::filesystem::AcFs;

/// Serves `fs` on `mountpoint` until the mount is unmounted.
///
/// `mount` blocks and returns once the mount is gone, whether the signal
/// handler or another process removed it.
pub fn run(fs: AcFs, mountpoint: &Path, options: &[MountOption]) -> Result<(), Box<dyn Error>> {
    install_signal_handler(mountpoint)?;
    mount(fs, mountpoint, options, None)?;
    Ok(())
}

/// Unmounting from the handler makes the blocking mount call return.
fn install_signal_handler(mountpoint: &Path) -> Result<(), ctrlc::Error> {
    let mountpoint = mountpoint.to_path_buf();
    ctrlc::set_handler(move || {
        info!("unmounting {}", mountpoint.display());
        if let Err(err) = unmount(&mountpoint) {
            warn!("cannot unmount {}: {err}", mountpoint.display());
        }
    })
}

fn unmount(mountpoint: &Path) -> io::Result<()> {
    let helpers: [(&str, &[&str]); 3] = [
        ("fusermount3", &["-u"]),
        ("fusermount", &["-u"]),
        ("umount", &[]),
    ];

    let mut last_error = None;
    for (program, args) in helpers {
        match Command::new(program).args(args).arg(mountpoint).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => last_error = Some(io::Error::other(format!("{program} exited {status}"))),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("no unmount helper available")))
}
