package agent

import (
	"errors"
	"testing"
	"time"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

func TestRegisterNewAgent(t *testing.T) {
	r := NewRegistry()
	caps := []string{"text", "image"}
	if err := r.Register("hermes", "http://localhost:8081", caps); err != nil {
		t.Fatalf("register: %v", err)
	}
	a, ok := r.Get("hermes")
	if !ok {
		t.Fatal("agent not found after register")
	}
	if a.Name != "hermes" || a.Endpoint != "http://localhost:8081" {
		t.Errorf("unexpected agent: %+v", a)
	}
	if len(a.Capabilities) != 2 || a.Capabilities[0] != "text" {
		t.Errorf("capabilities mismatch: %v", a.Capabilities)
	}
	if a.Status != model.StatusOnline {
		t.Errorf("expected online, got %v", a.Status)
	}
	if a.LastSeen <= 0 || a.RegisteredAt <= 0 {
		t.Errorf("timestamps not set: last=%d reg=%d", a.LastSeen, a.RegisteredAt)
	}
}

func TestRegisterEmptyName(t *testing.T) {
	r := NewRegistry()
	if err := r.Register("", "", nil); !errors.Is(err, ErrAgentNameEmpty) {
		t.Errorf("expected ErrAgentNameEmpty, got %v", err)
	}
}

// TestRegisterPreservesRegisteredAt verifies the upsert fix: re-registering
// must NOT change RegisteredAt (the Rust gateway has a bug where it does).
func TestRegisterPreservesRegisteredAt(t *testing.T) {
	r := NewRegistry()
	if err := r.Register("a", "", nil); err != nil {
		t.Fatal(err)
	}
	info1, _ := r.Get("a")
	firstRegistered := info1.RegisteredAt

	time.Sleep(5 * time.Millisecond)

	if err := r.Register("a", "http://new", []string{"text"}); err != nil {
		t.Fatal(err)
	}
	info2, _ := r.Get("a")

	if info2.RegisteredAt != firstRegistered {
		t.Errorf("RegisteredAt changed on re-register: %d -> %d", firstRegistered, info2.RegisteredAt)
	}
	if info2.LastSeen <= info1.LastSeen {
		t.Errorf("LastSeen should increase on re-register: %d -> %d", info1.LastSeen, info2.LastSeen)
	}
	if info2.Endpoint != "http://new" {
		t.Errorf("Endpoint not updated: %q", info2.Endpoint)
	}
	if info2.Status != model.StatusOnline {
		t.Errorf("re-registered agent should be online")
	}
}

func TestMarkOnlineOffline(t *testing.T) {
	r := NewRegistry()
	r.Register("a", "", nil)

	if err := r.MarkOffline("a"); err != nil {
		t.Fatal(err)
	}
	a, _ := r.Get("a")
	if a.Status != model.StatusOffline {
		t.Errorf("expected offline")
	}

	if err := r.MarkOnline("a"); err != nil {
		t.Fatal(err)
	}
	a, _ = r.Get("a")
	if a.Status != model.StatusOnline {
		t.Errorf("expected online")
	}
}

func TestMarkOnlineUnknown(t *testing.T) {
	r := NewRegistry()
	if err := r.MarkOnline("nope"); !errors.Is(err, ErrAgentNotFound) {
		t.Errorf("expected ErrAgentNotFound, got %v", err)
	}
}

func TestCheckHeartbeatRecentStaysOnline(t *testing.T) {
	r := NewRegistry()
	r.Register("a", "", nil)
	offlined := r.CheckHeartbeat(60)
	if len(offlined) != 0 {
		t.Errorf("expected no offlines, got %v", offlined)
	}
	a, _ := r.Get("a")
	if a.Status != model.StatusOnline {
		t.Errorf("should stay online")
	}
}

func TestCheckHeartbeatOldMarkedOffline(t *testing.T) {
	r := NewRegistry()
	r.Register("a", "", nil)
	// Simulate elapsed time by pushing LastSeen into the past.
	r.agents["a"].LastSeen = nowMillis() - 100_000 // 100s ago

	offlined := r.CheckHeartbeat(10)
	if len(offlined) != 1 || offlined[0] != "a" {
		t.Errorf("expected [a], got %v", offlined)
	}
	a, _ := r.Get("a")
	if a.Status != model.StatusOffline {
		t.Errorf("should be offline")
	}
}

func TestCheckHeartbeatAlreadyOfflineNotRelisted(t *testing.T) {
	r := NewRegistry()
	r.Register("a", "", nil)
	r.MarkOffline("a")
	r.agents["a"].LastSeen = nowMillis() - 100_000

	offlined := r.CheckHeartbeat(1)
	if len(offlined) != 0 {
		t.Errorf("already-offline agent should not be re-listed, got %v", offlined)
	}
}

func TestCheckHeartbeatMultipleAgents(t *testing.T) {
	r := NewRegistry()
	r.Register("a", "", nil)
	r.Register("b", "", nil)
	r.agents["b"].LastSeen = nowMillis() - 100_000

	offlined := r.CheckHeartbeat(10)
	if len(offlined) != 1 || offlined[0] != "b" {
		t.Errorf("expected [b], got %v", offlined)
	}
}

func TestCheckHeartbeatBoundaryWithinThreshold(t *testing.T) {
	r := NewRegistry()
	r.Register("a", "", nil)
	threshold := 5
	r.agents["a"].LastSeen = nowMillis() - int64(threshold)*1000 // exactly at boundary

	offlined := r.CheckHeartbeat(threshold)
	if len(offlined) != 0 {
		t.Errorf("at-boundary should stay online, got %v", offlined)
	}
}

func TestCheckHeartbeatJustPastThreshold(t *testing.T) {
	r := NewRegistry()
	r.Register("a", "", nil)
	r.agents["a"].LastSeen = nowMillis() - 11_000 // 11s ago, threshold 10

	offlined := r.CheckHeartbeat(10)
	if len(offlined) != 1 || offlined[0] != "a" {
		t.Errorf("just-past should be offline, got %v", offlined)
	}
}

func TestListAndLen(t *testing.T) {
	r := NewRegistry()
	if r.Len() != 0 {
		t.Errorf("expected empty")
	}
	r.Register("a", "", nil)
	r.Register("b", "", nil)
	if r.Len() != 2 {
		t.Errorf("expected 2, got %d", r.Len())
	}
	if !r.Contains("a") || !r.Contains("b") || r.Contains("c") {
		t.Errorf("Contains wrong")
	}
	if len(r.List()) != 2 {
		t.Errorf("List length wrong")
	}
}

func TestOnlineCount(t *testing.T) {
	r := NewRegistry()
	r.Register("a", "", nil)
	r.Register("b", "", nil)
	if r.OnlineCount() != 2 {
		t.Errorf("expected 2 online, got %d", r.OnlineCount())
	}
	r.MarkOffline("a")
	if r.OnlineCount() != 1 {
		t.Errorf("expected 1 online, got %d", r.OnlineCount())
	}
}
