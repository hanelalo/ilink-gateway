//! Message router — connects iLink messages, agent registry, commands,
//! and the HTTP API together.
//!
//! Routes incoming WeChat messages to the active agent's queue,
//! handles built-in commands (/use, /list, /status, /cmd),
//! and dispatches agent replies back to iLink.

use std::collections::HashMap;

use crate::agents::queue::MessageQueue;
use crate::agents::registry::AgentRegistry;
use crate::error::{GatewayError, Result};
use crate::ilink::types::{
    msg_type, AgentMessage, MediaItem, QueuedMessage, RouterCommand, WeixinMessage,
};
use crate::router::commands::{execute_command, is_dangerous_command, parse_command};
use crate::storage::sqlite_store::SqliteStore;

/// State keys used in the gateway_state table.
const KEY_ACTIVE_AGENT: &str = "active_agent";

/// Persist all agent_sessions for a given agent as one JSON blob under
/// "session:{agent_name}".
fn sessions_key(agent: &str) -> String {
    format!("sessions:{}", agent)
}

/// Central message router that coordinates message handling.
pub struct Router {
    registry: AgentRegistry,
    queue: MessageQueue,
    active_agent: Option<String>,
    cmd_max_output_chars: usize,
    /// Per-agent per-user ACP session IDs.
    /// Key: agent_name → (from_user → acp_session_id)
    agent_sessions: HashMap<String, HashMap<String, String>>,
}

impl Router {
    /// Create a new Router with the given registry and message queue.
    pub fn new(registry: AgentRegistry, queue: MessageQueue) -> Self {
        Self {
            registry,
            queue,
            active_agent: None,
            cmd_max_output_chars: 2000,
            agent_sessions: HashMap::new(),
        }
    }

    /// Load persisted state from the SQLite store.
    ///
    /// Restores active_agent and all agent session mappings.
    pub fn load_state(&mut self, store: &SqliteStore) {
        // Restore active agent (may reference an agent not yet registered)
        if let Ok(Some(agent)) = store.get_state(KEY_ACTIVE_AGENT) {
            self.active_agent = Some(agent);
        }

        // Restore all persisted session mappings (keys look like "sessions:*")
        // Walk all gateway_state keys to find session entries.
        if let Ok(keys) = store.get_state_keys_with_prefix("sessions:") {
            for key in keys {
                if let Some(agent_name) = key.strip_prefix("sessions:") {
                    if let Ok(Some(json)) = store.get_state(&key) {
                        if let Ok(sessions) =
                            serde_json::from_str::<HashMap<String, String>>(&json)
                        {
                            self.agent_sessions
                                .insert(agent_name.to_string(), sessions);
                        }
                    }
                }
            }
        }
    }

    /// Persist current state to the SQLite store.
    ///
    /// Saves active_agent and all agent session mappings.
    pub fn persist_state(&self, store: &SqliteStore) {
        // Save active agent
        if let Some(ref agent) = self.active_agent {
            let _ = store.set_state(KEY_ACTIVE_AGENT, agent);
        }

        // Save session mappings for all agents
        for (agent_name, sessions) in &self.agent_sessions {
            let key = sessions_key(agent_name);
            if let Ok(json) = serde_json::to_string(sessions) {
                let _ = store.set_state(&key, &json);
            }
        }
    }

    /// Get the cached ACP session ID for an agent's user conversation.
    pub fn get_session(&self, agent: &str, from_user: &str) -> Option<&str> {
        self.agent_sessions
            .get(agent)?
            .get(from_user)
            .map(|s| s.as_str())
    }

    /// Set or update the ACP session ID for an agent's user conversation.
    pub fn set_session(&mut self, agent: &str, from_user: &str, session_id: String) {
        self.agent_sessions
            .entry(agent.to_string())
            .or_default()
            .insert(from_user.to_string(), session_id);
    }

    /// Set the maximum number of characters for `/cmd` command output.
    pub fn set_cmd_max_output_chars(&mut self, max: usize) {
        self.cmd_max_output_chars = max;
    }

