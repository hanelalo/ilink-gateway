package feishu

import (
	"context"
	"fmt"
	"log"
	"path/filepath"
	"sync/atomic"
	"time"

	lark "github.com/larksuite/oapi-sdk-go/v3"
	"github.com/larksuite/oapi-sdk-go/v3/event/dispatcher"
	larkim "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"
	larkws "github.com/larksuite/oapi-sdk-go/v3/ws"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/dedup"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/msgctx"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/router"
)

// WSConfig carries the dependencies the feishu WebSocket handler needs.
type WSConfig struct {
	AppID         string
	AppSecret     string
	BaseURL       string // https://open.feishu.cn (default) or https://open.larksuite.com
	BotOpenID     string
	MediaCacheDir string
	CmdMaxOutput  int

	Faucet   *Client        // feishu API client
	Router   *router.State  // gateway router
	Dedup    *dedup.Cache   // message dedup
	MsgCtxs  *msgctx.Store  // reply-target lookup
}

// WSClient owns the feishu long-lived WebSocket connection.
type WSClient struct {
	cfg       WSConfig
	ws        *larkws.Client
	connected atomic.Bool
}

func NewWSClient(cfg WSConfig) *WSClient {
	w := &WSClient{cfg: cfg}

	handler := dispatcher.NewEventDispatcher("", "").OnP2MessageReceiveV1(
		func(ctx context.Context, event *larkim.P2MessageReceiveV1) error {
			return w.handleEvent(event)
		},
	)

	opts := []larkws.ClientOption{
		larkws.WithEventHandler(handler),
		larkws.WithAutoReconnect(true),
		larkws.WithOnReady(func() {
			w.connected.Store(true)
			log.Printf("feishu WS connected")
		}),
		larkws.WithOnReconnected(func() {
			w.connected.Store(true)
			log.Printf("feishu WS reconnected")
		}),
		larkws.WithOnDisconnected(func() {
			w.connected.Store(false)
			log.Printf("feishu WS disconnected")
		}),
		larkws.WithOnError(func(err error) {
			log.Printf("feishu WS error: %v", err)
		}),
	}
	if cfg.BaseURL == lark.LarkBaseUrl {
		opts = append(opts, larkws.WithDomain(lark.LarkBaseUrl))
	}

	w.ws = larkws.NewClient(cfg.AppID, cfg.AppSecret, opts...)
	return w
}

// Start blocks until the WebSocket loop exits.
func (w *WSClient) Start(ctx context.Context) error {
	return w.ws.Start(ctx)
}

// IsConnected reports whether the WebSocket is currently online.
func (w *WSClient) IsConnected() bool { return w.connected.Load() }

// handleEvent is the 3-second-budget event handler. Heavy work is dispatched
// to goroutines so the handler returns promptly (feishu requires sub-3s ACK
// or it will re-push, relying on dedup to absorb the duplicate).
func (w *WSClient) handleEvent(event *larkim.P2MessageReceiveV1) error {
	normalized, mentionedBot, ok := NormalizeEvent(event, w.cfg.BotOpenID)
	if !ok {
		return nil
	}

	// Group chat requires @bot; DMs always pass.
	if normalized.IsGroup && !mentionedBot {
		return nil
	}

	// Dedup (feishu re-pushes on slow ACK).
	key := dedup.DedupKey(normalized.MessageID, normalized.Text)
	if !w.cfg.Dedup.CheckAndRecord(key) {
		return nil
	}

	result, err := w.cfg.Router.HandleIncoming(normalized)
	if err != nil {
		log.Printf("router error (msg=%s): %v", normalized.MessageID, err)
		return nil
	}

	// Record reply-target context for later agent replies.
	if result.EnqueuedAgent != "" {
		w.cfg.MsgCtxs.Put(normalized.MessageID, msgctx.Info{
			ReceiveIDType: normalized.ReceiveIDType,
			ReceiveID:     normalized.ReceiveID,
			AgentName:     result.EnqueuedAgent,
			ReceivedAt:    time.Now(),
		})
	}

	// Built-in command replies go out synchronously-in-background.
	if result.SyncReply != "" {
		text := result.SyncReply
		idType, id := normalized.ReceiveIDType, normalized.ReceiveID
		go func() {
			if _, err := w.cfg.Faucet.SendText(context.Background(), idType, id, text); err != nil {
				log.Printf("send command reply: %v", err)
			}
		}()
	}

	// /cmd runs asynchronously; result is sent back when it completes.
	if result.PendingShell != nil {
		shell := *result.PendingShell
		idType, id := normalized.ReceiveIDType, normalized.ReceiveID
		maxOut := w.cfg.CmdMaxOutput
		go func() {
			out, err := router.ExecuteCommand(context.Background(), shell.Shell, shell.TimeoutSecs, maxOut)
			if err != nil {
				out = "Command error: " + err.Error()
			}
			if _, err := w.cfg.Faucet.SendText(context.Background(), idType, id, out); err != nil {
				log.Printf("send cmd output: %v", err)
			}
		}()
	}

	// Media download runs asynchronously; fills the queued message when done.
	if result.EnqueuedAgent != "" && len(result.MediaKeys) > 0 {
		go w.downloadMediaAsync(normalized, result.EnqueuedAgent, result.MediaKeys)
	}

	return nil
}

func (w *WSClient) downloadMediaAsync(msg model.IncomingMessage, agentName string, keys []model.MediaKey) {
	items := make([]model.MediaItem, 0, len(keys))
	for i, k := range keys {
		resourceType := "file"
		if k.Kind == "image" {
			resourceType = "image"
		}
		ext := mediaExt(k.Kind)
		dest := filepath.Join(
			w.cfg.MediaCacheDir,
			time.Now().Format("2006-01-02"),
			fmt.Sprintf("%s_%d%s", msg.MessageID, i, ext),
		)
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		err := w.cfg.Faucet.DownloadResource(ctx, msg.MessageID, k.Key, resourceType, dest)
		cancel()
		if err != nil {
			log.Printf("media download (msg=%s key=%s): %v", msg.MessageID, k.Key, err)
			continue
		}
		items = append(items, model.MediaItem{
			MediaType: mapMediaKind(k.Kind),
			LocalPath: dest,
		})
		if k.Name != "" {
			name := k.Name
			items[len(items)-1].OriginalName = &name
		}
	}
	if len(items) > 0 && !w.cfg.Router.UpdateLastMedia(agentName, items) {
		log.Printf("media backfill: no queued message for agent %q (already polled?)", agentName)
	}
}

func mediaExt(kind string) string {
	switch kind {
	case "image":
		return ".jpg"
	case "audio":
		return ".opus"
	case "media":
		return ".mp4"
	default:
		return ".bin"
	}
}

func mapMediaKind(kind string) string {
	switch kind {
	case "audio":
		return "voice"
	case "media":
		return "video"
	default:
		return kind
	}
}
