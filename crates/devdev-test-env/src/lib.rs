//! `devdev-test-env` — provisioner for the live-test fixture environment.
//!
//! The crate exposes a small library and a binary with five
//! subcommands (`apply`, `verify`, `reset-comments`, `destroy`,
//! `print-env`). The library is the place real provisioning logic
//! lives so it can be unit-tested without an HTTP round-trip — the
//! GitHub and ADO REST clients are pluggable via the
//! [`HostClient`] traits below.
//!
//! ## Design choices
//!
//! - **JSON manifest, not Terraform.** The manifest is a plain
//!   serde struct; `manifest.lock.json` carries server-assigned
//!   ids (PR numbers) backfilled after the first `apply`.
//! - **Hand-rolled REST.** Avoids dragging in `octocrab` / Azure
//!   SDK crates and the dependency-update friction they bring.
//! - **Idempotent by construction.** Every `ensure_*` operation
//!   reads first, then writes only if state diverges. A second
//!   `apply` on a clean fixture is a no-op (asserted in tests).
//! - **Admin credentials never reach test code.** The binary uses
//!   them; tests consume the env vars `print-env` emits, which
//!   reference *separate* lower-privilege tokens.

pub mod ado;
pub mod github;
pub mod manifest;
pub mod reset;
pub mod secret;

pub use manifest::{AdoFixture, GithubFixture, Manifest, ManifestLock};
pub use secret::Token;
