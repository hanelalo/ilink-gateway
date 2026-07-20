package config

import (
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// DmPolicy controls who can reach the gateway via direct message.
type DmPolicy int

const (
	DMDisabled DmPolicy = iota
	DMPairing   // currently equivalent to DMAllowlist (pairing flow not implemented)
	DMAllowlist
	DMOpen
)

func (p DmPolicy) Allow(userID string, allowList map[string]struct{}) bool {
	switch p {
	case DMDisabled:
		return false
	case DMPairing, DMAllowlist:
		_, ok := allowList[userID]
		return ok
	case DMOpen:
		return true
	}
	return false
}

func parseDmPolicy(s string) DmPolicy {
	switch strings.ToLower(strings.TrimSpace(s)) {
	case "disabled":
		return DMDisabled
	case "pairing":
		return DMPairing
	case "allowlist":
		return DMAllowlist
	case "open", "":
		return DMOpen
	default:
		return DMOpen // bogus → Open, matches Rust
	}
}

// GroupPolicy controls which group chats the gateway serves.
type GroupPolicy int

const (
	GroupDisabled GroupPolicy = iota
	GroupAll
	GroupAllowlist
)

func (p GroupPolicy) Allow(chatID string, allowList map[string]struct{}) bool {
	switch p {
	case GroupDisabled:
		return false
	case GroupAll:
		return true
	case GroupAllowlist:
		_, ok := allowList[chatID]
		return ok
	}
	return false
}

func parseGroupPolicy(s string) GroupPolicy {
	switch strings.ToLower(strings.TrimSpace(s)) {
	case "disabled", "":
		return GroupDisabled
	case "all":
		return GroupAll
	case "allowlist":
		return GroupAllowlist
	default:
		return GroupDisabled // bogus → Disabled
	}
}

// Config holds all gateway configuration loaded from environment variables.
type Config struct {
	HTTPAddr          string
	HTTPPort          int
	DBPath            string
	CmdTimeoutSecs    int
	CmdMaxOutputChars int
	DmPolicy          DmPolicy
	GroupPolicy       GroupPolicy
	AllowedUsers      map[string]struct{}
	AllowedGroups     map[string]struct{}

	FeishuAppID     string
	FeishuAppSecret string
	FeishuBaseURL   string
	FeishuBotOpenID string
	MediaCacheDir   string

	HeartbeatCheckIntervalSecs int
	HeartbeatTimeoutSecs      int
	DedupTTLSecs              int
	ReplyQueueDepth           int
	MediaCacheMaxAgeDays      int
	LogLevel                  string
}

// FromEnv loads configuration from GW_* environment variables.
func FromEnv() (Config, error) {
	cfg := Config{
		HTTPAddr:                   "127.0.0.1",
		HTTPPort:                   8765,
		DBPath:                     "~/.feishu-gateway/data.db",
		CmdTimeoutSecs:             30,
		CmdMaxOutputChars:          2000,
		DmPolicy:                   DMOpen,
		GroupPolicy:                GroupDisabled,
		FeishuBaseURL:              "https://open.feishu.cn",
		MediaCacheDir:              "~/.feishu-gateway/media",
		HeartbeatCheckIntervalSecs: 30,
		HeartbeatTimeoutSecs:       60,
		DedupTTLSecs:               300,
		ReplyQueueDepth:            256,
		MediaCacheMaxAgeDays:       7,
		LogLevel:                   "info",
		AllowedUsers:               map[string]struct{}{},
		AllowedGroups:              map[string]struct{}{},
	}

	if v, ok := os.LookupEnv("GW_HTTP_ADDR"); ok {
		cfg.HTTPAddr = v
	}
	cfg.HTTPPort = envInt("GW_HTTP_PORT", cfg.HTTPPort)
	if v, ok := os.LookupEnv("GW_DB_PATH"); ok {
		cfg.DBPath = v
	}
	cfg.CmdTimeoutSecs = envInt("GW_CMD_TIMEOUT", cfg.CmdTimeoutSecs)
	cfg.CmdMaxOutputChars = envInt("GW_CMD_MAX_OUTPUT", cfg.CmdMaxOutputChars)
	if v, ok := os.LookupEnv("GW_DM_POLICY"); ok {
		cfg.DmPolicy = parseDmPolicy(v)
	}
	if v, ok := os.LookupEnv("GW_GROUP_POLICY"); ok {
		cfg.GroupPolicy = parseGroupPolicy(v)
	}
	if v, ok := os.LookupEnv("GW_ALLOWED_USERS"); ok {
		cfg.AllowedUsers = ParseAllowedList(v)
	}
	if v, ok := os.LookupEnv("GW_ALLOWED_GROUPS"); ok {
		cfg.AllowedGroups = ParseAllowedList(v)
	}

	cfg.FeishuAppID = os.Getenv("GW_FEISHU_APP_ID")
	cfg.FeishuAppSecret = os.Getenv("GW_FEISHU_APP_SECRET")
	if v, ok := os.LookupEnv("GW_FEISHU_BASE_URL"); ok {
		cfg.FeishuBaseURL = v
	}
	cfg.FeishuBotOpenID = os.Getenv("GW_FEISHU_BOT_OPEN_ID")
	if v, ok := os.LookupEnv("GW_MEDIA_CACHE_DIR"); ok {
		cfg.MediaCacheDir = v
	}
	cfg.HeartbeatCheckIntervalSecs = envInt("GW_HEARTBEAT_CHECK_INTERVAL", cfg.HeartbeatCheckIntervalSecs)
	cfg.HeartbeatTimeoutSecs = envInt("GW_HEARTBEAT_TIMEOUT", cfg.HeartbeatTimeoutSecs)
	cfg.DedupTTLSecs = envInt("GW_DEDUP_TTL", cfg.DedupTTLSecs)
	cfg.ReplyQueueDepth = envInt("GW_REPLY_QUEUE_DEPTH", cfg.ReplyQueueDepth)
	cfg.MediaCacheMaxAgeDays = envInt("GW_MEDIA_CACHE_MAX_AGE_DAYS", cfg.MediaCacheMaxAgeDays)
	if v, ok := os.LookupEnv("GW_LOG_LEVEL"); ok {
		cfg.LogLevel = v
	}

	if cfg.FeishuAppID == "" {
		return cfg, errors.New("GW_FEISHU_APP_ID is required")
	}
	if cfg.FeishuAppSecret == "" {
		return cfg, errors.New("GW_FEISHU_APP_SECRET is required")
	}

	cfg.DBPath = expandTilde(cfg.DBPath)
	cfg.MediaCacheDir = expandTilde(cfg.MediaCacheDir)

	return cfg, nil
}

func envInt(key string, fallback int) int {
	if v, ok := os.LookupEnv(key); ok {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
	}
	return fallback
}

// ParseAllowedList splits a comma-separated string into a set, trimming
// whitespace and dropping empty entries.
func ParseAllowedList(s string) map[string]struct{} {
	m := map[string]struct{}{}
	for _, t := range strings.Split(s, ",") {
		t = strings.TrimSpace(t)
		if t != "" {
			m[t] = struct{}{}
		}
	}
	return m
}

func expandTilde(path string) string {
	if strings.HasPrefix(path, "~") {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, path[1:])
		}
	}
	return path
}
