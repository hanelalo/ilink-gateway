# Claude Code Adapter for wechat-gateway — 方案设计文档

> **目标**：让 Claude Code 作为 wechat-gateway 的第二个 agent（与 Hermes 并列），通过微信与 Claude Code 直接对话。
>
> **对标参考**：Happy Coder（`slopus/happy`）的 Remote 模式，使用 `@anthropic-ai/claude-agent-sdk` 驱动 Claude Code。
>
> **阅读对象**：开发者。本文用流程图 + 描述的方式讲清楚需求、实现细节、坑点、开发任务。

---

## 1. 整体架构

```
微信 ─── iLink ──→ wechat-gateway (Rust, :8765)
                        │
            ┌───────────┴───────────┐
            │                       │
     Hermes Plugin              Claude Adapter   ← 本项目
     (Python, poll)             (Node.js, poll)
            │                       │
       Hermes Agent         @anthropic-ai/claude-agent-sdk
                                     │
                               Claude Code
```

**Claude Adapter 和 Hermes Plugin 是平级关系**，都是 wechat-gateway 的 agent，共享同一套注册/轮询/回复 API。

### 与 Happy 的差异

| | Happy | 本方案 |
|---|---|---|
| 消息入口 | 手机 App → Happy Server | 微信 → wechat-gateway |
| SDK 驱动 | `query()` 异步迭代 | 同，`query()` 异步迭代 |
| 会话映射 | 1 Happy user = 1 Claude session | 1 微信用户 × 1 工作目录 = 1 Claude session |
| 权限审批 | 手机 App 弹窗让用户点 | 微信内交互审批（`/approve`/`/deny`），可切换自动模式 |
| 消息格式 | Markdown（App 原生渲染） | 纯文本（微信不支持 Markdown） |
| 工作目录 | 启动时固定 cwd | `/cd` 命令随时切换，每个目录独立 session |

---

## 2. 交互流程

### 2.1 启动流程

```
┌──────────────────┐        ┌──────────────────┐        ┌──────────────────┐
│  wechat-gateway  │        │  Claude Adapter   │        │  Claude SDK      │
│     (Rust)       │        │    (Node.js)      │        │  (TypeScript)    │
└────────┬─────────┘        └────────┬─────────┘        └────────┬─────────┘
         │                           │                           │
         │  ① POST /api/agents/register                          │
         │     { "name": "claude" }  │                           │
         │←──────────────────────────│                           │
         │  ② 200 { "ok": true }     │                           │
         │──────────────────────────→│                           │
         │                           │                           │
         │                           │  ③ 启动 poll loop         │
         │                           │  setInterval 1s           │
         │                           │                           │
         │  ④ GET /api/agents/claude/poll                        │
         │←──────────────────────────│  (心跳 + 拉消息)          │
         │  ⑤ { "messages": [] }     │                           │
         │──────────────────────────→│                           │
```

**步骤说明：**

1. **注册**：adapter 启动时 `POST /api/agents/register` 向 gateway 注册为 `"claude"`
2. **心跳**：每 1 秒 `GET /api/agents/{name}/poll`，既是拉消息也是心跳（gateway 通过 poll 时间戳判断 agent 在线）
3. **首次注册时**：gateway 若没有活跃 agent，会自动将 Claude 设为活跃 agent；用户可通过微信 `/use hermes` 切换

### 2.2 单轮对话流程（核心路径）

```
微信用户                 wechat-gateway            Claude Adapter            Claude SDK
   │                         │                         │                         │
   │  "帮我写个脚本"          │                         │                         │
   │────────────────────────→│                         │                         │
   │                         │  解析非命令消息          │                         │
   │                         │  → Router.handle_incoming│                        │
   │                         │  → 入队到 claude 队列    │                         │
   │                         │                         │                         │
   │                         │  GET /api/agents/claude/poll                      │
   │                         │←────────────────────────│  (1s 间隔轮询)          │
   │                         │  返回这条消息            │                         │
   │                         │────────────────────────→│                         │
   │                         │                         │                         │
   │                         │                         │  ① 查找或创建 session   │
   │                         │                         │  sessionMap[wxid]       │
   │                         │                         │  → resume: sessionId    │
   │                         │                         │  或 新建 query()        │
   │                         │                         │                         │
   │                         │                         │  ② SDK query({          │
   │                         │                         │      prompt: "帮我..."  │
   │                         │                         │      resume: "uuid"     │
   │                         │                         │    })                   │
   │                         │                         │────────────────────────→│
   │                         │                         │                         │
   │                         │                         │  ③ SDK 返回消息流       │
   │                         │                         │  for await (msg) {      │
   │                         │                         │    if assistant_text:   │
   │                         │                         │      collect reply      │
   │                         │                         │    if tool_use:         │
   │                         │                         │      auto approve       │
   │                         │                         │    if result:           │
   │                         │                         │      done               │
   │                         │                         │  }                      │
   │                         │                         │←────────────────────────│
   │                         │                         │                         │
   │                         │                         │  ④ 收集完整回复文本      │
   │                         │                         │  清理 Markdown 标记      │
   │                         │                         │                         │
   │                         │  POST /api/agents/claude/reply                   │
   │                         │  { reply_to_id, text } │                         │
   │                         │←────────────────────────│                         │
   │                         │                         │                         │
   │  "这是脚本：..."         │                         │                         │
   │←────────────────────────│                         │                         │
```

**步骤说明：**

1. **用户发消息到微信** → gateway 长轮询获取 → 路由器判断不是 `/` 命令 → 入队到 `claude` 的消息队列
2. **adapter 轮询到消息** → 提取 `id`, `from_user`, `text`
3. **SDK 会话管理**（见 §3.3）
4. **发送回复** → `POST /api/agents/claude/reply`，gateway 内部通过 channel → iLink sendmessage → 微信

### 2.3 多轮对话流程