    /// Set the active agent. Returns error if agent not registered.
    #[allow(dead_code)]
    pub fn set_active_agent(&mut self, name: &str) -> Result<()> {
        if !self.registry.contains(name) {
            return Err(GatewayError::AgentNotFound(name.to_string()));
        }
        self.active_agent = Some(name.to_string());
        Ok(())
    }

    /// Get the name of the active agent.
    pub fn active_agent(&self) -> Option<&str> {
        self.active_agent.as_deref()
    }

    /// Process an incoming WeChat message.
    ///
    /// Returns `Some(text)` if it is a built-in command that should be replied
    /// directly, or `None` if the message was routed to the active agent's queue.
    ///
    /// This method is synchronous by design — `Router` is shared behind
    /// `std::sync::Mutex` in HTTP API state, and a `MutexGuard` must not be
    /// held across `.await` points.  When the `/cmd` branch needs the async
    /// `execute_command` it bridges via `tokio::task::block_in_place`.
    pub fn handle_incoming(&mut self, msg: &WeixinMessage) -> Result<Option<String>> {
        let text = msg.text().ok_or_else(|| {
            GatewayError::Command("Message has no text content".to_string())
        })?;

        // Command messages start with "/"
        if text.starts_with('/') {
            return self.handle_command(text);
        }

        // Normal message — route to active agent
        let active = self.active_agent.as_deref().ok_or_else(|| {
            GatewayError::Command(
                "No active agent is set. Use /use <agent_name> to select one.".to_string(),
            )
        })?;

        let agent_msg = Self::to_agent_message(msg).ok_or_else(|| {
            GatewayError::Command("Failed to convert message for agent delivery".to_string())
        })?;

        let media = agent_msg.media.clone();
        let queued = QueuedMessage {
            id: agent_msg.id,
            from_user: agent_msg.from_user,
            text: agent_msg.text,
            timestamp: agent_msg.timestamp,
            context_token: agent_msg.context_token,
            message_type: agent_msg.message_type,
            delivered: false,
            media,
        };

        self.queue.enqueue(active, queued)?;
        Ok(None)
    }

