//! Gateway HTTP API client.
//!
//! Communicates with the wechat-gateway REST API:
//! - POST /api/agents/register
//! - GET /api/agents/{name}/poll
//! - POST /api/agents/{name}/reply

use crate::error::{ClientError, Result};
use serde::{Deserialize, Serialize};

/// A message received from the gateway (polled from WeChat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayMessage {
    pub id: String,
    pub from_user: String,
    pub text: String,
    pub timestamp: i64,
    pub context_token: String,
    pub message_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegisterResponse {
    ok: bool,
    active_agent: Option<String>,
    wechat_connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PollResponse {
    messages: Vec<GatewayMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplyRequest {
    reply_to_id: String,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplyResponse {
    ok: bool,
}

/// Client for the wechat-gateway REST API.
#[derive(Debug, Clone)]
pub struct GatewayClient {
    pub(crate) base_url: String,
    pub(crate) agent_name: String,
    client: reqwest::Client,
}

impl GatewayClient {
    /// Create a new gateway API client.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::Gateway` if the base URL or agent name is empty.
    pub fn new(base_url: &str, agent_name: &str) -> Result<Self> {
        if base_url.is_empty() {
            return Err(ClientError::Gateway(
                "base_url cannot be empty".to_string(),
            ));
        }
        if agent_name.is_empty() {
            return Err(ClientError::Gateway(
                "agent_name cannot be empty".to_string(),
            ));
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            agent_name: agent_name.to_string(),
            client: reqwest::Client::new(),
        })
    }

    /// Register this agent with the gateway.
    ///
    /// Returns `(active_agent, wechat_connected)` on success.
    ///
    /// # Errors
    ///
    /// Returns HTTP or gateway errors.
    pub async fn register(&self) -> Result<(Option<String>, bool)> {
        let url = format!("{}/api/agents/register", self.base_url);
        let body = serde_json::json!({
            "name": self.agent_name,
            "capabilities": ["text"],
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    ClientError::Gateway(format!(
                        "Cannot connect to gateway at {}: {}",
                        self.base_url, e
                    ))
                } else {
                    ClientError::from(e)
                }
            })?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClientError::Gateway(format!(
                "Gateway register returned {}: {}",
                status, text
            )));
        }

        let reg: RegisterResponse = resp.json().await?;

        if !reg.ok {
            return Err(ClientError::Gateway(
                "Gateway returned ok=false on register".to_string(),
            ));
        }

        Ok((reg.active_agent, reg.wechat_connected))
    }

    /// Poll for pending messages from the gateway.
    ///
    /// Returns an empty vec if no messages are available.
    ///
    /// # Errors
    ///
    /// Returns HTTP or gateway errors.
    pub async fn poll_messages(&self) -> Result<Vec<GatewayMessage>> {
        let url = format!(
            "{}/api/agents/{}/poll",
            self.base_url, self.agent_name
        );

        let resp = self.client.get(&url).send().await.map_err(|e| {
            if e.is_connect() {
                ClientError::Gateway(format!(
                    "Cannot connect to gateway at {}: {}",
                    self.base_url, e
                ))
            } else {
                ClientError::from(e)
            }
        })?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClientError::Gateway(format!(
                "Gateway poll returned {}: {}",
                status, text
            )));
        }

        let poll: PollResponse = resp.json().await?;
        Ok(poll.messages)
    }

    /// Send a reply to a WeChat message through the gateway.
    ///
    /// # Errors
    ///
    /// Returns HTTP or gateway errors.
    pub async fn send_reply(&self, reply_to_id: &str, text: &str) -> Result<()> {
        let url = format!(
            "{}/api/agents/{}/reply",
            self.base_url, self.agent_name
        );

        let body = ReplyRequest {
            reply_to_id: reply_to_id.to_string(),
            text: text.to_string(),
        };

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    ClientError::Gateway(format!(
                        "Cannot connect to gateway at {}: {}",
                        self.base_url, e
                    ))
                } else {
                    ClientError::from(e)
                }
            })?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClientError::Gateway(format!(
                "Gateway reply returned {}: {}",
                status, text
            )));
        }

        let rep: ReplyResponse = resp.json().await?;
        if !rep.ok {
            return Err(ClientError::Gateway(
                "Gateway returned ok=false on reply".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // GatewayClient::new
    // ------------------------------------------------------------------

    #[test]
    fn test_new_creates_client_with_correct_url() {
        let client = GatewayClient::new("http://localhost:8080", "hermes").unwrap();
        assert_eq!(client.base_url, "http://localhost:8080");
        assert_eq!(client.agent_name, "hermes");
    }

    #[test]
    fn test_new_trims_trailing_slash() {
        let client =
            GatewayClient::new("http://localhost:8080/", "hermes").unwrap();
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_new_rejects_empty_base_url() {
        let err = GatewayClient::new("", "hermes").unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));
    }

    #[test]
    fn test_new_rejects_empty_agent_name() {
        let err = GatewayClient::new("http://localhost:8080", "").unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));
    }

    // ------------------------------------------------------------------
    // GatewayClient::register
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_register_sends_correct_request_and_parses_response() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/register")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"active_agent":"hermes","wechat_connected":true}"#,
            )
            .expect(1)
            .match_body(mockito::Matcher::JsonString(
                r#"{"name":"hermes","capabilities":["text"]}"#.to_string(),
            ))
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let (active_agent, wechat_connected) = client.register().await.unwrap();

        assert_eq!(active_agent, Some("hermes".to_string()));
        assert!(wechat_connected);

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_register_handles_connection_refused() {
        // Use a URL that nobody is listening on
        let client =
            GatewayClient::new("http://127.0.0.1:1", "hermes").unwrap();
        let err = client.register().await.unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));
    }

    #[tokio::test]
    async fn test_register_handles_http_error_status() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/register")
            .with_status(500)
            .with_body("Internal Server Error")
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let err = client.register().await.unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_register_handles_ok_false() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/register")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":false,"active_agent":null,"wechat_connected":false}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let err = client.register().await.unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));

        mock.assert_async().await;
    }

    // ------------------------------------------------------------------
    // GatewayClient::poll_messages
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_poll_messages_returns_messages_when_available() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("GET", "/api/agents/hermes/poll")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "messages": [
                        {
                            "id": "msg1",
                            "from_user": "user123",
                            "text": "Hello",
                            "timestamp": 1700000000,
                            "context_token": "ctx1",
                            "message_type": "text"
                        }
                    ]
                }"#,
            )
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let messages = client.poll_messages().await.unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "msg1");
        assert_eq!(messages[0].from_user, "user123");
        assert_eq!(messages[0].text, "Hello");
        assert_eq!(messages[0].timestamp, 1700000000);
        assert_eq!(messages[0].context_token, "ctx1");
        assert_eq!(messages[0].message_type, "text");

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_poll_messages_returns_empty_vec_when_no_messages() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("GET", "/api/agents/hermes/poll")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"messages":[]}"#)
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let messages = client.poll_messages().await.unwrap();

        assert!(messages.is_empty());

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_poll_messages_handles_connection_refused() {
        let client =
            GatewayClient::new("http://127.0.0.1:1", "hermes").unwrap();
        let err = client.poll_messages().await.unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));
    }

    // ------------------------------------------------------------------
    // GatewayClient::send_reply
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_send_reply_sends_correct_body() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/hermes/reply")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true}"#)
            .expect(1)
            .match_body(mockito::Matcher::JsonString(
                r#"{"reply_to_id":"msg1","text":"Hello back"}"#.to_string(),
            ))
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        client.send_reply("msg1", "Hello back").await.unwrap();

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_send_reply_handles_error_response() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/hermes/reply")
            .with_status(500)
            .with_body("Server Error")
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let err = client.send_reply("msg1", "Hello back").await.unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_send_reply_handles_ok_false() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/hermes/reply")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false}"#)
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let err = client.send_reply("msg1", "Hello back").await.unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_send_reply_handles_connection_refused() {
        let client =
            GatewayClient::new("http://127.0.0.1:1", "hermes").unwrap();
        let err = client.send_reply("msg1", "Hello").await.unwrap_err();
        assert!(matches!(err, ClientError::Gateway(_)));
    }
}
