package main

import (
	"context"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"sync"
	"syscall"
	"time"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/agent"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/api"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/breaker"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/config"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/dedup"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/feishu"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/msgctx"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/reply"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/router"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/storage"
)

func main() {
	cfg, err := config.FromEnv()
	if err != nil {
		log.Fatalf("config: %v", err)
	}

	store, err := storage.NewSQLite(cfg.DBPath)
	if err != nil {
		log.Fatalf("open sqlite store %q: %v", cfg.DBPath, err)
	}
	defer func() {
		if cerr := store.Close(); cerr != nil {
			log.Printf("store close: %v", cerr)
		}
	}()
	reg := agent.NewRegistry()
	queue := agent.NewMessageQueue()
	state := router.NewState(cfg, reg, queue)

	if active, err := store.GetState("active_agent"); err == nil && active != "" {
		state.RestoreActiveAgent(active)
		log.Printf("restored active_agent=%q", active)
	}

	replyCh := make(chan model.AgentReply, cfg.ReplyQueueDepth)

	// Resilience primitives.
	brk := breaker.New(1, 30*time.Second, 30*time.Second)
	dedupCache := dedup.New(time.Duration(cfg.DedupTTLSecs) * time.Second)
	msgCtxs := msgctx.New()

	// Feishu client + WebSocket receiver.
	feishuClient := feishu.NewClient(cfg.FeishuAppID, cfg.FeishuAppSecret, cfg.FeishuBaseURL, brk)
	wsClient := feishu.NewWSClient(feishu.WSConfig{
		AppID:         cfg.FeishuAppID,
		AppSecret:     cfg.FeishuAppSecret,
		BaseURL:       cfg.FeishuBaseURL,
		BotOpenID:     cfg.FeishuBotOpenID,
		MediaCacheDir: cfg.MediaCacheDir,
		CmdMaxOutput:  cfg.CmdMaxOutputChars,
		Faucet:        feishuClient,
		Router:        state,
		Dedup:         dedupCache,
		MsgCtxs:       msgCtxs,
	})

	proc := reply.New(replyCh, feishuClient, msgCtxs, store)

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	var wg sync.WaitGroup
	wg.Add(3)
	go func() { defer wg.Done(); proc.Run(ctx) }()
	go func() {
		defer wg.Done()
		runHeartbeatChecker(ctx, state, cfg.HeartbeatCheckIntervalSecs, cfg.HeartbeatTimeoutSecs)
	}()
	go func() {
		defer wg.Done()
		runMsgCtxCleanup(ctx, msgCtxs, 5*time.Minute, 10*time.Minute)
	}()

	go func() {
		if err := wsClient.Start(ctx); err != nil {
			log.Printf("feishu WS exited: %v", err)
		}
	}()

	srv := api.NewServer(state, replyCh, store, wsClient.IsConnected)
	httpServer := &http.Server{
		Addr:    net.JoinHostPort(cfg.HTTPAddr, strconv.Itoa(cfg.HTTPPort)),
		Handler: srv.Handler(),
	}

	go func() {
		<-ctx.Done()
		log.Println("shutting down...")
		shutCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = httpServer.Shutdown(shutCtx)
	}()

	log.Printf("feishu-gateway HTTP listening on %s (base_url=%s)", httpServer.Addr, cfg.FeishuBaseURL)
	if cfg.FeishuBotOpenID == "" {
		log.Printf("warning: GW_FEISHU_BOT_OPEN_ID not set; group @-mention detection disabled")
	}
	if err := httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatalf("http server: %v", err)
	}
	wg.Wait()
}

func runHeartbeatChecker(ctx context.Context, state *router.State, intervalSecs, timeoutSecs int) {
	ticker := time.NewTicker(time.Duration(intervalSecs) * time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			for _, name := range state.CheckHeartbeat(timeoutSecs) {
				log.Printf("agent %q marked offline (heartbeat timeout)", name)
			}
		}
	}
}

func runMsgCtxCleanup(ctx context.Context, s *msgctx.Store, interval, maxAge time.Duration) {
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if n := s.Cleanup(maxAge); n > 0 {
				log.Printf("msgctx cleanup: evicted %d entries", n)
			}
		}
	}
}
