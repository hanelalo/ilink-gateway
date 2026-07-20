package storage

import "errors"

// ErrNotFound is returned by Get* methods when the key does not exist.
var ErrNotFound = errors.New("key not found")

// Store persists gateway state across restarts. The HTTP layer depends on this
// interface (not a concrete implementation) so the storage backend can be
// swapped — in-memory for tests/first-cut, SQLite once Go is upgraded.
type Store interface {
	GetState(key string) (string, error)
	SetState(key, value string) error
	SaveMsgAgentContext(msgID, ctx string) error
	GetMsgAgentContext(msgID string) (string, error)
	Close() error
}
