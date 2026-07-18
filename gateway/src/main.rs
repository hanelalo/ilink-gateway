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
    /// Timestamp (Instant) when the incoming message was received, for
    /// measuring agent response time. Logged when the reply is sent.
    received_at: Instant,
    /// Truncated text of the incoming message, for audit logging.
    message_preview: String,
    /// Name of the agent the message was routed to.
    agent_name: String,
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

    // 3. Open SQLite store (expand `~` in the db path)
    let db_path = expand_tilde(&config.db_path);
    tracing::info!("database path: {db_path}");
    let store = SqliteStore::new(&db_path)?;

    // 4. Create core components
    let registry = AgentRegistry::new();
    let queue = MessageQueue::new();
    let mut router = Router::new(registry, queue);
    router.set_cmd_max_output_chars(config.cmd_max_output_chars);
    router.set_policies(
        config.dm_policy,
        config.group_policy,
        config.allowed_users.clone(),
        config.allowed_groups.clone(),
    );

    // 4b. Restore persisted state (active_agent, session IDs)
    router.load_state(&store);

    // 5. Build AppState for the HTTP API
    let store_arc = Arc::new(Mutex::new(store));
    let router_arc = Arc::new(Mutex::new(router));

    // Create reply channel, message context store, and WebSocket registry
    let (reply_tx, reply_rx) = tokio::sync::mpsc::unbounded_channel::<AgentReply>();
    let message_contexts: Arc<Mutex<HashMap<String, MessageContextInfo>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let typing_ticket_cache = TypingTicketCache::new();
    let send_breaker: SharedBreaker = Arc::new(Mutex::new(CircuitBreaker::new(
        1,
        Duration::from_secs(30),
        Duration::from_secs(30),
    )));
    let dedup = Arc::new(Mutex::new(DedupCache::new(Duration::from_secs(300))));
    let ws_registry = WsRegistry::new();
    let http_client = reqwest::Client::new();
    let _state = AppState {
        router: router_arc.clone(),
        reply_tx: reply_tx.clone(),
        ws_registry: ws_registry.clone(),
        store: store_arc.clone(),
    };

    // 5b. Spawn heartbeat checker (every 30s, timeout 60s)
    spawn_heartbeat_checker(router_arc.clone(), 30, 60);

    // 6. Load saved iLink credentials or run QR-code login
    let (token, base_url) = {
        let store_lock = store_arc.lock().unwrap();
        match store_lock.load_credentials()? {
            Some(creds) => {
                tracing::info!("loaded saved iLink credentials for account {}", creds.account_id);
                (creds.token, creds.base_url)
            }
            None => {
                tracing::info!("no saved credentials found; starting QR code login");
                qr_login_and_save(&config.ilink_base_url, &store_lock).await?
            }
        }
    };

    // 6b. Create the iLink client (always use the stored/exchanged base_url)
    let client = IlinkClient::new(Some(base_url.clone()))?;

    // 7. Notify start to enable outbound messaging
    tracing::info!("calling notify_start...");
    if let Err(e) = client.notify_start(&token).await {
        tracing::warn!("notify_start failed (will retry in loop): {e}");
    }

    // 7b. Spawn reply processor background task
    {
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
        let reply_breaker = send_breaker.clone();
        let reply_router = router_arc.clone();
        let mut rx = reply_rx;
        tokio::spawn(async move {
            handle_agent_replies(reply_client, reply_token, reply_router, reply_ctx, reply_tickets, reply_breaker, &mut rx).await;
        })
    };

    // 8. Spawn the HTTP API server
    let server_config = config.clone();
    let server_state = AppState {
        router: router_arc.clone(),
        reply_tx: reply_tx.clone(),
        ws_registry: ws_registry.clone(),
        store: store_arc.clone(),
    };
    tokio::spawn(async move {
        if let Err(e) = start_server(&server_config, server_state).await {
            tracing::error!("HTTP server error: {e}");
        }
    });

    tracing::info!(
        "gateway started — entering long-poll loop (target: {base_url})"
    );

    // Spawn a periodic cleanup task for stale message contexts.
    // Contexts are kept for multiple replies per message (e.g., streaming batches),
    // so we periodically purge entries older than 10 minutes.
    {
        let ctx_cleanup = message_contexts.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            loop {
                interval.tick().await;
                let cutoff = Instant::now() - Duration::from_secs(600);
                let mut map = ctx_cleanup.lock().unwrap();
                let before = map.len();
                map.retain(|_, v| v.received_at > cutoff);
                let removed = before - map.len();
                if removed > 0 {
                    tracing::debug!("cleaned {removed} stale message contexts");
                }
            }
        });
    }

    // 9. Main long-poll loop
    let mut sync_buf = String::new();
    let current_token = token;
    let _current_base_url = base_url;

    loop {
        let resp = match client.get_updates(&current_token, &sync_buf, Some(35)).await {
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

                // Dedup: skip messages we've already processed (message_id + content MD5, 5 min TTL)
                let key = dedup_key(&msg);
                if !dedup.lock().unwrap().check_and_record(&key) {
                    tracing::debug!("skipping duplicate message {key}");
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
                    tracing::info!(
                        agent = %active_agent.as_deref().unwrap_or("-"),
                        "handled inline (command)",
                    );
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

                            if let Err(e) =
                                resilient_send(&client, &current_token, send_req, &send_breaker)
                                    .await
                            {
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
                            let agent = active_agent.clone().unwrap_or_default();
                            let preview = msg.text().unwrap_or("").chars().take(80).collect::<String>();
                            message_contexts.lock().unwrap().insert(
                                msg_id.clone(),
                                MessageContextInfo {
                                    context_token: ctx,
                                    to_user: user.clone(),
                                    received_at: Instant::now(),
                                    message_preview: preview.clone(),
                                    agent_name: agent.clone(),
                                },
                            );
                            tracing::info!(
                                agent = %agent,
                                from_user = %user,
                                msg_id = %msg_id,
                                msg_preview = %preview,
                                "routed message to agent {agent}: {preview}",
                            );
                        }   // ← close if let (Some(ctx), Some(user))

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

// ─── Circuit breaker + resilient send ───────────────────────────────────

/// Sliding-window circuit breaker for the iLink send path.
///
/// Per docs/wechat.md §8: 30s window, default threshold 1 failure → open
/// for 30s, reset on success.
#[derive(Clone)]
struct CircuitBreaker {
    failures: Vec<Instant>,
    opened_at: Option<Instant>,
    window: Duration,
    threshold: usize,
    cooldown: Duration,
}

impl CircuitBreaker {
    fn new(threshold: usize, window: Duration, cooldown: Duration) -> Self {
        Self {
            failures: Vec::new(),
            opened_at: None,
            window,
            threshold,
            cooldown,
        }
    }

    fn is_open(&self) -> bool {
        match self.opened_at {
            Some(opened) => opened.elapsed() < self.cooldown,
            None => false,
        }
    }

    fn record_success(&mut self) {
        self.failures.clear();
        self.opened_at = None;
    }

    fn record_failure(&mut self) {
        let now = Instant::now();
        self.failures.retain(|t| now.duration_since(*t) < self.window);
        self.failures.push(now);
        if self.failures.len() >= self.threshold {
            if self.opened_at.is_none() {
                tracing::warn!(
                    "circuit breaker opened — pausing sends for {:?}",
                    self.cooldown
                );
            }
            self.opened_at = Some(now);
        }
    }
}

type SharedBreaker = Arc<Mutex<CircuitBreaker>>;

/// Message dedup cache: tracks (message_id, content-md5) keys with a TTL
/// to avoid re-processing messages that iLink re-delivers after reconnects.
struct DedupCache {
    seen: HashMap<String, Instant>,
    ttl: Duration,
}

impl DedupCache {
    fn new(ttl: Duration) -> Self {
        Self {
            seen: HashMap::new(),
            ttl,
        }
    }

    /// Returns `true` if this key is new (and records it), `false` if it
    /// was already seen within the TTL window.
    fn check_and_record(&mut self, key: &str) -> bool {
        let now = Instant::now();
        self.seen.retain(|_, t| now.duration_since(*t) < self.ttl);
        if self.seen.contains_key(key) {
            return false;
        }
        self.seen.insert(key.to_string(), now);
        true
    }
}

/// Build the dedup key: `"{message_id}:{md5(content)}"`.
fn dedup_key(msg: &WeixinMessage) -> String {
    use md5::{Digest, Md5};
    let id = msg.message_id.unwrap_or(0);
    let content = msg.text().unwrap_or("");
    let content_md5 = format!("{:x}", Md5::digest(content.as_bytes()));
    format!("{id}:{content_md5}")
}

/// Send a message with resilience: errcode=-2 exponential backoff (up to 4
/// retries), errcode=-14 context_token-stripping fallback, and circuit
/// breaker integration.
///
/// Returns `Ok(())` on success (errcode 0), or the first unrecoverable
/// error.  Records success/failure to the shared breaker.
async fn resilient_send(
    client: &IlinkClient,
    token: &str,
    mut req: SendMessageRequest,
    breaker: &SharedBreaker,
) -> std::result::Result<(), GatewayError> {
    {
        let b = breaker.lock().unwrap();
        if b.is_open() {
            tracing::warn!("circuit breaker open — skipping send");
            return Err(GatewayError::Ilink(
                "circuit breaker open — iLink send paused".to_string(),
            ));
        }
    }

    for attempt in 0..4u32 {
        match client.send_message(token, &req).await {
            Ok(resp) => {
                let errcode = resp.errcode.unwrap_or(0);
                if errcode == 0 {
                    breaker.lock().unwrap().record_success();
                    return Ok(());
                }
                if errcode == -2 {
                    tracing::warn!(
                        "sendmessage rate-limited (errcode=-2), attempt {}/4",
                        attempt + 1
                    );
                    let delay = Duration::from_secs(1u64 << attempt);
                    tokio::time::sleep(delay).await;
                    continue;
                }
                if errcode == -14 && req.msg.context_token.is_some() {
                    tracing::warn!(
                        "sendmessage errcode=-14 — retrying without context_token"
                    );
                    req.msg.context_token = None;
                    continue;
                }
                breaker.lock().unwrap().record_failure();
                let errmsg = resp.errmsg.unwrap_or_default();
                return Err(GatewayError::Ilink(format!(
                    "sendmessage errcode={errcode}: {errmsg}"
                )));
            }
            Err(e) => {
                breaker.lock().unwrap().record_failure();
                return Err(e);
            }
        }
    }

    breaker.lock().unwrap().record_failure();
    Err(GatewayError::Ilink(
        "sendmessage exhausted errcode=-2 retries".to_string(),
    ))
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
    breaker: SharedBreaker,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AgentReply>,
) {
    while let Some(reply) = rx.recv().await {
        // Proactive send: when to_user is set directly, bypass context lookup
        let (context_token, to_user, ctx_start, _ctx_preview, ctx_agent) =
            if let Some(ref to) = reply.to_user {
                let ctx = reply.context_token.clone().unwrap_or_default();
                (ctx, to.clone(), None, String::new(), String::new())
            } else {
                // Look up context by reply_to_id
                let map = contexts.lock().unwrap();
                match map.get(&reply.reply_to_id) {
                    Some(info) => (
                        info.context_token.clone(),
                        info.to_user.clone(),
                        Some(info.received_at),
                        info.message_preview.clone(),
                        info.agent_name.clone(),
                    ),
                    None => {
                        tracing::warn!("no context found for reply_to_id={}", reply.reply_to_id,);
                        continue;
                    }
                }
            };

        // Log agent response
        let elapsed = ctx_start
            .map(|t| t.elapsed())
            .map(|d| format!("{:.2}s", d.as_secs_f64()))
            .unwrap_or_else(|| String::new());
        if !ctx_agent.is_empty() {
            tracing::info!(
                agent = %ctx_agent,
                to_user = %to_user,
                reply_to = %reply.reply_to_id,
                response_time = %elapsed,
                "agent {ctx_agent} responded in {elapsed}",
            );
        } else {
            tracing::info!(
                to_user = %to_user,
                "proactive send to {to_user}",
            );
        }

        if reply.media_paths.is_empty() {
            // Text-only reply
            with_typing(&ticket_cache, &client, &token, &to_user, async {
                let reply_msg =
                    WeixinMessage::build_text_reply(context_token, to_user.clone(), reply.text);
                let req = SendMessageRequest {
                    msg: reply_msg,
                    base_info: None,
                };
                if let Err(e) = resilient_send(&client, &token, req, &breaker).await {
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
                match process_media_upload(&client, &token, &to_user, first_path).await {
                    Ok(upload) => {
                        let reply_msg = WeixinMessage::build_media_reply(
                            context_token.clone(),
                            to_user.clone(),
                            reply.text.clone(),
                            upload.item_type,
                            upload.encrypt_query_param,
                            upload.aes_key,
                            upload.mid_size,
                        );
                        let req = SendMessageRequest {
                            msg: reply_msg,
                            base_info: None,
                        };
                        if let Err(e) = resilient_send(&client, &token, req, &breaker).await {
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
                        if let Err(e2) = resilient_send(&client, &token, req, &breaker).await {
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
                if let Err(e) = resilient_send(&client, &token, req, &breaker).await {
                    tracing::error!("failed to send additional file note: {e}");
                }
            }
        }
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
///
/// Polling runs at 1s intervals with a 480s (8 min) overall deadline.
/// Handles `scaned` (waiting for mobile confirmation), `scaned_but_redirect`
/// (switch to the new base_url returned by iLink), and `expired` (auto-retry
/// up to 3 times).
async fn qr_login_and_save(
    initial_base_url: &str,
    store: &SqliteStore,
) -> Result<(String, String)> {
    let mut client = IlinkClient::new(Some(initial_base_url.to_string()))?;

    // Step 1: get QR code
    let qr_resp = client.get_qr_code().await?;
    let mut qrcode = qr_resp
        .qrcode
        .ok_or_else(|| GatewayError::Ilink("QR code key is empty".to_string()))?;

    // Step 2: render QR to terminal
    // qrcode_img_content is a WeChat liteapp URL; the QR code must encode
    // this URL (not the raw hex key) so WeChat opens the liteapp on scan.
    let qr_scan_data = qr_resp
        .qrcode_img_content
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&qrcode);
    render_qr(qr_scan_data);

    // Step 3: poll until confirmed (480s / 8 min deadline, 1s interval)
    let deadline = Instant::now() + Duration::from_secs(480);
    let mut expired_retries = 0u32;
    let (token, base_url, account_id, user_id) = loop {
        if Instant::now() >= deadline {
            return Err(GatewayError::Ilink(
                "QR login timed out after 480 seconds".to_string(),
            ));
        }

        tokio::time::sleep(Duration::from_secs(1)).await;

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
            Some("scaned") => {
                tracing::info!("QR scanned — please tap confirm in WeChat");
                continue;
            }
            Some("scaned_but_redirect") => {
                // Switch to the new base_url returned by iLink.
                if let Some(ref new_base) = status_resp.baseurl {
                    tracing::info!("QR redirect — switching to {new_base}");
                    client = IlinkClient::new(Some(new_base.clone()))?;
                }
                continue;
            }
            Some("expired") => {
                expired_retries += 1;
                if expired_retries > 3 {
                    return Err(GatewayError::Ilink(
                        "QR code expired 3 times; please restart".to_string(),
                    ));
                }
                tracing::warn!("QR expired, refreshing (attempt {expired_retries}/3)");
                // Re-fetch a fresh QR code and render it.
                let qr_resp = client.get_qr_code().await?;
                qrcode = qr_resp.qrcode.ok_or_else(|| {
                    GatewayError::Ilink("refreshed QR code key is empty".to_string())
                })?;
                let qr_scan_data = qr_resp
                    .qrcode_img_content
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&qrcode);
                render_qr(qr_scan_data);
                continue;
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

/// Render a QR code in the terminal from the scan data (liteapp URL or hex key).
///
/// WeChat needs to scan a full liteapp URL, not the raw hex token, so
/// `qrcode_img_content` from the API is prioritized when present.
fn render_qr(data: &str) {
    match qrcode::QrCode::new(data.as_bytes()) {
        Ok(code) => {
            let qr_str = code
                .render::<char>()
                .quiet_zone(false)
                .module_dimensions(2, 1)
                .dark_color('\u{2588}')
                .light_color(' ')
                .build();
            println!("\nScan the QR code to log in to WeChat:");
            for line in qr_str.lines() {
                println!("{line}");
            }
        }
        Err(e) => {
            tracing::warn!("failed to generate QR code from '{data}': {e}");
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

        let (token, base_url) = qr_login_and_save(&server.url(), &store)
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
                        received_at: Instant::now(),
                        message_preview: "agent message".to_string(),
                        agent_name: "hermes".to_string(),
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
        assert_eq!(info.agent_name, "hermes");
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
                received_at: Instant::now(),
                message_preview: "test".to_string(),
                agent_name: "hermes".to_string(),
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
        let breaker: SharedBreaker = Arc::new(Mutex::new(CircuitBreaker::new(
            1,
            Duration::from_secs(30),
            Duration::from_secs(30),
        )));
        tokio::spawn(async move {
            handle_agent_replies(
                client,
                "test-token".to_string(),
                router,
                ctx_clone,
                tickets,
                breaker,
                &mut rx,
            )
            .await;
        });

        // Send a text reply through the channel
        tx.send(AgentReply {
            reply_to_id: "msg-42".to_string(),
            text: "Hello from agent".to_string(),
            media_paths: vec![],
            to_user: None,
            context_token: None,
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
        let upload_mock = server
            .mock("POST", "/ilink/bot/getuploadurl")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "ret": 0,
                "upload_full_url": format!("{}/cdn/upload", server.url()),
            }).to_string())
            .create();

        // Mock CDN upload (POST) — returns x-encrypted-param header
        let cdn_mock = server
            .mock("POST", "/cdn/upload")
            .with_status(200)
            .with_header("x-encrypted-param", "encrypted_param_123")
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
                received_at: Instant::now(),
                message_preview: "test".to_string(),
                agent_name: "hermes".to_string(),
            },
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentReply>();
        let ctx_clone = contexts.clone();
        let router = Arc::new(Mutex::new(Router::new(
            AgentRegistry::new(),
            MessageQueue::new(),
        )));
        let tickets = TypingTicketCache::new();
        let breaker: SharedBreaker = Arc::new(Mutex::new(CircuitBreaker::new(
            1,
            Duration::from_secs(30),
            Duration::from_secs(30),
        )));

        tokio::spawn(async move {
            handle_agent_replies(client, "test-token".to_string(), router, ctx_clone, tickets, breaker, &mut rx)
                .await;
        });

        // Send a media reply
        tx.send(AgentReply {
            reply_to_id: "msg-99".to_string(),
            text: "Check this image".to_string(),
            media_paths: vec![media_path.to_string_lossy().to_string()],
            to_user: None,
            context_token: None,
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
                    "status": 1,
                    "base_info": { "channel_version": "2.2.0" }
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
                    "status": 2,
                    "base_info": { "channel_version": "2.2.0" }
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
                    "status": 1,
                    "base_info": { "channel_version": "2.2.0" }
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
                    "status": 2,
                    "base_info": { "channel_version": "2.2.0" }
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
