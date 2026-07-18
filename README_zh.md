# wechat-gateway

微信 iLink 消息网关 — 将一条微信连接复用到多个 AI agent。

## 背景

微信 iLink Bot API 有独占限制：一个微信账号同时只能有一个长轮询连接。当你本地有多个 AI agent（Hermes、OpenClaw、Claude Code 等）都需要接入微信时，需要一个代理网关来维护微信的连接，做消息分发和切换。

## 架构

```
WeChat ←── iLink 协议 ──→ wechat-gateway (Rust)
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

**核心设计原则：**

- **Gateway 是中心** — 维护微信 iLink 独占长轮询连接
- **Agent 主动注册** — agent 启动时通过 HTTP 注册到 gateway，带上名字
- **`/use <name>` 切换** — 微信内发消息切换当前激活的 agent
- **`/cmd <shell>` 执行** — 微信内直接执行 shell 命令

## 项目结构

```
wechat-gateway/
├── gateway/                 # 网关核心 (Rust crate)
│   └── src/
│       ├── ilink/           # iLink 协议实现
│       │   ├── types.rs     # iLink 类型定义 (serde)
│       │   ├── client.rs    # HTTP 客户端 (扫码登录/长轮询/发消息/媒体上传)
│       │   ├── media.rs     # AES-128-ECB 加密解密 + CDN URL 校验
│       │   └── download.rs  # CDN 媒体下载 (SSRF 防护)
│       ├── agents/
│       │   ├── registry.rs  # Agent 注册表 (含心跳检测)
│       │   ├── queue.rs     # 消息队列
│       │   └── ws_registry.rs # WebSocket 连接注册表
│       ├── router/
│       │   ├── router.rs    # 消息路由 (支持媒体类型)
│       │   └── commands.rs  # 命令解析 (/use, /list, /status, /cmd)
│       ├── api/
│       │   ├── server.rs    # HTTP API (axum) + 回复通道
│       │   └── ws.rs        # WebSocket 处理器 (实时推送)
│       ├── storage/         # SQLite 凭证持久化
│       └── config.rs
│
├── client/hermes-wechat-plugin/  # Hermes 消息插件 (Python)
│   ├── adapter.py           # WeChatGatewayAdapter (register/poll/handle_message/reply)
│   ├── plugin.yaml          # 插件元数据
│   └── __init__.py          # 导出 register() 入口
│
└── docs/
```

### 消息流

```
WeChat → long-poll getupdates → Router.handle_incoming()
  ├── 是命令 (/use, /list, /status, /cmd)
  │     → 内置处理，直接发回微信
  └── 是普通消息
        → 有媒体附件则 CDN 下载 + AES 解密 + 缓存
        → 记录上下文 (用于回复路由)
        → 推入 active_agent 的消息队列
        → 推送至 agent WebSocket (如已连接)
        → agent 通过 GET /api/agents/{name}/poll 拉取
        → agent 处理完后 POST /api/agents/{name}/reply
        → main.rs 回复处理器通过 channel 接收
        → 通过 sendmessage 发回微信 (文本或媒体)
```

### 功能特性

- **Agent 心跳检测** — 通过 poll 时间戳自动检测离线 agent（30 秒检查, 60 秒超时）
- **媒体消息支持** — 图片/语音/视频/文件类型，AES-128-ECB CDN 加解密
- **回复通道** — 异步回复处理，通过 tokio mpsc 通道分离 HTTP API 和 iLink 发送
- **WebSocket 推送** — 实时推送消息到已连接的 agent，30s ping/pong 保活

### 内置命令

| 命令 | 用途 |
|------|------|
| `/use <name>` | 切换到指定 agent |
| `/list` | 列出已注册 agent |
| `/status` | 查看连接和 agent 状态 |
| `/cmd <shell>` | 执行 shell 命令（支持 `timeout <秒>` 前缀） |

## 快速开始

### 前置要求

- Rust 1.75+
- 一个微信账号（用于扫码登录）

### 构建

```bash
cd wechat-gateway

# 使用代理（国内网络需要）
export HTTP_PROXY=http://127.0.0.1:7897
export HTTPS_PROXY=http://127.0.0.1:7897

# 构建
cargo build --release
```

### 运行测试

```bash
# 运行全部测试
cargo test

# 仅运行网关测试
cargo test -p wechat-gateway
```

> **注意**: 所有模块都有完整的单元测试覆盖（约 440 个 gateway 测试）。

### 注册 Agent

Agent 启动后通过 HTTP 注册到网关：

```bash
curl -X POST http://127.0.0.1:8765/api/agents/register \
  -H 'Content-Type: application/json' \
  -d '{"name": "my-agent", "capabilities": ["text"]}'
```

### 轮询和回复

```bash
# Agent 轮询消息
curl http://127.0.0.1:8765/api/agents/my-agent/poll

# 回复消息
curl -X POST http://127.0.0.1:8765/api/agents/my-agent/reply \
  -H 'Content-Type: application/json' \
  -d '{"reply_to_id": "msg_id", "text": "回复内容"}'

# 回复消息（带媒体文件）
curl -X POST http://127.0.0.1:8765/api/agents/my-agent/reply \
  -H 'Content-Type: application/json' \
  -d '{"reply_to_id": "msg_id", "text": "图片回复", "media_paths": ["/tmp/image.jpg"]}'
```

### 查看状态

```bash
curl http://127.0.0.1:8765/api/status
```

## 配置

网关通过环境变量配置：

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `GW_HTTP_ADDR` | `127.0.0.1` | HTTP API 监听地址 |
| `GW_HTTP_PORT` | `8765` | HTTP API 端口 |
| `GW_ILINK_BASE_URL` | `https://ilinkai.weixin.qq.com` | iLink API 地址 |
| `GW_DB_PATH` | `~/.wechat-gateway/data.db` | 数据库路径 |
| `GW_CMD_TIMEOUT` | `30` | `/cmd` 默认超时(秒) |
| `GW_CMD_MAX_OUTPUT` | `2000` | `/cmd` 最大输出字符数 |

## iLink 协议要点

iLink 是腾讯官方的微信 Bot API 协议（2026 年开放），纯 HTTP/JSON。核心端点：

| 端点 | 功能 |
|------|------|
| `GET /ilink/bot/get_bot_qrcode` | 获取登录二维码 |
| `GET /ilink/bot/get_qrcode_status` | 轮询扫码状态 |
| `POST /ilink/bot/getupdates` | 长轮询接收消息 (35s hold) |
| `POST /ilink/bot/sendmessage` | 发送消息 |
| `POST /ilink/bot/sendtyping` | 发送"正在输入"状态 |
| `POST /ilink/bot/getuploadurl` | 获取 CDN 媒体上传地址 |
| `POST /ilink/bot/getconfig` | 获取 typing_ticket |
| `POST /ilink/bot/msg/notifystart` | 开启出站消息能力 |

每个请求需要 `X-WECHAT-UIN` Header（随机 uint32 → 十进制 → base64，每次请求重新生成）和 `AuthorizationType: ilink_bot_token`。

`errcode: -14` 表示临时会话超时，非凭据过期，休眠 600 秒后自动重试即可。扫码授权为长期授权，无需重新扫码。

## 许可证

MIT
