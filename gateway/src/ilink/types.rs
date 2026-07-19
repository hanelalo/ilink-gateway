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

impl BaseInfo {
    /// The channel version required by every iLink request.
    pub const CHANNEL_VERSION: &'static str = "2.2.0";

    /// Build a `BaseInfo` carrying the required `channel_version`.
    pub fn channel_default() -> Self {
        Self {
            channel_version: Some(Self::CHANNEL_VERSION.to_string()),
            bot_agent: None,
        }
    }
}

// ── Login / QR ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GetQrcodeResponse {
    #[allow(dead_code)]
    pub ret: i32,
    pub qrcode: Option<String>,
    pub qrcode_img_content: Option<String>,
    #[allow(dead_code)]
    pub errmsg: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QrcodeStatusResponse {
    #[allow(dead_code)]
    pub ret: i32,
    pub status: Option<String>,
    pub bot_token: Option<String>,
    pub baseurl: Option<String>,
    pub ilink_bot_id: Option<String>,
    pub ilink_user_id: Option<String>,
    #[allow(dead_code)]
    pub errmsg: Option<String>,
}

// ── CDN Upload ─────────────────────────────────────────────────────────

/// Upload request body for `POST /ilink/bot/getuploadurl`.
///
/// Field names match the iLink API exactly (note the inconsistent
/// snake_case: `aeskey`, `filesize`, `rawfilemd5` have no underscores,
/// while `to_user_id` and `no_need_thumb` do).
#[derive(Debug, Clone, Serialize)]
pub struct GetUploadUrlRequest {
    pub filekey: String,
    pub media_type: i32,
    pub to_user_id: String,
    pub rawsize: i64,
    pub rawfilemd5: String,
    pub filesize: i64,
    pub no_need_thumb: bool,
    pub aeskey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

/// `media_type` values used inside [`GetUploadUrlRequest`].
///
/// Note: this numbering differs from [`msg_type`] — the upload endpoint
/// uses its own ordering.
pub mod upload_media_type {
    pub const IMAGE: i32 = 1;
    pub const VIDEO: i32 = 2;
    pub const FILE: i32 = 3;
    pub const VOICE: i32 = 4;
}

/// CDN upload response (POST /ilink/bot/getuploadurl).
///
/// Prefer `upload_full_url`; fall back to `upload_param` (a pre-formed
/// query string to append to the CDN base URL).
#[derive(Debug, Deserialize)]
pub struct GetUploadUrlResponse {
    #[allow(dead_code)]
    #[serde(default)]
    pub ret: Option<i32>,
    #[serde(default)]
    pub upload_full_url: Option<String>,
    #[serde(default)]
    pub upload_param: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub errmsg: Option<String>,
}

// ── Message Items ────────────────────────────────────────────────────────

/// `encrypt_type` for outbound media items (AES-128-ECB is the only
/// documented value).
pub const ENCRYPT_TYPE_AES_128_ECB: i32 = 1;

/// Nested `media` object inside outbound image/voice/video/file items.
///
/// Carries the CDN reference produced by the upload flow.  Inbound media
/// items use flat `aes_key` / `encrypt_query_param` fields instead, so
/// `MediaRef` is only populated on the outbound (serialize) side.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt_query_param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt_type: Option<i32>,
}

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
    // Outbound: nested media + ciphertext size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_size: Option<i64>,
    // Inbound: flat CDN reference fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdn_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt_query_param: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageItem {
    // Outbound: nested media + ciphertext size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_size: Option<i64>,
    // Inbound: flat CDN reference fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdn_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt_query_param: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileItem {
    // Outbound: nested media + ciphertext size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_size: Option<i64>,
    // Common: file metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoItem {
    // Outbound: nested media + ciphertext size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_size: Option<i64>,
    // Inbound: flat CDN reference fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdn_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt_query_param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefMsgItem {
    pub msg_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefMsg {
    pub message_item: RefMsgItem,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_item: Option<VideoItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_msg: Option<RefMsg>,
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
    #[serde(alias = "room_id", skip_serializing_if = "Option::is_none")]
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

    /// Build a media reply with an encrypted CDN reference.
    ///
    /// Used when an agent sends back a media message (image, voice, video, or file).
    /// The `text` goes into `text_item` and, for file type, also into `file_item.file_name`.
    /// `mid_size` is the ciphertext (encrypted) file size — not the raw size.
    /// `aes_key` must already be in the API format: `base64(hex_string_of_original_key)`.
    pub fn build_media_reply(
        context_token: String,
        to_user: String,
        text: String,
        item_type: i32,
        encrypt_query_param: String,
        aes_key: String,
        mid_size: i64,
    ) -> Self {
        let media = MediaRef {
            encrypt_query_param: Some(encrypt_query_param),
            aes_key: Some(aes_key),
            encrypt_type: Some(ENCRYPT_TYPE_AES_128_ECB),
        };

        let mut item = MessageItem {
            item_type: Some(item_type),
            text_item: Some(TextItem {
                text: Some(text.clone()),
            }),
            ..Default::default()
        };

        match item_type {
            msg_type::IMAGE => {
                item.image_item = Some(ImageItem {
                    media: Some(media),
                    mid_size: Some(mid_size),
                    ..Default::default()
                });
            }
            msg_type::VOICE => {
                item.voice_item = Some(VoiceItem {
                    media: Some(media),
                    mid_size: Some(mid_size),
                    ..Default::default()
                });
            }
            msg_type::VIDEO => {
                item.video_item = Some(VideoItem {
                    media: Some(media),
                    mid_size: Some(mid_size),
                    ..Default::default()
                });
            }
            msg_type::FILE => {
                item.file_item = Some(FileItem {
                    media: Some(media),
                    mid_size: Some(mid_size),
                    file_name: Some(text),
                    ..Default::default()
                });
            }
            _ => {}
        }

        WeixinMessage {
            context_token: Some(context_token),
            to_user_id: Some(to_user),
            message_type: Some(chat_type::BOT),
            message_state: Some(message_state::FINISH),
            from_user_id: Some(String::new()),
            client_id: Some(new_client_id()),
            item_list: Some(vec![item]),
            ..Default::default()
        }
    }
}

