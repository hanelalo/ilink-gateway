// iLink protocol type definitions.
//
// These types match the iLink HTTP JSON API used by WeChat ClawBot.
// Reference: iLink Hub (https://github.com/jeffkit/ilink-hub) and
// Hermes weixin.py adapter.

use serde::{Deserialize, Serialize};

// ── Constants ────────────────────────────────────────────────────────────────

pub const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
pub const ILINK_CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";

// ── Common ───────────────────────────────────────────────────────────────────

/// Attached to every outgoing iLink request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BaseInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_agent: Option<String>,
}

// ── Login / QR ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GetQrcodeResponse {
    pub ret: i32,
    pub qrcode: Option<String>,
    pub qrcode_img_content: Option<String>,
    pub errmsg: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QrcodeStatusResponse {
    pub ret: i32,
    pub status: Option<String>,
    pub bot_token: Option<String>,
    pub baseurl: Option<String>,
    pub ilink_bot_id: Option<String>,
    pub ilink_user_id: Option<String>,
    pub errmsg: Option<String>,
}

// ── Message Items ────────────────────────────────────────────────────────────

pub mod msg_type {
    pub const TEXT: i32 = 1;
    pub const IMAGE: i32 = 2;
    pub const VOICE: i32 = 3;
    pub const FILE: i32 = 4;
    pub const VIDEO: i32 = 5;
}

pub mod message_state {
    pub const FINISH: i32 = 2;
}

pub mod chat_type {
    /// User message received from a contact.
    pub const USER: i32 = 1;
    /// Bot/agent message sent back.
    pub const BOT: i32 = 2;
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VoiceItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdn_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageItem {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_item: Option<TextItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_item: Option<VoiceItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_item: Option<ImageItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_item: Option<FileItem>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

// ── WeixinMessage: canonical message type ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WeixinMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_time_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// 1 = user message, 2 = bot message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_type: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_state: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_list: Option<Vec<MessageItem>>,
    /// Required for routing replies back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
}

impl WeixinMessage {
    /// Extract displayable text from the message.
    pub fn text(&self) -> Option<&str> {
        let items = self.item_list.as_ref()?;
        items
            .iter()
            .find_map(|item| item.text_item.as_ref()?.text.as_deref())
            .or_else(|| {
                items
                    .iter()
                    .find_map(|item| item.voice_item.as_ref()?.text.as_deref())
            })
    }

    /// Check if the message is a user message.
    pub fn is_user_message(&self) -> bool {
        self.message_type == Some(chat_type::USER)
    }

    /// Build a text reply to this message.
    pub fn build_text_reply(context_token: String, to_user: String, text: String) -> Self {
        WeixinMessage {
            context_token: Some(context_token),
            to_user_id: Some(to_user),
            message_type: Some(chat_type::BOT),
            message_state: Some(message_state::FINISH),
            from_user_id: Some(String::new()),
            client_id: Some(new_client_id()),
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::TEXT),
                text_item: Some(TextItem { text: Some(text) }),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }
}

fn new_client_id() -> String {
    format!("wechat-gw:{}", uuid::Uuid::new_v4())
}

// ── GetUpdates ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct GetUpdatesRequest {
    pub get_updates_buf: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct GetUpdatesResponse {
    #[serde(default)]
    pub ret: Option<i32>,
    #[serde(default)]
    pub errcode: Option<i32>,
    #[serde(default)]
    pub errmsg: Option<String>,
    #[serde(default)]
    pub msgs: Option<Vec<WeixinMessage>>,
    #[serde(default)]
    pub get_updates_buf: Option<String>,
}

// ── SendMessage ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SendMessageRequest {
    pub msg: WeixinMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageResponse {
    #[serde(default)]
    pub ret: Option<i32>,
    #[serde(default)]
    pub errmsg: Option<String>,
}

// ── SendTyping ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SendTypingRequest {
    pub ilink_user_id: String,
    pub typing_ticket: String,
    pub status: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

// ── GetConfig ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct GetConfigRequest {
    pub ilink_user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

#[derive(Debug, Deserialize)]
pub struct GetConfigResponse {
    #[serde(default)]
    pub ret: Option<i32>,
    #[serde(default)]
    pub typing_ticket: Option<String>,
    #[serde(default)]
    pub errmsg: Option<String>,
}

// ── Agent-facing types (gateway internal) ────────────────────────────────────

/// A normalized message from WeChat that agents receive.
/// This is the canonical format—agents see this, not the raw iLink JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: String,
    pub from_user: String,
    pub text: String,
    pub timestamp: i64,
    pub context_token: String,
    pub message_type: String, // "text" | "image" | "voice" | "file"
}

/// An agent's reply to a WeChat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReply {
    pub reply_to_id: String,
    pub text: String,
}

/// Status of a registered agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentStatus {
    Online,
    Offline,
}

/// Agent registration information.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub endpoint: Option<String>,
    pub capabilities: Vec<String>,
    pub status: AgentStatus,
    pub last_seen: i64,
    pub registered_at: i64,
}

