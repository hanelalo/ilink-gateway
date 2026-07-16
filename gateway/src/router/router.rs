//! Message router — connects iLink messages, agent registry, commands,
//! and the HTTP API together.
//!
//! Routes incoming WeChat messages to the active agent's queue,
//! handles built-in commands (/use, /list, /status, /cmd),
//! and dispatches agent replies back to iLink.

use crate::agents::queue::MessageQueue;
use crate::agents::registry::AgentRegistry;
use crate::error::{GatewayError, Result};
use crate::ilink::types::{msg_type, AgentMessage, QueuedMessage, RouterCommand, WeixinMessage};
use crate::router::commands::parse_command;

/// Central message router that coordinates message handling.
pub struct Router {
    registry: AgentRegistry,
    queue: MessageQueue,
    active_agent: Option<String>,
}

impl Router {
    /// Create a new Router with the given registry and message queue.
    pub fn new(registry: AgentRegistry, queue: MessageQueue) -> Self {
        Self {
            registry,
            queue,
            active_agent: None,
        }
    }

    /// Set the active agent. Returns error if agent not registered.
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

        let queued = QueuedMessage {
            id: agent_msg.id,
            from_user: agent_msg.from_user,
            text: agent_msg.text,
            timestamp: agent_msg.timestamp,
            context_token: agent_msg.context_token,
            message_type: agent_msg.message_type,
            delivered: false,
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
            Some(RouterCommand::Cmd { command, .. }) => {
                Ok(Some(format!("Command execution not supported: {command}")))
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
        let text = msg.text()?;
        let item_type = msg.item_list.as_ref()?.first()?.item_type.unwrap_or(msg_type::TEXT);
        let message_type = match item_type {
            msg_type::TEXT => "text",
            msg_type::IMAGE => "image",
            msg_type::VOICE => "voice",
            msg_type::FILE => "file",
            msg_type::VIDEO => "video",
            _ => "unknown",
        };

        Some(AgentMessage {
            id: msg.message_id.map(|id| id.to_string()).unwrap_or_default(),
            from_user: msg.from_user_id.clone().unwrap_or_default(),
            text: text.to_string(),
            timestamp: msg.create_time_ms.unwrap_or(0),
            context_token: msg.context_token.clone().unwrap_or_default(),
            message_type: message_type.to_string(),
        })
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
}
