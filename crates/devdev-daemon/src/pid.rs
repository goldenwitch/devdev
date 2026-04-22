//! PID file management and single-instance guard.

use std::path::Path;

const PID_FILENAME: &str = "daemon.pid";

/// Write the current process PID to `data_dir/daemon.pid`.
pub fn write_pid(data_dir: &Path) -> std::io::Result<()> {
    let pid = std::process::id();
    std::fs::write(data_dir.join(PID_FILENAME), pid.to_string())
}

/// Read the PID from the PID file. Returns `None` if the file doesn't exist.
pub fn read_pid(data_dir: &Path) -> std::io::Result<Option<u32>> {
    let path = data_dir.join(PID_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(content.trim().parse().ok())
}

/// Remove the PID file.
pub fn remove_pid(data_dir: &Path) -> std::io::Result<()> {
    let path = data_dir.join(PID_FILENAME);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Check if a process with the given PID is alive.
pub fn is_alive(pid: u32) -> bool {
    is_alive_platform(pid)
}

#[cfg(windows)]
fn is_alive_platform(pid: u32) -> bool {
    use std::process::Command;
    // Use tasklist to check if PID exists. This is a simple heuristic.
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            // tasklist outputs "INFO: No tasks are running..." if PID not found
            !stdout.contains("No tasks")
        })
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_alive_platform(pid: u32) -> bool {
    // Use `kill -0` to check if process exists without sending a signal.
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
