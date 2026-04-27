//! NDJSON file backend for [`devdev_tasks::IdempotencyLedger`].
//!
//! ## Wire format (v1)
//!
//! `<data_dir>/ledger.ndjson` — one JSON object per line:
//!
//! ```json
//! {"adapter":"github","resource_type":"pr_review","resource_id":"o/r#1","state_hash":"sha:abc","metadata":{},"recorded_at":1714003200,"tombstone":false}
//! ```
//!
//! Pruning rewrites the file with surviving entries (no in-place edits).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub use devdev_tasks::{IdempotencyLedger, LedgerError, LedgerKey};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedgerEntry {
    adapter: String,
    resource_type: String,
    resource_id: String,
    state_hash: String,
    #[serde(default)]
    metadata: serde_json::Value,
    recorded_at: u64,
    #[serde(default)]
    tombstone: bool,
}

impl LedgerEntry {
    fn key(&self) -> LedgerKey {
        LedgerKey::new(
            &self.adapter,
            &self.resource_type,
            &self.resource_id,
            &self.state_hash,
        )
    }
}

/// Append-only NDJSON file backend.
#[derive(Debug)]
pub struct NdjsonLedger {
    path: PathBuf,
    inner: Mutex<NdjsonInner>,
}

#[derive(Debug)]
struct NdjsonInner {
    index: HashMap<LedgerKey, u64>,
}

