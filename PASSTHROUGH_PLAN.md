# Passthrough plan

Not implemented. This note records what FUSE passthrough would take, measured on
`easy_fuser` 0.7.1 and `fuser` 0.18 against Linux 7.x.

## What passthrough buys

The kernel opens the backing file itself and serves reads from it, so the daemon
never sees the request. `--cache all` already serves a repeated buffered read
from the page cache at backing speed; the remaining gain is a cold or
`O_DIRECT` read, roughly 2x. Metadata operations — `lookup`, `getattr`,
`readdir` — are untouched by passthrough and those dominate a VCS workload.
Passthrough also bypasses the daemon's `write`, which affects the guardrail; see
the last section.

## What blocks it today

### The mount cannot ask for it: `init` is reachable, the reply is not

`FuseHandler::init(&self, req, config: &mut KernelConfig)` is a public,
overridable method and receives `KernelConfig`, so capability negotiation is
available from the handler:

```rust
config.add_capabilities(InitFlags::FUSE_PASSTHROUGH)?;  // Ok(()) when supported
config.set_max_stack_depth(1)?;                         // Ok(0) = previous value
```

`InitFlags` is not re-exported by `easy_fuser`; a direct `fuser = "0.18"`
dependency supplies it. The handler's `open` cannot carry the result, though.
Its signature is

```rust
fn open(&self, req, file_id, flags) -> FuseResult<(Self::FileHandle, FopenFlags)>
```

and the generated driver owns the `ReplyOpen` and calls `reply.opened(fh, flags)`
unconditionally, from inside the threadpool closure. `FopenFlags::FOPEN_PASSTHROUGH`
cannot be smuggled through the return value: `ReplyOpen::opened` asserts the bit
is absent (`fuser/src/reply.rs:362`), and the same assert guards
`ReplyCreate::created` (`reply.rs:510`). Setting the flag panics the event loop
thread, which unmounts the filesystem and fails the open with `EIO`.

`ReplyOpen` is also not constructible from outside `fuser`: `Reply::new` and
`ll::ResponseStruct::new_open` are `pub(crate)`, and `opened` hardcodes
`backing_id = 0`. Passthrough needs the reply object in the same scope that
holds the backing descriptor, to call `reply.open_backing(fd)` and then
`reply.opened_passthrough(fh, flags, &id)`.

### The descriptor must stay registered for the handle's lifetime

`BackingId` is an RAII registration with the connection. Dropping it while the
kernel holds the handle leaves that handle reading a closed backing file, so the
driver must keep a `BackingId` per FUSE handle from `open` to `release`.

## Option 1 — fork `easy_fuser`

The smallest change that keeps `AcFs` on the presets. Two template edits:

1. `templates/fuse_handler.rs.j2`, next to `open`, a defaulted method that
   returns the descriptor to register, or `None` to fall through to ordinary
   FUSE reads:

   ```rust
   fn open_passthrough(
       &self,
       _req: &RequestInfo,
       _file_id: &Self::TId,
       _flags: OpenFlags,
       _file_handle: &Self::FileHandle,
   ) -> FuseResult<Option<std::os::fd::OwnedFd>> {
       Ok(None)
   }
   ```

   The descriptor is separate from `Self::FileHandle` so the trait needs no
   `AsFd` bound; a handle type is not required to be a file descriptor.

2. `templates/fuse_driver.rs.j2`, in the `open` branch: call the method, and on
   `Some(fd)` register it and reply through `opened_passthrough`, else reply as
   today. Add `backing_ids: Arc<Mutex<HashMap<u64, BackingId>>>` to the driver,
   insert on success, remove in `release`.

That compiles and runs against 0.7.1; it is the only crate change needed.

## Option 2 — drive `fuser::Filesystem` directly

`MirrorFs`'s operations are public inherent methods, so a hand-written
`Filesystem` impl can call them and get `ReplyOpen` in hand without a fork. The
cost is that `easy_fuser`'s driver is `pub(crate)`: `FuseDriver`, `mount`,
`RequestResolver`, `FileIdResolver` and `InodeResolvable` are all private, and
only `InodeMapper` is public. Driving `fuser` directly therefore means
reimplementing the inode↔path mapping, the handle table and readdir
continuation — most of the generated driver. Only worth it if the presets are on
their way out anyway.

## Verifying registration

`FUSE_DEV_IOC_BACKING_OPEN` can fail with a bare `EPERM` while the kernel
advertises `FUSE_PASSTHROUGH` and `init` accepted `max_stack_depth >= 1`:

```
ioctl(4, FUSE_DEV_IOC_BACKING_OPEN, ...) = -1 EPERM (Operation not permitted)
```

A `None` from `open_backing` degrades silently to ordinary FUSE reads, so check
the return value and log it. Whether that `EPERM` is a sandbox artefact or a
credential requirement of the ioctl is unresolved: it reproduced under
`unshare -rm`, where `fusermount3` mounts as root and the daemon keeps uid 1000.
**Measure on a host where the daemon runs as the mounting user before investing
in this path.**

Confirm registration with `strace -f -e trace=pread64,ioctl` on the daemon: with
passthrough active, reads of that file produce no `pread64` on the backing file
and the `FUSE_DEV_IOC_BACKING_OPEN` ioctl succeeds.

## Interaction with the guardrail

Passthrough reads the backing file directly, so the daemon no longer inspects
the content path. That is safe for object files, which are immutable, but a
writable handle granted passthrough would write past `reject_object_file` —
including the sequence "write a temp file, rename it to a hash, keep writing
through the still-open descriptor", which mutates an object file.

Grant passthrough on read-only opens only, and keep every write rule on the
delegated path:

```rust
if !writes_content(flags) && self.object_name_def.is_file(&self.path(file_id)) {
    // open the backing file read-only and return the descriptor
}
```

Renaming onto an existing object already fails with `EEXIST`, so no further rule
is needed for the commit path.
