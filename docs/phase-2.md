# Phase 2 — wechat-gateway 功能补全

## 概述

Phase 1 实现了完整的功能骨架（iLink 协议、Agent 注册、消息路由、指令系统、Hermes ACP 客户端），Phase 2 补全三个缺失的关键功能。

---

## 1. Agent 心跳检测 ✅

### 问题

Agent 注册到 gateway 后，如果进程崩溃或网络断开，gateway 没有机制发现，一直标记为 `Online`。

### 方案

Gateway 负责检测心跳超时，而不是 agent 主动发心跳（简化 agent 侧实现）：

- Agent 每次 `poll` 请求到达时，更新 `AgentInfo.last_seen` 时间戳
- Gateway 启动一个后台任务，**每 30 秒**扫描所有 agent：
  - 当前时间 - `last_seen` > 60 秒 → 标记 `Offline`
  - 当前时间 - `last_seen` ≤ 60 秒 → 保持 `Online`
- 如果 active_agent 离线，回复微信时提示"当前 agent 已离线，请使用 /use 切换"
- 如果主动切换到一个离线 agent，允许切换成功但不自动激活

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/src/agents/registry.rs` | 新增 `check_heartbeat(threshold_secs: u64)` 方法，或已有 `mark_offline` 够用 |
| `gateway/src/main.rs` | 新增 `spawn_heartbeat_checker()` 后台任务，循环 sleep 30s 调用 registry 检测 |
| `gateway/src/api/server.rs` | `handle_poll()` 调用时更新 `last_seen`（目前 register 已设 online，poll 时 `mark_online` 即可） |

### 测试

- 模拟 registry 中有 agent，设置旧的 last_seen → 调用检查后变为 Offline
- agent 刚 poll 过 → 检查后仍 Online
- active_agent 离线时 handle_incoming 的回复包含提示

---

## 2. 媒体文件收发 ✅

### 问题

当前只支持纯文本消息。iLink 协议支持图片（type=2）、语音（type=3）、文件（type=4）、视频（type=5），但需要 AES-128-ECB 加密/解密以及 CDN 上传/下载。

### 已实现

- **类型定义** — `MediaItem`、`VideoItem`、`GetUploadUrlRequest/Response`，扩展 `ImageItem`/`VoiceItem` 增加 `encrypt_query_param`/`aes_key`
- **AES-128-ECB 模块** — `ilink/media.rs`，PKCS7 padding 加解密
- **CDN 下载** — `ilink/download.rs`，SSRF 防护（只允许 `*.cdn.weixin.qq.com`）
- **媒体提取** — `Router::extract_media_info()` 解析图片/语音/视频/文件 item
- **回复路径** — agent 回复带 `media_paths`，通过 channel 异步处理
- **get_upload_url** — `ilink/client.rs`，获取 CDN 上传地址

### 待完成（后续迭代）

- 完整的 CDN 加密上传 → 构造媒体 sendmessage 流程（目前 media_paths 回复发文本提示）

---
## 3. WebSocket 推送 ✅

### 问题

当前 agent 通过 HTTP 轮询拉取消息（模式 A），延迟受 poll interval 限制（默认 1s），实时性不够高。

### 已实现

- **WsRegistry** — `gateway/src/agents/ws_registry.rs`，`Arc<Mutex<HashMap<String, UnboundedSender<String>>>>`，每个 agent 最多一个 WS 连接，新的替换旧的
- **WS 端点** — `GET /ws/agents/{name}`，可选的实时通道
- **出站（gateway → agent）** — main.rs 的 poll 循环中，消息入队后同时尝试 WS 推送，JSON 格式 `{"type":"message",...}`
- **入站（agent → gateway）** — 收到 `{"type":"reply",...}` 后解析为 `AgentReply`，走现有的 `reply_tx` channel 发送
- **心跳** — 每 30s ping/pong，断线自动清理注册
- **回退** — WS 断开后继续等 HTTP poll，两者共存

### 问题

当前 agent 通过 HTTP 轮询拉取消息（模式 A），延迟受 poll interval 限制（默认 1s），实时性不够高。

### 方案

在现有 HTTP 轮询基础上，增加 WebSocket 推送作为可选的实时通道（模式 B）：

1. **新增 WebSocket 端点**：`GET /ws/agents/{name}`
   - Agent 连接后建立 WebSocket 长连接
   - Gateway 有新消息时，通过 WebSocket 直接推送给 agent
   - Agent 也可以通过 WebSocket 发送回复（或继续使用 HTTP reply）

2. **回退机制**：
   - WebSocket 断线时，agent 自动回退到 HTTP 轮询
   - Gateway 检测到 WebSocket 断开后，继续将消息放入队列等待 HTTP poll
   - 两者共存，agent 可以同时使用两种方式

3. **消息格式**（WebSocket JSON 帧）：
   ```json
   // Gateway → Agent (推送消息)
   {"type": "message", "id": "msg_xxx", "from_user": "...", "text": "...", "timestamp": 123, "context_token": "...", "message_type": "text"}
   
   // Agent → Gateway (回复)
   {"type": "reply", "reply_to_id": "msg_xxx", "text": "回复内容"}
   ```

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/src/api/server.rs` | 新增 WebSocket 路由，使用 axum 的 ws 支持 |
| `gateway/src/agents/queue.rs` | 新增 WebSocket sender 注册表（agent_name → mpsc::Sender），`enqueue` 时同时推送到 WebSocket |
| `gateway/Cargo.toml` | 确认 axum 的 ws feature 已开启 |
| `client/hermes/src/gateway/api.rs` | 可选：新增 WebSocket 客户端 |

### 实现要点

- axum 内置 ws 支持（`axum::extract::ws`）
- 每个 agent 最多维护一个 WebSocket 连接
- 新的 WebSocket 连接替换旧的（防止重复）
- 心跳：每 30s ping/pong 检测连接存活

### 测试

- WebSocket 连接建立和消息收发（使用 `axum::extract::ws` 的测试工具或 `tungstenite`）
- 多个 agent 同时 WebSocket 连接
- WebSocket 断线后消息转到 HTTP 轮询
- 心跳超时断开

---

| 实现优先级 | 状态 |
|------------|------|
| Agent 心跳检测 | ✅ 已完成 |
| 媒体文件收发（含 CDN 上传） | ✅ 已完成 |
| WebSocket 推送 | ✅ 已完成 |

建议按 1 → 2 → 3 的顺序实现。