impl NdjsonLedger {
    /// Open or create the ledger file at `path`. The parent directory
    /// must already exist.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, LedgerError> {
        let path = path.into();
        let mut index = HashMap::new();
        if path.exists() {
            let f = std::fs::File::open(&path)?;
            for (lineno, line) in BufReader::new(f).lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let entry: LedgerEntry = serde_json::from_str(&line)
                    .map_err(|e| LedgerError::Format(format!("line {}: {e}", lineno + 1)))?;
                if entry.tombstone {
                    index.remove(&entry.key());
                } else {
                    index.insert(entry.key(), entry.recorded_at);
                }
            }
        }
        Ok(Self {
            path,
            inner: Mutex::new(NdjsonInner { index }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn append_entry(&self, entry: &LedgerEntry) -> Result<(), LedgerError> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let mut line =
            serde_json::to_string(entry).map_err(|e| LedgerError::Format(e.to_string()))?;
        line.push('\n');
        f.write_all(line.as_bytes())?;
        f.sync_data()?;
        Ok(())
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl IdempotencyLedger for NdjsonLedger {
    fn seen(&self, key: &LedgerKey) -> Result<bool, LedgerError> {
        let inner = self.inner.lock().expect("ledger mutex poisoned");
        Ok(inner.index.contains_key(key))
    }

    fn record(&self, key: &LedgerKey, metadata: serde_json::Value) -> Result<(), LedgerError> {
        let recorded_at = now_unix();
        let entry = LedgerEntry {
            adapter: key.adapter.clone(),
            resource_type: key.resource_type.clone(),
            resource_id: key.resource_id.clone(),
            state_hash: key.state_hash.clone(),
            metadata,
            recorded_at,
            tombstone: false,
        };
        self.append_entry(&entry)?;
        let mut inner = self.inner.lock().expect("ledger mutex poisoned");
        inner.index.insert(key.clone(), recorded_at);
        Ok(())
    }

    fn prune(&self, older_than: Duration) -> Result<usize, LedgerError> {
        let cutoff = now_unix().saturating_sub(older_than.as_secs());
        let mut inner = self.inner.lock().expect("ledger mutex poisoned");

        let survivors: Vec<(LedgerKey, u64)> = inner
            .index
            .iter()
            .filter(|(_, ts)| **ts >= cutoff)
            .map(|(k, ts)| (k.clone(), *ts))
            .collect();

        let removed = inner.index.len() - survivors.len();

        let tmp = self.path.with_extension("ndjson.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            for (k, ts) in &survivors {
                let entry = LedgerEntry {
                    adapter: k.adapter.clone(),
                    resource_type: k.resource_type.clone(),
                    resource_id: k.resource_id.clone(),
                    state_hash: k.state_hash.clone(),
                    metadata: serde_json::Value::Null,
                    recorded_at: *ts,
                    tombstone: false,
                };
                let mut line = serde_json::to_string(&entry)
                    .map_err(|e| LedgerError::Format(e.to_string()))?;
                line.push('\n');
                f.write_all(line.as_bytes())?;
            }
            f.sync_data()?;
        }
        std::fs::rename(&tmp, &self.path)?;

        inner.index = survivors.into_iter().collect();
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn key(id: &str, sha: &str) -> LedgerKey {
        LedgerKey::new("github", "pr_review", id, sha)
    }

    #[test]
    fn record_then_seen() {
        let dir = tempdir().unwrap();
        let l = NdjsonLedger::open(dir.path().join("ledger.ndjson")).unwrap();
        let k = key("o/r#1", "sha:a");
        assert!(!l.seen(&k).unwrap());
        l.record(&k, serde_json::json!({"note":"first"})).unwrap();
        assert!(l.seen(&k).unwrap());
    }

    #[test]
    fn survives_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ledger.ndjson");
        let k = key("o/r#1", "sha:a");
        {
            let l = NdjsonLedger::open(&path).unwrap();
            l.record(&k, serde_json::Value::Null).unwrap();
        }
        let l2 = NdjsonLedger::open(&path).unwrap();
        assert!(l2.seen(&k).unwrap());
    }

    #[test]
    fn distinct_keys_are_distinct() {
        let dir = tempdir().unwrap();
        let l = NdjsonLedger::open(dir.path().join("l.ndjson")).unwrap();
        let k1 = key("o/r#1", "sha:a");
        let k2 = key("o/r#1", "sha:b");
        l.record(&k1, serde_json::Value::Null).unwrap();
        assert!(l.seen(&k1).unwrap());
        assert!(!l.seen(&k2).unwrap());
    }

    #[test]
    fn record_is_idempotent() {
        let dir = tempdir().unwrap();
        let l = NdjsonLedger::open(dir.path().join("l.ndjson")).unwrap();
        let k = key("o/r#1", "sha:a");
        l.record(&k, serde_json::Value::Null).unwrap();
        l.record(&k, serde_json::Value::Null).unwrap();
        assert!(l.seen(&k).unwrap());
    }

    #[test]
    fn prune_removes_old_only() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("l.ndjson");
        let l = NdjsonLedger::open(&path).unwrap();
        let k_old = key("o/r#1", "sha:old");
        let k_new = key("o/r#2", "sha:new");
        l.append_entry(&LedgerEntry {
            adapter: k_old.adapter.clone(),
            resource_type: k_old.resource_type.clone(),
            resource_id: k_old.resource_id.clone(),
            state_hash: k_old.state_hash.clone(),
            metadata: serde_json::Value::Null,
            recorded_at: 0,
            tombstone: false,
        })
        .unwrap();
        drop(l);
        let l = NdjsonLedger::open(&path).unwrap();
        l.record(&k_new, serde_json::Value::Null).unwrap();
        assert!(l.seen(&k_old).unwrap());
        assert!(l.seen(&k_new).unwrap());

        let removed = l.prune(Duration::from_secs(60)).unwrap();
        assert_eq!(removed, 1);
        assert!(!l.seen(&k_old).unwrap());
        assert!(l.seen(&k_new).unwrap());

        drop(l);
        let l = NdjsonLedger::open(&path).unwrap();
        assert!(!l.seen(&k_old).unwrap());
        assert!(l.seen(&k_new).unwrap());
    }

    #[test]
    fn empty_file_loads_clean() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("l.ndjson");
        std::fs::File::create(&path).unwrap();
        let l = NdjsonLedger::open(&path).unwrap();
        assert!(!l.seen(&key("o/r#1", "sha:a")).unwrap());
    }

    #[test]
    fn malformed_line_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("l.ndjson");
        std::fs::write(&path, "not json\n").unwrap();
        let err = NdjsonLedger::open(&path).unwrap_err();
        assert!(matches!(err, LedgerError::Format(_)));
    }

    #[test]
    fn concurrent_records_dedup() {
        use std::sync::Arc;
        use std::thread;

        let dir = tempdir().unwrap();
        let l = Arc::new(NdjsonLedger::open(dir.path().join("l.ndjson")).unwrap());
        let mut handles = vec![];
        for i in 0..16 {
            let l = Arc::clone(&l);
            handles.push(thread::spawn(move || {
                let k = key(&format!("o/r#{i}"), "sha:x");
                l.record(&k, serde_json::Value::Null).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        for i in 0..16 {
            assert!(l.seen(&key(&format!("o/r#{i}"), "sha:x")).unwrap());
        }
    }
}
