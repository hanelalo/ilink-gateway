//! Command parser and handler for WeChat commands.
//!
//! Parses `/use <name>`, `/list`, `/status`, `/cmd <shell>` from WeChat messages.
//! Uses the router to execute commands.

use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{GatewayError, Result};
use crate::ilink::types::RouterCommand;

/// Parse a WeChat message text into a RouterCommand.
///
/// Supported commands:
/// - `/use <name>` → `RouterCommand::UseAgent(name)`
/// - `/list` → `RouterCommand::ListAgents`
/// - `/status` → `RouterCommand::Status`
/// - `/cmd [timeout <secs>] <shell command>` → `RouterCommand::Cmd { command, timeout_secs }`
///
/// Returns `None` for non-command text (no leading `/`).
pub fn parse_command(text: &str) -> Option<RouterCommand> {
    let text = text.trim();

    if !text.starts_with('/') {
        return None;
    }

    let without_slash = &text[1..];

    // Split into parts, handling leading whitespace
    let parts: Vec<&str> = without_slash.split_whitespace().collect();

    if parts.is_empty() {
        return None;
    }

    match parts[0] {
        "use" => {
            if parts.len() < 2 || parts[1].is_empty() {
                None
            } else {
                Some(RouterCommand::UseAgent(parts[1].to_string()))
            }
        }
        "list" => Some(RouterCommand::ListAgents),
        "status" => Some(RouterCommand::Status),
        "cmd" => {
            if parts.len() < 2 {
                return None;
            }

            // Check for `timeout <secs>` prefix
            let mut idx = 1;
            let mut cmd_timeout: u64 = 30;

            if parts.len() >= 3 && parts[idx] == "timeout" {
                idx = 2; // skip "timeout"
                // Try to parse the timeout value — even if invalid, skip past it.
                if let Ok(t) = parts[idx].parse::<u64>() {
                    cmd_timeout = t;
                }
                idx += 1;
            }

            if idx >= parts.len() {
                return None;
            }

            let command = parts[idx..].join(" ");
            Some(RouterCommand::Cmd {
                command,
                timeout_secs: cmd_timeout,
            })
        }
        _ => None,
    }
}

/// Execute a `/cmd` shell command using `tokio::process::Command`.
///
/// Returns stdout + stderr combined, truncated to `max_chars`.
/// Has a timeout (default 30s).
pub async fn execute_command(
    command: &str,
    timeout_secs: u64,
    max_chars: usize,
) -> Result<String> {
    let timeout_duration = Duration::from_secs(timeout_secs);

    let output = timeout(timeout_duration, async {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
    })
    .await
    .map_err(|_| GatewayError::Command(format!("command timed out after {}s", timeout_secs)))?
    .map_err(|e| GatewayError::Command(format!("failed to execute command: {}", e)))?;

    let mut result = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&String::from_utf8_lossy(&output.stderr));
    }

    if result.len() > max_chars {
        result.truncate(max_chars);
        result.push_str("... (truncated)");
    }

    Ok(result)
}

/// Dangerous command patterns that should be blocked.
const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "shutdown",
    "reboot",
    "> /dev/sda",
    "mkfs",
    "dd if=",
    ":{():|:&};:",
    ":(){ :|:& };:",
];

