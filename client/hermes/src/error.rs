use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("ACP protocol error: {0}")]
    Acp(String),

    #[error("Gateway error: {0}")]
    Gateway(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Not connected to gateway")]
    NotConnected,

    #[error("ACP process exited unexpectedly: {0}")]
    AcpProcessExit(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub type Result<T> = std::result::Result<T, ClientError>;
