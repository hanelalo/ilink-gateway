use crate::error::Result;
use std::collections::HashSet;

/// DM (private message) admission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmPolicy {
    /// Reject all DMs.
    Disabled,
    /// Allow DMs from approved users only.
    ///
    /// NOTE: the full pairing approval flow (`hermes pairing approve`) is not
    /// implemented in this gateway — treat as `Allowlist` for now.
    Pairing,
    /// Allow DMs only from users in the allowlist.
    Allowlist,
    /// Allow all DMs.
    Open,
}

impl DmPolicy {
    fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "disabled" => Self::Disabled,
            "pairing" => Self::Pairing,
            "allowlist" => Self::Allowlist,
            "open" => Self::Open,
            _ => Self::Open,
        }
    }
}

/// Group message admission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupPolicy {
    /// Reject all group messages.
    Disabled,
    /// Accept messages from any group.
    All,
    /// Accept messages only from allowlisted groups.
    Allowlist,
}

impl GroupPolicy {
    fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "disabled" => Self::Disabled,
            "all" => Self::All,
            "allowlist" => Self::Allowlist,
            _ => Self::Disabled,
        }
    }
}

/// Parse a comma-separated list into a set of trimmed, non-empty strings.
fn parse_set(s: &str) -> HashSet<String> {
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

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
    pub dm_policy: DmPolicy,
    pub group_policy: GroupPolicy,
    pub allowed_users: HashSet<String>,
    pub allowed_groups: HashSet<String>,
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
            dm_policy: DmPolicy::Open,
            group_policy: GroupPolicy::Disabled,
            allowed_users: HashSet::new(),
            allowed_groups: HashSet::new(),
        }
    }
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self> {
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
            dm_policy: std::env::var("GW_DM_POLICY")
                .map(|v| DmPolicy::parse(&v))
                .unwrap_or(DmPolicy::Open),
            group_policy: std::env::var("GW_GROUP_POLICY")
                .map(|v| GroupPolicy::parse(&v))
                .unwrap_or(GroupPolicy::Disabled),
            allowed_users: std::env::var("GW_ALLOWED_USERS")
                .map(|v| parse_set(&v))
                .unwrap_or_default(),
            allowed_groups: std::env::var("GW_ALLOWED_GROUPS")
                .map(|v| parse_set(&v))
                .unwrap_or_default(),
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
        assert_eq!(cfg.dm_policy, DmPolicy::Open);
        assert_eq!(cfg.group_policy, GroupPolicy::Disabled);
    }

    #[test]
    fn test_config_from_env() {
        // Should fall back to defaults when env vars are not set.
        let cfg = GatewayConfig::from_env().unwrap();
        assert_eq!(cfg.http_addr, "127.0.0.1");
    }

    #[test]
    fn test_dm_policy_parse() {
        assert_eq!(DmPolicy::parse("disabled"), DmPolicy::Disabled);
        assert_eq!(DmPolicy::parse("pairing"), DmPolicy::Pairing);
        assert_eq!(DmPolicy::parse("allowlist"), DmPolicy::Allowlist);
        assert_eq!(DmPolicy::parse("open"), DmPolicy::Open);
        assert_eq!(DmPolicy::parse("OPEN"), DmPolicy::Open);
        assert_eq!(DmPolicy::parse("bogus"), DmPolicy::Open);
    }

    #[test]
    fn test_group_policy_parse() {
        assert_eq!(GroupPolicy::parse("disabled"), GroupPolicy::Disabled);
        assert_eq!(GroupPolicy::parse("all"), GroupPolicy::All);
        assert_eq!(GroupPolicy::parse("allowlist"), GroupPolicy::Allowlist);
        assert_eq!(GroupPolicy::parse("bogus"), GroupPolicy::Disabled);
    }

    #[test]
    fn test_parse_set() {
        let s = parse_set("wxid_a, wxid_b ,,wxid_c");
        assert_eq!(s.len(), 3);
        assert!(s.contains("wxid_a"));
        assert!(s.contains("wxid_b"));
        assert!(s.contains("wxid_c"));
    }

    #[test]
    fn test_parse_set_empty() {
        assert!(parse_set("").is_empty());
        assert!(parse_set(" , , ").is_empty());
    }
}
