# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Code Navigation

This repository is indexed by CodeGraph. Use `codegraph_explore` (MCP tool) for code understanding and navigation — prefer it over grep/find/Read for symbol lookup, architecture questions, and relationship discovery. A single call returns the verbatin source of all relevant symbols plus the call paths between them.

## Build & Test

```bash
# Build everything (workspace includes gateway + client/hermes)
cargo build

# Run all tests across workspace
cargo test

# Run only gateway tests
cargo test -p wechat-gateway

# Run only hermes client tests
cargo test -p wechat-gateway-client-hermes

# Run a single test
cargo test test_name

# Run with proxy (crates.io behind GFW)
HTTP_PROXY=http://127.0.0.1:7897 HTTPS_PROXY=http://127.0.0.1:7897 cargo build
```

## Architecture

Two Rust crates in a workspace, each with a binary entry point:

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

### `client/hermes/` — Hermes ACP client crate (binary: `wechat-gateway-client-hermes`)

Agent-side client that connects to the gateway and forwards messages to Hermes via its ACP protocol.

```
client/hermes/src/
├── main.rs           # Binary entry: register → spawn ACP → poll loop (with media logging)
├── gateway/api.rs    # HTTP client for gateway REST API (register, poll, reply, media-enabled)
├── acp/client.rs     # JSON-RPC 2.0 client over stdio for Hermes ACP subprocess
├── client.rs         # HermesClient orchestrator: register → poll loop → ACP session → reply
├── config.rs         # Env-based client configuration
└── error.rs          # ClientError enum
```

**ACP protocol**: Hermes' Agent Client Protocol runs `hermes acp` as a subprocess, communicating via JSON-RPC 2.0 over stdin/stdout. Key methods: `initialize` → `session/new` → `session/prompt` (streaming) → collect AgentMessageChunk from `session/update` notifications → `session/close`. Session recovery via `session/load`.

## Testing

Test-first development. Each module has `#[cfg(test)] mod tests` with unit tests.

- Gateway tests use `mockito` for HTTP mocking (iLink endpoints) and `tempfile` for storage tests
- ACP tests validate JSON-RPC serialization/deserialization with serde roundtrips
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

## Hermes Client Configuration (env vars)

| Var | Default | Description |
|-----|---------|-------------|
| `GW_GATEWAY_URL` | `http://127.0.0.1:8765` | Gateway API URL |
| `GW_AGENT_NAME` | `hermes` | Name to register with gateway |
| `GW_HERMES_BIN` | `hermes` | Path to hermes executable |
| `GW_HERMES_CWD` | cwd | Working dir for ACP sessions |
| `GW_POLL_INTERVAL` | `1` | Poll interval in seconds |
| `GW_ACP_TIMEOUT` | `300` | ACP session timeout in seconds |