fn new_client_id() -> String {
    format!("wechat-gw:{}", uuid::Uuid::new_v4())
}

// ── GetUpdates ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct GetUpdatesRequest {
    pub get_updates_buf: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct GetUpdatesResponse {
    #[allow(dead_code)]
    #[serde(default)]
    pub ret: Option<i32>,
    #[serde(default)]
    pub errcode: Option<i32>,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    #[serde(default)]
    pub ret: Option<i32>,
    #[serde(default)]
    pub errcode: Option<i32>,
    #[allow(dead_code)]
    #[serde(default)]
    pub errmsg: Option<String>,
    /// New context_token returned by the server — update the stored one.
    #[allow(dead_code)]
    #[serde(default)]
    pub context_token: Option<String>,
    /// Message ID assigned by iLink for this sent message.
    #[serde(default)]
    pub message_id: Option<i64>,
    /// Catch unknown fields for protocol probing.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

// ── SendTyping ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SendTypingRequest {
    pub ilink_user_id: String,
    pub typing_ticket: String,
    pub status: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

// ── GetConfig ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct GetConfigRequest {
    pub ilink_user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

#[derive(Debug, Deserialize)]
pub struct GetConfigResponse {
    #[allow(dead_code)]
    #[serde(default)]
    pub ret: Option<i32>,
    #[serde(default)]
    pub typing_ticket: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub errmsg: Option<String>,
}

// ── Agent-facing types (gateway internal) ────────────────────────────────────

/// Media attachment (agent-facing, serialized to JSON for poll API)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub media_type: String, // "image" | "voice" | "file" | "video"
    pub local_path: String, // path to locally cached file
    pub original_name: Option<String>, // file name (for file type)
}

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<MediaItem>,
    /// JSON string from the original agent's reply context.
    /// Non-empty when this message is a reference reply; the receiving agent
    /// can use it for secondary routing (e.g. claude adapter routes to workspace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_context: Option<String>,
}

/// An agent's reply to a WeChat message.
///
/// For normal replies, `reply_to_id` is used to look up the context_token
/// and to_user from the incoming message context. For proactive sends
/// (pairing codes, notifications), set `to_user` (and optionally
/// `context_token`) directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReply {
    pub reply_to_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media_paths: Vec<String>,
    /// When set, bypass the message_context lookup and send to this user
    /// instead (used for proactive sends like pairing codes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_user: Option<String>,
    /// Optional context_token for proactive sends. Defaults to "" if unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
    /// JSON string containing routing context (e.g. {"agent":"claude","workspace":"my-project"}).
    /// Set by agents when replying, stored by gateway for reference-reply routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_context: Option<String>,
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
    #[allow(dead_code)]
    pub endpoint: Option<String>,
    pub capabilities: Vec<String>,
    pub status: AgentStatus,
    pub last_seen: i64,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub delivered: bool,
    pub media: Vec<MediaItem>,
    pub agent_context: Option<String>,
}

