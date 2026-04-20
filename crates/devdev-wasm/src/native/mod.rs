//! Native-Rust tool implementations used as fallbacks when a WASM
//! build is impractical (e.g. `grep`, `find`, `diff`).
//!
//! Native tools share the `NativeTool` trait, which mirrors the shape of
//! the public `ToolEngine::execute` method but is an internal detail of
//! the registry — shell callers see a uniform `ToolResult` regardless of
//! backend.

use std::collections::HashMap;

use devdev_vfs::MemFs;

use crate::registry::ToolResult;

pub mod diff;
pub mod find;
pub mod grep;

pub(crate) trait NativeTool: Send + Sync {
    fn execute(
        &self,
        args: &[String],
        stdin: &[u8],
        env: &HashMap<String, String>,
        cwd: &str,
        fs: &MemFs,
    ) -> ToolResult;
}
