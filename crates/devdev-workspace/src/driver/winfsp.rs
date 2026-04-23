//! WinFSP-backed driver (Windows only).
//!
//! Mounts an [`Fs`] at a free drive letter via the hand-rolled FFI
//! in [`super::winfsp_sys`]. Callbacks receive UTF-16 paths from the
//! kernel; we resolve them against [`Fs`] using [`Fs::resolve`] and
//! translate [`Errno`] into NTSTATUS.
//!
//! **FileContext encoding.** We enable the
//! `UmFileContextIsUserContext2` flag so WinFSP stores our inode
//! number directly in the `PVOID` context slot. No per-handle heap
//! allocations; a close is a no-op.
//!
//! **Guard strategy.** Coarse (serialized). All ops take the `Fs`
//! mutex; WinFSP layer adds no additional concurrency.
//!
//! **What's not supported yet.** Reparse points (symlinks via NTFS
//! semantics), ACL manipulation, alternate data streams. They all
//! return `STATUS_NOT_IMPLEMENTED` — sufficient for typical dev
//! workloads (cargo build, ls, cat, rustc).

use super::{DriverError, MountHandle};
use crate::mem::{Errno, Fs, Ino, Kind, SetAttr, Timespec};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Mutex, OnceLock};

use super::winfsp_sys as ffi;

/// A live WinFSP mount. Drop to unmount.
pub struct WinFspDriver {
    mount_point: PathBuf,
    handle: MountHandlePtr,
    /// Kept alive for the lifetime of the mount; the FFI callbacks
    /// read this through the FSP_FILE_SYSTEM UserContext slot.
    _context: Box<FsContext>,
}

/// Newtype wrapper so we can mark the pointer `Send`. The pointer is
/// only dereferenced under the `Fs` mutex inside callbacks, and the
/// driver's `Drop` stops the dispatcher before the context is freed.
struct MountHandlePtr(*mut ffi::FSP_FILE_SYSTEM);

// SAFETY: See field doc. The underlying WinFSP handle is thread-safe
// for the operations we perform (create/set-mount/start/stop).
unsafe impl Send for MountHandlePtr {}
unsafe impl Sync for MountHandlePtr {}

struct FsContext {
    fs: Arc<Mutex<Fs>>,
}

impl WinFspDriver {
    /// Mount `fs` at an auto-selected free drive letter (Z: downwards).
    pub fn mount_auto(fs: Arc<Mutex<Fs>>) -> Result<Self, DriverError> {
        let letter = pick_free_drive_letter()
            .ok_or_else(|| DriverError::Mount("no free drive letters".into()))?;
        let mp_str = format!("{letter}:");
        Self::mount(fs, Path::new(&mp_str))
    }

