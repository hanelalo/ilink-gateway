# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 语言要求

所有回答必须使用中文。

## Code Navigation

This repository is indexed by CodeGraph. Use `codegraph_explore` (MCP tool) for code understanding and navigation — prefer it over grep/find/Read for symbol lookup, architecture questions, and relationship discovery. A single call returns the verbatin source of all relevant symbols plus the call paths between them.

## Build & Test

```bash
# Build gateway
cargo build

# Run all tests across workspace
cargo test

# Run only gateway tests
cargo test -p wechat-gateway

# Run a single test
cargo test test_name

# Run with proxy (crates.io behind GFW)
HTTP_PROXY=http://127.0.0.1:7897 HTTPS_PROXY=http://127.0.0.1:7897 cargo build
```

## Architecture

Single Rust crate (gateway/) + Python Hermes message plugin (client/hermes-wechat-plugin/):

### `gateway/` — iLink WeChat message gateway (binary: `wechat-gateway`)

Core gateway that maintains the single iLink long-poll connection to WeChat and routes messages to registered agents.

```
gateway/src/
├── main.rs            # Binary entry: config → QR login → long-poll loop
│                      #   + reply processor (channel-based text + media upload)
│                      #   + heartbeat checker (30s/60s)
│                      #   + media download from CDN (AES decrypt + cache)
│                      #   + WebSocket push to connected agents
├── ilink/             # WeChat iLink protocol implementation
│   ├── types.rs       # All iLink wire types, media types, AgentMessage/Reply
│   ├── client.rs      # HTTP client: QR login, getupdates, sendmessage, getuploadurl, etc.
│   ├── media.rs       # AES-128-ECB encrypt/decrypt, CDN URL validation, media upload
│   └── download.rs    # CDN media download with SSRF protection
├── agents/            # Agent lifecycle management
│   ├── registry.rs    # AgentRegistry: name→AgentInfo, online/offline, heartbeat check
│   ├── queue.rs       # MessageQueue: per-agent FIFO (Arc<Mutex<...>>)
│   └── ws_registry.rs # WsRegistry: active WebSocket connections per agent
├── router/            # Message routing and commands
│   ├── router.rs      # Router: registry + queue + commands, media extraction
│   └── commands.rs    # /use, /list, /status, /cmd + executor + dangerous filter
├── api/               # HTTP + WebSocket API
│   ├── server.rs      # Axum HTTP: register, poll, reply (via channel), status
│   └── ws.rs          # WebSocket handler: real-time push, agent reply parsing
├── storage/           # SQLite credential persistence
│   └── sqlite_store.rs
├── config.rs          # Env-based configuration
└── error.rs           # GatewayError enum
```

**Key constraint**: The iLink connection may return `errcode: -14` on temporary session timeout. The client sleeps 600s and retries automatically — QR authorization is long-term and does not require re-scanning.

**Message flow**: WeChat → iLink long-poll → Router.handle_incoming() → parse command or enqueue to active agent's queue → agent polls via HTTP API → agent replies via POST reply → reply processor (channel-based) sends sendmessage back to WeChat. Media messages are extracted into `AgentMessage.media` with CDN download + AES-128-ECB decryption path.

### `client/hermes-wechat-plugin/` — Hermes Message Plugin (Python)

Hermes platform adapter that connects to the gateway as a registered agent:

```
client/hermes-wechat-plugin/
├── adapter.py         # WeChatGatewayAdapter: register, poll, handle_message, send/reply
├── plugin.yaml        # Plugin metadata (requires_env: WECHAT_GATEWAY_URL)
└── __init__.py        # Exports register() entry point
```

**Adapter flow**: register with gateway → poll loop (1s interval) → convert messages to Hermes MessageEvent → forward to Hermes handle_message() → reply via POST reply (or proactive send with to_user for pairing codes).

Symlink to Hermes: `ln -s ~/develop/wechat-gateway/client/hermes-wechat-plugin ~/.hermes/plugins/wechat-gateway`

## Testing

Test-first development. Each module has `#[cfg(test)] mod tests` with unit tests.

- Gateway tests use `mockito` for HTTP mocking (iLink endpoints) and `tempfile` for storage tests
- Command tests execute real shell commands via `tokio::process::Command`
- Router tests construct `WeixinMessage` fixtures and verify routing decisions

## Gateway Configuration (env vars)

| Var | Default | Description |
|-----|---------|-------------|
| `GW_HTTP_ADDR` | `127.0.0.1` | HTTP API bind address |
| `GW_HTTP_PORT` | `8765` | HTTP API port |
| `GW_ILINK_BASE_URL` | `https://ilinkai.weixin.qq.com` | iLink API base URL |
| `GW_DB_PATH` | `~/.wechat-gateway/data.db` | SQLite database path |
| `GW_CMD_TIMEOUT` | `30` | `/cmd` default timeout in seconds |
| `GW_CMD_MAX_OUTPUT` | `2000` | `/cmd` max output chars |
