//! [`AcpHandler`] — the callback surface the client invokes when the agent
//! sends notifications or initiates requests during a prompt turn.
//!
//! This trait is intentionally dumb: it only shuttles typed ACP params to
//! a caller-provided implementation. Business logic (permission gating,
//! virtual terminals, VFS routing) lives in capability 12.

use async_trait::async_trait;

use crate::protocol::RpcError;
use crate::types::{
    CreateTerminalParams, CreateTerminalResult, KillTerminalParams, PermissionRequestParams,
    PermissionResponse, ReadTextFileParams, ReadTextFileResult, ReleaseTerminalParams,
    SessionUpdateParams, TerminalOutputParams, TerminalOutputResult, WaitForExitParams,
    WaitForExitResult, WriteTextFileParams,
};

/// Result alias — handler impls return either the typed ACP result or an
/// RPC error that will be forwarded verbatim to the agent.
pub type HandlerResult<T> = Result<T, RpcError>;

#[async_trait]
pub trait AcpHandler: Send + Sync {
    async fn on_permission_request(
        &self,
        params: PermissionRequestParams,
    ) -> HandlerResult<PermissionResponse>;

    async fn on_terminal_create(
        &self,
        params: CreateTerminalParams,
    ) -> HandlerResult<CreateTerminalResult>;

    async fn on_terminal_output(
        &self,
        params: TerminalOutputParams,
    ) -> HandlerResult<TerminalOutputResult>;

    async fn on_terminal_wait(
        &self,
        params: WaitForExitParams,
    ) -> HandlerResult<WaitForExitResult>;

    async fn on_terminal_kill(&self, params: KillTerminalParams) -> HandlerResult<()>;

    async fn on_terminal_release(&self, params: ReleaseTerminalParams) -> HandlerResult<()>;

    async fn on_fs_read(
        &self,
        params: ReadTextFileParams,
    ) -> HandlerResult<ReadTextFileResult>;

    async fn on_fs_write(&self, params: WriteTextFileParams) -> HandlerResult<()>;

    /// Notification — fire and forget.
    async fn on_session_update(&self, params: SessionUpdateParams);
}
