# AGENTS.md

Guidelines for working on `vcs-objectfs`, a FUSE filesystem built on
`easy_fuser`. See `README.md` for the project's purpose and layout.

## End-to-end tests

**Always run end-to-end tests under a timeout**, e.g. `timeout 60s cargo test --test e2e` or
`timeout 60s ./target/debug/vcs-objectfs <PATH>`. FUSE daemons hang for many
reasons — a wedged kernel channel, a missing `/dev/fuse`, an unmount that never completes, a
signal handler that deadlocks. A hung process can leave a stale mount and block the whole
session, so every test invocation gets a hard deadline.

Clean up after a timed-out run: `fusermount3 -u <PATH>` (fall back to `fusermount -u`, then
`umount`), and confirm the path is unmounted before retrying.

## Rust

- Target the toolchain in `Cargo.toml` (`rust-version`); edition 2024 idioms are expected.
- `cargo fmt` and `cargo clippy --all-targets` must be clean before a change is considered done.
- Propagate errors as `Result<_, Box<dyn Error>>` at the binary's outer layers, as the existing
  `main`/`cli` code does; use concrete error types inside library-ish modules.
- Log through the `log` crate with the existing verbosity mapping in `init_logging`; never print
  to stdout from the mount path, since diagnostics belong on stderr or in the log.
- Keep `unsafe` out unless a FUSE binding forces it, and document why where it appears.

## FUSE specifics

- Implement a rule by writing that operation on `AcFs` and removing it from the matching
  `delegate_fs!` list in `src/filesystem.rs`; leaving it listed silently discards the rule.
- FUSE callbacks run on parallel worker threads. Handler state must be `Send + Sync`; prefer
  stateless handlers and immutable data captured at construction over locks.
- Return precise `Errno` values. Access denials and missing entries must be distinguishable;
  never collapse failures into `ENOSYS`.
- Treat paths from the kernel as untrusted: join them to the backing descriptor path and reject
  traversal outside it.
- Assume callers are concurrent and adversarial, and that a request may be aborted at any point.
- An object file is a regular file named `[0-9a-f]+`. Every operation that could change its
  content, metadata, name or existence must reject it with `EPERM`, and every operation that
  could create one under that name must reject it as well. Directories are never object files,
  even with a hex name.

## Testing discipline

- Integration tests that touch `/dev/fuse` must be skippable when FUSE is unavailable; check for
  the device and `fusermount3` before mounting.
- Each mount/unmount pair in a test keeps one temp directory, mounted in place and removed on
  success and on failure.
- Never leave a mount active between test runs; a leaked mount makes later runs fail obscurely.
