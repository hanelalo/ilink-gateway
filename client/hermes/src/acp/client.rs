//! Hermes ACP JSON-RPC client.
//!
//! Spawns `hermes acp` as a subprocess and communicates via
//! JSON-RPC 2.0 over stdio.
//!
//! ACP protocol flow:
//! 1. Initialize → get capabilities
//! 2. NewSession → create a session (or LoadSession for existing)
//! 3. Send UserMessageChunk via session/prompt → get streaming AgentMessageChunk responses
//! 4. CloseSession → end session

use crate::error::{ClientError, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

// ------------------------------------------------------------------
// ACP JSON-RPC types
// ------------------------------------------------------------------

/// A JSON-RPC 2.0 request sent to the ACP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpMessage {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// A JSON-RPC 2.0 response received from the ACP server.
#[derive(Debug, Clone, Deserialize)]
pub struct AcpResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<AcpError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Deserialize)]
pub struct AcpError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

// ------------------------------------------------------------------
// ACP method parameter types
// ------------------------------------------------------------------

/// Parameters for the `initialize` method.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_capabilities: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_info: Option<serde_json::Value>,
}

impl Default for InitializeParams {
    fn default() -> Self {
        Self {
            protocol_version: 1,
            client_capabilities: Some(serde_json::json!({})),
            client_info: Some(serde_json::json!({
                "name": "wechat-gateway-hermes",
                "version": "0.1.0",
            })),
        }
    }
}

/// Parameters for the `session/new` method.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<serde_json::Value>>,
}

/// Parameters for the `session/prompt` method (PromptRequest).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    pub prompt: Vec<serde_json::Value>,
    pub session_id: String,
}

/// Parameters for the `session/close` method (CloseSessionRequest).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseSessionParams {
    pub session_id: String,
}

/// Parameters for the `session/load` method.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadSessionParams {
    pub session_id: String,
    pub cwd: String,
}

// ------------------------------------------------------------------
// AcpClient
// ------------------------------------------------------------------

/// Client that communicates with a Hermes ACP subprocess over stdio
/// using JSON-RPC 2.0.
pub struct AcpClient {
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    process: Arc<Mutex<Child>>,
    next_id: AtomicU64,
}

