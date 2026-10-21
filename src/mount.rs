use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use easy_fuser::fuse_parallel::prelude::*;
use log::{info, warn};

use crate::filesystem::AcFs;

/// How long to wait for the kernel to list the new mount.
const MOUNT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait between mount table reads.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Serves `fs` on `mountpoint` until the mount is unmounted.
///
/// `ready` runs once the mount is established. Serving continues until the mount
/// is gone, whether the signal handler or another process removed it.
pub fn run(
    fs: AcFs,
    mountpoint: &Path,
    options: &[MountOption],
    ready: impl FnOnce(),
) -> Result<(), Box<dyn Error>> {
    install_signal_handler(mountpoint)?;

    // The mount call blocks until unmount, so it runs alongside the readiness
    // check rather than before it.
    let options = options.to_vec();
    let mountpoint_owned = mountpoint.to_path_buf();
    let serving = std::thread::spawn(move || {
        mount(fs, mountpoint_owned, &options, None)?;
        Ok::<(), io::Error>(())
    });

    wait_until_mounted(mountpoint, &serving)?;
    info!("mounted {}", mountpoint.display());
    ready();

    serving.join().map_err(|_| "mount thread panicked")??;
    info!("unmounted {}", mountpoint.display());
    Ok(())
}

/// Waits for the mount to appear, failing early when the mount call itself fails.
fn wait_until_mounted(
    mountpoint: &Path,
    serving: &std::thread::JoinHandle<io::Result<()>>,
) -> Result<(), Box<dyn Error>> {
    let mut waited = Duration::ZERO;
    while waited < MOUNT_TIMEOUT {
        if is_mounted(mountpoint)? {
            return Ok(());
        }
        if serving.is_finished() {
            return Err("mount call ended before the mount appeared".into());
        }
        sleep(POLL_INTERVAL);
        waited += POLL_INTERVAL;
    }
    Err(format!(
        "{} did not mount within {} seconds",
        mountpoint.display(),
        MOUNT_TIMEOUT.as_secs()
    )
    .into())
}

fn is_mounted(mountpoint: &Path) -> io::Result<bool> {
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    let mountpoint = mountpoint.to_string_lossy();
    Ok(mounts.lines().any(|line| {
        let mut fields = line.split(' ');
        let path = fields.nth(4);
        line.contains(" - fuse ") && path == Some(&*mountpoint)
    }))
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

pub fn unmount(mountpoint: &Path) -> io::Result<()> {
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
