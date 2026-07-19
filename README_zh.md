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
                      ┌────────┴──────────┐
                      │                  │
              Hermes Plugin        Claude Adapter
              (Python, poll)     (Node.js, poll)
                      │                  │
                 Hermes Agent  @anthropic-ai/claude-agent-sdk
                                         │
                                    Claude Code
```

**核心设计原则：**

- **Gateway 是中心** — 维护微信 iLink 独占长轮询连接
- **Agent 主动注册** — agent 启动时通过 HTTP 注册到 gateway，带上名字
- **`/use <name>` 切换** — 微信内发消息切换当前激活的 agent（如 `/use claude`、`/use hermes`）
- **`/cmd <shell>` 执行** — 微信内直接执行 shell 命令
- **双 Agent 支持** — Hermes（Python）和 Claude Code（Node.js）可同时注册，按需切换

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
├── client/claude-code-adapter/  # Claude Code 适配器 (Node.js/TypeScript → wechat-claude)
│   └── src/
│       ├── index.ts             # 入口：注册 → 轮询 → 消息路由
│       ├── claude-session.ts    # Claude SDK query() 封装
│       ├── gateway-client.ts    # Gateway HTTP 客户端
│       ├── session-store.ts     # 会话持久化 (wxid → cwd → sessionId)
│       ├── query-manager.ts     # 运行时查询状态管理
│       ├── approval.ts          # 工具审批命令解析
│       ├── cd-command.ts        # /cd 工作目录切换
│       ├── formatter.ts         # Markdown 转纯文本
│       ├── streaming-batcher.ts # 流式合批、空闲提示、长回复分段
│       └── config.ts            # 环境变量加载
│
├── scripts/
│   └── build.sh                 # 构建 gateway + wechat-claude 二进制
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

- **双 Agent 支持** — 网关支持多个已注册 agent，通过微信 `/use <name>` 切换
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
- [Bun](https://bun.sh) 1.3+（编译 wechat-claude 时需要）。安装：

  ```bash
  curl -fsSL https://bun.sh/install | bash
  ```
- 已安装 [Hermes Agent](https://hermes-agent.nousresearch.com/)
- Python 3.10+ 并安装 `aiohttp`
- [Claude Code](https://claude.ai/code) CLI（用于 Claude Code Adapter）

### 1. 构建并启动网关

```bash
cd wechat-gateway

# 使用代理（国内网络需要）
export HTTP_PROXY=http://127.0.0.1:7897
export HTTPS_PROXY=http://127.0.0.1:7897

# 一键构建 gateway（Rust 二进制）
./scripts/build.sh

# 运行 — 终端输出二维码，用微信扫码并确认
./target/release/wechat-gateway
```

或者只构建 gateway：

```bash
cargo build --release
./target/release/wechat-gateway
```

首次运行会在终端打印二维码，用微信扫码并在手机上点**确认登录**。凭证保存到 `~/.wechat-gateway/data.db`，后续启动自动复用。

#### 以常驻后台服务运行（macOS launchd）

为了让 gateway 在重启、锁屏、休眠后持续运行，使用 launchd + `caffeinate`：

创建 `~/Library/LaunchAgents/com.wechat-gateway.plist`：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.wechat-gateway</string>

    <key>ProcessType</key>
    <string>Background</string>

    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/caffeinate</string>
        <string>-disu</string>
        <string>/path/to/wechat-gateway/target/release/wechat-gateway</string>
    </array>

    <key>EnvironmentVariables</key>
    <dict>
        <key>RUST_LOG</key>
        <string>info</string>
    </dict>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>StandardOutPath</key>
    <string>/Users/youruser/.wechat-gateway/stdout.log</string>

    <key>StandardErrorPath</key>
    <string>/Users/youruser/.wechat-gateway/stderr.log</string>
</dict>
</plist>
```

要点说明：
- `ProcessType: Background` — 防止 macOS 在屏幕关闭后挂起进程
- `caffeinate -disu` — 阻止显示器休眠、系统空闲休眠、系统休眠
- `KeepAlive: true` — 崩溃后自动重启
- `RunAtLoad: true` — 登录时自动启动

加载并启动：

```bash
launchctl load ~/Library/LaunchAgents/com.wechat-gateway.plist
```

查看状态：

```bash
launchctl list com.wechat-gateway
```

#### 以常驻后台服务运行 Claude Adapter

