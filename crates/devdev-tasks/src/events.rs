//! Internal daemon event bus — see also `devdev-daemon/src/events.rs`
//! historical home.
//!
//! Lives in `devdev-tasks` because tasks need to publish events
//! without taking a daemon dependency. The bus is a thin
//! `tokio::sync::broadcast` wrapper — see [`EventBus::publish`].

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonEvent {
    PrOpened {
        owner: String,
        repo: String,
        number: u64,
        head_sha: String,
    },
    PrUpdated {
        owner: String,
        repo: String,
        number: u64,
        head_sha: String,
    },
    PrClosed {
        owner: String,
        repo: String,
        number: u64,
        merged: bool,
    },
}

impl DaemonEvent {
    /// `(owner, repo, number)` — subscribers filter the broadcast on
    /// this tuple to scope to a single PR.
    pub fn pr_target(&self) -> Option<(&str, &str, u64)> {
        match self {
            DaemonEvent::PrOpened {
                owner,
                repo,
                number,
                ..
            }
            | DaemonEvent::PrUpdated {
                owner,
                repo,
                number,
                ..
            }
            | DaemonEvent::PrClosed {
                owner,
                repo,
                number,
                ..
            } => Some((owner.as_str(), repo.as_str(), *number)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<DaemonEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx }
    }

    /// Publish an event. Returns the number of receivers reached.
    /// A bus with no subscribers is normal, not a failure.
    pub fn publish(&self, event: DaemonEvent) -> usize {
        self.tx.send(event).unwrap_or(0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.tx.subscribe()
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(n: u64) -> DaemonEvent {
        DaemonEvent::PrOpened {
            owner: "o".into(),
            repo: "r".into(),
            number: n,
            head_sha: format!("sha{n}"),
        }
    }

    #[tokio::test]
    async fn publish_no_subscribers_is_ok() {
        let bus = EventBus::new();
        assert_eq!(bus.publish(opened(1)), 0);
    }

    #[tokio::test]
    async fn one_subscriber_receives() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(opened(1));
        let got = rx.recv().await.unwrap();
        assert_eq!(got, opened(1));
    }

    #[tokio::test]
    async fn two_subscribers_each_receive() {
        let bus = EventBus::new();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.publish(opened(1));
        assert_eq!(a.recv().await.unwrap(), opened(1));
        assert_eq!(b.recv().await.unwrap(), opened(1));
    }

    #[tokio::test]
    async fn pr_target_extracts_tuple() {
        let e = opened(42);
        assert_eq!(e.pr_target(), Some(("o", "r", 42)));
        let c = DaemonEvent::PrClosed {
            owner: "o".into(),
            repo: "r".into(),
            number: 42,
            merged: true,
        };
        assert_eq!(c.pr_target(), Some(("o", "r", 42)));
    }
}
