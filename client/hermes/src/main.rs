//! Hermes client binary for wechat-gateway.
//!
//! Connects to the wechat-gateway REST API and Hermes ACP subprocess,
//! polling for WeChat messages and forwarding them to Hermes for processing.
//!
//! ## Startup flow
//!
//! 1. Load [`Config`] from environment variables
//! 2. Initialize tracing/logging
//! 3. Create [`GatewayClient`] and register with the gateway
//! 4. Spawn `hermes acp` as a child process via [`AcpClient`]
//! 5. Run the ACP initialize handshake
//! 6. Enter a poll loop:
//!    - Poll the gateway for pending messages
//!    - For each message, create an ACP session and forward the text
//!    - Send any reply back through the gateway
//!    - Close the session
//!    - Sleep for the configured poll interval

use std::time::Duration;

use wechat_gateway_client_hermes::acp::client::AcpClient;
use wechat_gateway_client_hermes::config::Config;
use wechat_gateway_client_hermes::error::{ClientError, Result};
use wechat_gateway_client_hermes::gateway::api::GatewayClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing/logging with env-filter support.
    // Respects RUST_LOG; defaults to "info" if unset.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // 1. Load configuration from environment variables
    let config = Config::from_env()?;

    // 2. Create the gateway API client
    let gateway = GatewayClient::new(&config.gateway_url, &config.agent_name)?;

    // 3. Register this agent with the wechat-gateway
    tracing::info!(
        agent_name = %config.agent_name,
        gateway_url = %config.gateway_url,
        "Registering with gateway"
    );
    let (_active_agent, _wechat_connected) = gateway.register().await?;
    tracing::info!("Registered successfully");

    // 4. Spawn the Hermes ACP subprocess
    tracing::info!(hermes_bin = %config.hermes_bin, "Spawning ACP process");
    let acp = AcpClient::spawn(&config.hermes_bin).await?;

    // 5. Initialize the ACP connection (protocol handshake)
    tracing::info!("Initializing ACP connection");
    let _caps = acp.initialize().await?;
    tracing::info!("ACP initialized successfully");

    // 6. Enter the poll loop
    tracing::info!(
        poll_interval_secs = config.poll_interval_secs,
        "Starting poll loop"
    );
    Ok(poll_loop(&gateway, &acp, &config).await?)
}

