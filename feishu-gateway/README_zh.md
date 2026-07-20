# 飞书 Gateway

一个[飞书/Lark](https://open.feishu.cn/)消息网关，对外暴露与本仓库 Rust 版 [`wechat-gateway`](../gateway/) **完全相同的 HTTP API 契约**，让现有 agent 客户端（`client/claude-code-adapter`、`client/hermes-wechat-plugin`）**零协议改动**接入——只是后端 IM 从微信换成飞书。

## 状态

| 组件 | 状态 |
|------|------|
| HTTP API（register / poll / reply / status） | ✅ 可用 |
| Agent registry + queue + 心跳 | ✅ 可用 |
| Router（准入 / 命令 / 路由） | ✅ 可用 |
| 去重缓存 + 熔断器 | ✅ 可用 |
| 内存版状态存储 | ✅ 可用 |
| 飞书 WebSocket 接收 | ✅ 可用 |
| 飞书 client（发消息 / 上传 / 下载） | ✅ 可用 |
| Reply processor（真实发送） | ✅ 可用 |
| 媒体下载 / 缓存 | ✅ 可用 |
| SQLite 持久化存储 | ⏳ 内存版（待接 `modernc.org/sqlite`） |

HTTP 层今天已完全可用：agent 可以注册、轮询、回复、查询状态。飞书后端（WebSocket 事件接收 + 发消息）是剩余工作，需要 Go ≥ 1.20 编译。

## 架构

```
feishu-gateway/
├── cmd/feishu-gateway/      # 入口：装配组件、启动 HTTP server
└── internal/
    ├── model/               # AgentMessage、AgentReply、MediaItem、IncomingMessage
    ├── config/              # env 加载、DmPolicy / GroupPolicy
    ├── agent/               # Registry（心跳）+ MessageQueue（FIFO）
    ├── router/              # 准入 + slash 命令 + 入队
    ├── api/                 # HTTP handler（register/poll/reply/status）
    ├── storage/             # Store 接口 + 内存实现
    ├── dedup/               # TTL 去重缓存
    ├── breaker/             # 滑动窗口熔断器
    ├── feishu/              # 飞书 SDK 集成（待升级 Go）
    └── reply/               # Reply processor（待飞书 client）
```

**并发模型**：每个关注点跑在独立 goroutine —— 心跳检查、HTTP server、（未来）飞书 WS 接收、（未来）reply processor。共享状态用短而互不嵌套的互斥临界区，避免锁顺序死锁。

## 快速开始

### 前置条件

- 当前代码需 Go ≥ 1.19；接入飞书 SDK 后需 Go ≥ 1.20

### 构建与运行

```bash
make build
GW_FEISHU_APP_ID=cli_xxx GW_FEISHU_APP_SECRET=xxx make run
```

或直接：

```bash
go build -o bin/feishu-gateway ./cmd/feishu-gateway
GW_FEISHU_APP_ID=cli_xxx GW_FEISHU_APP_SECRET=xxx ./bin/feishu-gateway
```

### 测试

```bash
make test          # 所有包
make test-verbose  # 带每个测试输出
```

## 配置

全部通过 `GW_*` 环境变量配置。

| 变量 | 默认值 | 用途 |
|------|--------|------|
| `GW_HTTP_ADDR` | `127.0.0.1` | HTTP API 绑定地址 |
| `GW_HTTP_PORT` | `8765` | HTTP API 端口 |
| `GW_DB_PATH` | `~/.feishu-gateway/data.db` | SQLite 路径（待升级 Go） |
| `GW_CMD_TIMEOUT` | `30` | `/cmd` 默认超时（秒） |
| `GW_CMD_MAX_OUTPUT` | `2000` | `/cmd` 输出截断（字符） |
| `GW_DM_POLICY` | `open` | DM 准入：`disabled` / `pairing` / `allowlist` / `open` |
| `GW_GROUP_POLICY` | `disabled` | 群聊准入：`disabled` / `all` / `allowlist` |
| `GW_ALLOWED_USERS` | （空） | 允许的 open_id，逗号分隔 |
| `GW_ALLOWED_GROUPS` | （空） | 允许的 chat_id，逗号分隔 |
| `GW_FEISHU_APP_ID` | （必填） | 飞书 App ID |
| `GW_FEISHU_APP_SECRET` | （必填） | 飞书 App Secret |
| `GW_FEISHU_BASE_URL` | `https://open.feishu.cn` | LarkSuite 国际版：`https://open.larksuite.com` |
| `GW_FEISHU_BOT_OPEN_ID` | （空） | 机器人 open_id，启用群聊 @提及检测 |
| `GW_MEDIA_CACHE_DIR` | `~/.feishu-gateway/media` | 媒体下载缓存目录 |
| `GW_HEARTBEAT_CHECK_INTERVAL` | `30` | 心跳检查周期（秒） |
| `GW_HEARTBEAT_TIMEOUT` | `60` | 心跳超时阈值（秒） |
| `GW_DEDUP_TTL` | `300` | 消息去重 TTL（秒） |
| `GW_REPLY_QUEUE_DEPTH` | `256` | reply channel 缓冲大小 |
| `GW_MEDIA_CACHE_MAX_AGE_DAYS` | `7` | 媒体缓存保留天数 |
| `GW_LOG_LEVEL` | `info` | 日志级别 |

## HTTP API 契约

与 `wechat-gateway` 完全一致，agent 只需改 gateway URL 即可切换后端。

| Method | Path | Body | Response |
|--------|------|------|----------|
| POST | `/api/agents/register` | `{name, capabilities}` | `200 {ok, active_agent}` 或 `400 {ok:false, error}` |
| GET | `/api/agents/{name}/poll` | — | `200 {messages:[AgentMessage]}` 或 `404` |
| POST | `/api/agents/{name}/reply` | `{reply_to_id, text, media_paths?, to_user?, context_token?, agent_context?}` | 恒 `200 {ok:true}` |
| GET | `/api/status` | — | `{feishu:{connected}, active_agent, agents:{...}}` |

**隐含契约**（不能破坏）：
- `poll` 兼作心跳 —— 每次调用刷新 agent 的 `last_seen`
- 首次注册自动激活该 agent（client 依赖此行为）
- `reply` 恒返回 `200 {ok:true}`；失败只在日志里出现
- `media` 字段始终为 `[]`（绝不 `null`、绝不省略）—— client 类型为非可选
- `poll` 对未注册 agent 返回 `404` → 触发 client 重新注册

### 内置 slash 命令

由用户在 IM 端发送，gateway 直接处理：

- `/use <name>` —— 切换 active agent
- `/list` —— 列出已注册 agent
- `/status` —— 显示 gateway 状态
- `/cmd [timeout N] <shell>` —— 执行 shell 命令（危险命令拦截）
- `/gateway-help` —— 显示帮助

未识别的 `/xxx` 命令作为普通消息透传给 active agent。

## 开发

### 复用现有 client adapter

本项目的核心价值就是协议兼容。要把现有 client 指向本 gateway：

```bash
# Claude Code adapter
CLAUDE_GATEWAY_URL=http://127.0.0.1:8765 CLAUDE_GATEWAY_AGENT_NAME=claude \
  node dist/index.js

# Hermes 插件（需要飞书版 fork —— 见路线图）
WECHAT_GATEWAY_URL=http://127.0.0.1:8765 ...
```

### 路线图（升级 Go 之后）

1. 接入 `github.com/larksuite/oapi-sdk-go/v3` WebSocket 长连接
2. 飞书 client（发消息、回复、上传图片/文件、下载资源）
3. 事件归一化（类型映射、@占位符清理、post 富文本提取）
4. 真实 reply processor（resilient send + 熔断器）
5. 异步媒体下载 + 缓存
6. SQLite 持久化存储（`modernc.org/sqlite`）
7. Hermes 插件飞书 fork（解耦 `wechat_gateway` 平台身份）

完整设计见 `/Users/hanelalo/.claude/plans/go-gleaming-wolf.md`。

## 参考

- [飞书开放平台文档](https://open.feishu.cn/document)
- [飞书对接指南](../docs/feishu.md)（本仓库内，已校对）
- [Go SDK](https://github.com/larksuite/oapi-sdk-go)
- 姊妹项目：[`wechat-gateway`](../gateway/)（Rust）