    /// Mount `fs` at `mount_point`. For in-memory filesystems WinFSP
    /// only supports drive-letter mounts; pass a string like `"X:"`.
    pub fn mount(fs: Arc<Mutex<Fs>>, mount_point: &Path) -> Result<Self, DriverError> {
        ensure_winfsp_loaded();
        let context = Box::new(FsContext { fs });

        let mut vp: ffi::FSP_FSCTL_VOLUME_PARAMS = unsafe { std::mem::zeroed() };
        vp.Version = std::mem::size_of::<ffi::FSP_FSCTL_VOLUME_PARAMS>() as u16;
        vp.SectorSize = 4096;
        vp.SectorsPerAllocationUnit = 1;
        vp.MaxComponentLength = 255;
        vp.VolumeCreationTime = now_filetime();
        vp.VolumeSerialNumber = 0xDEAD_BEEF;
        vp.FileInfoTimeout = 1000;
        vp.flags = ffi::FLAG_CASE_SENSITIVE_SEARCH
            | ffi::FLAG_CASE_PRESERVED_NAMES
            | ffi::FLAG_UNICODE_ON_DISK
            | ffi::FLAG_UM_FILE_CONTEXT_IS_USER_CONTEXT2
            | ffi::FLAG_SUPPORTS_POSIX_UNLINK_RENAME;
        write_wchar_buf(&mut vp.FileSystemName, "DEVDEV");

        let mut handle: *mut ffi::FSP_FILE_SYSTEM = ptr::null_mut();
        // "WinFsp.Disk" is the device path for local-disk mounts
        // (drive-letter mounts). `FspFileSystemCreate` requires a
        // non-NULL device path — NULL yields STATUS_NO_SUCH_DEVICE
        // (0xC000000E).
        let device_path: Vec<u16> = "WinFsp.Disk"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let status = unsafe {
            ffi::FspFileSystemCreate(
                device_path.as_ptr() as ffi::PWSTR,
                &vp,
                &INTERFACE,
                &mut handle,
            )
        };
        if status != ffi::STATUS_SUCCESS {
            return Err(DriverError::Mount(format!(
                "FspFileSystemCreate failed: NTSTATUS 0x{:X}",
                status as u32
            )));
        }

        set_user_context(handle, context.as_ref() as *const _ as ffi::PVOID);

        unsafe {
            ffi::FspFileSystemSetOperationGuardStrategyF(
                handle,
                ffi::FSP_FILE_SYSTEM_OPERATION_GUARD_STRATEGY_COARSE,
            );
        }

        let mount_wide: Vec<u16> = OsStr::new(mount_point)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let status = unsafe {
            ffi::FspFileSystemSetMountPoint(handle, mount_wide.as_ptr() as ffi::PWSTR)
        };
        if status != ffi::STATUS_SUCCESS {
            unsafe { ffi::FspFileSystemDelete(handle) };
            return Err(DriverError::Mount(format!(
                "FspFileSystemSetMountPoint({}) failed: NTSTATUS 0x{:X}",
                mount_point.display(),
                status as u32
            )));
        }

        let status = unsafe { ffi::FspFileSystemStartDispatcher(handle, 1) };
        if status != ffi::STATUS_SUCCESS {
            unsafe {
                ffi::FspFileSystemRemoveMountPoint(handle);
                ffi::FspFileSystemDelete(handle);
            }
            return Err(DriverError::Mount(format!(
                "FspFileSystemStartDispatcher failed: NTSTATUS 0x{:X}",
                status as u32
            )));
        }

        Ok(Self {
            mount_point: mount_point.to_path_buf(),
            handle: MountHandlePtr(handle),
            _context: context,
        })
    }
}

impl Drop for WinFspDriver {
    fn drop(&mut self) {
        let h = self.handle.0;
        if h.is_null() {
            return;
        }
        unsafe {
            ffi::FspFileSystemStopDispatcher(h);
            ffi::FspFileSystemRemoveMountPoint(h);
            ffi::FspFileSystemDelete(h);
        }
    }
}

impl MountHandle for WinFspDriver {
    fn mount_point(&self) -> &Path {
        &self.mount_point
    }
}

// ---------------------------------------------------------------------------
// UserContext accessor (FSP_FILE_SYSTEM prefix layout)
// ---------------------------------------------------------------------------
//
// winfsp.h exposes `FSP_FILE_SYSTEM` as:
//
//   typedef struct _FSP_FILE_SYSTEM {
//       UINT16 Version;
//       PVOID UserContext;   // offset 8 on x86_64 due to alignment
//       ...
//   } FSP_FILE_SYSTEM;
//
// We only touch `UserContext`, so we declare a minimal prefix
// struct matching the known field layout.
#[repr(C)]
#[allow(non_snake_case)]
struct FspFileSystemPrefix {
    Version: u16,
    _pad: [u8; 6],
    UserContext: ffi::PVOID,
}

fn set_user_context(handle: *mut ffi::FSP_FILE_SYSTEM, ctx: ffi::PVOID) {
    unsafe {
        let p = handle as *mut FspFileSystemPrefix;
        (*p).UserContext = ctx;
    }
}

fn get_user_context(handle: *mut ffi::FSP_FILE_SYSTEM) -> *const FsContext {
    unsafe {
        let p = handle as *mut FspFileSystemPrefix;
        (*p).UserContext as *const FsContext
    }
}

// ---------------------------------------------------------------------------
// Callback table
// ---------------------------------------------------------------------------

