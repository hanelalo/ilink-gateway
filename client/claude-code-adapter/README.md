# Claude Code Adapter

通过微信管理 Claude Code 工作会话的适配器。作为 wechat-gateway 的 agent，接收微信消息并转发到 Claude Code SDK，支持多用户、多 workspace 管理。

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
| `HTTP_PROXY` / `HTTPS_PROXY` | - | 代理配置 |

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

## 微信审批命令

| 命令 | 说明 |
|------|------|
| `/approve` | 批准当前工具调用 |
| `/deny` | 拒绝当前工具调用 |
| `/approve session` | 批准并在当前会话中记住该工具 |
| `/approve on` | 切换为自动审批模式 |
| `/approve off` | 切换为交互审批模式 |
| `/cd` | 查看所有 workspace |
| `/cd <名称>` | 切换 workspace |
| `/cd + <别名> [路径]` | 添加别名 |
| `/cd - <别名>` | 删除别名 |
| `/cd close <名称>` | 关闭 workspace |

## 项目结构

```
src/
├── index.ts              # 主入口：注册、轮询、消息路由
├── streaming-batcher.ts  # 流式输出合批（T3.4）、超时提示（T3.5）、长回复分段（T3.6）
├── session-store.ts      # 会话持久化（T2.5）
├── query-manager.ts      # 查询管理（T2.7 跨目录并行）
├── claude-session.ts     # Claude SDK session 封装
├── approval.ts           # 审批命令解析
├── formatter.ts          # Markdown 转纯文本
├── cd-command.ts         # /cd 命令处理
├── gateway-client.ts     # Gateway HTTP 客户端
└── config.ts             # 环境变量配置
```

### 核心特性

- **T2.5 多轮对话**: sessionId 通过 `onSessionInit` 回调持久化到 JSON 文件，下次消息自动 `resumeSessionId`
- **T2.7 跨目录并行**: 不同 cwd 的 RunningQuery 独立存储，`/cd` 切换不影响已在运行的 session
- **T3.4 流式合批**: 文本缓存到 buffer，>1500 字符自动 flush；收到 tool_use 先 flush buffer；result 到达 flush 剩余 buffer；2s 空闲自动 flush
- **T3.5 超时提示**: 30 秒无活动发送 "Claude 正在处理中..." 到微信，不中断 session
- **T3.6 长回复分段**: 超过 3800 字符分成 `[1/N]`, `[2/N]`... 多段发送，段间间隔 500ms
- **T3.7 优雅关闭**: SIGINT/SIGTERM 时 abort 所有 query、resolve 所有 pending approval 为 deny、3s 后 exit
- **T3.8 全局异常**: `uncaughtException` / `unhandledRejection` 全局处理，防止进程意外退出
- **T3.9 轮询自愈**: poll 出错时打印警告并继续，下次轮询自动重试

## 开发

```bash
# 安装依赖
npm install

# 运行测试
npx vitest run

# 持续测试
npx vitest

# 运行
npx tsx src/index.ts
```
