package api

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/agent"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/config"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/router"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/storage"
)

func setupServer(t *testing.T) (*httptest.Server, *router.State, chan model.AgentReply) {
	t.Helper()
	cfg := config.Config{
		DmPolicy:         config.DMOpen,
		GroupPolicy:      config.GroupDisabled,
		CmdTimeoutSecs:   30,
		CmdMaxOutputChars: 2000,
		AllowedUsers:     map[string]struct{}{},
		AllowedGroups:    map[string]struct{}{},
	}
	reg := agent.NewRegistry()
	q := agent.NewMessageQueue()
	state := router.NewState(cfg, reg, q)
	replyCh := make(chan model.AgentReply, 16)
	store := storage.NewInMemory()
	srv := NewServer(state, replyCh, store, func() bool { return false })
	ts := httptest.NewServer(srv.Handler())
	t.Cleanup(ts.Close)
	return ts, state, replyCh
}

func postJSON(t *testing.T, url string, body any) *http.Response {
	t.Helper()
	b, _ := json.Marshal(body)
	resp, err := http.Post(url, "application/json", bytes.NewReader(b))
	if err != nil {
		t.Fatal(err)
	}
	return resp
}

func decode(t *testing.T, resp *http.Response) map[string]any {
	t.Helper()
	var body map[string]any
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
		t.Fatal(err)
	}
	return body
}

func register(t *testing.T, ts *httptest.Server, name string) {
	t.Helper()
	resp := postJSON(t, ts.URL+"/api/agents/register", map[string]any{
		"name": name, "capabilities": []string{"text"},
	})
	if resp.StatusCode != 200 {
		t.Fatalf("register status %d", resp.StatusCode)
	}
}

func TestRegisterReturnsActiveAgent(t *testing.T) {
	ts, _, _ := setupServer(t)
	resp := postJSON(t, ts.URL+"/api/agents/register", map[string]any{
		"name": "claude", "capabilities": []string{"text"},
	})
	if resp.StatusCode != 200 {
		t.Fatalf("status %d", resp.StatusCode)
	}
	body := decode(t, resp)
	if body["ok"] != true {
		t.Error("ok should be true")
	}
	if body["active_agent"] != "claude" {
		t.Errorf("active_agent=%v", body["active_agent"])
	}
}

func TestRegisterEmptyName(t *testing.T) {
	ts, _, _ := setupServer(t)
	resp := postJSON(t, ts.URL+"/api/agents/register", map[string]any{
		"name": "", "capabilities": []string{},
	})
	if resp.StatusCode != 400 {
		t.Fatalf("expected 400, got %d", resp.StatusCode)
	}
	body := decode(t, resp)
	if body["ok"] != false {
		t.Error("ok should be false")
	}
	if body["error"] == nil {
		t.Error("error should be present")
	}
}

func TestRegisterFirstAutoActivates(t *testing.T) {
	ts, _, _ := setupServer(t)
	register(t, ts, "first")
	// Second register must not steal active.
	resp := postJSON(t, ts.URL+"/api/agents/register", map[string]any{
		"name": "second", "capabilities": []string{"text"},
	})
	body := decode(t, resp)
	if body["active_agent"] != "first" {
		t.Errorf("expected first to remain active, got %v", body["active_agent"])
	}
}

func TestPollUnknownAgent404(t *testing.T) {
	ts, _, _ := setupServer(t)
	resp, err := http.Get(ts.URL + "/api/agents/ghost/poll")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 404 {
		t.Fatalf("expected 404, got %d", resp.StatusCode)
	}
}

func TestPollEmptyMessages(t *testing.T) {
	ts, _, _ := setupServer(t)
	register(t, ts, "claude")
	resp, err := http.Get(ts.URL + "/api/agents/claude/poll")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("status %d", resp.StatusCode)
	}
	body := decode(t, resp)
	msgs, ok := body["messages"].([]any)
	if !ok {
		t.Fatalf("messages not array: %T", body["messages"])
	}
	if len(msgs) != 0 {
		t.Errorf("expected empty, got %d", len(msgs))
	}
}

