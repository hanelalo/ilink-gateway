package config

import (
	"os"
	"testing"
)

// clearEnv removes all GW_* environment variables so each test starts clean.
func clearEnv(t *testing.T) {
	t.Helper()
	for _, e := range os.Environ() {
		if len(e) >= 3 && e[:3] == "GW_" {
			kv := splitEnv(e)
			t.Setenv(kv.key, "")      // clear via empty

			// unset truly
			os.Unsetenv(kv.key)
		}
	}
}

type envKV struct{ key, value string }

func splitEnv(e string) envKV {
	for i := 0; i < len(e); i++ {
		if e[i] == '=' {
			return envKV{key: e[:i], value: e[i+1:]}
		}
	}
	return envKV{key: e}
}

func TestDefaults(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")

	cfg, err := FromEnv()
	if err != nil {
		t.Fatalf("FromEnv: %v", err)
	}

	if cfg.HTTPAddr != "127.0.0.1" {
		t.Errorf("HTTPAddr = %q, want 127.0.0.1", cfg.HTTPAddr)
	}
	if cfg.HTTPPort != 8765 {
		t.Errorf("HTTPPort = %d, want 8765", cfg.HTTPPort)
	}
	if cfg.CmdTimeoutSecs != 30 {
		t.Errorf("CmdTimeoutSecs = %d, want 30", cfg.CmdTimeoutSecs)
	}
	if cfg.CmdMaxOutputChars != 2000 {
		t.Errorf("CmdMaxOutputChars = %d, want 2000", cfg.CmdMaxOutputChars)
	}
	if cfg.DmPolicy != DMOpen {
		t.Errorf("DmPolicy = %v, want DMOpen", cfg.DmPolicy)
	}
	if cfg.GroupPolicy != GroupDisabled {
		t.Errorf("GroupPolicy = %v, want GroupDisabled", cfg.GroupPolicy)
	}
	if cfg.FeishuBaseURL != "https://open.feishu.cn" {
		t.Errorf("FeishuBaseURL = %q, want https://open.feishu.cn", cfg.FeishuBaseURL)
	}
	if cfg.HeartbeatCheckIntervalSecs != 30 {
		t.Errorf("HeartbeatCheckIntervalSecs = %d, want 30", cfg.HeartbeatCheckIntervalSecs)
	}
	if cfg.HeartbeatTimeoutSecs != 60 {
		t.Errorf("HeartbeatTimeoutSecs = %d, want 60", cfg.HeartbeatTimeoutSecs)
	}
	if cfg.DedupTTLSecs != 300 {
		t.Errorf("DedupTTLSecs = %d, want 300", cfg.DedupTTLSecs)
	}
	if cfg.ReplyQueueDepth != 256 {
		t.Errorf("ReplyQueueDepth = %d, want 256", cfg.ReplyQueueDepth)
	}
	if cfg.MediaCacheMaxAgeDays != 7 {
		t.Errorf("MediaCacheMaxAgeDays = %d, want 7", cfg.MediaCacheMaxAgeDays)
	}
	if cfg.LogLevel != "info" {
		t.Errorf("LogLevel = %q, want info", cfg.LogLevel)
	}
}

func TestCustomPort(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")
	t.Setenv("GW_HTTP_PORT", "9999")

	cfg, err := FromEnv()
	if err != nil {
		t.Fatalf("FromEnv: %v", err)
	}
	if cfg.HTTPPort != 9999 {
		t.Errorf("HTTPPort = %d, want 9999", cfg.HTTPPort)
	}
}

func TestDmPolicyParsing(t *testing.T) {
	tests := []struct {
		input string
		want  DmPolicy
	}{
		{"disabled", DMDisabled},
		{"DISABLED", DMDisabled},
		{"pairing", DMPairing},
		{"allowlist", DMAllowlist},
		{"open", DMOpen},
		{"", DMOpen}, // empty → open
		{"bogus", DMOpen}, // unknown → open
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			got := parseDmPolicy(tt.input)
			if got != tt.want {
				t.Errorf("parseDmPolicy(%q) = %v, want %v", tt.input, got, tt.want)
			}
		})
	}
}

func TestDmPolicyFromEnv(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")
	t.Setenv("GW_DM_POLICY", "disabled")

	cfg, err := FromEnv()
	if err != nil {
		t.Fatalf("FromEnv: %v", err)
	}
	if cfg.DmPolicy != DMDisabled {
		t.Errorf("DmPolicy = %v, want DMDisabled", cfg.DmPolicy)
	}
}

func TestGroupPolicyParsing(t *testing.T) {
	tests := []struct {
		input string
		want  GroupPolicy
	}{
		{"disabled", GroupDisabled},
		{"DISABLED", GroupDisabled},
		{"all", GroupAll},
		{"allowlist", GroupAllowlist},
		{"", GroupDisabled}, // empty → disabled
		{"bogus", GroupDisabled}, // unknown → disabled
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			got := parseGroupPolicy(tt.input)
			if got != tt.want {
				t.Errorf("parseGroupPolicy(%q) = %v, want %v", tt.input, got, tt.want)
			}
		})
	}
}

func TestGroupPolicyFromEnv(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")
	t.Setenv("GW_GROUP_POLICY", "all")

	cfg, err := FromEnv()
	if err != nil {
		t.Fatalf("FromEnv: %v", err)
	}
	if cfg.GroupPolicy != GroupAll {
		t.Errorf("GroupPolicy = %v, want GroupAll", cfg.GroupPolicy)
	}
}

