//! User-surface scenario harness for DevDev.
//!
//! This crate is a **dev-only** test harness. Every scenario drives
//! DevDev through the same surfaces a real user hits: the `devdev`
//! binary, the IPC protocol, checkpoint files on disk, and
//! environment variables. See
//! [`spirit/scenarios/README.md`](../../../spirit/scenarios/README.md)
//! for the full charter.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use devdev_daemon::ipc::{IpcClient, read_port};
use devdev_vfs::MemFs;
use serde::{Deserialize, Serialize};

/// Default deadline for polling the port file after spawning `devdev up`.
pub const DEFAULT_PORT_WAIT: Duration = Duration::from_secs(10);

/// Default deadline for shutdown to complete after `devdev down`.
pub const DEFAULT_SHUTDOWN_WAIT: Duration = Duration::from_secs(10);

// ── Daemon handle ──────────────────────────────────────────────

/// A running `devdev up` subprocess owned by a scenario.
///
/// Drop semantics: if a scenario panics or returns before
/// [`DaemonProcess::shutdown`] is called, the child is killed on
/// drop so tests never leak daemon processes.
pub struct DaemonProcess {
    pub data_dir: PathBuf,
    pub port: u16,
    child: Option<Child>,
}

impl DaemonProcess {
    /// Spawn `devdev up --data-dir <data_dir> --foreground --github mock`
    /// and wait for the port file to appear.
    ///
    /// The scratch directory is owned by the caller (typically via
    /// [`tempfile::TempDir`]); this function never writes outside it.
    pub fn spawn(data_dir: &Path, from_checkpoint: bool) -> Result<Self> {
        let binary = devdev_binary_path()?;

        let mut cmd = Command::new(&binary);
        cmd.arg("up")
            .arg("--data-dir")
            .arg(data_dir)
            .arg("--foreground")
            .arg("--github")
            .arg("mock");
        if from_checkpoint {
            cmd.arg("--checkpoint");
        }
        // Make sure the child inherits no DEVDEV_HOME that would
        // override --data-dir. Scenarios set everything explicitly.
        cmd.env_remove("DEVDEV_HOME");
        cmd.env_remove("DEVDEV_GITHUB_ADAPTER");

        let child = cmd
            .spawn()
            .with_context(|| format!("spawning {}", binary.display()))?;

        let mut proc = DaemonProcess {
            data_dir: data_dir.to_path_buf(),
            port: 0,
            child: Some(child),
        };

        proc.port = proc
            .wait_for_port_file(DEFAULT_PORT_WAIT)
            .context("daemon never wrote daemon.port")?;

        Ok(proc)
    }

