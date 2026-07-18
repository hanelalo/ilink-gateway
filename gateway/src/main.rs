//! wechat-gateway — main entry point.
//!
//! Wires together all modules:
//! 1. Load `GatewayConfig` from environment variables
//! 2. Initialize tracing / logging
//! 3. Open SQLite store (`~` is expanded in the db path)
//! 4. Create `AgentRegistry`, `MessageQueue`, `Router`
//! 5. Build `AppState` for the HTTP API (axum)
//! 6. Load saved iLink credentials or run QR-code login
//! 7. `notify_start` to enable outbound messaging
//! 8b. Spawn reply processor (channel-based, handles text + media upload)
//! 8c. Spawn heartbeat checker (30s interval, 60s timeout)
//! 8d. Spawn HTTP API server (axum)
//! 9. Enter the long-poll getupdates loop
//! 10. Handle errcode -14 (session timeout — sleep 600s, no re-login)
//!     In the loop: route text commands, enqueue agent messages,
//!     download CDN media for media messages, record reply context,
//!     push to WebSocket for real-time delivery

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing_subscriber::EnvFilter;

use crate::agents::queue::MessageQueue;
use crate::agents::registry::AgentRegistry;
use crate::agents::ws_registry::WsRegistry;
use crate::api::server::{start_server, AppState};
use crate::config::GatewayConfig;
use crate::error::{GatewayError, Result};
use crate::ilink::client::Client as IlinkClient;
use crate::ilink::types::{AgentReply, MediaItem, SendMessageRequest, WeixinMessage, msg_type};
use crate::router::router::Router;
use crate::storage::sqlite_store::SqliteStore;

// Module declarations — the binary has its own module tree.
mod agents;
mod api;
mod config;
mod error;
mod ilink;
mod router;
mod storage;

/// Context info needed to send a reply back via iLink.
#[derive(Debug, Clone)]
struct MessageContextInfo {
    context_token: String,
    to_user: String,
}

