use std::os::fd::OwnedFd;
use std::path::PathBuf;

use easy_fuser::delegate_fs;
use easy_fuser::fuse_parallel::prelude::*;
use easy_fuser::fuse_presets::mirror_fs::{MirrorFs, MirrorFsTrait};
use easy_fuser::fuse_presets::{StatelessHandler, UnimplementedFuseHandler};

/// Every operation is delegated to a preset, so the mount mirrors the backing
/// directory exactly; the handlers that will carry access decisions are added
/// one operation at a time by replacing the matching delegate entry.
pub struct AcFs {
    mirror: MirrorFs,
    directory: StatelessHandler<PathBuf>,
    unsupported: UnimplementedFuseHandler<PathBuf>,
}

impl AcFs {
    pub fn new(source: PathBuf) -> Self {
        Self {
            mirror: MirrorFs::new(source),
            directory: StatelessHandler::new(),
            unsupported: UnimplementedFuseHandler::new(),
        }
    }
}

impl FuseHandler for AcFs {
    type TId = PathBuf;
    type FileHandle = OwnedFd;

    delegate_fs! { mirror, [
        access, bmap, copy_file_range, create, fallocate, flush, fsync, getattr,
        getlk, getxattr, ioctl, listxattr, lookup, lseek, mkdir, mknod, open,
        read, readdir, readdirplus, readlink, release, removexattr, rename,
        rmdir, setattr, setlk, setxattr, statfs, symlink, unlink, write
    ] }

    delegate_fs! { directory, [ forget, fsyncdir, opendir, releasedir ] }

    // MirrorFs has no hard-link support; a link handler belongs next to the paths
    // it records.
    delegate_fs! { unsupported, [ link ] }
}