static INTERFACE: ffi::FSP_FILE_SYSTEM_INTERFACE = ffi::FSP_FILE_SYSTEM_INTERFACE {
    GetVolumeInfo: Some(cb_get_volume_info),
    SetVolumeLabel: None,
    GetSecurityByName: Some(cb_get_security_by_name),
    Create: Some(cb_create),
    Open: Some(cb_open),
    Overwrite: Some(cb_overwrite),
    Cleanup: Some(cb_cleanup),
    Close: Some(cb_close),
    Read: Some(cb_read),
    Write: Some(cb_write),
    Flush: Some(cb_flush),
    GetFileInfo: Some(cb_get_file_info),
    SetBasicInfo: Some(cb_set_basic_info),
    SetFileSize: Some(cb_set_file_size),
    CanDelete: Some(cb_can_delete),
    Rename: Some(cb_rename),
    GetSecurity: None,
    SetSecurity: None,
    ReadDirectory: Some(cb_read_directory),
    ResolveReparsePoints: None,
    GetReparsePoint: None,
    SetReparsePoint: None,
    DeleteReparsePoint: None,
    GetStreamInfo: None,
    GetDirInfoByName: None,
    Control: None,
    SetDelete: None,
    CreateEx: None,
    OverwriteEx: None,
    GetEa: None,
    SetEa: None,
    DispatcherStopped: None,
    Reserved: [None; 31],
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a null-terminated UTF-16 WinFSP path into MemFs-friendly
/// bytes: backslashes → forward slashes, ensured to start with '/'.
/// Returns None on invalid UTF-16.
fn pwstr_to_fs_path(p: ffi::PWSTR) -> Option<Vec<u8>> {
    if p.is_null() {
        return None;
    }
    let wide = unsafe { wide_to_slice(p) };
    let s = String::from_utf16(wide).ok()?;
    let mut out = Vec::with_capacity(s.len() + 1);
    if !s.starts_with('\\') && !s.starts_with('/') {
        out.push(b'/');
    }
    for b in s.bytes() {
        out.push(if b == b'\\' { b'/' } else { b });
    }
    Some(out)
}

unsafe fn wide_to_slice<'a>(p: ffi::PWSTR) -> &'a [u16] {
    unsafe {
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(p, len)
    }
}

fn write_wchar_buf(buf: &mut [u16], s: &str) {
    let wide: Vec<u16> = s.encode_utf16().collect();
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    if n < buf.len() {
        buf[n] = 0;
    }
}

fn errno_to_ntstatus(e: Errno) -> ffi::NTSTATUS {
    match e {
        Errno::NoEnt => ffi::STATUS_OBJECT_NAME_NOT_FOUND,
        Errno::Exist => ffi::STATUS_OBJECT_NAME_COLLISION,
        Errno::NotDir => ffi::STATUS_NOT_A_DIRECTORY,
        Errno::IsDir => ffi::STATUS_FILE_IS_A_DIRECTORY,
        Errno::NotEmpty => ffi::STATUS_DIRECTORY_NOT_EMPTY,
        Errno::NameTooLong => ffi::STATUS_NAME_TOO_LONG,
        Errno::Io => ffi::STATUS_IO_DEVICE_ERROR,
        Errno::NoSpc => ffi::STATUS_DISK_FULL,
        Errno::Acces => ffi::STATUS_ACCESS_DENIED,
        Errno::BadF => ffi::STATUS_INVALID_HANDLE,
        Errno::NoSys => ffi::STATUS_NOT_IMPLEMENTED,
        Errno::Inval | Errno::Mlink => ffi::STATUS_INVALID_PARAMETER,
    }
}

fn ts_to_filetime(t: Timespec) -> u64 {
    let secs = if t.secs < 0 { 0u64 } else { t.secs as u64 };
    secs.saturating_mul(10_000_000)
        .saturating_add(u64::from(t.nanos) / 100)
        .saturating_add(ffi::FILETIME_UNIX_EPOCH_DELTA)
}

fn filetime_to_ts(ft: u64) -> Timespec {
    if ft < ffi::FILETIME_UNIX_EPOCH_DELTA {
        return Timespec::default();
    }
    let rel = ft - ffi::FILETIME_UNIX_EPOCH_DELTA;
    Timespec {
        secs: (rel / 10_000_000) as i64,
        nanos: ((rel % 10_000_000) * 100) as u32,
    }
}

fn now_filetime() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    d.as_secs()
        .saturating_mul(10_000_000)
        .saturating_add(u64::from(d.subsec_nanos()) / 100)
        .saturating_add(ffi::FILETIME_UNIX_EPOCH_DELTA)
}

