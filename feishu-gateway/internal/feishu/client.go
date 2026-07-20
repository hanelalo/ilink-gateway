package feishu

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"

	lark "github.com/larksuite/oapi-sdk-go/v3"
	larkim "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/breaker"
)

// ErrCircuitOpen is returned when the breaker is open and calls are rejected.
var ErrCircuitOpen = errors.New("circuit breaker open")

// Client wraps the Feishu lark.Client with breaker-guarded send/reply/upload/download.
// Token lifecycle (tenant_access_token refresh) is handled by the SDK itself.
type Client struct {
	lark    *lark.Client
	breaker *breaker.Breaker
}

// NewClient constructs a Feishu client. baseURL selects Feishu vs LarkSuite.
func NewClient(appID, appSecret, baseURL string, brk *breaker.Breaker) *Client {
	var opts []lark.ClientOptionFunc
	if baseURL == lark.LarkBaseUrl {
		opts = append(opts, lark.WithOpenBaseUrl(lark.LarkBaseUrl))
	} else if baseURL != "" && baseURL != lark.FeishuBaseUrl {
		opts = append(opts, lark.WithOpenBaseUrl(baseURL))
	}
	return &Client{lark: lark.NewClient(appID, appSecret, opts...), breaker: brk}
}

func (c *Client) gate() error {
	if c.breaker != nil && c.breaker.IsOpen() {
		return ErrCircuitOpen
	}
	return nil
}

func (c *Client) mark(ok bool) {
	if c.breaker == nil {
		return
	}
	if ok {
		c.breaker.RecordSuccess()
	} else {
		c.breaker.RecordFailure()
	}
}

// SendText is a shortcut for SendMessage with msg_type=text.
func (c *Client) SendText(ctx context.Context, receiveIDType, receiveID, text string) (string, error) {
	content, _ := json.Marshal(map[string]string{"text": text})
	return c.SendMessage(ctx, receiveIDType, receiveID, "text", string(content))
}

// SendMessage sends a message. content MUST be a JSON string (feishu contract).
// Returns the new message_id on success.
func (c *Client) SendMessage(ctx context.Context, receiveIDType, receiveID, msgType, content string) (string, error) {
	if err := c.gate(); err != nil {
		return "", err
	}
	req := larkim.NewCreateMessageReqBuilder().
		ReceiveIdType(receiveIDType).
		Body(larkim.NewCreateMessageReqBodyBuilder().
			ReceiveId(receiveID).
			MsgType(msgType).
			Content(content).
			Build()).
		Build()
	resp, err := c.lark.Im.V1.Message.Create(ctx, req)
	if err != nil {
		c.mark(false)
		return "", err
	}
	if !resp.Success() {
		c.mark(false)
		return "", fmt.Errorf("feishu send: code=%d msg=%s", resp.Code, resp.Msg)
	}
	c.mark(true)
	if resp.Data != nil && resp.Data.MessageId != nil {
		return *resp.Data.MessageId, nil
	}
	return "", nil
}

// ReplyMessage replies to an existing message. Returns the new message_id.
func (c *Client) ReplyMessage(ctx context.Context, messageID, msgType, content string) (string, error) {
	if err := c.gate(); err != nil {
		return "", err
	}
	req := larkim.NewReplyMessageReqBuilder().
		MessageId(messageID).
		Body(larkim.NewReplyMessageReqBodyBuilder().
			MsgType(msgType).
			Content(content).
			Build()).
		Build()
	resp, err := c.lark.Im.V1.Message.Reply(ctx, req)
	if err != nil {
		c.mark(false)
		return "", err
	}
	if !resp.Success() {
		c.mark(false)
		return "", fmt.Errorf("feishu reply: code=%d msg=%s", resp.Code, resp.Msg)
	}
	c.mark(true)
	if resp.Data != nil && resp.Data.MessageId != nil {
		return *resp.Data.MessageId, nil
	}
	return "", nil
}

// UploadImage uploads an image file and returns the image_key.
func (c *Client) UploadImage(ctx context.Context, path string) (string, error) {
	if err := c.gate(); err != nil {
		return "", err
	}
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	req := larkim.NewCreateImageReqBuilder().
		Body(larkim.NewCreateImageReqBodyBuilder().
			ImageType("message").
			Image(f).
			Build()).
		Build()
	resp, err := c.lark.Im.V1.Image.Create(ctx, req)
	if err != nil {
		c.mark(false)
		return "", err
	}
	if !resp.Success() {
		c.mark(false)
		return "", fmt.Errorf("feishu upload image: code=%d msg=%s", resp.Code, resp.Msg)
	}
	c.mark(true)
	if resp.Data != nil && resp.Data.ImageKey != nil {
		return *resp.Data.ImageKey, nil
	}
	return "", nil
}

// UploadFile uploads a non-image file and returns the file_key.
func (c *Client) UploadFile(ctx context.Context, path string) (string, error) {
	if err := c.gate(); err != nil {
		return "", err
	}
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	fileType := FileTypeForPath(path)
	req := larkim.NewCreateFileReqBuilder().
		Body(larkim.NewCreateFileReqBodyBuilder().
			FileType(fileType).
			FileName(filepath.Base(path)).
			File(f).
			Build()).
		Build()
	resp, err := c.lark.Im.V1.File.Create(ctx, req)
	if err != nil {
		c.mark(false)
		return "", err
	}
	if !resp.Success() {
		c.mark(false)
		return "", fmt.Errorf("feishu upload file: code=%d msg=%s", resp.Code, resp.Msg)
	}
	c.mark(true)
	if resp.Data != nil && resp.Data.FileKey != nil {
		return *resp.Data.FileKey, nil
	}
	return "", nil
}

// DownloadResource downloads a message resource (image/file) to destPath.
// resourceType is "image" or "file".
func (c *Client) DownloadResource(ctx context.Context, messageID, fileKey, resourceType, destPath string) error {
	if err := c.gate(); err != nil {
		return err
	}
	req := larkim.NewGetMessageResourceReqBuilder().
		MessageId(messageID).
		FileKey(fileKey).
		Type(resourceType).
		Build()
	resp, err := c.lark.Im.V1.MessageResource.Get(ctx, req)
	if err != nil {
		c.mark(false)
		return err
	}
	if !resp.Success() {
		c.mark(false)
		return fmt.Errorf("feishu download: code=%d msg=%s", resp.Code, resp.Msg)
	}
	c.mark(true)
	if err := os.MkdirAll(filepath.Dir(destPath), 0o755); err != nil {
		return err
	}
	out, err := os.Create(destPath)
	if err != nil {
		return err
	}
	defer out.Close()
	if resp.File == nil {
		return errors.New("feishu download: empty response body")
	}
	_, err = io.Copy(out, resp.File)
	return err
}
