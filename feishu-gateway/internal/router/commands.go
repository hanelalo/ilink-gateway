package router

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
	"time"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

// ErrCommandTimeout is returned when ExecuteCommand exceeds its timeout.
var ErrCommandTimeout = errors.New("command timed out")

// dangerousPatterns are forbidden substrings, matched case-insensitively.
var dangerousPatterns = []string{
	"rm -rf /",
	"rm -rf /*",
	"rm -rf ~",
	"shutdown",
	"reboot",
	"> /dev/sda",
	"mkfs",
	"dd if=",
	":{():|:&};:",
	":(){ :|:& };:",
}

// IsDangerousCommand reports whether cmd contains a forbidden pattern.
func IsDangerousCommand(cmd string) bool {
	lower := strings.ToLower(cmd)
	for _, p := range dangerousPatterns {
		if strings.Contains(lower, p) {
			return true
		}
	}
	return false
}

// ParseCommand parses a "/"-prefixed command. Returns ok=false for non-commands
// and unrecognized /xxx — the caller treats unrecognized commands as ordinary
// messages forwarded to the active agent.
func ParseCommand(text string, defaultTimeout int) (model.RouterCommand, bool) {
	text = strings.TrimSpace(text)
	if !strings.HasPrefix(text, "/") {
		return model.RouterCommand{}, false
	}
	body := text[1:]
	end := strings.IndexAny(body, " \t")
	var name, rest string
	if end < 0 {
		name = body
	} else {
		name = body[:end]
		rest = strings.TrimLeft(body[end:], " \t")
	}
	switch name {
	case "use":
		target := strings.TrimSpace(rest)
		if target == "" {
			return model.RouterCommand{}, false
		}
		return model.RouterCommand{Kind: model.CmdUseAgent, AgentName: target}, true
	case "list":
		return model.RouterCommand{Kind: model.CmdListAgents}, true
	case "status":
		return model.RouterCommand{Kind: model.CmdStatus}, true
	case "gateway-help", "help":
		return model.RouterCommand{Kind: model.CmdHelp}, true
	case "cmd":
		shell := rest
		timeout := defaultTimeout
		if strings.HasPrefix(shell, "timeout ") {
			t := strings.TrimPrefix(shell, "timeout ")
			spaceIdx := strings.IndexAny(t, " \t")
			if spaceIdx < 0 {
				// "timeout N" with no shell following it
				return model.RouterCommand{}, false
			}
			if n, err := strconv.Atoi(t[:spaceIdx]); err == nil && n > 0 {
				timeout = n
			}
			shell = strings.TrimLeft(t[spaceIdx:], " \t")
		}
		if strings.TrimSpace(shell) == "" {
			return model.RouterCommand{}, false
		}
		return model.RouterCommand{Kind: model.CmdShell, Shell: shell, TimeoutSecs: timeout}, true
	default:
		return model.RouterCommand{}, false
	}
}

// ExecuteCommand runs cmd via "sh -c", capturing combined stdout+stderr,
// truncated to maxChars. Returns ErrCommandTimeout on timeout.
func ExecuteCommand(ctx context.Context, cmd string, timeoutSecs, maxChars int) (string, error) {
	if timeoutSecs <= 0 {
		timeoutSecs = 30
	}
	ctx, cancel := context.WithTimeout(ctx, time.Duration(timeoutSecs)*time.Second)
	defer cancel()

	var combined bytes.Buffer
	c := exec.CommandContext(ctx, "sh", "-c", cmd)
	c.Stdout = &combined
	c.Stderr = &combined
	err := c.Run()

	if ctx.Err() == context.DeadlineExceeded {
		return "", ErrCommandTimeout
	}

	out := combined.String()
	if maxChars > 0 && len(out) > maxChars {
		out = out[:maxChars] + "... (truncated)"
	}
	if err != nil && out == "" {
		return "", fmt.Errorf("command failed: %w", err)
	}
	return out, nil
}
