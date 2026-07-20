package router

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/hanelalo/wechat-gateway/feishu-gateway/internal/model"
)

func TestParseCommandUse(t *testing.T) {
	cmd, ok := ParseCommand("/use hermes", 30)
	if !ok || cmd.Kind != model.CmdUseAgent || cmd.AgentName != "hermes" {
		t.Errorf("unexpected: %+v ok=%v", cmd, ok)
	}
}

func TestParseCommandUseNoArg(t *testing.T) {
	if _, ok := ParseCommand("/use", 30); ok {
		t.Error("expected ok=false for /use without arg")
	}
}

func TestParseCommandList(t *testing.T) {
	cmd, ok := ParseCommand("/list", 30)
	if !ok || cmd.Kind != model.CmdListAgents {
		t.Errorf("unexpected: %+v", cmd)
	}
}

func TestParseCommandStatus(t *testing.T) {
	cmd, ok := ParseCommand("/status", 30)
	if !ok || cmd.Kind != model.CmdStatus {
		t.Errorf("unexpected: %+v", cmd)
	}
}

func TestParseCommandHelp(t *testing.T) {
	for _, input := range []string{"/gateway-help", "/help"} {
		cmd, ok := ParseCommand(input, 30)
		if !ok || cmd.Kind != model.CmdHelp {
			t.Errorf("for %q: unexpected: %+v", input, cmd)
		}
	}
}

func TestParseCommandCmdSimple(t *testing.T) {
	cmd, ok := ParseCommand("/cmd echo hello", 30)
	if !ok || cmd.Kind != model.CmdShell || cmd.Shell != "echo hello" || cmd.TimeoutSecs != 30 {
		t.Errorf("unexpected: %+v", cmd)
	}
}

func TestParseCommandCmdWithTimeout(t *testing.T) {
	cmd, ok := ParseCommand("/cmd timeout 5 echo hi", 30)
	if !ok || cmd.Shell != "echo hi" || cmd.TimeoutSecs != 5 {
		t.Errorf("unexpected: %+v", cmd)
	}
}

func TestParseCommandCmdPreservesQuoting(t *testing.T) {
	cmd, ok := ParseCommand(`/cmd echo "hello world"`, 30)
	if !ok || cmd.Shell != `echo "hello world"` {
		t.Errorf("quoting lost: %q", cmd.Shell)
	}
}

func TestParseCommandCmdEmpty(t *testing.T) {
	if _, ok := ParseCommand("/cmd", 30); ok {
		t.Error("/cmd with no shell should not parse")
	}
	if _, ok := ParseCommand("/cmd timeout 5", 30); ok {
		t.Error("/cmd timeout 5 with no shell should not parse")
	}
}

func TestParseCommandNotACommand(t *testing.T) {
	if _, ok := ParseCommand("hello world", 30); ok {
		t.Error("plain text is not a command")
	}
}

func TestParseCommandUnrecognized(t *testing.T) {
	// Unrecognized /xxx must return ok=false so caller forwards to agent.
	if _, ok := ParseCommand("/foobar baz", 30); ok {
		t.Error("unrecognized command should return ok=false")
	}
}

func TestParseCommandTrimsWhitespace(t *testing.T) {
	cmd, ok := ParseCommand("  /status  ", 30)
	if !ok || cmd.Kind != model.CmdStatus {
		t.Errorf("unexpected after trim: %+v", cmd)
	}
}

func TestIsDangerousCommand(t *testing.T) {
	dangerous := []string{
		"rm -rf /",
		"RM -RF /",
		"rm -rf /*",
		"rm -rf ~",
		"sudo shutdown -h now",
		"mkfs.ext4 /dev/sda1",
		"dd if=/dev/zero of=/dev/sda",
		":(){ :|:& };:",
	}
	for _, c := range dangerous {
		if !IsDangerousCommand(c) {
			t.Errorf("expected dangerous: %q", c)
		}
	}
}

func TestIsDangerousCommandSafe(t *testing.T) {
	safe := []string{
		"ls -la",
		"echo hello",
		"rm file.txt",
		"docker rm -f container",
		"git status",
	}
	for _, c := range safe {
		if IsDangerousCommand(c) {
			t.Errorf("expected safe: %q", c)
		}
	}
}

func TestExecuteCommandEcho(t *testing.T) {
	out, err := ExecuteCommand(context.Background(), "echo hello", 5, 1000)
	if err != nil {
		t.Fatal(err)
	}
	if strings.TrimSpace(out) != "hello" {
		t.Errorf("unexpected output: %q", out)
	}
}

func TestExecuteCommandCapturesStderr(t *testing.T) {
	out, err := ExecuteCommand(context.Background(), "echo out; echo err 1>&2", 5, 1000)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "out") || !strings.Contains(out, "err") {
		t.Errorf("expected both stdout and stderr, got: %q", out)
	}
}

func TestExecuteCommandTimeout(t *testing.T) {
	start := time.Now()
	_, err := ExecuteCommand(context.Background(), "sleep 10", 1, 1000)
	elapsed := time.Since(start)
	if err == nil {
		t.Fatal("expected timeout error")
	}
	if elapsed > 3*time.Second {
		t.Errorf("took too long: %v", elapsed)
	}
}

func TestExecuteCommandTruncate(t *testing.T) {
	out, err := ExecuteCommand(context.Background(), "seq 1 500", 5, 50)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "(truncated)") {
		t.Errorf("expected truncation marker, got: %q", out)
	}
}
