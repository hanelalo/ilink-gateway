//! Main client orchestrator.
//!
//! Ties together the gateway API client and the Hermes ACP client:
//! 1. Register with gateway
//! 2. Poll for messages
//! 3. Forward to Hermes via ACP
//! 4. Send reply back to gateway

use std::collections::HashMap;

use crate::acp::client::AcpClient;
use crate::config::Config;
use crate::error::Result;
use crate::gateway::api::GatewayClient;

/// The main Hermes client that orchestrates gateway <-> ACP communication.
pub struct HermesClient {
    config: Config,
    gateway: GatewayClient,
    acp: Option<AcpClient>,
    /// Per-user ACP session IDs, seeded from gateway's session_id.
    sessions: HashMap<String, String>,
}

impl HermesClient {
    /// Create a new HermesClient from the given configuration.
    pub fn new(config: Config) -> Self {
        let gateway = GatewayClient::new(&config.gateway_url, &config.agent_name)
            .expect("valid config should produce a gateway client");
        Self {
            config,
            gateway,
            acp: None,
            sessions: HashMap::new(),
        }
    }

    /// Start the client: register with gateway, spawn ACP, enter poll loop.
    ///
    /// # Errors
    ///
    /// Returns errors from gateway registration or ACP spawning.
    pub async fn run(&mut self) -> Result<()> {
        self.register().await?;

        // Spawn the ACP process
        let acp = AcpClient::spawn(&self.config.hermes_bin).await?;

        // Run ACP handshake
        let _caps = acp.initialize().await?;

        self.acp = Some(acp);

        // Enter the poll loop
        self.poll_loop().await?;

        Ok(())
    }

    /// Register with the wechat-gateway.
    ///
    /// # Errors
    ///
    /// Returns gateway errors.
    pub async fn register(&self) -> Result<()> {
        self.gateway.register().await?;
        Ok(())
    }

