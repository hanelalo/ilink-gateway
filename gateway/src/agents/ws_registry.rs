//! WebSocket connection registry.
//!
//! Tracks active WebSocket connections per agent name.
//! Each agent can have at most one WS connection; new one replaces old.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

/// Registry of active WebSocket senders per agent.
#[derive(Clone)]
pub struct WsRegistry {
    inner: Arc<Mutex<HashMap<String, UnboundedSender<String>>>>,
}

impl WsRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register (or replace) a WebSocket sender for an agent.
    pub fn register(&self, name: String, tx: UnboundedSender<String>) {
        self.inner.lock().unwrap().insert(name, tx);
    }

    /// Unregister a WebSocket sender when a connection drops.
    pub fn unregister(&self, name: &str) {
        self.inner.lock().unwrap().remove(name);
    }

    /// Push a JSON message to an agent's WebSocket, if connected.
    ///
    /// Returns `true` if the message was sent, `false` if no WS connection.
    pub fn push(&self, name: &str, json: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        if let Some(tx) = inner.get(name) {
            tx.send(json.to_string()).is_ok()
        } else {
            false
        }
    }

    /// Check if an agent has an active WebSocket connection.
    pub fn is_connected(&self, name: &str) -> bool {
        self.inner.lock().unwrap().contains_key(name)
    }

    /// Number of active WebSocket connections.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    /// Check whether the registry is empty (no active connections).
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }
}

impl Default for WsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_registry_is_empty() {
        let registry = WsRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(!registry.is_connected("hermes"));
    }

    #[test]
    fn test_register_adds_connection() {
        let registry = WsRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register("hermes".to_string(), tx);

        assert!(registry.is_connected("hermes"));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_multiple_agents_independent() {
        let registry = WsRegistry::new();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();

        registry.register("hermes".to_string(), tx1);
        registry.register("zeus".to_string(), tx2);

        assert_eq!(registry.len(), 2);
        assert!(registry.is_connected("hermes"));
        assert!(registry.is_connected("zeus"));
    }

    #[test]
    fn test_unregister_removes_connection() {
        let registry = WsRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register("hermes".to_string(), tx);

        assert!(registry.is_connected("hermes"));
        registry.unregister("hermes");
        assert!(!registry.is_connected("hermes"));
        assert!(registry.is_empty());
    }

    #[test]
    fn test_unregister_nonexistent_does_not_panic() {
        let registry = WsRegistry::new();
        registry.unregister("nobody");
        // No panic — pass
        assert!(registry.is_empty());
    }

    #[test]
    fn test_push_returns_false_when_not_connected() {
        let registry = WsRegistry::new();
        assert!(!registry.push("hermes", r#"{"type":"test"}"#));
    }

    #[test]
    fn test_push_returns_true_and_delivers_when_connected() {
        let registry = WsRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register("hermes".to_string(), tx);

        assert!(registry.push("hermes", r#"{"type":"test"}"#));

        let received = rx.try_recv().expect("should have received message");
        assert_eq!(received, r#"{"type":"test"}"#);
    }

    #[test]
    fn test_push_to_unknown_agent_does_not_affect_others() {
        let registry = WsRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register("hermes".to_string(), tx);

        assert!(!registry.push("zeus", "msg"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_replace_old_connection() {
        let registry = WsRegistry::new();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();

        // Register first connection
        registry.register("hermes".to_string(), tx1);
        assert_eq!(registry.len(), 1);

        // Second registration replaces the first
        registry.register("hermes".to_string(), tx2);
        assert_eq!(registry.len(), 1);

        // Push goes to the new connection (tx2)
        assert!(registry.push("hermes", "via-tx2"));
        let received = rx2.try_recv().expect("should receive on tx2");
        assert_eq!(received, "via-tx2");
    }

    #[test]
    fn test_push_returns_false_when_receiver_dropped() {
        let registry = WsRegistry::new();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register("hermes".to_string(), tx);

        // Drop the receiver — simulating a disconnected agent
        drop(rx);

        // Push should fail because the receiver is gone
        assert!(!registry.push("hermes", "will-not-arrive"));
    }

    #[test]
    fn test_len_and_is_empty() {
        let registry = WsRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register("a".to_string(), tx);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());

        let (tx2, _rx2) = tokio::sync::mpsc::unbounded_channel();
        registry.register("b".to_string(), tx2);
        assert_eq!(registry.len(), 2);

        registry.unregister("a");
        assert_eq!(registry.len(), 1);

        registry.unregister("b");
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_default_is_empty() {
        let registry = WsRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_clone_is_independent_handle() {
        let registry = WsRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        registry.register("hermes".to_string(), tx);

        let cloned = registry.clone();
        assert!(cloned.is_connected("hermes"));

        cloned.push("hermes", "from-clone");
        let msg = rx.try_recv().unwrap();
        assert_eq!(msg, "from-clone");
    }
}
