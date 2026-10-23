//! Mounts the filesystem and checks the object file rules from the kernel side,
//! and that git and jj work with their object storage mounted.
//!
//! Skipped when FUSE is unavailable: the test needs `/dev/fuse`, `fusermount3`,
//! and a `mount` that can be written to. The git and jj tests are skipped when
//! the respective binary is missing from `PATH`; the jj test also needs git,
//! whose object store is the one it mounts.

use std::error::Error;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use tempfile::TempDir;

type Outcome = Result<(), Box<dyn Error>>;

struct Mount {
    directory: PathBuf,
    daemon: Child,
    /// Whether the mount created the directory, which it removes on drop.
    owned: bool,
}

/// Distinguishes the mount directories of mounts running side by side.
static MOUNTS: AtomicUsize = AtomicUsize::new(0);

impl Mount {
    fn start() -> Result<Self, Box<dyn Error>> {
        Self::start_with(&[])
    }

    fn start_with(arguments: &[&str]) -> Result<Self, Box<dyn Error>> {
        let serial = MOUNTS.fetch_add(1, Ordering::Relaxed);
        let directory = fs::canonicalize(std::env::temp_dir())?
            .join(format!("vcs-objectfs-e2e-{}-{serial}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory)?;
        let mut mount = Self::start_at(directory, arguments)?;
        mount.owned = true;
        Ok(mount)
    }

    /// Mounts an existing `directory`, which outlives the mount.
    ///
    /// The daemon stays in the foreground so this process owns its lifetime
    /// and can unmount it on drop.
    fn start_at(directory: PathBuf, arguments: &[&str]) -> Result<Self, Box<dyn Error>> {
        if !directory.is_dir() {
            return Err(format!("{} is not a directory", directory.display()).into());
        }
        let daemon = Command::new(env!("CARGO_BIN_EXE_vcs-objectfs"))
            .arg("--foreground")
            .args(arguments)
            .arg(&directory)
            .spawn()?;
        let mount = Self {
            directory,
            daemon,
            owned: false,
        };
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
        if self.owned {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

fn is_mounted(directory: &Path) -> io::Result<bool> {
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    let directory = directory.to_string_lossy();
    Ok(mounts.lines().any(|line| {
        let mut fields = line.split(' ');
        let mount_point = fields.nth(4);
        line.contains(" - fuse ") && mount_point == Some(&*directory)
    }))
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

/// A mount left behind by the background daemon, unmounted on drop.
struct BackgroundMount {
    directory: PathBuf,
}

impl Drop for BackgroundMount {
    fn drop(&mut self) {
        let _ = Command::new("fusermount3")
            .args(["-u"])
            .arg(&self.directory)
            .status();
        if self.directory.exists() && is_mounted(&self.directory).unwrap_or(false) {
            eprintln!("warning: {} is still mounted", self.directory.display());
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn default_run_detaches_and_returns() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }

    let serial = MOUNTS.fetch_add(1, Ordering::Relaxed);
    let directory = fs::canonicalize(std::env::temp_dir())?
        .join(format!("vcs-objectfs-e2e-{}-{serial}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory)?;

    // Without --foreground the command returns on its own once the mount is up.
    let output = Command::new(env!("CARGO_BIN_EXE_vcs-objectfs"))
        .arg(&directory)
        .output()?;
    assert!(output.status.success(), "the command exits successfully");

    let mount = BackgroundMount { directory };
    assert!(
        is_mounted(&mount.directory)?,
        "the mount outlives the command"
    );
    fs::write(mount.directory.join("scratch"), b"through the mount")?;
    assert_eq!(
        fs::read(mount.directory.join("scratch"))?,
        b"through the mount",
        "the detached daemon serves requests"
    );
    Ok(())
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
    let object = mount.path("deadbeefdeadbeef");
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
        denied(fs::write(mount.path("cafebabecafebabe"), b"new")),
        "create under an object name"
    );
    assert!(
        !mount.path("cafebabecafebabe").exists(),
        "no object was created"
    );

    // A short hex name is an ordinary, writable file.
    let short = mount.path("deadbeef");
    fs::write(&short, b"short")?;
    fs::write(&short, b"still short")?;
    fs::remove_file(&short)?;

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
    fs::rename(&nested, mount.path("ab/deadbeefdeadbeef"))?;
    assert!(
        denied(fs::remove_file(mount.path("ab/deadbeefdeadbeef"))),
        "nested unlink"
    );
    assert!(
        fs::remove_dir(mount.path("ab")).is_err(),
        "a non-empty directory stays"
    );
    Ok(())
}

#[test]
fn zero_min_length_makes_every_hex_name_an_object() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    let mount = Mount::start_with(&["--min-object-name-length", "0"])?;

    let object = mount.path("deadbeef");
    fs::write(mount.path("scratch"), b"any hex name")?;
    fs::rename(mount.path("scratch"), &object)?;
    assert_eq!(fs::read(&object)?, b"any hex name", "read");
    assert!(denied(fs::write(&object, b"changed")), "write");
    assert!(denied(fs::remove_file(&object)), "unlink");
    assert!(
        denied(fs::write(mount.path("ab"), b"new")),
        "create under a hex name"
    );

    // A single hex digit is enough to name an object.
    fs::write(mount.path("single"), b"one digit")?;
    fs::rename(mount.path("single"), mount.path("0"))?;
    assert!(
        denied(fs::write(mount.path("0"), b"changed")),
        "single digit"
    );

    let plain = mount.path("notes.txt");
    fs::write(&plain, b"text")?;
    fs::write(&plain, b"still writable")?;
    fs::remove_file(&plain)?;
    Ok(())
}

#[test]
fn excluded_patterns_stay_ordinary_files() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    let mount = Mount::start_with(&[
        "--exclude",
        "excluded/*",
        "--exclude",
        "**/*.tmp",
        "--exclude",
        "0123456789abcdef",
    ])?;
    fs::create_dir(mount.path("excluded"))?;

    // A hex-named file below an excluded directory is an ordinary file: it can
    // be created, written, renamed and removed.
    let object = mount.path("excluded/deadbeefdeadbeef");
    fs::write(&object, b"one")?;
    fs::write(&object, b"two")?;
    assert_eq!(fs::read(&object)?, b"two", "the content follows writes");
    fs::rename(&object, mount.path("excluded/moved"))?;
    fs::remove_file(mount.path("excluded/moved"))?;

    // A nested pattern applies at any depth.
    let nested = mount.path("ab/deadbeefdeadbeef.tmp");
    fs::create_dir(mount.path("ab"))?;
    fs::write(&nested, b"tmp")?;
    fs::write(&nested, b"tmp again")?;
    fs::remove_file(&nested)?;

    // An excluded name at the top of the source directory stays ordinary even
    // though it is hash-shaped.
    let top = mount.path("0123456789abcdef");
    fs::write(&top, b"ordinary")?;
    fs::write(&top, b"still ordinary")?;
    fs::rename(&top, mount.path("top-moved"))?;

    // Everything the patterns do not match keeps the object rule.
    let object = mount.path("cafebabecafebabe");
    fs::write(mount.path("scratch"), b"commit")?;
    fs::rename(mount.path("scratch"), &object)?;
    assert!(denied(fs::write(&object, b"changed")), "write");
    assert!(denied(fs::remove_file(&object)), "unlink");
    assert!(
        denied(fs::write(mount.path("feedfacefeedface"), b"new")),
        "create under an object name"
    );

    // An excluded path accepts a new file under a hash-shaped name.
    fs::write(mount.path("excluded/cafebabecafebabe"), b"new")?;
    fs::remove_file(mount.path("excluded/cafebabecafebabe"))?;
    Ok(())
}

#[test]
fn an_invalid_exclude_pattern_fails_to_start() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    let root = scratch_directory("vcs-objectfs-exclude-")?;
    let output = Command::new(env!("CARGO_BIN_EXE_vcs-objectfs"))
        .args([
            "--foreground",
            "--exclude",
            "[ab",
            root.path().to_str().unwrap(),
        ])
        .output()?;
    assert!(!output.status.success(), "the daemon exits non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[ab"),
        "the error names the pattern: {stderr}"
    );
    assert!(!is_mounted(root.path())?, "nothing was mounted");
    Ok(())
}

/// An executable found in `PATH`.
fn command_available(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| {
            fs::metadata(directory.join(name))
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    })
}

/// Runs `command` in `directory` and fails with both output streams when it exits non-zero.
fn run(directory: &Path, command: &mut Command) -> Result<String, Box<dyn Error>> {
    let output = command.current_dir(directory).output()?;
    if !output.status.success() {
        return Err(format!(
            "{} failed with {}: {}{}",
            command.get_program().to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A scratch directory in the resolved temporary directory, where a mount is reachable
/// under the path it was given.
fn scratch_directory(prefix: &str) -> Result<TempDir, Box<dyn Error>> {
    Ok(tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(fs::canonicalize(std::env::temp_dir())?)?)
}

/// Git that takes its configuration from the arguments and the repository only.
fn git(directory: &Path, arguments: &[&str]) -> Result<String, Box<dyn Error>> {
    run(
        directory,
        Command::new("git")
            .args(["-c", "user.name=e2e", "-c", "user.email=e2e@example.com"])
            .args(["-c", "init.defaultBranch=main"])
            .args(["-c", "protocol.file.allow=always"])
            .args(arguments)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE"),
    )
}

/// Subjects of all commits, oldest first.
fn git_subjects(directory: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let log = git(directory, &["log", "--all", "--reverse", "--format=%s"])?;
    Ok(log.lines().map(str::to_owned).collect())
}

/// Appends `content` to a file in the working tree and commits it.
fn git_commit(
    directory: &Path,
    file: &str,
    content: &str,
    message: &str,
) -> Result<(), Box<dyn Error>> {
    let path = directory.join(file);
    let mut bytes = fs::read(&path).unwrap_or_default();
    bytes.extend_from_slice(content.as_bytes());
    fs::write(&path, bytes)?;
    git(directory, &["add", file])?;
    git(directory, &["commit", "-q", "-m", message])?;
    Ok(())
}

/// An empty repository whose object store is mounted, with its work tree beside the mount.
fn mounted_git_repository(prefix: &str) -> Result<(TempDir, Mount, PathBuf), Box<dyn Error>> {
    let root = scratch_directory(prefix)?;
    let repo = root.path().join("repo");
    fs::create_dir_all(&repo)?;
    git(&repo, &["init", "-q"])?;
    let objects = repo.join(".git/objects");
    let mount = Mount::start_at(objects.clone(), &[])?;
    assert!(is_mounted(&objects)?, "the object store is mounted");
    assert!(
        !is_mounted(&repo)?,
        "the rest of the repository stays unmounted"
    );
    Ok((root, mount, repo))
}

/// A loose object of a Git object store, which Git names after its hash.
fn loose_object(objects: &Path) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let fanout = fs::read_dir(objects)?
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().len() == 2);
    let Some(fanout) = fanout else {
        return Ok(None);
    };
    Ok(fs::read_dir(fanout.path())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .next())
}

#[test]
fn git_commits_through_the_mount() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    if !command_available("git") {
        eprintln!("skipping: git is unavailable");
        return Ok(());
    }

    let (_root, _mount, repo) = mounted_git_repository("vcs-objectfs-git-")?;
    git_commit(&repo, "a.txt", "one\n", "one")?;
    git_commit(&repo, "a.txt", "two\n", "two")?;
    git_commit(&repo, "b.txt", "side\n", "side")?;

    assert_eq!(
        git_subjects(&repo)?,
        ["one", "two", "side"],
        "commits are written and read back"
    );
    git(&repo, &["fsck", "--strict", "--no-dangling"])?;

    // Git commits an object by renaming a temporary file to its hash, so the
    // resulting objects are immutable.
    let object = loose_object(&repo.join(".git/objects"))?.ok_or("no loose object was written")?;
    assert!(
        denied(fs::remove_file(&object)),
        "an object cannot be removed"
    );
    assert!(
        denied(fs::OpenOptions::new().write(true).open(&object).map(drop)),
        "an object cannot be opened for writing"
    );

    // Git writes temporary files and directories beside the objects while it
    // commits, which the mount leaves writable.
    fs::write(repo.join("c.txt"), b"more\n")?;
    git(&repo, &["add", "c.txt"])?;
    git(&repo, &["stash", "-q"])?;
    assert!(!repo.join("c.txt").exists(), "the stash takes the change");
    git(&repo, &["stash", "pop", "-q"])?;
    git(&repo, &["commit", "-q", "-m", "more"])?;
    assert_eq!(
        git_subjects(&repo)?,
        ["one", "two", "side", "more"],
        "commits continue beside existing objects"
    );

    // A push into the mounted store asks the receiving side to rename loose
    // objects into place, which lends them the object name and is refused. The
    // push reports the rejection and leaves the store as it was.
    let clone_root = scratch_directory("vcs-objectfs-git-clone-")?;
    let clone = clone_root.path().join("clone");
    git(
        &repo,
        &[
            "clone",
            "-q",
            repo.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    )?;
    let before_push = git_subjects(&repo)?;
    fs::write(clone.join("d.txt"), b"pushed\n")?;
    git(&clone, &["add", "d.txt"])?;
    git(&clone, &["commit", "-q", "-m", "pushed"])?;
    git(&clone, &["push", "-q", "origin", "main"])
        .expect_err("a push cannot commit into a mounted store");
    assert_eq!(
        git_subjects(&repo)?,
        before_push,
        "a rejected push changes nothing"
    );
    Ok(())
}

#[test]
fn git_collects_garbage_through_the_mount() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    if !command_available("git") {
        eprintln!("skipping: git is unavailable");
        return Ok(());
    }

    let (_root, _mount, repo) = mounted_git_repository("vcs-objectfs-git-gc-")?;
    git_commit(&repo, "a.txt", "one\n", "one")?;
    git_commit(&repo, "a.txt", "two\n", "two")?;
    git_commit(&repo, "b.txt", "three\n", "three")?;

    // A clone reads every object through the mount.
    let clone_root = scratch_directory("vcs-objectfs-git-gc-clone-")?;
    let clone = clone_root.path().join("clone");
    git(
        &repo,
        &[
            "clone",
            "-q",
            repo.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    )?;
    assert_eq!(
        git_subjects(&clone)?,
        ["one", "two", "three"],
        "a clone reads the history"
    );
    git(&clone, &["fsck", "--strict", "--no-dangling"])?;

    // Packing reads loose objects and writes a pack, so the repository keeps
    // working while the loose objects it packed stay behind write-protected.
    git(&repo, &["gc", "--quiet"])?;
    git(&repo, &["fsck", "--strict", "--no-dangling"])?;
    assert_eq!(
        git_subjects(&repo)?,
        ["one", "two", "three"],
        "the history survives a collection"
    );
    git_commit(&repo, "b.txt", "four\n", "four")?;
    assert_eq!(
        git_subjects(&repo)?,
        ["one", "two", "three", "four"],
        "commits continue after a collection"
    );
    git(&repo, &["fsck", "--strict", "--no-dangling"])?;
    Ok(())
}

/// Jujutsu with its configuration taken from the environment only.
fn jj(directory: &Path, arguments: &[&str]) -> Result<String, Box<dyn Error>> {
    run(
        directory,
        Command::new("jj")
            .args(["--color", "never"])
            .args(arguments)
            .env("JJ_CONFIG", "/dev/null")
            .env("JJ_USER", "e2e")
            .env("JJ_EMAIL", "e2e@example.com"),
    )
}

/// Descriptions of the changes from the root commit to the working copy, oldest first.
fn jj_descriptions(directory: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let log = jj(
        directory,
        &[
            "log",
            "--no-graph",
            "--reversed",
            "-r",
            "::@",
            "-T",
            "description.first_line() ++ \"\\n\"",
        ],
    )?;
    Ok(log
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

#[test]
fn jj_commits_through_the_mount() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    if !command_available("jj") {
        eprintln!("skipping: jj is unavailable");
        return Ok(());
    }
    // The git backend is the object storage this mount serves: jj's own backend
    // writes immutable files in place, which the mount refuses.
    if !command_available("git") {
        eprintln!("skipping: the jj git backend needs git");
        return Ok(());
    }

    let root = scratch_directory("vcs-objectfs-jj-")?;
    let repo = root.path().join("repo");
    fs::create_dir_all(&repo)?;
    jj(&repo, &["git", "init"])?;
    let objects = repo.join(".git/objects");
    let mount = Mount::start_at(objects.clone(), &[])?;
    assert!(is_mounted(&objects)?, "the object store is mounted");

    fs::write(repo.join("a.txt"), b"one\n")?;
    jj(&repo, &["describe", "-m", "first change"])?;
    jj(&repo, &["new"])?;
    fs::write(repo.join("b.txt"), b"two\n")?;
    jj(&repo, &["describe", "-m", "second change"])?;

    assert_eq!(
        jj_descriptions(&repo)?,
        ["first change", "second change"],
        "changes are written and read back"
    );
    assert_eq!(
        jj(&repo, &["file", "show", "-r", "@-", "a.txt"])?,
        "one\n",
        "a file is read out of the store"
    );

    // A squash writes a rewritten tree; its objects are new and become immutable.
    jj(&repo, &["squash", "--into", "@-", "-m", "squashed"])?;
    assert_eq!(
        jj_descriptions(&repo)?,
        ["squashed"],
        "the squashed change keeps its place"
    );
    assert_eq!(
        jj(&repo, &["file", "show", "-r", "@-", "a.txt"])?,
        "one\n",
        "the squashed change carries the first file"
    );
    assert_eq!(
        jj(&repo, &["file", "show", "-r", "@-", "b.txt"])?,
        "two\n",
        "the squashed change carries the second file"
    );

    // Git reads the same objects, which stay a valid object database that packs.
    git(&repo, &["fsck", "--strict", "--no-dangling"])?;
    git(&repo, &["gc", "--quiet"])?;
    assert_eq!(
        jj_descriptions(&repo)?,
        ["squashed"],
        "jj reads the repository after git repacked it"
    );
    let _ = mount;
    Ok(())
}

/// Writes to the directory a mount covers, bypassing the mount.
///
/// The descriptor names the directory opened before the mount, so the path
/// reaches the files the mount mirrors rather than the mount itself.
struct Backing {
    /// Keeps the descriptor the path names open.
    _handle: fs::File,
    directory: PathBuf,
}

impl Backing {
    fn open(directory: &Path) -> io::Result<Self> {
        let handle = fs::File::open(directory)?;
        Ok(Self {
            directory: PathBuf::from("/proc/self/fd").join(handle.as_raw_fd().to_string()),
            _handle: handle,
        })
    }

    fn write(&self, relative: &str, content: &[u8]) -> io::Result<()> {
        fs::write(self.directory.join(relative), content)
    }
}

/// Reads `relative` through the mount until it reports `expected`, for up to `limit`.
fn await_content(path: &Path, expected: &[u8], limit: Duration) -> Result<String, Box<dyn Error>> {
    let deadline = Instant::now() + limit;
    let mut observed = Vec::new();
    while Instant::now() < deadline {
        observed = fs::read(path)?;
        if observed == expected {
            return Ok(String::from_utf8_lossy(&observed).into_owned());
        }
        sleep(Duration::from_millis(50));
    }
    Err(format!(
        "{} kept reporting {:?} for {limit:?}, expected {:?}",
        path.display(),
        String::from_utf8_lossy(&observed),
        String::from_utf8_lossy(expected)
    )
    .into())
}

/// Bytes the daemon has read from files so far.
///
/// The kernel serves a retained page itself, so the count stays put when the
/// mount keeps the page and grows by the file size when the daemon reads the
/// backing file again.
fn daemon_reads(daemon: &Child) -> io::Result<u64> {
    let io = fs::read_to_string(format!("/proc/{}/io", daemon.id()))?;
    let entry = io
        .lines()
        .find_map(|line| line.strip_prefix("rchar: "))
        .ok_or_else(|| io::Error::other("no rchar entry"))?;
    entry
        .trim()
        .parse()
        .map_err(|err| io::Error::other(format!("cannot parse rchar: {err}")))
}

/// Reads `path` twice and reports the daemon reads the second read caused.
fn repeat_read_cost(mount: &Mount, path: &Path) -> Result<Vec<u64>, Box<dyn Error>> {
    let mut costs = Vec::new();
    for _ in 0..4 {
        let before = daemon_reads(&mount.daemon)?;
        assert_eq!(fs::read(path)?.len(), 1 << 20, "a read of the file");
        costs.push(daemon_reads(&mount.daemon)? - before);
    }
    Ok(costs)
}

#[test]
fn cache_all_serves_a_repeat_read_from_the_page_cache() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    for (arguments, cached) in [
        (&[][..], false),
        (&["--cache", "objects"][..], false),
        (&["--cache", "all"][..], true),
    ] {
        let mount = Mount::start_with(arguments)?;
        fs::write(mount.path("notes.txt"), vec![b'a'; 1 << 20])?;
        let costs = repeat_read_cost(&mount, &mount.path("notes.txt"))?;
        drop(mount);
        eprintln!("{arguments:?}: {costs:?}");
        if cached {
            assert!(
                costs[3] < 1 << 16,
                "{arguments:?} serves the repeat read from the page cache: {costs:?}"
            );
        } else {
            assert!(
                costs[3] >= 1 << 20,
                "{arguments:?} rereads the backing file: {costs:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn cached_content_follows_changes_made_outside_the_mount() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    for arguments in [&["--cache", "all"][..], &["--cache", "objects"][..]] {
        let root = scratch_directory("vcs-objectfs-cache-")?;
        let directory = fs::canonicalize(root.path())?;
        fs::write(directory.join("notes.txt"), b"first")?;
        let backing = Backing::open(&directory)?;
        let mount = Mount::start_at(directory, arguments)?;

        // Warm the page cache, then change the file behind the mount's back.
        let notes = mount.path("notes.txt");
        assert_eq!(fs::read(&notes)?, b"first", "the initial content");
        assert_eq!(fs::read(&notes)?, b"first", "the cached content");
        backing.write("notes.txt", b"second")?;

        // `--cache all` revalidates within one attribute TTL, `--cache objects`
        // never does for an ordinary file, so both must show the new content
        // within a few seconds at most.
        let observed = await_content(&notes, b"second", Duration::from_secs(5))?;
        assert_eq!(observed, "second", "an outside change is not hidden");
        drop(mount);
    }
    Ok(())
}

#[test]
fn cache_none_shows_an_outside_change_immediately() -> Outcome {
    if !fuse_available() {
        eprintln!("skipping: FUSE is unavailable");
        return Ok(());
    }
    let root = scratch_directory("vcs-objectfs-cache-")?;
    let directory = fs::canonicalize(root.path())?;
    fs::write(directory.join("notes.txt"), b"first")?;
    let backing = Backing::open(&directory)?;
    let mount = Mount::start_at(directory, &[])?;

    let notes = mount.path("notes.txt");
    assert_eq!(fs::read(&notes)?, b"first", "the initial content");
    backing.write("notes.txt", b"second")?;
    assert_eq!(
        fs::read(&notes)?,
        b"second",
        "the change is visible at once"
    );
    drop(mount);
    Ok(())
}