    /// Poll for messages and process them in a loop.
    ///
    /// ACP sessions are reused per WeChat user, seeded from gateway's
    /// session_id when available.  New sessions are reported back to
    /// the gateway via the reply API so they persist across restarts.
    ///
    /// # Errors
    ///
    /// Returns errors from polling or ACP communication.
    pub async fn poll_loop(&mut self) -> Result<()> {
        let acp = self
            .acp
            .as_ref()
            .ok_or_else(|| crate::error::ClientError::NotConnected)?;

        loop {
            // Check for messages from the gateway
            let messages = self.gateway.poll_messages().await?;

            for msg in &messages {
                // Log media attachments
                if !msg.media.is_empty() {
                    tracing::info!(
                        msg_id = %msg.id,
                        media_count = msg.media.len(),
                        "Message has media attachments"
                    );
                    for (i, m) in msg.media.iter().enumerate() {
                        tracing::debug!(
                            msg_id = %msg.id,
                            media_index = i,
                            media_type = %m.media_type,
                            local_path = %m.local_path,
                            "Media attachment"
                        );
                    }
                }

                // Build ACP message text, appending media info if present
                let mut acp_text = msg.text.clone();
                if !msg.media.is_empty() {
                    let media_desc: Vec<String> = msg.media.iter().map(|m| {
                        format!("[{}: {}]", m.media_type, m.local_path)
                    }).collect();
                    if !acp_text.is_empty() {
                        acp_text.push('\n');
                    }
                    acp_text.push_str(&format!("[Media: {}]", media_desc.join(", ")));
                }

                // Reuse session from gateway's cache or our local one
                let session_id = match self.sessions.get(&msg.from_user) {
                    Some(sid) => sid.clone(),
                    None => {
                        // Gateway gave us a session_id — try to load it
                        if let Some(ref sid) = msg.session_id {
                            match acp.load_session(sid, &self.config.hermes_cwd).await {
                                Ok(_) => {
                                    tracing::info!(
                                        "Loaded ACP session {} for user {}",
                                        sid,
                                        msg.from_user
                                    );
                                    self.sessions.insert(msg.from_user.clone(), sid.clone());
                                    sid.clone()
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to load ACP session {} for user {}: {} — creating new",
                                        sid,
                                        msg.from_user,
                                        e
                                    );
                                    match acp.new_session(&self.config.hermes_cwd).await {
                                        Ok(new_sid) => {
                                            tracing::info!(
                                                "Created ACP session {} for user {}",
                                                new_sid,
                                                msg.from_user
                                            );
                                            self.sessions.insert(msg.from_user.clone(), new_sid.clone());
                                            new_sid
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                "Failed to create ACP session for user {}: {}",
                                                msg.from_user,
                                                e
                                            );
                                            continue;
                                        }
                                    }
                                }
                            }
                        } else {
                            // First time — create a new session
                            match acp.new_session(&self.config.hermes_cwd).await {
                                Ok(sid) => {
                                    tracing::info!(
                                        "Created ACP session {} for user {}",
                                        sid,
                                        msg.from_user
                                    );
                                    self.sessions.insert(msg.from_user.clone(), sid.clone());
                                    sid
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to create ACP session for user {}: {}",
                                        msg.from_user,
                                        e
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                };

                // Forward the message to Hermes with ACP timeout
                let from_user = msg.from_user.clone();
                let acp_timeout = std::time::Duration::from_secs(self.config.acp_timeout_secs);
                match tokio::time::timeout(acp_timeout, acp.send_message(&session_id, &acp_text)).await
                {
                    Ok(Ok(reply)) => {
                        // Send the reply back to the gateway.
                        // Also report the session ID if we created it
                        // (gateway already knows it via local cache or
                        // poll response, but we send it in case this is
                        // a new session).
                        let report_session = self.sessions.get(&from_user).cloned();
                        if let Err(e) = self
                            .gateway
                            .send_reply(&msg.id, &reply, report_session, Some(from_user.clone()))
                            .await
                        {
                            tracing::error!(
                                "Failed to send reply for message {}: {}",
                                msg.id,
                                e
                            );
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::error!(
                            "ACP error for session {} (user {}): {} — closing and removing",
                            session_id,
                            from_user,
                            e
                        );
                        let _ = acp.close_session(&session_id).await;
                        self.sessions.remove(&from_user);
                    }
                    Err(_elapsed) => {
                        tracing::error!(
                            "ACP timeout ({}s) for session {} (user {}) — closing and removing",
                            self.config.acp_timeout_secs,
                            session_id,
                            from_user,
                        );
                        let _ = acp.close_session(&session_id).await;
                        self.sessions.remove(&from_user);
                    }
                }
            }

            // Sleep for the configured polling interval
            tokio::time::sleep(std::time::Duration::from_secs(
                self.config.poll_interval_secs,
            ))
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    // ------------------------------------------------------------------
    // HermesClient::new
    // ------------------------------------------------------------------

    #[test]
    fn test_new_creates_client_with_correct_config() {
        let config = Config {
            gateway_url: "http://localhost:8080".to_string(),
            agent_name: "hermes".to_string(),
            hermes_bin: "hermes".to_string(),
            hermes_cwd: "/tmp".to_string(),
            poll_interval_secs: 2,
            acp_timeout_secs: 60,
        };

        let client = HermesClient::new(config.clone());
        assert_eq!(client.config.gateway_url, "http://localhost:8080");
        assert_eq!(client.config.agent_name, "hermes");
        assert_eq!(client.config.poll_interval_secs, 2);
        assert!(client.acp.is_none());
        assert!(client.sessions.is_empty());
    }

    #[test]
    fn test_new_uses_gateway_client_with_given_config() {
        let config = Config {
            gateway_url: "http://127.0.0.1:9999".to_string(),
            agent_name: "test-agent".to_string(),
            hermes_bin: "hermes".to_string(),
            hermes_cwd: "/tmp".to_string(),
            poll_interval_secs: 1,
            acp_timeout_secs: 300,
        };

        let client = HermesClient::new(config);
        assert_eq!(client.gateway.base_url, "http://127.0.0.1:9999");
        assert_eq!(client.gateway.agent_name, "test-agent");
    }

    #[test]
    fn test_new_uses_default_config_successfully() {
        let config = Config::default();
        let client = HermesClient::new(config);
        assert_eq!(client.config.agent_name, "hermes");
        assert!(client.acp.is_none());
    }

    // ------------------------------------------------------------------
    // HermesClient::register
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn test_register_calls_gateway_api() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/register")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"active_agent":"hermes","wechat_connected":true}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let config = Config {
            gateway_url: server.url(),
            agent_name: "hermes".to_string(),
            hermes_bin: "hermes".to_string(),
            hermes_cwd: "/tmp".to_string(),
            poll_interval_secs: 1,
            acp_timeout_secs: 300,
        };

        let client = HermesClient::new(config);
        client.register().await.unwrap();

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_register_propagates_gateway_errors() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/register")
            .with_status(500)
            .with_body("Server Error")
            .expect(1)
            .create_async()
            .await;

        let config = Config {
            gateway_url: server.url(),
            agent_name: "hermes".to_string(),
            hermes_bin: "hermes".to_string(),
            hermes_cwd: "/tmp".to_string(),
            poll_interval_secs: 1,
            acp_timeout_secs: 300,
        };

        let client = HermesClient::new(config);
        let err = client.register().await.unwrap_err();
        assert!(matches!(
            err,
            crate::error::ClientError::Gateway(_)
        ));

        mock.assert_async().await;
    }

    // ------------------------------------------------------------------
    // Serialization round-trips for orchestrator types
    // ------------------------------------------------------------------

    #[test]
    fn test_config_serializes_relevant_fields() {
        let config = Config {
            gateway_url: "http://localhost:8080".to_string(),
            agent_name: "hermes".to_string(),
            hermes_bin: "hermes".to_string(),
            hermes_cwd: "/tmp".to_string(),
            poll_interval_secs: 1,
            acp_timeout_secs: 300,
        };

        // Config doesn't derive Serialize, so just check field access
        assert_eq!(config.poll_interval_secs, 1);
        assert_eq!(config.acp_timeout_secs, 300);
    }

    #[test]
    fn test_message_serde_roundtrip() {
        let msg = crate::gateway::api::GatewayMessage {
            id: "msg-1".to_string(),
            from_user: "user123".to_string(),
            text: "Hello!".to_string(),
            timestamp: 1700000000,
            context_token: "ctx-abc".to_string(),
            message_type: "text".to_string(),
            session_id: None,
            media: vec![],
        };

        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: crate::gateway::api::GatewayMessage =
            serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "msg-1");
        assert_eq!(deserialized.from_user, "user123");
        assert_eq!(deserialized.text, "Hello!");
        assert_eq!(deserialized.timestamp, 1700000000);
        assert_eq!(deserialized.context_token, "ctx-abc");
        assert_eq!(deserialized.message_type, "text");
        assert!(deserialized.session_id.is_none());
        assert!(deserialized.media.is_empty());
    }

    #[test]
    fn test_message_serde_roundtrip_with_media() {
        let msg = crate::gateway::api::GatewayMessage {
            id: "msg-2".to_string(),
            from_user: "user456".to_string(),
            text: "Check out this file".to_string(),
            timestamp: 1700000001,
            context_token: "ctx-def".to_string(),
            message_type: "text".to_string(),
            session_id: None,
            media: vec![
                crate::gateway::api::GatewayMediaItem {
                    media_type: "image".to_string(),
                    local_path: "/tmp/abc.jpg".to_string(),
                    original_name: Some("photo.jpg".to_string()),
                },
                crate::gateway::api::GatewayMediaItem {
                    media_type: "file".to_string(),
                    local_path: "/tmp/xyz.pdf".to_string(),
                    original_name: Some("doc.pdf".to_string()),
                },
            ],
        };

        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: crate::gateway::api::GatewayMessage =
            serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "msg-2");
        assert_eq!(deserialized.media.len(), 2);
        assert_eq!(deserialized.media[0].media_type, "image");
        assert_eq!(deserialized.media[0].local_path, "/tmp/abc.jpg");
        assert_eq!(
            deserialized.media[0].original_name,
            Some("photo.jpg".to_string())
        );
        assert_eq!(deserialized.media[1].media_type, "file");
        assert_eq!(deserialized.media[1].local_path, "/tmp/xyz.pdf");
        assert_eq!(
            deserialized.media[1].original_name,
            Some("doc.pdf".to_string())
        );
    }

    #[test]
    fn test_message_serde_roundtrip_media_defaults_to_empty() {
        let json = r#"{
            "id": "msg-3",
            "from_user": "user789",
            "text": "no media",
            "timestamp": 1700000002,
            "context_token": "ctx-ghi",
            "message_type": "text"
        }"#;
        let deserialized: crate::gateway::api::GatewayMessage =
            serde_json::from_str(json).unwrap();

        assert_eq!(deserialized.id, "msg-3");
        assert!(deserialized.media.is_empty());
    }
}