```
第一轮:
  用户: "分析这个项目结构"
  Claude: 回复 + 可能执行了 Bash/Read 工具
  → sessionId 持久化到 Claude 的 ~/.claude/projects/<hash>/<uuid>.jsonl

第二轮:
  用户: "那个函数在哪里定义的？"
  → adapter 发现 wxid 已有 session
  → query({ prompt: "那个函数...", resume: "已保存的uuid" })
  → Claude SDK 恢复完整上下文（包括第一轮的工具调用结果）
  → 继续对话
```

### 2.4 切换工作目录流程

```
微信用户                  adapter

  "/cd gw"
  ────────────────────────→
                           ① 解析 /cd 指令:
                              target = "gw"

                           ② 查 aliases:
                              aliases["gw"] → "/Users/.../wechat-gateway"

                           ③ 检查 sessions[wxid][cwd] 是否存在:
                              ├── 存在 → 加载已有 sessionId
                              └── 不存在 → 标记，下次消息时创建新 session

                           ④ 更新 activeCwd:
                              activeCwd = "/Users/.../wechat-gateway"

                           ⑤ 回复确认
  "**claude**:wiki
                                ←────────────────────────
   已切换到 wechat-gateway
   [session: uuid-xxx]
   /cd 可查看所有项目"

  "/cd"
  ───────────────────────→
                           ① 无参数 = 列出状态
                           ② 收集数据:
                              activeCwd
                              sessions 中所有 cwd
                              aliases 映射表
                           ③ 格式化回复
  "**claude**:wiki
                                ←────────────────────────
   可用: gw, game-site
   
   所有 workspace:
   · wiki [uuid-1...] 2h前 🟢运行中 ← 当前
   · wechat-gateway [uuid-2...] 3天前"
```

### 2.5 切换 agent 流程

```
用户: /use hermes
  → gateway 内置命令处理，切换到 hermes agent
  → 后续消息路由到 hermes

用户: /use claude
  → gateway 切换到 claude agent
  → 后续消息路由到 Claude Adapter

用户: /list
  → gateway 返回已注册 agent 列表

用户: /status
  → gateway 返回连接状态 + agent 列表
```

---

## 3. API 详细说明

### 3.1 wechat-gateway API（adapter 需要调用的）

所有 API 基础 URL：`http://127.0.0.1:8765`

#### 注册

```
POST /api/agents/register
Content-Type: application/json

请求体:
{
  "name": "claude",            // agent 名称，唯一标识
  "capabilities": ["text"]     // 能力声明
}

成功响应 (200):
{
  "ok": true,
  "active_agent": "claude",    // 当前活跃 agent（首次注册会自动设为活跃）
  "wechat_connected": false    // 微信连接状态（长轮询连接）
}

失败响应 (400):
{
  "ok": false,
  "error": "Agent with name 'claude' already registered"
}
```

**注意事项：**
- 重名注册不会报错，而是更新 `last_seen` 时间戳（相当于心跳刷新）
- 如果当前没有活跃 agent，首次注册的 agent 会自动被设为活跃

#### 轮询消息

```
GET /api/agents/{name}/poll

成功响应 (200):
{
  "messages": [
    {
      "id": "msg-uuid",                 // 消息 ID，回复时需要
      "from_user": "wxid_abc123",       // 微信用户 ID
      "text": "帮我写个脚本",            // 消息文本
      "timestamp": 1720000000,          // Unix 时间戳（秒）
      "context_token": "base64...",     // iLink 上下文 token
      "message_type": "text",           // text | image | voice | video | file
      "media": [                        // 媒体附件（可选）
        {
          "media_type": "image",
          "local_path": "/tmp/xxx.jpg",
          "original_name": "photo.jpg"
        }
      ]
    }
  ]
}

404 响应 (agent 未注册):
{
  "error": "Agent 'claude' not found"
}
```

**注意事项：**
- `from_user` 是微信用户 ID，格式为 `wxid_xxxxxxxx`，作为会话映射的 key
- 每次 poll 都会更新该 agent 的心跳时间戳（gateway 用 30s 检查 / 60s 超时）
- 消息队列是 FIFO：poll 返回后消息出队

#### 回复消息

```
POST /api/agents/{name}/reply
Content-Type: application/json

请求体:
{
  "reply_to_id": "msg-uuid",        // 必须: 要回复的消息 ID
  "text": "这是回复内容",            // 回复文本
  "media_paths": [],                // 可选: 媒体文件路径
  "to_user": null,                  // 可选: 主动发送时指定目标用户
  "context_token": null             // 可选: 主动发送时的上下文 token
}

成功响应 (200):
{ "ok": true }
```

**主动发送**（用于通知等无上下文的场景）：
```
{
  "reply_to_id": "",
  "text": "通知内容",
  "to_user": "wxid_abc123",
  "context_token": ""
}
```

### 3.2 @anthropic-ai/claude-agent-sdk API

#### 安装

```bash
npm install @anthropic-ai/claude-agent-sdk
```

SDK 版本跟随 Claude Code 安装。当前系统 Claude Code 版本：**v2.1.214**。

#### 核心 API: `query()`

```typescript
import { query, type Options, type Query } from '@anthropic-ai/claude-agent-sdk';

const q: Query = query({
  prompt: string | AsyncIterable<SDKUserMessage>,
  options: Options
});
```

**`Options` 关键字段：**

| 字段 | 类型 | 说明 |
|------|------|------|
| `cwd` | `string` | 工作目录，Claude 在此目录下操作文件 |
| `model` | `string` | 模型名，如 `"sonnet"`, `"opus"` |
| `resume` | `string?` | 恢复已有 session 的 UUID |
| `continue` | `boolean?` | 继续最近的 session |
| `permissionMode` | `string` | `"default"` \| `"acceptEdits"` \| `"bypassPermissions"` \| `"plan"` |
| `allowedTools` | `string[]?` | 白名单工具，如 `["Bash(git:*)", "Read"]` |
| `disallowedTools` | `string[]?` | 黑名单工具 |
| `maxTurns` | `number?` | 最大对话轮数 |
| `systemPrompt` | `object?` | 自定义 system prompt |
| `canUseTool` | `function?` | 工具调用回调，返回 `PermissionResult` |
| `abortController` | `AbortController?` | 取消当前 query |
| `env` | `Record<string,string>?` | 环境变量 |
| `effort` | `string?` | `"low"` \| `"medium"` \| `"high"` \| `"xhigh"` \| `"max"` |
| `mcpServers` | `object?` | MCP 服务器配置 |