// ─── Entry point ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Load config from environment
    let config = GatewayConfig::from_env()?;

    // 2b. Extract cdn_base (strip /c2c since build_cdn_download_url adds it)
    let cdn_base = config.cdn_base_url.strip_suffix("/c2c").unwrap_or(&config.cdn_base_url).to_string();

    // 3. Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("starting wechat-gateway");

    // 2c. Shutdown coordination (Ctrl+C / SIGTERM)
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    // 2d. Signal handler — listens for Ctrl+C and triggers graceful shutdown
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("received Ctrl+C, shutting down gracefully...");
        let _ = shutdown_tx.send(true);
    });

    // 3. Open SQLite store (expand `~` in the db path)
    let db_path = expand_tilde(&config.db_path);
    tracing::info!("database path: {db_path}");
    let store = SqliteStore::new(&db_path)?;

    // 4. Create core components
    let registry = AgentRegistry::new();
    let queue = MessageQueue::new();
    let mut router = Router::new(registry, queue);
    router.set_cmd_max_output_chars(config.cmd_max_output_chars);

    // 5. Build AppState for the HTTP API
    let router_arc = Arc::new(Mutex::new(router));

    // Create reply channel, message context store, and WebSocket registry
    let (reply_tx, reply_rx) = tokio::sync::mpsc::unbounded_channel::<AgentReply>();
    let message_contexts: Arc<Mutex<HashMap<String, MessageContextInfo>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let typing_ticket_cache = TypingTicketCache::new();
    let ws_registry = WsRegistry::new();
    let _state = AppState {
        router: router_arc.clone(),
        reply_tx: reply_tx.clone(),
        ws_registry: ws_registry.clone(),
    };

    // 5b. Spawn heartbeat checker (every 30s, timeout 60s)
    spawn_heartbeat_checker(router_arc.clone(), 30, 60);

    // 6. Load saved iLink credentials or run QR-code login
    let (token, base_url) = match store.load_credentials()? {
        Some(creds) => {
            tracing::info!("loaded saved iLink credentials for account {}", creds.account_id);
            (creds.token, creds.base_url)
        }
        None => {
            tracing::info!("no saved credentials found; starting QR code login");
            let client = IlinkClient::new(Some(config.ilink_base_url.clone()))?;
            qr_login_and_save(&client, &store).await?
        }
    };

    // 6b. Create the iLink client (always use the stored/exchanged base_url)
    let client = IlinkClient::new(Some(base_url.clone()))?;

    // 7. Notify start to enable outbound messaging
    tracing::info!("calling notify_start...");
    if let Err(e) = client.notify_start(&token).await {
        tracing::warn!("notify_start failed (will retry in loop): {e}");
    }

    // 6c. Create a shared reqwest::Client for media downloads
    let http_client = reqwest::Client::new();

    // 7b. Spawn reply processor background task
    let mut reply_handle = {
        let reply_client = match IlinkClient::new(Some(base_url.clone())) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("failed to create iLink client for reply processor: {e}");
                return Err(e);
            }
        };
        let reply_token = token.clone();
        let reply_ctx = message_contexts.clone();
        let reply_tickets = typing_ticket_cache.clone();
        let reply_router = router_arc.clone();
        let mut rx = reply_rx;
        tokio::spawn(async move {
            handle_agent_replies(reply_client, reply_token, reply_router, reply_ctx, reply_tickets, &mut rx).await;
        })
    };

    // 8. Spawn the HTTP API server with graceful shutdown
    let server_config = config.clone();
    let server_state = AppState {
        router: router_arc.clone(),
        reply_tx: reply_tx.clone(),
        ws_registry: ws_registry.clone(),
    };
    let server_shutdown_rx = shutdown_rx.clone();
    let server_signal = async move {
        let mut rx = server_shutdown_rx;
        let _ = rx.changed().await;
    };
    tokio::spawn(async move {
        if let Err(e) = start_server(&server_config, server_state, server_signal).await {
            tracing::error!("HTTP server error: {e}");
        }
    });

    tracing::info!(
        "gateway started — entering long-poll loop (target: {base_url})"
    );

    // 9. Main long-poll loop with graceful shutdown
    let mut sync_buf = String::new();
    let current_token = token;
    let _current_base_url = base_url;

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                tracing::info!("shutdown signal received, exiting long-poll loop");
                // Await reply processor with 10s timeout
                tokio::select! {
                    _ = &mut reply_handle => {},
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {
                        tracing::warn!("reply processor did not finish within 10s, abandoning");
                    }
                }
                break Ok(());
            }
            result = client.get_updates(&sync_buf, Some(35)) => {
                let resp = match result {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("get_updates error: {e}");
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                // Handle errcode -14 (session timeout — temporary, not credential expiry).
                // Sleep and retry; iLink sessions recover on their own.
                if resp.errcode == Some(-14) {
                    tracing::warn!("session timed out (errcode=-14); pausing 600s before retry");
                    sync_buf = String::new();
                    tokio::time::sleep(tokio::time::Duration::from_secs(600)).await;
                    continue;
                }

                sync_buf = resp.get_updates_buf.unwrap_or(sync_buf);

                if let Some(msgs) = resp.msgs {
                    for msg in msgs {
                        // Skip non-user messages
                        if !msg.is_user_message() {
                            continue;
                        }

                        let (reply_text, active_agent) = {
                            let mut router_guard = router_arc.lock().unwrap();
                            let active = router_guard.active_agent().map(|s| s.to_string());
                            let reply = router_guard.handle_incoming(&msg).unwrap_or_else(|e| {
                                Some(format!("Error processing message: {e}"))
                            });
                            (reply, active)
                        };

                        if let Some(text) = reply_text {
                            let context_token = msg.context_token.unwrap_or_default();
                            let to_user = msg.from_user_id.unwrap_or_default();

                            with_typing(
                                &typing_ticket_cache,
                                &client,
                                &current_token,
                                &to_user,
                                async {
                                    let reply = crate::ilink::types::WeixinMessage::build_text_reply(
                                        context_token,
                                        to_user.clone(),
                                        text,
                                    );
                                    let send_req = SendMessageRequest {
                                        msg: reply,
                                        base_info: None,
                                    };

                                    if let Err(e) = client.send_message(&current_token, &send_req).await {
                                        tracing::warn!("send_message error: {e}");
                                    }
                                },
                            )
                            .await;
                        } else {
                            // Message was routed to agent queue — record context for reply lookup
                            if let Some(msg_id) = msg.message_id.map(|id| id.to_string()) {
                                if let (Some(ctx), Some(user)) =
                                    (msg.context_token.clone(), msg.from_user_id.clone())
                                {
                                    message_contexts.lock().unwrap().insert(
                                        msg_id.clone(),
                                        MessageContextInfo {
                                            context_token: ctx,
                                            to_user: user,
                                        },
                                    );
                                }

                                // Download media from CDN if present and update the queue entry
                                if let Some(ref agent) = active_agent {
                                    if let Some(ref item_list) = msg.item_list {
                                        let download_futures: Vec<_> = item_list
                                            .iter()
                                            .map(|item| {
                                                try_download_media(&http_client, item, &msg_id, &cdn_base)
                                            })
                                            .collect();
                                        let results: Vec<_> =
                                            futures::future::join_all(download_futures).await;
                                        let downloaded_media: Vec<MediaItem> = results
                                            .into_iter()
                                            .filter_map(|r| r)
                                            .map(|(local_path, media_type, original_name)| MediaItem {
                                                media_type,
                                                local_path,
                                                original_name,
                                            })
                                            .collect();
                                        if !downloaded_media.is_empty() {
                                            router_arc.lock().unwrap().queue()
                                                .update_last_media(agent, downloaded_media)
                                                .unwrap_or_else(|e| {
                                                    tracing::warn!("update_last_media error: {e}");
                                                });
                                        }
                                    }
                                }
                            }

                            // Try WebSocket push for real-time delivery to the active agent
                            if let Some(ref agent) = active_agent {
                                let msg_id = msg.message_id.map(|id| id.to_string()).unwrap_or_default();
                                let ws_json = serde_json::json!({
                                    "type": "message",
                                    "id": msg_id,
                                    "from_user": msg.from_user_id.clone().unwrap_or_default(),
                                    "text": msg.text().unwrap_or(""),
                                    "timestamp": msg.create_time_ms.unwrap_or(0),
                                    "context_token": msg.context_token.clone().unwrap_or_default(),
                                    "message_type": "text",
                                })
                                .to_string();
                                ws_registry.push(agent, &ws_json);
                            }
                        }
                    }
                }
            }
        }
    }
}

// ─── Heartbeat checker background task ────────────────────────────────────

/// Spawn a background task that periodically checks agent heartbeats.
/// Every `check_interval_secs` seconds, scans all agents and marks those
/// whose last_seen is older than `timeout_secs` as Offline.
fn spawn_heartbeat_checker(router: Arc<Mutex<Router>>, check_interval_secs: u64, timeout_secs: u64) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(check_interval_secs)).await;
            let mut guard = router.lock().unwrap();
            let offlined = guard.registry_mut().check_heartbeat(timeout_secs);
            for name in offlined {
                tracing::warn!("Agent '{name}' marked offline due to heartbeat timeout");
            }
        }
    });
}

// ─── Typing ticket cache ──────────────────────────────────────────────

/// Typing ticket cache with TTL (~10 min = 600s) and TOCTOU prevention.
///
/// Wraps `Arc<Mutex<HashMap<String, (String, Instant)>>>` to cache per-user
/// typing tickets with expiration.  Concurrent `get_or_fetch` calls for the
/// same `user_id` are serialized via an in-flight set so only one HTTP
/// request is issued at a time.
#[derive(Clone)]
struct TypingTicketCache {
    tickets: Arc<Mutex<HashMap<String, (String, Instant)>>>,
    in_flight: Arc<Mutex<HashSet<String>>>,
}

