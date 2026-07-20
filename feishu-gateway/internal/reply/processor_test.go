package reply

import (
	"context"
	"strings"
	"sync"
	"testing"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/msgctx"
	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/storage"
)

// fakeFeishu implements feishuClient with configurable behaviour per call.
type fakeFeishu struct {
	mu sync.Mutex

	replyMessage func(ctx context.Context, messageID, msgType, content string) (string, error)
	sendMessage  func(ctx context.Context, receiveIDType, receiveID, msgType, content string) (string, error)
	uploadImage  func(ctx context.Context, path string) (string, error)
	uploadFile   func(ctx context.Context, path string) (string, error)

	// recorded invocations for assertions
	replyCalls   []replyCall
	sendCalls    []sendCall
	uploadImages []string
	uploadFiles  []string
}

type replyCall struct {
	messageID, msgType, content string
}

type sendCall struct {
	receiveIDType, receiveID, msgType, content string
}

func (f *fakeFeishu) ReplyMessage(ctx context.Context, messageID, msgType, content string) (string, error) {
	f.mu.Lock()
	f.replyCalls = append(f.replyCalls, replyCall{messageID, msgType, content})
	fn := f.replyMessage
	f.mu.Unlock()
	if fn != nil {
		return fn(ctx, messageID, msgType, content)
	}
	return "reply-msg-id", nil
}

func (f *fakeFeishu) SendMessage(ctx context.Context, receiveIDType, receiveID, msgType, content string) (string, error) {
	f.mu.Lock()
	f.sendCalls = append(f.sendCalls, sendCall{receiveIDType, receiveID, msgType, content})
	fn := f.sendMessage
	f.mu.Unlock()
	if fn != nil {
		return fn(ctx, receiveIDType, receiveID, msgType, content)
	}
	return "send-msg-id", nil
}

func (f *fakeFeishu) UploadImage(ctx context.Context, path string) (string, error) {
	f.mu.Lock()
	f.uploadImages = append(f.uploadImages, path)
	fn := f.uploadImage
	f.mu.Unlock()
	if fn != nil {
		return fn(ctx, path)
	}
	return "img-key-" + path, nil
}

func (f *fakeFeishu) UploadFile(ctx context.Context, path string) (string, error) {
	f.mu.Lock()
	f.uploadFiles = append(f.uploadFiles, path)
	fn := f.uploadFile
	f.mu.Unlock()
	if fn != nil {
		return fn(ctx, path)
	}
	return "file-key-" + path, nil
}

func newProcessor(t *testing.T, replyCh chan model.AgentReply, fc feishuClient) *Processor {
	t.Helper()
	return New(replyCh, fc, msgctx.New(), storage.NewInMemory())
}

// --- resolveTarget tests ---

func TestResolveTarget_replyWithContext(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	p.msgCtxs.Put("msg-1", msgctx.Info{
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_user",
		AgentName:     "claude",
	})

	idType, id, replyToID := p.resolveTarget(model.AgentReply{ReplyToID: "msg-1"})
	if idType != "open_id" {
		t.Errorf("receiveIDType = %q, want open_id", idType)
	}
	if id != "ou_user" {
		t.Errorf("receiveID = %q, want ou_user", id)
	}
	if replyToID != "msg-1" {
		t.Errorf("replyToID = %q, want msg-1", replyToID)
	}
}

func TestResolveTarget_replyContextLost(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	// No context stored for "msg-lost" — should fallback: open_id + reply.ReplyToID, replyToID=""
	idType, id, replyToID := p.resolveTarget(model.AgentReply{ReplyToID: "msg-lost"})
	if idType != "open_id" {
		t.Errorf("receiveIDType = %q, want open_id", idType)
	}
	if id != "msg-lost" {
		t.Errorf("receiveID = %q, want msg-lost", id)
	}
	if replyToID != "" {
		t.Errorf("replyToID = %q, want empty (fallback to SendMessage)", replyToID)
	}
}