fn fill_file_info(attr: &crate::mem::InodeAttr, out: &mut ffi::FSP_FSCTL_FILE_INFO) {
    out.FileAttributes = match attr.kind {
        Kind::Directory => ffi::FILE_ATTRIBUTE_DIRECTORY,
        Kind::File => ffi::FILE_ATTRIBUTE_NORMAL,
        Kind::Symlink => ffi::FILE_ATTRIBUTE_REPARSE_POINT,
    };
    out.ReparseTag = 0;
    out.FileSize = attr.size;
    out.AllocationSize = attr.size.div_ceil(4096) * 4096;
    out.CreationTime = ts_to_filetime(attr.crtime);
    out.LastAccessTime = ts_to_filetime(attr.atime);
    out.LastWriteTime = ts_to_filetime(attr.mtime);
    out.ChangeTime = ts_to_filetime(attr.ctime);
    out.IndexNumber = attr.ino.0;
    out.HardLinks = 0;
    out.EaSize = 0;
}

fn split_parent(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|&b| b == b'/') {
        Some(0) => (b"/", &path[1..]),
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (b"/", path),
    }
}

fn with_fs<R>(fs: *mut ffi::FSP_FILE_SYSTEM, f: impl FnOnce(&mut Fs) -> R) -> R {
    let ctx = get_user_context(fs);
    // SAFETY: FsContext is kept alive for the lifetime of the driver.
    // Drop stops the dispatcher before the context is freed, so no
    // callback can race with deallocation.
    let arc = unsafe { &(*ctx).fs };
    let mut guard = arc.lock().expect("fs mutex poisoned");
    f(&mut guard)
}

// ---------------------------------------------------------------------------
// Permissive security descriptor (built once, reused)
// ---------------------------------------------------------------------------

struct StaticSd {
    ptr: ffi::PSECURITY_DESCRIPTOR,
    len: u32,
}

// SAFETY: The pointer is allocated once and never mutated; WinFSP
// only reads it.
unsafe impl Send for StaticSd {}
unsafe impl Sync for StaticSd {}

fn permissive_sd() -> &'static StaticSd {
    static SD: OnceLock<StaticSd> = OnceLock::new();
    SD.get_or_init(|| {
        // Owner=SYSTEM, Group=SYSTEM, protected DACL allowing
        // SYSTEM, BUILTIN\Administrators, and Everyone full access.
        // WinFSP's kernel validation rejects DACL-only descriptors
        // returned from GetSecurityByName with ERROR_INVALID_SECURITY_DESCR
        // (1338); the owner/group SIDs are required.
        let sddl: Vec<u16> = "O:SYG:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;WD)"
            .encode_utf16()
            .chain([0])
            .collect();
        let mut out: ffi::PSECURITY_DESCRIPTOR = ptr::null_mut();
        let mut len = 0u32;
        let ok = unsafe {
            ffi::ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                ffi::SDDL_REVISION_1,
                &mut out,
                &mut len,
            )
        };
        assert!(ok != 0, "failed to build permissive SD");
        if len == 0 {
            len = unsafe { ffi::GetSecurityDescriptorLength(out) };
        }
        StaticSd { ptr: out, len }
    })
}

// ---------------------------------------------------------------------------
// Callback implementations
// ---------------------------------------------------------------------------

unsafe extern "system" fn cb_get_volume_info(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    info: *mut ffi::FSP_FSCTL_VOLUME_INFO,
) -> ffi::NTSTATUS {
    with_fs(fs, |g| {
        let limit = g.bytes_limit();
        let used = g.bytes_used();
        let free = limit.saturating_sub(used);
        unsafe {
            (*info).TotalSize = limit;
            (*info).FreeSize = free;
            (*info).VolumeLabelLength = 0;
            (*info).VolumeLabel[0] = 0;
        }
        ffi::STATUS_SUCCESS
    })
}

