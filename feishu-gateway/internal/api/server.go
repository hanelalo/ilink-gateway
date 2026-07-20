package api

import (
	"net/http"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/router"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/storage"
)

// Server wires the HTTP handlers to router state, the reply channel, and the
// persistent store.
type Server struct {
	state           *router.State
	replyCh         chan<- model.AgentReply
	store           storage.Store
	feishuConnected func() bool
}

func NewServer(state *router.State, replyCh chan<- model.AgentReply, store storage.Store, feishuConnected func() bool) *Server {
	return &Server{
		state:           state,
		replyCh:         replyCh,
		store:           store,
		feishuConnected: feishuConnected,
	}
}

// Handler returns the HTTP handler tree. Uses Go 1.22+ ServeMux pattern
// routing: method + path with {name} placeholder, read via r.PathValue.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /api/agents/register", s.handleRegister)
	mux.HandleFunc("GET /api/agents/{name}/poll", s.handlePoll)
	mux.HandleFunc("POST /api/agents/{name}/reply", s.handleReply)
	mux.HandleFunc("GET /api/status", s.handleStatus)
	return mux
}