#### 消息迭代

```typescript
for await (const msg of q) {
  switch (msg.type) {
    case 'system':
      // msg.subtype === 'init' → session_id, tools, slash_commands
      break;
    case 'assistant':
      // msg.message.content → [{type:'text', text:'...'}, {type:'tool_use', ...}]
      break;
    case 'user':
      // 工具调用结果，role='user', content 包含 tool_result
      break;
    case 'result':
      // 本轮结束，msg.subtype 可能是 'success' | 'error_max_turns' 等
      break;
  }
}
```

**四种消息类型：**

| 类型 | 何时出现 | 内容 |
|------|----------|------|
| `system` | 会话初始化时 | `subtype: "init"` → `session_id`, `model`, `tools`, `slash_commands` |
| `assistant` | Claude 每次回复 | `message.content[]` — 文本块 + 工具调用块 |
| `user` | 工具执行完后 | `message.content[]` — `tool_result` 块 |
| `result` | 本轮对话结束 | `subtype`, `usage`, `stop_reason` |

#### `canUseTool` 回调

```typescript
canUseTool: async (toolName, input, options) => {
  // options: { toolUseID, signal, agentID?, ... }
  return {
    behavior: 'allow',           // 'allow' | 'deny'
    updatedInput: input,          // 可修改工具参数
    permissions: {                // 权限覆盖
      additionalDirectories: [],
    }
  };
}
```

adapter 场景下，我们始终返回 `{ behavior: 'allow' }`（信任环境）。

#### 关键环境变量

```typescript
env: {
  CLAUDE_CODE_ENTRYPOINT: 'remote_mobile',  // 防止 SDK session 被 claude --resume 隐藏
}
```

Happy 的教训（slopus/happy#1202）：SDK 默认给 spawned Claude 进程设置 `CLAUDE_CODE_ENTRYPOINT="sdk-ts"`，导致这些 session 在 `claude --resume` 的交互式 picker 中不可见。设置 `"remote_mobile"` 可以保持可见性。

### 3.3 Claude Session 持久化

Claude SDK 的 session 以 JSONL 文件存储在：

```
~/.claude/projects/<cwd-path-hash>/<session-uuid>.jsonl
```

- 每行一个 JSON 对象（消息记录）
- SDK 的 `resume` 选项会读取这个文件恢复上下文
- adapter 需要维护 **wxid → cwd → sessionId** 的映射

**映射存储策略（`~/.wechat-gateway/claude-sessions.json`）**：

```json
{
  "wxid_abc123": {
    "aliases": {
      "wiki": "/Users/hanelalo/develop/wiki",
      "gw": "/Users/hanelalo/develop/wechat-gateway"
    },
    "activeCwd": "/Users/hanelalo/develop/wiki",
    "sessions": {
      "/Users/hanelalo/develop/wiki": {
        "sessionId": "uuid-1",
        "lastActive": 1720000000,
        "approvedTools": ["Bash", "Read"]
      },
      "/Users/hanelalo/develop/wechat-gateway": {
        "sessionId": "uuid-2",
        "lastActive": 1720000001,
        "approvedTools": ["Glob"]
      }
    }
  }
}
```

**字段说明**：

| 字段 | 层级 | 说明 |
|------|------|------|
| `aliases` | wxid | 目录别名字典，`{ 别名: 绝对路径 }` |
| `activeCwd` | wxid | 当前活跃工作目录（绝对路径） |
| `sessions` | wxid/cwd | key 为绝对路径，value 为 session 数据 |
| `sessionId` | cwd | Claude SDK session UUID，首次为空，`system/init` 后填入 |
| `lastActive` | cwd | 上次消息时间戳，用于排序和过期清理 |
| `approvedTools` | cwd | `/approve session` 积累的工具白名单 |

---

## 4. 关键技术细节

### 4.1 会话管理策略

```
每个微信用户 (wxid) × 每个工作目录 (cwd) → 一个 Claude Code session

初始化:
  wxid 在 cwd 首次出现 → query({ prompt: "...", cwd: path })
  → system/init 消息 → 拿到 session_id → 保存到 sessions[cwd].sessionId

恢复:
  wxid + cwd 已存在 → query({ prompt: "...", cwd: path, resume: savedSessionId })
  → SDK 从 JSONL 恢复完整上下文（包括工具调用历史）

切换 cwd:
  /cd <alias> → 更新 activeCwd
  → 下次消息时根据 activeCwd 查 session
  → 已有 session 则 resume，否则新建

过期清理:
  7 天未活跃的 session → 删除 sessions[cwd]（不删除 Claude JSONL 文件）
```

**为什么不用单 session + 上下文注入？**
- Claude session 的 JSONL 包含完整工具调用结果，无法通过纯文本注入
- 不同目录下的 session 天然隔离，各自的工具调用历史和上下文互不干扰
- `/cd` 切换后 resume 对应 session，和本地 `claude --resume <uuid>` 体验一致

### 4.2 Claude 回复文本提取

Claude SDK 的消息流中，文本内容分布在多个 `assistant` 消息块中：

```
流程:
  收到 assistant 消息
  ├── 遍历 msg.message.content[]
  │   ├── { type: "text", text: "..." } → 追加到回复缓冲区
  │   └── { type: "tool_use", name: "Bash", ... } → canUseTool 自动批准
  └── 收到 result 消息 → 本轮结束，发送回复缓冲区
```

**注意**：一次 `query()` 调用中可能产生多个 assistant 消息（Claude 执行多步工具调用），需要累积所有 `text` 块。

### 4.3 微信回复格式化

微信不支持 Markdown。adapter 需要将 Claude 的 Markdown 回复转换为纯文本：

