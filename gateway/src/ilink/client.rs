//! iLink HTTP client implementation.
//!
//! Handles WeChat ClawBot iLink protocol:
//! - QR code login
//! - Long-poll getupdates
//! - Send message / typing
//! - Get config (typing ticket)

use crate::error::{GatewayError, Result};
use crate::ilink::types::*;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

/// HTTP client for the WeChat iLink Bot API.
pub struct Client {
    client: reqwest::Client,
    base_url: String,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Generate a per-request `X-WECHAT-UIN` header value.
///
/// The value is a random `u32` converted to its decimal string then
/// base64-encoded, as required by the iLink protocol.
fn generate_uin() -> String {
    let n: u32 = rand::rng().random();
    BASE64.encode(n.to_string())
}

/// Build the standard set of iLink request headers.
///
/// Every iLink request carries:
/// - `AuthorizationType: ilink_bot_token`
/// - `X-WECHAT-UIN: <base64(random_u32_string)>`  (unique per call)
/// - `Authorization: Bearer <token>`               (only when `token` is `Some`)
fn build_headers(token: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();

    headers.insert(
        "AuthorizationType".parse::<reqwest::header::HeaderName>().unwrap(),
        HeaderValue::from_static("ilink_bot_token"),
    );

    headers.insert(
        "X-WECHAT-UIN".parse::<reqwest::header::HeaderName>().unwrap(),
        HeaderValue::from_str(&generate_uin()).unwrap(),
    );

    if let Some(token) = token {
        let auth_value = format!("Bearer {token}");
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&auth_value).unwrap());
    }

    headers
}

/// Try to parse a JSON body and fail with `Ilink` if `ret` is non-zero.
///
/// Used by endpoints that return `Result<()>` (send_typing, notify_start).
fn check_ret(body: &str) -> Result<()> {
    if body.is_empty() {
        return Ok(());
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(ret) = v.get("ret").and_then(|r| r.as_i64()) {
            if ret != 0 {
                let errmsg = v
                    .get("errmsg")
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown error");
                return Err(GatewayError::Ilink(errmsg.to_string()));
            }
        }
    }
    Ok(())
}

// ── Client API ──────────────────────────────────────────────────────────────

impl Client {
    /// Create a new iLink HTTP client.
    ///
    /// When `base_url` is `None` the default [`ILINK_BASE_URL`] is used.
    pub fn new(base_url: Option<String>) -> Result<Self> {
        let client = reqwest::Client::builder().build()?;
        let base_url = base_url.unwrap_or_else(|| ILINK_BASE_URL.to_string());
        Ok(Client { client, base_url })
    }

