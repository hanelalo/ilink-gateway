use crate::error::Result;

/// Configuration for the Hermes client.
#[derive(Debug, Clone)]
pub struct Config {
    /// wechat-gateway base URL (e.g. "http://127.0.0.1:8765")
    pub gateway_url: String,

    /// Agent name to register with the gateway.
    pub agent_name: String,

    /// Path to the `hermes` executable.
    pub hermes_bin: String,

    /// Working directory for Hermes ACP sessions.
    pub hermes_cwd: String,

    /// Poll interval in seconds for gateway messages.
    pub poll_interval_secs: u64,

    /// ACP session timeout in seconds (max wait for a reply).
    pub acp_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gateway_url: "http://127.0.0.1:8765".to_string(),
            agent_name: "hermes".to_string(),
            hermes_bin: "hermes".to_string(),
            hermes_cwd: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            poll_interval_secs: 1,
            acp_timeout_secs: 300,
        }
    }
}

impl Config {
    /// Create config from environment variables.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            gateway_url: std::env::var("GW_GATEWAY_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8765".to_string()),
            agent_name: std::env::var("GW_AGENT_NAME")
                .unwrap_or_else(|_| "hermes".to_string()),
            hermes_bin: std::env::var("GW_HERMES_BIN")
                .unwrap_or_else(|_| "hermes".to_string()),
            hermes_cwd: std::env::var("GW_HERMES_CWD")
                .unwrap_or_else(|_| {
                    std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| ".".to_string())
                }),
            poll_interval_secs: std::env::var("GW_POLL_INTERVAL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
            acp_timeout_secs: std::env::var("GW_ACP_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert_eq!(cfg.gateway_url, "http://127.0.0.1:8765");
        assert_eq!(cfg.agent_name, "hermes");
        assert_eq!(cfg.poll_interval_secs, 1);
    }

    #[test]
    fn test_from_env_falls_back_to_defaults() {
        // Without env vars set, should use defaults
        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.gateway_url, "http://127.0.0.1:8765");
        assert_eq!(cfg.agent_name, "hermes");
    }
}