func TestResolveTarget_proactiveOpenID(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	toUser := "ou_proactive"
	idType, id, replyToID := p.resolveTarget(model.AgentReply{
		ReplyToID: "",
		ToUser:    &toUser,
	})
	if idType != "open_id" {
		t.Errorf("receiveIDType = %q, want open_id", idType)
	}
	if id != "ou_proactive" {
		t.Errorf("receiveID = %q, want ou_proactive", id)
	}
	if replyToID != "" {
		t.Errorf("replyToID = %q, want empty", replyToID)
	}
}

func TestResolveTarget_proactiveChatID(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	toUser := "oc_chat"
	ctxToken := "chat_id"
	idType, id, replyToID := p.resolveTarget(model.AgentReply{
		ReplyToID:    "",
		ToUser:       &toUser,
		ContextToken: &ctxToken,
	})
	if idType != "chat_id" {
		t.Errorf("receiveIDType = %q, want chat_id", idType)
	}
	if id != "oc_chat" {
		t.Errorf("receiveID = %q, want oc_chat", id)
	}
	if replyToID != "" {
		t.Errorf("replyToID = %q, want empty", replyToID)
	}
}

func TestResolveTarget_noTarget(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	idType, id, replyToID := p.resolveTarget(model.AgentReply{})
	if idType != "" || id != "" || replyToID != "" {
		t.Errorf("expected all empty, got (%q, %q, %q)", idType, id, replyToID)
	}
}

// --- handle tests ---

func TestHandle_replyMessage(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	p.msgCtxs.Put("msg-1", msgctx.Info{
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_user",
	})

	p.handle(context.Background(), model.AgentReply{
		ReplyToID: "msg-1",
		Text:      "hello world",
	})

	if len(fc.replyCalls) != 1 {
		t.Fatalf("expected 1 ReplyMessage call, got %d", len(fc.replyCalls))
	}
	c := fc.replyCalls[0]
	if c.messageID != "msg-1" {
		t.Errorf("ReplyMessage messageID = %q, want msg-1", c.messageID)
	}
	if c.msgType != "text" {
		t.Errorf("ReplyMessage msgType = %q, want text", c.msgType)
	}
	if !strings.Contains(c.content, "hello world") {
		t.Errorf("ReplyMessage content missing text: %q", c.content)
	}
	if len(fc.sendCalls) != 0 {
		t.Errorf("expected 0 SendMessage calls, got %d", len(fc.sendCalls))
	}
}

func TestHandle_replyContextLostFallback(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	// No context for "msg-lost" — resolveTarget returns (open_id, "msg-lost", "").
	// send() sees replyToID="" and goes straight to SendMessage.
	p.handle(context.Background(), model.AgentReply{
		ReplyToID: "msg-lost",
		Text:      "fallback text",
	})

	if len(fc.replyCalls) != 0 {
		t.Fatalf("expected 0 ReplyMessage attempts (no replyToID), got %d", len(fc.replyCalls))
	}
	if len(fc.sendCalls) != 1 {
		t.Fatalf("expected 1 SendMessage call, got %d", len(fc.sendCalls))
	}
	sc := fc.sendCalls[0]
	if sc.receiveIDType != "open_id" {
		t.Errorf("SendMessage receiveIDType = %q, want open_id", sc.receiveIDType)
	}
	if sc.receiveID != "msg-lost" {
		t.Errorf("SendMessage receiveID = %q, want msg-lost (reply.ReplyToID)", sc.receiveID)
	}
}

func TestHandle_proactiveOpenID(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	toUser := "ou_proactive"
	p.handle(context.Background(), model.AgentReply{
		ToUser: &toUser,
		Text:   "proactive",
	})

	if len(fc.sendCalls) != 1 {
		t.Fatalf("expected 1 SendMessage call, got %d", len(fc.sendCalls))
	}
	sc := fc.sendCalls[0]
	if sc.receiveIDType != "open_id" {
		t.Errorf("SendMessage receiveIDType = %q, want open_id", sc.receiveIDType)
	}
	if sc.receiveID != "ou_proactive" {
		t.Errorf("SendMessage receiveID = %q, want ou_proactive", sc.receiveID)
	}
	if len(fc.replyCalls) != 0 {
		t.Errorf("expected 0 ReplyMessage calls, got %d", len(fc.replyCalls))
	}
}