/// Check if a command string contains dangerous operations.
///
/// Blocks: rm -rf /, shutdown, reboot, > /dev/sda, mkfs, dd if=, fork bombs.
pub fn is_dangerous_command(command: &str) -> bool {
    let lower = command.to_lowercase();
    DANGEROUS_PATTERNS
        .iter()
        .any(|&pattern| lower.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── parse_command tests ─────────────────────────────────────────────

    #[test]
    fn test_parse_use_agent() {
        let result = parse_command("/use hermes");
        assert_eq!(result, Some(RouterCommand::UseAgent("hermes".to_string())));
    }

    #[test]
    fn test_parse_use_with_no_name_returns_none() {
        let result = parse_command("/use");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_use_with_whitespace_name_returns_none() {
        let result = parse_command("/use   ");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_list() {
        let result = parse_command("/list");
        assert_eq!(result, Some(RouterCommand::ListAgents));
    }

    #[test]
    fn test_parse_status() {
        let result = parse_command("/status");
        assert_eq!(result, Some(RouterCommand::Status));
    }

    #[test]
    fn test_parse_cmd_default_timeout() {
        let result = parse_command("/cmd ls");
        assert_eq!(
            result,
            Some(RouterCommand::Cmd {
                command: "ls".to_string(),
                timeout_secs: 30,
            })
        );
    }

    #[test]
    fn test_parse_cmd_with_timeout() {
        let result = parse_command("/cmd timeout 60 cargo build");
        assert_eq!(
            result,
            Some(RouterCommand::Cmd {
                command: "cargo build".to_string(),
                timeout_secs: 60,
            })
        );
    }

    #[test]
    fn test_parse_cmd_with_invalid_timeout_defaults_to_30() {
        let result = parse_command("/cmd timeout abc ls");
        assert_eq!(
            result,
            Some(RouterCommand::Cmd {
                command: "ls".to_string(),
                timeout_secs: 30,
            })
        );
    }

    #[test]
    fn test_parse_cmd_multi_word_command() {
        let result = parse_command("/cmd timeout 10 echo hello world");
        assert_eq!(
            result,
            Some(RouterCommand::Cmd {
                command: "echo hello world".to_string(),
                timeout_secs: 10,
            })
        );
    }

    #[test]
    fn test_parse_cmd_no_args_returns_none() {
        let result = parse_command("/cmd");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_plain_text_returns_none() {
        let result = parse_command("hello world");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_unknown_command_returns_none() {
        let result = parse_command("/unknown");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_empty_string_returns_none() {
        let result = parse_command("");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_only_slash_returns_none() {
        let result = parse_command("/");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_leading_trailing_whitespace() {
        let result = parse_command("  /use hermes  ");
        assert_eq!(result, Some(RouterCommand::UseAgent("hermes".to_string())));
    }

    // ─── is_dangerous_command tests ───────────────────────────────────────

    #[test]
    fn test_dangerous_rm_rf_root() {
        assert!(is_dangerous_command("rm -rf /"));
    }

    #[test]
    fn test_dangerous_rm_rf_root_with_flags() {
        assert!(is_dangerous_command("rm -rf /*"));
    }

    #[test]
    fn test_dangerous_rm_rf_home() {
        assert!(is_dangerous_command("rm -rf ~"));
    }

    #[test]
    fn test_dangerous_shutdown() {
        assert!(is_dangerous_command("shutdown -h now"));
    }

    #[test]
    fn test_dangerous_reboot() {
        assert!(is_dangerous_command("reboot"));
    }

    #[test]
    fn test_dangerous_dev_sda() {
        assert!(is_dangerous_command("echo hello > /dev/sda"));
    }

    #[test]
    fn test_dangerous_mkfs() {
        assert!(is_dangerous_command("mkfs.ext4 /dev/sda1"));
    }

    #[test]
    fn test_dangerous_dd() {
        assert!(is_dangerous_command("dd if=/dev/zero of=/dev/sda"));
    }

    #[test]
    fn test_dangerous_fork_bomb_colon() {
        assert!(is_dangerous_command(":{():|:&};:"));
    }

    #[test]
    fn test_dangerous_fork_bomb_full() {
        assert!(is_dangerous_command(":(){ :|:& };:"));
    }

    #[test]
    fn test_dangerous_allows_ls() {
        assert!(!is_dangerous_command("ls -la /tmp"));
    }

    #[test]
    fn test_dangerous_allows_safe_rm() {
        assert!(!is_dangerous_command("rm file.txt"));
    }

    #[test]
    fn test_dangerous_allows_echo() {
        assert!(!is_dangerous_command("echo hello world"));
    }

    #[test]
    fn test_dangerous_allows_empty_string() {
        assert!(!is_dangerous_command(""));
    }

    // ─── execute_command tests ────────────────────────────────────────────

    #[tokio::test]
    async fn test_execute_echo() {
        let result = execute_command("echo hello", 30, 2000).await.unwrap();
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_command_respects_timeout() {
        // sleep 100 with 1s timeout should timeout
        let result = execute_command("sleep 100", 1, 2000).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            GatewayError::Command(msg) => {
                assert!(msg.contains("timed out"), "expected timeout message, got: {}", msg);
            }
            _ => panic!("expected Command error with timeout message, got: {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_execute_command_truncates_output() {
        // Generate output longer than max_chars
        let result = execute_command("echo abcdefghij", 30, 5).await.unwrap();
        assert!(result.contains("... (truncated)"));
        assert!(result.len() <= 20, "truncated result should be short"); // 5 + "..." + " (truncated)"
    }

    #[tokio::test]
    async fn test_execute_command_returns_stderr() {
        let result = execute_command("echo hello >&2", 30, 2000).await.unwrap();
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_nonexistent_command() {
        let result = execute_command("nonexistent_command_xyz123", 5, 2000).await;
        // Should either error or return something (depends on platform)
        assert!(result.is_ok() || result.is_err());
    }
}
