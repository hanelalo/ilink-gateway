package feishu

import (
	"testing"

	larkim "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"
)

func ptr(s string) *string { return &s }

func makeTextEvent(text, chatType string) *larkim.P2MessageReceiveV1 {
	return &larkim.P2MessageReceiveV1{
		Event: &larkim.P2MessageReceiveV1Data{
			Sender: &larkim.EventSender{
				SenderId: &larkim.UserId{OpenId: ptr("ou_sender")},
			},
			Message: &larkim.EventMessage{
				MessageId:   ptr("om_123"),
				ChatId:      ptr("oc_chat1"),
				ChatType:    ptr(chatType),
				MessageType: ptr("text"),
				Content:     ptr(`{"text":"` + text + `"}`),
				CreateTime:  ptr("1700000000000"),
			},
		},
	}
}

func TestNormalizeTextDM(t *testing.T) {
	msg, mentioned, ok := NormalizeEvent(makeTextEvent("hello world", "p2p"), "ou_bot")
	if !ok {
		t.Fatal("expected ok")
	}
	if msg.Text != "hello world" {
		t.Errorf("text=%q", msg.Text)
	}
	if msg.MessageType != "text" {
		t.Errorf("type=%q", msg.MessageType)
	}
	if msg.FromUser != "ou_sender" {
		t.Errorf("from=%q", msg.FromUser)
	}
	if msg.IsGroup {
		t.Error("should be DM")
	}
	if msg.ReceiveIDType != "open_id" || msg.ReceiveID != "ou_sender" {
		t.Errorf("receive=%s/%s", msg.ReceiveIDType, msg.ReceiveID)
	}
	if msg.MessageID != "om_123" {
		t.Errorf("id=%q", msg.MessageID)
	}
	if msg.Timestamp != 1700000000000 {
		t.Errorf("ts=%d", msg.Timestamp)
	}
	if mentioned {
		t.Error("should not be mentioned in DM text")
	}
}

func TestNormalizeGroupUsesChatID(t *testing.T) {
	msg, _, ok := NormalizeEvent(makeTextEvent("hi", "group"), "ou_bot")
	if !ok {
		t.Fatal("ok expected")
	}
	if !msg.IsGroup {
		t.Error("should be group")
	}
	if msg.ReceiveIDType != "chat_id" || msg.ReceiveID != "oc_chat1" {
		t.Errorf("group receive should be chat_id/oc_chat1, got %s/%s", msg.ReceiveIDType, msg.ReceiveID)
	}
}

func TestNormalizeImage(t *testing.T) {
	ev := makeTextEvent("", "p2p")
	ev.Event.Message.MessageType = ptr("image")
	ev.Event.Message.Content = ptr(`{"image_key":"img_abc"}`)
	msg, _, ok := NormalizeEvent(ev, "ou_bot")
	if !ok {
		t.Fatal("ok expected")
	}
	if msg.MessageType != "image" {
		t.Errorf("type=%q", msg.MessageType)
	}
	if len(msg.MediaKeys) != 1 || msg.MediaKeys[0].Kind != "image" || msg.MediaKeys[0].Key != "img_abc" {
		t.Errorf("mediakeys=%+v", msg.MediaKeys)
	}
}

func TestNormalizeFile(t *testing.T) {
	ev := makeTextEvent("", "p2p")
	ev.Event.Message.MessageType = ptr("file")
	ev.Event.Message.Content = ptr(`{"file_key":"file_abc","file_name":"report.pdf"}`)
	msg, _, ok := NormalizeEvent(ev, "ou_bot")
	if !ok {
		t.Fatal("ok expected")
	}
	if msg.MessageType != "file" {
		t.Errorf("type=%q", msg.MessageType)
	}
	if len(msg.MediaKeys) != 1 || msg.MediaKeys[0].Kind != "file" || msg.MediaKeys[0].Key != "file_abc" {
		t.Errorf("mediakeys=%+v", msg.MediaKeys)
	}
	if msg.MediaKeys[0].Name != "report.pdf" {
		t.Errorf("expected file_name=report.pdf, got %q", msg.MediaKeys[0].Name)
	}
}

func TestNormalizeAudioMappedToVoice(t *testing.T) {
	ev := makeTextEvent("", "p2p")
	ev.Event.Message.MessageType = ptr("audio")
	ev.Event.Message.Content = ptr(`{"file_key":"file_abc"}`)
	msg, _, _ := NormalizeEvent(ev, "ou_bot")
	if msg.MessageType != "voice" {
		t.Errorf("expected voice, got %q", msg.MessageType)
	}
	if len(msg.MediaKeys) != 1 || msg.MediaKeys[0].Kind != "audio" {
		t.Errorf("mediakeys=%+v", msg.MediaKeys)
	}
}

func TestNormalizeMediaMappedToVideo(t *testing.T) {
	ev := makeTextEvent("", "p2p")
	ev.Event.Message.MessageType = ptr("media")
	ev.Event.Message.Content = ptr(`{"file_key":"file_abc"}`)
	msg, _, _ := NormalizeEvent(ev, "ou_bot")
	if msg.MessageType != "video" {
		t.Errorf("expected video, got %q", msg.MessageType)
	}
}

func TestNormalizeMentionCleanup(t *testing.T) {
	ev := makeTextEvent("@_user_1 hello", "group")
	ev.Event.Message.Mentions = []*larkim.MentionEvent{
		{Key: ptr("@_user_1"), Id: &larkim.UserId{OpenId: ptr("ou_bot")}, Name: ptr("MyBot")},
	}
	msg, mentioned, _ := NormalizeEvent(ev, "ou_bot")
	if msg.Text != "hello" {
		t.Errorf("mention placeholder not cleaned, text=%q", msg.Text)
	}
	if !mentioned {
		t.Error("should detect bot mentioned")
	}
}

func TestNormalizeMentionNotBot(t *testing.T) {
	ev := makeTextEvent("@_user_1 hello", "group")
	ev.Event.Message.Mentions = []*larkim.MentionEvent{
		{Key: ptr("@_user_1"), Id: &larkim.UserId{OpenId: ptr("ou_other")}, Name: ptr("Someone")},
	}
	_, mentioned, _ := NormalizeEvent(ev, "ou_bot")
	if mentioned {
		t.Error("should not detect bot mentioned when @-ing someone else")
	}
}

func TestNormalizePost(t *testing.T) {
	ev := makeTextEvent("", "p2p")
	ev.Event.Message.MessageType = ptr("post")
	ev.Event.Message.Content = ptr(`{"zh_cn":{"title":"T","content":[[{"tag":"text","text":"hello "},{"tag":"text","text":"world"}]]}}`)
	msg, _, _ := NormalizeEvent(ev, "ou_bot")
	if msg.Text != "T\nhello world" {
		t.Errorf("post text extraction wrong: %q", msg.Text)
	}
	if msg.MessageType != "text" {
		t.Errorf("post should map to text type, got %q", msg.MessageType)
	}
}

func TestNormalizeNilEvent(t *testing.T) {
	_, _, ok := NormalizeEvent(nil, "ou_bot")
	if ok {
		t.Error("nil event should return ok=false")
	}
}

func TestNormalizeUnknownTypeFallback(t *testing.T) {
	ev := makeTextEvent("", "p2p")
	ev.Event.Message.MessageType = ptr("sticker")
	msg, _, _ := NormalizeEvent(ev, "ou_bot")
	if msg.MessageType != "text" {
		t.Errorf("unknown should fall back to text, got %q", msg.MessageType)
	}
	if msg.Text == "" {
		t.Error("fallback text should mention unsupported type")
	}
}
