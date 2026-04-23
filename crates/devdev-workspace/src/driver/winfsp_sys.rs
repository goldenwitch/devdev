//! Raw FFI declarations for WinFSP (Windows only).
//!
//! Hand-written from `<winfsp/winfsp.h>` — we deliberately avoid
//! pulling in a wrapper crate (the maintained ones are GPL-3.0 and
//! we're MIT). Only the subset of types/functions used by our
//! [`super::winfsp`] driver is declared here.
//!
//! All function imports are resolved at load time against
//! `winfsp-x64.dll` (shipped with WinFSP); see `build.rs` for the
//! link config.

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code,
    clippy::upper_case_acronyms
)]

use std::ffi::c_void;

// --- Primitive aliases ------------------------------------------------------
pub type NTSTATUS = i32;
pub type BOOLEAN = u8;
pub type PVOID = *mut c_void;
pub type PWSTR = *mut u16;
pub type PCWSTR = *const u16;
pub type WCHAR = u16;
pub type UINT16 = u16;
pub type UINT32 = u32;
pub type UINT64 = u64;
pub type ULONG = u32;
pub type PULONG = *mut u32;
pub type SIZE_T = usize;
pub type PSIZE_T = *mut usize;
pub type PSECURITY_DESCRIPTOR = PVOID;
pub type SECURITY_INFORMATION = u32;

// --- NTSTATUS constants (subset) -------------------------------------------
pub const STATUS_SUCCESS: NTSTATUS = 0;
pub const STATUS_INVALID_PARAMETER: NTSTATUS = 0xC000_000Du32 as i32;
pub const STATUS_NOT_IMPLEMENTED: NTSTATUS = 0xC000_0002u32 as i32;
pub const STATUS_NO_SUCH_FILE: NTSTATUS = 0xC000_000Fu32 as i32;
pub const STATUS_OBJECT_NAME_NOT_FOUND: NTSTATUS = 0xC000_0034u32 as i32;
pub const STATUS_OBJECT_NAME_COLLISION: NTSTATUS = 0xC000_0035u32 as i32;
pub const STATUS_OBJECT_PATH_NOT_FOUND: NTSTATUS = 0xC000_003Au32 as i32;
pub const STATUS_NOT_A_DIRECTORY: NTSTATUS = 0xC000_0103u32 as i32;
pub const STATUS_FILE_IS_A_DIRECTORY: NTSTATUS = 0xC000_00BAu32 as i32;
pub const STATUS_DIRECTORY_NOT_EMPTY: NTSTATUS = 0xC000_0101u32 as i32;
pub const STATUS_NAME_TOO_LONG: NTSTATUS = 0xC000_0106u32 as i32;
pub const STATUS_IO_DEVICE_ERROR: NTSTATUS = 0xC000_0185u32 as i32;
pub const STATUS_ACCESS_DENIED: NTSTATUS = 0xC000_0022u32 as i32;
pub const STATUS_DISK_FULL: NTSTATUS = 0xC000_007Fu32 as i32;
pub const STATUS_INVALID_HANDLE: NTSTATUS = 0xC000_0008u32 as i32;
pub const STATUS_BUFFER_OVERFLOW: NTSTATUS = 0x8000_0005u32 as i32;
pub const STATUS_END_OF_FILE: NTSTATUS = 0xC000_0011u32 as i32;

// --- Windows file attribute bits we use ------------------------------------
pub const FILE_ATTRIBUTE_DIRECTORY: UINT32 = 0x10;
pub const FILE_ATTRIBUTE_NORMAL: UINT32 = 0x80;
pub const FILE_ATTRIBUTE_READONLY: UINT32 = 0x01;
pub const FILE_ATTRIBUTE_REPARSE_POINT: UINT32 = 0x400;

// Cleanup flags (set in `Cleanup` callback `Flags` param).
pub const FspCleanupDelete: ULONG = 0x01;
pub const FspCleanupSetAllocationSize: ULONG = 0x02;
pub const FspCleanupSetArchiveBit: ULONG = 0x10;
pub const FspCleanupSetLastAccessTime: ULONG = 0x20;
pub const FspCleanupSetLastWriteTime: ULONG = 0x40;
pub const FspCleanupSetChangeTime: ULONG = 0x80;

