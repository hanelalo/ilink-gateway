//! HTTP API server (axum).
//!
//! Endpoints:
//! - POST /api/agents/register
//! - GET  /api/agents/{name}/poll
//! - POST /api/agents/{name}/reply
//! - GET  /api/status

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::agents::ws_registry::WsRegistry;
use crate::config::GatewayConfig;
use crate::error::Result;
use crate::ilink::types::{AgentReply, AgentStatus};
use crate::router::router::Router as InternalRouter;

// ─── Request types ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReplyRequest {
    pub reply_to_id: String,
    pub text: String,
    #[serde(default)]
    pub media_paths: Vec<String>,
}

// ─── Application state ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub router: Arc<Mutex<InternalRouter>>,
    pub reply_tx: UnboundedSender<AgentReply>,
    pub ws_registry: WsRegistry,
}

// ─── Router builder ─────────────────────────────────────────────────────────

/// Build the axum Router with all API endpoints.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/agents/register", post(handle_register))
        .route("/api/agents/{name}/poll", get(handle_poll))
        .route("/api/agents/{name}/reply", post(handle_reply))
        .route("/api/status", get(handle_status))
        .route("/ws/agents/{name}", get(super::ws::ws_handler))
        .with_state(Arc::new(state))
}

/// Start the HTTP server on the configured address.
pub async fn start_server(config: &GatewayConfig, state: AppState) -> Result<()> {
    let app = build_router(state).into_make_service();
    let addr = format!("{}:{}", config.http_addr, config.http_port);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| {
            crate::error::GatewayError::Config(format!("Failed to bind {addr}: {e}"))
        })?;

    tracing::info!("HTTP server listening on {addr}");

    axum::serve(listener, app)
        .await
        .map_err(|e| {
            crate::error::GatewayError::Config(format!("HTTP server error: {e}"))
        })?;

    Ok(())
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// POST /api/agents/register
pub async fn handle_register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut router = state.router.lock().unwrap();
    let registry = router.registry_mut();

    match registry.register(&body.name, None, &body.capabilities) {
        Ok(()) => {
            let active = router.active_agent().map(|s| s.to_string());
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "active_agent": active,
                    "wechat_connected": false,
                })),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "ok": false,
                "error": e.to_string(),
            })),
        ),
    }
}

/// GET /api/agents/{name}/poll
pub async fn handle_poll(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut router = state.router.lock().unwrap();

    if !router.registry().contains(&name) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Agent '{}' not found", name)
            })),
        );
    }

    // Refresh heartbeat — agent is alive
    let _ = router.registry_mut().mark_online(&name);

    let messages = router.queue().dequeue_all(&name).unwrap_or_default();

    let agent_msgs: Vec<serde_json::Value> = messages
        .into_iter()
        .map(|m| {
            let media: Vec<serde_json::Value> = m
                .media
                .into_iter()
                .map(|item| {
                    serde_json::json!({
                        "media_type": item.media_type,
                        "local_path": item.local_path,
                        "original_name": item.original_name,
                    })
                })
                .collect();
            serde_json::json!({
                "id": m.id,
                "from_user": m.from_user,
                "text": m.text,
                "timestamp": m.timestamp,
                "context_token": m.context_token,
                "message_type": m.message_type,
                "media": media,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({"messages": agent_msgs})),
    )
}

/// POST /api/agents/{name}/reply
///
/// The reply is acknowledged here. Actual sending via iLink happens in main.rs
/// when the iLink client is available.
pub async fn handle_reply(
    State(state): State<Arc<AppState>>,
    Path(_name): Path<String>,
    Json(body): Json<ReplyRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Send reply through channel
    let reply = AgentReply {
        reply_to_id: body.reply_to_id,
        text: body.text,
        media_paths: body.media_paths,
    };
    if state.reply_tx.send(reply).is_err() {
        tracing::warn!("reply channel closed, dropping reply");
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({"ok": true})),
    )
}

