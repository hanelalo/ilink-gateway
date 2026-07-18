# Claude Code Adapter

A WeChat-connected Claude Code session manager. Registers as an agent with wechat-gateway, receives WeChat messages, and forwards them to Claude Code via `@anthropic-ai/claude-agent-sdk`. Supports multi-user, multi-workspace management.

**Status**: Complete with 189 unit tests across all modules.

## Prerequisites

- Node.js >= 18
- [Claude Code](https://claude.ai/code) CLI (`npm install -g @anthropic-ai/claude-code`)
- Claude API Key (configured in Claude Code)
- [wechat-gateway](https://github.com/hanelalo/wechat-gateway) running

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CLAUDE_GATEWAY_URL` | `http://127.0.0.1:8765` | Gateway HTTP API URL |
| `CLAUDE_GATEWAY_AGENT_NAME` | `claude` | Agent name for registration |
| `CLAUDE_MODEL` | `sonnet` | Claude model |
| `CLAUDE_CWD` | `process.cwd()` | Default working directory |
| `CLAUDE_POLL_INTERVAL` | `1000` | Poll interval (ms) |
| `CLAUDE_EFFORT` | `medium` | Claude Code effort level |
| `CLAUDE_SESSION_STORE_PATH` | `~/.wechat-gateway/claude-sessions.json` | Session persistence path |
| `HTTP_PROXY` / `HTTPS_PROXY` | - | HTTP proxy configuration |

## Quick Start

```bash
# 1. Configure Claude Code (if not already done)
claude

# 2. Start gateway
cd /path/to/wechat-gateway/gateway
cargo run

# 3. Start adapter
cd /path/to/wechat-gateway/client/claude-code-adapter
npx tsx src/index.ts
```

## WeChat Commands

Send these commands from WeChat to control the adapter:

| Command | Description |
|---------|-------------|
| **Workspace Management** | |
| `/cd` | List all workspaces and aliases |
| `/cd <name>` | Switch workspace (supports aliases, path basename, absolute path) |
| `/cd + <alias> [path]` | Add an alias (uses current workspace if path is omitted) |
| `/cd - <alias>` | Remove an alias |
| `/cd close <name>` | Close workspace (abort running query and clear session) |
| **Tool Approval** | |
| `/approve` | Approve current tool call |
| `/deny` | Deny current tool call |
| `/approve session` | Approve and whitelist this tool for the session |
| `/approve on` | Switch to auto-approve mode |
| `/approve off` | Switch back to interactive approval mode |

## Message Routing Flow

```
WeChat message → gateway → poll → adapter receives message
  ├── /cd command → handleCdCommand() → process and reply directly
  ├── /approve|/deny → parse approval command → resolve pendingApproval or toggle mode
  └── other text → find RunningQuery by activeCwd
        ├── already running → enqueue to messageQueue (wait in line)
        └── idle → start Claude Code session
              ├── first time → query({ prompt, cwd }) → save sessionId via onSessionInit
              └── resume → query({ prompt, cwd, resume: sessionId })
```

## Project Structure

```
src/
├── index.ts              # Entry: register, poll, message routing, process lifecycle
├── config.ts             # Environment variable loading
├── gateway-client.ts     # Gateway HTTP API wrapper
├── session-store.ts      # Session persistence (wxid → cwd → sessionId)
├── query-manager.ts      # Runtime query state management
├── claude-session.ts     # Claude SDK query() wrapper
├── cd-command.ts         # /cd command handling (path resolution, alias management)
├── approval.ts           # Approval command parsing + timeout
├── formatter.ts          # Markdown to plain text
├── streaming-batcher.ts  # Stream batching + idle alert + long reply splitting
│
# Test files (.test.ts mirrors each source file)
├── config.test.ts
├── gateway-client.test.ts
├── session-store.test.ts
├── query-manager.test.ts
├── claude-session.test.ts
├── cd-command.test.ts
├── approval.test.ts
├── formatter.test.ts
├── streaming-batcher.test.ts
├── index.test.ts
```

## Core Features

- **Multi-turn dialogue**: sessionId is persisted to JSON file via `onSessionInit` callback; next message automatically uses `resumeSessionId` for full context
- **Cross-directory parallelism**: Different cwd's RunningQuery are stored independently (`Map<wxid, Map<cwd, RunningQuery>>`); `/cd` switching does not affect running sessions
- **Stream batching**: Text buffered until >1500 chars auto-flush; tool_use flushes buffer first; result flushes remaining; 2s idle auto-flush
- **Idle alert**: 30s no activity sends "Claude is processing..." to WeChat without interrupting the session
- **Long reply splitting**: Replies exceeding 3800 chars are split into `[1/N]`, `[2/N]`... segments sent 500ms apart
- **Tool approval**: Interactive approval (`/approve`/`/deny` in WeChat) + session whitelist + auto mode; 60s timeout auto-denies
- **Security controls**: Permanent blacklist `Bash(sudo:*)`, `Bash(rm -rf:*)`, `Bash(chmod:*)`; first-time sessions pre-whitelist `Read/Glob/Grep`
- **Message queuing**: Subsequent messages for the same wxid + cwd are automatically queued and processed sequentially after the current session ends
- **Graceful shutdown**: On SIGINT/SIGTERM, abort all queries, resolve all pending approvals as deny, exit after 3s
- **Global error handling**: `uncaughtException` / `unhandledRejection` global handlers prevent unexpected process exit
- **Poll self-healing**: Poll errors print warnings and continue; next poll retries automatically; 404 triggers re-registration

## Testing

Uses Vitest. All tests are mocked (SDK and HTTP) — no external services required.

```bash
# Run all tests
npm test

# Watch mode
npm run test:watch

# With coverage
npx vitest run --coverage
```

## Development

```bash
# Install dependencies
npm install

# Run
npx tsx src/index.ts

# Build
npm run build
```