/// Exchanged message stored in queue.
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub id: String,
    pub from_user: String,
    pub text: String,
    pub timestamp: i64,
    pub context_token: String,
    pub message_type: String,
    pub delivered: bool,
}

/// Router command types.
#[derive(Debug, Clone, PartialEq)]
pub enum RouterCommand {
    UseAgent(String),
    ListAgents,
    Status,
    Cmd { command: String, timeout_secs: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_text_reply_sets_outbound_fields() {
        let msg = WeixinMessage::build_text_reply(
            "ctx-123".to_string(),
            "user@wechat".to_string(),
            "hello".to_string(),
        );
        assert_eq!(msg.from_user_id.as_deref(), Some(""));
        assert_eq!(msg.message_type, Some(chat_type::BOT));
        assert_eq!(msg.message_state, Some(message_state::FINISH));
        assert_eq!(msg.to_user_id.as_deref(), Some("user@wechat"));
        assert!(msg.client_id.as_deref().unwrap().starts_with("wechat-gw:"));
    }

    #[test]
    fn test_text_extraction_from_text_item() {
        let msg = WeixinMessage {
            message_type: Some(chat_type::USER),
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::TEXT),
                text_item: Some(TextItem {
                    text: Some("你好".to_string()),
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert_eq!(msg.text(), Some("你好"));
        assert!(msg.is_user_message());
    }

    #[test]
    fn test_text_extraction_falls_back_to_voice() {
        let msg = WeixinMessage {
            item_list: Some(vec![MessageItem {
                item_type: Some(msg_type::VOICE),
                voice_item: Some(VoiceItem {
                    text: Some("语音转文字".to_string()),
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert_eq!(msg.text(), Some("语音转文字"));
    }

    #[test]
    fn test_text_returns_none_when_no_item_list() {
        let msg = WeixinMessage::default();
        assert!(msg.text().is_none());
    }

    #[test]
    fn test_is_user_message() {
        let user_msg = WeixinMessage {
            message_type: Some(chat_type::USER),
            ..Default::default()
        };
        assert!(user_msg.is_user_message());

        let bot_msg = WeixinMessage {
            message_type: Some(chat_type::BOT),
            ..Default::default()
        };
        assert!(!bot_msg.is_user_message());
    }

    #[test]
    fn test_build_text_reply_generates_unique_client_id() {
        let msg1 = WeixinMessage::build_text_reply(
            "ctx".to_string(),
            "u".to_string(),
            "a".to_string(),
        );
        let msg2 = WeixinMessage::build_text_reply(
            "ctx".to_string(),
            "u".to_string(),
            "b".to_string(),
        );
        assert_ne!(msg1.client_id, msg2.client_id);
    }

    #[test]
    fn test_agent_message_roundtrip() {
        let msg = AgentMessage {
            id: "msg-1".to_string(),
            from_user: "user@wx".to_string(),
            text: "hello".to_string(),
            timestamp: 1700000000,
            context_token: "token-abc".to_string(),
            message_type: "text".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "msg-1");
        assert_eq!(deserialized.from_user, "user@wx");
    }

    #[test]
    fn test_agent_reply_roundtrip() {
        let reply = AgentReply {
            reply_to_id: "msg-1".to_string(),
            text: "hello back".to_string(),
        };
        let json = serde_json::to_string(&reply).unwrap();
        let deserialized: AgentReply = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.reply_to_id, "msg-1");
    }

    #[test]
    fn test_get_updates_request_serialization() {
        let req = GetUpdatesRequest {
            get_updates_buf: "buf-abc".to_string(),
            base_info: None,
            timeout: Some(35),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("get_updates_buf"));
        assert!(json.contains("35"));
    }

    #[test]
    fn test_send_typing_request_serialization() {
        let req = SendTypingRequest {
            ilink_user_id: "user@wx".to_string(),
            typing_ticket: "ticket-abc".to_string(),
            status: 1,
            base_info: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("typing_ticket"));
        assert!(json.contains(r#""status":1"#));
    }

    #[test]
    fn test_get_config_request_serialization() {
        let req = GetConfigRequest {
            ilink_user_id: "user@wx".to_string(),
            context_token: Some("ctx-123".to_string()),
            base_info: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("ilink_user_id"));
        assert!(json.contains("ctx-123"));
    }

    #[test]
    fn test_router_command_deserialization() {
        let cmd = RouterCommand::UseAgent("hermes".to_string());
        assert_eq!(
            cmd,
            RouterCommand::UseAgent("hermes".to_string())
        );

        let cmd2 = RouterCommand::Cmd {
            command: "ls -la".to_string(),
            timeout_secs: 30,
        };
        match cmd2 {
            RouterCommand::Cmd {
                ref command,
                timeout_secs,
            } => {
                assert_eq!(command, "ls -la");
                assert_eq!(timeout_secs, 30);
            }
            _ => panic!("expected Cmd variant"),
        }
    }

    #[test]
    fn test_new_client_id_format() {
        let id = new_client_id();
        assert!(id.starts_with("wechat-gw:"));
        assert_eq!(id.len(), 46); // "wechat-gw:" (10) + UUID (36)
    }
}
