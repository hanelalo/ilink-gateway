package dedup

import (
	"testing"
	"time"
)

func TestCheckAndRecordNew(t *testing.T) {
	c := New(time.Minute)
	if !c.CheckAndRecord("a") {
		t.Error("first occurrence should return true")
	}
}

func TestCheckAndRecordDuplicate(t *testing.T) {
	c := New(time.Minute)
	c.CheckAndRecord("a")
	if c.CheckAndRecord("a") {
		t.Error("second occurrence should return false")
	}
}

func TestCheckAndRecordAfterTTL(t *testing.T) {
	c := New(20 * time.Millisecond)
	c.CheckAndRecord("a")
	time.Sleep(30 * time.Millisecond)
	if !c.CheckAndRecord("a") {
		t.Error("after TTL the key should be considered new again")
	}
}

func TestCheckAndRecordDifferentKeys(t *testing.T) {
	c := New(time.Minute)
	if !c.CheckAndRecord("a") {
		t.Error("a should be new")
	}
	if !c.CheckAndRecord("b") {
		t.Error("b should be new")
	}
	if c.CheckAndRecord("a") {
		t.Error("a should now be a duplicate")
	}
}

func TestDedupKeyVariesByText(t *testing.T) {
	k1 := DedupKey("msg1", "hello")
	k2 := DedupKey("msg1", "world")
	k3 := DedupKey("msg2", "hello")
	if k1 == k2 {
		t.Error("same msgID different text should differ")
	}
	if k1 == k3 {
		t.Error("different msgID same text should differ")
	}
}

func TestLazyEviction(t *testing.T) {
	c := New(20 * time.Millisecond)
	c.CheckAndRecord("old")
	time.Sleep(30 * time.Millisecond)
	// A new key should trigger lazy eviction of "old"; then "old" is new again.
	c.CheckAndRecord("new")
	if !c.CheckAndRecord("old") {
		t.Error("evicted key should be new again")
	}
}