| Claude 输出 | 微信显示 |
|-------------|----------|
| `**粗体**` | `粗体` |
| `*斜体*` | `斜体` |
| `` `code` `` | `code` |
| ` ```代码块``` ` | `[代码]` 或原文保留 |
| `- 列表项` | `· 列表项` |
| `[链接](url)` | `链接 (url)` |
| `# 标题` | `【标题】` |

**策略**：做轻量级清理，不强求完美渲染。核心原则：**信息不丢失、可读**。

### 4.4 长时间运行的处理

Claude 可能在工具执行中花费较长时间（比如运行 `npm install`）。

```
adapter 处理:
  ① 收到用户消息 → poll loop 暂停（避免并发问题）
  ② query() 启动 → for await 迭代 SDK 消息
  ③ 如果超过 30 秒无新消息 → 发送 "Claude 正在处理中..." 到微信
  ④ result 到达 → 发送完整回复 → poll loop 恢复
```

**并发控制**：同一 wxid + 同一 cwd 的请求串行处理（排队）。同一 wxid + 不同 cwd 的请求可并行——各自独立的 queryManager entry，由 `queryManager.get(wxid, cwd)` 区分。

### 4.5 工具调用审批机制

adapter 默认以 **交互审批模式** 启动（`permissionMode: "default"`），Claude 每次调用工具前，通过微信询问用户。用户可随时切换为自动审批。

#### 审批命令

| 命令 | 作用域 | 效果 |
|------|--------|------|
| `/approve on` | 全局 | 切换为自动审批，此后所有工具不再询问 |
| `/approve off` | 全局 | 恢复交互审批，每次工具调用都询问 |
| `/approve` | 当前工具 | 批准这一次的工具调用 |
| `/deny` | 当前工具 | 拒绝这一次的工具调用 |
| `/approve session` | 当前 session | 批准本次调用，并将此**工具名**加入 session 白名单，后续自动放行 |

**"工具名"的定义**：取 Claude 工具名称，如 `Bash`、`Write`、`Read`、`Edit`、`Glob`、`Grep` 等。

#### 审批流程图

```
Claude 想调用 Bash: npm install react
        │
        ▼
┌─────────────────────────────────────────────────┐
│         canUseTool(toolName, input)              │
│                                                  │
│  ① 检查 session 白名单: toolName 在名单中?       │
│     ├── 是 → return { behavior: 'allow' }       │
│     └── 否 ↓                                    │
│                                                  │
│  ② 检查 permissionMode: bypassPermissions?       │
│     ├── 是 → return { behavior: 'allow' }       │
│     └── 否 ↓                                    │
│                                                  │
│  ③ 发送审批请求到微信                             │
│     "Claude 想执行 Bash: npm install react       │
│      回复: /approve 批准  /deny 拒绝             │
│             /approve session 记住并批准"          │
│                                                  │
│  ④ 创建 Promise，等待用户回复                     │
│     canUseTool 是 async，return 前 SDK 阻塞      │
│                                                  │
│  ⑤ 用户回复到达:                                 │
│     /approve         → resolve allow             │
│     /deny            → resolve deny              │
│     /approve session → resolve allow             │
│                        + toolName 加入白名单       │
│     /approve on      → resolve allow             │
│                        + setPermissionMode(       │
│                            bypassPermissions)     │
│                                                  │
│  ⑥ 超时处理（60s 无回复）:                        │
│     发微信 "审批超时，已自动拒绝"                  │
│     → resolve deny                               │
└─────────────────────────────────────────────────┘
```

#### 并发处理的关键细节

审批指令（`/approve`、`/deny`、`/approve on`、`/approve off`、`/approve session`）和 `/cd` 一样是 adapter 层面的控制指令，由主循环拦截，不传给 Claude SDK。完整的消息路由逻辑见 §4.7。

#### session 白名单存储

白名单是每个 session（即每个 cwd）的属性，在 `claude-sessions.json` 中与 sessionId 存在一起：

```json
{
  "wxid_abc123": {
    "activeCwd": "/Users/hanelalo/develop/wiki",
    "sessions": {
      "/Users/hanelalo/develop/wiki": {
        "sessionId": "uuid-xxx",
        "lastActive": 1720000000,
        "approvedTools": ["Bash", "Read", "Glob"]   // ← 白名单
      }
    }
  }
}
```

白名单生命周期 = session 生命周期。`/new` 新建 session 时清空白名单。

#### 默认安全策略

```typescript
// 启动默认值
permissionMode: 'default'         // 交互审批模式（不是 bypassPermissions）

// 首次创建 session 时白名单预填安全工具
approvedTools: ['Read', 'Glob', 'Grep']  // 只读工具默认放行

// 始终黑名单（即使 /approve session 也不生效）
disallowedTools: ['Bash(sudo:*)', 'Bash(rm -rf:*)', 'Bash(chmod:*)']
```

**设计理由**：`Read`/`Glob`/`Grep` 是纯只读操作，没有安全风险，默认放行减少审批噪音。`Bash` 和 `Write` 等修改操作默认需要审批。

### 4.6 工作目录切换（`/cd` 命令）

Claude Code 是项目级工具，需要在具体项目目录下运行。`/cd` 命令让用户在微信上切换 Claude 的工作目录，每个目录维护独立 session。

#### `/cd` 命令格式

| 命令 | 效果 |
|------|------|
| `/cd <alias>` | 切换到别名对应的目录 |
| `/cd <绝对路径>` | 切换到指定目录（无别名时） |
| `/cd` | 列出当前状态：别名表 + 所有 workspace 及其状态 |
| `/cd + <alias>` | 将当前 activeCwd 注册为别名 |
| `/cd + <alias> <路径>` | 将指定路径注册为别名 |
| `/cd - <alias>` | 删除别名 |
| `/cd close <alias>` | 关闭指定 workspace：中止正在运行的 query，清除 session 映射 |

**默认别名**：启动时自动将 `process.cwd()` 注册为 `"."`。

#### `/cd` 切换流程（实现细节）

