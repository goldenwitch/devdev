//! WASM runtime engine and tool registry for DevDev sandbox.
//!
//! Manages Wasmtime-based execution of WASI tools against the virtual filesystem.

pub mod engine;
mod native;
pub mod registry;

pub use engine::{WasmEngine, WasmError, WasmRunConfig, WasmRunResult};
pub use registry::{RegistryError, ToolEngine, ToolResult, WasmToolRegistry};