// Create disposition (low 8 bits of CreateOptions param passed to Create).
pub const FILE_DIRECTORY_FILE: UINT32 = 0x0000_0001;

// --- VolumeParams (sizeof == 504 for V1) -----------------------------------
//
// Kept as #[repr(C)] with explicit bitfield emulation: WinFSP packs a
// series of 1-bit flags into a single UINT32. We expose that as a
// single `flags: u32` and provide setter helpers. Field ordering
// must match winfsp.h exactly.
#[repr(C)]
pub struct FSP_FSCTL_VOLUME_PARAMS {
    pub Version: UINT16,
    pub SectorSize: UINT16,
    pub SectorsPerAllocationUnit: UINT16,
    pub MaxComponentLength: UINT16,
    pub VolumeCreationTime: UINT64,
    pub VolumeSerialNumber: UINT32,
    pub TransactTimeout: UINT32,
    pub IrpTimeout: UINT32,
    pub IrpCapacity: UINT32,
    pub FileInfoTimeout: UINT32,
    /// First bitfield word. Bits, LSB first:
    ///  0 CaseSensitiveSearch, 1 CasePreservedNames, 2 UnicodeOnDisk,
    ///  3 PersistentAcls, 4 ReparsePoints, 5 ReparsePointsAccessCheck,
    ///  6 NamedStreams, 7 HardLinks, 8 ExtendedAttributes,
    ///  9 ReadOnlyVolume, 10 PostCleanupWhenModifiedOnly,
    /// 11 PassQueryDirectoryPattern, 12 AlwaysUseDoubleBuffering,
    /// 13 PassQueryDirectoryFileName, 14 FlushAndPurgeOnCleanup,
    /// 15 DeviceControl, 16 UmFileContextIsUserContext2,
    /// 17 UmFileContextIsFullContext, 18 UmNoReparsePointsDirCheck,
    /// 19-23 UmReservedFlags, 24 AllowOpenInKernelMode,
    /// 25 CasePreservedExtendedAttributes, 26 WslFeatures,
    /// 27 DirectoryMarkerAsNextOffset, 28 RejectIrpPriorToTransact0,
    /// 29 SupportsPosixUnlinkRename, 30 PostDispositionWhenNecessaryOnly,
    /// 31 KmReservedFlags.
    pub flags: UINT32,
    pub Prefix: [WCHAR; 192],
    pub FileSystemName: [WCHAR; 16],
    /// V1 extension bitfield (timeouts valid flags).
    pub flags2: UINT32,
    pub VolumeInfoTimeout: UINT32,
    pub DirInfoTimeout: UINT32,
    pub SecurityTimeout: UINT32,
    pub StreamInfoTimeout: UINT32,
    pub EaTimeout: UINT32,
    pub FsextControlCode: UINT32,
    pub Reserved32: [UINT32; 1],
    pub Reserved64: [UINT64; 2],
}

