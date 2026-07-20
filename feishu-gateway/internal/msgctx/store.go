package msgctx

import (
	"sync"
	"time"
)

// Info stores what the reply processor needs to route an agent reply back to
// the originating IM chat: the receive_id_type / receive_id to send to, plus
// which agent the original message was routed to.
type Info struct {
	ReceiveIDType string // "open_id" or "chat_id"
	ReceiveID     string
	AgentName     string
	ReceivedAt    time.Time
}

// Store maps message_id → Info. Written by the feishu WS handler when a
// message is enqueued, read by the reply processor when an agent replies.
type Store struct {
	mu sync.RWMutex
	m  map[string]Info
}

func New() *Store {
	return &Store{m: make(map[string]Info)}
}

func (s *Store) Put(msgID string, info Info) {
	s.mu.Lock()
	s.m[msgID] = info
	s.mu.Unlock()
}

func (s *Store) Get(msgID string) (Info, bool) {
	s.mu.RLock()
	info, ok := s.m[msgID]
	s.mu.RUnlock()
	return info, ok
}

// Cleanup drops entries older than maxAge. Returns the number evicted.
func (s *Store) Cleanup(maxAge time.Duration) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	cutoff := time.Now().Add(-maxAge)
	n := 0
	for k, v := range s.m {
		if v.ReceivedAt.Before(cutoff) {
			delete(s.m, k)
			n++
		}
	}
	return n
}