    /// Handle a command message (starts with "/").
    fn handle_command(&mut self, text: &str) -> Result<Option<String>> {
        match parse_command(text) {
            Some(RouterCommand::UseAgent(name)) => {
                if !self.registry.contains(&name) {
                    return Ok(Some(format!("Agent '{name}' not found")));
                }
                self.active_agent = Some(name.clone());
                Ok(Some(format!("Switched to agent '{name}'")))
            }
            Some(RouterCommand::ListAgents) => Ok(Some(self.build_list_text())),
            Some(RouterCommand::Status) => Ok(Some(self.build_status_text())),
            Some(RouterCommand::Cmd {
                command,
                timeout_secs,
            }) => {
                if is_dangerous_command(&command) {
                    return Ok(Some(format!(
                        "Dangerous command blocked: {command}"
                    )));
                }
                // execute_command is async but we are behind a std::sync::MutexGuard
                // so we bridge via block_in_place + Handle::block_on.
                let max = self.cmd_max_output_chars;
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        execute_command(&command, timeout_secs, max).await
                    })
                });
                match result {
                    Ok(output) => Ok(Some(output)),
                    Err(e) => Ok(Some(format!("Command error: {e}"))),
                }
            }
            None => Ok(Some(format!("Unknown command: {text}"))),
        }
    }

    /// Build a Status string for /status command.
    pub fn build_status_text(&self) -> String {
        let active = self.active_agent.as_deref().unwrap_or("(none)");
        let count = self.registry.len();
        format!(
            "Gateway status:\n  Active agent: {active}\n  Registered agents: {count}"
        )
    }

    /// Build a List string for /list command.
    pub fn build_list_text(&self) -> String {
        let agents = self.registry.list();
        if agents.is_empty() {
            return "No agents registered.".to_string();
        }
        let lines: Vec<String> = agents
            .iter()
            .map(|a| {
                use crate::ilink::types::AgentStatus;
                let status = match a.status {
                    AgentStatus::Online => "online",
                    AgentStatus::Offline => "offline",
                };
                format!("  {} [{}]", a.name, status)
            })
            .collect();
        lines.join("\n")
    }

    /// Convert a WeixinMessage to a canonical AgentMessage for agent delivery.
    pub fn to_agent_message(msg: &WeixinMessage) -> Option<AgentMessage> {
        let text = msg.text().unwrap_or("");
        let item_type = msg
            .item_list
            .as_ref()?
            .first()?
            .item_type
            .unwrap_or(msg_type::TEXT);
        let message_type = match item_type {
            msg_type::TEXT => "text",
            msg_type::IMAGE => "image",
            msg_type::VOICE => "voice",
            msg_type::FILE => "file",
            msg_type::VIDEO => "video",
            _ => "unknown",
        };

        let media = Self::extract_media_info(msg);

        Some(AgentMessage {
            id: msg.message_id.map(|id| id.to_string()).unwrap_or_default(),
            from_user: msg.from_user_id.clone().unwrap_or_default(),
            text: text.to_string(),
            timestamp: msg.create_time_ms.unwrap_or(0),
            context_token: msg.context_token.clone().unwrap_or_default(),
            message_type: message_type.to_string(),
            media,
        })
    }

    /// Extract media information from a WeixinMessage's item list.
    ///
    /// Returns a vec with zero or one `MediaItem` depending on whether the
    /// first item in the list is a media type (image, voice, video, or file).
    pub fn extract_media_info(msg: &WeixinMessage) -> Vec<MediaItem> {
        let item = match msg.item_list.as_ref().and_then(|items| items.first()) {
            Some(item) => item,
            None => return vec![],
        };

        let item_type = item.item_type.unwrap_or(msg_type::TEXT);
        match item_type {
            msg_type::IMAGE => {
                if let Some(ref img) = item.image_item {
                    vec![MediaItem {
                        media_type: "image".to_string(),
                        local_path: String::new(),
                        original_name: img.md5.clone(),
                    }]
                } else {
                    vec![]
                }
            }
            msg_type::VOICE => {
                if item.voice_item.is_some() {
                    vec![MediaItem {
                        media_type: "voice".to_string(),
                        local_path: String::new(),
                        original_name: None,
                    }]
                } else {
                    vec![]
                }
            }
            msg_type::VIDEO => {
                if let Some(ref video) = item.video_item {
                    vec![MediaItem {
                        media_type: "video".to_string(),
                        local_path: String::new(),
                        original_name: video.md5.clone(),
                    }]
                } else {
                    vec![]
                }
            }
            msg_type::FILE => {
                if let Some(ref file) = item.file_item {
                    vec![MediaItem {
                        media_type: "file".to_string(),
                        local_path: String::new(),
                        original_name: file.file_name.clone(),
                    }]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }

    /// Get a mutable reference to the registry.
    pub fn registry_mut(&mut self) -> &mut AgentRegistry {
        &mut self.registry
    }

    /// Get a reference to the queue.
    pub fn queue(&self) -> &MessageQueue {
        &self.queue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ilink::types::{FileItem, ImageItem, VideoItem, VoiceItem};
    use crate::ilink::types::MessageItem;

    // ─── Helpers ─────────────────────────────────────────────────────────────────

    fn make_text_msg(text: &str) -> WeixinMessage {
        use crate::ilink::types::{MessageItem, TextItem};
        WeixinMessage {
            message_id: Some(42),
            from_user_id: Some("user@wx".to_string()),
            create_time_ms: Some(1_000_000),
            context_token: Some("ctx-123".to_string()),
            message_type: Some(crate::ilink::types::chat_type::USER),
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

    fn setup() -> Router {
        Router::new(AgentRegistry::new(), MessageQueue::new())
    }

    fn register_agent(router: &mut Router, name: &str) {
        let caps = vec!["text".to_string()];
        router
            .registry_mut()
            .register(name, None, &caps)
            .unwrap();
    }

    // ─── Tests ───────────────────────────────────────────────────────────────────

    #[test]
    fn test_new_router_has_no_active_agent() {
        let router = setup();
        assert!(router.active_agent().is_none());
    }

    #[test]
    fn test_set_active_agent_works_after_registration() {
        let mut router = setup();
        register_agent(&mut router, "hermes");
        router.set_active_agent("hermes").unwrap();
        assert_eq!(router.active_agent(), Some("hermes"));
    }

    #[test]
    fn test_set_active_agent_fails_for_unknown_agent() {
        let mut router = setup();
        let result = router.set_active_agent("nobody");
        assert!(result.is_err());
        match result {
            Err(GatewayError::AgentNotFound(name)) => assert_eq!(name, "nobody"),
            _ => panic!("expected AgentNotFound"),
        }
    }

    #[test]
    fn test_handle_incoming_list_command() {
        let mut router = setup();
        register_agent(&mut router, "hermes");
        register_agent(&mut router, "zeus");
        let msg = make_text_msg("/list");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("hermes"));
        assert!(text.contains("zeus"));
    }

    #[test]
    fn test_handle_incoming_status_command() {
        let mut router = setup();
        register_agent(&mut router, "hermes");
        router.set_active_agent("hermes").unwrap();
        let msg = make_text_msg("/status");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("hermes"));
    }

    #[test]
    fn test_handle_incoming_use_command_switches_agent() {
        let mut router = setup();
        register_agent(&mut router, "hermes");
        let msg = make_text_msg("/use hermes");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("Switched"));
        assert!(text.contains("hermes"));
        assert_eq!(router.active_agent(), Some("hermes"));
    }

    #[test]
    fn test_handle_incoming_use_unknown_returns_error_text() {
        let mut router = setup();
        let msg = make_text_msg("/use nobody");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("not found"));
    }

    #[test]
    fn test_handle_incoming_normal_message_enqueues_for_active_agent() {
        let mut router = setup();
        register_agent(&mut router, "hermes");
        router.set_active_agent("hermes").unwrap();
        let msg = make_text_msg("hello world");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_none());
        assert!(router.queue().has_pending("hermes"));
    }

    #[test]
    fn test_handle_incoming_normal_message_no_active_agent_returns_error() {
        let mut router = setup();
        let msg = make_text_msg("hello");
        let result = router.handle_incoming(&msg);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_incoming_unknown_command() {
        let mut router = setup();
        let msg = make_text_msg("/foobar");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("Unknown"));
    }

    /// /cmd test.  Needs a multi-thread runtime because handle_command uses
    /// `tokio::task::block_in_place` to bridge to async execute_command.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_handle_incoming_cmd_executes_command() {
        let mut router = setup();
        let msg = make_text_msg("/cmd echo hello_from_cmd");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("hello_from_cmd"));
    }

    #[test]
    fn test_handle_incoming_cmd_blocked_dangerous() {
        let mut router = setup();
        let msg = make_text_msg("/cmd rm -rf /");
        let result = router.handle_incoming(&msg).unwrap();
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("Dangerous") || text.contains("blocked"));
    }

    #[test]
    fn test_to_agent_message_converts_correctly() {
        let msg = make_text_msg("test message");
        let agent_msg = Router::to_agent_message(&msg).unwrap();
        assert_eq!(agent_msg.text, "test message");
        assert_eq!(agent_msg.from_user, "user@wx");
        assert_eq!(agent_msg.id, "42");
        assert_eq!(agent_msg.context_token, "ctx-123");
        assert_eq!(agent_msg.message_type, "text");
    }

    #[test]
    fn test_to_agent_message_returns_none_for_empty_message() {
        let msg = WeixinMessage::default();
        assert!(Router::to_agent_message(&msg).is_none());
    }

    #[test]
    fn test_extract_media_info_image_returns_media_item() {
        let msg = WeixinMessage {
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::IMAGE),
                image_item: Some(ImageItem {
                    cdn_url: Some("https://cdn.weixin.qq.com/img".to_string()),
                    md5: Some("abc123".to_string()),
                    aes_key: Some("aes-key-123".to_string()),
                    encrypt_query_param: Some("enc=xyz".to_string()),
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let media = Router::extract_media_info(&msg);
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].media_type, "image");
        assert!(media[0].local_path.is_empty());
        assert_eq!(media[0].original_name.as_deref(), Some("abc123"));
    }

    #[test]
    fn test_extract_media_info_voice_returns_media_item() {
        let msg = WeixinMessage {
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::VOICE),
                voice_item: Some(VoiceItem {
                    cdn_url: Some("https://cdn.weixin.qq.com/voice".to_string()),
                    aes_key: Some("v-aes-key".to_string()),
                    encrypt_query_param: Some("enc=abc".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let media = Router::extract_media_info(&msg);
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].media_type, "voice");
        assert!(media[0].local_path.is_empty());
        assert!(media[0].original_name.is_none());
    }

    #[test]
    fn test_extract_media_info_video_returns_media_item() {
        let msg = WeixinMessage {
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::VIDEO),
                video_item: Some(VideoItem {
                    cdn_url: Some("https://cdn.weixin.qq.com/video".to_string()),
                    aes_key: Some("vid-aes".to_string()),
                    encrypt_query_param: Some("enc=vid".to_string()),
                    md5: Some("md5-video".to_string()),
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let media = Router::extract_media_info(&msg);
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].media_type, "video");
        assert_eq!(media[0].original_name.as_deref(), Some("md5-video"));
    }

    #[test]
    fn test_extract_media_info_file_returns_media_item() {
        let msg = WeixinMessage {
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::FILE),
                file_item: Some(FileItem {
                    file_name: Some("document.pdf".to_string()),
                    file_size: Some(1024),
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let media = Router::extract_media_info(&msg);
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].media_type, "file");
        assert_eq!(
            media[0].original_name.as_deref(),
            Some("document.pdf")
        );
    }

    #[test]
    fn test_extract_media_info_text_returns_empty() {
        let msg = make_text_msg("hello");
        let media = Router::extract_media_info(&msg);
        assert!(media.is_empty());
    }

    #[test]
    fn test_extract_media_info_empty_item_list() {
        let msg = WeixinMessage::default();
        let media = Router::extract_media_info(&msg);
        assert!(media.is_empty());
    }

    #[test]
    fn test_extract_media_info_missing_item_data_returns_empty() {
        // IMAGE type but no image_item
        let msg = WeixinMessage {
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::IMAGE),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let media = Router::extract_media_info(&msg);
        assert!(media.is_empty());
    }

    #[test]
    fn test_to_agent_message_with_image_includes_media() {
        let msg = WeixinMessage {
            message_id: Some(99),
            from_user_id: Some("user@wx".to_string()),
            context_token: Some("ctx-img".to_string()),
            message_type: Some(crate::ilink::types::chat_type::USER),
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::IMAGE),
                image_item: Some(ImageItem {
                    md5: Some("img-md5".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let agent_msg = Router::to_agent_message(&msg).unwrap();
        assert_eq!(agent_msg.message_type, "image");
        assert_eq!(agent_msg.text, "");
        assert_eq!(agent_msg.media.len(), 1);
        assert_eq!(agent_msg.media[0].media_type, "image");
    }

    #[test]
    fn test_build_status_text_contains_expected_fields() {
        let mut router = setup();
        register_agent(&mut router, "hermes");
        router.set_active_agent("hermes").unwrap();
        let status = router.build_status_text();
        assert!(status.contains("hermes"));
        assert!(status.contains("Registered agents"));
    }

    #[test]
    fn test_build_list_text_contains_agent_names() {
        let mut router = setup();
        register_agent(&mut router, "hermes");
        register_agent(&mut router, "zeus");
        let list = router.build_list_text();
        assert!(list.contains("hermes"));
        assert!(list.contains("zeus"));
    }

    #[test]
    fn test_build_list_text_empty_when_no_agents() {
        let router = setup();
        let list = router.build_list_text();
        assert!(list.contains("No agents registered"));
    }

    #[test]
    fn test_registry_and_registry_mut() {
        let mut router = setup();
        assert!(router.registry().is_empty());
        register_agent(&mut router, "hermes");
        assert_eq!(router.registry().len(), 1);
    }

    #[test]
    fn test_queue_reference() {
        let router = setup();
        assert!(!router.queue().has_pending("hermes"));
    }

    #[test]
    fn test_set_cmd_max_output_chars() {
        let mut router = setup();
        assert_eq!(router.cmd_max_output_chars, 2000);
        router.set_cmd_max_output_chars(500);
        assert_eq!(router.cmd_max_output_chars, 500);
    }
}
