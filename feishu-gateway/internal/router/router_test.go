package router

import (
	"errors"
	"strings"
	"testing"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/agent"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/config"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

func defaultCfg() config.Config {
	return config.Config{
		DmPolicy:         config.DMOpen,
		GroupPolicy:      config.GroupDisabled,
		CmdTimeoutSecs:   30,
		CmdMaxOutputChars: 2000,
		AllowedUsers:     map[string]struct{}{},
		AllowedGroups:    map[string]struct{}{},
	}
}

func setup(cfg config.Config) (*State, *agent.Registry, *agent.MessageQueue) {
	reg := agent.NewRegistry()
	q := agent.NewMessageQueue()
	return NewState(cfg, reg, q), reg, q
}

func dmIncoming(text string) model.IncomingMessage {
	return model.IncomingMessage{
		MessageID:     "m1",
		FromUser:      "ou_sender",
		Text:          text,
		MessageType:   "text",
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_sender",
		Timestamp:     1234567890000,
	}
}

func groupIncoming(text, chatID string) model.IncomingMessage {
	m := dmIncoming(text)
	m.IsGroup = true
	m.ChatID = chatID
	m.ReceiveIDType = "chat_id"
	m.ReceiveID = chatID
	return m
}

func registerAgent(t *testing.T, s *State, name string) {
	t.Helper()
	if _, err := s.Register(name, "", []string{"text"}); err != nil {
		t.Fatal(err)
	}
}

func TestHandleIncomingEnqueuesToActive(t *testing.T) {
	s, _, q := setup(defaultCfg())
	registerAgent(t, s, "hermes")

	res, err := s.HandleIncoming(dmIncoming("hello"))
	if err != nil {
		t.Fatal(err)
	}
	if res.EnqueuedAgent != "hermes" {
		t.Errorf("expected enqueue to hermes, got %q", res.EnqueuedAgent)
	}
	if !q.HasPending("hermes") {
		t.Error("expected pending message")
	}
}

func TestHandleIncomingNoActiveAgent(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	_, err := s.HandleIncoming(dmIncoming("hello"))
	if !errors.Is(err, ErrNoActiveAgent) {
		t.Errorf("expected ErrNoActiveAgent, got %v", err)
	}
}

func TestHandleIncomingDmDisabled(t *testing.T) {
	cfg := defaultCfg()
	cfg.DmPolicy = config.DMDisabled
	s, _, q := setup(cfg)
	registerAgent(t, s, "hermes")

	res, err := s.HandleIncoming(dmIncoming("hello"))
	if err != nil {
		t.Fatal(err)
	}
	if res.EnqueuedAgent != "" {
		t.Error("DM should be dropped by disabled policy")
	}
	if q.HasPending("hermes") {
		t.Error("no message should be queued")
	}
}

func TestHandleIncomingDmAllowlist(t *testing.T) {
	cfg := defaultCfg()
	cfg.DmPolicy = config.DMAllowlist
	cfg.AllowedUsers = map[string]struct{}{"ou_vip": {}}
	s, _, q := setup(cfg)
	registerAgent(t, s, "hermes")

	// Non-allowlisted → dropped
	res, _ := s.HandleIncoming(dmIncoming("hello"))
	if res.EnqueuedAgent != "" {
		t.Error("non-allowlisted DM should be dropped")
	}

	// Allowlisted → enqueued
	m := dmIncoming("hi")
	m.FromUser = "ou_vip"
	res, _ = s.HandleIncoming(m)
	if res.EnqueuedAgent != "hermes" {
		t.Error("allowlisted DM should be enqueued")
	}
	if !q.HasPending("hermes") {
		t.Error("expected pending")
	}
}

func TestHandleIncomingGroupDisabled(t *testing.T) {
	s, _, q := setup(defaultCfg()) // GroupPolicy defaults to Disabled
	registerAgent(t, s, "hermes")

	res, _ := s.HandleIncoming(groupIncoming("hello", "oc_group1"))
	if res.EnqueuedAgent != "" {
		t.Error("group should be dropped by disabled policy")
	}
	if q.HasPending("hermes") {
		t.Error("no message should be queued")
	}
}

func TestHandleIncomingGroupAll(t *testing.T) {
	cfg := defaultCfg()
	cfg.GroupPolicy = config.GroupAll
	s, _, q := setup(cfg)
	registerAgent(t, s, "hermes")

	res, _ := s.HandleIncoming(groupIncoming("hello", "oc_anygroup"))
	if res.EnqueuedAgent != "hermes" {
		t.Error("group should be allowed by All policy")
	}
	if !q.HasPending("hermes") {
		t.Error("expected pending")
	}
}

func TestHandleIncomingCommandUse(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	registerAgent(t, s, "hermes")
	registerAgent(t, s, "claude")

	res, err := s.HandleIncoming(dmIncoming("/use claude"))
	if err != nil {
		t.Fatal(err)
	}
	if res.SyncReply == "" {
		t.Error("expected sync reply")
	}
	if s.GetActiveAgent() != "claude" {
		t.Errorf("expected active=claude, got %q", s.GetActiveAgent())
	}
}

func TestHandleIncomingCommandUseNotFound(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	registerAgent(t, s, "hermes")

	res, _ := s.HandleIncoming(dmIncoming("/use ghost"))
	if res.SyncReply == "" || !strings.Contains(res.SyncReply, "not found") {
		t.Errorf("expected not-found reply, got %q", res.SyncReply)
	}
	if s.GetActiveAgent() != "hermes" {
		t.Error("active agent should be unchanged")
	}
}

func TestHandleIncomingCommandList(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	registerAgent(t, s, "hermes")
	registerAgent(t, s, "claude")

	res, _ := s.HandleIncoming(dmIncoming("/list"))
	if !strings.Contains(res.SyncReply, "hermes") || !strings.Contains(res.SyncReply, "claude") {
		t.Errorf("list should name agents: %q", res.SyncReply)
	}
}

func TestHandleIncomingCommandStatus(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	registerAgent(t, s, "hermes")

	res, _ := s.HandleIncoming(dmIncoming("/status"))
	if !strings.Contains(res.SyncReply, "Active agent: hermes") {
		t.Errorf("status should show active: %q", res.SyncReply)
	}
}

func TestHandleIncomingCommandHelp(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	registerAgent(t, s, "hermes")

	res, _ := s.HandleIncoming(dmIncoming("/gateway-help"))
	if !strings.Contains(res.SyncReply, "/use") || !strings.Contains(res.SyncReply, "/cmd") {
		t.Errorf("help should list commands: %q", res.SyncReply)
	}
}

func TestHandleIncomingCommandCmdPendingShell(t *testing.T) {
	s, _, q := setup(defaultCfg())
	registerAgent(t, s, "hermes")

	res, _ := s.HandleIncoming(dmIncoming("/cmd echo hi"))
	if res.PendingShell == nil {
		t.Fatal("expected PendingShell")
	}
	if res.PendingShell.Shell != "echo hi" {
		t.Errorf("shell mismatch: %q", res.PendingShell.Shell)
	}
	if res.EnqueuedAgent != "" || q.HasPending("hermes") {
		t.Error("/cmd should not enqueue a message")
	}
}

func TestHandleIncomingCommandCmdDangerous(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	registerAgent(t, s, "hermes")

	res, _ := s.HandleIncoming(dmIncoming("/cmd rm -rf /"))
	if res.PendingShell != nil {
		t.Error("dangerous command should not produce PendingShell")
	}
	if res.SyncReply == "" || !strings.Contains(res.SyncReply, "blocked") {
		t.Errorf("expected blocked reply, got %q", res.SyncReply)
	}
}

func TestHandleIncomingUnknownCommandPassthrough(t *testing.T) {
	s, _, q := setup(defaultCfg())
	registerAgent(t, s, "hermes")

	res, err := s.HandleIncoming(dmIncoming("/foobar something"))
	if err != nil {
		t.Fatal(err)
	}
	// Unrecognized command forwards to agent as a normal message.
	if res.EnqueuedAgent != "hermes" {
		t.Error("unrecognized command should pass through to agent")
	}
	if !q.HasPending("hermes") {
		t.Error("expected pending")
	}
}

func TestRegisterAutoActivatesFirst(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	if s.GetActiveAgent() != "" {
		t.Fatal("should start with no active agent")
	}
	active, _ := s.Register("first", "", nil)
	if active != "first" {
		t.Errorf("first register should auto-activate, got %q", active)
	}
	// Second register must NOT steal active.
	active, _ = s.Register("second", "", nil)
	if active != "first" {
		t.Errorf("second register should not change active, got %q", active)
	}
}

func TestPollDrainsAndMarksOnline(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	registerAgent(t, s, "hermes")

	s.HandleIncoming(dmIncoming("msg1"))
	s.HandleIncoming(dmIncoming("msg2"))

	msgs, err := s.Poll("hermes")
	if err != nil {
		t.Fatal(err)
	}
	if len(msgs) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(msgs))
	}
	// Second poll drains nothing.
	msgs2, _ := s.Poll("hermes")
	if len(msgs2) != 0 {
		t.Errorf("expected drain, got %d", len(msgs2))
	}
}

func TestPollUnknownAgent(t *testing.T) {
	s, _, _ := setup(defaultCfg())
	_, err := s.Poll("ghost")
	if !errors.Is(err, agent.ErrAgentNotFound) {
		t.Errorf("expected ErrAgentNotFound, got %v", err)
	}
}
