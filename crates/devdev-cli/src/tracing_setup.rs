//! Install the global `tracing` subscriber for the `devdev` binary.
//!
//! Two sinks are multiplexed through `tracing_subscriber::registry()`:
//!
//! * **stderr** — `WARN` by default, `DEBUG` under `--verbose`.
//! * **file** (optional) — `TRACE` regardless of `--verbose`; only
//!   installed when `--trace-file <path>` was supplied.
//!
//! `RUST_LOG` is honoured as an override on top of the stderr layer —
//! operators can crank individual modules up without rebuilding.
//!
//! The module is named `tracing_setup` (not `tracing`) so it does not
//! shadow the `tracing` crate at the cli crate root.

use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer as _, Registry};

/// Install the global subscriber. Returns an opaque guard that keeps
/// the file-writer alive — drop it at the end of `main` so TRACE
/// output is flushed on a clean exit.
///
/// Calling this twice in one process panics (the underlying
/// `set_global_default` check).
pub fn init(verbose: bool, trace_file: Option<&Path>) -> TracingGuard {
    let stderr_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(if verbose { "debug" } else { "warn" })
    });
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_ansi(false)
        .with_filter(stderr_filter)
        .boxed();

    let (file_layer, file_handle) = match trace_file {
        Some(path) => match File::create(path) {
            Ok(file) => {
                let shared = Arc::new(Mutex::new(file));
                let writer_for_layer = shared.clone();
                let layer = fmt::layer()
                    .with_writer(move || MutexWriter(writer_for_layer.clone()))
                    .with_target(true)
                    .with_ansi(false)
                    .with_filter(
                        EnvFilter::try_new("trace")
                            .expect("literal filter always parses"),
                    )
                    .boxed();
                (Some(layer), Some(shared))
            }
            Err(e) => {
                eprintln!(
                    "devdev: warning: could not open --trace-file {}: {e}",
                    path.display()
                );
                (None, None)
            }
        },
        None => (None, None),
    };

    Registry::default()
        .with(stderr_layer)
        .with(file_layer)
        .init();

    TracingGuard { _file: file_handle }
}

/// Opaque RAII handle returned by [`init`]. Keeps the trace-file writer
/// alive for the duration of `main`.
pub struct TracingGuard {
    _file: Option<Arc<Mutex<File>>>,
}

/// Emit the one-time `acp::init` record expected by AC-07.
pub fn emit_startup_banner(protocol_version: u32, agent: &str) {
    tracing::info!(
        target: "acp::init",
        protocol_version,
        agent,
        "acp subscriber installed",
    );
}

/// `std::io::Write` shim over `Arc<Mutex<File>>`. Each record locks,
/// writes, and releases — fine for a low-volume diagnostic stream.
struct MutexWriter(Arc<Mutex<File>>);

impl std::io::Write for MutexWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut g = self
            .0
            .lock()
            .map_err(|_| std::io::Error::other("trace-file mutex poisoned"))?;
        g.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut g = self
            .0
            .lock()
            .map_err(|_| std::io::Error::other("trace-file mutex poisoned"))?;
        g.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_type_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<TracingGuard>();
    }
}