// Bit accessors for FSP_FSCTL_VOLUME_PARAMS.flags
pub const FLAG_CASE_SENSITIVE_SEARCH: UINT32 = 1 << 0;
pub const FLAG_CASE_PRESERVED_NAMES: UINT32 = 1 << 1;
pub const FLAG_UNICODE_ON_DISK: UINT32 = 1 << 2;
pub const FLAG_PERSISTENT_ACLS: UINT32 = 1 << 3;
pub const FLAG_UM_FILE_CONTEXT_IS_USER_CONTEXT2: UINT32 = 1 << 16;
pub const FLAG_SUPPORTS_POSIX_UNLINK_RENAME: UINT32 = 1 << 29;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct FSP_FSCTL_VOLUME_INFO {
    pub TotalSize: UINT64,
    pub FreeSize: UINT64,
    pub VolumeLabelLength: UINT16,
    pub VolumeLabel: [WCHAR; 32],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct FSP_FSCTL_FILE_INFO {
    pub FileAttributes: UINT32,
    pub ReparseTag: UINT32,
    pub AllocationSize: UINT64,
    pub FileSize: UINT64,
    pub CreationTime: UINT64,
    pub LastAccessTime: UINT64,
    pub LastWriteTime: UINT64,
    pub ChangeTime: UINT64,
    pub IndexNumber: UINT64,
    pub HardLinks: UINT32,
    pub EaSize: UINT32,
}

#[repr(C)]
pub struct FSP_FSCTL_DIR_INFO {
    pub Size: UINT16,
    pub FileInfo: FSP_FSCTL_FILE_INFO,
    /// Union in C; we only use NextOffset. Treat as opaque padding.
    pub NextOffsetOrPadding: [u8; 24],
    /// Variable-length flexible member in C. Followed by the name
    /// bytes (UTF-16, no null terminator). Addressed via pointer
    /// arithmetic in `build_dir_info`.
    pub FileNameBuf: [WCHAR; 0],
}

// --- FSP_FILE_SYSTEM opaque handle -----------------------------------------
#[repr(C)]
pub struct FSP_FILE_SYSTEM {
    _private: [u8; 0],
}

// --- FSP_FILE_SYSTEM_INTERFACE (callback table) ----------------------------
//
// Ordering per winfsp.h. Any unused slot is NULL. Total 64 pointers
// for ABI stability.
#[repr(C)]
pub struct FSP_FILE_SYSTEM_INTERFACE {
    pub GetVolumeInfo: Option<
        unsafe extern "system" fn(*mut FSP_FILE_SYSTEM, *mut FSP_FSCTL_VOLUME_INFO) -> NTSTATUS,
    >,
    pub SetVolumeLabel: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PWSTR,
            *mut FSP_FSCTL_VOLUME_INFO,
        ) -> NTSTATUS,
    >,
    pub GetSecurityByName: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PWSTR,
            *mut UINT32,
            PSECURITY_DESCRIPTOR,
            *mut SIZE_T,
        ) -> NTSTATUS,
    >,
    pub Create: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PWSTR,
            UINT32,
            UINT32,
            UINT32,
            PSECURITY_DESCRIPTOR,
            UINT64,
            *mut PVOID,
            *mut FSP_FSCTL_FILE_INFO,
        ) -> NTSTATUS,
    >,
    pub Open: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PWSTR,
            UINT32,
            UINT32,
            *mut PVOID,
            *mut FSP_FSCTL_FILE_INFO,
        ) -> NTSTATUS,
    >,
    pub Overwrite: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            UINT32,
            BOOLEAN,
            UINT64,
            *mut FSP_FSCTL_FILE_INFO,
        ) -> NTSTATUS,
    >,
    pub Cleanup:
        Option<unsafe extern "system" fn(*mut FSP_FILE_SYSTEM, PVOID, PWSTR, ULONG) -> NTSTATUS>,
    pub Close: Option<unsafe extern "system" fn(*mut FSP_FILE_SYSTEM, PVOID) -> NTSTATUS>,
    pub Read: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            PVOID,
            UINT64,
            ULONG,
            PULONG,
        ) -> NTSTATUS,
    >,
    pub Write: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            PVOID,
            UINT64,
            ULONG,
            BOOLEAN,
            BOOLEAN,
            PULONG,
            *mut FSP_FSCTL_FILE_INFO,
        ) -> NTSTATUS,
    >,
    pub Flush: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            *mut FSP_FSCTL_FILE_INFO,
        ) -> NTSTATUS,
    >,
    pub GetFileInfo: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            *mut FSP_FSCTL_FILE_INFO,
        ) -> NTSTATUS,
    >,
    pub SetBasicInfo: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            UINT32,
            UINT64,
            UINT64,
            UINT64,
            UINT64,
            *mut FSP_FSCTL_FILE_INFO,
        ) -> NTSTATUS,
    >,
    pub SetFileSize: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            UINT64,
            BOOLEAN,
            *mut FSP_FSCTL_FILE_INFO,
        ) -> NTSTATUS,
    >,
    pub CanDelete:
        Option<unsafe extern "system" fn(*mut FSP_FILE_SYSTEM, PVOID, PWSTR) -> NTSTATUS>,
    pub Rename: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            PWSTR,
            PWSTR,
            BOOLEAN,
        ) -> NTSTATUS,
    >,
    pub GetSecurity: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            PSECURITY_DESCRIPTOR,
            *mut SIZE_T,
        ) -> NTSTATUS,
    >,
    pub SetSecurity: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR,
        ) -> NTSTATUS,
    >,
    pub ReadDirectory: Option<
        unsafe extern "system" fn(
            *mut FSP_FILE_SYSTEM,
            PVOID,
            PWSTR,
            PWSTR,
            PVOID,
            ULONG,
            PULONG,
        ) -> NTSTATUS,
    >,
    pub ResolveReparsePoints: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub GetReparsePoint: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub SetReparsePoint: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub DeleteReparsePoint: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub GetStreamInfo: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub GetDirInfoByName: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub Control: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub SetDelete: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub CreateEx: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub OverwriteEx: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub GetEa: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub SetEa: Option<unsafe extern "system" fn() -> NTSTATUS>,
    pub DispatcherStopped:
        Option<unsafe extern "system" fn(*mut FSP_FILE_SYSTEM, BOOLEAN) -> NTSTATUS>,
    /// 31 reserved function pointers at the tail; must be NULL. We
    /// hold them as a single fixed-size array.
    pub Reserved: [Option<unsafe extern "system" fn() -> NTSTATUS>; 31],
}

