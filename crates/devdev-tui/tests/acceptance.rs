//! Acceptance tests for P2-03 — Chat TUI & Headless Mode.

use devdev_daemon::ipc::{IpcResponse, IpcServer};
use devdev_tui::chat::{ChatHistory, ChatMessage, ChatRole};
use devdev_tui::headless::{self, HeadlessInput, HeadlessOutput};
use devdev_tui::ipc_client::{DaemonConnection, DaemonEvent};

// ── Chat history ───────────────────────────────────────────────

#[test]
fn chat_history_push_and_list() {
    let mut hist = ChatHistory::new();
    hist.push(ChatMessage::user("hello"));
    hist.push(ChatMessage::agent("hi there", true));

    assert_eq!(hist.len(), 2);
    assert_eq!(hist.messages()[0].role, ChatRole::User);
    assert_eq!(hist.messages()[1].role, ChatRole::Agent);
}

#[test]
fn chat_agent_text_streams() {
    let mut hist = ChatHistory::new();
    hist.append_agent_text("Loading");
    hist.append_agent_text(" workspace...");

    assert_eq!(hist.len(), 1);
    assert_eq!(hist.messages()[0].text, "Loading workspace...");
    assert!(!hist.messages()[0].complete);
}

#[test]
fn chat_agent_done_shows_complete() {
    let mut hist = ChatHistory::new();
    hist.append_agent_text("Working...");
    hist.complete_agent_message();

    assert!(hist.messages()[0].complete);
}

#[test]
fn chat_scroll_history() {
    let mut hist = ChatHistory::new();
    for i in 0..50 {
        hist.push(ChatMessage::user(format!("msg {i}")));
    }

    hist.scroll_up(10);
    assert_eq!(hist.scroll_offset(), 10);

    hist.scroll_down(3);
    assert_eq!(hist.scroll_offset(), 7);

    hist.scroll_to_bottom();
    assert_eq!(hist.scroll_offset(), 0);
}

// ── Headless NDJSON ────────────────────────────────────────────

#[test]
fn headless_parse_message() {
    let input = headless::parse_input(r#"{"type":"message","text":"hello"}"#).unwrap();
    match input {
        HeadlessInput::Message { text } => assert_eq!(text, "hello"),
        _ => panic!("expected Message"),
    }
}

#[test]
fn headless_parse_approval_response() {
    let input =
        headless::parse_input(r#"{"type":"approval_response","approve":true}"#).unwrap();
    match input {
        HeadlessInput::ApprovalResponse { approve } => assert!(approve),
        _ => panic!("expected ApprovalResponse"),
    }
}

#[test]
fn headless_format_agent_text() {
    let output = HeadlessOutput::AgentText {
        text: "working...".into(),
        done: false,
    };
    let json = headless::format_output(&output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "agent_text");
    assert_eq!(parsed["text"], "working...");
    assert_eq!(parsed["done"], false);
}

#[test]
fn headless_format_approval_request() {
    let output = HeadlessOutput::ApprovalRequest {
        action: "post_review".into(),
        details: serde_json::json!({"pr": 42}),
    };
    let json = headless::format_output(&output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "approval_request");
    assert_eq!(parsed["action"], "post_review");
    assert_eq!(parsed["details"]["pr"], 42);
}

#[test]
fn headless_json_schema_valid() {
    // All output variants produce valid JSON with a "type" field.
    let outputs = vec![
        HeadlessOutput::AgentText {
            text: "x".into(),
            done: true,
        },
        HeadlessOutput::AgentDone {
            full_text: "x".into(),
        },
        HeadlessOutput::ApprovalRequest {
            action: "a".into(),
            details: serde_json::json!({}),
        },
        HeadlessOutput::Status {
            message: "m".into(),
        },
        HeadlessOutput::Error {
            message: "e".into(),
        },
    ];

    for out in &outputs {
        let json = headless::format_output(out).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["type"].is_string(), "missing type field");
    }
}

// ── DaemonEvent ────────────────────────────────────────────────

#[test]
fn daemon_event_from_json_agent_text() {
    let json = serde_json::json!({"type": "agent_text", "text": "hello", "done": false});
    let event = DaemonEvent::from_json(&json).unwrap();
    match event {
        DaemonEvent::AgentText { text, done } => {
            assert_eq!(text, "hello");
            assert!(!done);
        }
        _ => panic!("expected AgentText"),
    }
}

#[test]
fn daemon_event_roundtrip() {
    let event = DaemonEvent::ApprovalRequest {
        action: "deploy".into(),
        details: serde_json::json!({"env": "prod"}),
    };
    let json = event.to_json();
    let restored = DaemonEvent::from_json(&json).unwrap();
    match restored {
        DaemonEvent::ApprovalRequest { action, details } => {
            assert_eq!(action, "deploy");
            assert_eq!(details["env"], "prod");
        }
        _ => panic!("expected ApprovalRequest"),
    }
}

#[test]
fn daemon_event_to_headless_output() {
    let event = DaemonEvent::StatusUpdate {
        message: "task created".into(),
    };
    let output: HeadlessOutput = event.into();
    let json = headless::format_output(&output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "status");
    assert_eq!(parsed["message"], "task created");
}

// ── IPC client integration ─────────────────────────────────────

#[tokio::test]
async fn ipc_client_connect_to_daemon() {
    let server = IpcServer::bind().await.unwrap();
    let port = server.port();

    // Server handler.
    let handle = tokio::spawn(async move {
        let mut conn = server.accept().await.unwrap();
        let req = conn.read_request().await.unwrap().unwrap();
        assert_eq!(req.method, "status");
        let resp = IpcResponse::ok(
            req.id,
            serde_json::json!({"running": true, "tasks": 0}),
        );
        conn.write_response(&resp).await.unwrap();
    });

    let mut client = DaemonConnection::connect_to_port(port).await.unwrap();
    let resp = client.status().await.unwrap();
    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["running"], true);

    handle.await.unwrap();
}

#[tokio::test]
async fn ipc_client_send_message() {
    let server = IpcServer::bind().await.unwrap();
    let port = server.port();

    let handle = tokio::spawn(async move {
        let mut conn = server.accept().await.unwrap();
        let req = conn.read_request().await.unwrap().unwrap();
        assert_eq!(req.method, "send");
        assert_eq!(req.params["text"], "hello");
        let resp = IpcResponse::ok(
            req.id,
            serde_json::json!({"response": "hi"}),
        );
        conn.write_response(&resp).await.unwrap();
    });

    let mut client = DaemonConnection::connect_to_port(port).await.unwrap();
    let resp = client.send_message("hello").await.unwrap();
    assert!(resp.error.is_none());

    handle.await.unwrap();
}

#[tokio::test]
async fn ipc_client_shutdown() {
    let server = IpcServer::bind().await.unwrap();
    let port = server.port();

    let handle = tokio::spawn(async move {
        let mut conn = server.accept().await.unwrap();
        let req = conn.read_request().await.unwrap().unwrap();
        assert_eq!(req.method, "shutdown");
        let resp = IpcResponse::ok(
            req.id,
            serde_json::json!({"checkpoint_saved": true}),
        );
        conn.write_response(&resp).await.unwrap();
    });

    let mut client = DaemonConnection::connect_to_port(port).await.unwrap();
    let resp = client.shutdown().await.unwrap();
    assert_eq!(resp.result.unwrap()["checkpoint_saved"], true);

    handle.await.unwrap();
}