/// Main poll loop.
///
/// Fetches pending messages from the gateway, processes each through a
/// dedicated ACP session, sends any reply back, then sleeps for the
/// configured interval.
async fn poll_loop(
    gateway: &GatewayClient,
    acp: &AcpClient,
    config: &Config,
) -> Result<()> {
    let poll_delay = Duration::from_secs(config.poll_interval_secs);

    loop {
        let messages = gateway.poll_messages().await?;

        for msg in &messages {
            tracing::info!(
                msg_id = %msg.id,
                from_user = %msg.from_user,
                "Processing message"
            );

            // Create a new ACP session for this message
            let session_id = acp.new_session(&config.hermes_cwd).await?;
            tracing::debug!(session_id = %session_id, "Created ACP session");

            // Forward the message text to Hermes and collect the reply
            let reply = acp.send_message(&session_id, &msg.text).await?;
            tracing::debug!(session_id = %session_id, "Got reply from ACP");

            // Send the reply back through the gateway (if non-empty)
            if !reply.is_empty() {
                gateway.send_reply(&msg.id, &reply).await?;
                tracing::debug!(msg_id = %msg.id, "Sent reply to gateway");
            }

            // Close the ACP session
            acp.close_session(&session_id).await?;
            tracing::debug!(session_id = %session_id, "Closed ACP session");
        }

        tokio::time::sleep(poll_delay).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // ---------------------------------------------------------------
    // Config integration tests
    // ---------------------------------------------------------------

    #[test]
    fn test_config_from_env_uses_defaults() {
        let config = Config::from_env().unwrap();

        assert_eq!(config.gateway_url, "http://127.0.0.1:8765");
        assert_eq!(config.agent_name, "hermes");
        assert_eq!(config.hermes_bin, "hermes");
        assert_eq!(config.poll_interval_secs, 1);
        assert_eq!(config.acp_timeout_secs, 300);
    }

    #[test]
    fn test_config_from_env_respects_env_vars() {
        // Save original values to restore after the test
        let restore = EnvRestore::new(&[
            "GW_GATEWAY_URL",
            "GW_AGENT_NAME",
            "GW_HERMES_BIN",
            "GW_HERMES_CWD",
            "GW_POLL_INTERVAL",
            "GW_ACP_TIMEOUT",
        ]);

        env::set_var("GW_GATEWAY_URL", "http://test.local:9999");
        env::set_var("GW_AGENT_NAME", "integration-test-agent");
        env::set_var("GW_HERMES_BIN", "/usr/local/bin/hermes");
        env::set_var("GW_POLL_INTERVAL", "15");
        env::set_var("GW_ACP_TIMEOUT", "120");

        let config = Config::from_env().unwrap();

        assert_eq!(config.gateway_url, "http://test.local:9999");
        assert_eq!(config.agent_name, "integration-test-agent");
        assert_eq!(config.hermes_bin, "/usr/local/bin/hermes");
        assert_eq!(config.poll_interval_secs, 15);
        assert_eq!(config.acp_timeout_secs, 120);

        // Restore original env vars (drop guard runs here)
        drop(restore);
    }

    // ---------------------------------------------------------------
    // GatewayClient integration tests with mockito
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn test_gateway_client_register_with_mock_server() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/register")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"active_agent":"hermes","wechat_connected":true}"#,
            )
            .match_body(mockito::Matcher::JsonString(
                r#"{"name":"hermes","capabilities":["text"]}"#.to_string(),
            ))
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let (active_agent, wechat_connected) = client.register().await.unwrap();

        assert_eq!(active_agent, Some("hermes".to_string()));
        assert!(wechat_connected);

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_gateway_client_poll_messages_with_mock_server() {
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
                            "text": "Hello from WeChat",
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
        assert_eq!(messages[0].text, "Hello from WeChat");
        assert_eq!(messages[0].timestamp, 1700000000);
        assert_eq!(messages[0].context_token, "ctx1");
        assert_eq!(messages[0].message_type, "text");

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_gateway_client_poll_returns_empty_when_no_messages() {
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
    async fn test_gateway_client_send_reply_with_mock_server() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/hermes/reply")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true}"#)
            .match_body(mockito::Matcher::JsonString(
                r#"{"reply_to_id":"msg1","text":"Hello back"}"#.to_string(),
            ))
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        client.send_reply("msg1", "Hello back").await.unwrap();

        mock.assert_async().await;
    }

    // ---------------------------------------------------------------
    // GatewayClient error propagation tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn test_gateway_client_register_handles_http_error() {
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
        assert!(
            matches!(err, ClientError::Gateway(_)),
            "expected Gateway error, got {:?}",
            err
        );

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_gateway_client_send_reply_handles_http_error() {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("POST", "/api/agents/hermes/reply")
            .with_status(500)
            .with_body("Server Error")
            .expect(1)
            .create_async()
            .await;

        let client = GatewayClient::new(&server.url(), "hermes").unwrap();
        let err = client.send_reply("msg1", "Hello").await.unwrap_err();
        assert!(
            matches!(err, ClientError::Gateway(_)),
            "expected Gateway error, got {:?}",
            err
        );

        mock.assert_async().await;
    }

    // ---------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------

    /// Saves the current values of environment variables and restores
    /// them on drop. Used to prevent env-var pollution between tests.
    struct EnvRestore {
        keys: Vec<String>,
        saved: Vec<Option<String>>,
    }

    impl EnvRestore {
        fn new(keys: &[&str]) -> Self {
            let saved: Vec<Option<String>> = keys
                .iter()
                .map(|k| env::var(k).ok())
                .collect();
            Self {
                keys: keys.iter().map(|k| k.to_string()).collect(),
                saved,
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (i, key) in self.keys.iter().enumerate() {
                match &self.saved[i] {
                    Some(val) => env::set_var(key, val),
                    None => env::remove_var(key),
                }
            }
        }
    }
}
