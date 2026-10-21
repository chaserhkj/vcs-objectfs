use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use easy_fuser::delegate_fs;
use easy_fuser::fuse_parallel::prelude::*;
use easy_fuser::fuse_presets::mirror_fs::{MirrorFs, MirrorFsTrait};
use easy_fuser::fuse_presets::{StatelessHandler, UnimplementedFuseHandler};

use crate::object::{is_object_file, is_object_name};

/// Mount that makes object files immutable and mirrors every other entry.
///
/// An object file is a regular file whose name is a lowercase hex string. A file
/// becomes one when it is renamed to such a name, which commits it: from then on
/// its content, metadata, name and existence stay fixed. Directories, symlinks
/// and files with other names behave exactly as in the backing directory.
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

    fn path(&self, file_id: &Path) -> PathBuf {
        self.mirror.source_dir().join(file_id)
    }

    /// Refuses to change an object file at `path`.
    fn reject_object_file(&self, path: &Path) -> FuseResult<()> {
        if !is_object_file(path) {
            return Ok(());
        }
        Err(ErrorKind::PermissionDenied.to_error(format!(
            "{} is an object file and immutable",
            path.display()
        )))
    }

    /// Refuses to give a new entry the name of an object file.
    fn reject_object_creation(&self, path: &Path, how: &str) -> FuseResult<()> {
        if !is_object_name(path.file_name().unwrap_or_default()) {
            return Ok(());
        }
        Err(ErrorKind::PermissionDenied.to_error(format!(
            "{}: objects are created by renaming a file to that name, {how} cannot create one",
            path.display()
        )))
    }
}

/// Whether these open flags allow the file content to change.
fn writes_content(flags: OpenFlags) -> bool {
    flags.0 & libc::O_ACCMODE != libc::O_RDONLY || flags.0 & libc::O_TRUNC != 0
}

impl FuseHandler for AcFs {
    type TId = PathBuf;
    type FileHandle = OwnedFd;

    delegate_fs! { mirror, [
        bmap, flush, fsync, getattr, getlk, getxattr, ioctl, listxattr, lookup,
        lseek, mkdir, read, readdir, readdirplus, readlink, release, rmdir,
        setlk, statfs, symlink
    ] }

    delegate_fs! { directory, [ forget, fsyncdir, opendir, releasedir ] }

    // MirrorFs has no hard-link support; a link handler belongs next to the paths
    // it records.
    delegate_fs! { unsupported, [ link ] }

    fn access(&self, req: &RequestInfo, file_id: PathBuf, mask: AccessFlags) -> FuseResult<()> {
        let mut mask = mask;
        if is_object_file(&self.path(&file_id)) {
            mask.remove(AccessFlags::W_OK);
        }
        self.mirror.access(req, file_id, mask)
    }

    fn open(
        &self,
        req: &RequestInfo,
        file_id: PathBuf,
        flags: OpenFlags,
    ) -> FuseResult<(Self::FileHandle, FopenFlags)> {
        if writes_content(flags) {
            self.reject_object_file(&self.path(&file_id))?;
        }
        self.mirror.open(req, file_id, flags)
    }

    fn create(
        &self,
        req: &RequestInfo,
        parent_id: PathBuf,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: OpenFlags,
    ) -> FuseResult<(Self::FileHandle, FileAttribute, FopenFlags)> {
        let path = self.path(&parent_id).join(name);
        if writes_content(flags) {
            self.reject_object_file(&path)?;
        }
        if !is_object_file(&path) {
            self.reject_object_creation(&path, "creating a file")?;
        }
        self.mirror.create(req, parent_id, name, mode, umask, flags)
    }

    fn mknod(
        &self,
        req: &RequestInfo,
        parent_id: PathBuf,
        name: &OsStr,
        mode: u32,
        umask: u32,
        rdev: DeviceType,
    ) -> FuseResult<FileAttribute> {
        if mode & libc::S_IFMT == libc::S_IFREG {
            self.reject_object_creation(&self.path(&parent_id).join(name), "mknod")?;
        }
        self.mirror.mknod(req, parent_id, name, mode, umask, rdev)
    }

    fn write(
        &self,
        req: &RequestInfo,
        file_id: PathBuf,
        file_handle: Option<&mut Self::FileHandle>,
        seek: SeekFrom,
        data: Vec<u8>,
        write_flags: WriteFlags,
        flags: OpenFlags,
        lock_owner: Option<u64>,
    ) -> FuseResult<u32> {
        self.reject_object_file(&self.path(&file_id))?;
        self.mirror.write(
            req,
            file_id,
            file_handle,
            seek,
            data,
            write_flags,
            flags,
            lock_owner,
        )
    }

    fn fallocate(
        &self,
        req: &RequestInfo,
        file_id: PathBuf,
        file_handle: Option<&mut Self::FileHandle>,
        offset: i64,
        length: i64,
        mode: FallocateFlags,
    ) -> FuseResult<()> {
        self.reject_object_file(&self.path(&file_id))?;
        self.mirror
            .fallocate(req, file_id, file_handle, offset, length, mode)
    }

    fn copy_file_range(
        &self,
        req: &RequestInfo,
        file_in: PathBuf,
        file_handle_in: Option<&Self::FileHandle>,
        offset_in: u64,
        file_out: PathBuf,
        file_handle_out: Option<&Self::FileHandle>,
        offset_out: u64,
        len: u64,
        flags: CopyFileRangeFlags,
    ) -> FuseResult<u32> {
        self.reject_object_file(&self.path(&file_out))?;
        self.mirror.copy_file_range(
            req,
            file_in,
            file_handle_in,
            offset_in,
            file_out,
            file_handle_out,
            offset_out,
            len,
            flags,
        )
    }

    fn setattr(
        &self,
        req: &RequestInfo,
        file_id: PathBuf,
        attrs: SetAttrRequest,
        file_handle: Option<&mut Self::FileHandle>,
    ) -> FuseResult<FileAttribute> {
        self.reject_object_file(&self.path(&file_id))?;
        self.mirror.setattr(req, file_id, attrs, file_handle)
    }

    fn setxattr(
        &self,
        req: &RequestInfo,
        file_id: PathBuf,
        name: &OsStr,
        value: Vec<u8>,
        flags: SetXAttrFlags,
        position: u32,
    ) -> FuseResult<()> {
        self.reject_object_file(&self.path(&file_id))?;
        self.mirror
            .setxattr(req, file_id, name, value, flags, position)
    }

    fn removexattr(&self, req: &RequestInfo, file_id: PathBuf, name: &OsStr) -> FuseResult<()> {
        self.reject_object_file(&self.path(&file_id))?;
        self.mirror.removexattr(req, file_id, name)
    }

    fn unlink(&self, req: &RequestInfo, parent_id: PathBuf, name: &OsStr) -> FuseResult<()> {
        self.reject_object_file(&self.path(&parent_id).join(name))?;
        self.mirror.unlink(req, parent_id, name)
    }

    fn rename(
        &self,
        req: &RequestInfo,
        parent_id: PathBuf,
        name: &OsStr,
        newparent: PathBuf,
        newname: &OsStr,
        flags: RenameFlags,
    ) -> FuseResult<()> {
        self.reject_object_file(&self.path(&parent_id).join(name))?;
        let target = self.path(&newparent).join(newname);
        if is_object_file(&target) {
            return Err(ErrorKind::FileExists.to_error(format!(
                "{} is an object file and keeps its name",
                target.display()
            )));
        }
        self.mirror
            .rename(req, parent_id, name, newparent, newname, flags)
    }
}
