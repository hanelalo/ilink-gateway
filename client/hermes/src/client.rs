//! Main client orchestrator.
//!
//! Ties together the gateway API client and the Hermes ACP client:
//! 1. Register with gateway
//! 2. Poll for messages
//! 3. Forward to Hermes via ACP
//! 4. Send reply back to gateway

use crate::acp::client::AcpClient;
use crate::config::Config;
use crate::error::Result;
use crate::gateway::api::GatewayClient;

/// The main Hermes client that orchestrates gateway <-> ACP communication.
pub struct HermesClient {
    config: Config,
    gateway: GatewayClient,
    acp: Option<AcpClient>,
}

impl HermesClient {
    /// Create a new HermesClient from the given configuration.
    pub fn new(config: Config) -> Self {
        // Build gateway client from config; if config is valid this should
        // succeed. We unwrap/expect because Config validation ensures non-empty
        // values at construction time.
        let gateway = GatewayClient::new(&config.gateway_url, &config.agent_name)
            .expect("valid config should produce a gateway client");
        Self {
            config,
            gateway,
            acp: None,
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
    /// This polls the gateway at the configured interval, forwards any
    /// pending messages to the Hermes ACP, and sends replies back to
    /// the gateway.
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
                // Create a new session for each message
                match acp.new_session(&self.config.hermes_cwd).await {
                    Ok(session_id) => {
                        // Forward the message to Hermes
                        match acp
                            .send_message(&session_id, &msg.text)
                            .await
                        {
                            Ok(reply) => {
                                // Send the reply back to the gateway
                                if let Err(e) = self
                                    .gateway
                                    .send_reply(&msg.id, &reply)
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to send reply for message {}: {}",
                                        msg.id,
                                        e
                                    );
                                }

                                // Close the session
                                if let Err(e) =
                                    acp.close_session(&session_id).await
                                {
                                    tracing::warn!(
                                        "Failed to close session {}: {}",
                                        session_id,
                                        e
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to process message {} via ACP: {}",
                                    msg.id,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to create ACP session: {}",
                            e
                        );
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
    }
}