unsafe extern "system" fn cb_get_security_by_name(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_name: ffi::PWSTR,
    p_file_attributes: *mut ffi::UINT32,
    sd: ffi::PSECURITY_DESCRIPTOR,
    p_sd_size: *mut ffi::SIZE_T,
) -> ffi::NTSTATUS {
    let Some(path) = pwstr_to_fs_path(file_name) else {
        return ffi::STATUS_INVALID_PARAMETER;
    };
    let attr_result =
        with_fs(fs, |g| g.resolve(&path).and_then(|ino| g.getattr(ino)));
    let attr = match attr_result {
        Ok(a) => a,
        Err(e) => return errno_to_ntstatus(e),
    };
    if !p_file_attributes.is_null() {
        let fattr = match attr.kind {
            Kind::Directory => ffi::FILE_ATTRIBUTE_DIRECTORY,
            Kind::File => ffi::FILE_ATTRIBUTE_NORMAL,
            Kind::Symlink => ffi::FILE_ATTRIBUTE_REPARSE_POINT,
        };
        unsafe { *p_file_attributes = fattr };
    }
    if !p_sd_size.is_null() {
        let perm = permissive_sd();
        let want = perm.len as usize;
        let have = unsafe { *p_sd_size };
        unsafe { *p_sd_size = want };
        if sd.is_null() || have < want {
            return ffi::STATUS_BUFFER_OVERFLOW;
        }
        unsafe { std::ptr::copy_nonoverlapping(perm.ptr as *const u8, sd as *mut u8, want) };
    }
    ffi::STATUS_SUCCESS
}

unsafe extern "system" fn cb_open(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_name: ffi::PWSTR,
    _create_options: ffi::UINT32,
    _granted_access: ffi::UINT32,
    p_file_context: *mut ffi::PVOID,
    file_info: *mut ffi::FSP_FSCTL_FILE_INFO,
) -> ffi::NTSTATUS {
    let Some(path) = pwstr_to_fs_path(file_name) else {
        return ffi::STATUS_INVALID_PARAMETER;
    };
    with_fs(fs, |g| {
        let ino = match g.resolve(&path) {
            Ok(i) => i,
            Err(e) => return errno_to_ntstatus(e),
        };
        let attr = match g.getattr(ino) {
            Ok(a) => a,
            Err(e) => return errno_to_ntstatus(e),
        };
        unsafe {
            *p_file_context = ino.0 as ffi::PVOID;
            fill_file_info(&attr, &mut *file_info);
        }
        ffi::STATUS_SUCCESS
    })
}

unsafe extern "system" fn cb_create(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_name: ffi::PWSTR,
    create_options: ffi::UINT32,
    _granted_access: ffi::UINT32,
    _file_attributes: ffi::UINT32,
    _sd: ffi::PSECURITY_DESCRIPTOR,
    _allocation_size: ffi::UINT64,
    p_file_context: *mut ffi::PVOID,
    file_info: *mut ffi::FSP_FSCTL_FILE_INFO,
) -> ffi::NTSTATUS {
    let Some(path) = pwstr_to_fs_path(file_name) else {
        return ffi::STATUS_INVALID_PARAMETER;
    };
    let (parent_path, name) = split_parent(&path);
    if name.is_empty() {
        return ffi::STATUS_OBJECT_NAME_COLLISION;
    }
    let want_dir = (create_options & ffi::FILE_DIRECTORY_FILE) != 0;
    with_fs(fs, |g| {
        let parent = match g.resolve(parent_path) {
            Ok(i) => i,
            Err(e) => return errno_to_ntstatus(e),
        };
        let res = if want_dir {
            g.mkdir(parent, name, 0o755)
        } else {
            g.create(parent, name, 0o644)
        };
        let attr = match res {
            Ok(a) => a,
            Err(e) => return errno_to_ntstatus(e),
        };
        unsafe {
            *p_file_context = attr.ino.0 as ffi::PVOID;
            fill_file_info(&attr, &mut *file_info);
        }
        ffi::STATUS_SUCCESS
    })
}

