package feishu

import (
	"encoding/json"
	"strconv"
	"strings"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"

	larkim "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"
)

// NormalizeEvent converts a Feishu P2MessageReceiveV1 event into a
// platform-agnostic model.IncomingMessage. Returns:
//   - the normalized message
//   - whether the bot was @-mentioned (only meaningful for group chats)
//   - whether the event was valid (false → caller should silently drop)
func NormalizeEvent(event *larkim.P2MessageReceiveV1, botOpenID string) (model.IncomingMessage, bool, bool) {
	if event == nil || event.Event == nil || event.Event.Message == nil || event.Event.Sender == nil {
		return model.IncomingMessage{}, false, false
	}
	em := event.Event.Message
	sender := event.Event.Sender

	messageID := strPtrOr(em.MessageId, "")
	chatID := strPtrOr(em.ChatId, "")
	chatType := strPtrOr(em.ChatType, "")
	msgType := strPtrOr(em.MessageType, "")
	content := strPtrOr(em.Content, "")
	createTime := strPtrOr(em.CreateTime, "")

	isGroup := chatType == "group" || chatType == "topic_group"

	fromUser := ""
	if sender.SenderId != nil {
		fromUser = strPtrOr(sender.SenderId.OpenId, "")
	}

	mentionedBot := false
	if botOpenID != "" {
		for _, m := range em.Mentions {
			if m == nil || m.Id == nil {
				continue
			}
			if strPtrOr(m.Id.OpenId, "") == botOpenID {
				mentionedBot = true
				break
			}
		}
	}

	text, mappedType, mediaKeys := normalizeContent(msgType, content)

	// Clean @-mention placeholders ("@_user_1") from text.
	for _, m := range em.Mentions {
		if m != nil && m.Key != nil {
			text = strings.ReplaceAll(text, *m.Key, "")
		}
	}
	text = strings.TrimSpace(text)

	var ts int64
	if createTime != "" {
		if n, err := strconv.ParseInt(createTime, 10, 64); err == nil {
			ts = n
		}
	}

	receiveIDType := "open_id"
	receiveID := fromUser
	if isGroup {
		receiveIDType = "chat_id"
		receiveID = chatID
	}

	return model.IncomingMessage{
		MessageID:     messageID,
		FromUser:      fromUser,
		ChatID:        chatID,
		IsGroup:       isGroup,
		Text:          text,
		MessageType:   mappedType,
		Timestamp:     ts,
		MediaKeys:     mediaKeys,
		ReceiveIDType: receiveIDType,
		ReceiveID:     receiveID,
	}, mentionedBot, true
}

func strPtrOr(p *string, fallback string) string {
	if p == nil {
		return fallback
	}
	return *p
}

// normalizeContent maps a feishu message_type + content JSON pair to the
// gateway's internal (text, mappedType, mediaKeys) triple.
func normalizeContent(msgType, content string) (text, mappedType string, mediaKeys []model.MediaKey) {
	switch msgType {
	case "text":
		var c struct {
			Text string `json:"text"`
		}
		_ = json.Unmarshal([]byte(content), &c)
		return c.Text, "text", nil

	case "post":
		return extractPostText(content), "text", nil

	case "image":
		var c struct {
			ImageKey string `json:"image_key"`
		}
		_ = json.Unmarshal([]byte(content), &c)
		return "", "image", []model.MediaKey{{Kind: "image", Key: c.ImageKey}}

	case "file":
		var c struct {
			FileKey  string `json:"file_key"`
			FileName string `json:"file_name"`
		}
		_ = json.Unmarshal([]byte(content), &c)
		return "", "file", []model.MediaKey{{Kind: "file", Key: c.FileKey, Name: c.FileName}}

	case "audio":
		var c struct {
			FileKey string `json:"file_key"`
		}
		_ = json.Unmarshal([]byte(content), &c)
		return "", "voice", []model.MediaKey{{Kind: "audio", Key: c.FileKey}}

	case "media":
		var c struct {
			FileKey string `json:"file_key"`
		}
		_ = json.Unmarshal([]byte(content), &c)
		return "", "video", []model.MediaKey{{Kind: "media", Key: c.FileKey}}

	default:
		return "[不支持的消息类型: " + msgType + "]", "text", nil
	}
}

// extractPostText traverses a post message content tree and concatenates all
// text/a element text. Post content:
//
//	{"zh_cn":{"title":"...","content":[[{"tag":"text","text":"..."}, ...], ...]}}
func extractPostText(content string) string {
	var post struct {
		ZhCn postBody `json:"zh_cn"`
		EnUs postBody `json:"en_us"`
	}
	if err := json.Unmarshal([]byte(content), &post); err != nil {
		return ""
	}
	body := post.ZhCn
	if len(body.Content) == 0 {
		body = post.EnUs
	}
	var b strings.Builder
	if body.Title != "" {
		b.WriteString(body.Title)
		b.WriteString("\n")
	}
	for _, line := range body.Content {
		for _, elem := range line {
			tag, _ := elem["tag"].(string)
			if tag == "text" || tag == "a" {
				if t, ok := elem["text"].(string); ok {
					b.WriteString(t)
				}
			}
		}
		b.WriteString("\n")
	}
	return strings.TrimSpace(b.String())
}

type postBody struct {
	Title   string              `json:"title"`
	Content [][]map[string]any `json:"content"`
}
