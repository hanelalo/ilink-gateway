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
                           ┌────────┴────────┐
                           │ Hermes Plugin    │
                           │ (Python, poll)   │
                           └────────┬────────┘
                                    │
                               Hermes Agent
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
│       │   ├── client.rs    # HTTP client (QR login / long-poll / send message / media upload)
│       │   ├── media.rs     # AES-128-ECB encryption/decryption + CDN URL validation
│       │   └── download.rs  # CDN media download with SSRF protection
│       ├── agents/
│       │   ├── registry.rs  # Agent registry with heartbeat tracking
│       │   ├── queue.rs     # Message queue
│       │   └── ws_registry.rs # WebSocket connection registry
│       ├── router/
│       │   ├── router.rs    # Message router (media-aware)
│       │   └── commands.rs  # Command parser (/use, /list, /status, /cmd)
│       ├── api/
│       │   ├── server.rs    # HTTP API (axum) + reply channel
│       │   └── ws.rs        # WebSocket handler (real-time push)
│       ├── storage/         # SQLite credential persistence
│       └── config.rs
│
├── client/hermes-wechat-plugin/  # Hermes message plugin (Python)
│   ├── adapter.py           # WeChatGatewayAdapter (register/poll/handle_message/reply)
│   ├── plugin.yaml          # Plugin metadata
│   └── __init__.py          # Exports register() entry point
│
└── docs/
```

### Message Flow

```
WeChat → long-poll getupdates → Router.handle_incoming()
  ├── is command (/use, /list, /status, /cmd)
  │     → handle built-in, reply directly to WeChat
  └── is normal message
        → if media: download from CDN + AES decrypt + cache
        → record context (for reply routing)
        → enqueue to active agent's message queue
        → push to agent's WebSocket (if connected)
        → agent pulls via GET /api/agents/{name}/poll
        → agent processes, then POST /api/agents/{name}/reply
        → main.rs reply processor receives via channel
        → sends via sendmessage back to WeChat (text or media)
```

### Features

- **Heartbeat detection** — gateway auto-detects offline agents via poll timestamps (30s check, 60s timeout)
- **Media support** — image/voice/video/file, AES-128-ECB CDN encrypt on send, decrypt on receive
- **Reply channel** — async reply processing via tokio mpsc, separates HTTP API from iLink sending
- **WebSocket push** — real-time message delivery to connected agents with 30s ping/pong

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
- [Hermes Agent](https://hermes-agent.nousresearch.com/) installed (for the plugin path)
- Python 3.10+ with `aiohttp`

### 1. Build and Run the Gateway

```bash
cd wechat-gateway

# Use proxy if needed (e.g. behind GFW)
export HTTP_PROXY=http://127.0.0.1:7897
export HTTPS_PROXY=http://127.0.0.1:7897

# Build
cargo build --release

# Run — scan the QR code in terminal to log in
./target/release/wechat-gateway
```

On first run, the gateway prints a QR code in the terminal. Scan it with WeChat and tap **Confirm** on your phone. Credentials are saved to `~/.wechat-gateway/data.db` and reused automatically on restart.

### 2. Install the Hermes Plugin

Symlink the plugin into Hermes' plugin directory:

```bash
ln -s ~/develop/wechat-gateway/client/hermes-wechat-plugin ~/.hermes/plugins/wechat-gateway
```

Add environment variables to `~/.hermes/.env`:

```bash
WECHAT_GATEWAY_URL=http://127.0.0.1:8765
WECHAT_GATEWAY_AGENT_NAME=hermes
```

Enable the plugin in `~/.hermes/config.yaml`:

```yaml
plugins:
  enabled:
    - wechat-gateway

gateway:
  platforms:
    wechat_gateway:
      enabled: true
      extra:
        dm_policy: pairing  # requires approval before first use
```

### 3. Start Hermes

```bash
hermes gateway restart
```

Hermes loads the plugin, which registers itself (`hermes`) with the gateway and starts polling for messages.

### 4. Pair and Chat

Send a message from WeChat to the bot account. Since `dm_policy` is `pairing`, Hermes will prompt you to authorize:

```
Unauthorized user <wxid> on wechat_gateway
```

On the Hermes CLI, approve the user:

```bash
hermes pairing approve <wxid> wechat_gateway
```

Hermes sends the pairing code back through the gateway to you on WeChat. After pairing, messages are handled normally — including all slash commands like `/new`.

### Run Tests

```bash
# Run all tests across the workspace
cargo test

# Run gateway tests only
cargo test -p wechat-gateway
```

> **Note**: All gateway modules have complete unit test coverage (~440 tests).

### API Reference (for custom agents)

Any HTTP client can act as an agent by registering directly:

```bash
# Register
curl -X POST http://127.0.0.1:8765/api/agents/register \
  -H 'Content-Type: application/json' \
  -d '{"name": "my-agent", "capabilities": ["text"]}'

# Poll for pending messages
curl http://127.0.0.1:8765/api/agents/my-agent/poll

# Reply
curl -X POST http://127.0.0.1:8765/api/agents/my-agent/reply \
  -H 'Content-Type: application/json' \
  -d '{"reply_to_id": "msg_id", "text": "Hello from my agent!"}'

# Proactive send (pairing codes, notifications)
curl -X POST http://127.0.0.1:8765/api/agents/my-agent/reply \
  -H 'Content-Type: application/json' \
  -d '{"reply_to_id": "", "text": "Your code is 12345678", "to_user": "wxid_xxx"}'

# Check status
curl http://127.0.0.1:8765/api/status
```

### WebSocket (real-time push)

```bash
# Connect via websocat or similar
websocat ws://127.0.0.1:8765/ws/agents/my-agent

# Receive pushed messages as JSON
{"type":"message","id":"...","from_user":"wxid_xxx","text":"hello","context_token":"..."}

# Send a reply via WebSocket
{"type":"reply","reply_to_id":"msg_id","text":"Hello back"}
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

## iLink Protocol Overview

iLink is Tencent's official WeChat Bot API (opened in 2026), pure HTTP/JSON. Key endpoints:

| Endpoint | Function |
|----------|----------|
| `GET /ilink/bot/get_bot_qrcode` | Get QR code for login |
| `GET /ilink/bot/get_qrcode_status` | Poll QR scan status |
| `POST /ilink/bot/getupdates` | Long-poll receive messages (35s hold) |
| `POST /ilink/bot/sendmessage` | Send a message |
| `POST /ilink/bot/sendtyping` | Send typing indicator |
| `POST /ilink/bot/getuploadurl` | Get CDN media upload URL |
| `POST /ilink/bot/getconfig` | Get typing ticket |
| `POST /ilink/bot/msg/notifystart` | Enable outbound message capability |

Each request requires the `X-WECHAT-UIN` header (random uint32 → decimal string → base64, regenerated per request) and `AuthorizationType: ilink_bot_token`.

The WeChat iLink connection may return `errcode: -14` on temporary session timeout. The client sleeps 600s and retries automatically — QR authorization is long-term and does not require re-scanning.

## License

MIT
