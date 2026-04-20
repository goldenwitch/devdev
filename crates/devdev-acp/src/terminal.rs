//! Dedicated worker thread for a [`ShellSession`].
//!
//! `ShellSession` transitively holds `Arc<Mutex<dyn VirtualGit>>`, and
//! `dyn VirtualGit` is deliberately neither `Send` nor `Sync` because
//! `git2::Repository` wraps a raw libgit2 pointer. That makes it
//! impossible to hand a `ShellSession` to `tokio::task::spawn_blocking`
//! or to hold a lock guard across an `.await` in a `Send` future.
//!
//! [`ShellWorker`] sidesteps the constraint: the session is pinned to
//! one OS thread for its entire life. Callers send `ShellCommand`s
//! through an `mpsc` channel and receive the `ShellResult` on a
//! `oneshot`. The worker exits cleanly when the command sender is
//! dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use devdev_shell::{ShellResult, ShellSession};
use tokio::sync::{mpsc, oneshot};

/// One unit of work for the worker: a command to execute and a channel
/// to report back on.
pub(crate) struct ShellJob {
    pub command: String,
    pub reply: oneshot::Sender<ShellResult>,
}

/// Shared inner state. Wrapped in `Arc` so [`ShellWorker`] is cheap to
/// clone while the worker thread is joined exactly once at the final
/// drop.
struct Inner {
    tx: Option<mpsc::Sender<ShellJob>>,
    alive: AtomicBool,
    join: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Close the channel so the worker's blocking_recv returns None.
        self.tx.take();
        if let Ok(mut slot) = self.join.lock()
            && let Some(handle) = slot.take()
        {
            let _ = handle.join();
        }
    }
}

/// Handle to a worker thread that owns a single [`ShellSession`].
///
/// Clone is cheap (shares an `Arc`). The worker thread is spawned once
/// at [`ShellWorker::spawn`] and runs until the last clone is dropped.
#[derive(Clone)]
pub struct ShellWorker {
    inner: Arc<Inner>,
}

impl ShellWorker {
    /// Spawn a worker thread, construct the `ShellSession` **on that
    /// thread**, and return a handle.
    ///
    /// The session is built on the worker thread (not moved in) because
    /// `dyn VirtualGit` is intentionally `!Send` — libgit2 repositories
    /// wrap raw pointers. Keeping construction and use pinned to one
    /// thread for the session's lifetime keeps every safety invariant
    /// intact without `unsafe impl Send`.
    ///
    /// `channel_depth` bounds the in-flight job queue; exceeding it
    /// backpressures the caller.
    pub fn spawn<F>(build: F, channel_depth: usize) -> Self
    where
        F: FnOnce() -> ShellSession + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel::<ShellJob>(channel_depth.max(1));
        let alive = AtomicBool::new(true);

        let inner = Arc::new(Inner {
            tx: Some(tx),
            alive,
            join: std::sync::Mutex::new(None),
        });

        let alive_thread = inner.clone();
        let join = thread::Builder::new()
            .name("devdev-shell-worker".to_owned())
            .spawn(move || {
                let mut session = build();
                while let Some(job) = rx.blocking_recv() {
                    let result = session.execute(&job.command);
                    let _ = job.reply.send(result);
                }
                alive_thread.alive.store(false, Ordering::Release);
            })
            .expect("failed to spawn devdev-shell-worker thread");

        *inner.join.lock().expect("worker join slot poisoned") = Some(join);

        Self { inner }
    }

    /// Submit a command and await the result. Returns `None` if the
    /// worker has shut down (channel closed).
    pub async fn execute(&self, command: String) -> Option<ShellResult> {
        let tx = self.inner.tx.as_ref()?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(ShellJob {
            command,
            reply: reply_tx,
        })
        .await
        .ok()?;
        reply_rx.await.ok()
    }

    /// True while the worker thread is still running.
    pub fn is_alive(&self) -> bool {
        self.inner.alive.load(Ordering::Acquire)
    }
}