```
入口: adapter 主循环中 poll 到消息

  function handleMessage(msg):
    text = msg.text.trim()
    
    // ── 1. 拦截 /cd 指令 ──
    if text starts with "/cd":
      → handleCd(msg.from_user, text)
      → return  (不传给 Claude SDK)

    // ── 2. 普通消息，根据 activeCwd 路由 ──
    cwd = sessions[wxid].activeCwd
    sessionData = sessions[wxid].sessions[cwd]
    query({ prompt: text, cwd: cwd, resume: sessionData?.sessionId })


  function handleCd(wxid, text):
    parts = text.split(/\s+/)
    // parts[0] = "/cd"

    if parts.length === 1:
      // /cd — 列出状态
      return formatStatus(wxid)

    if parts[1] === "+":
      // /cd + <alias> [<path>]
      return addAlias(wxid, parts[2], parts[3])

    if parts[1] === "-":
      // /cd - <alias>
      return removeAlias(wxid, parts[2])

    if parts[1] === "close":
      // /cd close <target>
      return closeWorkspace(wxid, parts[2])

    // /cd <target> — 切换目录
    return switchCwd(wxid, parts[1])
```

##### 步骤 1: 路径解析

```
  function resolvePath(wxid, target):
    // ① 查别名
    if target in sessions[wxid].aliases:
      return sessions[wxid].aliases[target]
    
    // ② 精确匹配已有 session 的 cwd
    for (cwd of sessions[wxid].sessions):
      if cwd === target: return cwd
    
    // ③ 模糊匹配已有 session 的目录名
    for (cwd of sessions[wxid].sessions):
      if basename(cwd) === target: return cwd
    
    // ④ 当作绝对路径
    if target starts with "/":
      // 安全检查: 路径必须存在且是目录
      if existsSync(target) && statSync(target).isDirectory():
        return target
      return error("路径不存在或不是目录: " + target)
    
    return error("未找到项目: " + target)
```

##### 步骤 2: 切换目录

```
  function switchCwd(wxid, target):
    path = resolvePath(wxid, target)
    if path is error: return error message
    
    // 更新 activeCwd
    sessions[wxid].activeCwd = path
    
    // 检查是否已有 session
    if sessions[wxid].sessions[path]:
      sessionId = sessions[wxid].sessions[path].sessionId
      lastActive = formatTime(sessions[wxid].sessions[path].lastActive)
      reply = "**claude**:{basename(path)}\n\n已切换到 {basename(path)} [{sessionId.slice(0,8)}...]\n上次活跃: {lastActive}"
    else:
      reply = "**claude**:{basename(path)}\n\n已切换到 {basename(path)} [新会话]\n下次消息时将创建 Claude session"
    
    // 写文件
    await writeSessionsFile()
    return reply
```

##### 步骤 3: 列出状态

```
  function formatStatus(wxid):
    data = sessions[wxid]
    active = data.activeCwd
    aliasList = Object.entries(data.aliases)
      .filter(([name, path]) => name !== ".")
      .map(([name]) => name)
      .join(", ")
    
    lines = [
      "**claude**:{basename(active)}\n",
      "当前: {basename(active)}  /cd 可切换项目",
      aliasList ? "可用: {aliasList}" : "",
      "",
      "所有 workspace:"
    ]
    
    for ([cwd, session] of sorted by lastActive desc):
      running = queryManager.get(wxid, cwd) ? "🟢运行中" : ""
      marker = cwd === active ? " ← 当前" : ""
      lastTime = formatRelative(session.lastActive)  // "2h前" / "3天前"
      sid = session.sessionId?.slice(0,8) || "新"
      lines.push("  · {basename(cwd)} [{sid}] {lastTime} {running}{marker}")
    
    lines.push("", "命令: /cd name 切换  /cd +name 别名  /cd close name 关闭")
    return lines.join("\n")
```

#### 别名管理

```
  function addAlias(wxid, name, explicitPath):
    path = explicitPath || sessions[wxid].activeCwd
    
    // 安全检查名称
    if name is empty or contains "/" or " ":
      return error("别名只能包含字母、数字、连字符")
    
    sessions[wxid].aliases[name] = path
    await writeSessionsFile()
    return "**claude**:{basename(path)}\n\n已添加别名: {name} = {path}"


  function removeAlias(wxid, name):
    if name not in sessions[wxid].aliases:
      return error("别名不存在: " + name)
    if name === ".":
      return error("不能删除默认别名 '.'")
    delete sessions[wxid].aliases[name]
    await writeSessionsFile()
    return "**claude**:{basename(path)}\n\n已删除别名: " + name


  function closeWorkspace(wxid, target):
    path = resolvePath(wxid, target)
    if path is error: return error message
    
    // 检查是否有正在运行的 query
    running = queryManager.get(wxid, path)
    if running:
      running.abortController.abort()     // 中止 SDK query
      queryManager.remove(wxid, path)
    
    // 从 sessions 中删除
    if sessions[wxid].sessions[path]:
      delete sessions[wxid].sessions[path]
    
    // 如果关闭的是当前 activeCwd，自动切到下一个
    if sessions[wxid].activeCwd === path:
      remaining = Object.keys(sessions[wxid].sessions)
      if remaining.length > 0:
        sessions[wxid].activeCwd = remaining[0]
    
    await writeSessionsFile()
    return "**claude**:{basename(path)}\n\n已关闭 workspace: {basename(path)}"
```

#### 首次消息时创建 session

```
  function handleMessage(msg):
    // ... 拦截 /cd /approve 等指令 ...
    
    cwd = sessions[wxid].activeCwd
    sessionData = sessions[wxid].sessions[cwd]
    
    // 如果此 cwd 还没有 session
    if !sessionData || !sessionData.sessionId:
      // 新建 entry（如果还没建）
      if !sessionData:
        sessions[wxid].sessions[cwd] = {
          sessionId: null,
          lastActive: Date.now(),
          approvedTools: ["Read", "Glob", "Grep"]  // 默认只读白名单
        }
      
      // 调用 SDK 新建 session
      query({ prompt: text, cwd: cwd })
      // system/init 消息到达时:
      //   sessions[wxid].sessions[cwd].sessionId = init.session_id
      //   await writeSessionsFile()
    else:
      // 已有 session，resume
      query({ prompt: text, cwd: cwd, resume: sessionData.sessionId })
```

