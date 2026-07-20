# Feishu Gateway

A [Feishu/Lark](https://open.feishu.cn/) message gateway that exposes the same
HTTP API contract as the Rust [`wechat-gateway`](../gateway/) in this repo, so
existing agent clients (`client/claude-code-adapter`, `client/hermes-wechat-plugin`)
can connect with **zero protocol changes** — only the IM backend swaps from
WeChat to Feishu.

## Status

| Component | State |
|-----------|-------|
| HTTP API (register / poll / reply / status) | ✅ Working |
| Agent registry + queue + heartbeat | ✅ Working |
| Router (admission / commands / routing) | ✅ Working |
| Dedup cache + circuit breaker | ✅ Working |
| In-memory state store | ✅ Working |
| Feishu WebSocket receiver | ✅ Working |
| Feishu client (send / upload / download) | ✅ Working |
| Reply processor (real send) | ✅ Working |
| Media download / cache | ✅ Working |
| SQLite persistent store | ⏳ In-memory (pending `modernc.org/sqlite`) |

The HTTP layer is fully functional today: agents can register, poll, reply, and
query status. The Feishu backend (WebSocket event ingestion + message sending)
is the remaining work and requires building against Go ≥ 1.20.

## Architecture

```
feishu-gateway/
├── cmd/feishu-gateway/      # Entry point: wires components, starts HTTP server
└── internal/
    ├── model/               # AgentMessage, AgentReply, MediaItem, IncomingMessage
    ├── config/              # Env loading, DmPolicy / GroupPolicy
    ├── agent/               # Registry (heartbeat) + MessageQueue (FIFO)
    ├── router/              # Admission + slash commands + enqueue
    ├── api/                 # HTTP handlers (register/poll/reply/status)
    ├── storage/             # Store interface + in-memory impl
    ├── dedup/               # TTL dedup cache
    ├── breaker/             # Sliding-window circuit breaker
    ├── feishu/              # Feishu SDK integration (pending Go upgrade)
    └── reply/               # Reply processor (pending Feishu client)
```

**Concurrency model**: each concern runs in its own goroutine — heartbeat
checker, HTTP server, (future) Feishu WS receiver, (future) reply processor.
Shared state uses short, non-nested mutex critical sections to avoid
lock-ordering deadlocks.

## Quick start

### Prerequisites

- Go ≥ 1.19 for the current code; Go ≥ 1.20 once the Feishu SDK is wired in

### Build & run

```bash
make build
GW_FEISHU_APP_ID=cli_xxx GW_FEISHU_APP_SECRET=xxx make run
```

Or directly:

```bash
go build -o bin/feishu-gateway ./cmd/feishu-gateway
GW_FEISHU_APP_ID=cli_xxx GW_FEISHU_APP_SECRET=xxx ./bin/feishu-gateway
```

### Test

```bash
make test          # all packages
make test-verbose  # with per-test output
```

## Configuration

All config is via `GW_*` environment variables.

| Variable | Default | Purpose |
|----------|---------|---------|
| `GW_HTTP_ADDR` | `127.0.0.1` | HTTP API bind address |
| `GW_HTTP_PORT` | `8765` | HTTP API port |
| `GW_DB_PATH` | `~/.feishu-gateway/data.db` | SQLite path (pending Go upgrade) |
| `GW_CMD_TIMEOUT` | `30` | `/cmd` default timeout (seconds) |
| `GW_CMD_MAX_OUTPUT` | `2000` | `/cmd` output truncation (chars) |
| `GW_DM_POLICY` | `open` | DM admission: `disabled` / `pairing` / `allowlist` / `open` |
| `GW_GROUP_POLICY` | `disabled` | Group admission: `disabled` / `all` / `allowlist` |
| `GW_ALLOWED_USERS` | _(empty)_ | Allowed open_id list, comma-separated |
| `GW_ALLOWED_GROUPS` | _(empty)_ | Allowed chat_id list, comma-separated |
| `GW_FEISHU_APP_ID` | _(required)_ | Feishu app ID |
| `GW_FEISHU_APP_SECRET` | _(required)_ | Feishu app secret |
| `GW_FEISHU_BASE_URL` | `https://open.feishu.cn` | LarkSuite int'l: `https://open.larksuite.com` |
| `GW_FEISHU_BOT_OPEN_ID` | _(empty)_ | Bot open_id, enables group @-mention detection |
| `GW_MEDIA_CACHE_DIR` | `~/.feishu-gateway/media` | Media download cache directory |
| `GW_HEARTBEAT_CHECK_INTERVAL` | `30` | Heartbeat check period (seconds) |
| `GW_HEARTBEAT_TIMEOUT` | `60` | Heartbeat timeout threshold (seconds) |
| `GW_DEDUP_TTL` | `300` | Message dedup TTL (seconds) |
| `GW_REPLY_QUEUE_DEPTH` | `256` | Reply channel buffer size |
| `GW_MEDIA_CACHE_MAX_AGE_DAYS` | `7` | Media cache retention (days) |
| `GW_LOG_LEVEL` | `info` | Log level |

## HTTP API contract

Identical to `wechat-gateway`, so agents switch backends by changing only the
gateway URL.

| Method | Path | Body | Response |
|--------|------|------|----------|
| POST | `/api/agents/register` | `{name, capabilities}` | `200 {ok, active_agent}` or `400 {ok:false, error}` |
| GET | `/api/agents/{name}/poll` | — | `200 {messages:[AgentMessage]}` or `404` |
| POST | `/api/agents/{name}/reply` | `{reply_to_id, text, media_paths?, to_user?, context_token?, agent_context?}` | Always `200 {ok:true}` |
| GET | `/api/status` | — | `{feishu:{connected}, active_agent, agents:{...}}` |

**Implicit contract** (must not break):
- `poll` doubles as heartbeat — every call refreshes the agent's `last_seen`
- First registration auto-activates that agent (clients depend on this)
- `reply` always returns `200 {ok:true}`; failures surface only in logs
- `media` field is always `[]` (never `null`, never omitted) — clients type it non-optional
- `poll` returns `404` for unregistered agents → triggers client re-register

### Built-in slash commands

Sent by users in the IM client; handled directly by the gateway:

- `/use <name>` — switch active agent
- `/list` — list registered agents
- `/status` — show gateway status
- `/cmd [timeout N] <shell>` — run a shell command (dangerous commands blocked)
- `/gateway-help` — show help

Unrecognized `/xxx` commands are forwarded to the active agent as ordinary
messages.

## Development

### Reusing the existing client adapters

The whole point is protocol compatibility. To point an existing client at this
gateway:

```bash
# Claude Code adapter
CLAUDE_GATEWAY_URL=http://127.0.0.1:8765 CLAUDE_GATEWAY_AGENT_NAME=claude \
  node dist/index.js

# Hermes plugin (requires a Feishu-flavored fork — see Roadmap)
WECHAT_GATEWAY_URL=http://127.0.0.1:8765 ...
```

### Roadmap (post Go-upgrade)

1. Wire `github.com/larksuite/oapi-sdk-go/v3` WebSocket long connection
2. Feishu client (send message, reply, upload image/file, download resource)
3. Event normalization (type mapping, @-mention cleanup, post rich-text extraction)
4. Real reply processor with resilient send + circuit breaker
5. Async media download + cache
6. SQLite persistent store (`modernc.org/sqlite`)
7. Hermes plugin Feishu fork (decouples `wechat_gateway` platform identity)

See `/Users/hanelalo/.claude/plans/go-gleaming-wolf.md` for the full design.

## Reference

- [Feishu open platform docs](https://open.feishu.cn/document)
- [Feishu integration guide](../docs/feishu.md) (in this repo, vetted)
- [Go SDK](https://github.com/larksuite/oapi-sdk-go)
- Sibling project: [`wechat-gateway`](../gateway/) (Rust)