// TestPollMediaAlwaysArray is the critical regression guard: the media field
// MUST serialize as [] (never null, never omitted). The TS client types
// `media: MediaItem[]` (non-optional), so null would crash it.
func TestPollMediaAlwaysArray(t *testing.T) {
	ts, state, _ := setupServer(t)
	register(t, ts, "claude")
	state.HandleIncoming(model.IncomingMessage{
		MessageID:     "m1",
		FromUser:      "ou_sender",
		Text:          "hi",
		MessageType:   "text",
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_sender",
	})
	resp, err := http.Get(ts.URL + "/api/agents/claude/poll")
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(resp.Body)
	bodyStr := string(body)
	if strings.Contains(bodyStr, `"media":null`) {
		t.Errorf("media must not be null:\n%s", bodyStr)
	}
	if !strings.Contains(bodyStr, `"media":[]`) {
		t.Errorf("media must be []:\n%s", bodyStr)
	}
}

func TestPollDrainsQueue(t *testing.T) {
	ts, state, _ := setupServer(t)
	register(t, ts, "claude")
	state.HandleIncoming(model.IncomingMessage{
		MessageID: "m1", FromUser: "u", Text: "first",
		MessageType: "text", ReceiveIDType: "open_id", ReceiveID: "u",
	})
	state.HandleIncoming(model.IncomingMessage{
		MessageID: "m2", FromUser: "u", Text: "second",
		MessageType: "text", ReceiveIDType: "open_id", ReceiveID: "u",
	})

	resp, _ := http.Get(ts.URL + "/api/agents/claude/poll")
	body := decode(t, resp)
	if len(body["messages"].([]any)) != 2 {
		t.Errorf("expected 2 messages, got %v", body["messages"])
	}

	// Second poll should be empty (drained).
	resp2, _ := http.Get(ts.URL + "/api/agents/claude/poll")
	body2 := decode(t, resp2)
	if len(body2["messages"].([]any)) != 0 {
		t.Errorf("queue should be drained")
	}
}

func TestReplyAlways200(t *testing.T) {
	ts, _, _ := setupServer(t)
	register(t, ts, "claude")
	resp := postJSON(t, ts.URL+"/api/agents/claude/reply", model.AgentReply{
		ReplyToID: "m1", Text: "hello",
	})
	if resp.StatusCode != 200 {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	body := decode(t, resp)
	if body["ok"] != true {
		t.Error("ok should be true")
	}
}

func TestReplyPutsOnChannel(t *testing.T) {
	ts, _, replyCh := setupServer(t)
	register(t, ts, "claude")
	postJSON(t, ts.URL+"/api/agents/claude/reply", model.AgentReply{
		ReplyToID: "m1", Text: "hi",
	})
	select {
	case reply := <-replyCh:
		if reply.Text != "hi" {
			t.Errorf("text=%q", reply.Text)
		}
	case <-time.After(time.Second):
		t.Error("no reply received on channel")
	}
}

func TestStatusFormat(t *testing.T) {
	ts, _, _ := setupServer(t)
	register(t, ts, "claude")
	resp, err := http.Get(ts.URL + "/api/status")
	if err != nil {
		t.Fatal(err)
	}
	body := decode(t, resp)
	if body["feishu"] == nil {
		t.Error("missing feishu field")
	}
	if body["active_agent"] != "claude" {
		t.Errorf("active=%v", body["active_agent"])
	}
	agents, ok := body["agents"].(map[string]any)
	if !ok {
		t.Fatalf("agents not object: %T", body["agents"])
	}
	claude, ok := agents["claude"].(map[string]any)
	if !ok {
		t.Fatal("claude not in agents")
	}
	if claude["status"] != "online" {
		t.Errorf("status=%v", claude["status"])
	}
}

func TestStatusActiveAgentNullWhenEmpty(t *testing.T) {
	ts, _, _ := setupServer(t)
	resp, _ := http.Get(ts.URL + "/api/status")
	body := decode(t, resp)
	if body["active_agent"] != nil {
		t.Errorf("expected null when no agent active, got %v", body["active_agent"])
	}
}
