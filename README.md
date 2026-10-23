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

The motivation is a lightweight guardrail for an AI agent running from a VCS
repository. Such an agent holds its user's authority, so one that misbehaves, or
one that follows instructions injected through the content it reads, can issue
any write the user can. Against this mount, every committed object stays byte
for byte as it was, whatever the agent attempts. The guardrail covers VCS
operations alone: it adds a layer over a sandbox rather than replacing one, and
the sandbox still bounds the rest of the agent's reach.

## Design

An **object file** is a regular file whose name is a lowercase hexadecimal
string at least `--min-object-name-length` characters long — `[0-9a-f]{16,}` by
default, the hash; a length of 0 accepts `[0-9a-f]+`. A shorter hex name is an
**ordinary file**. Everything else is an ordinary file too.

`--exclude <PATTERN>` removes paths from that rule: a file whose path relative
to the source directory matches the glob is an ordinary file whatever its name.
The flag is repeatable and a match on any pattern excludes. `*` and `?` stop at
a `/` while `**` spans directories, so `--exclude 'info/*'` covers a
directory's entries, `--exclude '*.lock'` the top level only, and
`--exclude '**/*.tmp'` any depth. An invalid pattern stops startup with an
error naming it.

| Entry                              | Behavior                            |
| ---------------------------------- | ----------------------------------- |
| Object file                        | Immutable                           |
| Directory, symlink, other file     | Same as the backing directory       |
| Path matching `--exclude`          | Ordinary file, whatever its name    |

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

The flake builds the same binary with the toolchain and dependencies pinned in
`flake.lock`:

```sh
nix build        # ./result/bin/vcs-objectfs
nix develop      # cargo, clippy, rustfmt and fusermount3
```

`nix build` runs the unit tests only: the end-to-end tests need a mount
namespace, which the build sandbox does not provide.

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

The command returns once the mount is established and leaves the daemon running
in the background; a startup failure is reported on stderr with a non-zero exit
status. Pass `--foreground` to serve until unmount instead.

| Option                             | Effect                                        |
| ---------------------------------- | --------------------------------------------- |
| `-f`, `--foreground`               | Stay in the foreground instead of detaching   |
| `-o`, `--option`                   | Extra FUSE mount option, repeatable           |
| `-m`, `--min-object-name-length <LEN>` | Shortest hex name treated as an object; 16, 0 for any |
| `--cache <MODE>`                   | Content kept across opens: `objects`, `all`, `none` |
| `-e`, `--exclude <PATTERN>`        | Source-relative glob kept ordinary, repeatable |
| `-v`, `--verbose`                  | Debug logging; repeat for trace               |

Unmount with `fusermount3 -u <PATH>`. In the foreground, Ctrl-C / SIGTERM
unmount as well.

## Cache

Reading a file costs two copies: the daemon reads the backing file and hands the
bytes back through `/dev/fuse`. The kernel can keep those bytes in memory
instead and serve the next read itself, but only when the mount asks for it. The
ask carries a risk: if the backing file changes, the kernel may hand out the old
bytes. `--cache` picks how much to trade that risk for speed.

| `--cache` | Cached | Not cached | An outside change shows up |
| --------- | ------ | ---------- | -------------------------- |
| `objects` | object files | ordinary files | on the next read of an ordinary file, never for an object file |
| `all`     | everything | nothing | within one second |
| `none`    | nothing | everything | on the next read |

`objects` is the default. It caches only object files: their content never
changes after the commit, so the cached bytes can never be stale, and an
ordinary file is still read from the daemon every time. `all` caches everything,
and therefore asks the kernel to re-check a file's metadata once a second; when
that re-check sees the file changed, the kernel throws the cached bytes away and
reads again. One second is that re-check interval, so an outside change can go
unnoticed for up to one second. `none` caches nothing, so every read reaches the
daemon. Writes made through the mount update the cache directly and are visible
at once.

## Layout

| File                | Role                                                |
| ------------------- | --------------------------------------------------- |
| `src/main.rs`       | Argument handling, mount options, process lifecycle |
| `src/backing.rs`    | Open handle to the directory the mount covers       |
| `src/cli.rs`        | Command line definition and path validation         |
| `src/filesystem.rs` | `AcFs` FUSE handler, cache modes and immutability rules |
| `src/exclude.rs`    | Globs of source-relative paths kept ordinary        |
| `src/mount.rs`      | Mount call and unmount on termination signals       |
| `src/daemon.rs`     | `--foreground` switch, fork and startup handshake   |
| `src/object.rs`     | Object file name and identity                       |
| `nix/package.nix`   | Nix derivation for the binary                       |
| `flake.nix`         | flake-parts outputs: packages, dev shell, formatter |
| `AGENTS.md`         | Access rule set and implementation rules            |
| `PASSTHROUGH_PLAN.md` | What FUSE passthrough would take, and why it is deferred |

`AcFs` delegates every operation without a rule to an `easy_fuser` preset. A rule
is added by implementing that operation on `AcFs` and removing it from the
matching `delegate_fs!` list.

`AcFs::open` returns `FOPEN_KEEP_CACHE` for the files `--cache` retains, on top
of the preset's handle.
