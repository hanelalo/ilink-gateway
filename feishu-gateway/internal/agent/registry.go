package agent

import (
	"errors"
	"sync"
	"time"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

// Sentinel errors. Handlers match on these to pick HTTP status codes.
var (
	ErrAgentNameEmpty = errors.New("agent name cannot be empty")
	ErrAgentNotFound  = errors.New("agent not found")
)

// Registry tracks agent name → info, with online/offline driven by heartbeats.
type Registry struct {
	mu     sync.RWMutex
	agents map[string]*model.AgentInfo
}

func NewRegistry() *Registry {
	return &Registry{agents: make(map[string]*model.AgentInfo)}
}

func nowMillis() int64 {
	return time.Now().UnixMilli()
}

// Register creates or updates an agent. On update, RegisteredAt is preserved
// (the Rust gateway's docstring claims this but its code overwrites
// RegisteredAt every call — a bug we do not replicate).
func (r *Registry) Register(name, endpoint string, capabilities []string) error {
	if name == "" {
		return ErrAgentNameEmpty
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	now := nowMillis()
	if existing, ok := r.agents[name]; ok {
		existing.Endpoint = endpoint
		existing.Capabilities = capabilities
		existing.Status = model.StatusOnline
		existing.LastSeen = now
		return nil
	}
	r.agents[name] = &model.AgentInfo{
		Name:         name,
		Endpoint:     endpoint,
		Capabilities: capabilities,
		Status:       model.StatusOnline,
		LastSeen:     now,
		RegisteredAt: now,
	}
	return nil
}

func (r *Registry) MarkOnline(name string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	a, ok := r.agents[name]
	if !ok {
		return ErrAgentNotFound
	}
	a.Status = model.StatusOnline
	a.LastSeen = nowMillis()
	return nil
}

func (r *Registry) MarkOffline(name string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	a, ok := r.agents[name]
	if !ok {
		return ErrAgentNotFound
	}
	a.Status = model.StatusOffline
	return nil
}

// CheckHeartbeat transitions online agents whose last_seen is older than
// timeoutSecs to offline. Returns the names transitioned in this pass.
// Agents already offline are not re-listed.
func (r *Registry) CheckHeartbeat(timeoutSecs int) []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	cutoff := nowMillis() - int64(timeoutSecs)*1000
	var offlined []string
	for name, a := range r.agents {
		if a.Status == model.StatusOnline && a.LastSeen < cutoff {
			a.Status = model.StatusOffline
			offlined = append(offlined, name)
		}
	}
	return offlined
}

func (r *Registry) Get(name string) (model.AgentInfo, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	a, ok := r.agents[name]
	if !ok {
		return model.AgentInfo{}, false
	}
	return *a, true
}

func (r *Registry) Contains(name string) bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	_, ok := r.agents[name]
	return ok
}

func (r *Registry) List() []model.AgentInfo {
	r.mu.RLock()
	defer r.mu.RUnlock()
	out := make([]model.AgentInfo, 0, len(r.agents))
	for _, a := range r.agents {
		out = append(out, *a)
	}
	return out
}

func (r *Registry) Len() int {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return len(r.agents)
}

func (r *Registry) OnlineCount() int {
	r.mu.RLock()
	defer r.mu.RUnlock()
	n := 0
	for _, a := range r.agents {
		if a.Status == model.StatusOnline {
			n++
		}
	}
	return n
}