/// GET /api/status
pub async fn handle_status(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let router = state.router.lock().unwrap();
    let registry = router.registry();

    let agents: serde_json::Value = {
        let list = registry.list();
        let mut map = serde_json::Map::new();
        for agent in list {
            let status_label = match agent.status {
                AgentStatus::Online => "online",
                AgentStatus::Offline => "offline",
            };
            map.insert(
                agent.name.clone(),
                serde_json::json!({
                    "status": status_label,
                    "capabilities": agent.capabilities,
                    "last_seen": agent.last_seen,
                }),
            );
        }
        serde_json::Value::Object(map)
    };

    Json(serde_json::json!({
        "wechat": { "connected": false },
        "active_agent": router.active_agent(),
        "agents": agents,
    }))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn make_state() -> AppState {
        let (reply_tx, _) = tokio::sync::mpsc::unbounded_channel();
        AppState {
            router: Arc::new(Mutex::new(InternalRouter::new(
                crate::agents::registry::AgentRegistry::new(),
                crate::agents::queue::MessageQueue::new(),
            ))),
            reply_tx,
            ws_registry: WsRegistry::new(),
        }
    }

    /// Helper: send a JSON POST request using oneshot pattern.
    /// state is Arc<AppState> so mutations persist across request handlers.
    async fn post_json(state: &Arc<AppState>, uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let app = build_router((**state).clone()).into_service();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        (status, json)
    }

    /// Helper: send a GET request using oneshot pattern.
    async fn get_json(state: &Arc<AppState>, uri: &str) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::default())
            .unwrap();
        let app = build_router((**state).clone()).into_service();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn test_register_returns_200_with_ok_true() {
        let state = Arc::new(make_state());

        let (status, json) = post_json(&state, "/api/agents/register", serde_json::json!({
            "name": "hermes",
            "capabilities": ["text"],
        }))
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn test_registered_agent_appears_in_status() {
        let state = Arc::new(make_state());

        // Register an agent
        let (status, _) = post_json(&state, "/api/agents/register", serde_json::json!({
            "name": "hermes",
            "capabilities": ["text"],
        }))
        .await;
        assert_eq!(status, StatusCode::OK);

        // Check status — same Arc<AppState> so the agent is visible
        let (_, status_json) = get_json(&state, "/api/status").await;
        assert!(status_json["agents"].as_object().unwrap().contains_key("hermes"));
        assert_eq!(status_json["agents"]["hermes"]["status"], "online");
    }

    #[tokio::test]
    async fn test_poll_returns_empty_messages_after_registration() {
        let state = Arc::new(make_state());

        // Register
        let (status, _) = post_json(&state, "/api/agents/register", serde_json::json!({
            "name": "hermes",
            "capabilities": ["text"],
        }))
        .await;
        assert_eq!(status, StatusCode::OK);

        // Poll
        let (status, json) = get_json(&state, "/api/agents/hermes/poll").await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["messages"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_reply_returns_200() {
        let state = Arc::new(make_state());

        let (status, json) = post_json(&state, "/api/agents/hermes/reply", serde_json::json!({
            "reply_to_id": "msg-1",
            "text": "hello back",
        }))
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn test_reply_sends_through_channel() {
        let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(AppState {
            router: Arc::new(Mutex::new(InternalRouter::new(
                crate::agents::registry::AgentRegistry::new(),
                crate::agents::queue::MessageQueue::new(),
            ))),
            reply_tx,
            ws_registry: WsRegistry::new(),
        });

        let (_status, _json) = post_json(&state, "/api/agents/hermes/reply", serde_json::json!({
            "reply_to_id": "msg-1",
            "text": "hello back",
            "media_paths": ["/tmp/file.pdf"],
        }))
        .await;

        // Verify the reply was sent through the channel
        let reply = reply_rx
            .try_recv()
            .expect("should have received a reply on channel");
        assert_eq!(reply.reply_to_id, "msg-1");
        assert_eq!(reply.text, "hello back");
        assert_eq!(reply.media_paths, vec!["/tmp/file.pdf"]);
    }

    #[tokio::test]
    async fn test_status_returns_valid_json_structure() {
        let state = Arc::new(make_state());

        let (status, json) = get_json(&state, "/api/status").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json.get("wechat").is_some());
        assert!(json.get("active_agent").is_some());
        assert!(json.get("agents").is_some());
        assert_eq!(json["wechat"]["connected"], false);
    }

    #[tokio::test]
    async fn test_poll_updates_last_seen() {
        let state = Arc::new(make_state());

        // Register via API
        let (status, _) = post_json(&state, "/api/agents/register", serde_json::json!({
            "name": "hermes",
            "capabilities": ["text"],
        }))
        .await;
        assert_eq!(status, StatusCode::OK);

        // Get last_seen before poll
        let (_, json_before) = get_json(&state, "/api/status").await;
        let before = json_before["agents"]["hermes"]["last_seen"].as_i64().unwrap();

        // Small delay
        tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;

        // Poll — should update last_seen
        let (status, _) = get_json(&state, "/api/agents/hermes/poll").await;
        assert_eq!(status, StatusCode::OK);

        // Get last_seen after poll
        let (_, json_after) = get_json(&state, "/api/status").await;
        let after = json_after["agents"]["hermes"]["last_seen"].as_i64().unwrap();

        assert!(after > before, "last_seen should increase after poll");
    }

    #[tokio::test]
    async fn test_poll_unknown_agent_returns_404() {
        let state = Arc::new(make_state());

        let (status, json) = get_json(&state, "/api/agents/nobody/poll").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("nobody"));
    }

    #[tokio::test]
    async fn test_duplicate_registration_updates_last_seen() {
        let state = Arc::new(make_state());

        // Register first time
        let (status, _) = post_json(&state, "/api/agents/register", serde_json::json!({
            "name": "hermes",
            "capabilities": ["text"],
        }))
        .await;
        assert_eq!(status, StatusCode::OK);

        // Get first last_seen via status
        let (_, json1) = get_json(&state, "/api/status").await;
        let first_seen = json1["agents"]["hermes"]["last_seen"].as_i64().unwrap();

        // Small delay then register again
        tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;

        let (status, _) = post_json(&state, "/api/agents/register", serde_json::json!({
            "name": "hermes",
            "capabilities": ["text"],
        }))
        .await;
        assert_eq!(status, StatusCode::OK);

        // Check second last_seen
        let (_, json2) = get_json(&state, "/api/status").await;
        let second_seen = json2["agents"]["hermes"]["last_seen"].as_i64().unwrap();

        assert!(
            second_seen > first_seen,
            "last_seen should increase on re-registration"
        );
    }
}