创建 `~/Library/LaunchAgents/com.wechat-claude.plist`：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.wechat-claude</string>

    <key>ProcessType</key>
    <string>Background</string>

    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/caffeinate</string>
        <string>-disu</string>
        <string>/path/to/wechat-gateway/target/release/wechat-claude</string>
    </array>

    <key>EnvironmentVariables</key>
    <dict>
        <key>CLAUDE_MODEL</key>
        <string>sonnet</string>
        <key>CLAUDE_EFFORT</key>
        <string>high</string>
        <key>CLAUDE_CWD</key>
        <string>/Users/youruser</string>
    </dict>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>StandardOutPath</key>
    <string>/Users/youruser/.wechat-gateway/claude-stdout.log</string>

    <key>StandardErrorPath</key>
    <string>/Users/youruser/.wechat-gateway/claude-stderr.log</string>
</dict>
</plist>
```

#### 更新代码后重新部署

```bash
./scripts/build.sh

# 重启 gateway
launchctl unload ~/Library/LaunchAgents/com.wechat-gateway.plist
launchctl load ~/Library/LaunchAgents/com.wechat-gateway.plist

# 重启 claude adapter
launchctl unload ~/Library/LaunchAgents/com.wechat-claude.plist
launchctl load ~/Library/LaunchAgents/com.wechat-claude.plist
```

凭据保存在 SQLite 中，无需重新扫码。

### 2. 安装 Hermes 插件

将插件软链接到 Hermes 插件目录：

```bash
ln -s ~/develop/wechat-gateway/client/hermes-wechat-plugin ~/.hermes/plugins/wechat-gateway
```

在 `~/.hermes/.env` 中添加环境变量：

```bash
WECHAT_GATEWAY_URL=http://127.0.0.1:8765
WECHAT_GATEWAY_AGENT_NAME=hermes
```

在 `~/.hermes/config.yaml` 中启用插件：

```yaml
plugins:
  enabled:
    - wechat-gateway

gateway:
  platforms:
    wechat_gateway:
      enabled: true
      extra:
        dm_policy: pairing  # 首次使用需审批
```

### 3. 启动 Hermes

```bash
hermes gateway restart
```

Hermes 加载插件后会自动向 gateway 注册（名称 `hermes`）并开始轮询消息。

### 4. 配对并聊天

向机器人账号发一条微信消息。由于 `dm_policy` 设为 `pairing`，Hermes 会提示未授权：

```
Unauthorized user <wxid> on wechat_gateway
```

在 Hermes CLI 中审批该用户：

```bash
hermes pairing approve <wxid> wechat_gateway
```

Hermes 会通过 gateway 将配对码发回你的微信。配对完成后，消息正常处理，所有斜杠命令（如 `/new`）均可用。
### 5. 启动 Claude Code Adapter（备选 Agent）

Claude Code Adapter 是独立的 agent，连接到 Claude Code 而非 Hermes。编译为独立二进制，无需任何运行时依赖：

```bash
# 一键构建
./scripts/build.sh

# 运行
./target/release/wechat-claude
```

该适配器以 `claude` 名称注册到网关。在微信中切换 agent：

```
/use claude    → 切换到 Claude Code
/use hermes    → 切换回 Hermes
```

在微信中与 Claude Code 交互：

```
/cd              → 查看 workspace
/cd wiki         → 切换到 wiki 目录
/approve on      → 开启自动审批
帮我分析这个项目  → 消息发送给 Claude Code
```

完整文档见 `client/claude-code-adapter/README.md`。

### 运行测试

```bash
# 运行全部 Rust 测试
cargo test

# 仅运行网关测试
cargo test -p wechat-gateway

# 运行 Claude Code Adapter 测试
cd client/claude-code-adapter && npm test
```

> **注意**: 所有模块都有完整的单元测试覆盖（约 440 个 gateway 测试）。

### API 参考（自定义 Agent）

任何 HTTP 客户端都可以直接注册为 agent：

```bash
# 注册
curl -X POST http://127.0.0.1:8765/api/agents/register \
  -H 'Content-Type: application/json' \
  -d '{"name": "my-agent", "capabilities": ["text"]}'

# 轮询消息
curl http://127.0.0.1:8765/api/agents/my-agent/poll

# 回复消息
curl -X POST http://127.0.0.1:8765/api/agents/my-agent/reply \
  -H 'Content-Type: application/json' \
  -d '{"reply_to_id": "msg_id", "text": "回复内容"}'

# 主动发送（配对码、通知等）
curl -X POST http://127.0.0.1:8765/api/agents/my-agent/reply \
  -H 'Content-Type: application/json' \
  -d '{"reply_to_id": "", "text": "配对码: 12345678", "to_user": "wxid_xxx"}'

# 查看状态
curl http://127.0.0.1:8765/api/status
```

### WebSocket 推送

```bash
# 连接 WebSocket（需安装 websocat）
websocat ws://127.0.0.1:8765/ws/agents/my-agent

# 收到推送消息（JSON）
{"type":"message","id":"...","from_user":"wxid_xxx","text":"你好","context_token":"..."}

# 通过 WebSocket 回复
{"type":"reply","reply_to_id":"msg_id","text":"回复内容"}
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
