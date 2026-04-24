//! Acceptance tests for P2-06 — Session Router.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use devdev_daemon::router::{
    AgentResponse, ResponseChunk, RouterError, SessionBackend, SessionContext, SessionRouter,
};
use tokio::sync::{Mutex, mpsc};

// ── Mock backend ───────────────────────────────────────────────

struct MockBackend {
    next_session_id: AtomicU64,
    responses: Mutex<HashMap<String, Vec<String>>>, // session_id → accumulated prompts
    destroyed: Mutex<Vec<String>>,
}

impl MockBackend {
    fn new() -> Self {
        Self {
            next_session_id: AtomicU64::new(1),
            responses: Mutex::new(HashMap::new()),
            destroyed: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl SessionBackend for MockBackend {
    async fn create_session(&self, _cwd: &str) -> Result<String, RouterError> {
        let id = format!("s-{}", self.next_session_id.fetch_add(1, Ordering::SeqCst));
        self.responses.lock().await.insert(id.clone(), Vec::new());
        Ok(id)
    }

    async fn send_prompt(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<AgentResponse, RouterError> {
        let mut responses = self.responses.lock().await;
        let prompts = responses
            .get_mut(session_id)
            .ok_or_else(|| RouterError::SessionNotFound(session_id.into()))?;
        prompts.push(text.to_string());

        Ok(AgentResponse {
            text: format!("echo: {text}"),
            stop_reason: "endTurn".into(),
        })
    }

    async fn send_prompt_streaming(
        &self,
        session_id: &str,
        text: &str,
        tx: mpsc::Sender<ResponseChunk>,
    ) -> Result<(), RouterError> {
        let mut responses = self.responses.lock().await;
        let prompts = responses
            .get_mut(session_id)
            .ok_or_else(|| RouterError::SessionNotFound(session_id.into()))?;
        prompts.push(text.to_string());

        let words: Vec<&str> = text.split_whitespace().collect();
        for word in &words {
            let _ = tx.send(ResponseChunk::Text(format!("{word} "))).await;
        }
        let _ = tx
            .send(ResponseChunk::Done {
                stop_reason: "endTurn".into(),
            })
            .await;
        Ok(())
    }

    async fn destroy_session(&self, session_id: &str) -> Result<(), RouterError> {
        self.responses.lock().await.remove(session_id);
        self.destroyed.lock().await.push(session_id.to_string());
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[tokio::test]
async fn create_session_for_task() {
    let backend = Arc::new(MockBackend::new());
    let router = SessionRouter::new(backend);

    let handle = router
        .create_session("t-1", SessionContext::default())
        .await
        .unwrap();

    assert_eq!(handle.task_id, "t-1");
    assert!(!handle.session_id().is_empty());
    assert!(router.get_session_id("t-1").await.is_some());
}

#[tokio::test]
async fn send_prompt_gets_response() {
    let backend = Arc::new(MockBackend::new());
    let router = SessionRouter::new(backend);

    let handle = router
        .create_session("t-1", SessionContext::default())
        .await
        .unwrap();

    let resp = handle.send_prompt("hello world").await.unwrap();
    assert_eq!(resp.text, "echo: hello world");
    assert_eq!(resp.stop_reason, "endTurn");
}

#[tokio::test]
async fn send_prompt_streaming_yields_chunks() {
    let backend = Arc::new(MockBackend::new());
    let router = SessionRouter::new(backend);

    let handle = router
        .create_session("t-1", SessionContext::default())
        .await
        .unwrap();

    let mut rx = handle.send_prompt_streaming("hello world").await.unwrap();

    let mut chunks = Vec::new();
    while let Some(chunk) = rx.recv().await {
        chunks.push(chunk);
    }

    // "hello" and "world" as text chunks, then Done.
    assert!(chunks.len() >= 3);
    assert!(matches!(chunks.last(), Some(ResponseChunk::Done { .. })));
}

#[tokio::test]
async fn multiple_sessions_independent() {
    let backend: Arc<dyn SessionBackend> = Arc::new(MockBackend::new());
    let router = SessionRouter::new(Arc::clone(&backend));

    let h1 = router
        .create_session("t-1", SessionContext::default())
        .await
        .unwrap();
    let h2 = router
        .create_session("t-2", SessionContext::default())
        .await
        .unwrap();

    let r1 = h1.send_prompt("from task 1").await.unwrap();
    let r2 = h2.send_prompt("from task 2").await.unwrap();

    assert_eq!(r1.text, "echo: from task 1");
    assert_eq!(r2.text, "echo: from task 2");
    assert_ne!(h1.session_id(), h2.session_id());
}

#[tokio::test]
async fn destroy_session_cleans_up() {
    let backend = Arc::new(MockBackend::new());
    let backend_ref = Arc::clone(&backend);
    let router = SessionRouter::new(backend_ref as Arc<dyn SessionBackend>);

    let _handle = router
        .create_session("t-1", SessionContext::default())
        .await
        .unwrap();

    router.destroy_session("t-1").await.unwrap();
    assert!(router.get_session_id("t-1").await.is_none());
    assert_eq!(backend.destroyed.lock().await.len(), 1);
}

#[tokio::test]
async fn destroy_nonexistent_errors() {
    let backend = Arc::new(MockBackend::new());
    let router = SessionRouter::new(backend);

    let result = router.destroy_session("t-999").await;
    assert!(matches!(result, Err(RouterError::SessionNotFound(_))));
}

#[tokio::test]
async fn interactive_session_works() {
    let backend = Arc::new(MockBackend::new());
    let router = SessionRouter::new(backend);

    let handle = router.create_interactive_session().await.unwrap();
    let resp = handle.send_prompt("chat message").await.unwrap();
    assert_eq!(resp.text, "echo: chat message");
}

#[tokio::test]
async fn duplicate_session_errors() {
    let backend = Arc::new(MockBackend::new());
    let router = SessionRouter::new(backend);

    let _h = router
        .create_session("t-1", SessionContext::default())
        .await
        .unwrap();

    let result = router
        .create_session("t-1", SessionContext::default())
        .await;
    assert!(matches!(result, Err(RouterError::SessionExists(_))));
}

#[tokio::test]
async fn context_injected_on_create() {
    let backend = Arc::new(MockBackend::new());
    let backend_ref = Arc::clone(&backend);
    let router = SessionRouter::new(backend_ref as Arc<dyn SessionBackend>);

    let ctx = SessionContext {
        system_prompt: "You are a PR reviewer".into(),
        repo_paths: vec!["org/repo".into()],
        prior_observations: vec!["PR has 3 commits".into()],
    };

    let handle = router.create_session("t-1", ctx).await.unwrap();

    // The backend should have received the context prompt.
    let responses = backend.responses.lock().await;
    let prompts = responses.get(handle.session_id()).unwrap();
    // First prompt is the context injection, echoed back.
    assert!(prompts[0].contains("PR reviewer"));
    assert!(prompts[0].contains("org/repo"));
}

#[tokio::test]
async fn crash_recovery_recreates_sessions() {
    let backend: Arc<dyn SessionBackend> = Arc::new(MockBackend::new());
    let router = SessionRouter::new(Arc::clone(&backend));

    let _h1 = router
        .create_session("t-1", SessionContext::default())
        .await
        .unwrap();
    let _h2 = router
        .create_session("t-2", SessionContext::default())
        .await
        .unwrap();

    let sessions_before = router.active_sessions().await;
    assert_eq!(sessions_before.len(), 2);

    // Simulate crash recovery.
    router.recover().await.unwrap();

    let sessions_after = router.active_sessions().await;
    assert_eq!(sessions_after.len(), 2);

    // Session IDs should have changed (new sessions were created).
    let old_ids: Vec<String> = sessions_before.iter().map(|(_, s)| s.clone()).collect();
    let new_ids: Vec<String> = sessions_after.iter().map(|(_, s)| s.clone()).collect();
    for old in &old_ids {
        assert!(
            !new_ids.contains(old),
            "session IDs should change after recovery"
        );
    }
}

#[tokio::test]
async fn concurrent_sends_from_multiple_tasks() {
    let backend = Arc::new(MockBackend::new());
    let router = Arc::new(SessionRouter::new(backend));

    // Create 5 sessions.
    let mut handles = Vec::new();
    for i in 0..5 {
        let h = router
            .create_session(&format!("t-{i}"), SessionContext::default())
            .await
            .unwrap();
        handles.push(h);
    }

    // Send prompts concurrently.
    let mut join_handles = Vec::new();
    for (i, handle) in handles.into_iter().enumerate() {
        join_handles.push(tokio::spawn(async move {
            let resp = handle.send_prompt(&format!("msg from {i}")).await.unwrap();
            (i, resp)
        }));
    }

    for jh in join_handles {
        let (i, resp) = jh.await.unwrap();
        assert_eq!(resp.text, format!("echo: msg from {i}"));
    }
}
