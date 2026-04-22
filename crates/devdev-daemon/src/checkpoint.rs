//! Atomic checkpoint file writes.

use std::path::Path;

/// Write data atomically: write to `.tmp`, then rename to target.
/// Prevents corruption if the process is killed mid-write.
pub fn atomic_write(target: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, target)?;
    Ok(())
}