#### 并发与边界处理

| 场景 | 处理 |
|------|------|
| `/cd` 时当前正在运行 query | `/cd` 切换 activeCwd 并回复确认，但**不影响正在运行的 query**（它已经用旧 cwd 启动了）。下次消息使用新 cwd。 |
| `/cd close` 关闭正在运行的 workspace | `abortController.abort()` 中止 query，删除 sessions 映射。不删除 Claude JSONL 文件。如果关闭的是当前 activeCwd，自动切到下一个 workspace。 |
| `/cd` 到一个不存在的路径 | resolvePath 第④步检查 `existsSync`，返回错误提示 |
| 别名冲突（多个别名指向同一路径） | 允许，不需要去重 |
| 两个用户同时操作同一个 wxid | 不会发生——adapter 单进程、同 wxid 串行 |
| adapter 重启 | 从 JSON 恢复 aliases、activeCwd、sessions，无需重连 |

### 4.7 跨目录并行执行

当用户 `/cd` 从 A 切换到 B 时，A 的 `query()` 继续运行。不同 cwd 的 query 是独立的子进程，互不干扰。

#### 运行时状态模型

持久化存储（JSON 文件）只存稳定状态，运行中的临时状态在内存里：

```
持久化 (claude-sessions.json):
  wxid → { aliases, activeCwd, sessions[cwd] → { sessionId, lastActive, approvedTools } }

内存 (QueryManager):
  wxid → Map<cwd, RunningQuery>:
    {
      cwd: "/Users/.../wiki",
      query: <AsyncIterable>,        // SDK 返回的 Query 对象
      abortController: AbortController,
      pendingApproval: {              // 当前 cwd 上的审批等待
        toolName: "Bash",
        resolver: PromiseResolver,
        timer: Timeout
      },
      replyBuffer: "",                // 累积的 assistant 回复文本
      messageQueue: []                // 同 cwd 内的排队消息
    }
```

#### 消息路由（adapter 主循环）

```
  poll 到消息 msg:

  // ── 全局拦截：adapter 级指令 ──
  if msg.text 匹配 /cd:
    handleCd(msg.from_user, msg.text)
    return

  // ── 根据 activeCwd 找到目标 query ──
  cwd = sessions[msg.from_user].activeCwd
  runningQuery = queryManager.get(msg.from_user, cwd)

  // ── 审批指令：路由到对应 cwd 的 pendingApproval ──
  if msg.text 匹配 /approve 或 /deny:
    approval = runningQuery?.pendingApproval
    if approval:
      resolve approval (allow/deny)
    else:
      reply "**claude**:{basename}\n\n当前没有待审批的操作"
    return

  if msg.text 匹配 /approve on|off:
    permissionMode = msg.text === "/approve on" ? "bypassPermissions" : "default"
    reply "**claude**:{basename}\n\n已切换为{自动审批/交互审批}模式"
    return

  // ── 普通消息 ──
  if runningQuery && runningQuery.query is still iterating:
    // 当前 cwd 有正在运行的 query
    if runningQuery.pendingApproval:
      // query 阻塞在审批等待中，普通消息排队
      runningQuery.messageQueue.push(msg)
    else:
      // query 正在执行工具，消息排队
      runningQuery.messageQueue.push(msg)
    return

  // 当前 cwd 空闲 → 启动新 query
  if !sessions[wxid].sessions[cwd]:
    // 首次，创建 session entry
    ...
  queryManager.start(msg.from_user, cwd, {
    prompt: msg.text,
    resume: sessions[wxid].sessions[cwd].sessionId
  })
```

#### 回复的归属

所有 Claude 输出统一使用 `**{cwd_basename}**\n\n` 作为消息头（见下方流式推送），`cwd_basename` 取 `basename(activeCwd)`。审批指令（`/approve`/`/deny`）默认作用于当前 `activeCwd` 的 pending approval。

#### 流式推送

**所有发往微信的消息（Claude 输出 + adapter 系统消息）统一格式**：

```
**claude**:{workspace}

{content}
```

- `**claude**` 加粗，区分 agent 来源（与 hermes 等其他 agent 并列时一目了然）
- `{workspace}` 不加粗，取 `basename(activeCwd)`，如 `wiki`、`wechat-gateway`
- Claude 回复、审批提示、`/cd` 确认、错误提示全部用这个格式

assistant 消息的每个 text 块和 tool_use 块到达时**先缓存，合批后发送**——避免每条小消息都变成独立微信气泡。

**合批规则**：

| 触发条件 | 行为 |
|----------|------|
| buffer 超过 1500 字符 | 立即发送 buffer，清空 |
| 收到 tool_use 块 | 先发送 buffer（如有），再单独发送 tool_use 通知 |
| 收到 result 消息 | 发送剩余 buffer |
| text 块到达后 2 秒无新块 | 发送 buffer |

**消息格式**：

```
**claude**:wiki

{content}
```

tool_use 单独发送（不和文本混在一起）：

```
**claude**:wiki

🔧 Bash: npm install
```

审批提示同理：

```
**claude**:wiki

Claude 想执行 Bash: npm run build
回复: /approve  /deny  /approve session
```

**实现**：

```
assistant 消息迭代:
  for (block of msg.message.content):
    if block.type === "text":
      buffer += block.text
      resetTimer(2000)                    // 重新计时 2s
      if buffer.length > 1500:
        flush()

    if block.type === "tool_use":
      flush()                             // 先发文本
      gateway.reply("**claude**:{workspace}\n\n🔧 {name}: {summary}")

  timer 到期:
    flush()

result 消息:
  flush()                                 // 最后一批
  → 处理排队消息 → queryManager.remove()

function flush():
  if buffer 非空:
    gateway.reply("**claude**:{workspace}\n\n" + buffer)
    buffer = ""
```

