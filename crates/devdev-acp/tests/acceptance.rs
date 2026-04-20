//! Acceptance tests for Cap 10 — ACP Protocol Types & Serialization.
//!
//! Each test maps to one acceptance criterion from capabilities/10-acp-protocol.md.

use std::io::Cursor;

use devdev_acp::{
    Message, NdjsonReader, NdjsonWriter, Notification, Request, RequestId, Response, RpcError,
};
use devdev_acp::types::{
    ContentBlock, CreateTerminalParams, InitializeParams, ClientCapabilities,
    ClientInfo, FsCapabilities, PermissionKind, PermissionRequestParams,
    SessionUpdate, SessionUpdateParams, ToolCall, ToolCallKind, ToolCallStatus,
    ToolCallUpdate, PlanEntry,
};

/// AC: Round-trip serialize InitializeParams → JSON → deserialize → identical struct.
#[test]
fn initialize_params_roundtrip() {
    let params = InitializeParams {
        protocol_version: 1,
        client_capabilities: ClientCapabilities {
            fs: Some(FsCapabilities {
                read_text_file: true,
                write_text_file: true,
            }),
            terminal: Some(true),
        },
        client_info: ClientInfo {
            name: "devdev".into(),
            version: "0.1.0".into(),
        },
    };
    let json = serde_json::to_string(&params).unwrap();
    let roundtrip: InitializeParams = serde_json::from_str(&json).unwrap();
    assert_eq!(params, roundtrip);
    // Verify camelCase field names
    assert!(json.contains("protocolVersion"));
    assert!(json.contains("clientCapabilities"));
    assert!(json.contains("readTextFile"));
}

/// AC: AgentMessageChunk round-trip through JSON.
#[test]
fn session_update_agent_message_chunk_roundtrip() {
    let variant = SessionUpdate::AgentMessageChunk {
        content: ContentBlock { text: "hello".into() },
    };
    let json = serde_json::to_string(&variant).unwrap();
    let roundtrip: SessionUpdate = serde_json::from_str(&json).unwrap();
    assert_eq!(variant, roundtrip);
}

/// AC: AgentThoughtChunk round-trip through JSON.
#[test]
fn session_update_agent_thought_chunk_roundtrip() {
    let variant = SessionUpdate::AgentThoughtChunk {
        content: ContentBlock { text: "thinking...".into() },
    };
    let json = serde_json::to_string(&variant).unwrap();
    let roundtrip: SessionUpdate = serde_json::from_str(&json).unwrap();
    assert_eq!(variant, roundtrip);
}

/// AC: ToolCall round-trip through JSON.
#[test]
fn session_update_tool_call_roundtrip() {
    let variant = SessionUpdate::ToolCall(ToolCall {
        tool_call_id: "tc-1".into(),
        title: "Read file".into(),
        kind: ToolCallKind::Read,
        status: ToolCallStatus::Completed,
        raw_input: Some(serde_json::json!({"path": "/foo.rs"})),
    });
    let json = serde_json::to_string(&variant).unwrap();
    let roundtrip: SessionUpdate = serde_json::from_str(&json).unwrap();
    assert_eq!(variant, roundtrip);
}

/// AC: ToolCallUpdate round-trip through JSON.
#[test]
fn session_update_tool_call_update_roundtrip() {
    let variant = SessionUpdate::ToolCallUpdate(ToolCallUpdate {
        tool_call_id: "tc-1".into(),
        status: ToolCallStatus::Failed,
        output: Some("error: not found".into()),
    });
    let json = serde_json::to_string(&variant).unwrap();
    let roundtrip: SessionUpdate = serde_json::from_str(&json).unwrap();
    assert_eq!(variant, roundtrip);
}

/// AC: Plan round-trip through JSON.
#[test]
fn session_update_plan_roundtrip() {
    let variant = SessionUpdate::Plan {
        entries: vec![PlanEntry {
            title: "Step 1".into(),
            status: "done".into(),
        }],
    };
    let json = serde_json::to_string(&variant).unwrap();
    let roundtrip: SessionUpdate = serde_json::from_str(&json).unwrap();
    assert_eq!(variant, roundtrip);
}

/// AC: NdjsonWriter produces one JSON object per line, newline-terminated.
#[test]
fn ndjson_writer_one_per_line() {
    let mut buf = Vec::new();
    {
        let mut writer = NdjsonWriter::new(&mut buf);
        let msg1 = Message::Request(Request::new(1u64, "initialize", None));
        let msg2 = Message::Notification(Notification::new("session/cancel", None));
        writer.send(&msg1).unwrap();
        writer.send(&msg2).unwrap();
    }
    let output = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "Expected 2 lines, got: {output}");
    // Each line must be valid JSON
    for line in &lines {
        let _: serde_json::Value = serde_json::from_str(line).unwrap();
    }
    // Output must end with a newline
    assert!(output.ends_with('\n'));
}

