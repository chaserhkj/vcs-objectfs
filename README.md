# vcs-objectfs

A FUSE filesystem that treats VCS object files as immutable and passes every
other file through unchanged.

Built on [`easy_fuser`](https://github.com/Alogani/easy_fuser).

## Why

A VCS object directory (`objects/` in Git, the object store of other systems) is
content-addressed: an object's name is a hash of its content, so an object that is
written once is never modified, renamed or deleted afterwards. Tools obey that
contract by convention; a mount can enforce it.

The rule is also how an object comes into being. Writers create a file under a
temporary name, write it, then rename it to its hash. The rename is the commit
step: the file keeps its bytes and becomes read-only from that moment on.

## Design

An **object file** is a regular file whose name is a lowercase hexadecimal
string at least `--min-object-name-length` characters long — `[0-9a-f]{16,}` by
default, the hash; a length of 0 accepts `[0-9a-f]+`. A shorter hex name is an
**ordinary file**. Everything else is an ordinary file too.

| Entry                              | Behavior                            |
| ---------------------------------- | ----------------------------------- |
| Object file                        | Immutable                           |
| Directory, symlink, other file     | Same as the backing directory       |

Object files are immutable in the sense that all of these fail with `EPERM`:

- `unlink` and `rename` of an object file, and renaming anything onto it;
- `open` for writing, `write`, `truncate`, `fallocate`, `copy_file_range` into
  one;
- `chmod`, `chown`, `utimes` and every other `setattr`;
- `setxattr` and `removexattr`;
- `create` and `mknod` under an object file name, whether or not a file already
  has that name.

`access` reports an object file as not writable, so permission checks agree with
the operation results.

Renaming a file to an object file name commits it. Before the rename the file is
ordinary and fully writable; after it, no operation can change its content,
metadata or name. A commit never overwrites an object: a rename whose destination
is an existing object file fails with `EEXIST`. `renameat2(2)` flags reach the
backing rename unchanged.

A directory is never an object file, even when its name is hex — Git's fanout
directories (`objects/ab/`) are ordinary directories whose entries may each be
objects.

## Build

Requires a Rust toolchain and the FUSE 3 runtime (`/dev/fuse`, `fusermount3`).

```sh
cargo build --release
```

## Test

End-to-end tests mount the filesystem, so they run in a private mount namespace:

```sh
unshare -rm timeout 60s cargo test --test e2e
```

`tests/e2e.rs` checks the object rules from the kernel side and drives git and jj
against a mounted object store. A test that needs a tool missing from `PATH` is
skipped with a note on stderr.

## Run

```sh
vcs-objectfs <PATH>
```

`PATH` is served and mounted over in place: after the mount, that same path shows
the filesystem. The backing directory is reached through an open handle on
`/proc/self/fd`, so `/proc` must be mounted.

| Option                             | Effect                                        |
| ---------------------------------- | --------------------------------------------- |
| `-o`, `--option`                   | Extra FUSE mount option, repeatable           |
| `-m`, `--min-object-name-length <LEN>` | Shortest hex name treated as an object; 16, 0 for any |
| `-v`, `--verbose`                  | Debug logging; repeat for trace               |

Unmount with `fusermount3 -u <PATH>`, or with Ctrl-C / SIGTERM.

## Layout

| File                | Role                                                |
| ------------------- | --------------------------------------------------- |
| `src/main.rs`       | Argument handling, mount options, process lifecycle |
| `src/backing.rs`    | Open handle to the directory the mount covers       |
| `src/cli.rs`        | Command line definition and path validation         |
| `src/filesystem.rs` | `AcFs` FUSE handler and the immutability rules      |
| `src/mount.rs`      | Mount call and unmount on termination signals       |
| `src/object.rs`     | Object file name and identity                       |
| `AGENTS.md`         | Access rule set and implementation rules            |

`AcFs` delegates every operation without a rule to an `easy_fuser` preset. A rule
is added by implementing that operation on `AcFs` and removing it from the
matching `delegate_fs!` list.