func TestParseAllowedList(t *testing.T) {
	tests := []struct {
		input string
		want  map[string]struct{}
	}{
		{"", map[string]struct{}{}},
		{"user1", map[string]struct{}{"user1": {}}},
		{"user1,user2", map[string]struct{}{"user1": {}, "user2": {}}},
		{" user1 , user2 ", map[string]struct{}{"user1": {}, "user2": {}}},
		{"user1,,user2", map[string]struct{}{"user1": {}, "user2": {}}},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			got := ParseAllowedList(tt.input)
			if len(got) != len(tt.want) {
				t.Fatalf("ParseAllowedList(%q) = %v, want %v", tt.input, got, tt.want)
			}
			for k := range tt.want {
				if _, ok := got[k]; !ok {
					t.Errorf("missing key %q in result", k)
				}
			}
		})
	}
}

func TestAllowedUsersFromEnv(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")
	t.Setenv("GW_ALLOWED_USERS", "alice,bob,charlie")

	cfg, err := FromEnv()
	if err != nil {
		t.Fatalf("FromEnv: %v", err)
	}
	if len(cfg.AllowedUsers) != 3 {
		t.Fatalf("AllowedUsers len = %d, want 3", len(cfg.AllowedUsers))
	}
	for _, u := range []string{"alice", "bob", "charlie"} {
		if _, ok := cfg.AllowedUsers[u]; !ok {
			t.Errorf("AllowedUsers missing %q", u)
		}
	}
}

func TestMissingAppID(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")
	// GW_FEISHU_APP_ID intentionally not set

	_, err := FromEnv()
	if err == nil {
		t.Fatal("expected error when GW_FEISHU_APP_ID is missing")
	}
}

func TestMissingAppSecret(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	// GW_FEISHU_APP_SECRET intentionally not set

	_, err := FromEnv()
	if err == nil {
		t.Fatal("expected error when GW_FEISHU_APP_SECRET is missing")
	}
}

func TestHTTPAddrFromEnv(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")
	t.Setenv("GW_HTTP_ADDR", "0.0.0.0")

	cfg, err := FromEnv()
	if err != nil {
		t.Fatalf("FromEnv: %v", err)
	}
	if cfg.HTTPAddr != "0.0.0.0" {
		t.Errorf("HTTPAddr = %q, want 0.0.0.0", cfg.HTTPAddr)
	}
}

func TestInvalidPortReturnsDefault(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")
	t.Setenv("GW_HTTP_PORT", "not-a-number")

	cfg, err := FromEnv()
	if err != nil {
		t.Fatalf("FromEnv: %v", err)
	}
	if cfg.HTTPPort != 8765 {
		t.Errorf("HTTPPort = %d, want default 8765", cfg.HTTPPort)
	}
}

func TestTildeExpansionInDBPath(t *testing.T) {
	clearEnv(t)
	t.Setenv("GW_FEISHU_APP_ID", "cli_appid")
	t.Setenv("GW_FEISHU_APP_SECRET", "secret")

	cfg, err := FromEnv()
	if err != nil {
		t.Fatalf("FromEnv: %v", err)
	}
	// The default DBPath contains a tilde — it should be expanded.
	if cfg.DBPath[0] == '~' {
		t.Errorf("DBPath = %q, expected tilde expansion", cfg.DBPath)
	}
	if cfg.DBPath == "" {
		t.Error("DBPath should not be empty")
	}
}

func TestDmPolicyAllow(t *testing.T) {
	allowList := map[string]struct{}{"alice": {}}

	tests := []struct {
		name   string
		policy DmPolicy
		user   string
		want   bool
	}{
		{"disabled_allows_nobody", DMDisabled, "alice", false},
		{"disabled_rejects_unknown", DMDisabled, "bob", false},
		{"pairing_allows_allowlisted", DMPairing, "alice", true},
		{"pairing_rejects_unknown", DMPairing, "bob", false},
		{"allowlist_allows_allowlisted", DMAllowlist, "alice", true},
		{"allowlist_rejects_unknown", DMAllowlist, "bob", false},
		{"open_allows_anyone", DMOpen, "bob", true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := tt.policy.Allow(tt.user, allowList)
			if got != tt.want {
				t.Errorf("Allow(%q) = %v, want %v", tt.user, got, tt.want)
			}
		})
	}
}

func TestGroupPolicyAllow(t *testing.T) {
	allowList := map[string]struct{}{"chat-ops": {}}

	tests := []struct {
		name   string
		policy GroupPolicy
		chatID string
		want   bool
	}{
		{"disabled_allows_nobody", GroupDisabled, "chat-ops", false},
		{"disabled_rejects_unknown", GroupDisabled, "chat-random", false},
		{"all_allows_anyone", GroupAll, "chat-random", true},
		{"allowlist_allows_allowlisted", GroupAllowlist, "chat-ops", true},
		{"allowlist_rejects_unknown", GroupAllowlist, "chat-random", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := tt.policy.Allow(tt.chatID, allowList)
			if got != tt.want {
				t.Errorf("Allow(%q) = %v, want %v", tt.chatID, got, tt.want)
			}
		})
	}
}