/// AC: NdjsonReader parses a multi-line NDJSON stream into individual Messages.
#[test]
fn ndjson_reader_multiline() {
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"}}"#, "\n",
    );
    let cursor = Cursor::new(input.as_bytes());
    let mut reader = NdjsonReader::new(cursor);

    let msg1 = reader.recv().unwrap().unwrap();
    assert!(matches!(msg1, Message::Request(ref r) if r.method == "initialize"));

    let msg2 = reader.recv().unwrap().unwrap();
    assert!(matches!(msg2, Message::Request(ref r) if r.method == "session/new"));

    // EOF
    assert!(reader.recv().unwrap().is_none());
}

/// AC: PermissionRequestParams deserializes from ACP-spec-like JSON.
#[test]
fn permission_request_params_from_spec_json() {
    let json = r#"{
        "sessionId": "sess-123",
        "toolCall": {
            "toolCallId": "tc-42",
            "title": "Write to /foo.rs"
        },
        "options": [
            {"optionId": "allow-once", "kind": "allowOnce", "name": "Allow once"},
            {"optionId": "reject", "kind": "rejectOnce", "name": "Reject"}
        ]
    }"#;
    let params: PermissionRequestParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.session_id, "sess-123");
    assert_eq!(params.tool_call.tool_call_id, "tc-42");
    assert_eq!(params.options.len(), 2);
    assert_eq!(params.options[0].kind, PermissionKind::AllowOnce);
    assert_eq!(params.options[1].kind, PermissionKind::RejectOnce);
}

/// AC: CreateTerminalParams deserializes from ACP-spec-like JSON.
#[test]
fn create_terminal_params_from_spec_json() {
    let json = r#"{
        "sessionId": "sess-123",
        "command": "bash",
        "args": ["-c", "echo hello"],
        "cwd": "/project",
        "env": [{"name": "FOO", "value": "bar"}],
        "outputByteLimit": 65536
    }"#;
    let params: CreateTerminalParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.session_id, "sess-123");
    assert_eq!(params.command, "bash");
    assert_eq!(params.args, vec!["-c", "echo hello"]);
    assert_eq!(params.cwd, Some("/project".into()));
    assert_eq!(params.env.len(), 1);
    assert_eq!(params.env[0].name, "FOO");
    assert_eq!(params.output_byte_limit, Some(65536));
}

/// AC: Unknown fields in JSON are ignored (forward compatibility).
#[test]
fn unknown_fields_ignored() {
    let json = r#"{
        "sessionId": "sess-1",
        "command": "ls",
        "futureField": true,
        "anotherNewThing": {"nested": 42}
    }"#;
    let params: CreateTerminalParams = serde_json::from_str(json).unwrap();
    assert_eq!(params.session_id, "sess-1");
    assert_eq!(params.command, "ls");
}

/// AC: RequestId handles numeric IDs.
#[test]
fn request_id_numeric() {
    let json = r#"{"jsonrpc":"2.0","id":42,"method":"test"}"#;
    let req: Request = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, RequestId::Number(42));
}

/// AC: RequestId handles string IDs.
#[test]
fn request_id_string() {
    let json = r#"{"jsonrpc":"2.0","id":"abc-123","method":"test"}"#;
    let req: Request = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, RequestId::String("abc-123".into()));
}

/// AC: Error response with code -32000 deserializes correctly.
#[test]
fn error_response_minus_32000() {
    let json = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32000,
            "message": "Authentication required"
        }
    }"#;
    let resp: Response = serde_json::from_str(json).unwrap();
    assert!(resp.result.is_none());
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32000);
    assert_eq!(err.message, "Authentication required");
    assert!(err.data.is_none());
}

// ── Additional robustness tests ─────────────────────────────────

/// Verify SessionUpdateParams wrapping works end-to-end.
#[test]
fn session_update_params_roundtrip() {
    let params = SessionUpdateParams {
        session_id: "sess-1".into(),
        update: SessionUpdate::AgentMessageChunk {
            content: ContentBlock { text: "hi".into() },
        },
    };
    let json = serde_json::to_string(&params).unwrap();
    let roundtrip: SessionUpdateParams = serde_json::from_str(&json).unwrap();
    assert_eq!(params, roundtrip);
}

/// Verify NdjsonReader skips blank lines between messages.
#[test]
fn ndjson_reader_skips_blank_lines() {
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"a"}"#, "\n",
        "\n",
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"b"}"#, "\n",
    );
    let cursor = Cursor::new(input.as_bytes());
    let mut reader = NdjsonReader::new(cursor);

    let msg1 = reader.recv().unwrap().unwrap();
    assert!(matches!(msg1, Message::Request(ref r) if r.method == "a"));

    let msg2 = reader.recv().unwrap().unwrap();
    assert!(matches!(msg2, Message::Request(ref r) if r.method == "b"));

    assert!(reader.recv().unwrap().is_none());
}

/// Verify Response::success helper serializes with "result" and no "error".
#[test]
fn response_success_helper() {
    let ok = Response::success(1u64, serde_json::json!({"status": "ok"}));
    let json = serde_json::to_string(&ok).unwrap();
    assert!(json.contains("\"result\""));
    assert!(!json.contains("\"error\""));
}

/// Verify Response::error helper serializes with "error" and no "result".
#[test]
fn response_error_helper() {
    let err = Response::error(2u64, RpcError {
        code: -32601,
        message: "Method not found".into(),
        data: None,
    });
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("\"error\""));
    assert!(!json.contains("\"result\""));
}