unsafe extern "system" fn cb_overwrite(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    _file_attributes: ffi::UINT32,
    _replace: ffi::BOOLEAN,
    _allocation_size: ffi::UINT64,
    file_info: *mut ffi::FSP_FSCTL_FILE_INFO,
) -> ffi::NTSTATUS {
    let ino = Ino(file_context as u64);
    with_fs(fs, |g| {
        let sa = SetAttr {
            size: Some(0),
            ..Default::default()
        };
        let attr = match g.setattr(ino, sa) {
            Ok(a) => a,
            Err(e) => return errno_to_ntstatus(e),
        };
        unsafe { fill_file_info(&attr, &mut *file_info) };
        ffi::STATUS_SUCCESS
    })
}

unsafe extern "system" fn cb_cleanup(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    file_name: ffi::PWSTR,
    flags: ffi::ULONG,
) -> ffi::NTSTATUS {
    if (flags & ffi::FspCleanupDelete) != 0 {
        let Some(path) = pwstr_to_fs_path(file_name) else {
            return ffi::STATUS_SUCCESS;
        };
        let (parent_path, name) = split_parent(&path);
        let _ = with_fs(fs, |g| -> Result<(), Errno> {
            let parent = g.resolve(parent_path)?;
            let ino = Ino(file_context as u64);
            let attr = g.getattr(ino)?;
            if attr.kind == Kind::Directory {
                g.rmdir(parent, name)
            } else {
                g.unlink(parent, name)
            }
        });
    }
    ffi::STATUS_SUCCESS
}

unsafe extern "system" fn cb_close(
    _fs: *mut ffi::FSP_FILE_SYSTEM,
    _file_context: ffi::PVOID,
) -> ffi::NTSTATUS {
    ffi::STATUS_SUCCESS
}

unsafe extern "system" fn cb_read(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    buffer: ffi::PVOID,
    offset: ffi::UINT64,
    length: ffi::ULONG,
    p_bytes_transferred: ffi::PULONG,
) -> ffi::NTSTATUS {
    let ino = Ino(file_context as u64);
    with_fs(fs, |g| {
        let attr = match g.getattr(ino) {
            Ok(a) => a,
            Err(e) => return errno_to_ntstatus(e),
        };
        if offset >= attr.size {
            return ffi::STATUS_END_OF_FILE;
        }
        match g.read(ino, offset, length) {
            Ok(data) => {
                let n = data.len();
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), buffer as *mut u8, n);
                    *p_bytes_transferred = n as u32;
                }
                ffi::STATUS_SUCCESS
            }
            Err(e) => errno_to_ntstatus(e),
        }
    })
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn cb_write(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    buffer: ffi::PVOID,
    offset: ffi::UINT64,
    length: ffi::ULONG,
    _write_to_end_of_file: ffi::BOOLEAN,
    _constrained_io: ffi::BOOLEAN,
    p_bytes_transferred: ffi::PULONG,
    file_info: *mut ffi::FSP_FSCTL_FILE_INFO,
) -> ffi::NTSTATUS {
    let ino = Ino(file_context as u64);
    let slice = unsafe { std::slice::from_raw_parts(buffer as *const u8, length as usize) };
    with_fs(fs, |g| match g.write(ino, offset, slice) {
        Ok(n) => match g.getattr(ino) {
            Ok(attr) => {
                unsafe {
                    *p_bytes_transferred = n;
                    fill_file_info(&attr, &mut *file_info);
                }
                ffi::STATUS_SUCCESS
            }
            Err(e) => errno_to_ntstatus(e),
        },
        Err(e) => errno_to_ntstatus(e),
    })
}

unsafe extern "system" fn cb_flush(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    file_info: *mut ffi::FSP_FSCTL_FILE_INFO,
) -> ffi::NTSTATUS {
    if file_info.is_null() {
        return ffi::STATUS_SUCCESS;
    }
    let ino = Ino(file_context as u64);
    if ino.0 == 0 {
        return ffi::STATUS_SUCCESS;
    }
    with_fs(fs, |g| match g.getattr(ino) {
        Ok(attr) => {
            unsafe { fill_file_info(&attr, &mut *file_info) };
            ffi::STATUS_SUCCESS
        }
        Err(e) => errno_to_ntstatus(e),
    })
}

