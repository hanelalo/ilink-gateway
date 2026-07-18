# Claude Code Adapter 开发任务清单

## 项目结构

```
client/claude-code-adapter/
├── package.json
├── tsconfig.json
├── src/
│   ├── index.ts          # 入口：启动 → 注册 → poll loop + 消息路由
│   ├── gateway-client.ts # wechat-gateway API 客户端
│   ├── query-manager.ts  # 运行时查询管理：Map<cwd, RunningQuery>
│   ├── claude-session.ts # Claude SDK query 封装
│   ├── session-store.ts  # wxid → cwd → sessionId 持久化 + 白名单 + 别名
│   ├── approval.ts       # 工具审批逻辑 + 审批指令解析
│   ├── formatter.ts      # Markdown → 纯文本转换
│   └── config.ts         # 环境变量读取
```

## 环境信息

- Node.js v20.12.0, npm 10.5.0, TypeScript (tsx for dev)
- Claude Code v2.1.214, SDK: `@anthropic-ai/claude-agent-sdk@0.3.214` (固定版本)
- Gateway API: `POST /api/agents/register`, `GET /api/agents/{name}/poll`, `POST /api/agents/{name}/reply`

---

## Phase 1: 基础框架

### T1.1 项目初始化  ✅

- 创建 `client/claude-code-adapter/package.json`
  - type: "module", 依赖: `@anthropic-ai/claude-agent-sdk` (固定 `~0.3.214`)
  - devDeps: `typescript`, `@types/node`, `tsx`
  - scripts: `"dev": "tsx src/index.ts"`, `"build": "tsc"`, `"start": "node dist/index.js"`
- 创建 `client/claude-code-adapter/tsconfig.json`
  - target: ES2022, module: NodeNext, outDir: dist, strict: true

### T1.2 实现 `config.ts`  ✅

- 读取环境变量: `CLAUDE_GATEWAY_URL`, `CLAUDE_GATEWAY_AGENT_NAME`, `CLAUDE_MODEL`, `CLAUDE_CWD`, `CLAUDE_POLL_INTERVAL`, `CLAUDE_EFFORT`, `CLAUDE_SESSION_STORE_PATH`, `HTTP_PROXY`/`HTTPS_PROXY`
- 提供合理的默认值（如 gateway URL 默认 `http://127.0.0.1:8765`）

### T1.3 实现 `gateway-client.ts`  ✅

- 封装三个 HTTP 调用（用 Node.js 原生 `fetch()`）
  - `register()`: `POST /api/agents/register { name, capabilities: ["text"] }`
  - `poll()`: `GET /api/agents/{name}/poll` → AgentMessage[]
  - `reply(replyToId, text)`: `POST /api/agents/{name}/reply`
  - `sendProactive(toUser, text)`: 主动发送
- 类型定义对齐 Rust 端的 `AgentMessage` (`id`, `from_user`, `text`, `timestamp`, `context_token`, `message_type`, `media[]`)
- 404 自动重新注册

### T1.4 实现 `index.ts` 主循环（Phase 1 骨架）  ✅

- 启动流程: 读取 config → 创建 GatewayClient → 注册 → 进入 poll 循环
- 轮询到消息后打印日志
- 验证: 启动 adapter → 微信发消息 → 日志可见消息内容

---

## Phase 2: Claude SDK 集成与会话管理

### T2.1 实现 `session-store.ts`  ✅

- 数据模型匹配设计文档 §3.3: `wxid → { aliases, activeCwd, sessions: { cwd → { sessionId, lastActive, approvedTools } } }`
- `loadAll()` / `saveAll()` 方法
- 文件路径: `~/.wechat-gateway/claude-sessions.json`（expandTilde 解析 `~`）
- 单文件存储，所有 wxid 存一起

### T2.2 实现 `query-manager.ts`  ✅

- `Map<wxid, Map<cwd, RunningQuery>>`
- RunningQuery: `{ query, abortController, pendingApproval, messageQueue, replyBuffer }`
- 方法: `start()`, `get()`, `remove()`, `abort()`

### T2.3 实现 `claude-session.ts`  ✅

