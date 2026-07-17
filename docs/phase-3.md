# Phase 3 — wechat-gateway 体验补全

## 概述

Phase 1 实现了完整的功能骨架，Phase 2 补全了心跳检测、媒体收发、WebSocket 推送三大缺失功能。Phase 3 聚焦于用户体验和工程完善：覆盖协议细节、处理边界情况、补全可观测性和部署质量。

---

## 1. "正在输入" 状态

### 问题

当前回复消息前没有调用 `sendtyping` 和 `getconfig`。微信用户发完消息后没有任何反馈，直到收到完整的回复（可能耗时几秒到几十秒），体验上像"消息丢了"。

### 方案

参考 iLink 协议规范，发消息前需要三步：

1. **获取 typing_ticket** — 首次向用户发消息前调 `getconfig`，ticket 可缓存约 10 分钟
2. **发送"正在输入"** — 调用 `sendtyping`，`status=1` 表示开始输入
3. **取消"正在输入"** — 发送完消息后调 `sendtyping`，`status=2` 表示取消

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/src/main.rs` | `handle_agent_replies` 中发送消息前加 typing indicator |
| `gateway/src/router/router.rs` | 命令回复（/status, /list 等）发送前也加 typing |
| `gateway/src/ilink/client.rs` | 已有 `send_typing` 和 `get_config` 方法，无需新增 |

### 实现要点

- `typing_ticket` 按 `ilink_user_id` 缓存，有效期约 10 分钟
- 避免并发从同 `ilink_user_id` 重复请求 `getconfig`
- 如果 `getconfig` 或 `sendtyping` 失败，不应阻塞消息发送（静默忽略即可）
- 长回复（如 `/cmd` 输出）应在回复内容到达前就显示 typing

### 测试

- `getconfig` 响应缓存逻辑
- 多次发送到同一用户复用缓存 ticket
- 缓存过期后重新请求
- `sendtyping` 失败不阻塞消息发送

---

## 2. `cdn_base_url` 配置联动

### 问题

目前 `GatewayConfig` 有 `cdn_base_url` 字段和环境变量 `GW_CDN_BASE_URL`，但在 main.rs 的媒体下载逻辑中 hardcode 了 `https://novac2c.cdn.weixin.qq.com`，没有真正使用配置值。

### 方案

将所有 hardcode 的 CDN 域名替换为 `config.cdn_base_url` 的引用。

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/src/main.rs` | 媒体下载时传 `config.cdn_base_url` 而非字符串常量 |
| `gateway/src/ilink/media.rs` | `build_cdn_download_url` 等函数调用处用动态 URL |
| `gateway/src/config.rs` | 无需变更，已有字段 |

### 测试

- 环境变量 `GW_CDN_BASE_URL` 自定义值正确生效
- 默认值回退正确

---

## 3. 优雅关闭

### 问题

当前 `main.rs` 没有注册信号处理器，`Ctrl+C` 直接杀死进程。iLink 连接没有正常断开，消息队列中的未处理消息直接丢失，WebSocket 连接被强制断开。

### 方案

使用 `tokio::signal` 监听 SIGTERM/SIGINT，收到信号后：

1. 停止 long-poll 循环（设置退出标志）
2. 关闭 HTTP 服务器（调用 `axum::serve` 的 `with_graceful_shutdown`）
3. 等待正在处理的回复完成（设置超时，默认 5 秒）
4. 清理 WebSocket 连接
5. 退出

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/src/main.rs` | 注册信号处理器，添加 graceful shutdown 逻辑 |
| `gateway/src/api/server.rs` | `start_server` 支持 `with_graceful_shutdown` 参数 |

### 实现要点

- `axum::serve` 原生支持 `with_graceful_shutdown( signal )`，传入 future 即可
- 需要 `tokio/signal` feature（当前 `tokio` 已有 `"full"`，无需加依赖）
- 回复处理器的 `mpsc::UnboundedReceiver` 会有 `None` 信号（sender drop），天然退出循环
- 超时保护：5 秒后即使未完成也强制退出

### 测试

- `start_server` 的 graceful shutdown 参数
- 信号触发后现有回复完成
- 超时场景测试

---

## 4. 退出消息通知

### 问题

网关重启或关闭时，有活跃的微信会话场景下，用户收到消息没回复，会以为是 agent 出问题了。

### 方案

在优雅关闭流程中，向最近有过消息交互的用户发送一条通知消息（如"⚠️ 网关正在维护，暂时离线"）。

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/src/main.rs` | 关闭前遍历活跃消息上下文，发通知消息 |

### 实现要点

- 从 `message_contexts` 中提取 `to_user` 列表（去重）
- 发送文本通知，忽略任何发送错误
- 通知消息发送后，可以移除该条目

### 测试

- `message_contexts` 中用户被正确通知
- 发送失败不阻塞关闭流程

---

## 5. 集成测试套件

### 问题

当前所有测试都是单元测试，没有端到端的集成测试。无法验证完整消息流（mock 微信 → main.rs 处理 → agent 轮询 → 回复 → mock 微信发送）的正确性。

### 方案

在 `gateway/tests/` 下创建集成测试套件，使用 mockito 模拟 iLink 服务端和 agent API，验证完整消息流：

1. **基础消息流**：
   - 启动 mock iLink 服务（getupdates + sendmessage）
   - 注册 agent
   - 模拟微信发消息
   - 验证 agent 轮询到消息
   - 模拟 agent 回复
   - 验证 iLink sendmessage 被调用且参数正确

2. **命令消息流**：
   - `/use` 切换后，消息路由到正确的 agent
   - `/cmd` 执行并返回结果
   - `/list` 返回格式正确

3. **媒体消息流**：
   - 微信发图片 → agent 轮询到包含 `media[].local_path`
   - agent 回复带 `media_paths` → 验证 getuploadurl 和 CDN PUT 被调用

4. **心跳检测流**：
   - 注册 agent，模拟 poll 超时
   - 验证 agent 被标记为 offline

5. **多 agent 流**：
   - 注册多个 agent，切换使用
   - 消息分别路由到不同的 agent

### 涉及模块

| 模块 | 变更 |
|------|------|
| `gateway/tests/` | **新目录** — 集成测试文件 |

### 实现要点

- 每个测试 fork 一个 `tokio::runtime`，启动完整的事件循环
- 使用 `mockito` 的异步 mock
- 使用 `tempfile` 创建临时数据库路径
- 测试之间完全隔离（不同端口、不同数据库）

### 需要新增的 dev-dependencies

- `portpicker` — 分配随机端口避免测试冲突

---

## 实现优先级

| 项目 | 优先级 | 原因 |
|------|--------|------|
| `cdn_base_url` 配置联动 | P0 | 简单修复，影响正确性 |
| 优雅关闭 | P0 | 生产必备，进程管理基础 |
| "正在输入" 状态 | P1 | 体验优化，非功能正确性 |
| 退出消息通知 | P1 | 体验优化，用户友好 |
| 集成测试套件 | P2 | 工程质量，长期维护 |
