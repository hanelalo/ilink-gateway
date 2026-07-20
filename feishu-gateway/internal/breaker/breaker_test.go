package breaker

import (
	"testing"
	"time"
)

func TestInitiallyClosed(t *testing.T) {
	b := New(1, 30*time.Second, 30*time.Second)
	if b.IsOpen() {
		t.Error("breaker should start closed")
	}
}

func TestOpensAtThreshold(t *testing.T) {
	b := New(3, 30*time.Second, 30*time.Second)
	b.RecordFailure()
	b.RecordFailure()
	if b.IsOpen() {
		t.Error("should still be closed at 2 failures (threshold 3)")
	}
	b.RecordFailure()
	if !b.IsOpen() {
		t.Error("should open at threshold")
	}
}

func TestSuccessResets(t *testing.T) {
	b := New(1, 30*time.Second, 30*time.Second)
	b.RecordFailure()
	if !b.IsOpen() {
		t.Error("should be open after 1 failure (threshold 1)")
	}
	b.RecordSuccess()
	if b.IsOpen() {
		t.Error("success should reset")
	}
}

func TestHalfOpensAfterCooldown(t *testing.T) {
	b := New(1, 30*time.Second, 20*time.Millisecond)
	b.RecordFailure()
	if !b.IsOpen() {
		t.Fatal("should be open")
	}
	time.Sleep(25 * time.Millisecond)
	if b.IsOpen() {
		t.Error("should half-open (allow trial) after cooldown")
	}
}

func TestFailuresOutsideWindowEvicted(t *testing.T) {
	b := New(2, 20*time.Millisecond, 30*time.Second)
	b.RecordFailure()
	time.Sleep(25 * time.Millisecond)
	b.RecordFailure()
	// First failure is outside the window, so only 1 counts — not open.
	if b.IsOpen() {
		t.Error("stale failure should not count toward threshold")
	}
	// One more recent failure hits threshold 2.
	b.RecordFailure()
	if !b.IsOpen() {
		t.Error("two recent failures should open")
	}
}