- 封装 SDK `query()` 调用
- 消息迭代: system/init → session_id 保存, assistant/text → 收集, assistant/tool_use → 触发审批回调, result → 结束
- 传入 `canUseTool` 回调（交给 approval.ts）
- 传入 `env`: `{ CLAUDE_CODE_ENTRYPOINT: 'remote_mobile', ...代理 }`

### T2.4 主循环消息路由  ✅

- poll 消息 → 拦截 adapter 指令 (/cd, /approve, /deny) → 根据 activeCwd 找 RunningQuery → 空闲则启动 / 运行中则排队

### T2.5 多轮对话  ✅

- 第二轮起用 `resume: sessionId` + `cwd: activeCwd`
- session 从 session-store 加载，传给 claude-session.ts

### T2.6 实现 `/cd` 命令  ✅

- `/cd` (无参数): 列出别名 + workspace 状态
- `/cd <target>`: 路径解析（别名 → 精确匹配 → 模糊匹配 → 绝对路径）
- `/cd + <alias> [<path>]`: 添加别名
- `/cd - <alias>`: 删除别名
- `/cd close <target>`: 关闭 workspace
- 回复格式: `**claude**:{basename}\n\n{content}`

### T2.7 跨目录并行  ✅

- 不同 cwd 的 query 独立运行，A 运行时 `/cd B` 启动 B 不中断 A

### T2.8 验证

- 微信两轮对话，Claude 记住上下文
- `/cd` 切换后 session 隔离
- A 运行时切 B，A 完成后结果不丢

---

## Phase 3: 审批机制 + 格式化 + 生产就绪

### T3.1 实现 `approval.ts`  ✅

- 指令解析: `/approve`, `/deny`, `/approve session`, `/approve on`, `/approve off`
- 审批流程: 白名单检查 → permissionMode 检查 → 发微信 → Promise 等待
- 60s 超时自动拒绝
- 首次 session 预填白名单: `['Read', 'Glob', 'Grep']`
- 常驻黑名单（SDK disallowedTools）: `['Bash(sudo:*)', 'Bash(rm -rf:*)', 'Bash(chmod:*)']`

### T3.2 审批指令路由  ✅

- poll 到的审批指令不传 SDK，直接在 adapter 消费
- `/approve`/`/deny` → 解析当前 activeCwd 的 pendingApproval
- `/approve on`/`/approve off` → 设置 permissionMode

### T3.3 实现 `formatter.ts`  ✅

- Markdown → 微信纯文本: 去掉 `**`、`*`、`` ` ``，代码块保留内容，列表 `-` → `·`，链接 `[文本](url)` → `文本 (url)`，标题 `#` → `【】`，删除分割线

### T3.4 流式合批  ✅

- buffer 超过 1500 字符 → 发送
- 收到 tool_use → 先发 buffer，再发工具通知
- 收到 result → 发剩余 buffer
- 2s 空闲 → 发 buffer

### T3.5 超时提示  ✅

- 30s 无 assistant 文本 → 发 "Claude 正在处理中..."

### T3.6 长回复分段  ✅

- 超过 3800 字符 → 分 `[1/3]`, `[2/3]` 等多段，间隔 500ms

### T3.7 优雅关闭  ✅

- SIGINT/SIGTERM: abort 当前 query, resolve 所有 pending 为 deny, 3s 后强制退出

### T3.8 异常处理  ✅

- `process.on('uncaughtException')` 和 `process.on('unhandledRejection')` 捕获

### T3.9 心跳/重连  ✅

- Gateway 断开自动重新注册
- poll 循环异常自愈

### T3.10 环境变量配置文档  ✅

- 所有环境变量在 README 中列出
- 示例启动命令

---

## 关键设计决策

1. **ESM 模块**：Node.js v20 原生支持
2. **原生 fetch()**：无需额外 HTTP 依赖
3. **SDK 版本固定**：`~0.3.214` 匹配 Claude Code v2.1.214
4. **消息合批**：1500 字符或 2s 空闲触发发送，避免大量小气泡
5. **只读工具默认放行**：Read/Glob/Grep 首次 session 自动加入白名单
6. **审批/对话指令不走 SDK**：直接在 adapter 层拦截处理
7. **CLAUDE_CODE_ENTRYPOINT**：设置为 `remote_mobile` 防止 SDK session 被终端 `claude --resume` 过滤
