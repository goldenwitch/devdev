//! FUSE-backed driver (Linux only). Thin translation layer between
//! the kernel FUSE protocol and our [`Fs`] backing store.
//!
//! Every callback follows the pattern:
//!   1. lock the shared `Fs`,
//!   2. call the matching method,
//!   3. reply with either the translated result or an errno.
//!
//! No caching, no fast paths, no business logic.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    BackgroundSession, FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request,
    TimeOrNow,
};

use super::{DriverError, MountHandle};
use crate::mem::{Errno, Fs, Ino, InodeAttr, Kind, SetAttr, Timespec};

const TTL: Duration = Duration::from_secs(1);
const GENERATION: u64 = 0;
const BLOCK_SIZE: u32 = 4096;

/// A live FUSE mount. Drop to unmount.
pub struct FuseDriver {
    mount_point: PathBuf,
    _session: BackgroundSession,
}

impl FuseDriver {
    /// Mount `fs` at `mount_point` (must already exist and be a dir).
    pub fn mount(fs: Arc<Mutex<Fs>>, mount_point: &Path) -> Result<Self, DriverError> {
        if !mount_point.is_dir() {
            return Err(DriverError::BadMountPoint(mount_point.to_path_buf()));
        }
        let options = vec![MountOption::FSName("devdev".into())];
        let adapter = FuseAdapter { fs };
        let session = fuser::spawn_mount2(adapter, mount_point, &options)
            .map_err(|e| DriverError::Mount(e.to_string()))?;
        Ok(Self {
            mount_point: mount_point.to_path_buf(),
            _session: session,
        })
    }
}

impl MountHandle for FuseDriver {
    fn mount_point(&self) -> &Path {
        &self.mount_point
    }
}

struct FuseAdapter {
    fs: Arc<Mutex<Fs>>,
}

impl FuseAdapter {
    fn lock(&self) -> std::sync::MutexGuard<'_, Fs> {
        self.fs.lock().expect("fs mutex poisoned")
    }
}

fn errno_to_libc(e: Errno) -> i32 {
    match e {
        Errno::NoEnt => libc::ENOENT,
        Errno::Exist => libc::EEXIST,
        Errno::NotDir => libc::ENOTDIR,
        Errno::IsDir => libc::EISDIR,
        Errno::NotEmpty => libc::ENOTEMPTY,
        Errno::Inval => libc::EINVAL,
        Errno::NoSpc => libc::ENOSPC,
        Errno::NameTooLong => libc::ENAMETOOLONG,
        Errno::Io => libc::EIO,
        Errno::Mlink => libc::EMLINK,
        Errno::Acces => libc::EACCES,
        Errno::BadF => libc::EBADF,
        Errno::NoSys => libc::ENOSYS,
    }
}

fn ts_to_system(t: Timespec) -> SystemTime {
    let secs = if t.secs < 0 { 0 } else { t.secs as u64 };
    UNIX_EPOCH + Duration::new(secs, t.nanos)
}

fn system_to_ts(t: SystemTime) -> Timespec {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => Timespec {
            secs: d.as_secs() as i64,
            nanos: d.subsec_nanos(),
        },
        Err(_) => Timespec::default(),
    }
}

fn time_or_now_to_ts(t: TimeOrNow) -> Timespec {
    match t {
        TimeOrNow::Now => system_to_ts(SystemTime::now()),
        TimeOrNow::SpecificTime(st) => system_to_ts(st),
    }
}

fn to_file_attr(attr: &InodeAttr) -> FileAttr {
    let kind = match attr.kind {
        Kind::File => FileType::RegularFile,
        Kind::Directory => FileType::Directory,
        Kind::Symlink => FileType::Symlink,
    };
    // Report every inode as owned by the process that mounted the FUSE
    // filesystem. The kernel enforces the standard mode-bit permission
    // check against the caller's uid against these owner fields, and
    // since the agent child runs as the same user that mounted, this
    // lets the agent read/write/mkdir inside its own workspace. MemFs
    // itself stays platform-neutral (it stores 0/0) — Unix ownership
    // is purely a driver concern.
    let (uid, gid) = mount_owner();
    FileAttr {
        ino: attr.ino.0,
        size: attr.size,
        blocks: attr.size.div_ceil(512),
        atime: ts_to_system(attr.atime),
        mtime: ts_to_system(attr.mtime),
        ctime: ts_to_system(attr.ctime),
        crtime: ts_to_system(attr.crtime),
        kind,
        perm: attr.mode & 0o7777,
        nlink: attr.nlink,
        uid,
        gid,
        rdev: 0,
        blksize: BLOCK_SIZE,
        flags: 0,
    }
}

/// Cached (euid, egid) of the process at driver-init time. Used to
/// report inode ownership to the kernel so FUSE permission checks
/// resolve correctly for the mounting user.
fn mount_owner() -> (u32, u32) {
    use std::sync::OnceLock;
    static OWNER: OnceLock<(u32, u32)> = OnceLock::new();
    *OWNER.get_or_init(|| {
        // SAFETY: geteuid / getegid are POSIX-guaranteed infallible
        // syscalls. No preconditions, no invariants to uphold.
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        (uid, gid)
    })
}

