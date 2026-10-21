# vcs-objectfs

A FUSE filesystem that enforces access rules on a VCS object directory, giving
object storage **write-only, append-only** semantics: writers can add objects,
readers of the mount see nothing back, and no existing object can be modified
or removed.

Built on [`easy_fuser`](https://github.com/Alogani/easy_fuser).

## Why

VCS object directories (`objects/` in Git, the object store of other systems)
are content-addressed and immutable: an object is written once, then only read
or replicated. Serving them through FUSE lets a host enforce that contract
instead of trusting every tool that writes there:

- new objects may be created, existing ones never changed or deleted;
- clients that only need to push cannot read what they pushed;
- pack files can grow by appending, but not be rewritten in place.

## Build

Requires a Rust toolchain and the FUSE 3 runtime (`/dev/fuse`, `fusermount3`).

```sh
cargo build --release
```

## Run

```sh
vcs-objectfs <SOURCE> <MOUNTPOINT>
```

`MOUNTPOINT` is created when missing and must lie outside `SOURCE`.

| Option            | Effect                              |
| ----------------- | ----------------------------------- |
| `-r`, `--read-only` | Mount read-only                   |
| `-o`, `--option`  | Extra FUSE mount option, repeatable |
| `-v`, `--verbose` | Debug logging; repeat for trace     |

Unmount with `fusermount3 -u <MOUNTPOINT>`, or with Ctrl-C / SIGTERM.

## Layout

| File                | Role                                                |
| ------------------- | --------------------------------------------------- |
| `src/main.rs`       | Argument handling, mount options, process lifecycle |
| `src/cli.rs`        | Command line definition and path validation         |
| `src/filesystem.rs` | `AcFs` FUSE handler and its operation routing       |
| `src/mount.rs`      | Mount call and unmount on termination signals       |
| `AGENT.md`          | Access rule set and implementation rules            |

`AcFs` delegates every operation to an `easy_fuser` preset. A rule is added by
implementing that operation on `AcFs` and removing it from the matching
`delegate_fs!` list.
