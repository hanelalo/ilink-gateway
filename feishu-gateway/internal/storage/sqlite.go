package storage

import (
	"database/sql"
	"fmt"
	"os"
	"path/filepath"
	"time"

	_ "modernc.org/sqlite" // pure-Go driver, registers "sqlite"
)

// msgContextMax matches the Rust gateway's MSG_AGENT_CONTEXT_MAX (200). Keeps
// the table bounded; the oldest row is evicted when exceeded.
const msgContextMax = 200

// SQLiteStore persists gateway state and msg_agent_context across restarts
// using a single-file SQLite database. The driver is modernc.org/sqlite (pure
// Go, no CGO), so the gateway binary stays statically linkable.
type SQLiteStore struct {
	db *sql.DB
}

// NewSQLite opens (or creates) the database at path, initializes the schema,
// and returns a ready store. The parent directory is created if missing.
//
// SetMaxOpenConns(1) serializes writes — SQLite uses a process-wide write
// lock and would otherwise surface "database is locked" under concurrent
// upserts from the reply processor + HTTP handlers.
func NewSQLite(path string) (*SQLiteStore, error) {
	if path == "" {
		return nil, fmt.Errorf("sqlite: path is required")
	}

	if dir := filepath.Dir(path); dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return nil, fmt.Errorf("sqlite: create db dir %q: %w", dir, err)
		}
	}

	// Busy timeout lets concurrent writers wait briefly (5s) instead of
	// failing immediately with SQLITE_BUSY. With MaxOpenConns(1) this is
	// mostly belt-and-suspenders.
	dsn := fmt.Sprintf("file:%s?_pragma=busy_timeout(5000)&_pragma=journal_mode(WAL)&_pragma=foreign_keys(ON)", path)
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("sqlite: open %q: %w", path, err)
	}
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	db.SetConnMaxLifetime(0)

	if err := initSchema(db); err != nil {
		_ = db.Close()
		return nil, err
	}

	return &SQLiteStore{db: db}, nil
}

func initSchema(db *sql.DB) error {
	const schema = `
CREATE TABLE IF NOT EXISTS gateway_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS msg_agent_context (
    msg_id     TEXT PRIMARY KEY,
    context    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_msg_agent_context_created_at
    ON msg_agent_context(created_at);
`
	if _, err := db.Exec(schema); err != nil {
		return fmt.Errorf("sqlite: init schema: %w", err)
	}
	return nil
}

// GetState returns the value for key, or ErrNotFound if missing.
func (s *SQLiteStore) GetState(key string) (string, error) {
	var v string
	err := s.db.QueryRow(`SELECT value FROM gateway_state WHERE key = ?`, key).Scan(&v)
	if err == sql.ErrNoRows {
		return "", ErrNotFound
	}
	if err != nil {
		return "", fmt.Errorf("sqlite: get state %q: %w", key, err)
	}
	return v, nil
}

// SetState upserts a gateway state key/value.
func (s *SQLiteStore) SetState(key, value string) error {
	if _, err := s.db.Exec(
		`INSERT OR REPLACE INTO gateway_state (key, value) VALUES (?, ?)`,
		key, value,
	); err != nil {
		return fmt.Errorf("sqlite: set state %q: %w", key, err)
	}
	return nil
}

// SaveMsgAgentContext stores msg_id → context. When the table exceeds
// msgContextMax rows, the oldest entry (by created_at, then insertion order)
// is evicted — mirroring the Rust gateway's LRU semantics.
func (s *SQLiteStore) SaveMsgAgentContext(msgID, ctx string) error {
	tx, err := s.db.Begin()
	if err != nil {
		return fmt.Errorf("sqlite: begin tx: %w", err)
	}
	defer func() { _ = tx.Rollback() }() // no-op if committed

	if _, err := tx.Exec(
		`INSERT OR REPLACE INTO msg_agent_context (msg_id, context, created_at) VALUES (?, ?, ?)`,
		msgID, ctx, time.Now().UnixMilli(),
	); err != nil {
		return fmt.Errorf("sqlite: save msg_agent_context %q: %w", msgID, err)
	}

	var count int
	if err := tx.QueryRow(`SELECT COUNT(*) FROM msg_agent_context`).Scan(&count); err != nil {
		return fmt.Errorf("sqlite: count msg_agent_context: %w", err)
	}

	if count > msgContextMax {
		// Evict the single oldest row. Tiebreak by rowid for stable ordering.
		if _, err := tx.Exec(
			`DELETE FROM msg_agent_context
			 WHERE rowid = (
			     SELECT rowid FROM msg_agent_context
			     ORDER BY created_at ASC, rowid ASC
			     LIMIT 1
			 )`,
		); err != nil {
			return fmt.Errorf("sqlite: trim msg_agent_context: %w", err)
		}
	}

	if err := tx.Commit(); err != nil {
		return fmt.Errorf("sqlite: commit msg_agent_context: %w", err)
	}
	return nil
}

// GetMsgAgentContext returns the context saved for msgID, or ErrNotFound.
func (s *SQLiteStore) GetMsgAgentContext(msgID string) (string, error) {
	var ctx string
	err := s.db.QueryRow(
		`SELECT context FROM msg_agent_context WHERE msg_id = ?`,
		msgID,
	).Scan(&ctx)
	if err == sql.ErrNoRows {
		return "", ErrNotFound
	}
	if err != nil {
		return "", fmt.Errorf("sqlite: get msg_agent_context %q: %w", msgID, err)
	}
	return ctx, nil
}

// Close releases the database handle. Idempotent — safe to call multiple times.
func (s *SQLiteStore) Close() error {
	if s.db == nil {
		return nil
	}
	err := s.db.Close()
	s.db = nil
	return err
}