#### 并发场景表

| 场景 | 行为 |
|------|------|
| 在 A 聊天，`/cd B`，A 还在跑 | A 继续运行，完成后回复到微信。不丢结果。 |
| 在 A 聊天，A 在等审批，`/cd B`，发消息给 B | A 的审批继续等待，B 启动新 query。两个独立。 |
| 在 A 聊天，`/cd B`，B 聊天完成后 `/cd A` 回来 | 回到 A。如果 A 的 query 还在跑，新消息排队；如果已完成，恢复多轮对话。 |
| A 和 B 同时需要审批 | 各自 `pendingApproval` 独立，审批提示带 `**cwd**` 头区分来源 |
| `/approve on` 在 A 的审批等待期间发出 | `setPermissionMode('bypassPermissions')` 只影响后续新的 query 调用。当前已启动的 query 不受影响——它的 permissionMode 在 `query()` 调用时已确定。 |

---

## 5. 项目结构

```
wechat-gateway/
├── client/
│   ├── hermes-wechat-plugin/     # 已有：Hermes adapter
│   │   ├── adapter.py
│   │   ├── plugin.yaml
│   │   └── __init__.py
│   │
│   └── claude-code-adapter/      # 新增：Claude Code adapter
│       ├── package.json
│       ├── src/
│       │   ├── index.ts          # 入口：启动 → 注册 → poll loop
│       │   ├── gateway-client.ts # wechat-gateway API 客户端
│       │   ├── query-manager.ts  # 运行时查询管理：Map<cwd, RunningQuery>，并发维护
│       │   ├── claude-session.ts # Claude SDK query 封装 + session 管理
│       │   ├── session-store.ts  # wxid → cwd → sessionId 持久化 + 白名单 + 别名
│       │   ├── approval.ts       # 工具审批逻辑 + 审批指令解析
│       │   ├── formatter.ts      # Markdown → 纯文本转换
│       │   └── config.ts         # 环境变量读取
│       └── tsconfig.json
```

---

## 6. 开发任务清单

### Phase 1: 基础框架（1-2 天）

- [ ] **T1.1** 初始化 Node.js 项目，安装 `@anthropic-ai/claude-agent-sdk`
- [ ] **T1.2** 实现 `gateway-client.ts`：封装 register / poll / reply 三个 HTTP 调用
- [ ] **T1.3** 实现 `index.ts` 主流程：注册 → 1s 轮询 → 收到消息打印日志
- [ ] **T1.4** 验证：启动 adapter → 微信发消息 → adapter 日志可见消息内容

### Phase 2: Claude SDK 集成（2-3 天）

- [ ] **T2.1** 实现 `query-manager.ts`：`Map<wxid, Map<cwd, RunningQuery>>`，管理查询生命周期（启动/完成/中止）
- [ ] **T2.2** 实现 `claude-session.ts`：`query()` 封装，消息迭代，文本收集，`canUseTool` 回调注册
- [ ] **T2.3** 实现 `session-store.ts`：JSON 文件读写，`wxid → cwd → sessionId` 映射，别名管理，文件锁
- [ ] **T2.4** 实现消息路由：poll 消息 → 拦截 adapter 指令 → 根据 activeCwd 找到 RunningQuery → 空闲则启动/运行中则排队
- [ ] **T2.5** 实现多轮对话：第二轮起用 `resume: sessionId` + `cwd: activeCwd`
- [ ] **T2.6** 实现 `/cd` 命令：路径解析（别名 → 精确匹配 → 模糊匹配 → 绝对路径）、切换 activeCwd、列出状态、别名增删
- [ ] **T2.7** 实现跨目录并行：不同 cwd 的 query 独立运行，A 运行时 `/cd B` 启动 B 的 query 不中断 A
- [ ] **T2.8** 验证：微信用户发两轮对话，Claude 能记住上下文；`/cd` 切换目录后 session 隔离正常；A 运行时切换 B 发消息，A 完成后结果不丢

### Phase 3: 审批机制 + 生产就绪（2-3 天）

- [ ] **T3.1** 实现 `approval.ts`：审批指令解析（`/approve`/`/deny`/`/approve on`/`/approve off`/`/approve session`）
- [ ] **T3.2** 实现审批流程：`canUseTool` → 白名单检查 → permissionMode 检查 → 发微信 → Promise 等待用户回复
- [ ] **T3.3** 实现 `setPermissionMode` 动态切换：`/approve on` → `bypassPermissions`，`/approve off` → `default`
- [ ] **T3.4** 实现 session 白名单：`/approve session` 将工具名持久化到 `session-store`
- [ ] **T3.5** 实现审批超时：60s 无回复自动拒绝
- [ ] **T3.6** 实现 `formatter.ts`：Markdown → 微信纯文本
- [ ] **T3.7** 实现超时提示（30s 无输出发 "处理中..."）
- [ ] **T3.8** 实现并发控制：同一 wxid 串行，新消息排队；审批指令优先消费不走 SDK
- [ ] **T3.9** 实现信号处理：SIGINT/SIGTERM 优雅关闭（取消当前 query，resolve 所有 pending 审批为 deny）
- [ ] **T3.10** 实现心跳/重连：gateway 连接断开自动重试
- [ ] **T3.11** 环境变量配置：`CLAUDE_GATEWAY_URL`, `CLAUDE_GATEWAY_AGENT_NAME`, `CLAUDE_MODEL`, `CLAUDE_CWD`
- [ ] **T3.12** 编写 README：安装、配置、启动、审批命令说明、常见问题

### Phase 4: 打磨（可选）

- [ ] **T4.1** 长回复自动分段（微信单条消息最大约 4000 字符）
- [ ] **T4.2** `/new` 命令支持：清除当前 wxid + activeCwd 的 session 和白名单，新建对话
- [ ] **T4.3** 用 `claude agents --json` 展示活跃 session 状态
- [ ] **T4.4** systemd/launchd 守护进程配置