impl AcpClient {
    /// Spawn `hermes acp` and wait for it to be ready.
    ///
    /// # Errors
    ///
    /// Returns errors if the binary cannot be spawned or if stdio pipes
    /// cannot be acquired.
    pub async fn spawn(hermes_bin: &str) -> Result<Self> {
        let mut child = Command::new(hermes_bin)
            .arg("acp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                ClientError::AcpProcessExit(format!(
                    "Failed to spawn '{} acp': {}",
                    hermes_bin, e
                ))
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ClientError::AcpProcessExit(
                "Failed to capture stdin of ACP process".to_string(),
            )
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            ClientError::AcpProcessExit(
                "Failed to capture stdout of ACP process".to_string(),
            )
        })?;

        Ok(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            process: Arc::new(Mutex::new(child)),
            next_id: AtomicU64::new(1),
        })
    }

    /// Generate the next request ID (sequential, starting from 1).
    pub fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send a JSON-RPC request and wait for the response.
    ///
    /// Writes the request to the process's stdin and reads lines from
    /// its stdout until a response matching our request `id` is found.
    /// This handles streaming ACP methods (like `session/prompt`) that
    /// emit intermediate notifications before the final response.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::Acp` if the process has exited or the
    /// response contains an error.
    pub async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_request_id();
        let msg = AcpMessage {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };

        let request_line =
            serde_json::to_string(&msg).map_err(ClientError::Serialize)?;

        {
            let mut stdin = self.stdin.lock().await;
            writeln!(stdin, "{}", request_line).map_err(|e| {
                ClientError::Acp(format!("Failed to write to ACP stdin: {}", e))
            })?;
            stdin.flush().map_err(|e| {
                ClientError::Acp(format!("Failed to flush ACP stdin: {}", e))
            })?;
        }

        // ACP methods like session/prompt emit multiple JSON-RPC lines:
        // intermediate notifications (no `id`) followed by the final
        // response (with our request `id`).  Keep reading until we find
        // the matching response.
        let mut response_line = String::new();
        loop {
            response_line.clear();
            {
                let mut stdout = self.stdout.lock().await;
                stdout.read_line(&mut response_line).map_err(|e| {
                    ClientError::Acp(format!("Failed to read from ACP stdout: {}", e))
                })?;
            }

            if response_line.is_empty() {
                return Err(ClientError::AcpProcessExit(
                    "ACP process closed stdout unexpectedly".to_string(),
                ));
            }

            // Try to parse.  If it's a notification (no `id`) or
            // a response for a different request, skip it.
            let line = response_line.trim();
            let v: serde_json::Value =
                serde_json::from_str(line).map_err(ClientError::Serialize)?;
            if v.get("id") != Some(&serde_json::json!(id)) {
                continue;
            }

            let resp: AcpResponse =
                serde_json::from_str(line).map_err(ClientError::Serialize)?;

            if let Some(err) = resp.error {
                return Err(ClientError::Acp(format!(
                    "ACP error (code {}): {}",
                    err.code, err.message
                )));
            }

            return resp.result.ok_or_else(|| {
                ClientError::Acp(
                    "ACP response missing both result and error".to_string(),
                )
            });
        }
    }

    /// Initialize the ACP connection (handshake).
    ///
    /// Sends the `initialize` method with protocol version and
    /// capabilities.
    ///
    /// # Errors
    ///
    /// Returns ACP protocol errors.
    pub async fn initialize(&self) -> Result<serde_json::Value> {
        let params = InitializeParams::default();
        let params_value = serde_json::to_value(params)?;
        self.send_request("initialize", Some(params_value)).await
    }

    /// Create a new session.
    ///
    /// Returns the session ID string.
    ///
    /// # Errors
    ///
    /// Returns ACP protocol errors.
    pub async fn new_session(&self, cwd: &str) -> Result<String> {
        let params = NewSessionParams {
            cwd: cwd.to_string(),
            mcp_servers: Some(vec![]),
        };
        let params_value = serde_json::to_value(params)?;
        let result = self
            .send_request("session/new", Some(params_value))
            .await?;

        let session_id = result
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ClientError::Acp(
                    "sessions/new response missing sessionId".to_string(),
                )
            })?;

        Ok(session_id.to_string())
    }

    /// Load an existing session.
    ///
    /// Returns the session ID string on success.
    ///
    /// # Errors
    ///
    /// Returns ACP protocol errors if the session does not exist.
    pub async fn load_session(&self, session_id: &str, cwd: &str) -> Result<String> {
        let params = LoadSessionParams {
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
        };
        let params_value = serde_json::to_value(params)?;
        let result = self
            .send_request("session/load", Some(params_value))
            .await?;

        let sid = result
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ClientError::Acp(
                    "session/load response missing sessionId".to_string(),
                )
            })?;

        Ok(sid.to_string())
    }

    /// Send a user message to a session and collect the complete reply text.
    ///
    /// This sends a `session/prompt` request and collects text from
    /// `session/update` notifications (`AgentMessageChunk`) that
    /// arrive before the final `PromptResponse`.
    ///
    /// # Errors
    ///
    /// Returns ACP protocol errors.
    pub async fn send_message(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<String> {
        let params = PromptParams {
            prompt: vec![serde_json::json!({
                "type": "text",
                "text": text,
            })],
            session_id: session_id.to_string(),
        };
        let params_value = serde_json::to_value(params)?;

        let id = self.next_request_id();
        let msg = AcpMessage {
            jsonrpc: "2.0".to_string(),
            id,
            method: "session/prompt".to_string(),
            params: Some(params_value),
        };

        let request_line =
            serde_json::to_string(&msg).map_err(ClientError::Serialize)?;

        {
            let mut stdin = self.stdin.lock().await;
            writeln!(stdin, "{}", request_line).map_err(|e| {
                ClientError::Acp(format!("Failed to write to ACP stdin: {}", e))
            })?;
            stdin.flush().map_err(|e| {
                ClientError::Acp(format!("Failed to flush ACP stdin: {}", e))
            })?;
        }

        // Read streaming lines: collect text from session/update
        // notifications (AgentMessageChunk), stop at the final
        // PromptResponse matching our request id.
        let mut reply_texts: Vec<String> = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            {
                let mut stdout = self.stdout.lock().await;
                stdout.read_line(&mut line).map_err(|e| {
                    ClientError::Acp(format!("Failed to read from ACP stdout: {}", e))
                })?;
            }

            if line.is_empty() {
                return Err(ClientError::AcpProcessExit(
                    "ACP process closed stdout unexpectedly".to_string(),
                ));
            }

            let v: serde_json::Value =
                serde_json::from_str(line.trim()).map_err(ClientError::Serialize)?;

            // If this is the final response matching our request id, stop
            if v.get("id") == Some(&serde_json::json!(id)) {
                break;
            }

            // Collect text only from session/update notifications whose
            // sessionUpdate == "agent_message_chunk".  Other update types
            // (agent_thought_chunk, user_message_chunk, tool_call_*, etc.)
            // carry content.text too but must NOT be mixed into the reply.
            let is_agent_message = v
                .get("params")
                .and_then(|p| p.get("update"))
                .and_then(|u| u.get("sessionUpdate"))
                .and_then(|s| s.as_str())
                == Some("agent_message_chunk");

            if is_agent_message {
                if let Some(text_content) = v
                    .get("params")
                    .and_then(|p| p.get("update"))
                    .and_then(|u| u.get("content"))
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.as_str())
                {
                    reply_texts.push(text_content.to_string());
                }
            }
        }

        Ok(reply_texts.join(""))
    }

    /// Close a session.
    ///
    /// # Errors
    ///
    /// Returns ACP protocol errors.
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        let params = CloseSessionParams {
            session_id: session_id.to_string(),
        };
        let params_value = serde_json::to_value(params)?;
        self.send_request("session/close", Some(params_value)).await?;
        Ok(())
    }

    /// Shut down the ACP process.
    ///
    /// # Errors
    ///
    /// Returns IO errors from killing the process.
    pub fn shutdown(&mut self) -> Result<()> {
        // We need exclusive access to the process. Using try_lock since
        // this is a synchronous method — if the async lock is held, this
        // will fail (which is acceptable during shutdown).
        let mut process = self.process.try_lock().map_err(|_| {
            ClientError::Acp(
                "Could not acquire process lock for shutdown".to_string(),
            )
        })?;
        process.kill()?;
        process.wait()?;
        Ok(())
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // JSON-RPC serialization / deserialization
    // ------------------------------------------------------------------

    #[test]
    fn test_acp_message_serialization() {
        let msg = AcpMessage {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "initialize".to_string(),
            params: Some(serde_json::json!({"key": "value"})),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let expected =
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"key":"value"}}"#;
        assert_eq!(json, expected);
    }

    #[test]
    fn test_acp_message_no_params_omits_field() {
        let msg = AcpMessage {
            jsonrpc: "2.0".to_string(),
            id: 2,
            method: "sessions/abc/close".to_string(),
            params: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        // params should be absent
        assert!(!json.contains("params"));
        assert_eq!(
            json,
            r#"{"jsonrpc":"2.0","id":2,"method":"sessions/abc/close"}"#
        );
    }

    #[test]
    fn test_acp_response_deserialization_with_result() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"abc-123"}}"#;
        let resp: AcpResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, 1);
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        assert_eq!(result["sessionId"], "abc-123");
    }

    #[test]
    fn test_acp_response_deserialization_with_error() {
        let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found","data":null}}"#;
        let resp: AcpResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, 1);
        assert!(resp.result.is_none());

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
        assert!(err.data.is_none());
    }

    #[test]
    fn test_acp_error_deserialization() {
        let json =
            r#"{"code":-32700,"message":"Parse error","data":{"detail":"bad json"}}"#;
        let err: AcpError = serde_json::from_str(json).unwrap();

        assert_eq!(err.code, -32700);
        assert_eq!(err.message, "Parse error");
        assert_eq!(err.data.unwrap()["detail"], "bad json");
    }

    #[test]
    fn test_acp_error_without_data() {
        let json = r#"{"code":-32600,"message":"Invalid Request"}"#;
        let err: AcpError = serde_json::from_str(json).unwrap();

        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid Request");
        assert!(err.data.is_none());
    }

    // ------------------------------------------------------------------
    // Request ID generation
    // ------------------------------------------------------------------

    #[test]
    fn test_next_request_id_starts_at_one() {
        // We can't test with spawn() since it requires a real binary,
        // so we create a minimal test harness
        let id_counter = AtomicU64::new(1);

        assert_eq!(id_counter.fetch_add(1, Ordering::SeqCst), 1);
        assert_eq!(id_counter.fetch_add(1, Ordering::SeqCst), 2);
        assert_eq!(id_counter.fetch_add(1, Ordering::SeqCst), 3);
    }

    #[test]
    fn test_next_request_id_is_sequential() {
        // Simulate what AcpClient does internally
        let next_id = AtomicU64::new(1);

        let ids: Vec<u64> = (0..5)
            .map(|_| next_id.fetch_add(1, Ordering::SeqCst))
            .collect();

        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
    }

    // ------------------------------------------------------------------
    // InitializeParams serialization
    // ------------------------------------------------------------------

    #[test]
    fn test_initialize_params_serialization() {
        let params = InitializeParams {
            protocol_version: 1,
            client_capabilities: Some(serde_json::json!({})),
            client_info: Some(serde_json::json!({
                "name": "wechat-gateway-hermes",
                "version": "0.1.0",
            })),
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["protocolVersion"], 1);
        assert!(json.get("clientCapabilities").is_some());
        assert_eq!(json["clientInfo"]["name"], "wechat-gateway-hermes");
        assert_eq!(json["clientInfo"]["version"], "0.1.0");
    }

    #[test]
    fn test_initialize_params_default() {
        let params = InitializeParams::default();

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["protocolVersion"], 1);
        assert!(json.get("clientCapabilities").is_some());
        assert_eq!(json["clientInfo"]["name"], "wechat-gateway-hermes");
    }

    // ------------------------------------------------------------------
    // NewSessionParams serialization
    // ------------------------------------------------------------------

    #[test]
    fn test_new_session_params_serialization() {
        let params = NewSessionParams {
            cwd: "/Users/test".to_string(),
            mcp_servers: None,
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["cwd"], "/Users/test");
        assert!(json.get("mcpServers").is_none(), "mcpServers should be absent when None");
    }

    #[test]
    fn test_new_session_params_without_mode() {
        let params = NewSessionParams {
            cwd: "/Users/test".to_string(),
            mcp_servers: None,
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["cwd"], "/Users/test");
    }

    #[test]
    fn test_new_session_params_with_mcp_servers() {
        let params = NewSessionParams {
            cwd: "/Users/test".to_string(),
            mcp_servers: Some(vec![]),
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["cwd"], "/Users/test");
        assert_eq!(json["mcpServers"].as_array().unwrap().len(), 0);
    }

    // ------------------------------------------------------------------
    // PromptParams serialization
    // ------------------------------------------------------------------

    #[test]
    fn test_prompt_params_serialization() {
        let params = PromptParams {
            prompt: vec![serde_json::json!({
                "type": "text",
                "text": "hello",
            })],
            session_id: "sess-123".to_string(),
        };

        let json = serde_json::to_value(&params).unwrap();
        let prompt = json["prompt"].as_array().unwrap();
        assert_eq!(prompt[0]["type"], "text");
        assert_eq!(prompt[0]["text"], "hello");
        assert_eq!(json["sessionId"], "sess-123");
    }

    // ------------------------------------------------------------------
    // CloseSessionParams serialization
    // ------------------------------------------------------------------

    #[test]
    fn test_close_session_params_serialization() {
        let params = CloseSessionParams {
            session_id: "sess-xyz".to_string(),
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["sessionId"], "sess-xyz");
    }

    // ------------------------------------------------------------------
    // Full JSON-RPC message round-trip
    // ------------------------------------------------------------------

    #[test]
    fn test_acp_message_roundtrip() {
        let original = AcpMessage {
            jsonrpc: "2.0".to_string(),
            id: 42,
            method: "test/method".to_string(),
            params: Some(serde_json::json!({"foo": "bar"})),
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: AcpMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.jsonrpc, "2.0");
        assert_eq!(deserialized.id, 42);
        assert_eq!(deserialized.method, "test/method");
        assert_eq!(deserialized.params.unwrap()["foo"], "bar");
    }

    #[test]
    fn test_acp_response_roundtrip() {
        let json = r#"{"jsonrpc":"2.0","id":5,"result":{"ok":true}}"#;
        let resp: AcpResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, 5);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap()["ok"], true);
    }

    // ------------------------------------------------------------------
    // Send message format (UserMessageChunk)
    // ------------------------------------------------------------------

    #[test]
    fn test_prompt_params_content_as_blocks() {
        let params = PromptParams {
            prompt: vec![serde_json::json!({
                "type": "text",
                "text": "Hello, Hermes!",
            })],
            session_id: "sess-abc".to_string(),
        };

        let json = serde_json::to_value(&params).unwrap();
        let blocks = json["prompt"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "Hello, Hermes!");
        assert_eq!(json["sessionId"], "sess-abc");
    }
}