func TestHandle_proactiveChatID(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	toUser := "oc_chat"
	ctxToken := "chat_id"
	p.handle(context.Background(), model.AgentReply{
		ToUser:       &toUser,
		ContextToken: &ctxToken,
		Text:         "group msg",
	})

	if len(fc.sendCalls) != 1 {
		t.Fatalf("expected 1 SendMessage call, got %d", len(fc.sendCalls))
	}
	sc := fc.sendCalls[0]
	if sc.receiveIDType != "chat_id" {
		t.Errorf("SendMessage receiveIDType = %q, want chat_id", sc.receiveIDType)
	}
	if sc.receiveID != "oc_chat" {
		t.Errorf("SendMessage receiveID = %q, want oc_chat", sc.receiveID)
	}
}

func TestHandle_mediaImage(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	p.msgCtxs.Put("msg-img", msgctx.Info{
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_user",
	})

	p.handle(context.Background(), model.AgentReply{
		ReplyToID:  "msg-img",
		MediaPaths: []string{"/tmp/photo.png", "/tmp/logo.jpg"},
	})

	if len(fc.uploadImages) != 2 {
		t.Fatalf("expected 2 UploadImage calls, got %d: %v", len(fc.uploadImages), fc.uploadImages)
	}
	if fc.uploadImages[0] != "/tmp/photo.png" {
		t.Errorf("uploadImages[0] = %q, want /tmp/photo.png", fc.uploadImages[0])
	}
	if fc.uploadImages[1] != "/tmp/logo.jpg" {
		t.Errorf("uploadImages[1] = %q, want /tmp/logo.jpg", fc.uploadImages[1])
	}

	if len(fc.replyCalls) != 2 {
		t.Fatalf("expected 2 ReplyMessage calls (one per image), got %d", len(fc.replyCalls))
	}
	for i, c := range fc.replyCalls {
		if c.msgType != "image" {
			t.Errorf("replyCalls[%d].msgType = %q, want image", i, c.msgType)
		}
		if c.messageID != "msg-img" {
			t.Errorf("replyCalls[%d].messageID = %q, want msg-img", i, c.messageID)
		}
	}
}

func TestHandle_mediaFile(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	p.msgCtxs.Put("msg-file", msgctx.Info{
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_user",
	})

	p.handle(context.Background(), model.AgentReply{
		ReplyToID:  "msg-file",
		MediaPaths: []string{"/tmp/doc.pdf", "/tmp/notes.txt"},
	})

	if len(fc.uploadFiles) != 2 {
		t.Fatalf("expected 2 UploadFile calls, got %d: %v", len(fc.uploadFiles), fc.uploadFiles)
	}
	if fc.uploadFiles[0] != "/tmp/doc.pdf" {
		t.Errorf("uploadFiles[0] = %q, want /tmp/doc.pdf", fc.uploadFiles[0])
	}
	if fc.uploadFiles[1] != "/tmp/notes.txt" {
		t.Errorf("uploadFiles[1] = %q, want /tmp/notes.txt", fc.uploadFiles[1])
	}

	if len(fc.replyCalls) != 2 {
		t.Fatalf("expected 2 ReplyMessage calls (one per file), got %d", len(fc.replyCalls))
	}
	for i, c := range fc.replyCalls {
		if c.msgType != "file" {
			t.Errorf("replyCalls[%d].msgType = %q, want file", i, c.msgType)
		}
	}
}

