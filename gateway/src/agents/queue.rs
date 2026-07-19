//! Message queue — per-agent FIFO message queues.
//!
//! When a WeChat message arrives, it's queued for the active agent.
//! The agent polls and dequeues messages.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::ilink::types::{MediaItem, QueuedMessage};

/// A per-agent FIFO message queue, shareable via Clone (Arc<Mutex<...>>).
#[derive(Clone)]
pub struct MessageQueue {
    inner: Arc<Mutex<MessageQueueInner>>,
}

struct MessageQueueInner {
    /// Map of agent_name → Vec of queued messages (FIFO).
    queues: HashMap<String, Vec<QueuedMessage>>,
}

impl MessageQueue {
    /// Create a new empty message queue.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MessageQueueInner {
                queues: HashMap::new(),
            })),
        }
    }

    /// Push a message to an agent's queue.
    ///
    /// If the agent does not have a queue yet, one is created.
    pub fn enqueue(&self, agent: &str, msg: QueuedMessage) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.queues.entry(agent.to_string()).or_default().push(msg);
        Ok(())
    }

    /// Pop all pending messages for an agent (drain).
    ///
    /// Returns an empty vec if the agent has no queue or no pending messages.
    pub fn dequeue_all(&self, agent: &str) -> Result<Vec<QueuedMessage>> {
        let mut inner = self.inner.lock().unwrap();
        Ok(inner.queues.get_mut(agent).map_or_else(Vec::new, |q| std::mem::take(q)))
    }

    /// Peek at queue length for an agent.
    #[allow(dead_code)]
    pub fn len(&self, agent: &str) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.queues.get(agent).map_or(0, |q| q.len())
    }

    /// Check if the queue for a specific agent is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self, agent: &str) -> bool {
        self.len(agent) == 0
    }

    /// Check if an agent has any pending messages.
    #[allow(dead_code)]
    pub fn has_pending(&self, agent: &str) -> bool {
        self.len(agent) > 0
    }

    /// Get total pending messages across all agents.
    #[allow(dead_code)]
    pub fn total_pending(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.queues.values().map(|q| q.len()).sum()
    }

    /// Remove all messages for an agent.
    #[allow(dead_code)]
    pub fn clear(&self, agent: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(queue) = inner.queues.get_mut(agent) {
            queue.clear();
        }
        Ok(())
    }

    /// Update the media field of the last enqueued message for an agent (in-place).
    ///
    /// If the agent has no queue or the queue is empty, this is a no-op.
    pub fn update_last_media(&self, agent: &str, media: Vec<MediaItem>) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(queue) = inner.queues.get_mut(agent) {
            if let Some(last) = queue.last_mut() {
                last.media = media;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn make_msg(id: &str, text: &str) -> QueuedMessage {
    use std::time::{SystemTime, UNIX_EPOCH};
    QueuedMessage {
        id: id.to_string(),
        from_user: "user@wx".to_string(),
        text: text.to_string(),
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
        context_token: "ctx-123".to_string(),
        message_type: "text".to_string(),
        delivered: false,
        media: vec![],
        agent_context: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_and_dequeue() {
        let queue = MessageQueue::new();
        let msg = make_msg("msg-1", "hello");

        queue.enqueue("hermes", msg.clone()).unwrap();
        let msgs = queue.dequeue_all("hermes").unwrap();

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, "msg-1");
        assert_eq!(msgs[0].text, "hello");
    }

    #[test]
    fn test_dequeue_is_fifo() {
        let queue = MessageQueue::new();
        queue.enqueue("hermes", make_msg("msg-1", "first")).unwrap();
        queue.enqueue("hermes", make_msg("msg-2", "second")).unwrap();
        queue.enqueue("hermes", make_msg("msg-3", "third")).unwrap();

        let msgs = queue.dequeue_all("hermes").unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].id, "msg-1");
        assert_eq!(msgs[1].id, "msg-2");
        assert_eq!(msgs[2].id, "msg-3");
    }

    #[test]
    fn test_dequeue_all_drains_queue() {
        let queue = MessageQueue::new();
        queue.enqueue("hermes", make_msg("msg-1", "hello")).unwrap();
        assert_eq!(queue.len("hermes"), 1);

        let msgs = queue.dequeue_all("hermes").unwrap();
        assert_eq!(msgs.len(), 1);

        // After drain, len should be 0
        assert_eq!(queue.len("hermes"), 0);
    }

    #[test]
    fn test_has_pending() {
        let queue = MessageQueue::new();
        assert!(!queue.has_pending("hermes"));

        queue.enqueue("hermes", make_msg("msg-1", "hello")).unwrap();
        assert!(queue.has_pending("hermes"));

        queue.dequeue_all("hermes").unwrap();
        assert!(!queue.has_pending("hermes"));
    }

    #[test]
    fn test_total_pending() {
        let queue = MessageQueue::new();
        assert_eq!(queue.total_pending(), 0);

        queue.enqueue("hermes", make_msg("msg-1", "a")).unwrap();
        assert_eq!(queue.total_pending(), 1);

        queue.enqueue("zeus", make_msg("msg-2", "b")).unwrap();
        queue.enqueue("zeus", make_msg("msg-3", "c")).unwrap();
        assert_eq!(queue.total_pending(), 3);

        queue.dequeue_all("hermes").unwrap();
        assert_eq!(queue.total_pending(), 2);

        queue.dequeue_all("zeus").unwrap();
        assert_eq!(queue.total_pending(), 0);
    }

    #[test]
    fn test_clear_removes_messages() {
        let queue = MessageQueue::new();
        queue.enqueue("hermes", make_msg("msg-1", "hello")).unwrap();
        queue.enqueue("hermes", make_msg("msg-2", "world")).unwrap();
        assert_eq!(queue.len("hermes"), 2);

        queue.clear("hermes").unwrap();
        assert_eq!(queue.len("hermes"), 0);
        assert!(!queue.has_pending("hermes"));
    }

    #[test]
    fn test_different_agents_have_independent_queues() {
        let queue = MessageQueue::new();
        queue.enqueue("hermes", make_msg("msg-1", "hello from hermes")).unwrap();
        queue.enqueue("zeus", make_msg("msg-2", "hello from zeus")).unwrap();
        queue.enqueue("hermes", make_msg("msg-3", "another for hermes")).unwrap();

        assert_eq!(queue.len("hermes"), 2);
        assert_eq!(queue.len("zeus"), 1);

        let hermes_msgs = queue.dequeue_all("hermes").unwrap();
        assert_eq!(hermes_msgs.len(), 2);
        assert_eq!(hermes_msgs[0].text, "hello from hermes");
        assert_eq!(hermes_msgs[1].text, "another for hermes");

        // Zeus still has his message
        assert_eq!(queue.len("zeus"), 1);
        let zeus_msgs = queue.dequeue_all("zeus").unwrap();
        assert_eq!(zeus_msgs.len(), 1);
        assert_eq!(zeus_msgs[0].text, "hello from zeus");
    }

    #[test]
    fn test_enqueue_creates_queue_for_new_agent() {
        let queue = MessageQueue::new();
        assert_eq!(queue.len("new-agent"), 0);

        queue.enqueue("new-agent", make_msg("msg-1", "first!")).unwrap();
        assert_eq!(queue.len("new-agent"), 1);
        assert!(queue.has_pending("new-agent"));

        let msgs = queue.dequeue_all("new-agent").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, "msg-1");
    }

    #[test]
    fn test_dequeue_all_for_unknown_agent_returns_empty() {
        let queue = MessageQueue::new();
        let msgs = queue.dequeue_all("nonexistent").unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_len_for_unknown_agent_is_zero() {
        let queue = MessageQueue::new();
        assert_eq!(queue.len("nonexistent"), 0);
        assert!(queue.is_empty("nonexistent"));
    }

    #[test]
    fn test_clear_for_unknown_agent_does_not_error() {
        let queue = MessageQueue::new();
        // Should not panic or error
        queue.clear("nonexistent").unwrap();
    }

    #[test]
    fn test_update_last_media_updates_in_place() {
        let queue = MessageQueue::new();
        queue.enqueue("hermes", make_msg("msg-1", "hello")).unwrap();
        assert!(queue.has_pending("hermes"));

        let media = vec![MediaItem {
            media_type: "image".to_string(),
            local_path: "/tmp/cached/image.jpg".to_string(),
            original_name: Some("img.jpg".to_string()),
        }];
        queue.update_last_media("hermes", media.clone()).unwrap();

        // Peek by dequeueing
        let msgs = queue.dequeue_all("hermes").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].media.len(), 1);
        assert_eq!(msgs[0].media[0].media_type, "image");
        assert_eq!(msgs[0].media[0].local_path, "/tmp/cached/image.jpg");
        assert_eq!(msgs[0].media[0].original_name.as_deref(), Some("img.jpg"));
    }

    #[test]
    fn test_update_last_media_empty_queue_is_noop() {
        let queue = MessageQueue::new();
        let media = vec![MediaItem {
            media_type: "image".to_string(),
            local_path: "/tmp/img.jpg".to_string(),
            original_name: None,
        }];
        // Should not panic
        queue.update_last_media("hermes", media).unwrap();
        assert!(!queue.has_pending("hermes"));
    }

    #[test]
    fn test_update_last_media_updates_only_last_message() {
        let queue = MessageQueue::new();
        queue.enqueue("hermes", make_msg("msg-1", "first")).unwrap();
        queue.enqueue("hermes", make_msg("msg-2", "second")).unwrap();
        queue.enqueue("hermes", make_msg("msg-3", "third")).unwrap();

        let media = vec![MediaItem {
            media_type: "video".to_string(),
            local_path: "/tmp/video.mp4".to_string(),
            original_name: None,
        }];
        queue.update_last_media("hermes", media).unwrap();

        let msgs = queue.dequeue_all("hermes").unwrap();
        assert_eq!(msgs.len(), 3);
        // First two messages unchanged
        assert!(msgs[0].media.is_empty());
        assert!(msgs[1].media.is_empty());
        // Last message updated
        assert_eq!(msgs[2].media.len(), 1);
        assert_eq!(msgs[2].media[0].media_type, "video");
        assert_eq!(msgs[2].media[0].local_path, "/tmp/video.mp4");
    }
}
