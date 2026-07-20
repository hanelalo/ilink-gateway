package dedup

import (
	"crypto/md5"
	"fmt"
	"sync"
	"time"
)

// Cache is a TTL-based dedup store. CheckAndRecord returns true for a key not
// seen within the TTL window, false for a duplicate. Expired entries are
// evicted lazily on each access.
type Cache struct {
	mu   sync.Mutex
	seen map[string]time.Time
	ttl  time.Duration
}

func New(ttl time.Duration) *Cache {
	return &Cache{seen: make(map[string]time.Time), ttl: ttl}
}

func (c *Cache) CheckAndRecord(key string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	now := time.Now()
	for k, t := range c.seen {
		if now.Sub(t) > c.ttl {
			delete(c.seen, k)
		}
	}
	if _, ok := c.seen[key]; ok {
		return false
	}
	c.seen[key] = now
	return true
}

// DedupKey builds a dedup key from message ID and text content, matching the
// Rust gateway's (message_id, md5(content)) scheme.
func DedupKey(msgID, text string) string {
	h := md5.Sum([]byte(text))
	return fmt.Sprintf("%s:%x", msgID, h)
}
