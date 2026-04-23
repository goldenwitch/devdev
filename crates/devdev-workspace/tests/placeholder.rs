//! Placeholder so the Windows test binary isn't empty. Linux-only
//! tests live in `fuse_mount.rs`.

#[cfg(not(target_os = "linux"))]
#[test]
fn windows_placeholder() {}
