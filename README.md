# wechat-gateway

A WeChat iLink message gateway — multiplex one WeChat connection to multiple AI agents.

## Background

The WeChat iLink Bot API has an exclusive-access constraint: only one long-poll connection can exist per WeChat account at a time. When you have multiple AI agents (Hermes, OpenClaw, Claude Code, etc.) that all need WeChat access, you need a proxy gateway to maintain the WeChat connection and distribute messages.

## Architecture

```
WeChat ←── iLink Protocol ──→ wechat-gateway (Rust)
                                    │
                           ┌────────┴────────┐
                           │  Agent Router    │
                           │  /cmd Executor   │
                           └────────┬────────┘
                                    │
                ┌───────────────────┼───────────────────┐
                │                   │                   │
            Hermes              OpenClaw            ...(agent)
         (HTTP polling)       (HTTP polling)       (HTTP polling)
```

**Core principles:**

- **Gateway-centric** — maintains the exclusive iLink long-poll connection
- **Agent self-registration** — agents register at startup via HTTP with a name
- **`/use <name>` switching** — switch active agent via WeChat messages
- **`/cmd <shell>` execution** — execute shell commands directly from WeChat

## Repository Structure

```
wechat-gateway/
├── gateway/                 # Gateway core (Rust crate)
│   └── src/
│       ├── ilink/           # iLink protocol implementation
│       │   ├── types.rs     # iLink type definitions (serde)
│       │   └── client.rs    # HTTP client (QR login / long-poll / send message)
│       ├── agents/
│       │   ├── registry.rs  # Agent registry
│       │   └── queue.rs     # Message queue
│       ├── router/
│       │   ├── router.rs    # Message router
│       │   └── commands.rs  # Command parser (/use, /list, /status, /cmd)
│       ├── api/server.rs    # HTTP API (axum)
│       ├── storage/         # SQLite credential persistence
│       └── config.rs
│
├── client/hermes/           # Hermes ACP client (Rust crate)
│   └── src/
│       ├── gateway/api.rs   # Gateway API client
│       ├── acp/client.rs    # Hermes ACP JSON-RPC communication
│       └── client.rs        # Main orchestrator loop
│
└── docs/
```

### Message Flow

```
WeChat → long-poll getupdates → Router.handle_incoming()
  ├── is command (/use, /list, /status, /cmd)
  │     → handle built-in, reply directly to WeChat
  └── is normal message
        → enqueue to active agent's message queue
        → agent pulls via GET /api/agents/{name}/poll
        → agent processes, then POST /api/agents/{name}/reply
        → gateway sends reply via sendmessage back to WeChat
```

### Built-in Commands

| Command | Usage |
|---------|-------|
| `/use <name>` | Switch to a specific agent |
| `/list` | List registered agents |
| `/status` | View connection and agent status |
| `/cmd <shell>` | Execute a shell command (supports `timeout <secs>` prefix) |

## Quick Start

### Prerequisites

- Rust 1.75+
- A WeChat account (for QR code login)

### Build

```bash
cd wechat-gateway

# Use proxy if needed (e.g. behind GFW)
export HTTP_PROXY=http://127.0.0.1:7897
export HTTPS_PROXY=http://127.0.0.1:7897

# Build
cargo build --release
```

### Run Tests

```bash
# Run all tests across the workspace
cargo test

# Run gateway tests only
cargo test -p wechat-gateway

# Run Hermes client tests only
cargo test -p wechat-gateway-client-hermes
```

> **Note**: The `main.rs` entry point is not yet implemented. All modules have complete unit test coverage.

### Registering an Agent

```bash
curl -X POST http://127.0.0.1:8765/api/agents/register \
  -H 'Content-Type: application/json' \
  -d '{"name": "my-agent", "capabilities": ["text"]}'
```

### Polling & Replying

```bash
# Poll for pending messages
curl http://127.0.0.1:8765/api/agents/my-agent/poll

# Send a reply
curl -X POST http://127.0.0.1:8765/api/agents/my-agent/reply \
  -H 'Content-Type: application/json' \
  -d '{"reply_to_id": "msg_id", "text": "Hello from my agent!"}'
```

### Gateway Status

```bash
curl http://127.0.0.1:8765/api/status
```

## Configuration

The gateway is configured via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `GW_HTTP_ADDR` | `127.0.0.1` | HTTP API bind address |
| `GW_HTTP_PORT` | `8765` | HTTP API port |
| `GW_ILINK_BASE_URL` | `https://ilinkai.weixin.qq.com` | iLink API base URL |
| `GW_DB_PATH` | `~/.wechat-gateway/data.db` | SQLite database path |
| `GW_CMD_TIMEOUT` | `30` | `/cmd` default timeout (seconds) |
| `GW_CMD_MAX_OUTPUT` | `2000` | `/cmd` max output characters |
| `GW_GATEWAY_URL` | `http://127.0.0.1:8765` | Hermes client gateway URL |
| `GW_AGENT_NAME` | `hermes` | Hermes registration name |
| `GW_HERMES_BIN` | `hermes` | Hermes executable path |
| `GW_HERMES_CWD` | current dir | ACP session working directory |
| `GW_POLL_INTERVAL` | `1` | Poll interval (seconds) |
| `GW_ACP_TIMEOUT` | `300` | ACP session timeout (seconds) |

## iLink Protocol Overview

iLink is Tencent's official WeChat Bot API (opened in 2026), pure HTTP/JSON. Key endpoints:

| Endpoint | Function |
|----------|----------|
| `GET /ilink/bot/get_bot_qrcode` | Get QR code for login |
| `GET /ilink/bot/get_qrcode_status` | Poll QR scan status |
| `POST /ilink/bot/getupdates` | Long-poll receive messages (35s hold) |
| `POST /ilink/bot/sendmessage` | Send a message |
| `POST /ilink/bot/sendtyping` | Send typing indicator |
| `POST /ilink/bot/getconfig` | Get typing ticket |
| `POST /ilink/bot/msg/notifystart` | Enable outbound message capability |

Each request requires the `X-WECHAT-UIN` header (random uint32 → decimal string → base64, regenerated per request) and `AuthorizationType: ilink_bot_token`.

iLink tokens are valid for 24 hours. `errcode: -14` in `getupdates` response indicates expiry and triggers QR re-login.

## License

MIT
