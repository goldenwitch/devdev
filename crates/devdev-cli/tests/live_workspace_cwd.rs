//! PoC: does Copilot CLI tolerate a mounted virtual-FS path as its
//! session cwd?
//!
//! This is Phase 1 of the skeptic-proof VFS plan. We mount a real
//! `Workspace`, hand the mount path to `AcpSessionBackend` as cwd,
//! ask Copilot to write a distinctive file in its current directory,
//! and verify the file appears **both** at the host mount path (what
//! the kernel sees) and in the in-memory `Fs` directly (what our
//! driver is serving). If both agree, the cwd works and a skeptic
//! can't wave it away as a host write.
//!
//! Outcome of this PoC decides whether the real wiring uses the
//! mount root (e.g. `Z:\`) as cwd or a pre-seeded subdir like
//! `Z:\workspace`.
//!
//! Gated identically to `live_mcp.rs`: `--ignored` plus
//! `DEVDEV_LIVE_COPILOT=1`. Requires a signed-in `copilot` on PATH
//! and (on Windows) WinFSP installed.

use std::sync::Arc;
use std::time::Duration;

use devdev_cli::acp_backend::AcpSessionBackend;
use devdev_daemon::router::SessionBackend;
use devdev_workspace::Workspace;

fn live_enabled() -> bool {
    std::env::var("DEVDEV_LIVE_COPILOT")
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[cfg(windows)]
fn which_windows(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(';') {
        for ext in &[".cmd", ".bat", ".exe"] {
            let candidate = std::path::Path::new(dir).join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

fn resolve_copilot() -> Option<String> {
    if cfg!(windows) {
        #[cfg(windows)]
        {
            return which_windows("copilot");
        }
        #[allow(unreachable_code)]
        None
    } else {
        Some("copilot".to_string())
    }
}

fn init_tracing() {
    let default_filter =
        "devdev_acp::wire=trace,devdev_workspace=debug,devdev_cli=debug,warn";
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .with_target(true)
        .try_init();
}

/// Turn a mount `Path` into a string suitable as a process cwd.
///
/// WinFSP mounts come back as `Z:` (no trailing slash). Spawning a
/// process with cwd `Z:` uses the *saved current directory on drive
/// Z*, which is not what we want — force the root by appending `\`.
/// On Linux the FUSE tempdir is already absolute and complete.
fn cwd_string_for_mount(mount: &std::path::Path) -> String {
    let s = mount.display().to_string();
    if cfg!(windows) && s.len() == 2 && s.ends_with(':') {
        format!("{s}\\")
    } else {
        s
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires live, signed-in Copilot CLI + WinFSP; run with DEVDEV_LIVE_COPILOT=1 and --ignored"]
async fn copilot_accepts_mount_root_as_cwd() {
    if !live_enabled() {
        eprintln!("skipped: DEVDEV_LIVE_COPILOT != 1");
        return;
    }
    init_tracing();

    let copilot_bin = match resolve_copilot() {
        Some(p) => p,
        None => {
            eprintln!("skipped: `copilot` not on PATH");
            return;
        }
    };
    eprintln!("[poc] using copilot binary: {copilot_bin}");

    // Mount a fresh workspace. On Windows this auto-selects a free
    // drive letter (Z: downwards); on Linux it's a tempdir.
    let mut workspace = Workspace::new();
    let mount = match workspace.mount() {
        Ok(mp) => mp,
        Err(e) => panic!("workspace mount failed (WinFSP installed?): {e}"),
    };
    let fs_handle = workspace.fs();

    // Pre-seed /workspace/ so we can compare "mount root" vs
    // "subdir of mount" as cwd candidates.
    {
        let mut fs = fs_handle.lock().expect("fs mutex poisoned");
        fs.mkdir_p(b"/workspace", 0o755).expect("mkdir /workspace");
    }

    // Out-of-process sanity: can an external listing see the mount?
    // Two probes: in-process std::fs (baseline, known to work for
    // WinFSP mounts) and a child PowerShell `Test-Path` (what a
    // subprocess sees). If they disagree, the mount is scoped too
    // narrowly for Copilot to use.
    match std::fs::read_dir(&mount) {
        Ok(entries) => {
            let names: Vec<_> = entries
                .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect();
            eprintln!(
                "[poc] in-process read_dir({}) OK: {:?}",
                mount.display(),
                names
            );
        }
        Err(e) => eprintln!("[poc] in-process read_dir({}) FAIL: {e}", mount.display()),
    }
    {
        let probe_arg = format!("Test-Path '{}\\' ", mount.display());
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &probe_arg])
            .output();
        match out {
            Ok(o) => eprintln!(
                "[poc] child PowerShell `Test-Path {}\\` status={:?} stdout={:?} stderr={:?}",
                mount.display(),
                o.status.code(),
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim(),
            ),
            Err(e) => eprintln!("[poc] child PowerShell probe failed to spawn: {e}"),
        }
    }

    // Candidate cwds to try in order.
    let root_cwd = cwd_string_for_mount(&mount);
    let subdir_cwd = {
        let mut s = root_cwd.clone();
        if !s.ends_with('\\') && !s.ends_with('/') {
            s.push(std::path::MAIN_SEPARATOR);
        }
        s.push_str("workspace");
        s
    };
    // Control: a real host tempdir. If this also fails, our Copilot
    // install is broken — separates environment problems from
    // WinFSP-specific ones.
    let control_cwd = {
        let d = std::env::temp_dir().join(format!("devdev-poc-control-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("mkdir control");
        d.display().to_string()
    };
    // Long-path form some tools require.
    let long_cwd = format!("\\\\?\\{}\\", root_cwd.trim_end_matches('\\'));
    // NTFS junction trick: make a junction on the host's real FS that
    // targets the WinFSP mount. Copilot stats the junction (which
    // lives on C:, passes validation), then any real I/O through it
    // lands in our virtual FS.
    let junction_cwd = {
        let j = std::env::temp_dir()
            .join(format!("devdev-poc-junction-{}", std::process::id()));
        let _ = std::fs::remove_dir(&j);
        let out = std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                &j.display().to_string(),
                &root_cwd.trim_end_matches('\\').to_string(),
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                eprintln!(
                    "[poc] mklink /J {} -> {}: OK",
                    j.display(),
                    root_cwd.trim_end_matches('\\'),
                );
                j.display().to_string()
            }
            Ok(o) => {
                eprintln!(
                    "[poc] mklink failed: status={:?} stdout={:?} stderr={:?}",
                    o.status.code(),
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr),
                );
                // Keep a non-existent path in the list; it will just
                // get rejected too and we'll record the data.
                j.display().to_string()
            }
            Err(e) => {
                eprintln!("[poc] mklink spawn failed: {e}");
                j.display().to_string()
            }
        }
    };
    eprintln!(
        "[poc] cwd candidates: root={root_cwd:?}, subdir={subdir_cwd:?}, \
         long={long_cwd:?}, junction={junction_cwd:?}, control={control_cwd:?}"
    );

    let nonce = format!("poc-{}", std::process::id());
    let prompt = format!(
        "Create a file named probe.txt in your current working directory \
         containing exactly the bytes `{nonce}` with no trailing newline. \
         Reply with the single word DONE when the file is written. \
         Do not create any other files."
    );

    let backend = AcpSessionBackend::new(
        copilot_bin.clone(),
        vec!["--acp".to_string(), "--allow-all-tools".to_string()],
        None,
    );

    // Try the mount root first; if Copilot refuses it, fall back to
    // the pre-seeded subdir. The outcome decides Phase 2's wiring.
    let (chosen_cwd, session_id) = {
        let mut tried = Vec::new();
        let mut result = None;
        for candidate in [&root_cwd, &subdir_cwd, &long_cwd, &junction_cwd, &control_cwd] {
            match tokio::time::timeout(
                Duration::from_secs(45),
                backend.create_session(candidate),
            )
            .await
            {
                Ok(Ok(sid)) => {
                    eprintln!("[poc] create_session accepted cwd {candidate:?} -> {sid}");
                    result = Some((candidate.clone(), sid));
                    break;
                }
                Ok(Err(e)) => {
                    eprintln!("[poc] create_session rejected {candidate:?}: {e}");
                    tried.push((candidate.clone(), format!("{e}")));
                }
                Err(_) => {
                    eprintln!("[poc] create_session timed out for {candidate:?}");
                    tried.push((candidate.clone(), "timeout".into()));
                }
            }
        }
        match result {
            Some(pair) => pair,
            None => panic!(
                "Copilot rejected every cwd candidate. Tried: {tried:?}. \
                 Conclusion: the mount is visible in-process but not to Copilot's \
                 subprocess — revisit plan."
            ),
        }
    };
    eprintln!(
        "[poc] winning cwd: {chosen_cwd:?} session {session_id}. \
         mount was {:?} (if these differ, skeptic proof below applies \
         to the control cwd only and WILL assert mismatch).",
        mount.display()
    );

    let response = tokio::time::timeout(
        Duration::from_secs(180),
        backend.send_prompt(&session_id, &prompt),
    )
    .await
    .expect("send_prompt timed out after 180s")
    .expect("send_prompt errored");

    eprintln!("[poc] agent reply: {}", response.text);
    eprintln!("[poc] stop_reason: {}", response.stop_reason);

    let _ = backend.destroy_session(&session_id).await;

    // ── Skeptic proof: two independent windows must agree ──

    // Window 1: kernel mount view. Look up the file under whichever
    // cwd Copilot accepted, since the agent was told to write to "the
    // current working directory".
    let probe_host_path = std::path::PathBuf::from(&chosen_cwd).join("probe.txt");
    let via_mount = match std::fs::read(&probe_host_path) {
        Ok(bytes) => bytes,
        Err(e) => panic!(
            "probe.txt not visible at {}: {e}. Agent reply: {:?}",
            probe_host_path.display(),
            response.text,
        ),
    };
    eprintln!(
        "[poc] via mount: {} bytes = {:?}",
        via_mount.len(),
        String::from_utf8_lossy(&via_mount),
    );

    // Window 2: direct readdir from the in-memory Fs. Derive the
    // /-rooted path from the chosen cwd.
    let fs_path_str = {
        let mount_prefix = mount.display().to_string();
        let stripped = chosen_cwd
            .strip_prefix(&mount_prefix)
            .unwrap_or("")
            .replace('\\', "/");
        let mut p = stripped.trim_end_matches('/').to_string();
        p.push_str("/probe.txt");
        if !p.starts_with('/') {
            p.insert(0, '/');
        }
        p
    };
    eprintln!("[poc] reading Fs path {fs_path_str:?}");
    let via_fs = {
        let fs = fs_handle.lock().expect("fs mutex poisoned");
        fs.read_path(fs_path_str.as_bytes())
            .expect("probe.txt missing from in-memory Fs")
    };
    eprintln!(
        "[poc] via Fs:    {} bytes = {:?}",
        via_fs.len(),
        String::from_utf8_lossy(&via_fs),
    );

    assert_eq!(
        via_mount, via_fs,
        "mount view and Fs view disagree — mount is not serving our Fs"
    );
    let text = String::from_utf8_lossy(&via_mount);
    assert!(
        text.contains(&nonce),
        "file doesn't contain the nonce {nonce:?}; agent may have written elsewhere. \
         content={text:?}"
    );

    // Keep the workspace alive until here; drop unmounts.
    drop(workspace);
    // Give the driver a moment to release the drive letter cleanly.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let _ = Arc::clone(&fs_handle); // keep fs_handle alive to satisfy borrow checker paranoia
}