unsafe extern "system" fn cb_get_file_info(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    file_info: *mut ffi::FSP_FSCTL_FILE_INFO,
) -> ffi::NTSTATUS {
    let ino = Ino(file_context as u64);
    with_fs(fs, |g| match g.getattr(ino) {
        Ok(attr) => {
            unsafe { fill_file_info(&attr, &mut *file_info) };
            ffi::STATUS_SUCCESS
        }
        Err(e) => errno_to_ntstatus(e),
    })
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn cb_set_basic_info(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    _file_attributes: ffi::UINT32,
    _creation_time: ffi::UINT64,
    last_access_time: ffi::UINT64,
    last_write_time: ffi::UINT64,
    change_time: ffi::UINT64,
    file_info: *mut ffi::FSP_FSCTL_FILE_INFO,
) -> ffi::NTSTATUS {
    let ino = Ino(file_context as u64);
    let to_ts = |ft: u64| (ft != 0).then(|| filetime_to_ts(ft));
    let sa = SetAttr {
        atime: to_ts(last_access_time),
        mtime: to_ts(last_write_time),
        ctime: to_ts(change_time),
        ..Default::default()
    };
    with_fs(fs, |g| match g.setattr(ino, sa) {
        Ok(attr) => {
            unsafe { fill_file_info(&attr, &mut *file_info) };
            ffi::STATUS_SUCCESS
        }
        Err(e) => errno_to_ntstatus(e),
    })
}

unsafe extern "system" fn cb_set_file_size(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    new_size: ffi::UINT64,
    set_allocation_size: ffi::BOOLEAN,
    file_info: *mut ffi::FSP_FSCTL_FILE_INFO,
) -> ffi::NTSTATUS {
    if set_allocation_size != 0 {
        return unsafe { cb_get_file_info(fs, file_context, file_info) };
    }
    let ino = Ino(file_context as u64);
    let sa = SetAttr {
        size: Some(new_size),
        ..Default::default()
    };
    with_fs(fs, |g| match g.setattr(ino, sa) {
        Ok(attr) => {
            unsafe { fill_file_info(&attr, &mut *file_info) };
            ffi::STATUS_SUCCESS
        }
        Err(e) => errno_to_ntstatus(e),
    })
}

unsafe extern "system" fn cb_can_delete(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    _file_name: ffi::PWSTR,
) -> ffi::NTSTATUS {
    let ino = Ino(file_context as u64);
    with_fs(fs, |g| match g.getattr(ino) {
        Ok(attr) => {
            if attr.kind == Kind::Directory {
                match g.readdir(ino, 0) {
                    Ok(entries) => {
                        let real = entries
                            .iter()
                            .filter(|e| e.name.as_slice() != b"." && e.name.as_slice() != b"..")
                            .count();
                        if real == 0 {
                            ffi::STATUS_SUCCESS
                        } else {
                            ffi::STATUS_DIRECTORY_NOT_EMPTY
                        }
                    }
                    Err(e) => errno_to_ntstatus(e),
                }
            } else {
                ffi::STATUS_SUCCESS
            }
        }
        Err(e) => errno_to_ntstatus(e),
    })
}

unsafe extern "system" fn cb_rename(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    _file_context: ffi::PVOID,
    file_name: ffi::PWSTR,
    new_file_name: ffi::PWSTR,
    _replace: ffi::BOOLEAN,
) -> ffi::NTSTATUS {
    let (Some(src), Some(dst)) =
        (pwstr_to_fs_path(file_name), pwstr_to_fs_path(new_file_name))
    else {
        return ffi::STATUS_INVALID_PARAMETER;
    };
    let (sp, sn) = split_parent(&src);
    let (dp, dn) = split_parent(&dst);
    with_fs(fs, |g| {
        let src_parent = match g.resolve(sp) {
            Ok(i) => i,
            Err(e) => return errno_to_ntstatus(e),
        };
        let dst_parent = match g.resolve(dp) {
            Ok(i) => i,
            Err(e) => return errno_to_ntstatus(e),
        };
        match g.rename(src_parent, sn, dst_parent, dn) {
            Ok(()) => ffi::STATUS_SUCCESS,
            Err(e) => errno_to_ntstatus(e),
        }
    })
}

unsafe extern "system" fn cb_read_directory(
    fs: *mut ffi::FSP_FILE_SYSTEM,
    file_context: ffi::PVOID,
    _pattern: ffi::PWSTR,
    marker: ffi::PWSTR,
    buffer: ffi::PVOID,
    length: ffi::ULONG,
    p_bytes_transferred: ffi::PULONG,
) -> ffi::NTSTATUS {
    let ino = Ino(file_context as u64);
    let marker_bytes: Option<Vec<u8>> = if marker.is_null() {
        None
    } else {
        let wide = unsafe { wide_to_slice(marker) };
        String::from_utf16(wide).ok().map(|s| s.into_bytes())
    };

    let entries = with_fs(fs, |g| g.readdir(ino, 0));
    let entries = match entries {
        Ok(e) => e,
        Err(e) => return errno_to_ntstatus(e),
    };

    // Pre-fetch attrs so we don't re-lock the mutex per entry.
    let attrs: Vec<_> = with_fs(fs, |g| {
        entries
            .iter()
            .map(|e| g.getattr(e.ino).ok())
            .collect()
    });

    let mut past_marker = marker_bytes.is_none();
    for (entry, attr_opt) in entries.iter().zip(attrs.iter()) {
        if !past_marker {
            if entry.name.as_slice() == marker_bytes.as_deref().unwrap_or(&[]) {
                past_marker = true;
            }
            continue;
        }
        let Some(attr) = attr_opt else { continue };

        let name_wide: Vec<u16> = match std::str::from_utf8(&entry.name) {
            Ok(s) => s.encode_utf16().collect(),
            Err(_) => continue,
        };
        let name_bytes = name_wide.len() * 2;
        let header_size = std::mem::size_of::<ffi::FSP_FSCTL_DIR_INFO>();
        let total = header_size + name_bytes;
        let mut scratch = vec![0u8; total];
        unsafe {
            let di = scratch.as_mut_ptr() as *mut ffi::FSP_FSCTL_DIR_INFO;
            (*di).Size = total as u16;
            fill_file_info(attr, &mut (*di).FileInfo);
            let name_dst = scratch.as_mut_ptr().add(header_size) as *mut u16;
            std::ptr::copy_nonoverlapping(name_wide.as_ptr(), name_dst, name_wide.len());
            let cont =
                ffi::FspFileSystemAddDirInfo(di, buffer, length, p_bytes_transferred);
            if cont == 0 {
                return ffi::STATUS_SUCCESS;
            }
        }
    }

    unsafe {
        ffi::FspFileSystemAddDirInfo(ptr::null_mut(), buffer, length, p_bytes_transferred);
    }
    ffi::STATUS_SUCCESS
}

// ---------------------------------------------------------------------------
// Helpers: drive letter picking
// ---------------------------------------------------------------------------

fn pick_free_drive_letter() -> Option<char> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    let mask = unsafe { GetLogicalDrives() };
    for b in (b'D'..=b'Z').rev() {
        let bit = 1u32 << (b - b'A');
        if mask & bit == 0 {
            return Some(b as char);
        }
    }
    None
}

/// Ensure `winfsp-x64.dll` is loaded before any delay-loaded import
/// is called. WinFSP's installer does not add its `bin\` directory
/// to `PATH`, so the normal DLL search fails. We mirror the loader
/// logic of WinFSP's own `FspLoad` helper: resolve the install dir
/// (via the `WINFSP_PATH` env var, otherwise the default install
/// location) and `LoadLibraryW` the full DLL path. Once the module
/// is pinned in memory the delay-load stubs bind successfully on
/// first call.
fn ensure_winfsp_loaded() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let install: PathBuf = std::env::var_os("WINFSP_PATH")
            .unwrap_or_else(|| OsString::from(r"C:\Program Files (x86)\WinFsp"))
            .into();
        let dll_name = match std::env::consts::ARCH {
            "x86" => "winfsp-x86.dll",
            "aarch64" => "winfsp-a64.dll",
            _ => "winfsp-x64.dll",
        };
        let dll_path = install.join("bin").join(dll_name);
        let wide: Vec<u16> = dll_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // Best-effort: a null return here will surface as a later
        // `FspFileSystemCreate` failure with a clearer error path.
        unsafe {
            ffi::LoadLibraryW(wide.as_ptr());
        }
    });
}