    fn wait_for_port_file(&self, deadline: Duration) -> Result<u16> {
        let port_file = self.data_dir.join("daemon.port");
        let end = Instant::now() + deadline;
        while Instant::now() < end {
            if port_file.exists()
                && let Some(port) = read_port(&self.data_dir)
                    .context("reading port file")?
            {
                return Ok(port);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err(anyhow!(
            "timed out waiting for {}",
            port_file.display()
        ))
    }

    /// Open a fresh IPC connection to the daemon.
    pub async fn connect(&self) -> Result<IpcClient> {
        IpcClient::connect(self.port)
            .await
            .with_context(|| format!("connecting to daemon on port {}", self.port))
    }

    /// Call IPC `status` and return the parsed `result` object.
    pub async fn status(&self) -> Result<serde_json::Value> {
        let mut client = self.connect().await?;
        let resp = client.request("status", serde_json::json!({})).await?;
        if let Some(err) = resp.error {
            return Err(anyhow!("status IPC error: {}", err.message));
        }
        resp.result
            .ok_or_else(|| anyhow!("status response had no result"))
    }

    /// Ask the daemon to shut down via the `devdev down` binary and
    /// wait for the child to exit cleanly.
    ///
    /// Drives `devdev down` as a fresh subprocess so we exercise the
    /// same code path a user would. After the daemon exits, the
    /// child handle is consumed — subsequent calls are a no-op.
    pub fn shutdown(&mut self) -> Result<()> {
        if self.child.is_none() {
            return Ok(());
        }

        let binary = devdev_binary_path()?;
        let status = Command::new(&binary)
            .arg("down")
            .arg("--data-dir")
            .arg(&self.data_dir)
            .env_remove("DEVDEV_HOME")
            .status()
            .with_context(|| format!("spawning {} down", binary.display()))?;
        if !status.success() {
            return Err(anyhow!(
                "`devdev down` exited with {:?}",
                status.code()
            ));
        }

        // Wait for the `up` child to observe the shutdown flag and exit.
        let end = Instant::now() + DEFAULT_SHUTDOWN_WAIT;
        while Instant::now() < end {
            if let Some(child) = self.child.as_mut()
                && let Some(exit) = child.try_wait()?
            {
                if !exit.success() {
                    return Err(anyhow!(
                        "`devdev up` exited with {:?} after down",
                        exit.code()
                    ));
                }
                self.child = None;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        Err(anyhow!(
            "`devdev up` did not exit within {:?} after down",
            DEFAULT_SHUTDOWN_WAIT
        ))
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Best-effort kill; scenarios that exit cleanly should
            // have called shutdown() first.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Resolve the path to the built `devdev` binary.
///
/// Delegates to `assert_cmd::cargo::cargo_bin`, which walks up the
/// target dir and finds the binary by name regardless of whether
/// it was built in debug or release. This works even though the
/// `devdev` binary lives in a different package (`devdev-cli`) from
/// this crate — `CARGO_BIN_EXE_devdev` would not be set in that case.
fn devdev_binary_path() -> Result<PathBuf> {
    let path = assert_cmd::cargo::cargo_bin("devdev");
    if path.exists() {
        return Ok(path);
    }
    Err(anyhow!(
        "could not locate the `devdev` binary at {}; build with `cargo build -p devdev-cli` first",
        path.display()
    ))
}

// ── Checkpoint projection ──────────────────────────────────────

/// A stable, host-independent projection of a DevDev checkpoint
/// suitable for equality assertions across runs and across
/// implementations.
///
/// Deliberately excludes timestamps and mode bits — those round-trip
/// correctly but vary by host (umask, clock). Scenarios that need to
/// prove those round-trip should add their own projection rather
/// than widening this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointProjection {
    /// Absolute paths of every node, sorted.
    pub paths: Vec<String>,
    /// For regular files: path → SHA256 hex of content.
    pub file_hashes: BTreeMap<String, String>,
    /// Active mount points.
    pub mounts: BTreeSet<String>,
}

impl CheckpointProjection {
    /// Decode a checkpoint blob and project to the stable shape.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let fs = MemFs::deserialize(data)
            .map_err(|e| anyhow!("deserialize checkpoint: {e}"))?;
        Ok(Self::from_memfs(&fs))
    }

    /// Project an in-memory `MemFs`.
    pub fn from_memfs(fs: &MemFs) -> Self {
        use devdev_vfs::types::Node;

        let mut paths = Vec::new();
        let mut file_hashes = BTreeMap::new();

        for (path, node) in fs.tree() {
            let s = path.to_string_lossy().to_string();
            paths.push(s.clone());
            if let Node::File { content, .. } = node {
                file_hashes.insert(s, sha256_hex(content));
            }
        }
        paths.sort();

        let mounts = fs
            .mounts()
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        CheckpointProjection {
            paths,
            file_hashes,
            mounts,
        }
    }

    /// True if both projections describe the same filesystem.
    pub fn equals(&self, other: &Self) -> bool {
        self == other
    }
}

/// Minimal SHA256 without pulling a new dep — `devdev-scenarios`
/// deliberately has a narrow dep list. Implementation is the NIST
/// FIPS 180-4 test-vector-verified version used nowhere else in
/// the codebase; keep it private.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

mod sha256 {
    //! Self-contained SHA-256. ~60 lines, RFC 6234 reference.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    pub fn digest(msg: &[u8]) -> [u8; 32] {
        // Pre-processing: padding.
        let bit_len = (msg.len() as u64).wrapping_mul(8);
        let mut padded = msg.to_vec();
        padded.push(0x80);
        while padded.len() % 64 != 56 {
            padded.push(0x00);
        }
        padded.extend_from_slice(&bit_len.to_be_bytes());

        let mut h = H0;
        for chunk in padded.chunks(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    chunk[i * 4],
                    chunk[i * 4 + 1],
                    chunk[i * 4 + 2],
                    chunk[i * 4 + 3],
                ]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7)
                    ^ w[i - 15].rotate_right(18)
                    ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17)
                    ^ w[i - 2].rotate_right(19)
                    ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }

            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for i in 0..64 {
                let s1 =
                    e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let t1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 =
                    a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
            h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g);
            h[7] = h[7].wrapping_add(hh);
        }

        let mut out = [0u8; 32];
        for (i, word) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

// ── Host isolation check ───────────────────────────────────────

/// Snapshot of file paths under a directory, used to prove a
/// scenario didn't leak writes outside its scratch area.
///
/// Tracks paths only (not contents) — a leaked write of any size
/// is a scenario failure regardless of what was written.
#[derive(Debug, Clone, Default)]
pub struct DirSnapshot(pub BTreeSet<PathBuf>);

impl DirSnapshot {
    pub fn capture(root: &Path) -> Result<Self> {
        let mut out = BTreeSet::new();
        if root.exists() {
            collect(root, root, &mut out)?;
        }
        Ok(DirSnapshot(out))
    }

    /// Difference between two snapshots: (added, removed).
    pub fn diff(&self, other: &Self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let added: Vec<_> = other.0.difference(&self.0).cloned().collect();
        let removed: Vec<_> = self.0.difference(&other.0).cloned().collect();
        (added, removed)
    }
}

fn collect(
    root: &Path,
    dir: &Path,
    out: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
        out.insert(rel);
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect(root, &p, out)?;
        }
    }
    Ok(())
}

// ── Fixture helpers ────────────────────────────────────────────

/// Project root (workspace root), determined from
/// `CARGO_MANIFEST_DIR` at compile time.
pub fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <ws>/crates/devdev-scenarios; go up twice.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("CARGO_MANIFEST_DIR has two parents")
}

/// Directory holding the committed scenario fixtures.
pub fn fixtures_dir() -> PathBuf {
    workspace_root().join("spirit").join("scenarios").join("fixtures")
}
