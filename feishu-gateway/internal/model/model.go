package model

// MediaItem is a media attachment on a message delivered to an agent.
type MediaItem struct {
	MediaType    string  `json:"media_type"`
	LocalPath    string  `json:"local_path"`
	OriginalName *string `json:"original_name,omitempty"`
}

// AgentMessage is the message envelope pushed from gateway to agent via poll.
// Field order MUST match the Rust gateway's poll response exactly.
type AgentMessage struct {
	ID           string      `json:"id"`
	FromUser     string      `json:"from_user"`
	Text         string      `json:"text"`
	Timestamp    int64       `json:"timestamp"`
	ContextToken string      `json:"context_token"`
	MessageType  string      `json:"message_type"`
	Media        []MediaItem `json:"media"` // never omitempty; init to []MediaItem{} not nil
	AgentContext *string     `json:"agent_context,omitempty"`
}

// AgentReply is the body of POST /api/agents/{name}/reply.
type AgentReply struct {
	ReplyToID    string   `json:"reply_to_id"`
	Text         string   `json:"text"`
	MediaPaths   []string `json:"media_paths,omitempty"`
	ToUser       *string  `json:"to_user,omitempty"`
	ContextToken *string  `json:"context_token,omitempty"`
	AgentContext *string  `json:"agent_context,omitempty"`
}

// IsProactive reports whether this reply is a proactive send (no reply_to_id,
// target specified via ToUser) rather than a reply to a known message.
func (r AgentReply) IsProactive() bool {
	return r.ReplyToID == "" && r.ToUser != nil && *r.ToUser != ""
}

// AgentStatus is the online/offline state of a registered agent.
type AgentStatus int

const (
	StatusOffline AgentStatus = iota
	StatusOnline
)

func (s AgentStatus) String() string {
	if s == StatusOnline {
		return "online"
	}
	return "offline"
}

// AgentInfo holds the registry entry for a registered agent.
type AgentInfo struct {
	Name         string
	Endpoint     string
	Capabilities []string
	Status       AgentStatus
	LastSeen     int64 // millis
	RegisteredAt int64 // millis
}

// QueuedMessage is a message waiting to be drained by an agent poll.
type QueuedMessage struct {
	ID           string
	FromUser     string
	Text         string
	Timestamp    int64
	ContextToken string
	MessageType  string
	Media        []MediaItem // always non-nil
	AgentContext *string
}

// ToAgentMessage converts a queued message to its poll-response form, ensuring
// Media is a non-nil slice so json.Marshal emits "media":[] not "media":null.
func (q QueuedMessage) ToAgentMessage() AgentMessage {
	media := q.Media
	if media == nil {
		media = []MediaItem{}
	}
	return AgentMessage{
		ID:           q.ID,
		FromUser:     q.FromUser,
		Text:         q.Text,
		Timestamp:    q.Timestamp,
		ContextToken: q.ContextToken,
		MessageType:  q.MessageType,
		Media:        media,
		AgentContext: q.AgentContext,
	}
}

// RouterCommandKind enumerates the gateway built-in slash commands.
type RouterCommandKind int

const (
	CmdUseAgent RouterCommandKind = iota
	CmdListAgents
	CmdStatus
	CmdShell
	CmdHelp
)

// RouterCommand is the parsed result of a slash command from the IM side.
type RouterCommand struct {
	Kind        RouterCommandKind
	AgentName   string // CmdUseAgent
	Shell       string // CmdShell
	TimeoutSecs int    // CmdShell
}

// MediaKey references a platform resource (feishu image_key / file_key) that
// the gateway must download before delivering to the agent.
type MediaKey struct {
	Kind string // "image" | "file" | "audio" | "media"
	Key  string
}

// IncomingMessage is the platform-agnostic representation of a message
// received from the IM platform. The feishu package converts SDK events into
// this shape so the router has no SDK dependency.
type IncomingMessage struct {
	MessageID     string
	FromUser      string // sender stable id (feishu: open_id)
	ChatID        string // group chat id; empty for p2p
	IsGroup       bool
	Text          string // already cleaned of @-mention placeholders
	MessageType   string // text | image | voice | video | file
	Timestamp     int64
	MediaKeys     []MediaKey
	ReceiveIDType string // "open_id" or "chat_id" — where to send replies
	ReceiveID     string
}