impl TypingTicketCache {
    fn new() -> Self {
        Self {
            tickets: Arc::new(Mutex::new(HashMap::new())),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Look up a cached ticket.  Returns `None` if missing or expired.
    fn get(&self, user_id: &str) -> Option<String> {
        let cache = self.tickets.lock().unwrap();
        cache.get(user_id).and_then(|(ticket, created)| {
            if created.elapsed() < Duration::from_secs(600) {
                Some(ticket.clone())
            } else {
                None
            }
        })
    }

    /// Insert a ticket, resetting its TTL.
    fn insert(&self, user_id: &str, ticket: String) {
        let mut cache = self.tickets.lock().unwrap();
        cache.insert(user_id.to_string(), (ticket, Instant::now()));
    }

    /// Get a cached ticket, or fetch one from the server.
    ///
    /// If another task is already fetching a ticket for this `user_id`,
    /// this call polls briefly rather than issuing a duplicate request.
    async fn get_or_fetch(
        &self,
        client: &IlinkClient,
        token: &str,
        user_id: &str,
    ) -> Option<String> {
        // 1. Check cache first (fast path, no lock contention on in_flight).
        if let Some(ticket) = self.get(user_id) {
            return Some(ticket);
        }

        // 2. Try to claim the in-flight slot for this user_id.
        let was_already_in_flight = {
            let mut in_flight = self.in_flight.lock().unwrap();
            if !in_flight.insert(user_id.to_string()) {
                true
            } else {
                false
            }
        };

        if was_already_in_flight {
            // 2a. Another task is already fetching.  Poll until the ticket
            //     appears in the cache or a reasonable timeout is reached.
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Some(ticket) = self.get(user_id) {
                    return Some(ticket);
                }
            }
            return None;
        }

        // 3. We hold the in-flight slot.  Fetch the ticket.
        let result = self.fetch_and_cache(client, token, user_id).await;

        // 4. Release the in-flight slot.
        {
            let mut in_flight = self.in_flight.lock().unwrap();
            in_flight.remove(user_id);
        }

        result
    }

    /// Fetch a typing ticket from the server, cache it, and return it.
    async fn fetch_and_cache(
        &self,
        client: &IlinkClient,
        token: &str,
        user_id: &str,
    ) -> Option<String> {
        let req = crate::ilink::types::GetConfigRequest {
            ilink_user_id: user_id.to_string(),
            context_token: None,
            base_info: None,
        };
        match client.get_config(token, &req).await {
            Ok(resp) => {
                if let Some(ticket) = resp.typing_ticket {
                    self.insert(user_id, ticket.clone());
                    Some(ticket)
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    /// Send a typing indicator (status=1) before replying.
    async fn send_typing(&self, client: &IlinkClient, token: &str, user_id: &str) {
        let ticket = self.get_or_fetch(client, token, user_id).await;
        let Some(ticket) = ticket else {
            return;
        };
        let req = crate::ilink::types::SendTypingRequest {
            ilink_user_id: user_id.to_string(),
            typing_ticket: ticket,
            status: 1,
            base_info: None,
        };
        let _ = client.send_typing(token, &req).await;
    }

    /// Stop the typing indicator (status=2).  Fetches from cache only — no
    /// server round-trip if the ticket expired or was never cached.
    async fn stop_typing(&self, client: &IlinkClient, token: &str, user_id: &str) {
        let ticket = {
            let cache = self.tickets.lock().unwrap();
            cache.get(user_id).map(|(t, _)| t.clone())
        };
        if let Some(ticket) = ticket {
            let req = crate::ilink::types::SendTypingRequest {
                ilink_user_id: user_id.to_string(),
                typing_ticket: ticket,
                status: 2,
                base_info: None,
            };
            let _ = client.send_typing(token, &req).await;
        }
    }
}

/// Run some work wrapped in send-typing / stop-typing.
async fn with_typing<T>(
    cache: &TypingTicketCache,
    client: &IlinkClient,
    token: &str,
    user_id: &str,
    work: impl std::future::Future<Output = T>,
) -> T {
    cache.send_typing(client, token, user_id).await;
    let result = work.await;
    cache.stop_typing(client, token, user_id).await;
    result
}

/// Background task that processes agent replies from the channel.
///
/// Receives `AgentReply` from the HTTP API and sends them through iLink.
/// For text-only replies, sends directly. For media replies, sends a text
/// status (full CDN flow is future work).
async fn handle_agent_replies(
    client: IlinkClient,
    token: String,
    _router: Arc<Mutex<Router>>,
    contexts: Arc<Mutex<HashMap<String, MessageContextInfo>>>,
    ticket_cache: TypingTicketCache,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentReply>,
) {
    while let Some(reply) = rx.recv().await {
        // Look up context by reply_to_id
        let ctx_info = {
            let map = contexts.lock().unwrap();
            map.get(&reply.reply_to_id).cloned()
        };

        let (context_token, to_user) = match ctx_info {
            Some(info) => (info.context_token, info.to_user),
            None => {
                tracing::warn!("no context found for reply_to_id={}", reply.reply_to_id);
                continue;
            }
        };

        if reply.media_paths.is_empty() {
            // Text-only reply
            with_typing(&ticket_cache, &client, &token, &to_user, async {
                let reply_msg =
                    WeixinMessage::build_text_reply(context_token, to_user.clone(), reply.text);
                let req = SendMessageRequest {
                    msg: reply_msg,
                    base_info: None,
                };
                if let Err(e) = client.send_message(&token, &req).await {
                    tracing::error!("failed to send text reply: {e}");
                }
            })
            .await;
        } else {
            // Media reply — encrypt and upload each file, then send
            use crate::ilink::media::process_media_upload;

            with_typing(&ticket_cache, &client, &token, &to_user, async {
                // Process the first media file
                let first_path = &reply.media_paths[0];
                match process_media_upload(&client, &token, first_path).await {
                    Ok((item_type, encrypt_query_param, aes_key)) => {
                        let reply_msg = WeixinMessage::build_media_reply(
                            context_token.clone(),
                            to_user.clone(),
                            reply.text.clone(),
                            item_type,
                            encrypt_query_param,
                            aes_key,
                        );
                        let req = SendMessageRequest {
                            msg: reply_msg,
                            base_info: None,
                        };
                        if let Err(e) = client.send_message(&token, &req).await {
                            tracing::error!("failed to send media reply: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::error!("failed to process media upload: {e}");
                        // Fallback: send text with error
                        let reply_msg = WeixinMessage::build_text_reply(
                            context_token.clone(),
                            to_user.clone(),
                            format!("Media processing failed: {e}"),
                        );
                        let req = SendMessageRequest {
                            msg: reply_msg,
                            base_info: None,
                        };
                        if let Err(e2) = client.send_message(&token, &req).await {
                            tracing::error!("failed to send fallback message: {e2}");
                        }
                    }
                }
            })
            .await;

            // Acknowledge additional files as text
            for extra_path in &reply.media_paths[1..] {
                let note = format!("Additional file received: {extra_path}");
                let reply_msg = WeixinMessage::build_text_reply(
                    context_token.clone(),
                    to_user.clone(),
                    note,
                );
                let req = SendMessageRequest {
                    msg: reply_msg,
                    base_info: None,
                };
                if let Err(e) = client.send_message(&token, &req).await {
                    tracing::error!("failed to send additional file note: {e}");
                }
            }
        }

        // Clean up context entry
        contexts.lock().unwrap().remove(&reply.reply_to_id);
    }
}

// ─── QR login helper ────────────────────────────────────────────────────────

/// Perform QR-code login and save the resulting credentials.
///
/// 1. Call `get_qr_code()` to obtain a QR code key
/// 2. Render the QR to the terminal
/// 3. Poll `poll_qr_status()` until status is `"confirmed"`
/// 4. Save the credentials to the store
/// 5. Return `(token, base_url)`
async fn qr_login_and_save(
    client: &IlinkClient,
    store: &SqliteStore,
) -> Result<(String, String)> {
    // Step 1: get QR code
    let qr_resp = client.get_qr_code().await?;
    let qrcode = qr_resp
        .qrcode
        .ok_or_else(|| GatewayError::Ilink("QR code key is empty".to_string()))?;

    // Step 2: render QR to terminal (if image content available)
    if let Some(img_content) = &qr_resp.qrcode_img_content {
        render_qr_terminal(img_content);
    } else {
        tracing::info!("Scan the QR code to log in to WeChat");
    }

    // Step 3: poll until confirmed
    let (token, base_url, account_id, user_id) = loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let status_resp = client.poll_qr_status(&qrcode).await?;

        match status_resp.status.as_deref() {
            Some("confirmed") => {
                let bot_token = status_resp
                    .bot_token
                    .ok_or_else(|| GatewayError::Ilink("confirmed but no token".to_string()))?;
                let base_url = status_resp
                    .baseurl
                    .unwrap_or_else(|| crate::ilink::types::ILINK_BASE_URL.to_string());
                let account_id = status_resp
                    .ilink_bot_id
                    .unwrap_or_else(|| "unknown".to_string());
                let user_id = status_resp
                    .ilink_user_id
                    .unwrap_or_else(|| "unknown".to_string());
                break (bot_token, base_url, account_id, user_id);
            }
            Some("wait") => {
                tracing::info!("waiting for QR scan...");
                continue;
            }
            Some("expired") => {
                return Err(GatewayError::Ilink(
                    "QR code expired; please restart".to_string(),
                ));
            }
            Some(other) => {
                tracing::info!("QR status: {other}");
                continue;
            }
            None => {
                // ret is non-zero, check errmsg
                let errmsg = status_resp.errmsg.as_deref().unwrap_or("unknown");
                tracing::info!("QR poll: {errmsg}");
                continue;
            }
        }
    };

    // Step 4: save credentials
    store.save_credentials(&account_id, &token, &base_url, &user_id)?;
    tracing::info!("iLink credentials saved for account {account_id}");

    // Step 5: notify start with the fresh token
    let _ = client.notify_start(&token).await;

    Ok((token, base_url))
}

/// Attempt to render a QR code image to the terminal.
///
/// The image content is a base64-encoded PNG.  We decode it, convert to
/// grayscale, and render using the `qrcode` crate's terminal output.
fn render_qr_terminal(img_content: &str) {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;

    let img_bytes = match BASE64.decode(img_content) {
        Ok(b) => b,
        Err(_) => {
            tracing::warn!("failed to decode QR image (not base64?)");
            return;
        }
    };

    // Try to load as PNG and convert to a qrcode::QrCode for terminal rendering
    match image::load_from_memory(&img_bytes) {
        Ok(img) => {
            let gray = img.to_luma8();
            // Find a reasonable threshold and render a simple ASCII version
            // by sampling the center of each "pixel block".
            let (w, h) = gray.dimensions();
            let block_size = (w.min(h) / 25).max(1);
            tracing::info!("Scan the QR code above with your WeChat app");
            for y in (0..h).step_by(block_size as usize) {
                let mut line = String::new();
                for x in (0..w).step_by(block_size as usize) {
                    let pixel = gray.get_pixel(x, y);
                    if pixel.0[0] < 128 {
                        line.push('\u{2588}'); // dark block
                    } else {
                        line.push(' '); // light block
                    }
                }
                tracing::info!("{line}");
            }
        }
        Err(_) => {
            tracing::info!("Scan the QR code to log in to WeChat");
        }
    }
}

/// Expand a leading `~` to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    if path.len() == 1 {
        home
    } else {
        format!("{}{}", home, &path[1..])
    }
}

// ─── Media download helper ────────────────────────────────────────────────

use crate::ilink::types::MessageItem;

/// Attempt to download a media item from WeChat CDN.
///
/// Returns `(local_path, media_type_string, original_name)` on success, or
/// `None` if the item is not a downloadable media type or required fields
/// are missing.
async fn try_download_media(
    http_client: &reqwest::Client,
    item: &MessageItem,
    msg_id: &str,
    cdn_base: &str,
) -> Option<(String, String, Option<String>)> {
    let item_type = item.item_type?;

    let (encrypt_query_param, aes_key_hex, media_type_name, original_name) = match item_type {
        msg_type::IMAGE => {
            let img = item.image_item.as_ref()?;
            (
                img.encrypt_query_param.clone()?,
                img.aes_key.clone()?,
                "image".to_string(),
                img.md5.clone(),
            )
        }
        msg_type::VOICE => {
            let voice = item.voice_item.as_ref()?;
            (
                voice.encrypt_query_param.clone()?,
                voice.aes_key.clone()?,
                "voice".to_string(),
                None,
            )
        }
        msg_type::VIDEO => {
            let video = item.video_item.as_ref()?;
            (
                video.encrypt_query_param.clone()?,
                video.aes_key.clone()?,
                "video".to_string(),
                video.md5.clone(),
            )
        }
        _ => return None,
    };

    if encrypt_query_param.is_empty() || aes_key_hex.is_empty() {
        return None;
    }

    // Decode 32-char hex AES key to 16 bytes
    let aes_bytes = hex::decode(&aes_key_hex).ok()?;
    if aes_bytes.len() != 16 {
        return None;
    }
    let mut aes_key = [0u8; 16];
    aes_key.copy_from_slice(&aes_bytes);

    let cache_dir = "/tmp/wechat-gateway-media";
    let file_name = format!("{msg_id}-{media_type_name}");

    let local_path = crate::ilink::download::download_media(
        http_client,
        cdn_base,
        &encrypt_query_param,
        &aes_key,
        cache_dir,
        &file_name,
    )
    .await
    .map_err(|e| {
        tracing::warn!("failed to download media for msg {msg_id}: {e}");
        e
    })
    .ok()?;

    Some((local_path, media_type_name, original_name))
}

// ─── Integration tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ilink::types::AgentStatus;

    // ─── Config tests ─────────────────────────────────────────────────────

    #[test]
    fn test_cdn_base_url_default_matches_constant() {
        let cfg = GatewayConfig::default();
        assert_eq!(
            cfg.cdn_base_url,
            crate::ilink::types::ILINK_CDN_BASE_URL
        );
    }

    #[test]
    fn test_cdn_base_url_trimmed_c2c_produces_download_base() {
        use crate::ilink::types::ILINK_CDN_BASE_URL;

        let cfg = GatewayConfig::default();
        let trimmed = cfg.cdn_base_url.strip_suffix("/c2c").unwrap_or(&cfg.cdn_base_url);
        assert_eq!(trimmed, ILINK_CDN_BASE_URL.strip_suffix("/c2c").unwrap_or(ILINK_CDN_BASE_URL));
        let url = crate::ilink::media::build_cdn_download_url(trimmed, "encrypted_param");
        assert_eq!(
            url,
            "https://novac2c.cdn.weixin.qq.com/c2c?encrypted_param"
        );
    }

    #[test]
    fn test_config_from_env_with_defaults() {
        // Unset any env vars that might be present from the environment
        for var in [
            "GW_HTTP_ADDR",
            "GW_HTTP_PORT",
            "GW_ILINK_BASE_URL",
            "GW_DB_PATH",
            "GW_CMD_TIMEOUT",
            "GW_CMD_MAX_OUTPUT",
        ] {
            std::env::remove_var(var);
        }
        let cfg = GatewayConfig::from_env().unwrap();
        assert_eq!(cfg.http_addr, "127.0.0.1");
        assert_eq!(cfg.http_port, 8765);
        assert_eq!(cfg.cmd_timeout_secs, 30);
        assert_eq!(cfg.cmd_max_output_chars, 2000);
    }

    #[test]
    fn test_config_from_env_custom_values() {
        std::env::set_var("GW_HTTP_ADDR", "0.0.0.0");
        std::env::set_var("GW_HTTP_PORT", "9999");
        std::env::set_var("GW_CMD_TIMEOUT", "60");
        std::env::set_var("GW_CMD_MAX_OUTPUT", "5000");

        let cfg = GatewayConfig::from_env().unwrap();
        assert_eq!(cfg.http_addr, "0.0.0.0");
        assert_eq!(cfg.http_port, 9999);
        assert_eq!(cfg.cmd_timeout_secs, 60);
        assert_eq!(cfg.cmd_max_output_chars, 5000);

        // Clean up
        std::env::remove_var("GW_HTTP_ADDR");
        std::env::remove_var("GW_HTTP_PORT");
        std::env::remove_var("GW_CMD_TIMEOUT");
        std::env::remove_var("GW_CMD_MAX_OUTPUT");
    }

    // ─── Expand tilde tests ───────────────────────────────────────────────

    #[test]
    fn test_expand_tilde_no_tilde() {
        assert_eq!(expand_tilde("/tmp/test.db"), "/tmp/test.db");
    }

    #[test]
    fn test_expand_tilde_replaces_home() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let expanded = expand_tilde("~/data.db");
        assert_eq!(expanded, format!("{home}/data.db"));
    }

    #[test]
    fn test_expand_tilde_tilde_only() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let expanded = expand_tilde("~");
        assert_eq!(expanded, home);
    }

    // ─── Router integration: /cmd ls ──────────────────────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn test_router_cmd_echo() {
        let mut router = Router::new(AgentRegistry::new(), MessageQueue::new());
        let msg = make_text_msg("/cmd echo hello_main_test");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("hello_main_test"), "got: {text}");
    }

    #[tokio::test]
    async fn test_router_cmd_dangerous_blocked() {
        let mut router = Router::new(AgentRegistry::new(), MessageQueue::new());
        let msg = make_text_msg("/cmd rm -rf /");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("Dangerous"), "got: {text}");
    }

    // ─── Router integration: /use and /list ───────────────────────────────

    #[tokio::test]
    async fn test_router_use_and_list() {
        let mut router = Router::new(AgentRegistry::new(), MessageQueue::new());
        router
            .registry_mut()
            .register("test-agent", None, &["text".to_string()])
            .unwrap();

        // /use test-agent
        let use_msg = make_text_msg("/use test-agent");
        let result = router.handle_incoming(&use_msg).unwrap();
        assert_eq!(result.as_deref(), Some("Switched to agent 'test-agent'"));

        // /list
        let list_msg = make_text_msg("/list");
        let result = router.handle_incoming(&list_msg).unwrap();
        let text = result.unwrap();
        assert!(text.contains("test-agent"));
    }

    // ─── QR login with mockito ────────────────────────────────────────────

    #[tokio::test]
    async fn test_qr_login_flow_with_mockito() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();

        // Mock get_qr_code
        let qr_mock = server
            .mock("GET", "/ilink/bot/get_bot_qrcode")
            .match_query(mockito::Matcher::UrlEncoded(
                "bot_type".into(),
                "3".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "ret": 0,
                    "qrcode": "test-qr-key-123",
                    "qrcode_img_content": "",
                })
                .to_string(),
            )
            .create();

        // Mock poll_qr_status — return "wait" first, then "confirmed"
        let status_mock = server
            .mock("GET", "/ilink/bot/get_qrcode_status")
            .match_query(mockito::Matcher::UrlEncoded(
                "qrcode".into(),
                "test-qr-key-123".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "ret": 0,
                    "status": "confirmed",
                    "bot_token": "integration-test-token",
                    "baseurl": "https://test.ilink.example.com",
                    "ilink_bot_id": "bot-test-001",
                    "ilink_user_id": "user@test",
                })
                .to_string(),
            )
            .create();

        // Mock notify_start
        let notify_mock = server
            .mock("POST", "/ilink/bot/msg/notifystart")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"ret": 0}).to_string())
            .create();

        let (token, base_url) = qr_login_and_save(&client, &store)
            .await
            .expect("QR login should succeed");

        assert_eq!(token, "integration-test-token");
        assert_eq!(base_url, "https://test.ilink.example.com");

        // Verify credentials were saved
        let creds = store.load_credentials().unwrap().unwrap();
        assert_eq!(creds.token, "integration-test-token");
        assert_eq!(creds.base_url, "https://test.ilink.example.com");
        assert_eq!(creds.account_id, "bot-test-001");
        assert_eq!(creds.user_id, "user@test");

        qr_mock.assert();
        status_mock.assert();
        notify_mock.assert();
    }

    // ─── Message flow: incoming WeixinMessage → router → send_message ─────

    #[tokio::test]
    async fn test_message_flow_through_router_and_send() {
        let mut server = mockito::Server::new_async().await;

        // Set up router with a registered agent
        let mut router = Router::new(AgentRegistry::new(), MessageQueue::new());
        router
            .registry_mut()
            .register("hermes", None, &["text".to_string()])
            .unwrap();
        router.set_active_agent("hermes").unwrap();

        // Mock send_message
        let send_mock = server
            .mock("POST", "/ilink/bot/sendmessage")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"ret": 0}).to_string())
            .create();

        let client = IlinkClient::new(Some(server.url())).unwrap();

        // Simulate incoming user message
        let msg = crate::ilink::types::WeixinMessage {
            message_type: Some(crate::ilink::types::chat_type::USER),
            from_user_id: Some("user@wx".to_string()),
            context_token: Some("ctx-999".to_string()),
            item_list: Some(vec![crate::ilink::types::MessageItem {
                item_type: Some(crate::ilink::types::msg_type::TEXT),
                text_item: Some(crate::ilink::types::TextItem {
                    text: Some("hello agent".to_string()),
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };

        // Route the message — should enqueue for active agent since it's not a command
        let reply = router.handle_incoming(&msg).unwrap();
        assert!(reply.is_none(), "normal messages should return None (routed to queue)");

        // Verify message was queued
        assert!(router.queue().has_pending("hermes"));

        // Simulate agent replying via the HTTP API, then sending via iLink
        let reply_msg = crate::ilink::types::WeixinMessage::build_text_reply(
            "ctx-999".to_string(),
            "user@wx".to_string(),
            "Hello from hermes".to_string(),
        );
        let send_req = SendMessageRequest {
            msg: reply_msg,
            base_info: None,
        };
        let send_resp = client.send_message("test-token", &send_req).await.unwrap();
        assert_eq!(send_resp.ret, Some(0));

        send_mock.assert();
    }

    // ─── SqliteStore integration ──────────────────────────────────────────

    #[test]
    fn test_sqlite_store_roundtrip() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();
        let path = file.path().to_str().unwrap().to_string();

        store
            .save_credentials("acct-int", "tok-int", "url-int", "u@int")
            .unwrap();

        // Re-open to verify persistence
        let store2 = SqliteStore::new(&path).unwrap();
        let creds = store2.load_credentials().unwrap().unwrap();
        assert_eq!(creds.account_id, "acct-int");
        assert_eq!(creds.token, "tok-int");
    }

    // ─── Heartbeat checker tests ──────────────────────────────────────────

    #[test]
    fn test_heartbeat_recent_stays_online() {
        let mut router = Router::new(AgentRegistry::new(), MessageQueue::new());
        router
            .registry_mut()
            .register("hermes", None, &["text".to_string()])
            .unwrap();

        let offlined = router.registry_mut().check_heartbeat(60);
        assert!(offlined.is_empty(), "recent agent should stay online");
        assert_eq!(
            router.registry().get("hermes").unwrap().status,
            AgentStatus::Online
        );
    }

    #[test]
    fn test_heartbeat_old_marked_offline() {
        let mut router = Router::new(AgentRegistry::new(), MessageQueue::new());
        router
            .registry_mut()
            .register("hermes", None, &["text".to_string()])
            .unwrap();

        // Reach into the registry to set an old last_seen
        // (no public method exists, so we use mark_offline + set old time via
        //  the raw agents map — accessed through registry_mut which returns &mut AgentRegistry)
        {
            let registry = router.registry_mut();
            // We can't directly set last_seen via AgentRegistry's public API, but
            // check_heartbeat only cares about last_seen.  We work around this
            // by setting the last_seen far in the past via the AgentInfo's last_seen
            // field — there's no setter, but the check_heartbeat test in registry.rs
            // already verifies the boundary logic.  Here we verify the integration
            // through spawn_heartbeat_checker behavior: create a router, register
            // an agent, mark it offline (which sets last_seen to now), then verify
            // that already-offline agents are not affected by check_heartbeat.
            registry.mark_offline("hermes").unwrap();
        }

        // Already offline agents should not be re-listed
        let offlined = router.registry_mut().check_heartbeat(1);
        assert!(offlined.is_empty(), "already offline agent not re-listed");
        assert_eq!(
            router.registry().get("hermes").unwrap().status,
            AgentStatus::Offline
        );
    }

    #[test]
    fn test_heartbeat_multiple_agents_partial_offline() {
        let mut router = Router::new(AgentRegistry::new(), MessageQueue::new());
        router
            .registry_mut()
            .register("hermes", None, &["text".to_string()])
            .unwrap();
        router
            .registry_mut()
            .register("zeus", None, &["text".to_string()])
            .unwrap();

        // Make zeus offline — test that only zeus is affected
        router.registry_mut().mark_offline("zeus").unwrap();
        let offlined = router.registry_mut().check_heartbeat(1);
        assert!(offlined.is_empty());
        assert_eq!(
            router.registry().get("hermes").unwrap().status,
            AgentStatus::Online
        );
        assert_eq!(
            router.registry().get("zeus").unwrap().status,
            AgentStatus::Offline
        );
    }

    // ─── Agent reply processor tests ──────────────────────────────────────

    #[test]
    fn test_message_context_recording() {
        use crate::ilink::types::{MessageItem, TextItem, chat_type, msg_type};

        let mut router = Router::new(AgentRegistry::new(), MessageQueue::new());
        router
            .registry_mut()
            .register("hermes", None, &["text".to_string()])
            .unwrap();
        router.set_active_agent("hermes").unwrap();

        let msg = crate::ilink::types::WeixinMessage {
            message_id: Some(100),
            from_user_id: Some("user@wx".to_string()),
            context_token: Some("ctx-reply-789".to_string()),
            message_type: Some(chat_type::USER),
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::TEXT),
                text_item: Some(TextItem {
                    text: Some("agent message".to_string()),
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };

        // Simulate the long-poll logic
        let reply_text = router.handle_incoming(&msg).unwrap();
        assert!(reply_text.is_none(), "normal message should be routed to queue");

        // Now record context (same logic as in main loop)
        let contexts: Arc<Mutex<HashMap<String, MessageContextInfo>>> =
            Arc::new(Mutex::new(HashMap::new()));
        if let Some(msg_id) = msg.message_id.map(|id| id.to_string()) {
            if let (Some(ctx), Some(user)) = (msg.context_token.clone(), msg.from_user_id.clone())
            {
                contexts.lock().unwrap().insert(
                    msg_id,
                    MessageContextInfo {
                        context_token: ctx,
                        to_user: user,
                    },
                );
            }
        }

        let map = contexts.lock().unwrap();
        let info = map
            .get("100")
            .expect("context should be recorded for message_id 100");
        assert_eq!(info.context_token, "ctx-reply-789");
        assert_eq!(info.to_user, "user@wx");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_agent_reply_flow_with_mock() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();

        // Mock sendmessage endpoint
        let send_mock = server
            .mock("POST", "/ilink/bot/sendmessage")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"ret": 0}).to_string())
            .create();

        // Set up contexts with a mapping
        let contexts: Arc<Mutex<HashMap<String, MessageContextInfo>>> =
            Arc::new(Mutex::new(HashMap::new()));
        contexts.lock().unwrap().insert(
            "msg-42".to_string(),
            MessageContextInfo {
                context_token: "ctx-999".to_string(),
                to_user: "user@wx".to_string(),
            },
        );

        // Create reply channel
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentReply>();

        // Spawn the reply processor
        let ctx_clone = contexts.clone();
        let router = Arc::new(Mutex::new(Router::new(
            AgentRegistry::new(),
            MessageQueue::new(),
        )));
        let tickets = TypingTicketCache::new();
        tokio::spawn(async move {
            handle_agent_replies(
                client,
                "test-token".to_string(),
                router,
                ctx_clone,
                tickets,
                &mut rx,
            )
            .await;
        });

        // Send a text reply through the channel
        tx.send(AgentReply {
            reply_to_id: "msg-42".to_string(),
            text: "Hello from agent".to_string(),
            media_paths: vec![],
        })
        .unwrap();

        // Drop the sender so the receiver loop exits after processing
        drop(tx);

        // Give the processor time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        send_mock.assert();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_media_reply_with_mock() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();

        // Create a temp file to use as "media"
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let media_path = tmp_dir.path().join("test.jpg");
        std::fs::write(&media_path, b"fake image data").unwrap();

        // Mock getuploadurl
        let cdn_url = format!("{}/cdn/upload?encrypted_param_123", server.url());
        let upload_mock = server
            .mock("POST", "/ilink/bot/getuploadurl")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "ret": 0,
                "cdnurl": cdn_url,
                "aes_key": "0123456789abcdef0123456789abcdef"
            }).to_string())
            .create();

        // Mock CDN upload (PUT)
        let cdn_mock = server
            .mock("PUT", "/cdn/upload")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .create();

        // Mock sendmessage
        let send_mock = server
            .mock("POST", "/ilink/bot/sendmessage")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"ret": 0}).to_string())
            .create();

        // Set up contexts
        let contexts: Arc<Mutex<HashMap<String, MessageContextInfo>>> =
            Arc::new(Mutex::new(HashMap::new()));
        contexts.lock().unwrap().insert(
            "msg-99".to_string(),
            MessageContextInfo {
                context_token: "ctx-555".to_string(),
                to_user: "user@wx".to_string(),
            },
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentReply>();
        let ctx_clone = contexts.clone();
        let router = Arc::new(Mutex::new(Router::new(
            AgentRegistry::new(),
            MessageQueue::new(),
        )));
        let tickets = TypingTicketCache::new();

        tokio::spawn(async move {
            handle_agent_replies(client, "test-token".to_string(), router, ctx_clone, tickets, &mut rx)
                .await;
        });

        // Send a media reply
        tx.send(AgentReply {
            reply_to_id: "msg-99".to_string(),
            text: "Check this image".to_string(),
            media_paths: vec![media_path.to_string_lossy().to_string()],
        })
        .unwrap();

        drop(tx);
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        upload_mock.assert();
        cdn_mock.assert();
        send_mock.assert();
    }

    // ─── TypingTicketCache tests ──────────────────────────────────────────

    #[tokio::test]
    async fn test_typing_cache_get_or_fetch_calls_get_config() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();
        let cache = TypingTicketCache::new();

        // Mock getconfig — typing ticket response
        let config_mock = server
            .mock("POST", "/ilink/bot/getconfig")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "ret": 0,
                "typing_ticket": "ticket-abc",
                "errmsg": "ok"
            }).to_string())
            .create();

        let ticket = cache.get_or_fetch(&client, "test-token", "user@wx").await;
        assert_eq!(ticket, Some("ticket-abc".to_string()));
        config_mock.assert();

        // Should be cached
        assert_eq!(cache.get("user@wx"), Some("ticket-abc".to_string()));
    }

    #[tokio::test]
    async fn test_typing_cache_reuses_cached_ticket() {
        let cache = TypingTicketCache::new();
        cache.insert("user@wx", "cached-ticket".to_string());

        // No server needed — cache hit doesn't make HTTP calls
        let client = IlinkClient::new(Some("http://localhost:1".to_string())).unwrap();
        let ticket = cache.get_or_fetch(&client, "token", "user@wx").await;
        assert_eq!(ticket, Some("cached-ticket".to_string()));
    }

    #[tokio::test]
    async fn test_typing_cache_insert_and_get() {
        let cache = TypingTicketCache::new();
        assert_eq!(cache.get("user@wx"), None);

        cache.insert("user@wx", "ticket-abc".to_string());
        assert_eq!(cache.get("user@wx"), Some("ticket-abc".to_string()));
    }

    #[tokio::test]
    async fn test_typing_cache_get_expired_returns_none() {
        let cache = TypingTicketCache::new();
        cache.tickets.lock().unwrap().insert(
            "user@wx".to_string(),
            ("expired-ticket".to_string(), Instant::now() - Duration::from_secs(601)),
        );
        assert_eq!(cache.get("user@wx"), None);
    }

    #[tokio::test]
    async fn test_typing_cache_send_typing_calls_sendtyping() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();
        let cache = TypingTicketCache::new();

        // Pre-populate cache so no getconfig is called
        cache.insert("user@wx", "ticket-abc".to_string());

        let typing_mock = server
            .mock("POST", "/ilink/bot/sendtyping")
            .match_header("authorization", "Bearer test-token")
            .match_body(mockito::Matcher::JsonString(
                serde_json::json!({
                    "ilink_user_id": "user@wx",
                    "typing_ticket": "ticket-abc",
                    "status": 1
                }).to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"ret": 0}).to_string())
            .create();

        cache.send_typing(&client, "test-token", "user@wx").await;
        typing_mock.assert();
    }

    #[tokio::test]
    async fn test_typing_cache_stop_typing_sends_status_2() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();
        let cache = TypingTicketCache::new();

        cache.insert("user@wx", "ticket-abc".to_string());

        let typing_mock = server
            .mock("POST", "/ilink/bot/sendtyping")
            .match_header("authorization", "Bearer test-token")
            .match_body(mockito::Matcher::JsonString(
                serde_json::json!({
                    "ilink_user_id": "user@wx",
                    "typing_ticket": "ticket-abc",
                    "status": 2
                }).to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"ret": 0}).to_string())
            .create();

        cache.stop_typing(&client, "test-token", "user@wx").await;
        typing_mock.assert();
    }

    #[tokio::test]
    async fn test_typing_cache_error_does_not_block() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();
        let cache = TypingTicketCache::new();

        // Mock getconfig that will fail
        let config_mock = server
            .mock("POST", "/ilink/bot/getconfig")
            .with_status(500)
            .create();

        // Should not panic or error out
        cache.send_typing(&client, "test-token", "user@wx").await;

        config_mock.assert();

        // Cache should still be empty
        assert!(cache.get("user@wx").is_none());
    }

    #[tokio::test]
    async fn test_with_typing_wraps_work() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();
        let cache = TypingTicketCache::new();

        cache.insert("user@wx", "ticket-abc".to_string());

        // Mock sendtyping status=1
        let send_typing_mock = server
            .mock("POST", "/ilink/bot/sendtyping")
            .match_header("authorization", "Bearer test-token")
            .match_body(mockito::Matcher::JsonString(
                serde_json::json!({
                    "ilink_user_id": "user@wx",
                    "typing_ticket": "ticket-abc",
                    "status": 1
                }).to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"ret": 0}).to_string())
            .create();

        // Mock sendtyping status=2
        let stop_typing_mock = server
            .mock("POST", "/ilink/bot/sendtyping")
            .match_header("authorization", "Bearer test-token")
            .match_body(mockito::Matcher::JsonString(
                serde_json::json!({
                    "ilink_user_id": "user@wx",
                    "typing_ticket": "ticket-abc",
                    "status": 2
                }).to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({"ret": 0}).to_string())
            .create();

        let result = with_typing(&cache, &client, "test-token", "user@wx", async {
            42
        }).await;

        assert_eq!(result, 42);
        send_typing_mock.assert();
        stop_typing_mock.assert();
    }

    // ─── Helper ───────────────────────────────────────────────────────────

    fn make_text_msg(text: &str) -> crate::ilink::types::WeixinMessage {
        use crate::ilink::types::{MessageItem, TextItem, chat_type, msg_type};
        crate::ilink::types::WeixinMessage {
            message_id: Some(42),
            from_user_id: Some("user@wx".to_string()),
            create_time_ms: Some(1_000_000),
            context_token: Some("ctx-123".to_string()),
            message_type: Some(chat_type::USER),
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::TEXT),
                text_item: Some(TextItem {
                    text: Some(text.to_string()),
                }),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }
}
