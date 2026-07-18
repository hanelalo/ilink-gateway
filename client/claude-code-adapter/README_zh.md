# Claude Code Adapter

通过微信管理 Claude Code 工作会话的适配器。作为 wechat-gateway 的 agent，接收微信消息并转发到 Claude Code SDK，支持多用户、多 workspace 管理。

**状态**: 开发完成，共 189 个单元测试，覆盖所有模块。

## 前置要求

- Node.js >= 18
- [Claude Code](https://claude.ai/code) CLI (`npm install -g @anthropic-ai/claude-code`)
- Claude API Key (配置在 Claude Code 中)
- [wechat-gateway](https://github.com/hanelalo/wechat-gateway) 运行中

## 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `CLAUDE_GATEWAY_URL` | `http://127.0.0.1:8765` | gateway HTTP API 地址 |
| `CLAUDE_GATEWAY_AGENT_NAME` | `claude` | 注册的 agent 名称 |
| `CLAUDE_MODEL` | `sonnet` | Claude 模型 |
| `CLAUDE_CWD` | `process.cwd()` | 默认工作目录 |
| `CLAUDE_POLL_INTERVAL` | `1000` | 轮询间隔(毫秒) |
| `CLAUDE_EFFORT` | `medium` | Claude Code effort 级别 |
| `CLAUDE_SESSION_STORE_PATH` | `~/.wechat-gateway/claude-sessions.json` | 会话持久化路径 |
| `HTTP_PROXY` / `HTTPS_PROXY` | - | HTTP 代理配置 |

## 快速启动

```bash
# 1. 配置 Claude Code（如果尚未完成）
claude

# 2. 启动 gateway
cd /path/to/wechat-gateway/gateway
cargo run

# 3. 启动 adapter
cd /path/to/wechat-gateway/client/claude-code-adapter
npx tsx src/index.ts
```

## 微信交互命令

在微信中发送以下命令控制 adapter：

| 命令 | 说明 |
|------|------|
| **Workspace 管理** | |
| `/cd` | 查看所有 workspace 和别名 |
| `/cd <名称>` | 切换 workspace（支持别名、路径 basename、绝对路径） |
| `/cd + <别名> [路径]` | 添加别名（不指定路径则使用当前 workspace） |
| `/cd - <别名>` | 删除别名 |
| `/cd close <名称>` | 关闭 workspace（中止运行中的 query 并清除 session） |
| **工具审批** | |
| `/approve` | 批准当前工具调用 |
| `/deny` | 拒绝当前工具调用 |
| `/approve session` | 批准并将该工具加入 session 白名单 |
| `/approve on` | 切换为自动审批模式（不再询问工具权限） |
| `/approve off` | 切换回交互审批模式 |

## 消息路由流程

```
微信消息 → gateway → poll → adapter 收到消息
  ├── /cd 命令 → handleCdCommand() → 直接处理并回复
  ├── /approve|/deny → 解析审批指令 → 处理 pendingApproval 或切换模式
  └── 其他文本 → 根据 activeCwd 查找 RunningQuery
        ├── 运行中 → 入队 messageQueue（排队等待）
        └── 空闲 → 启动 Claude Code session
              ├── 首次 → query({ prompt, cwd }) → onSessionInit 保存 sessionId
              └── 恢复 → query({ prompt, cwd, resume: sessionId })
```

## 项目结构

```
src/
├── index.ts              # 入口：注册、轮询、消息路由、进程生命周期
├── config.ts             # 环境变量配置加载
├── gateway-client.ts     # Gateway HTTP API 封装
├── session-store.ts      # Session 持久化（wxid → cwd → sessionId）
├── query-manager.ts      # 运行时查询状态管理
├── claude-session.ts     # Claude SDK query() 封装
├── cd-command.ts         # /cd 命令处理（路径解析、别名管理）
├── approval.ts           # 审批命令解析 + 超时
├── formatter.ts          # Markdown 转纯文本
├── streaming-batcher.ts  # 流式合批 + 超时提示 + 长回复分段
│
# 测试文件（.test.ts 与源码一一对应）
├── config.test.ts
├── gateway-client.test.ts
├── session-store.test.ts
├── query-manager.test.ts
├── claude-session.test.ts
├── cd-command.test.ts
├── approval.test.ts
├── formatter.test.ts
├── streaming-batcher.test.ts
├── index.test.ts
```

## 核心特性

- **多轮对话**: sessionId 通过 `onSessionInit` 回调持久化到 JSON 文件，下次消息自动 `resumeSessionId`，保持完整上下文
- **跨目录并行**: 不同 cwd 的 RunningQuery 独立存储（`Map<wxid, Map<cwd, RunningQuery>>`），`/cd` 切换不影响已在运行的 session
- **流式合批**: 文本缓存到 buffer，>1500 字符自动 flush；收到 tool_use 先 flush buffer；result 到达 flush 剩余 buffer；2s 空闲自动 flush
- **超时提示**: 30 秒无活动发送 "Claude 正在处理中..." 到微信，不中断 session
- **长回复分段**: 超过 3800 字符分成 `[1/N]`, `[2/N]`... 多段发送，段间间隔 500ms
- **工具审批**: 交互审批（微信内 `/approve`/`/deny`）+ session 白名单 + 自动模式；60s 超时自动拒绝
- **安全控制**: 常驻黑名单 `Bash(sudo:*)`, `Bash(rm -rf:*)`, `Bash(chmod:*)`；首次 session 预填 `Read/Glob/Grep` 白名单
- **消息排队**: 同一 wxid + cwd 的后续消息自动排队，当前 session 结束后依次处理
- **优雅关闭**: SIGINT/SIGTERM 时 abort 所有 query、resolve 所有 pending approval 为 deny、3s 后 exit
- **全局异常**: `uncaughtException` / `unhandledRejection` 全局处理，防止进程意外退出
- **轮询自愈**: poll 出错时打印警告并继续，下次轮询自动重试；404 自动重新注册

## 测试

使用 Vitest 测试框架。所有测试不依赖外部服务（SDK 和 HTTP 皆注入 mock）。

```bash
# 运行全部测试
npm test

# 持续测试
npm run test:watch

# 覆盖测试
npx vitest run --coverage
```

## 开发

```bash
# 安装依赖
npm install

# 运行
npx tsx src/index.ts

# 构建
npm run build
```
