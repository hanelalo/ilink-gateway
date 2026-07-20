package agent

import (
	"sync"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

// MessageQueue is a per-agent FIFO of QueuedMessages.
type MessageQueue struct {
	mu     sync.Mutex
	queues map[string][]model.QueuedMessage
}

func NewMessageQueue() *MessageQueue {
	return &MessageQueue{queues: make(map[string][]model.QueuedMessage)}
}

// Enqueue appends a message to the named agent's queue, creating it if needed.
func (q *MessageQueue) Enqueue(agentName string, msg model.QueuedMessage) {
	q.mu.Lock()
	defer q.mu.Unlock()
	q.queues[agentName] = append(q.queues[agentName], msg)
}

// DequeueAll drains and returns all pending messages for the agent.
// Returns a non-nil empty slice when the agent has no messages.
func (q *MessageQueue) DequeueAll(agentName string) []model.QueuedMessage {
	q.mu.Lock()
	defer q.mu.Unlock()
	msgs := q.queues[agentName]
	q.queues[agentName] = nil
	if msgs == nil {
		return []model.QueuedMessage{}
	}
	return msgs
}

func (q *MessageQueue) HasPending(agentName string) bool {
	q.mu.Lock()
	defer q.mu.Unlock()
	return len(q.queues[agentName]) > 0
}

// UpdateLastMedia overwrites the Media field on the most recently queued
// message for the agent. Used to backfill media after async download completes.
// Returns false if the agent has no queued messages.
func (q *MessageQueue) UpdateLastMedia(agentName string, media []model.MediaItem) bool {
	q.mu.Lock()
	defer q.mu.Unlock()
	msgs := q.queues[agentName]
	if len(msgs) == 0 {
		return false
	}
	msgs[len(msgs)-1].Media = media
	return true
}
