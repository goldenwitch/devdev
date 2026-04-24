//! User-surface scenarios for DevDev. Each `#[tokio::test]` here
//! pairs 1:1 with a Markdown file in `../catalog/` of the same
//! ID. The pairing is enforced by `integrity.rs`.
//!
//! Scenarios spawn the real `devdev` binary via the
//! `devdev_scenarios` harness. No engine internals are constructed
//! here.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use devdev_scenarios::{CheckpointProjection, DaemonProcess, DirSnapshot};
use tempfile::TempDir;

/// Layout owned by a scenario:
///
/// * `outer` — our private scratch root (a `TempDir`). We snapshot
///   this before and after and assert any change lives inside
///   `data_dir`. Using a nested tempdir (instead of snapshotting
///   `std::env::temp_dir()`) avoids permission issues on Windows
///   where the parent temp root contains other users' files.
/// * `data_dir` — the path passed as `--data-dir` to `devdev up`.
struct Scratch {
    outer: TempDir,
    data_dir: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let outer = tempfile::tempdir().expect("outer tempdir");
        let data_dir = outer.path().join("devdev-home");
        fs::create_dir(&data_dir).expect("create data_dir");
        Scratch { outer, data_dir }
    }

    fn outer(&self) -> &Path {
        self.outer.path()
    }
}

/// Assert that between two snapshots of the outer scratch, every
/// added or removed path lives inside `data_dir`. Anything outside
/// is a host-isolation violation.
fn assert_confined(outer: &Path, data_dir: &Path, before: &DirSnapshot, after: &DirSnapshot) {
    let data_rel = data_dir.strip_prefix(outer).expect("data_dir inside outer");
    let (added, removed) = before.diff(after);
    let leaks_added: Vec<_> = added
        .into_iter()
        .filter(|p| !p.starts_with(data_rel))
        .collect();
    let leaks_removed: Vec<_> = removed
        .into_iter()
        .filter(|p| !p.starts_with(data_rel))
        .collect();
    assert!(
        leaks_added.is_empty() && leaks_removed.is_empty(),
        "host isolation violated (outside {data_rel:?}):\n  +{leaks_added:?}\n  -{leaks_removed:?}"
    );
}

// ── S01 ─────────────────────────────────────────────────────────

/// S01 — Empty workspace up and down.
/// See: crates/devdev-scenarios/catalog/S01-empty-workspace-up-and-down.md
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s01_empty_workspace_up_and_down() {
    let scratch = Scratch::new();
    let before = DirSnapshot::capture(scratch.outer()).expect("snapshot before");

    let mut daemon = DaemonProcess::spawn(&scratch.data_dir, false).expect("devdev up");

    // Status response has the documented shape.
    let status = daemon.status().await.expect("status");
    assert!(
        status.get("tasks").is_some() && status.get("sessions").is_some(),
        "status missing documented keys: {status}"
    );

    daemon.shutdown().expect("devdev down clean exit");

    // Lifecycle files cleaned up.
    assert!(
        !scratch.data_dir.join("daemon.pid").exists(),
        "daemon.pid not removed"
    );
    assert!(
        !scratch.data_dir.join("daemon.port").exists(),
        "daemon.port not removed"
    );

    // Checkpoint written and decodes to an empty-workspace MemFs
    // (just the root `/` directory, no files, no mounts).
    let cp_path = scratch.data_dir.join("checkpoint.bin");
    assert!(cp_path.exists(), "checkpoint.bin missing after down");
    let proj = CheckpointProjection::from_bytes(&fs::read(&cp_path).expect("read checkpoint"))
        .expect("decode checkpoint");
    assert_eq!(
        proj.paths,
        vec!["/".to_string()],
        "fresh checkpoint should contain only the root dir"
    );
    assert!(
        proj.file_hashes.is_empty(),
        "fresh checkpoint should have no files, got {:?}",
        proj.file_hashes
    );
    assert!(
        proj.mounts.is_empty(),
        "fresh checkpoint should have no mounts, got {:?}",
        proj.mounts
    );

    let after = DirSnapshot::capture(scratch.outer()).expect("snapshot after");
    assert_confined(scratch.outer(), &scratch.data_dir, &before, &after);
}

