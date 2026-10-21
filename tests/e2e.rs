//! Mounts the filesystem and checks the object file rules from the kernel side.
//!
//! Skipped when FUSE is unavailable: the test needs `/dev/fuse`, `fusermount3`,
//! and a `mount` that can be written to.

use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

type Outcome = Result<(), Box<dyn Error>>;

struct Mount {
    directory: PathBuf,
    daemon: Child,
}

impl Mount {
    fn start() -> Result<Self, Box<dyn Error>> {
        let directory = fs::canonicalize(std::env::temp_dir())?
            .join(format!("vcs-objectfs-e2e-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory)?;

        let daemon = Command::new(env!("CARGO_BIN_EXE_vcs-objectfs"))
            .arg(&directory)
            .spawn()?;
        let mount = Self { directory, daemon };
        mount.wait_until_mounted()?;
        Ok(mount)
    }

    fn wait_until_mounted(&self) -> Result<(), Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if is_mounted(&self.directory)? {
                return Ok(());
            }
            sleep(Duration::from_millis(50));
        }
        Err(format!("{} did not mount", self.directory.display()).into())
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.directory.join(relative)
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = Command::new("fusermount3")
            .args(["-u"])
            .arg(&self.directory)
            .status();
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn is_mounted(directory: &Path) -> io::Result<bool> {
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    let directory = directory.to_string_lossy();
    Ok(mounts
        .lines()
        .any(|line| line.contains(" - fuse ") && line.contains(&*directory)))
}

fn fuse_available() -> bool {
    Path::new("/dev/fuse").exists()
        && Command::new("fusermount3")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        && fs::metadata(std::env::temp_dir())
            .is_ok_and(|meta| meta.permissions().mode() & 0o200 != 0)
}

fn denied(outcome: io::Result<()>) -> bool {
    outcome.is_err_and(|err| err.kind() == io::ErrorKind::PermissionDenied)
}

#[test]
fn object_files_are_immutable() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    let mount = Mount::start()?;

    // An ordinary file is writable, renamable and removable.
    let scratch = mount.path("scratch");
    fs::write(&scratch, b"one")?;
    fs::write(&scratch, b"two")?;
    let renamed = mount.path("scratch-renamed");
    fs::rename(&scratch, &renamed)?;

    // Committing is a rename to a hash-shaped name.
    let object = mount.path("deadbeef");
    fs::rename(&renamed, &object)?;
    assert_eq!(fs::read(&object)?, b"two", "the commit keeps the content");
    assert!(fs::metadata(&object)?.is_file(), "the commit succeeds");

    // The object cannot be changed in any way.
    assert!(denied(fs::write(&object, b"three")), "write");
    assert!(
        denied(fs::OpenOptions::new().write(true).open(&object).map(drop)),
        "open for writing"
    );
    assert!(
        denied(fs::OpenOptions::new().append(true).open(&object).map(drop)),
        "open for appending"
    );
    assert!(
        denied(
            fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&object)
                .map(drop)
        ),
        "open with truncate"
    );
    assert!(denied(fs::remove_file(&object)), "unlink");
    assert!(
        denied(fs::rename(&object, mount.path("elsewhere"))),
        "rename"
    );
    assert!(
        denied(fs::set_permissions(
            &object,
            fs::Permissions::from_mode(0o600)
        )),
        "chmod"
    );
    assert!(
        matches!(
            fs::OpenOptions::new().create_new(true).write(true).open(&object),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists || err.kind() == io::ErrorKind::PermissionDenied
        ),
        "create over an object"
    );

    // A commit never replaces an existing object.
    let rival = mount.path("rival");
    fs::write(&rival, b"rival")?;
    assert!(
        fs::rename(&rival, &object).is_err(),
        "a commit keeps the first object"
    );
    assert_eq!(fs::read(&object)?, b"two", "the first object survives");
    assert!(rival.exists(), "the rejected file stays under its own name");

    // Creating under an object name directly is refused.
    assert!(
        denied(fs::write(mount.path("cafebabe"), b"new")),
        "create under an object name"
    );
    assert!(!mount.path("cafebabe").exists(), "no object was created");

    // Reading the object still works.
    assert_eq!(fs::read(&object)?, b"two", "read");

    // Everything else still mirrors the backing directory.
    let scratch = mount.path("scratch");
    fs::write(&scratch, b"still writable")?;
    fs::rename(&scratch, mount.path("moved"))?;
    fs::remove_file(mount.path("moved"))?;

    // A fanout directory with a hex name is an ordinary directory whose
    // entries can be committed.
    fs::create_dir(mount.path("ab"))?;
    assert!(mount.path("ab").is_dir(), "an ordinary directory");
    let nested = mount.path("ab/nested");
    fs::write(&nested, b"text")?;
    fs::rename(&nested, mount.path("ab/deadbeef"))?;
    assert!(
        denied(fs::remove_file(mount.path("ab/deadbeef"))),
        "nested unlink"
    );
    assert!(
        fs::remove_dir(mount.path("ab")).is_err(),
        "a non-empty directory stays"
    );
    Ok(())
}
