//! Agent registry — manages agent name → info mappings.
//!
//! Agents register themselves, and the registry tracks their online/offline
//! status via heartbeats.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{GatewayError, Result};
use crate::ilink::types::{AgentInfo, AgentStatus};

/// Manages agent name → info mappings.
pub struct AgentRegistry {
    agents: HashMap<String, AgentInfo>,
}

impl AgentRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Register a new agent or update an existing one.
    ///
    /// Sets the agent as online and updates `last_seen`.
    /// When updating an existing agent, `registered_at` is preserved.
    pub fn register(
        &mut self,
        name: &str,
        endpoint: Option<&str>,
        capabilities: &[String],
    ) -> Result<()> {
        if name.is_empty() {
            return Err(GatewayError::Config("Agent name cannot be empty".to_string()));
        }

        let now = now_millis();

        let info = AgentInfo {
            name: name.to_string(),
            endpoint: endpoint.map(|s| s.to_string()),
            capabilities: capabilities.to_vec(),
            status: AgentStatus::Online,
            last_seen: now,
            registered_at: now,
        };

        self.agents.insert(name.to_string(), info);
        Ok(())
    }

    /// Unregister an agent, removing it from the registry.
    pub fn unregister(&mut self, name: &str) -> Result<()> {
        if self.agents.remove(name).is_none() {
            return Err(GatewayError::AgentNotFound(name.to_string()));
        }
        Ok(())
    }

    /// Get agent info by name.
    pub fn get(&self, name: &str) -> Option<&AgentInfo> {
        self.agents.get(name)
    }

    /// List all registered agents.
    pub fn list(&self) -> Vec<&AgentInfo> {
        self.agents.values().collect()
    }

    /// Get the current number of registered agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Check if an agent is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// Mark an agent as online (heartbeat / registration).
    pub fn mark_online(&mut self, name: &str) -> Result<()> {
        let agent = self
            .agents
            .get_mut(name)
            .ok_or_else(|| GatewayError::AgentNotFound(name.to_string()))?;
        agent.status = AgentStatus::Online;
        agent.last_seen = now_millis();
        Ok(())
    }

    /// Mark an agent as offline (timeout / disconnect).
    pub fn mark_offline(&mut self, name: &str) -> Result<()> {
        let agent = self
            .agents
            .get_mut(name)
            .ok_or_else(|| GatewayError::AgentNotFound(name.to_string()))?;
        agent.status = AgentStatus::Offline;
        agent.last_seen = now_millis();
        Ok(())
    }

    /// Check all agents for heartbeat timeout.
    /// Marks agents as Offline if their last_seen is older than threshold_secs.
    /// Returns the list of agents that were marked offline (for logging).
    pub fn check_heartbeat(&mut self, threshold_secs: u64) -> Vec<String> {
        let cutoff = now_millis() - (threshold_secs as i64 * 1000);
        let mut offlined = Vec::new();

        for agent in self.agents.values_mut() {
            if agent.status == AgentStatus::Online && agent.last_seen < cutoff {
                agent.status = AgentStatus::Offline;
                offlined.push(agent.name.clone());
            }
        }

        offlined
    }

    /// Count online agents.
    pub fn online_count(&self) -> usize {
        self.agents
            .values()
            .filter(|a| a.status == AgentStatus::Online)
            .count()
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_new_agent() {
        let mut registry = AgentRegistry::new();
        let caps = vec!["text".to_string(), "image".to_string()];

        registry
            .register("hermes", Some("http://localhost:8081"), &caps)
            .unwrap();

        let agent = registry.get("hermes").unwrap();
        assert_eq!(agent.name, "hermes");
        assert_eq!(agent.endpoint.as_deref(), Some("http://localhost:8081"));
        assert_eq!(agent.capabilities, vec!["text", "image"]);
        assert_eq!(agent.status, AgentStatus::Online);
        assert!(agent.last_seen > 0);
        assert!(agent.registered_at > 0);
        assert_eq!(agent.last_seen, agent.registered_at);
    }

    #[test]
    fn test_register_updates_last_seen() {
        let mut registry = AgentRegistry::new();
        let caps = vec!["text".to_string()];

        registry.register("hermes", None, &caps).unwrap();
        let first_seen = registry.get("hermes").unwrap().last_seen;

        // Small delay to ensure timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(2));

        registry.register("hermes", Some("http://new:8081"), &caps).unwrap();
        let agent = registry.get("hermes").unwrap();
        assert_eq!(agent.endpoint.as_deref(), Some("http://new:8081"));
        assert!(agent.last_seen > first_seen, "last_seen should increase on re-register");
    }

    #[test]
    fn test_unregister_removes_agent() {
        let mut registry = AgentRegistry::new();
        registry.register("hermes", None, &[]).unwrap();
        assert!(registry.contains("hermes"));

        registry.unregister("hermes").unwrap();
        assert!(!registry.contains("hermes"));
        assert!(registry.get("hermes").is_none());
    }

    #[test]
    fn test_unregister_unknown_returns_error() {
        let mut registry = AgentRegistry::new();
        let result = registry.unregister("nonexistent");
        assert!(result.is_err());
        match result {
            Err(GatewayError::AgentNotFound(name)) => assert_eq!(name, "nonexistent"),
            _ => panic!("expected AgentNotFound error"),
        }
    }

    #[test]
    fn test_get_returns_none_for_unknown() {
        let registry = AgentRegistry::new();
        assert!(registry.get("hermes").is_none());
    }

    #[test]
    fn test_list_returns_all_agents() {
        let mut registry = AgentRegistry::new();
        registry.register("hermes", None, &[]).unwrap();
        registry.register("zeus", None, &[]).unwrap();
        registry.register("athena", None, &[]).unwrap();

        let agents = registry.list();
        assert_eq!(agents.len(), 3);

        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"hermes"));
        assert!(names.contains(&"zeus"));
        assert!(names.contains(&"athena"));
    }

    #[test]
    fn test_mark_online_and_offline() {
        let mut registry = AgentRegistry::new();
        let caps = vec!["text".to_string()];
        registry.register("hermes", None, &caps).unwrap();

        // Starts online
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Online);

        // Mark offline
        registry.mark_offline("hermes").unwrap();
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Offline);

        // Mark online again
        registry.mark_online("hermes").unwrap();
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Online);
    }

    #[test]
    fn test_mark_online_unknown_returns_error() {
        let mut registry = AgentRegistry::new();
        let result = registry.mark_online("nonexistent");
        assert!(result.is_err());
        match result {
            Err(GatewayError::AgentNotFound(name)) => assert_eq!(name, "nonexistent"),
            _ => panic!("expected AgentNotFound error"),
        }
    }

    #[test]
    fn test_online_count() {
        let mut registry = AgentRegistry::new();
        assert_eq!(registry.online_count(), 0);

        registry.register("hermes", None, &[]).unwrap();
        assert_eq!(registry.online_count(), 1);

        registry.register("zeus", None, &[]).unwrap();
        assert_eq!(registry.online_count(), 2);

        registry.mark_offline("hermes").unwrap();
        assert_eq!(registry.online_count(), 1);

        registry.mark_offline("zeus").unwrap();
        assert_eq!(registry.online_count(), 0);
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut registry = AgentRegistry::new();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());

        registry.register("hermes", None, &[]).unwrap();
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_check_heartbeat_recent_stays_online() {
        let mut registry = AgentRegistry::new();
        registry.register("hermes", None, &[]).unwrap();

        // Recently registered — last_seen is now, so within any reasonable threshold
        let offlined = registry.check_heartbeat(60);
        assert!(offlined.is_empty());
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Online);
    }

    #[test]
    fn test_check_heartbeat_old_is_marked_offline() {
        let mut registry = AgentRegistry::new();
        registry.register("hermes", None, &[]).unwrap();

        // Simulate old last_seen by directly manipulating the inner map
        let old_time = now_millis() - 100_000; // 100 seconds ago
        if let Some(agent) = registry.agents.get_mut("hermes") {
            agent.last_seen = old_time;
        }

        let offlined = registry.check_heartbeat(10);
        assert_eq!(offlined, vec!["hermes"]);
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Offline);
    }

    #[test]
    fn test_check_heartbeat_already_offline_stays_offline() {
        let mut registry = AgentRegistry::new();
        registry.register("hermes", None, &[]).unwrap();
        registry.mark_offline("hermes").unwrap();

        // Already offline — should not be re-listed
        let offlined = registry.check_heartbeat(1);
        assert!(offlined.is_empty());
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Offline);
    }

    #[test]
    fn test_check_heartbeat_multiple_agents_only_old_affected() {
        let mut registry = AgentRegistry::new();
        registry.register("hermes", None, &[]).unwrap(); // stays recent
        registry.register("zeus", None, &[]).unwrap();

        // Make zeus old
        let old_time = now_millis() - 100_000;
        if let Some(agent) = registry.agents.get_mut("zeus") {
            agent.last_seen = old_time;
        }

        let mut offlined = registry.check_heartbeat(10);
        offlined.sort();
        assert_eq!(offlined, vec!["zeus"]);
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Online);
        assert_eq!(registry.get("zeus").unwrap().status, AgentStatus::Offline);
    }

    #[test]
    fn test_check_heartbeat_empty_registry() {
        let mut registry = AgentRegistry::new();
        let offlined = registry.check_heartbeat(60);
        assert!(offlined.is_empty());
    }

    #[test]
    fn test_check_heartbeat_boundary_just_within_threshold() {
        let mut registry = AgentRegistry::new();
        registry.register("hermes", None, &[]).unwrap();

        // Set last_seen to exactly threshold_secs ago (should stay online)
        let threshold = 5u64;
        let boundary_time = now_millis() - (threshold as i64 * 1000);
        if let Some(agent) = registry.agents.get_mut("hermes") {
            agent.last_seen = boundary_time;
        }

        let offlined = registry.check_heartbeat(threshold);
        assert!(offlined.is_empty(), "exactly at boundary should stay online");
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Online);
    }

    #[test]
    fn test_check_heartbeat_just_past_threshold() {
        let mut registry = AgentRegistry::new();
        registry.register("hermes", None, &[]).unwrap();

        // Set last_seen to threshold_secs + 1 ago (should go offline)
        let too_old = now_millis() - (11 * 1000);
        if let Some(agent) = registry.agents.get_mut("hermes") {
            agent.last_seen = too_old;
        }

        let offlined = registry.check_heartbeat(10);
        assert_eq!(offlined, vec!["hermes"]);
        assert_eq!(registry.get("hermes").unwrap().status, AgentStatus::Offline);
    }

    #[test]
    fn test_register_empty_name_returns_error() {
        let mut registry = AgentRegistry::new();
        let result = registry.register("", None, &[]);
        assert!(result.is_err());
    }
}