func TestHandle_mixedMedia(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	p.msgCtxs.Put("msg-mix", msgctx.Info{
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_user",
	})

	p.handle(context.Background(), model.AgentReply{
		ReplyToID:  "msg-mix",
		Text:       "See attached",
		MediaPaths: []string{"/tmp/img.png", "/tmp/data.xlsx"},
	})

	// Text should be a reply call
	if len(fc.replyCalls) != 3 {
		t.Fatalf("expected 3 ReplyMessage calls (text + image + file), got %d", len(fc.replyCalls))
	}
	if fc.replyCalls[0].msgType != "text" {
		t.Errorf("replyCalls[0].msgType = %q, want text", fc.replyCalls[0].msgType)
	}
	if fc.replyCalls[1].msgType != "image" {
		t.Errorf("replyCalls[1].msgType = %q, want image", fc.replyCalls[1].msgType)
	}
	if fc.replyCalls[2].msgType != "file" {
		t.Errorf("replyCalls[2].msgType = %q, want file", fc.replyCalls[2].msgType)
	}

	if len(fc.uploadImages) != 1 || fc.uploadImages[0] != "/tmp/img.png" {
		t.Errorf("uploadImages = %v, want [/tmp/img.png]", fc.uploadImages)
	}
	if len(fc.uploadFiles) != 1 || fc.uploadFiles[0] != "/tmp/data.xlsx" {
		t.Errorf("uploadFiles = %v, want [/tmp/data.xlsx]", fc.uploadFiles)
	}
}

func TestHandle_noTargetDrops(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	// ReplyToID empty, ToUser nil → no resolvable target
	p.handle(context.Background(), model.AgentReply{
		Text: "should be dropped",
	})

	if len(fc.sendCalls) != 0 {
		t.Errorf("expected 0 SendMessage calls, got %d", len(fc.sendCalls))
	}
	if len(fc.replyCalls) != 0 {
		t.Errorf("expected 0 ReplyMessage calls, got %d", len(fc.replyCalls))
	}
}

func TestHandle_uploadImageFailureSkips(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	var uploadCount int
	fc.uploadImage = func(_ context.Context, path string) (string, error) {
		uploadCount++
		if uploadCount == 1 {
			return "", assertAnError("upload failed for " + path)
		}
		// Second upload succeeds
		return "img-key-" + path, nil
	}
	p := newProcessor(t, replyCh, fc)

	p.msgCtxs.Put("msg-u", msgctx.Info{
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_user",
	})

	p.handle(context.Background(), model.AgentReply{
		ReplyToID:  "msg-u",
		Text:       "text part",
		MediaPaths: []string{"/tmp/fail.png", "/tmp/ok.jpg"},
	})

	// Text should still succeed
	if len(fc.replyCalls) != 2 {
		t.Fatalf("expected 2 ReplyMessage calls (text + ok image), got %d", len(fc.replyCalls))
	}
	if fc.replyCalls[0].msgType != "text" {
		t.Errorf("replyCalls[0].msgType = %q, want text", fc.replyCalls[0].msgType)
	}
	// Second reply should be for ok.jpg (the image that succeeded)
	if fc.replyCalls[1].msgType != "image" {
		t.Errorf("replyCalls[1].msgType = %q, want image", fc.replyCalls[1].msgType)
	}
	// Should have tried to upload both
	if len(fc.uploadImages) != 2 {
		t.Fatalf("expected 2 UploadImage attempts, got %d", len(fc.uploadImages))
	}
}

func TestHandle_emptyTextOnlySendsMedia(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	p.msgCtxs.Put("msg-e", msgctx.Info{
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_user",
	})

	p.handle(context.Background(), model.AgentReply{
		ReplyToID:  "msg-e",
		Text:       "  \t  ", // whitespace-only
		MediaPaths: []string{"/tmp/photo.png"},
	})

	// No text call, just one image reply
	if len(fc.replyCalls) != 1 {
		t.Fatalf("expected 1 ReplyMessage call (image only), got %d", len(fc.replyCalls))
	}
	if fc.replyCalls[0].msgType != "image" {
		t.Errorf("msgType = %q, want image", fc.replyCalls[0].msgType)
	}
	if len(fc.uploadImages) != 1 {
		t.Fatalf("expected 1 UploadImage, got %d", len(fc.uploadImages))
	}
}