---

## 7. 坑点与处理方案

### 坑点 1: `@anthropic-ai/claude-agent-sdk` 版本兼容

**问题**：SDK npm 包必须与系统安装的 Claude Code CLI 版本匹配，否则可能出现 spawn 失败或行为不一致。

**处理**：
- adapter 启动时检查 `claude --version`，与 SDK 的 `package.json` 中的依赖版本比对
- 在 `package.json` 中固定 SDK 版本（不要用 `^`）
- 如果版本不匹配，打印警告但尝试继续

### 坑点 2: SDK spawn 的 Claude 进程是子进程

**问题**：`query()` 会在内部 spawn 一个 `claude` 子进程。如果 adapter 被 SIGTERM 杀死，子进程可能变成孤儿进程继续占用资源。

**处理**：
- adapter 的 `SIGINT`/`SIGTERM` 处理中调用 `abortController.abort()` 取消当前 query
- query 被 abort 后 SDK 会发送 SIGTERM 给子进程
- 延迟 3 秒等待子进程退出，超时则 SIGKILL
- 所有 query 取消后才 `process.exit(0)`

### 坑点 3: session 文件竞争

**问题**：如果用户同时在终端运行 `claude --resume <id>` 和 adapter，两个进程会同时读写同一个 JSONL 文件。

**处理**：
- adapter 只读 JSONL（通过 SDK 的 `resume`），不直接写入
- SDK 内部有文件锁机制
- 但如果用户用 `claude` CLI 直接操控同一个 session，adapter 的上下文可能过期
- **建议**：文档中注明 "如果要在终端用 Claude，请 `/new` 开启新对话，不要 resume adapter 的 session"

### 坑点 4: 回复长度限制

**问题**：微信单条消息有长度限制（约 4000 字符）。Claude 的回复可能很长。

**处理**：
- 如果回复超过 3800 字符，分段发送（每段 3800 字符以内）
- 分段标记：`[1/3] ...` `[2/3] ...` `[3/3] ...`
- 段与段之间间隔 500ms，避免微信限速

### 坑点 5: 代理配置

**问题**：Anthropic API 在国内需要代理。Claude CLI 通过自己的配置读取代理，但 SDK spawn 的子进程可能不继承。

**处理**：
- adapter 读环境变量 `HTTP_PROXY` / `HTTPS_PROXY`
- 传递给 SDK 的 `env` 选项：`env: { HTTP_PROXY, HTTPS_PROXY, ...process.env }`
- 验证：启动后检查是否能正常调用 SDK

### 坑点 6: `CLAUDE_CODE_ENTRYPOINT` 导致终端 `claude --resume` 看不到 session

**问题**：如 Happy 的 slopus/happy#1202，SDK 默认设置 `CLAUDE_CODE_ENTRYPOINT="sdk-ts"`，Claude Code 的 `--resume` picker 会过滤掉这些 session。

**处理**：
- 在 SDK `env` 中显式设置 `CLAUDE_CODE_ENTRYPOINT: "remote_mobile"`
- `"remote_mobile"` 在 Claude Code 的 allowlist 中，不会被 picker 过滤

### 坑点 7: 工具调用超时

**问题**：`Bash`、`Write` 等工具调用可能需要很长时间（比如下载大文件）。

**处理**：
- `canUseTool` 回调中可以设置 signal 来中止工具
- adapter 的超时提示与工具执行是并行的：30 秒无 assistant 文本 → 发提示，但不中止
- Claude SDK 自身有工具超时机制，无需额外处理

### 坑点 9: 审批等待期间的消息路由

**问题**：`canUseTool` 等待用户审批时，poll 循环继续运行。此时如果用户发来普通消息（非审批指令），不能直接发给 SDK——SDK 正在阻塞等待 `canUseTool` 返回。

**处理**：
- 审批等待期间，普通消息排队存入 `pendingMessages` 队列
- `canUseTool` resolve → SDK 继续执行 → 当前 query 完成后 → 处理 pending 消息
- 审批指令（`/approve` 等）优先级最高，立即消费不排队

### 坑点 10: `/approve session` 的持久化时机

**问题**：如果 adapter 在 `canUseTool` resolve 后、写入 JSON 文件前崩溃，白名单丢失。

**处理**：
- 先在内存中更新白名单
- 立即 `resolve({ behavior: 'allow' })`（不阻塞 SDK）
- 异步写入 JSON 文件
- 如果写入失败，下次 poll 时从文件恢复的是旧数据，但内存中的已生效——同一 session 内不影响

### 坑点 8: Node.js 进程管理

**问题**：adapter 是一个常驻进程，需要考虑崩溃恢复。

**处理**：
- 所有未捕获异常用 `process.on('uncaughtException')` 捕获
- 崩溃后自动重启（外部用 launchd 或简单的 while 循环）
- session 映射存储在文件中，重启不丢失

### 坑点 11: 多用户并发审批

**问题**：两个微信用户同时触发审批，需要区分各自的 pending approval。

**处理**：
- `pendingApprovals` 按 wxid 索引：`Map<wxid, PromiseResolver>`
- 每个 wxid 同时只有一个审批等待（因为同 wxid 的消息串行处理）
- 审批指令从 `msg.from_user` 找到对应的 resolver

---

## 8. 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `CLAUDE_GATEWAY_URL` | `http://127.0.0.1:8765` | wechat-gateway API 地址 |
| `CLAUDE_GATEWAY_AGENT_NAME` | `claude` | 注册到 gateway 的名字 |
| `CLAUDE_MODEL` | `sonnet` | Claude 模型（sonnet / opus） |
| `CLAUDE_CWD` | `process.cwd()` | Claude 工作目录 |
| `CLAUDE_POLL_INTERVAL` | `1000` | 轮询间隔（毫秒） |
| `CLAUDE_EFFORT` | `medium` | 思考深度（low/medium/high/xhigh/max） |
| `HTTP_PROXY` / `HTTPS_PROXY` | - | 代理设置，传给 SDK |
