package breaker

import (
	"sync"
	"time"
)

// Breaker is a sliding-window circuit breaker. After `threshold` failures
// within `window`, the circuit opens and stays open for `cooldown`. After
// cooldown it half-opens: the next call is allowed through; a failure
// re-opens, a success resets.
type Breaker struct {
	mu        sync.Mutex
	failures  []time.Time
	openedAt  time.Time
	window    time.Duration
	threshold int
	cooldown  time.Duration
}

func New(threshold int, window, cooldown time.Duration) *Breaker {
	return &Breaker{window: window, threshold: threshold, cooldown: cooldown}
}

// IsOpen reports whether calls should be rejected. After cooldown elapses the
// breaker half-opens (returns false) to allow one trial call.
func (b *Breaker) IsOpen() bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.openedAt.IsZero() {
		return false
	}
	if time.Since(b.openedAt) > b.cooldown {
		// Half-open: allow a trial.
		b.openedAt = time.Time{}
		b.failures = nil
		return false
	}
	return true
}

func (b *Breaker) RecordSuccess() {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.failures = nil
	b.openedAt = time.Time{}
}

func (b *Breaker) RecordFailure() {
	b.mu.Lock()
	defer b.mu.Unlock()
	now := time.Now()
	kept := b.failures[:0]
	for _, t := range b.failures {
		if now.Sub(t) <= b.window {
			kept = append(kept, t)
		}
	}
	b.failures = append(kept, now)
	if len(b.failures) >= b.threshold {
		b.openedAt = now
	}
}
