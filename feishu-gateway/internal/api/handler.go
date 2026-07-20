package api

import (
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"time"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/agent"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

func writeJSON(w http.ResponseWriter, status int, body any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(body)
}

func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Name         string   `json:"name"`
		Capabilities []string `json:"capabilities"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]any{"ok": false, "error": "invalid JSON: " + err.Error()})
		return
	}
	active, err := s.state.Register(req.Name, "", req.Capabilities)
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]any{"ok": false, "error": err.Error()})
		return
	}
	if active != "" {
		if err := s.store.SetState("active_agent", active); err != nil {
			log.Printf("warn: persist active_agent: %v", err)
		}
	}
	resp := map[string]any{"ok": true}
	if active != "" {
		resp["active_agent"] = active
	} else {
		resp["active_agent"] = nil
	}
	writeJSON(w, http.StatusOK, resp)
}

func (s *Server) handlePoll(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	msgs, err := s.state.Poll(name)
	if err != nil {
		if errors.Is(err, agent.ErrAgentNotFound) {
			writeJSON(w, http.StatusNotFound, map[string]any{"error": fmt.Sprintf("Agent '%s' not found", name)})
			return
		}
		writeJSON(w, http.StatusInternalServerError, map[string]any{"error": err.Error()})
		return
	}
	out := make([]model.AgentMessage, len(msgs))
	for i, m := range msgs {
		out[i] = m.ToAgentMessage()
	}
	writeJSON(w, http.StatusOK, map[string]any{"messages": out})
}

func (s *Server) handleReply(w http.ResponseWriter, r *http.Request) {
	var reply model.AgentReply
	if err := json.NewDecoder(r.Body).Decode(&reply); err != nil {
		// Per contract, /reply always returns 200 {ok:true} even on decode
		// failure — the agent only checks the ok field.
		writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
		return
	}
	select {
	case s.replyCh <- reply:
	case <-time.After(1 * time.Second):
		log.Printf("warn: reply channel full, dropping reply (reply_to=%s agent=%s)",
			reply.ReplyToID, r.PathValue("name"))
	}
	// Always 200 — errors surface only in gateway logs, matching Rust behavior.
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *Server) handleStatus(w http.ResponseWriter, r *http.Request) {
	agents := s.state.ListAgents()
	agentMap := make(map[string]any, len(agents))
	for _, a := range agents {
		agentMap[a.Name] = map[string]any{
			"status":       a.Status.String(),
			"capabilities": a.Capabilities,
			"last_seen":    a.LastSeen,
		}
	}
	connected := false
	if s.feishuConnected != nil {
		connected = s.feishuConnected()
	}
	active := s.state.GetActiveAgent()
	var activeVal any
	if active != "" {
		activeVal = active
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"feishu":       map[string]bool{"connected": connected},
		"active_agent": activeVal,
		"agents":       agentMap,
	})
}
