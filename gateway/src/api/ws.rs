//! WebSocket handler for real-time agent message push.
//!
//! Agents can open a WebSocket connection to receive messages in real time
//! instead of (or in addition to) HTTP long-polling.  The WebSocket is an
//! optional real-time channel — HTTP polling continues to work.
//!
//! Endpoint:
//! - GET /ws/agents/{name} — WebSocket upgrade

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::api::server::AppState;
use crate::ilink::types::AgentReply;

/// GET /ws/agents/{name} — WebSocket upgrade endpoint.
///
/// After upgrade, the gateway pushes incoming messages as JSON over the
/// WebSocket and accepts reply JSON messages from the agent.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(name): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| handle_socket(socket, name, state))
}

async fn handle_socket(socket: WebSocket, name: String, state: Arc<AppState>) {
    // Split the WebSocket into sender (Sink) and receiver (Stream) halves.
    // This lets us concurrently send outbound messages + pings and receive
    // inbound messages from the agent.
    let (mut ws_writer, mut ws_reader) = socket.split();
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<String>();

    // Register this connection (replaces any old connection for the same agent).
    state.ws_registry.register(name.clone(), msg_tx);

    // ── Background task: send outbound messages and keepalive pings ───────
    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
    // Tick immediately so the first sleep is a full 30s interval.
    ping_interval.tick().await;

    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                // Gateway → Agent: push a message from the registry
                Some(json) = msg_rx.recv() => {
                    if ws_writer.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                // Ping every 30 seconds to keep the connection alive
                _ = ping_interval.tick() => {
                    if ws_writer.send(Message::Ping(Bytes::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // ── Main loop: receive inbound messages (replies) from the agent ────
    while let Some(msg) = ws_reader.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                // Try to parse as a reply
                if let Ok(reply) = serde_json::from_str::<serde_json::Value>(&text) {
                    if reply.get("type").and_then(|t| t.as_str()) == Some("reply") {
                        let agent_reply = AgentReply {
                            reply_to_id: reply
                                .get("reply_to_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            text: reply
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            media_paths: reply
                                .get("media_paths")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            to_user: reply
                                .get("to_user")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            context_token: reply
                                .get("context_token")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        };
                        // Forward the reply through the existing reply channel
                        let _ = state.reply_tx.send(agent_reply);
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Pong(_)) => {
                // Ignore — the gateway sends pings, and the agent responds with pongs.
            }
            Err(e) => {
                tracing::warn!("WebSocket error for agent '{name}': {e}");
                break;
            }
            _ => {}
        }
    }

    // Clean up
    send_task.abort();
    state.ws_registry.unregister(&name);
}