// --- Imported functions ----------------------------------------------------
#[link(name = "winfsp-x64")]
unsafe extern "system" {
    pub fn FspFileSystemCreate(
        DevicePath: PWSTR,
        VolumeParams: *const FSP_FSCTL_VOLUME_PARAMS,
        Interface: *const FSP_FILE_SYSTEM_INTERFACE,
        PFileSystem: *mut *mut FSP_FILE_SYSTEM,
    ) -> NTSTATUS;

    pub fn FspFileSystemDelete(FileSystem: *mut FSP_FILE_SYSTEM);

    pub fn FspFileSystemSetMountPoint(
        FileSystem: *mut FSP_FILE_SYSTEM,
        MountPoint: PWSTR,
    ) -> NTSTATUS;

    pub fn FspFileSystemRemoveMountPoint(FileSystem: *mut FSP_FILE_SYSTEM);

    pub fn FspFileSystemStartDispatcher(
        FileSystem: *mut FSP_FILE_SYSTEM,
        ThreadCount: ULONG,
    ) -> NTSTATUS;

    pub fn FspFileSystemStopDispatcher(FileSystem: *mut FSP_FILE_SYSTEM);

    /// Append a dir entry to the ReadDirectory output buffer. Pass
    /// DirInfo = NULL to signal EOF (flushes any buffered entries).
    pub fn FspFileSystemAddDirInfo(
        DirInfo: *mut FSP_FSCTL_DIR_INFO,
        Buffer: PVOID,
        Length: ULONG,
        PBytesTransferred: PULONG,
    ) -> BOOLEAN;

    pub fn FspFileSystemSetOperationGuardStrategyF(
        FileSystem: *mut FSP_FILE_SYSTEM,
        GuardStrategy: UINT32,
    );
}

// Coarse guard strategy = all ops mutually exclusive (we use our own
// Mutex anyway, but this keeps WinFSP's reentrancy model simple).
pub const FSP_FILE_SYSTEM_OPERATION_GUARD_STRATEGY_COARSE: UINT32 = 1;

/// Magic constant from winfsp.h: FILETIME delta from 1601-01-01 to
/// 1970-01-01 in 100-ns ticks.
pub const FILETIME_UNIX_EPOCH_DELTA: u64 = 116_444_736_000_000_000;

// Build a permissive self-relative security descriptor using
// `advapi32!ConvertStringSecurityDescriptorToSecurityDescriptorW`.
// Caller must `LocalFree` the returned pointer.
#[link(name = "advapi32")]
unsafe extern "system" {
    pub fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        StringSecurityDescriptor: PCWSTR,
        StringSDRevision: u32,
        SecurityDescriptor: *mut PSECURITY_DESCRIPTOR,
        SecurityDescriptorSize: *mut u32,
    ) -> i32; // BOOL
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn LocalFree(hMem: PVOID) -> PVOID;
    pub fn GetSecurityDescriptorLength(pSecurityDescriptor: PSECURITY_DESCRIPTOR) -> u32;
    pub fn LoadLibraryW(lpLibFileName: PCWSTR) -> PVOID;
}

pub const SDDL_REVISION_1: u32 = 1;
