// Shared error types for wechat-gateway.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("iLink protocol error: {0}")]
    Ilink(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Agent offline: {0}")]
    #[allow(dead_code)]
    AgentOffline(String),

    #[error("Command execution error: {0}")]
    Command(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Not connected to WeChat")]
    #[allow(dead_code)]
    NotConnected,
}

pub type Result<T> = std::result::Result<T, GatewayError>;
