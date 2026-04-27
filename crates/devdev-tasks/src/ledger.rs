//! Idempotency ledger trait — shared abstraction for "have we seen
//! this state before?"
//!
//! See [`crates/devdev-daemon/src/ledger.rs`](../../../devdev-daemon/src/ledger.rs)
//! for the file-backed implementation. The trait lives here because
//! tasks (which can't depend on the daemon crate) consume it.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Logical key into the ledger. Equality is by all four fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LedgerKey {
    pub adapter: String,
    pub resource_type: String,
    pub resource_id: String,
    pub state_hash: String,
}

impl LedgerKey {
    pub fn new(
        adapter: impl Into<String>,
        resource_type: impl Into<String>,
        resource_id: impl Into<String>,
        state_hash: impl Into<String>,
    ) -> Self {
        Self {
            adapter: adapter.into(),
            resource_type: resource_type.into(),
            resource_id: resource_id.into(),
            state_hash: state_hash.into(),
        }
    }
}

/// Backend-agnostic ledger errors.
#[derive(thiserror::Error, Debug)]
pub enum LedgerError {
    #[error("ledger I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ledger format error: {0}")]
    Format(String),
}

/// Abstract durable "have we seen this state?" store.
pub trait IdempotencyLedger: Send + Sync {
    fn seen(&self, key: &LedgerKey) -> Result<bool, LedgerError>;
    fn record(&self, key: &LedgerKey, metadata: serde_json::Value) -> Result<(), LedgerError>;
    fn prune(&self, older_than: Duration) -> Result<usize, LedgerError>;
}
