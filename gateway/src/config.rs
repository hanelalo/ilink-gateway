use crate::error::Result;

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub http_addr: String,
    pub http_port: u16,
    pub ilink_base_url: String,
    pub cdn_base_url: String,
    pub db_path: String,
    #[allow(dead_code)]
    pub cmd_timeout_secs: u64,
    pub cmd_max_output_chars: usize,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            http_addr: "127.0.0.1".to_string(),
            http_port: 8765,
            ilink_base_url: crate::ilink::types::ILINK_BASE_URL.to_string(),
            cdn_base_url: crate::ilink::types::ILINK_CDN_BASE_URL.to_string(),
            db_path: "~/.wechat-gateway/data.db".to_string(),
            cmd_timeout_secs: 30,
            cmd_max_output_chars: 2000,
        }
    }
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self> {
        // Simple env-based config for now.
        Ok(GatewayConfig {
            http_addr: std::env::var("GW_HTTP_ADDR").unwrap_or_else(|_| "127.0.0.1".to_string()),
            http_port: std::env::var("GW_HTTP_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8765),
            ilink_base_url: std::env::var("GW_ILINK_BASE_URL")
                .unwrap_or_else(|_| crate::ilink::types::ILINK_BASE_URL.to_string()),
            cdn_base_url: std::env::var("GW_CDN_BASE_URL")
                .unwrap_or_else(|_| crate::ilink::types::ILINK_CDN_BASE_URL.to_string()),
            db_path: std::env::var("GW_DB_PATH")
                .unwrap_or_else(|_| "~/.wechat-gateway/data.db".to_string()),
            cmd_timeout_secs: std::env::var("GW_CMD_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            cmd_max_output_chars: std::env::var("GW_CMD_MAX_OUTPUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2000),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.http_port, 8765);
        assert_eq!(cfg.cmd_timeout_secs, 30);
    }

    #[test]
    fn test_config_from_env() {
        // Should fall back to defaults when env vars are not set.
        let cfg = GatewayConfig::from_env().unwrap();
        assert_eq!(cfg.http_addr, "127.0.0.1");
    }
}
