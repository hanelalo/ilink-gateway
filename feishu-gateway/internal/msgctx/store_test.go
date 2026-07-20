package msgctx

import (
	"testing"
	"time"
)

func TestPutAndGet(t *testing.T) {
	s := New()
	s.Put("m1", Info{ReceiveIDType: "open_id", ReceiveID: "ou_a", AgentName: "claude"})
	info, ok := s.Get("m1")
	if !ok {
		t.Fatal("missing")
	}
	if info.ReceiveID != "ou_a" || info.AgentName != "claude" {
		t.Errorf("unexpected info: %+v", info)
	}
}

func TestGetMissing(t *testing.T) {
	s := New()
	if _, ok := s.Get("nope"); ok {
		t.Error("expected miss")
	}
}

func TestCleanup(t *testing.T) {
	s := New()
	s.Put("old", Info{ReceiveIDType: "open_id", ReceiveID: "ou_a", ReceivedAt: time.Now().Add(-20 * time.Minute)})
	s.Put("new", Info{ReceiveIDType: "open_id", ReceiveID: "ou_b", ReceivedAt: time.Now()})

	n := s.Cleanup(10 * time.Minute)
	if n != 1 {
		t.Errorf("expected 1 evicted, got %d", n)
	}
	if _, ok := s.Get("old"); ok {
		t.Error("old should be evicted")
	}
	if _, ok := s.Get("new"); !ok {
		t.Error("new should remain")
	}
}
