//! Shell parser, builtins, and pipeline executor for DevDev sandbox.
//!
//! Parses bash-like command strings into ASTs and executes them
//! against the virtual filesystem, WASM tools, and git engine.

pub mod ast;
pub mod builtins;
pub mod dispatch;
pub mod error;
pub mod executor;
pub mod expand;
pub mod parser;
pub mod session;
pub mod state;
pub mod tokenizer;

// Re-exports for convenience.
pub use ast::{Command, CommandList, Operator, Pipeline, Redirect, RedirectKind, Word, WordPart};
pub use builtins::{try_builtin, BuiltinResult};
pub use dispatch::{dispatch, DispatchCtx, DispatchOutput};
pub use error::ParseError;
pub use executor::{execute, ShellResult};
pub use expand::{expand_word, expand_words};
pub use parser::parse;
pub use session::ShellSession;
pub use state::ShellState;
