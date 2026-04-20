//! Deterministic NDJSON agent used by `tests/acceptance_cli.rs`.
//!
//! Speaks just enough of the ACP protocol to make `devdev eval`
//! happy: replies to `initialize` + `session/new`, drives one
//! `session/prompt` turn, then exits.
//!
//! The scenario is picked via `DEVDEV_FAKE_AGENT_SCRIPT`:
//!
//! * `"happy"` (default) — issue one `terminal/create echo hello`,
//!   stream two agent_message_chunks, reply `end_turn`.
//! * `"noop"` — no tool calls, one chunk, `end_turn`.
//!
//! The binary is pure sync `stdin`/`stdout` — no tokio, no clap — so
//! the test harness stays cheap.

use std::io::{BufRead, BufReader, Write};

use devdev_acp::protocol::{Message, Notification, Request, RequestId, Response};
use devdev_acp::types::{
    AgentCapabilities, AgentInfo, ContentBlock, InitializeResult, NewSessionResult, PromptResult,
    SessionUpdate, SessionUpdateParams, StopReason,
};

fn main() {
    let script = std::env::var("DEVDEV_FAKE_AGENT_SCRIPT").unwrap_or_else(|_| "happy".to_owned());

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let session_id = "sess-fake-1".to_owned();
    let mut next_id: u64 = 10_000;

    // ── handshake ────────────────────────────────────────────────────
    handle_initialize(&mut reader, &mut out);
    handle_session_new(&mut reader, &mut out, &session_id);

    // ── session/prompt ──────────────────────────────────────────────
    let prompt = read_request(&mut reader);
    assert_eq!(prompt.method, "session/prompt");

    match script.as_str() {
        "noop" => {
            send_chunk(&mut out, &session_id, "nothing to see here");
        }
        _ /* "happy" */ => {
            // One tool call: echo hello. Expect a success response.
            let tid = format!("term-{}", next_id);
            next_id += 1;
            send_terminal_create(&mut out, &session_id, next_id, "echo", &["hello"]);
            next_id += 1;
            let _resp = read_response(&mut reader);

            send_terminal_output(&mut out, &session_id, next_id, &tid);
            let _ = read_response(&mut reader);

            send_chunk(&mut out, &session_id, "ran tool,");
            send_chunk(&mut out, &session_id, " all good");
        }
    }

    // Conclude the prompt turn.
    write_msg(
        &mut out,
        &Message::Response(Response::success(
            prompt.id,
            serde_json::to_value(PromptResult {
                stop_reason: StopReason::EndTurn,
            })
            .unwrap(),
        )),
    );

    // Let the CLI send shutdown; swallow silently and exit.
    let _ = reader.fill_buf();
}

// ── helpers ─────────────────────────────────────────────────────────────

fn handle_initialize<R: BufRead, W: Write>(reader: &mut R, out: &mut W) {
    let req = read_request(reader);
    assert_eq!(req.method, "initialize");
    let result = serde_json::to_value(InitializeResult {
        protocol_version: 1,
        agent_info: AgentInfo {
            name: "devdev-fake-agent".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        agent_capabilities: AgentCapabilities { streaming: None },
        auth_methods: vec![],
    })
    .unwrap();
    write_msg(out, &Message::Response(Response::success(req.id, result)));
}

fn handle_session_new<R: BufRead, W: Write>(reader: &mut R, out: &mut W, session_id: &str) {
    let req = read_request(reader);
    assert_eq!(req.method, "session/new");
    let result = serde_json::to_value(NewSessionResult {
        session_id: session_id.to_owned(),
    })
    .unwrap();
    write_msg(out, &Message::Response(Response::success(req.id, result)));
}

fn send_chunk<W: Write>(out: &mut W, session_id: &str, text: &str) {
    let params = serde_json::to_value(SessionUpdateParams {
        session_id: session_id.to_owned(),
        update: SessionUpdate::AgentMessageChunk {
            content: ContentBlock { text: text.into() },
        },
    })
    .unwrap();
    write_msg(
        out,
        &Message::Notification(Notification::new("session/update", Some(params))),
    );
}

fn send_terminal_create<W: Write>(
    out: &mut W,
    session_id: &str,
    id: u64,
    command: &str,
    args: &[&str],
) {
    let params = serde_json::json!({
        "sessionId": session_id,
        "command": command,
        "args": args,
    });
    write_msg(
        out,
        &Message::Request(Request::new(
            RequestId::Number(id),
            "terminal/create",
            Some(params),
        )),
    );
}

fn send_terminal_output<W: Write>(out: &mut W, session_id: &str, id: u64, terminal_id: &str) {
    let params = serde_json::json!({
        "sessionId": session_id,
        "terminalId": terminal_id,
    });
    write_msg(
        out,
        &Message::Request(Request::new(
            RequestId::Number(id),
            "terminal/output",
            Some(params),
        )),
    );
}

fn write_msg<W: Write>(out: &mut W, msg: &Message) {
    let s = serde_json::to_string(msg).expect("serialize Message");
    writeln!(out, "{s}").expect("write stdout");
    out.flush().expect("flush stdout");
}

fn read_request<R: BufRead>(reader: &mut R) -> Request {
    match read_msg(reader) {
        Some(Message::Request(r)) => r,
        other => panic!("fake agent: expected Request, got {other:?}"),
    }
}

fn read_response<R: BufRead>(reader: &mut R) -> Response {
    match read_msg(reader) {
        Some(Message::Response(r)) => r,
        other => panic!("fake agent: expected Response, got {other:?}"),
    }
}

fn read_msg<R: BufRead>(reader: &mut R) -> Option<Message> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).expect("read_line");
    if n == 0 {
        return None;
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return read_msg(reader);
    }
    serde_json::from_str(trimmed)
        .unwrap_or_else(|e| panic!("fake agent: could not parse {trimmed:?}: {e}"))
}