func TestHandle_agentContextSaved(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	store := storage.NewInMemory()
	p := New(replyCh, fc, msgctx.New(), store)

	p.msgCtxs.Put("msg-ac", msgctx.Info{
		ReceiveIDType: "open_id",
		ReceiveID:     "ou_user",
	})

	ac := `{"agent":"claude","cwd":"/tmp"}`
	p.handle(context.Background(), model.AgentReply{
		ReplyToID:    "msg-ac",
		Text:         "with context",
		AgentContext: &ac,
	})

	if len(fc.replyCalls) != 1 {
		t.Fatalf("expected 1 ReplyMessage call, got %d", len(fc.replyCalls))
	}

	// Context is stored under the *returned* message ID ("reply-msg-id"),
	// not the reply_to argument ("msg-ac").
	got, err := store.GetMsgAgentContext("reply-msg-id")
	if err != nil {
		t.Fatalf("GetMsgAgentContext: %v", err)
	}
	if got != ac {
		t.Errorf("agent context = %q, want %q", got, ac)
	}
}

// --- send tests ---

func TestSend_replySuccess(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	p := newProcessor(t, replyCh, fc)

	msgID, err := p.send(context.Background(), "msg-1", "open_id", "ou_user", "text", `{"text":"hi"}`)
	if err != nil {
		t.Fatalf("send: %v", err)
	}
	if msgID != "reply-msg-id" {
		t.Errorf("msgID = %q, want reply-msg-id", msgID)
	}
	if len(fc.replyCalls) != 1 {
		t.Fatalf("expected 1 ReplyMessage call, got %d", len(fc.replyCalls))
	}
	if len(fc.sendCalls) != 0 {
		t.Errorf("expected 0 SendMessage calls on success, got %d", len(fc.sendCalls))
	}
}

func TestSend_replyFallbackToSend(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	fc.replyMessage = func(_ context.Context, messageID, msgType, content string) (string, error) {
		return "", assertAnError("reply failed")
	}
	p := newProcessor(t, replyCh, fc)

	msgID, err := p.send(context.Background(), "msg-1", "open_id", "ou_user", "text", `{"text":"hi"}`)
	if err != nil {
		t.Fatalf("send fallback: %v", err)
	}
	if msgID != "send-msg-id" {
		t.Errorf("msgID = %q, want send-msg-id", msgID)
	}
	if len(fc.replyCalls) != 1 {
		t.Fatalf("expected 1 ReplyMessage attempt, got %d", len(fc.replyCalls))
	}
	if len(fc.sendCalls) != 1 {
		t.Fatalf("expected 1 SendMessage fallback, got %d", len(fc.sendCalls))
	}
	if fc.sendCalls[0].receiveIDType != "open_id" {
		t.Errorf("receiveIDType = %q, want open_id", fc.sendCalls[0].receiveIDType)
	}
	if fc.sendCalls[0].receiveID != "ou_user" {
		t.Errorf("receiveID = %q, want ou_user", fc.sendCalls[0].receiveID)
	}
}

func TestSend_replyFallbackNoReceiveID(t *testing.T) {
	replyCh := make(chan model.AgentReply, 1)
	fc := &fakeFeishu{}
	fc.replyMessage = func(_ context.Context, messageID, msgType, content string) (string, error) {
		return "", assertAnError("reply failed")
	}
	p := newProcessor(t, replyCh, fc)

	// Empty receiveIDType/receiveID → cannot fallback (context-lost scenario with
	// no context).
	_, err := p.send(context.Background(), "msg-1", "", "", "text", `{"text":"hi"}`)
	if err == nil {
		t.Fatal("expected error when reply fails and no receiveID is provided")
	}
	if len(fc.replyCalls) != 1 {
		t.Fatalf("expected 1 ReplyMessage call, got %d", len(fc.replyCalls))
	}
	if len(fc.sendCalls) != 0 {
		t.Errorf("expected 0 SendMessage calls when receiveID empty, got %d", len(fc.sendCalls))
	}
}

// assertAnError returns a simple error for test purposes.
type assertAnError string

func (e assertAnError) Error() string { return string(e) }