    /// `GET /ilink/bot/get_bot_qrcode?bot_type=3` — obtain a QR code for
    /// scanning with the WeChat mobile app.
    pub async fn get_qr_code(&self) -> Result<GetQrcodeResponse> {
        let url = format!("{}/ilink/bot/get_bot_qrcode?bot_type=3", self.base_url);
        let resp = self
            .client
            .get(&url)
            .headers(build_headers(None))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GatewayError::Ilink(format!("HTTP {}", resp.status())));
        }
        Ok(serde_json::from_str(&resp.text().await?)?)
    }

    /// `GET /ilink/bot/get_qrcode_status?qrcode=<key>` — poll the current
    /// login status for the given QR code key.
    pub async fn poll_qr_status(&self, qrcode: &str) -> Result<QrcodeStatusResponse> {
        let url = format!(
            "{}/ilink/bot/get_qrcode_status?qrcode={}",
            self.base_url,
            urlencoding(qrcode)
        );
        let resp = self
            .client
            .get(&url)
            .headers(build_headers(None))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GatewayError::Ilink(format!("HTTP {}", resp.status())));
        }
        Ok(serde_json::from_str(&resp.text().await?)?)
    }

    /// `POST /ilink/bot/getupdates` — long-poll for new messages.
    ///
    /// The server holds the request for ~35 seconds.  A timeout returns an
    /// empty response.
    pub async fn get_updates(
        &self,
        sync_buf: &str,
        timeout: Option<u32>,
    ) -> Result<GetUpdatesResponse> {
        let url = format!("{}/ilink/bot/getupdates", self.base_url);
        let body = GetUpdatesRequest {
            get_updates_buf: sync_buf.to_string(),
            base_info: None,
            timeout,
        };
        let resp = self
            .client
            .post(&url)
            .headers(build_headers(None))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GatewayError::Ilink(format!("HTTP {}", resp.status())));
        }
        Ok(serde_json::from_str(&resp.text().await?)?)
    }

    /// `POST /ilink/bot/sendmessage` — send a message to a WeChat user.
    pub async fn send_message(
        &self,
        token: &str,
        req: &SendMessageRequest,
    ) -> Result<SendMessageResponse> {
        let url = format!("{}/ilink/bot/sendmessage", self.base_url);
        let resp = self
            .client
            .post(&url)
            .headers(build_headers(Some(token)))
            .json(req)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GatewayError::Ilink(format!("HTTP {}", resp.status())));
        }
        Ok(serde_json::from_str(&resp.text().await?)?)
    }

    /// `POST /ilink/bot/sendtyping` — show or cancel the "typing" indicator.
    ///
    /// Status: 1 = start typing, 2 = stop typing.
    pub async fn send_typing(
        &self,
        token: &str,
        req: &SendTypingRequest,
    ) -> Result<()> {
        let url = format!("{}/ilink/bot/sendtyping", self.base_url);
        let resp = self
            .client
            .post(&url)
            .headers(build_headers(Some(token)))
            .json(req)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GatewayError::Ilink(format!("HTTP {}", resp.status())));
        }
        check_ret(&resp.text().await?)
    }

    /// `POST /ilink/bot/getconfig` — obtain the typing ticket for a user.
    pub async fn get_config(
        &self,
        token: &str,
        req: &GetConfigRequest,
    ) -> Result<GetConfigResponse> {
        let url = format!("{}/ilink/bot/getconfig", self.base_url);
        let resp = self
            .client
            .post(&url)
            .headers(build_headers(Some(token)))
            .json(req)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GatewayError::Ilink(format!("HTTP {}", resp.status())));
        }
        Ok(serde_json::from_str(&resp.text().await?)?)
    }

    /// `POST /ilink/bot/getuploadurl` — obtain CDN upload URL and AES key.
    pub async fn get_upload_url(
        &self,
        token: &str,
        req: &GetUploadUrlRequest,
    ) -> Result<GetUploadUrlResponse> {
        let url = format!("{}/ilink/bot/getuploadurl", self.base_url);
        let resp = self
            .client
            .post(&url)
            .headers(build_headers(Some(token)))
            .json(req)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GatewayError::Ilink(format!("HTTP {}", resp.status())));
        }
        Ok(serde_json::from_str(&resp.text().await?)?)
    }

    /// `POST /ilink/bot/msg/notifystart` — required at connection start
    /// to signal the bot is ready to receive updates.
    pub async fn notify_start(&self, token: &str) -> Result<()> {
        let url = format!("{}/ilink/bot/msg/notifystart", self.base_url);
        let resp = self
            .client
            .post(&url)
            .headers(build_headers(Some(token)))
            .json(&serde_json::json!({}))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(GatewayError::Ilink(format!("HTTP {}", resp.status())));
        }
        check_ret(&resp.text().await?)
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Minimal percent-encoding for a query-parameter value.
///
/// This is used for the QR code key in `poll_qr_status`.  A full-blown
/// URL-encoding crate would be overkill here, so we handle just the characters
/// that the iLink API may produce (e.g. `/`, `+`).
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push_str("%20"),
            _ => {
                // Percent-encode everything else.
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── UIN helper tests ────────────────────────────────────────────────

    #[test]
    fn test_generate_uin_produces_base64() {
        let uin = generate_uin();
        // Base64 uses A-Z, a-z, 0-9, +, /, and = for padding.
        assert!(
            uin.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            "UIN should be valid base64: {uin}"
        );
        assert!(!uin.is_empty(), "UIN should not be empty");
    }

    #[test]
    fn test_generate_uin_changes_each_call() {
        let uin1 = generate_uin();
        let uin2 = generate_uin();
        assert_ne!(
            uin1, uin2,
            "X-WECHAT-UIN must change on every call"
        );
    }

    // ── Header tests ────────────────────────────────────────────────────

    #[test]
    fn test_build_headers_without_token() {
        let headers = build_headers(None);

        assert!(
            headers.contains_key("authorizationtype"),
            "should contain AuthorizationType"
        );
        assert!(
            headers.contains_key("x-wechat-uin"),
            "should contain X-WECHAT-UIN"
        );
        assert!(
            !headers.contains_key("authorization"),
            "should NOT contain Authorization when no token given"
        );
    }

    #[test]
    fn test_build_headers_with_token() {
        let headers = build_headers(Some("my-token"));

        assert_eq!(
            headers.get("authorizationtype").unwrap().to_str().unwrap(),
            "ilink_bot_token",
        );
        assert!(
            headers.contains_key("x-wechat-uin"),
            "should contain X-WECHAT-UIN"
        );
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert_eq!(auth, "Bearer my-token");
    }

    // ── QR code ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_qr_code_deserializes() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("GET", "/ilink/bot/get_bot_qrcode")
            .match_query(mockito::Matcher::UrlEncoded("bot_type".into(), "3".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "ret": 0,
                    "qrcode": "qr-key-abc",
                    "qrcode_img_content": "base64img...",
                    "errmsg": "ok"
                })
                .to_string(),
            )
            .create();

        let resp = client.get_qr_code().await.unwrap();
        assert_eq!(resp.ret, 0);
        assert_eq!(resp.qrcode.as_deref(), Some("qr-key-abc"));
        assert_eq!(resp.qrcode_img_content.as_deref(), Some("base64img..."));

        _mock.assert();
    }

    #[tokio::test]
    async fn test_get_qr_code_sends_required_headers() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("GET", "/ilink/bot/get_bot_qrcode")
            .match_query(mockito::Matcher::UrlEncoded("bot_type".into(), "3".into()))
            .match_header("authorizationtype", "ilink_bot_token")
            .match_header("x-wechat-uin", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": 0}).to_string())
            .create();

        client.get_qr_code().await.unwrap();
        _mock.assert();
    }

    // ── QR status ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_poll_qr_status_returns_confirmed_token() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("GET", "/ilink/bot/get_qrcode_status")
            .match_query(mockito::Matcher::UrlEncoded("qrcode".into(), "key-123".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "ret": 0,
                    "status": "confirmed",
                    "bot_token": "bot-token-xyz",
                    "baseurl": "https://cn.ilink.example.com",
                    "ilink_bot_id": "bot-001",
                    "ilink_user_id": "user@wx"
                })
                .to_string(),
            )
            .create();

        let resp = client.poll_qr_status("key-123").await.unwrap();
        assert_eq!(resp.ret, 0);
        assert_eq!(resp.status.as_deref(), Some("confirmed"));
        assert_eq!(resp.bot_token.as_deref(), Some("bot-token-xyz"));
        assert_eq!(resp.baseurl.as_deref(), Some("https://cn.ilink.example.com"));
        assert_eq!(resp.ilink_bot_id.as_deref(), Some("bot-001"));
        assert_eq!(resp.ilink_user_id.as_deref(), Some("user@wx"));

        _mock.assert();
    }

    #[tokio::test]
    async fn test_poll_qr_status_handles_wait() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("GET", "/ilink/bot/get_qrcode_status")
            .match_query(mockito::Matcher::UrlEncoded("qrcode".into(), "key-456".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": 101, "status": "wait", "errmsg": "waiting for scan"}).to_string())
            .create();

        let resp = client.poll_qr_status("key-456").await.unwrap();
        assert_eq!(resp.ret, 101);
        assert_eq!(resp.status.as_deref(), Some("wait"));
        assert_eq!(resp.bot_token, None);

        _mock.assert();
    }

    #[tokio::test]
    async fn test_poll_qr_status_handles_expired() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("GET", "/ilink/bot/get_qrcode_status")
            .match_query(mockito::Matcher::UrlEncoded("qrcode".into(), "key-exp".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({"ret": 102, "status": "expired", "errmsg": "QR code expired"}).to_string(),
            )
            .create();

        let resp = client.poll_qr_status("key-exp").await.unwrap();
        assert_eq!(resp.ret, 102);
        assert_eq!(resp.status.as_deref(), Some("expired"));

        _mock.assert();
    }

    // ── GetUpdates ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_updates_returns_messages() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("POST", "/ilink/bot/getupdates")
            .match_header("content-type", "application/json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "ret": 0,
                    "msgs": [
                        {
                            "seq": 42,
                            "message_id": 1001,
                            "from_user_id": "user@wx",
                            "message_type": 1,
                            "item_list": [
                                {"type": 1, "text_item": {"text": "hello"}}
                            ],
                            "context_token": "ctx-abc"
                        }
                    ],
                    "get_updates_buf": "buf-next"
                })
                .to_string(),
            )
            .create();

        let resp = client.get_updates("buf-init", Some(30)).await.unwrap();
        assert_eq!(resp.ret, Some(0));
        assert!(resp.msgs.is_some());
        let msgs = resp.msgs.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].seq, Some(42));
        assert_eq!(msgs[0].from_user_id.as_deref(), Some("user@wx"));
        assert_eq!(msgs[0].text(), Some("hello"));
        assert_eq!(resp.get_updates_buf.as_deref(), Some("buf-next"));

        _mock.assert();
    }

    #[tokio::test]
    async fn test_get_updates_handles_timeout_empty_response() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("POST", "/ilink/bot/getupdates")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "ret": 0,
                    "msgs": [],
                    "get_updates_buf": "buf-same"
                })
                .to_string(),
            )
            .create();

        let resp = client.get_updates("buf-same", Some(35)).await.unwrap();
        assert_eq!(resp.ret, Some(0));
        let msgs = resp.msgs.unwrap_or_default();
        assert!(msgs.is_empty(), "no messages expected on timeout");

        _mock.assert();
    }

    // ── SendMessage ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_send_message_sends_correct_body() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let req = SendMessageRequest {
            msg: WeixinMessage {
                to_user_id: Some("target@wx".into()),
                message_type: Some(chat_type::BOT),
                item_list: Some(vec![MessageItem {
                    item_type: Some(msg_type::TEXT),
                    text_item: Some(TextItem {
                        text: Some("hi there".into()),
                    }),
                    ..Default::default()
                }]),
                context_token: Some("ctx-999".into()),
                ..Default::default()
            },
            base_info: None,
        };

        let _mock = server
            .mock("POST", "/ilink/bot/sendmessage")
            .match_header("authorization", "Bearer bot-token-xyz")
            .match_header("authorizationtype", "ilink_bot_token")
            .match_body(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": 0, "errmsg": "ok"}).to_string())
            .create();

        let resp = client.send_message("bot-token-xyz", &req).await.unwrap();
        assert_eq!(resp.ret, Some(0));
        _mock.assert();
    }

    // ── SendTyping ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_send_typing_start() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let req = SendTypingRequest {
            ilink_user_id: "user@wx".into(),
            typing_ticket: "ticket-abc".into(),
            status: 1,
            base_info: None,
        };

        let _mock = server
            .mock("POST", "/ilink/bot/sendtyping")
            .match_header("authorization", "Bearer tok-1")
            .match_body(mockito::Matcher::JsonString(
                json!({
                    "ilink_user_id": "user@wx",
                    "typing_ticket": "ticket-abc",
                    "status": 1
                })
                .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": 0}).to_string())
            .create();

        let result = client.send_typing("tok-1", &req).await;
        assert!(result.is_ok(), "send_typing should succeed: {result:?}");
        _mock.assert();
    }

    #[tokio::test]
    async fn test_send_typing_stop() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let req = SendTypingRequest {
            ilink_user_id: "user@wx".into(),
            typing_ticket: "ticket-abc".into(),
            status: 2,
            base_info: None,
        };

        let _mock = server
            .mock("POST", "/ilink/bot/sendtyping")
            .match_header("authorization", "Bearer tok-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": 0}).to_string())
            .create();

        let result = client.send_typing("tok-1", &req).await;
        assert!(result.is_ok());
        _mock.assert();
    }

    // ── GetConfig ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_config_returns_typing_ticket() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let req = GetConfigRequest {
            ilink_user_id: "user@wx".into(),
            context_token: Some("ctx-abc".into()),
            base_info: None,
        };

        let _mock = server
            .mock("POST", "/ilink/bot/getconfig")
            .match_header("authorization", "Bearer tok-2")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": 0, "typing_ticket": "ticket-xyz", "errmsg": "ok"}).to_string())
            .create();

        let resp = client.get_config("tok-2", &req).await.unwrap();
        assert_eq!(resp.ret, Some(0));
        assert_eq!(resp.typing_ticket.as_deref(), Some("ticket-xyz"));
        _mock.assert();
    }

    // ── NotifyStart ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_notify_start_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("POST", "/ilink/bot/msg/notifystart")
            .match_header("authorization", "Bearer start-token")
            .match_header("authorizationtype", "ilink_bot_token")
            .match_header("content-type", "application/json")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": 0}).to_string())
            .create();

        let result = client.notify_start("start-token").await;
        assert!(result.is_ok(), "notify_start should succeed: {result:?}");
        _mock.assert();
    }

    #[tokio::test]
    async fn test_notify_start_fails_on_nonzero_ret() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("POST", "/ilink/bot/msg/notifystart")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": -1, "errmsg": "invalid token"}).to_string())
            .create();

        let result = client.notify_start("bad-token").await;
        assert!(result.is_err(), "non-zero ret should produce an error");
        match result {
            Err(GatewayError::Ilink(msg)) => assert_eq!(msg, "invalid token"),
            other => panic!("expected Ilink error, got {other:?}"),
        }
        _mock.assert();
    }

    // ── GetUploadUrl ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_upload_url_returns_cdn_url_and_key() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let req = GetUploadUrlRequest {
            aes_key: "test-aes-key-123".to_string(),
            item_type: msg_type::IMAGE,
            file_size: 65536,
            file_md5: "d41d8cd98f00b204e9800998ecf8427e".to_string(),
            base_info: None,
        };

        let _mock = server
            .mock("POST", "/ilink/bot/getuploadurl")
            .match_header("authorization", "Bearer upload-token")
            .match_header("authorizationtype", "ilink_bot_token")
            .match_body(mockito::Matcher::JsonString(
                json!({
                    "aes_key": "test-aes-key-123",
                    "type": 2,
                    "file_size": 65536,
                    "file_md5": "d41d8cd98f00b204e9800998ecf8427e"
                })
                .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "ret": 0,
                    "cdnurl": "https://cdn.weixin.qq.com/upload",
                    "aes_key": "response-aes-key",
                    "errmsg": "ok"
                })
                .to_string(),
            )
            .create();

        let resp = client.get_upload_url("upload-token", &req).await.unwrap();
        assert_eq!(resp.ret, 0);
        assert_eq!(
            resp.cdnurl.as_deref(),
            Some("https://cdn.weixin.qq.com/upload")
        );
        assert_eq!(resp.aes_key.as_deref(), Some("response-aes-key"));
        _mock.assert();
    }

    #[tokio::test]
    async fn test_get_upload_url_handles_http_error() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let req = GetUploadUrlRequest {
            aes_key: "key".to_string(),
            item_type: 2,
            file_size: 1024,
            file_md5: "abc".to_string(),
            base_info: None,
        };

        let _mock = server
            .mock("POST", "/ilink/bot/getuploadurl")
            .with_status(400)
            .create();

        let result = client.get_upload_url("token", &req).await;
        assert!(result.is_err(), "HTTP 400 should propagate as an error");
        _mock.assert();
    }

    // ── HTTP error propagation ──────────────────────────────────────────

    #[tokio::test]
    async fn test_get_qr_code_returns_error_on_http_failure() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        let _mock = server
            .mock("GET", "/ilink/bot/get_bot_qrcode")
            .match_query(mockito::Matcher::UrlEncoded("bot_type".into(), "3".into()))
            .with_status(500)
            .create();

        let result = client.get_qr_code().await;
        assert!(result.is_err(), "HTTP 500 should propagate as an error");
        _mock.assert();
    }

    #[tokio::test]
    async fn test_poll_qr_status_urlencodes_qrcode() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::new(Some(server.url())).unwrap();

        // QR code keys can contain characters like '/' that need encoding.
        let _mock = server
            .mock("GET", "/ilink/bot/get_qrcode_status")
            .match_query(mockito::Matcher::UrlEncoded("qrcode".into(), "key/with/slashes".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({"ret": 101, "status": "wait"}).to_string())
            .create();

        let _resp = client.poll_qr_status("key/with/slashes").await.unwrap();
        _mock.assert();
    }

    // ── Client construction ─────────────────────────────────────────────

    #[test]
    fn test_new_with_default_base_url() {
        let client = Client::new(None).unwrap();
        assert_eq!(client.base_url, ILINK_BASE_URL);
    }

    #[test]
    fn test_new_with_custom_base_url() {
        let client =
            Client::new(Some("http://localhost:9999".into())).unwrap();
        assert_eq!(client.base_url, "http://localhost:9999");
    }

    // ── urlencoding helper ──────────────────────────────────────────────

    #[test]
    fn test_urlencoding_encodes_special_chars() {
        assert_eq!(urlencoding("hello"), "hello");
        assert_eq!(urlencoding("a b"), "a%20b");
        assert_eq!(urlencoding("key/with/slashes"), "key%2Fwith%2Fslashes");
        assert_eq!(urlencoding("plus+sign"), "plus%2Bsign");
    }
}
