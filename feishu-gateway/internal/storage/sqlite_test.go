package storage

import (
	"path/filepath"
	"testing"
)

func newTestSQLiteStore(t *testing.T) *SQLiteStore {
	t.Helper()
	dir := t.TempDir()
	path := filepath.Join(dir, "test.db")
	s, err := NewSQLite(path)
	if err != nil {
		t.Fatalf("NewSQLite: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func TestSQLiteStateRoundTrip(t *testing.T) {
	s := newTestSQLiteStore(t)

	if _, err := s.GetState("missing"); err != ErrNotFound {
		t.Fatalf("GetState(missing) = %v, want ErrNotFound", err)
	}

	if err := s.SetState("active_agent", "claude"); err != nil {
		t.Fatalf("SetState: %v", err)
	}

	got, err := s.GetState("active_agent")
	if err != nil {
		t.Fatalf("GetState: %v", err)
	}
	if got != "claude" {
		t.Fatalf("GetState = %q, want claude", got)
	}
}

func TestSQLiteStateUpsert(t *testing.T) {
	s := newTestSQLiteStore(t)

	if err := s.SetState("k", "v1"); err != nil {
		t.Fatal(err)
	}
	if err := s.SetState("k", "v2"); err != nil {
		t.Fatal(err)
	}

	got, err := s.GetState("k")
	if err != nil {
		t.Fatalf("GetState: %v", err)
	}
	if got != "v2" {
		t.Fatalf("after upsert: %q, want v2", got)
	}
}

func TestSQLiteMsgContextRoundTrip(t *testing.T) {
	s := newTestSQLiteStore(t)

	if _, err := s.GetMsgAgentContext("nope"); err != ErrNotFound {
		t.Fatalf("GetMsgAgentContext(missing) = %v, want ErrNotFound", err)
	}

	ctx := `{"agent":"claude","cwd":"/tmp"}`
	if err := s.SaveMsgAgentContext("msg-1", ctx); err != nil {
		t.Fatalf("Save: %v", err)
	}

	got, err := s.GetMsgAgentContext("msg-1")
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if got != ctx {
		t.Fatalf("Get = %q, want %q", got, ctx)
	}
}

func TestSQLiteMsgContextUpdateExisting(t *testing.T) {
	s := newTestSQLiteStore(t)

	if err := s.SaveMsgAgentContext("msg-1", `{"agent":"old"}`); err != nil {
		t.Fatal(err)
	}
	if err := s.SaveMsgAgentContext("msg-1", `{"agent":"new"}`); err != nil {
		t.Fatal(err)
	}

	got, err := s.GetMsgAgentContext("msg-1")
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if got != `{"agent":"new"}` {
		t.Fatalf("update: %q", got)
	}
}

func TestSQLiteMsgContextEvictionAtCap(t *testing.T) {
	s := newTestSQLiteStore(t)

	// Fill to the cap. msg-0..msg-199, oldest first by insertion time.
	for i := 0; i < msgContextMax; i++ {
		key := fmtKey(i)
		if err := s.SaveMsgAgentContext(key, "{}"); err != nil {
			t.Fatalf("Save %s: %v", key, err)
		}
	}

	// Insert one more → should evict the oldest (msg-0).
	if err := s.SaveMsgAgentContext("msg-new", "{}"); err != nil {
		t.Fatalf("Save overflow: %v", err)
	}

	if _, err := s.GetMsgAgentContext("msg-0000"); err != ErrNotFound {
		t.Fatalf("oldest not evicted: err=%v", err)
	}
	if _, err := s.GetMsgAgentContext("msg-new"); err != nil {
		t.Fatalf("new entry missing: %v", err)
	}

	// Ensure exactly msgContextMax rows remain.
	var count int
	if err := s.db.QueryRow(`SELECT COUNT(*) FROM msg_agent_context`).Scan(&count); err != nil {
		t.Fatalf("count: %v", err)
	}
	if count != msgContextMax {
		t.Fatalf("row count after eviction = %d, want %d", count, msgContextMax)
	}
}

func TestSQLiteMsgContextReopen(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "persist.db")

	// Write then close.
	{
		s, err := NewSQLite(path)
		if err != nil {
			t.Fatalf("NewSQLite: %v", err)
		}
		if err := s.SaveMsgAgentContext("msg-9", `{"x":1}`); err != nil {
			t.Fatalf("Save: %v", err)
		}
		if err := s.SetState("active_agent", "claude"); err != nil {
			t.Fatalf("SetState: %v", err)
		}
		if err := s.Close(); err != nil {
			t.Fatalf("Close: %v", err)
		}
	}

	// Reopen and verify persistence.
	s, err := NewSQLite(path)
	if err != nil {
		t.Fatalf("reopen NewSQLite: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })

	got, err := s.GetMsgAgentContext("msg-9")
	if err != nil {
		t.Fatalf("GetMsgAgentContext after reopen: %v", err)
	}
	if got != `{"x":1}` {
		t.Fatalf("ctx after reopen = %q", got)
	}

	active, err := s.GetState("active_agent")
	if err != nil {
		t.Fatalf("GetState after reopen: %v", err)
	}
	if active != "claude" {
		t.Fatalf("active_agent after reopen = %q", active)
	}
}

func TestSQLiteSchemaCreatedOnFirstOpen(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "first.db")

	s, err := NewSQLite(path)
	if err != nil {
		t.Fatalf("NewSQLite: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })

	for _, table := range []string{"gateway_state", "msg_agent_context"} {
		var name string
		err := s.db.QueryRow(
			`SELECT name FROM sqlite_master WHERE type='table' AND name=?`,
			table,
		).Scan(&name)
		if err != nil {
			t.Fatalf("table %s missing after init: %v", table, err)
		}
	}

	// Sanity: index also created.
	var idxName string
	err = s.db.QueryRow(
		`SELECT name FROM sqlite_master WHERE type='index' AND name='idx_msg_agent_context_created_at'`,
	).Scan(&idxName)
	if err != nil {
		t.Fatalf("expected index missing: %v", err)
	}
}

func TestSQLiteNewSQLiteCreatesParentDir(t *testing.T) {
	dir := t.TempDir()
	nested := filepath.Join(dir, "deep", "nested", "data.db")

	s, err := NewSQLite(nested)
	if err != nil {
		t.Fatalf("NewSQLite with nested dir: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })

	if err := s.SetState("k", "v"); err != nil {
		t.Fatalf("SetState: %v", err)
	}
}

func TestSQLiteNewSQLiteEmptyPathRejected(t *testing.T) {
	if _, err := NewSQLite(""); err == nil {
		t.Fatal("NewSQLite(\"\") should error")
	}
}

func TestSQLiteCloseIdempotent(t *testing.T) {
	s := newTestSQLiteStore(t)
	if err := s.Close(); err != nil {
		t.Fatalf("first Close: %v", err)
	}
	if err := s.Close(); err != nil {
		t.Fatalf("second Close: %v", err)
	}
}

// fmtKey formats an integer as msg-0000 style so lexical and chronological
// ordering match (keeps test output readable).
func fmtKey(i int) string {
	return formatPadded(i)
}

func formatPadded(i int) string {
	const width = 4
	out := make([]byte, width)
	for j := width - 1; j >= 0; j-- {
		out[j] = byte('0' + (i % 10))
		i /= 10
	}
	return "msg-" + string(out)
}
