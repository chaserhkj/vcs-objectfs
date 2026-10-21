//! Running the mount in a detached background process.

use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::Duration;

use crate::mount;

/// How long the parent waits for the child to establish the mount.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Byte the child writes through the startup pipe once the mount exists.
const MOUNTED: u8 = b'+';

/// Byte that introduces an error message through the startup pipe.
const FAILED: u8 = b'!';

/// Reports the mount's startup outcome to the waiting parent, if there is one.
///
/// The startup pipe stays open until [`Readiness::ready`] or until the child
/// writes a failure, so the parent never mistakes a crashed child for a mount.
pub struct Readiness {
    /// Startup pipe of the forked child; absent when mounting in the foreground.
    pipe: Option<File>,
}

impl Readiness {
    fn new(writer: File) -> Self {
        Self { pipe: Some(writer) }
    }

    /// Reports readiness for a mount that keeps the invoking terminal.
    pub fn foreground() -> Self {
        Self { pipe: None }
    }

    /// Announces the established mount and, in the background, releases the terminal.
    ///
    /// The parent returns once this call has written its byte, so standard
    /// streams are redirected afterwards: nothing can depend on them later.
    pub fn ready(&mut self) {
        let Some(mut writer) = self.pipe.take() else {
            return;
        };
        let _ = writer.write_all(&[MOUNTED]);
        redirect_standard_streams();
    }

    fn report(&mut self, error: &dyn std::fmt::Display) {
        if let Some(mut writer) = self.pipe.take() {
            let _ = write!(writer, "{}{error}", FAILED as char);
        }
    }
}

/// Serves the filesystem in a forked child and returns once `mountpoint` is mounted.
///
/// The child detaches from the terminal and outlives the parent. A startup
/// failure reaches the parent as its own error, so the exit status still
/// reports mount problems.
pub fn spawn<F>(mountpoint: &Path, serve: F) -> Result<(), Box<dyn Error>>
where
    F: FnOnce(&mut Readiness) -> Result<(), Box<dyn Error>>,
{
    let (reader, writer) = pipe()?;

    // SAFETY(fork): the mount path of the process is single-threaded here, and
    // the child only acts on its own copy of the process state.
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if child == 0 {
        run_child(reader, writer, serve);
    }

    drop(writer);
    wait_for_startup(reader, child, mountpoint)
}

/// Serves the mount in the forked child and never returns.
fn run_child<F>(reader: File, writer: File, serve: F) -> !
where
    F: FnOnce(&mut Readiness) -> Result<(), Box<dyn Error>>,
{
    drop(reader);
    // SAFETY(signal): SIG_IGN is a valid disposition for SIGPIPE, which keeps a
    // write to the closed startup pipe from terminating the daemon.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };

    let mut readiness = Readiness::new(writer);
    if let Err(error) = detach_session().and_then(|()| serve(&mut readiness)) {
        log::error!("{error}");
        readiness.report(&error);
        // SAFETY(exit): the forked child must skip the inherited atexit
        // handlers and destructors.
        unsafe { libc::_exit(1) }
    }
    // SAFETY(exit): as above.
    unsafe { libc::_exit(0) }
}

/// Starts a new session so the mount no longer belongs to the parent's terminal.
fn detach_session() -> Result<(), Box<dyn Error>> {
    // SAFETY(setsid): the forked child is not a session leader.
    if unsafe { libc::setsid() } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    std::env::set_current_dir("/")?;
    Ok(())
}

fn redirect_standard_streams() {
    let Ok(null) = File::options().read(true).write(true).open("/dev/null") else {
        return;
    };
    for descriptor in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY(dup2): both descriptors are valid and distinct.
        unsafe { libc::dup2(null.as_raw_fd(), descriptor) };
    }
}

fn pipe() -> io::Result<(File, File)> {
    let mut descriptors = [0; 2];
    // SAFETY(pipe): the array has room for both descriptors the call writes.
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY(from_raw_fd): both descriptors are fresh and owned by this process.
    let (reader, writer) = unsafe {
        (
            OwnedFd::from_raw_fd(descriptors[0]),
            OwnedFd::from_raw_fd(descriptors[1]),
        )
    };
    Ok((File::from(reader), File::from(writer)))
}

/// Waits for the child's mount to come up or for it to report a failure.
fn wait_for_startup(
    mut reader: File,
    child: libc::pid_t,
    mountpoint: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut descriptor = libc::pollfd {
        fd: reader.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY(poll): one initialized descriptor; the timeout is in milliseconds.
    let ready = unsafe { libc::poll(&mut descriptor, 1, STARTUP_TIMEOUT.as_millis() as i32) };
    if ready < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if ready == 0 {
        stop_child(child, mountpoint);
        return Err(format!(
            "{} did not mount within {} seconds",
            mountpoint.display(),
            STARTUP_TIMEOUT.as_secs()
        )
        .into());
    }

    let mut outcome = [0u8];
    if reader.read_exact(&mut outcome).is_err() {
        reap(child);
        return Err("background mount exited before it was ready".into());
    }
    if outcome[0] == MOUNTED {
        return Ok(());
    }

    let mut message = String::new();
    let _ = reader.read_to_string(&mut message);
    reap(child);
    let message = message.trim();
    if message.is_empty() {
        return Err("background mount failed".into());
    }
    Err(message.to_string().into())
}

/// Kills and reaps a child that never mounted, then clears any partial mount.
fn stop_child(child: libc::pid_t, mountpoint: &Path) {
    // SAFETY(kill): the child is the process created by fork above.
    unsafe { libc::kill(child, libc::SIGKILL) };
    reap(child);
    let _ = mount::unmount(mountpoint);
}

fn reap(child: libc::pid_t) {
    let mut status = 0;
    // SAFETY(waitpid): the child is the process created by fork above.
    unsafe { libc::waitpid(child, &mut status, 0) };
}