fn kind_to_ft(k: Kind) -> FileType {
    match k {
        Kind::File => FileType::RegularFile,
        Kind::Directory => FileType::Directory,
        Kind::Symlink => FileType::Symlink,
    }
}

impl Filesystem for FuseAdapter {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        tracing::debug!(parent, ?name, "fuse::lookup");
        let fs = self.lock();
        match fs.lookup(Ino(parent), name.as_bytes()) {
            Ok(attr) => reply.entry(&TTL, &to_file_attr(&attr), GENERATION),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        tracing::debug!(ino, "fuse::getattr");
        let fs = self.lock();
        match fs.getattr(Ino(ino)) {
            Ok(attr) => reply.attr(&TTL, &to_file_attr(&attr)),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        tracing::debug!(ino, "fuse::setattr");
        let sa = SetAttr {
            mode: mode.map(|m| (m & 0o7777) as u16),
            uid,
            gid,
            size,
            atime: atime.map(time_or_now_to_ts),
            mtime: mtime.map(time_or_now_to_ts),
            ctime: ctime.map(system_to_ts),
        };
        let mut fs = self.lock();
        match fs.setattr(Ino(ino), sa) {
            Ok(attr) => reply.attr(&TTL, &to_file_attr(&attr)),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        tracing::debug!(ino, offset, size, "fuse::read");
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let fs = self.lock();
        match fs.read(Ino(ino), offset as u64, size) {
            Ok(data) => reply.data(&data),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        tracing::debug!(ino, offset, len = data.len(), "fuse::write");
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let mut fs = self.lock();
        match fs.write(Ino(ino), offset as u64, data) {
            Ok(n) => reply.written(n),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        tracing::debug!(parent, ?name, mode, "fuse::mkdir");
        let mut fs = self.lock();
        match fs.mkdir(Ino(parent), name.as_bytes(), (mode & 0o7777) as u16) {
            Ok(attr) => reply.entry(&TTL, &to_file_attr(&attr), GENERATION),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        tracing::debug!(parent, ?name, "fuse::rmdir");
        let mut fs = self.lock();
        match fs.rmdir(Ino(parent), name.as_bytes()) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        tracing::debug!(ino, offset, "fuse::readdir");
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let fs = self.lock();
        let entries = match fs.readdir(Ino(ino), offset as u64) {
            Ok(e) => e,
            Err(e) => {
                reply.error(errno_to_libc(e));
                return;
            }
        };
        for (i, entry) in entries.iter().enumerate() {
            let next = offset + i as i64 + 1;
            let name = OsStr::from_bytes(&entry.name);
            if reply.add(entry.ino.0, next, kind_to_ft(entry.kind), name) {
                break;
            }
        }
        reply.ok();
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        tracing::debug!(parent, ?name, mode, "fuse::create");
        let mut fs = self.lock();
        match fs.create(Ino(parent), name.as_bytes(), (mode & 0o7777) as u16) {
            Ok(attr) => reply.created(&TTL, &to_file_attr(&attr), GENERATION, 0, 0),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        tracing::debug!(parent, ?name, "fuse::unlink");
        let mut fs = self.lock();
        match fs.unlink(Ino(parent), name.as_bytes()) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn symlink(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        link: &Path,
        reply: ReplyEntry,
    ) {
        tracing::debug!(parent, ?name, ?link, "fuse::symlink");
        let mut fs = self.lock();
        match fs.symlink(Ino(parent), name.as_bytes(), link.as_os_str().as_bytes()) {
            Ok(attr) => reply.entry(&TTL, &to_file_attr(&attr), GENERATION),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn readlink(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyData) {
        tracing::debug!(ino, "fuse::readlink");
        let fs = self.lock();
        match fs.readlink(Ino(ino)) {
            Ok(target) => reply.data(&target),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn link(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        newparent: u64,
        newname: &OsStr,
        reply: ReplyEntry,
    ) {
        tracing::debug!(ino, newparent, ?newname, "fuse::link");
        let mut fs = self.lock();
        match fs.link(Ino(ino), Ino(newparent), newname.as_bytes()) {
            Ok(attr) => reply.entry(&TTL, &to_file_attr(&attr), GENERATION),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        tracing::debug!(parent, ?name, newparent, ?newname, "fuse::rename");
        let mut fs = self.lock();
        match fs.rename(
            Ino(parent),
            name.as_bytes(),
            Ino(newparent),
            newname.as_bytes(),
        ) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno_to_libc(e)),
        }
    }

    fn open(&mut self, _req: &Request<'_>, _ino: u64, _flags: i32, reply: ReplyOpen) {
        reply.opened(0, 0);
    }

    #[allow(clippy::too_many_arguments)]
    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn opendir(&mut self, _req: &Request<'_>, _ino: u64, _flags: i32, reply: ReplyOpen) {
        reply.opened(0, 0);
    }

    fn releasedir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn fsync(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn fsyncdir(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        let fs = self.lock();
        let limit = fs.bytes_limit();
        let used = fs.bytes_used();
        let free = limit.saturating_sub(used);
        let blocks = limit / BLOCK_SIZE as u64;
        let bfree = free / BLOCK_SIZE as u64;
        reply.statfs(blocks, bfree, bfree, 0, 0, BLOCK_SIZE, 255, BLOCK_SIZE);
    }
}
