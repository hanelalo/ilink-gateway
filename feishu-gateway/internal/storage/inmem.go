package storage

import (
	"sync"
	"time"
)

const maxContextEntries = 200

// InMemoryStore is a process-local Store used for tests and until SQLite is
// wired in. Not persisted across restarts.
type InMemoryStore struct {
	mu       sync.Mutex
	state    map[string]string
	contexts map[string]contextEntry
}

type contextEntry struct {
	context   string
	createdAt int64
}

func NewInMemory() *InMemoryStore {
	return &InMemoryStore{
		state:    make(map[string]string),
		contexts: make(map[string]contextEntry),
	}
}

func (s *InMemoryStore) GetState(key string) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	v, ok := s.state[key]
	if !ok {
		return "", ErrNotFound
	}
	return v, nil
}

func (s *InMemoryStore) SetState(key, value string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.state[key] = value
	return nil
}

func (s *InMemoryStore) SaveMsgAgentContext(msgID, ctx string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.contexts[msgID] = contextEntry{context: ctx, createdAt: time.Now().UnixMilli()}
	if len(s.contexts) > maxContextEntries {
		// Evict oldest entry (LRU-ish).
		var oldestKey string
		var oldest int64
		for k, v := range s.contexts {
			if oldest == 0 || v.createdAt < oldest {
				oldest = v.createdAt
				oldestKey = k
			}
		}
		delete(s.contexts, oldestKey)
	}
	return nil
}

func (s *InMemoryStore) GetMsgAgentContext(msgID string) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	v, ok := s.contexts[msgID]
	if !ok {
		return "", ErrNotFound
	}
	return v.context, nil
}

func (s *InMemoryStore) Close() error { return nil }