// ── S05 ─────────────────────────────────────────────────────────

/// S05 — Teardown leaves nothing.
/// See: crates/devdev-scenarios/catalog/S05-teardown-leaves-nothing.md
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s05_teardown_leaves_nothing() {
    let scratch = Scratch::new();
    let before = DirSnapshot::capture(scratch.outer()).expect("snapshot before");

    let mut daemon = DaemonProcess::spawn(&scratch.data_dir, false).expect("devdev up");
    let _ = daemon.status().await.expect("status");
    daemon.shutdown().expect("devdev down");

    // Strictly enumerate what's left in the data dir.
    let remaining: BTreeSet<String> = fs::read_dir(&scratch.data_dir)
        .expect("read data_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    let expected: BTreeSet<String> = ["checkpoint.bin"].iter().map(|s| s.to_string()).collect();
    let unexpected: Vec<_> = remaining.difference(&expected).cloned().collect();
    assert!(
        unexpected.is_empty(),
        "data dir has unexpected leftovers: {unexpected:?}"
    );

    let after = DirSnapshot::capture(scratch.outer()).expect("snapshot after");
    assert_confined(scratch.outer(), &scratch.data_dir, &before, &after);
}

// ── S06 ─────────────────────────────────────────────────────────

/// S06 — Checkpoint round-trip.
/// See: crates/devdev-scenarios/catalog/S06-checkpoint-round-trip.md
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s06_checkpoint_round_trip() {
    let scratch = Scratch::new();

    // Phase 1: start fresh, capture a baseline checkpoint.
    let mut d1 = DaemonProcess::spawn(&scratch.data_dir, false).expect("devdev up (1)");
    let status1 = d1.status().await.expect("status 1");
    d1.shutdown().expect("devdev down (1)");

    let cp_path = scratch.data_dir.join("checkpoint.bin");
    let bytes1 = fs::read(&cp_path).expect("read checkpoint 1");
    let proj1 = CheckpointProjection::from_bytes(&bytes1).expect("decode 1");

    // Phase 2: resume from checkpoint, immediately checkpoint again.
    let mut d2 = DaemonProcess::spawn(&scratch.data_dir, true).expect("devdev up --checkpoint");
    let status2 = d2.status().await.expect("status 2");
    d2.shutdown().expect("devdev down (2)");

    let bytes2 = fs::read(&cp_path).expect("read checkpoint 2");
    let proj2 = CheckpointProjection::from_bytes(&bytes2).expect("decode 2");

    // The projection (paths + hashes + mounts) must round-trip
    // byte-identically even if the raw checkpoint bytes differ
    // (e.g., timestamp drift in metadata).
    assert!(
        proj1.equals(&proj2),
        "checkpoint projection changed across round-trip\n  before: {proj1:?}\n  after:  {proj2:?}"
    );

    // The shape of status is stable across restarts.
    assert_eq!(
        status1.get("tasks").and_then(|v| v.as_u64()),
        status2.get("tasks").and_then(|v| v.as_u64()),
        "tasks count changed across checkpoint: {status1} vs {status2}"
    );
}

// ── S06b: checkpoint-missing fallback ──────────────────────────

/// S06 corollary — `--checkpoint` with no `checkpoint.bin` must
/// behave like a fresh start, not error. Covered inline rather
/// than as its own catalog entry because it's a narrow guard
/// around a single branch in `Daemon::start`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s06_checkpoint_missing_is_fresh_start() {
    let scratch = Scratch::new();

    let mut daemon =
        DaemonProcess::spawn(&scratch.data_dir, true).expect("devdev up --checkpoint on empty dir");
    let _ = daemon.status().await.expect("status");
    daemon.shutdown().expect("devdev down");
}
