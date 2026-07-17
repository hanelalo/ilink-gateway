# Phase 2 — wechat-gateway 功能补全

## 概述

Phase 1 实现了完整的功能骨架（iLink 协议、Agent 注册、消息路由、指令系统、Hermes ACP 客户端），Phase 2 补全四个缺失的关键功能。

---

## 1. Agent 心跳检测

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

## 2. 24h 自动重连

### 问题

iLink token 有效期 24 小时，过期后 `getupdates` 返回 `errcode: -14`。当前 main.rs 检测到 -14 就会立即重扫码，但最佳体验应该是在到期前主动预警、让用户选择是否重连、或在到期前完成无缝切换。

### 方案

参考 weixin-ClawBot-API 的 24h 自动重连机制：

1. **token 到期前的主动预警**（剩余 ~2h 时）：
   - 向活跃的微信用户发送消息："⚠️ 微信连接将在 2 小时后过期，回复 Y 立即重新连接，回复 N 稍后提醒"
   - 用户回复 Y → 获取新二维码 → 用户扫码 → token 原子替换
   - 用户回复 N → 30 分钟后再次提醒
   - 最后 30 分钟 → 强制重连，无需确认

2. **技术实现**：
   - `main.rs` 的 poll loop 旁启动一个 `reconnect_timer` 异步任务
   - 记录 `login_time`，用 `ArcSwap<String>` 存储 token（原子替换）
   - 重连期间旧 token 继续工作，新 token 扫码成功后 `getupdates` 自动用新 token
   - 重连过程有重入守卫（防止多个重连同时进行）

3. **扫码到一半 token 过期**：
   - 当前 `main.rs` 在 `getupdates` 返回 -14 时立即触发重扫码
   - 把这个逻辑改为兜底：如果主动预警流程走不通（用户没看到、没回复），最后的保障就是 -14 被动触发重连

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/src/main.rs` | 新增 `start_reconnect_timer()` 异步任务，`send_wechat_message()` 工具函数 |
| `gateway/src/ilink/client.rs` | 无变更（已有 `get_qr_code` / `poll_qr_status` / `send_message`） |
| `gateway/src/storage/sqlite_store.rs` | 无变更（已有 `save_credentials`） |

### 配置项（新增环境变量）

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `GW_SESSION_DURATION` | `86400` | token 有效期（秒），默认 24h |
| `GW_RECONNECT_WARNING_BEFORE` | `7200` | 提前多久预警（秒），默认 2h |
| `GW_RECONNECT_REMINDER_INTERVAL` | `1800` | 用户拖延后再次提醒间隔（秒），默认 30min |
| `GW_RECONNECT_FORCE_BEFORE` | `1800` | 强制重连阈值（秒），默认 30min |

### 测试

- 模拟 `login_time` 接近到期 → 预警消息发送
- 模拟用户回复 Y → 重扫码流程触发
- 模拟用户回复 N → 计时器重置，下次再问
- 模拟 -14 被动触发 → 兜底重连
- 重连期间旧 token 仍能发消息

---

## 3. 媒体文件收发

### 问题

当前只支持纯文本消息。iLink 协议支持图片（type=2）、语音（type=3）、文件（type=4）、视频（type=5），但需要 AES-128-ECB 加密/解密以及 CDN 上传/下载。

### 方案

参考 Hermes `weixin.py` 和 iLink Hub `upstream.rs` 的实现：

**接收媒体消息：**
1. `getupdates` 返回的 `item_list` 中包含媒体类型的 item
2. 根据 `type` 判断媒体类型，提取 `encrypt_query_param`（或 `full_url`）+ `aes_key`
3. 如果是 `encrypt_query_param`：构造 CDN URL 下载 → AES-128-ECB 解密 → 保存到本地缓存
4. 如果是 `full_url`：直接下载（需校验 CDN URL 白名单防 SSRF）→ 保存到本地缓存
5. 将本地文件路径放入 `AgentMessage` 的 `media_paths` 字段（需要先扩展 `AgentMessage` 和 `QueuedMessage`）

**发送媒体消息：**
1. agent 回复时附带 `media_paths: ["/tmp/image.png"]`
2. gateway 读取文件 → `getuploadurl` 获取上传地址 → AES-128-ECB 加密 → 上传 CDN
3. 构造媒体类型的 `sendmessage` 请求体（包含 `encrypt_query_param`、`aes_key` 等）

**AES-128-ECB 加密/解密：**
```rust
fn aes128_ecb_encrypt(plaintext: &[u8], key: &[u8]) -> Vec<u8> {
    // PKCS7 padding + AES-128-ECB encrypt
}
fn aes128_ecb_decrypt(ciphertext: &[u8], key: &[u8]) -> Vec<u8> {
    // AES-128-ECB decrypt + PKCS7 unpad
}
```

需要新增依赖 `aes` crate 或使用 `cryptography` 的 Rust 绑定。也可以参考 iLink Hub 的实现方式。

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/src/ilink/types.rs` | 扩展 `AgentMessage` 和 `QueuedMessage` 增加 `media_paths` 字段；文件接收/发送类型的 serde 确认 |
| `gateway/src/ilink/client.rs` | 新增 `get_upload_url()`，新增 `upload_media()` 方法；新增 `download_media()` 方法 |
| `gateway/src/ilink/media.rs` | **新文件** — AES-128-ECB 加密/解密工具函数，CDN URL 构建 |
| `gateway/src/router/router.rs` | `handle_incoming` 处理媒体消息时提取媒体信息 |
| `gateway/Cargo.toml` | 新增 `aes` 或 `crypto-common` 依赖 |

### 媒体文件类型

| type | 含义 | 入站处理 | 出站处理 |
|------|------|----------|----------|
| 2 | 图片 | CDN 下载 → AES 解密 → 缓存 `.jpg` | 读文件 → AES 加密 → CDN 上传 → send |
| 3 | 语音（silk） | CDN 下载 → AES 解密 → 缓存 `.silk` | 同文件流程 |
| 4 | 文件 | CDN 下载 → AES 解密 → 缓存 | 同文件流程 |
| 5 | 视频 | CDN 下载 → AES 解密 → 缓存 `.mp4` | 同文件流程 |

### 测试

- AES-128-ECB 加密/解密向量测试（固定 key/plaintext 验证结果）
- CDN URL 白名单校验（允许/拒绝）
- `getuploadurl` 请求/响应序列化
- 媒体消息解析（从 mock getupdates 响应中提取媒体信息）
- SSRF 防护测试（拒绝非微信 CDN 域名）

---

## 4. WebSocket 推送

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

## 实现优先级

1. **Agent 心跳检测** — 最简单，改动最小，不影响其他功能
2. **24h 自动重连** — 核心体验问题，生产必须
3. **媒体文件收发** — 功能完整性的关键缺口，但技术复杂度较高
4. **WebSocket 推送** — 性能优化，优先级最低

建议按 1 → 2 → 3 → 4 的顺序实现。
