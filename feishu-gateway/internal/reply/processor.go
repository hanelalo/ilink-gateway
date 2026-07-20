package reply

import (
	"context"
	"encoding/json"
	"log"
	"strings"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/feishu"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/msgctx"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/storage"
)

// Processor consumes AgentReply values from the reply channel and dispatches
// them to feishu. Single goroutine — avoids hitting the feishu QPS limit and
// keeps reply ordering deterministic per message.
type Processor struct {
	replyCh <-chan model.AgentReply
	feishu  *feishu.Client
	msgCtxs *msgctx.Store
	store   storage.Store
}

func New(replyCh <-chan model.AgentReply, fc *feishu.Client, msgCtxs *msgctx.Store, store storage.Store) *Processor {
	return &Processor{replyCh: replyCh, feishu: fc, msgCtxs: msgCtxs, store: store}
}

// Run blocks until the context is cancelled or the reply channel is closed.
func (p *Processor) Run(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case reply, ok := <-p.replyCh:
			if !ok {
				return
			}
			p.handle(ctx, reply)
		}
	}
}

func (p *Processor) handle(ctx context.Context, reply model.AgentReply) {
	receiveIDType, receiveID, replyToID := p.resolveTarget(reply)
	if receiveID == "" && replyToID == "" {
		log.Printf("reply dropped: no resolvable target (reply_to=%s)", reply.ReplyToID)
		return
	}

	// 1. Send the text part (if any).
	if strings.TrimSpace(reply.Text) != "" {
		content, _ := json.Marshal(map[string]string{"text": reply.Text})
		msgID, err := p.send(ctx, replyToID, receiveIDType, receiveID, "text", string(content))
		if err != nil {
			log.Printf("reply text send failed (reply_to=%s): %v", reply.ReplyToID, err)
		} else if reply.AgentContext != nil && msgID != "" {
			if err := p.store.SaveMsgAgentContext(msgID, *reply.AgentContext); err != nil {
				log.Printf("save agent_context (msg=%s): %v", msgID, err)
			}
		}
	}

	// 2. Send media attachments. One feishu message per file.
	for _, path := range reply.MediaPaths {
		if feishu.IsImagePath(path) {
			key, err := p.feishu.UploadImage(ctx, path)
			if err != nil {
				log.Printf("upload image %s: %v", path, err)
				continue
			}
			content, _ := json.Marshal(map[string]string{"image_key": key})
			if _, err := p.send(ctx, replyToID, receiveIDType, receiveID, "image", string(content)); err != nil {
				log.Printf("send image %s: %v", path, err)
			}
		} else {
			key, err := p.feishu.UploadFile(ctx, path)
			if err != nil {
				log.Printf("upload file %s: %v", path, err)
				continue
			}
			content, _ := json.Marshal(map[string]string{"file_key": key})
			if _, err := p.send(ctx, replyToID, receiveIDType, receiveID, "file", string(content)); err != nil {
				log.Printf("send file %s: %v", path, err)
			}
		}
	}
}

// resolveTarget returns (receiveIDType, receiveID, replyToID). A non-empty
// replyToID means "reply to this message"; otherwise it's a proactive send.
func (p *Processor) resolveTarget(reply model.AgentReply) (receiveIDType, receiveID, replyToID string) {
	if reply.ReplyToID != "" {
		if info, ok := p.msgCtxs.Get(reply.ReplyToID); ok {
			return info.ReceiveIDType, info.ReceiveID, reply.ReplyToID
		}
		// Context lost (restart / cleanup). Best-effort: treat as DM open_id.
		return "open_id", reply.ReplyToID, ""
	}
	if reply.ToUser != nil && *reply.ToUser != "" {
		idType := "open_id"
		if reply.ContextToken != nil && *reply.ContextToken == "chat_id" {
			idType = "chat_id"
		}
		return idType, *reply.ToUser, ""
	}
	return "", "", ""
}

// send routes to ReplyMessage when we have a replyToID, else SendMessage.
// On reply failure (e.g. original message recalled), it falls back to a
// fresh SendMessage to the same chat.
func (p *Processor) send(ctx context.Context, replyToID, receiveIDType, receiveID, msgType, content string) (string, error) {
	if replyToID != "" {
		msgID, err := p.feishu.ReplyMessage(ctx, replyToID, msgType, content)
		if err == nil {
			return msgID, nil
		}
		log.Printf("reply failed (msg=%s), falling back to send: %v", replyToID, err)
		if receiveIDType == "" || receiveID == "" {
			return "", err
		}
	}
	return p.feishu.SendMessage(ctx, receiveIDType, receiveID, msgType, content)
}
