package agent

import (
	"testing"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

func msg(id string) model.QueuedMessage {
	return model.QueuedMessage{ID: id, FromUser: "u", Text: id, Media: []model.MediaItem{}}
}

func TestEnqueueAndDequeueAll(t *testing.T) {
	q := NewMessageQueue()
	q.Enqueue("a", msg("1"))
	q.Enqueue("a", msg("2"))
	q.Enqueue("a", msg("3"))

	if !q.HasPending("a") {
		t.Error("expected pending")
	}
	got := q.DequeueAll("a")
	if len(got) != 3 {
		t.Fatalf("expected 3, got %d", len(got))
	}
	// FIFO order
	for i, m := range got {
		want := string(rune('1' + i))
		if m.ID != want {
			t.Errorf("index %d: want %q, got %q", i, want, m.ID)
		}
	}
	if q.HasPending("a") {
		t.Error("queue should be empty after drain")
	}
}

func TestDequeueAllEmptyReturnsNonNil(t *testing.T) {
	q := NewMessageQueue()
	got := q.DequeueAll("nobody")
	if got == nil {
		t.Error("should return non-nil slice for empty queue")
	}
	if len(got) != 0 {
		t.Errorf("expected empty slice, got %d items", len(got))
	}
}

func TestDequeueAllDrainsQueue(t *testing.T) {
	q := NewMessageQueue()
	q.Enqueue("a", msg("1"))
	first := q.DequeueAll("a")
	second := q.DequeueAll("a")
	if len(first) != 1 || len(second) != 0 {
		t.Errorf("drain should clear queue: first=%d second=%d", len(first), len(second))
	}
}

func TestIsolationBetweenAgents(t *testing.T) {
	q := NewMessageQueue()
	q.Enqueue("a", msg("1"))
	q.Enqueue("b", msg("2"))
	if len(q.DequeueAll("a")) != 1 {
		t.Error("a should have 1")
	}
	if len(q.DequeueAll("b")) != 1 {
		t.Error("b should have 1")
	}
}

func TestUpdateLastMedia(t *testing.T) {
	q := NewMessageQueue()
	q.Enqueue("a", msg("1"))
	q.Enqueue("a", msg("2"))

	media := []model.MediaItem{{MediaType: "image", LocalPath: "/tmp/x.jpg"}}
	if !q.UpdateLastMedia("a", media) {
		t.Fatal("UpdateLastMedia returned false")
	}
	msgs := q.DequeueAll("a")
	if len(msgs[1].Media) != 1 || msgs[1].Media[0].LocalPath != "/tmp/x.jpg" {
		t.Errorf("last message media not updated: %+v", msgs[1].Media)
	}
	if len(msgs[0].Media) != 0 {
		t.Errorf("first message media should be untouched: %+v", msgs[0].Media)
	}
}

func TestUpdateLastMediaEmptyQueue(t *testing.T) {
	q := NewMessageQueue()
	if q.UpdateLastMedia("a", nil) {
		t.Error("expected false for empty queue")
	}
}
