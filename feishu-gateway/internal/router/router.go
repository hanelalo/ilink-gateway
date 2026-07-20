package router

import (
	"errors"
	"fmt"
	"strings"
	"sync"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/agent"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/config"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

// ErrNoActiveAgent is returned when a message arrives but no agent is active.
var ErrNoActiveAgent = errors.New("no active agent")

const helpText = `Gateway commands:
  /use <name>              Switch active agent
  /list                    List registered agents
  /status                  Show gateway status
  /cmd [timeout N] <shell> Run shell command
  /gateway-help            Show this help`

// IncomingResult is what HandleIncoming produces for one inbound message.
type IncomingResult struct {
	SyncReply     string               // non-empty → reply immediately to user
	PendingShell  *model.RouterCommand // non-empty → caller runs /cmd async
	EnqueuedAgent string               // non-empty → message was queued to this agent
	MediaKeys     []model.MediaKey     // media keys belonging to the queued message
}

// State holds the active agent pointer and admission policies. Registry and
// Queue are owned externally (they are concurrency-safe on their own); State's
// own mutex only guards activeAgent and the policy snapshot. All operations
// use short, non-nested critical sections to avoid lock-ordering deadlocks.
type State struct {
	mu            sync.Mutex
	registry      *agent.Registry
	queue         *agent.MessageQueue
	activeAgent   string
	dmPolicy      config.DmPolicy
	groupPolicy   config.GroupPolicy
	allowedUsers  map[string]struct{}
	allowedGroups map[string]struct{}
	cmdTimeout    int
	cmdMaxOutput  int
}

func NewState(cfg config.Config, reg *agent.Registry, q *agent.MessageQueue) *State {
	return &State{
		registry:      reg,
		queue:         q,
		dmPolicy:      cfg.DmPolicy,
		groupPolicy:   cfg.GroupPolicy,
		allowedUsers:  cfg.AllowedUsers,
		allowedGroups: cfg.AllowedGroups,
		cmdTimeout:    cfg.CmdTimeoutSecs,
		cmdMaxOutput:  cfg.CmdMaxOutputChars,
	}
}

func (s *State) GetActiveAgent() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.activeAgent
}

// RestoreActiveAgent sets the active agent without verifying registry membership.
// Used at startup to restore persisted state before agents re-register.
func (s *State) RestoreActiveAgent(name string) {
	s.mu.Lock()
	s.activeAgent = name
	s.mu.Unlock()
}

func (s *State) SetActiveAgent(name string) error {
	if !s.registry.Contains(name) {
		return agent.ErrAgentNotFound
	}
	s.mu.Lock()
	s.activeAgent = name
	s.mu.Unlock()
	return nil
}

func (s *State) ListAgents() []model.AgentInfo {
	return s.registry.List()
}

// Register creates/updates an agent and auto-activates it when no agent is
// currently active. Returns the resulting active agent name.
func (s *State) Register(name, endpoint string, caps []string) (string, error) {
	if err := s.registry.Register(name, endpoint, caps); err != nil {
		return "", err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.activeAgent == "" {
		s.activeAgent = name
	}
	return s.activeAgent, nil
}

// MarkOnline refreshes the agent's heartbeat.
func (s *State) MarkOnline(name string) error {
	return s.registry.MarkOnline(name)
}

// Poll marks the agent online and drains its queue.
func (s *State) Poll(name string) ([]model.QueuedMessage, error) {
	if !s.registry.Contains(name) {
		return nil, agent.ErrAgentNotFound
	}
	if err := s.registry.MarkOnline(name); err != nil {
		return nil, err
	}
	return s.queue.DequeueAll(name), nil
}

func (s *State) Enqueue(agentName string, msg model.QueuedMessage) {
	s.queue.Enqueue(agentName, msg)
}

func (s *State) UpdateLastMedia(agentName string, media []model.MediaItem) bool {
	return s.queue.UpdateLastMedia(agentName, media)
}

func (s *State) CheckHeartbeat(timeoutSecs int) []string {
	return s.registry.CheckHeartbeat(timeoutSecs)
}

// HandleIncoming applies admission, command dispatch, and enqueue logic.
// Returns IncomingResult. A zero result with nil error means the message was
// silently dropped (admission denied).
func (s *State) HandleIncoming(msg model.IncomingMessage) (IncomingResult, error) {
	// 1. Admission
	if msg.IsGroup {
		if !s.groupPolicy.Allow(msg.ChatID, s.allowedGroups) {
			return IncomingResult{}, nil
		}
	} else {
		if !s.dmPolicy.Allow(msg.FromUser, s.allowedUsers) {
			return IncomingResult{}, nil
		}
	}

	// 2. Command?
	if strings.HasPrefix(msg.Text, "/") {
		cmd, ok := ParseCommand(msg.Text, s.cmdTimeout)
		if ok {
			return s.handleCommand(cmd)
		}
		// Unrecognized /xxx → falls through to agent as a normal message.
	}

	// 3. Enqueue to active agent
	s.mu.Lock()
	active := s.activeAgent
	s.mu.Unlock()
	if active == "" {
		return IncomingResult{}, ErrNoActiveAgent
	}
	s.queue.Enqueue(active, model.QueuedMessage{
		ID:           msg.MessageID,
		FromUser:     msg.FromUser,
		Text:         msg.Text,
		Timestamp:    msg.Timestamp,
		ContextToken: msg.MessageID,
		MessageType:  msg.MessageType,
		Media:        []model.MediaItem{},
	})
	return IncomingResult{
		EnqueuedAgent: active,
		MediaKeys:     msg.MediaKeys,
	}, nil
}

func (s *State) handleCommand(cmd model.RouterCommand) (IncomingResult, error) {
	switch cmd.Kind {
	case model.CmdUseAgent:
		if !s.registry.Contains(cmd.AgentName) {
			return IncomingResult{SyncReply: fmt.Sprintf("Agent '%s' not found", cmd.AgentName)}, nil
		}
		s.mu.Lock()
		s.activeAgent = cmd.AgentName
		s.mu.Unlock()
		return IncomingResult{SyncReply: fmt.Sprintf("Switched to agent '%s'", cmd.AgentName)}, nil
	case model.CmdListAgents:
		return IncomingResult{SyncReply: s.formatList()}, nil
	case model.CmdStatus:
		return IncomingResult{SyncReply: s.formatStatus()}, nil
	case model.CmdHelp:
		return IncomingResult{SyncReply: helpText}, nil
	case model.CmdShell:
		if IsDangerousCommand(cmd.Shell) {
			return IncomingResult{SyncReply: "⚠️ Command blocked for safety."}, nil
		}
		c := cmd
		return IncomingResult{PendingShell: &c}, nil
	}
	return IncomingResult{}, fmt.Errorf("unknown command kind: %d", cmd.Kind)
}

func (s *State) formatList() string {
	agents := s.registry.List()
	var b strings.Builder
	fmt.Fprintf(&b, "Registered agents (%d):", len(agents))
	for _, a := range agents {
		fmt.Fprintf(&b, "\n  • %s [%s]", a.Name, a.Status)
	}
	return b.String()
}

func (s *State) formatStatus() string {
	s.mu.Lock()
	active := s.activeAgent
	s.mu.Unlock()
	agents := s.registry.List()
	online := 0
	for _, a := range agents {
		if a.Status == model.StatusOnline {
			online++
		}
	}
	if active == "" {
		active = "(none)"
	}
	return fmt.Sprintf("Active agent: %s\nRegistered: %d agents (%d online)", active, len(agents), online)
}