/// Router command types.
#[derive(Debug, Clone, PartialEq)]
pub enum RouterCommand {
    UseAgent(String),
    ListAgents,
    Status,
    Cmd { command: String, timeout_secs: u64 },
    Help,
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
                    ..Default::default()
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
            media: vec![],
            agent_context: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "msg-1");
        assert_eq!(deserialized.from_user, "user@wx");
        assert!(deserialized.media.is_empty());
    }

    #[test]
    fn test_agent_reply_roundtrip() {
        let reply = AgentReply {
            reply_to_id: "msg-1".to_string(),
            text: "hello back".to_string(),
            media_paths: vec![],
            to_user: None,
            context_token: None,
            agent_context: None,
        };
        let json = serde_json::to_string(&reply).unwrap();
        let deserialized: AgentReply = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.reply_to_id, "msg-1");
        assert!(deserialized.to_user.is_none());
        assert!(deserialized.context_token.is_none());
    }

    #[test]
    fn test_agent_message_with_media_serialization() {
        let msg = AgentMessage {
            id: "msg-1".to_string(),
            from_user: "user@wx".to_string(),
            text: "check this image".to_string(),
            timestamp: 1700000000,
            context_token: "token-abc".to_string(),
            message_type: "image".to_string(),
            media: vec![MediaItem {
                media_type: "image".to_string(),
                local_path: "/tmp/cache/abc.jpg".to_string(),
                original_name: None,
            }],
            agent_context: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("media"));
        assert!(json.contains("abc.jpg"));
        let deserialized: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.media.len(), 1);
        assert_eq!(deserialized.media[0].media_type, "image");
    }

    #[test]
    fn test_agent_message_empty_media_omitted() {
        let msg = AgentMessage {
            id: "msg-1".to_string(),
            from_user: "user@wx".to_string(),
            text: "hello".to_string(),
            timestamp: 1700000000,
            context_token: "token-abc".to_string(),
            message_type: "text".to_string(),
            media: vec![],
            agent_context: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        // media field should not appear in JSON when empty
        assert!(!json.contains("media"));
    }

    #[test]
    fn test_agent_reply_with_media_paths_serialization() {
        let reply = AgentReply {
            reply_to_id: "msg-1".to_string(),
            text: "here is a file".to_string(),
            media_paths: vec!["/tmp/file.pdf".to_string()],
            to_user: None,
            context_token: None,
            agent_context: None,
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains("media_paths"));
        assert!(json.contains("file.pdf"));
        let deserialized: AgentReply = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.media_paths.len(), 1);
    }

    #[test]
    fn test_agent_reply_empty_media_paths_omitted() {
        let reply = AgentReply {
            reply_to_id: "msg-1".to_string(),
            text: "hello".to_string(),
            media_paths: vec![],
            to_user: None,
            context_token: None,
            agent_context: None,
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(!json.contains("media_paths"));
    }

    #[test]
    fn test_agent_reply_proactive_send_roundtrip() {
        let reply = AgentReply {
            reply_to_id: String::new(),
            text: "配对码: 12345678".to_string(),
            media_paths: vec![],
            to_user: Some("wx_user_id".to_string()),
            context_token: Some(String::new()),
            agent_context: None,
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains("to_user"));
        assert!(json.contains("context_token"));
        let deserialized: AgentReply = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.to_user.as_deref(), Some("wx_user_id"));
        assert!(deserialized.context_token.is_some());
        assert!(deserialized.agent_context.is_none());
    }

    #[test]
    fn test_get_upload_url_request_serialization() {
        let req = GetUploadUrlRequest {
            filekey: "abc123".to_string(),
            media_type: 1,
            to_user_id: "user@wx".to_string(),
            rawsize: 1024,
            rawfilemd5: "d41d8cd98f00b204e9800998ecf8427e".to_string(),
            filesize: 1040,
            no_need_thumb: true,
            aeskey: "aabbccdd".to_string(),
            base_info: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""filekey":"abc123""#));
        assert!(json.contains(r#""media_type":1"#));
        assert!(json.contains(r#""to_user_id":"user@wx""#));
        assert!(json.contains(r#""rawsize":1024"#));
        assert!(json.contains("rawfilemd5"));
        assert!(json.contains(r#""filesize":1040"#));
        assert!(json.contains(r#""no_need_thumb":true"#));
        assert!(json.contains(r#""aeskey":"aabbccdd""#));
    }

    #[test]
    fn test_get_upload_url_response_deserialization() {
        let json = r#"{"ret":0,"upload_full_url":"https://novac2c.cdn.weixin.qq.com/c2c?up=abc","upload_param":"up=abc"}"#;
        let resp: GetUploadUrlResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.ret, Some(0));
        assert_eq!(
            resp.upload_full_url.as_deref(),
            Some("https://novac2c.cdn.weixin.qq.com/c2c?up=abc")
        );
        assert_eq!(resp.upload_param.as_deref(), Some("up=abc"));
    }

    #[test]
    fn test_get_upload_url_response_partial() {
        // Some fields may be absent on error responses
        let json = r#"{"ret":-1,"errmsg":"invalid param"}"#;
        let resp: GetUploadUrlResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.ret, Some(-1));
        assert!(resp.upload_full_url.is_none());
        assert!(resp.upload_param.is_none());
    }

    #[test]
    fn test_video_item_serialization() {
        let item = VideoItem {
            cdn_url: Some("https://cdn.weixin.qq.com/video".to_string()),
            aes_key: Some("aes-key-123".to_string()),
            encrypt_query_param: Some("enc=abc".to_string()),
            md5: Some("d41d8cd98f00b204e9800998ecf8427e".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("cdn_url"));
        assert!(json.contains("aes_key"));
        assert!(json.contains("encrypt_query_param"));
        assert!(json.contains("md5"));
        let deserialized: VideoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.cdn_url.as_deref(), Some("https://cdn.weixin.qq.com/video"));
    }

    #[test]
    fn test_video_item_default() {
        let item = VideoItem::default();
        let json = serde_json::to_string(&item).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_message_item_with_video_deserialization() {
        let json = r#"{
            "type": 5,
            "video_item": {
                "cdn_url": "https://cdn.weixin.qq.com/video",
                "aes_key": "key123",
                "encrypt_query_param": "enc=abc",
                "md5": "d41d8cd98f00b204e9800998ecf8427e"
            }
        }"#;
        let item: MessageItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.item_type, Some(5));
        let video = item.video_item.unwrap();
        assert_eq!(video.cdn_url.as_deref(), Some("https://cdn.weixin.qq.com/video"));
        assert_eq!(video.aes_key.as_deref(), Some("key123"));
    }

    #[test]
    fn test_message_item_without_video() {
        let item = MessageItem {
            item_type: Some(msg_type::TEXT),
            ..Default::default()
        };
        assert!(item.video_item.is_none());
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

    #[test]
    fn test_build_media_reply_image_sets_encrypt_and_aes() {
        let msg = WeixinMessage::build_media_reply(
            "ctx-img".to_string(),
            "user@wx".to_string(),
            String::new(),
            msg_type::IMAGE,
            "enc=abc123".to_string(),
            "aes-key-456".to_string(),
            1024,
        );
        assert_eq!(msg.message_type, Some(chat_type::BOT));
        assert_eq!(msg.message_state, Some(message_state::FINISH));
        assert_eq!(msg.to_user_id.as_deref(), Some("user@wx"));

        let items = msg.item_list.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, Some(msg_type::IMAGE));

        let img = items[0].image_item.as_ref().unwrap();
        let media = img.media.as_ref().unwrap();
        assert_eq!(
            media.encrypt_query_param.as_deref(),
            Some("enc=abc123")
        );
        assert_eq!(media.aes_key.as_deref(), Some("aes-key-456"));
        assert_eq!(media.encrypt_type, Some(ENCRYPT_TYPE_AES_128_ECB));
        assert_eq!(img.mid_size, Some(1024));
    }

    #[test]
    fn test_build_media_reply_video_sets_fields() {
        let msg = WeixinMessage::build_media_reply(
            "ctx-vid".to_string(),
            "user@wx".to_string(),
            String::new(),
            msg_type::VIDEO,
            "enc=video123".to_string(),
            "vid-aes-key".to_string(),
            2048,
        );
        let items = msg.item_list.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, Some(msg_type::VIDEO));

        let video = items[0].video_item.as_ref().unwrap();
        let media = video.media.as_ref().unwrap();
        assert_eq!(
            media.encrypt_query_param.as_deref(),
            Some("enc=video123")
        );
        assert_eq!(media.aes_key.as_deref(), Some("vid-aes-key"));
        assert_eq!(video.mid_size, Some(2048));
    }

    #[test]
    fn test_build_media_reply_file_sets_file_name() {
        let msg = WeixinMessage::build_media_reply(
            "ctx-file".to_string(),
            "user@wx".to_string(),
            "report.pdf".to_string(),
            msg_type::FILE,
            "enc=file".to_string(),
            "file-aes".to_string(),
            512,
        );
        let items = msg.item_list.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_type, Some(msg_type::FILE));

        let file = items[0].file_item.as_ref().unwrap();
        assert_eq!(file.file_name.as_deref(), Some("report.pdf"));
        assert!(file.media.is_some());
        assert_eq!(file.mid_size, Some(512));
    }

    #[test]
    fn test_build_media_reply_voice_sets_fields() {
        let msg = WeixinMessage::build_media_reply(
            "ctx-voice".to_string(),
            "user@wx".to_string(),
            String::new(),
            msg_type::VOICE,
            "enc=voice".to_string(),
            "voice-aes".to_string(),
            256,
        );
        let items = msg.item_list.unwrap();
        assert_eq!(items[0].item_type, Some(msg_type::VOICE));

        let voice = items[0].voice_item.as_ref().unwrap();
        let media = voice.media.as_ref().unwrap();
        assert_eq!(media.encrypt_query_param.as_deref(), Some("enc=voice"));
        assert_eq!(media.aes_key.as_deref(), Some("voice-aes"));
    }
}
