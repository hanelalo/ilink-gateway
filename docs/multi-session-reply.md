# 基于引用回复的多 Agent / 多工作区消息路由方案

## 1. 需求背景

gateway 支持多个 agent 并行（hermes、claude 等），claude-code-adapter 内部还支持多个 workspace 并行（通过 `/cd` 切换）。微信上的消息列表会混排来自不同 agent、不同 workspace 的消息。

用户希望在微信中长按某条消息 → 选择「引用回复」→ 输入文字 → 发送，这条回复能够自动路由到**被引用消息**所对应的 agent 和 workspace，而不是由当前 active agent 处理。

## 2. 设计目标

1. 被引用的回复消息能正确路由到该回复所属的 agent
2. 对于 claude adapter，还能进一步路由到该回复所属的 workspace
3. hermes 等其他无 workspace 概念的 agent 不需要二次路由
4. gateway 不感知 workspace 概念，只负责 agent 级路由
5. 改动量最小，向前兼容

## 3. 核心概念

### 3.1 agent_context

`agent_context` 是一个可选的 JSON 字符串，贯穿整个消息生命周期：

```
agent 回复 → gateway 存 msg_id ↔ agent_context → 引用回复到来 → gateway 查 agent_context → 路由到 agent
```

**固定字段：** `agent` — 对应 agent 名称（必填）

**扩展字段：** agent 的 client 可自行添加其他字段。

**示例：**

```json
// hermes 回复时
{"agent": "hermes"}

// claude 回复时
{"agent": "claude", "workspace": "my-project"}
```

### 3.2 关键 ID

| ID | 来源 | 类型 | 用途 |
|----|------|------|------|
| `message_id` | `sendmessage` 响应中的 `message_id` | 数值型（i64），存映射时 `to_string()` | 作为 `msg_id → agent_context` 映射的 key |
| `ref_msg.message_item.msg_id` | 引用回复的 item_list 中 | 字符串（数字的字符串形式） | 在映射中查找被引用消息的 agent_context |

两者是同一数值的不同表现形式，映射时统一用字符串 key。

## 4. 协议细节

### 4.1 数据流总图

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ Gateway 长轮询循环                                                          │
│                                                                             │
│ get_updates → WeixinMessage                                                 │
│   ├─ 普通消息: 走 handle_incoming → active agent                           │
│   └─ 引用回复: 识别 ref_msg → 查 SQLite → 路由到指定 agent                 │
│                                                                             │
│ handle_agent_replies (回复处理器)                                           │
│   AgentReply → sendmessage → sendmessage 响应带回 message_id                │
│   → SQLite 存 msg_id → agent_context                                        │
└─────────────────────────────────────────────────────────────────────────────┘
         ▲                            │
         │ POST reply                 │ poll
         │                            ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ Agent (hermes / claude adapter)                                             │
│                                                                             │
│ 回复时: POST /api/agents/{name}/reply { reply_to_id, text, agent_context }  │
│ 消费时: AgentMessage 带 agent_context → 非空则二次路由                       │
│   - hermes: 直接处理消息                                                    │
│   - claude: 解析 workspace → 路由到对应 workspace session                   │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 reply 请求（agent → gateway）

**HTTP:** `POST /api/agents/{name}/reply`

**当前报文：**
```json
{
  "reply_to_id": "42",
  "text": "这是回复内容"
}
```

**改动后报文（agent_context 可选，向后兼容）：**
```json
{
  "reply_to_id": "42",
  "text": "这是回复内容",
  "agent_context": "{\"agent\":\"claude\",\"workspace\":\"my-project\"}"
}
```

**proactive send 不变，无需 agent_context：**
```json
{
  "reply_to_id": "",
  "text": "通知消息",
  "to_user": "o9cq80xxx@im.wechat",
  "context_token": ""
}
```

### 4.3 reply 响应（gateway → agent）

**当前响应：**
```json
{"ok": true}
```

**改动后响应：** 不变，gateway 不需要告知 agent 发送结果（消息 ID 由 gateway 内部处理）。

### 4.4 下行消息（gateway → agent，poll 响应）

**当前 `AgentMessage`：**
```json
{
  "id": "7484409702373212424",
  "from_user": "o9cq80xxx@im.wechat",
  "text": "这是真的吗？",
  "timestamp": 1784422326551,
  "context_token": "AARzJWAF...",
  "message_type": "text",
  "media": []
}
```

**改动后（引用回复时，新增 agent_context）：**
```json
{
  "id": "7484409702373212424",
  "from_user": "o9cq80xxx@im.wechat",
  "text": "这是真的吗？",
  "timestamp": 1784422326551,
  "context_token": "AARzJWAF...",
  "message_type": "text",
  "media": [],
  "agent_context": "{\"agent\":\"claude\",\"workspace\":\"my-project\"}"
}
```

**普通消息 `agent_context` 为 null 或缺失**，agent 端忽略即可。

### 4.5 sendmessage 响应（iLink → gateway）

**当前解析：**
```json
{"ret": 0}
```
（serde 静默忽略未知字段，实际 iLink 返回了更多字段）

**实际 iLink 返回（已通过日志验证）：**
```json
{
  "ret": 0,
  "message_id": 7484409538682029960
}
```

**改动后：** 明确解析 `message_id` 字段。

### 4.6 引用回复消息体（iLink → gateway）

**已通过日志验证的引用回复报文结构：**
```json
{
  "seq": 98,
  "message_id": 7484409702373212424,
  "from_user_id": "o9cq80xW7CUQFg3cfKLVlg7F0U6s@im.wechat",
  "to_user_id": "48ad865c44c7@im.bot",
  "message_type": 1,
  "item_list": [
    {
      "type": 1,
      "text_item": { "text": "这是真的吗？" },
      "msg_id": "v1:3306632368402003437",
      "ref_msg": {
        "message_item": {
          "msg_id": "7484409538682029960",
          "type": 0,
          "create_time_ms": 1784422288000,
          "update_time_ms": 1784422288000,
          "is_completed": true
        }
      }
    }
  ],
  "context_token": "AARzJWAF..."
}
```

取用路径：`item_list[0].ref_msg.message_item.msg_id` → `"7484409538682029960"`

## 5. 详细实现

### 5.1 开发任务总览

| # | 范围 | 描述 | 文件 |
|---|------|------|------|
| 1 | Rust types | 新增 RefMsg 结构体，扩展 MessageItem | `gateway/src/ilink/types.rs` |
| 2 | Rust types | SendMessageResponse 加 message_id | `gateway/src/ilink/types.rs` |
| 3 | Rust types | AgentReply 加 agent_context | `gateway/src/ilink/types.rs` |
| 4 | Rust types | AgentMessage 加 agent_context | `gateway/src/ilink/types.rs` |
| 5 | API | ReplyRequest 加 agent_context | `gateway/src/api/server.rs` |
| 6 | Storage | SQLite 新增 msg_agent_context 表和方法 | `gateway/src/storage/sqlite_store.rs` |
| 7 | Main | 回复成功时存 msg_id → agent_context | `gateway/src/main.rs` |
| 8 | Router | 新增指定 agent 路由方法 | `gateway/src/router/router.rs` |
| 9 | Main | 引用回复识别、查映射、路由 | `gateway/src/main.rs` |
| 10 | TS types | AgentMessage 类型加 agent_context | `client/claude-code-adapter/src/gateway-client.ts` |
| 11 | TS reply | reply 方法支持 agent_context | `client/claude-code-adapter/src/gateway-client.ts` |
| 12 | TS main | 回复时传 agent_context + 消费时二次路由 | `client/claude-code-adapter/src/index.ts` |

### 5.2 任务 1：新增 RefMsg 结构体 + MessageItem 扩展

**文件：** `gateway/src/ilink/types.rs`

**改动：**

```rust
// 新增，放在 MessageItem 定义之前
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefMsgItem {
    pub msg_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefMsg {
    pub message_item: RefMsgItem,
}

// MessageItem 加 ref_msg 字段
pub struct MessageItem {
    // ... 现有字段不变
    pub video_item: Option<VideoItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_msg: Option<RefMsg>,        // ← 新增
    #[serde(flatten)]
    pub extra: serde_json::Value,       // 保留，捕获其他未知字段
}
```

**注意：** `RefMsg` 和 `RefMsgItem` 目前只关心 `msg_id`，其余字段（`type`、`create_time_ms` 等）先不定义，落到 `extra` 中即可。后续协议明确后再显式声明。

### 5.3 任务 2：SendMessageResponse 加 message_id

**文件：** `gateway/src/ilink/types.rs`

```rust
pub struct SendMessageResponse {
    pub ret: Option<i32>,
    pub errcode: Option<i32>,
    pub errmsg: Option<String>,
    pub context_token: Option<String>,
    pub message_id: Option<i64>,  // ← 新增
    #[serde(flatten)]
    pub extra: serde_json::Value,  // 保留兜底
}
```

### 5.4 任务 3：AgentReply 加 agent_context

**文件：** `gateway/src/ilink/types.rs`

```rust
pub struct AgentReply {
    pub reply_to_id: String,
    pub text: String,
    pub media_paths: Vec<String>,
    pub to_user: Option<String>,
    pub context_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_context: Option<String>,  // ← 新增
}
```

### 5.5 任务 4：AgentMessage 加 agent_context

**文件：** `gateway/src/ilink/types.rs`

```rust
pub struct AgentMessage {
    pub id: String,
    pub from_user: String,
    pub text: String,
    pub timestamp: i64,
    pub context_token: String,
    pub message_type: String,
    pub media: Vec<MediaItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_context: Option<String>,  // ← 新增
}
```

### 5.6 任务 5：ReplyRequest 加 agent_context

**文件：** `gateway/src/api/server.rs`

```rust
pub struct ReplyRequest {
    pub reply_to_id: String,
    pub text: String,
    pub media_paths: Vec<String>,
    pub to_user: Option<String>,
    pub context_token: Option<String>,
    #[serde(default)]
    pub agent_context: Option<String>,  // ← 新增
}
```

`handle_reply` 中从 `ReplyRequest` 取出 `agent_context` 放到 `AgentReply` 中。

### 5.7 任务 6：SQLite 新增 msg_agent_context 表

**文件：** `gateway/src/storage/sqlite_store.rs`

新增表结构（在 `conn.execute_batch` 中添加）：

```sql
CREATE TABLE IF NOT EXISTS msg_agent_context (
    msg_id      TEXT PRIMARY KEY,
    context     TEXT NOT NULL,
    created_at  INTEGER NOT NULL
)
```

新增方法：

```rust
/// 保存 msg_id → agent_context 映射，最多保留 RECENT_LIMIT 条
pub fn save_msg_agent_context(&self, msg_id: &str, context: &str) -> Result<()>

/// 根据 msg_id 查询 agent_context
pub fn get_msg_agent_context(&self, msg_id: &str) -> Result<Option<String>>
```

**`save_msg_agent_context` 实现逻辑：**

1. `INSERT OR REPLACE INTO msg_agent_context (msg_id, context, created_at) VALUES (?1, ?2, UNIXEPOCH())`
2. 查询当前总行数：`SELECT COUNT(*) FROM msg_agent_context`
3. 如果 ≥ 200，删除最早的一条：`DELETE FROM msg_agent_context WHERE msg_id = (SELECT msg_id FROM msg_agent_context ORDER BY created_at ASC LIMIT 1)`

**竞争条件说明：** gateway 的回复处理器是单线程的（同一时刻只会处理一个 `AgentReply`），所以不存在并发竞争 200 条限制的问题。但即使有并发也不会超过 200+少量的程度，不做悲观处理。

### 5.8 任务 7：回复成功时存 msg_id → agent_context

**文件：** `gateway/src/main.rs`

**位置：** `handle_agent_replies` 函数中，`resilient_send` 成功后。

```rust
// resilient_send 当前返回 Ok(())，不携带 message_id。
// 需要两个改动：
//
// a) resilient_send 签名改为返回 message_id（Option<String> 或类似）
// b) 调用方在成功后存映射

// resilient_send 改动（第 656-714 行）：
// 原本:
//   async fn resilient_send(...) -> std::result::Result<(), GatewayError>
// 改为:
//   async fn resilient_send(...) -> std::result::Result<Option<String>, GatewayError>
// 在 errcode == 0 分支返回 resp.message_id.map(|id| id.to_string())

// handle_agent_replies 中的调用（第 783 行附近）：
// 原本:
//   if let Err(e) = resilient_send(&client, &token, req, &breaker).await { ... }
// 改为:
//   match resilient_send(&client, &token, req, &breaker).await {
//       Ok(Some(msg_id)) => {
//           if let Some(ref ctx) = reply.agent_context {
//               if let Err(e) = store.save_msg_agent_context(&msg_id, ctx) {
//                   tracing::warn!("failed to save agent context: {e}");
//               }
//           }
//       }
//       Ok(None) => {} // 正常但无 message_id
//       Err(e) => tracing::error!("failed to send text reply: {e}"),
//   }
```

**注意点：**

- `handle_agent_replies` 当前签名为 `async fn handle_agent_replies(client, token, _router, contexts, ticket_cache, breaker, rx)`，需要新增 `store: Arc<Mutex<SqliteStore>>` 参数
- 调用处位于 `main.rs` 第 152-169 行，相应同步更新
- hermes 回复时 `agent_context` 为 `None`，此分支跳过，不影响原有逻辑
- 修改 `resilient_send` 时，所有调用处需同步更新：main.rs 中有 5 处调用 `resilient_send`

### 5.9 任务 8：Router 新增指定 agent 路由方法

**文件：** `gateway/src/router/router.rs`

当前 `handle_incoming` 固定走 active agent。新增方法：

```rust
impl Router {
    /// 将消息路由到指定的 agent，绕过 active agent 检查。
    pub fn route_to_agent(
        &mut self,
        msg: &WeixinMessage,
        agent_name: &str,
        agent_context: Option<String>,
    ) -> Result<()> {
        // 1. 检查 agent 是否存在（复用 registry.contains）
        // 2. to_agent_message → QueuedMessage，并设置 agent_context
        // 3. enqueue
    }
}
```

**与 `handle_incoming` 的关系：**

- `handle_incoming` 保持原有逻辑不变（含 admission policy 检查）
- `route_to_agent` 专供引用回复使用，直接路由到指定 agent，但也应执行 admission policy 检查
- 两者共用 `to_agent_message` 等内部方法

### 5.10 任务 9：引用回复识别、查映射、路由

**文件：** `gateway/src/main.rs`

**位置：** 长轮询循环中，接收到用户消息后，在调用 `handle_incoming` 之前。

**伪代码：**

```rust
// 识别引用回复
let ref_msg_id = msg.item_list
    .as_ref()?
    .first()?
    .ref_msg
    .as_ref()?
    .message_item
    .msg_id
    .clone();

if let Some(ref_msg_id) = ref_msg_id {
    // 查 SQLite 获取 agent_context
    if let Ok(Some(context_json)) = store.get_msg_agent_context(&ref_msg_id) {
        // 解析 agent 字段
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&context_json) {
            if let Some(agent) = parsed.get("agent").and_then(|v| v.as_str()) {
                // 路由到指定 agent
                let mut router = router_arc.lock().unwrap();
                if router.registry().contains(agent) {
                    router.route_to_agent(&msg, agent, Some(context_json))?;
                    continue; // 跳过后续 handle_incoming
                }
            }
        }
    }
}

// 非引用回复，继续原有逻辑
let (reply_text, active_agent) = { ... };
```

**关键判断链：**

1. `item_list` 非空
2. `item_list[0].ref_msg` 非空
3. `item_list[0].ref_msg.message_item.msg_id` 非空
4. SQLite 查到对应映射
5. `agent` 字段有效且 agent 已注册
6. 任一环节不满足 → fallback 到现有 active agent 逻辑

**admission policy 处理：** `route_to_agent` 内部应执行 admission policy 检查。如果引用回复的用户/群不在允许列表中，应该丢弃。这和 `handle_incoming` 的逻辑一致。

### 5.11 任务 10：TS AgentMessage 类型加 agent_context

**文件：** `client/claude-code-adapter/src/gateway-client.ts`

```typescript
export interface AgentMessage {
  id: string;
  from_user: string;
  text: string;
  timestamp: number;
  context_token: string;
  message_type: string;
  media: MediaItem[];
  agent_context?: string;  // ← 新增
}
```

### 5.12 任务 11：TS reply 方法支持 agent_context

**文件：** `client/claude-code-adapter/src/gateway-client.ts`

```typescript
async reply(replyToId: string, text: string, mediaPaths?: string[], agentContext?: string): Promise<boolean> {
  const body: Record<string, unknown> = {
    reply_to_id: replyToId,
    text,
  };
  if (mediaPaths && mediaPaths.length > 0) {
    body.media_paths = mediaPaths;
  }
  if (agentContext) {
    body.agent_context = agentContext;
  }
  // ... rest unchanged
}
```

所有 `client.reply()` 调用处需确保 `agent_context` 被传入。目前 adapter 的 `startClaudeSession` 的回调中有多处调用 `client.reply`，需要在 `startClaudeSession` 的 closure 中捕获当前 workspace 信息。

### 5.13 任务 12：TS 回复时传 agent_context + 消费时二次路由

**文件：** `client/claude-code-adapter/src/index.ts`

**两个改动点：**

**改动 A：回复时传 agent_context**

`startClaudeSession` 当前调用时没有 workspace 上下文。需要在其回调或参数中加上当前 workspace 信息。

```typescript
// handleMessage 中，调用 startClaudeSession 之前
const agentContext = JSON.stringify({
  agent: config.agentName,
  workspace: basename,
});

// startClaudeSession 的参数和回调中透传 agentContext
// 在 onFlush / onIdle 等回调中，调用 client.reply 时传入 agentContext
```

具体影响到的 `client.reply` 调用位置（`index.ts` 中搜索 `client.reply`）：

- `handleMessage` 中的 command 回复（第 141、165、175 行等）
- `streaming-batcher` 的 `onFlush` 回调（第 273-291 行）
- `onIdle` 回调（第 292-302 行）
- approval prompt 回复（第 342 行）

每个调用处都需要传入 `agentContext`。建议将 `agentContext` 作为 `handleMessage` 的局部变量，或闭包捕获。

**改动 B：消费时二次路由**

```typescript
async function handleMessage(msg: AgentMessage): Promise<void> {
  const wxid = msg.from_user;
  const text = msg.text.trim();

  // ---- 0. 引用回复路由 ----
  if (msg.agent_context) {
    try {
      const ctx = JSON.parse(msg.agent_context);
      const targetWorkspace = ctx.workspace;
      if (targetWorkspace && targetWorkspace !== path.basename(user.activeCwd || config.cwd)) {
        // 切换到目标 workspace 后再处理
        // 注意：此时可能另一个 workspace 有正在进行的 session
        // 需要按目标 workspace 获取/创建 user 和 session
        user = ensureUser(wxid);
        // 切换到目标 workspace
        // 然后继续下面的流程
      }
    } catch {
      // agent_context 解析失败，忽略，按当前 workspace 处理
    }
  }

  // ---- 1. Global command interception (不变) ----
  // ...
}
```

**注意点：**

1. 当 `agent_context` 指定了不同的 workspace 时，当前 workspace 的 `runningQuery` 不受影响
2. 引用回复应该直接投递到目标 workspace，不走当前 workspace 的 queue
3. 如果目标 workspace 有正在运行的 session，引用回复应该入队到目标 workspace 的 queue（复用现有的 `messageQueue` 机制）
4. 需要确保 `ensureUser` 和 `sessionData` 中目标 workspace 的 session 状态正确

## 6. Proxy 层统一处理方案

当前方案中，Rust gateway 负责 agent 级路由，claude adapter 负责 workspace 级二次路由。但由于 adapter 中对 `client.reply` 的调用分散在各处，`agent_context` 的透传会比较繁琐。

一种替代思路是**不再在 adapter 传 `agent_context`**，而是让 gateway 完全接管：

**替代方案 A（不考虑）：gateway 感知 workspace**

gateway 维护完整的 `msg_id → {agent, workspace}` 映射。引用回复到来时根据 `agent` 路由到 claude agent，同时 `AgentMessage.agent_context` 中携带 `workspace`。adapter 只在收到 `agent_context` 时做二次路由，但不需要在回复时传 `agent_context`。

**问题：** adapter 回复后 `sendmessage` 返回 `message_id`，但 gateway 此时拿不到这个回复属于哪个 workspace。需要 adapter 在回复时额外传 workspace ID，又回到了需要 adapter 传 `agent_context` 的问题。

→ 因此当前方案（adapter 传 `agent_context`，gateway 存映射，gateway 透传 `agent_context`，adapter 二次路由）是最干净的分层。

## 7. 边界情况

### 7.1 引用回复时目标 agent 未注册

gateway 查 SQLite 拿到 `agent_context` 后发现目标 agent 未注册 → fallback 到 active agent 处理。

```rust
// main.rs 引用回复路由处
if router.registry().contains(agent) {
    router.route_to_agent(&msg, agent, Some(context_json))?;
} else {
    // agent 未注册，fallback 到 active agent
    tracing::warn!("ref_msg agent '{agent}' not registered, falling back to active agent");
    let reply = router.handle_incoming(&msg)?;
    // ...
}
```

### 7.2 引用消息的 msg_id 在 SQLite 中不存在

SQLite 只保留 200 条，或者 gateway 重启后映射丢失。

→ fallback 到 active agent，并打印警告日志。

### 7.3 sendmessage 响应没有 message_id

某些情况下（如错误响应）响应中可能没有 `message_id`。

→ `resilient_send` 返回 `Ok(None)`，不存映射，后续引用回复退化到 fallback 逻辑。

### 7.4 agent_context JSON 解析失败

agent 传了非法 JSON → gateway 存库和解析时需做错误处理。

→ `serde_json::from_str` 失败时走日志警告 + fallback。

### 7.5 adapter 进程重启

adapter 重启后，本地的 session 状态丢失。但 gateway 侧的 `msg_id → agent_context` 映射仍在 SQLite 中（不上限 200 条），所以重启后的引用回复仍能正确路由到 claude agent。

但 adapter 重启后不知道之前各 workspace 的状态，二次路由时需要：

- 如果 `agent_context` 中有 `workspace`，按 workspace 查找或创建 session
- 如果该 workspace 的 `activeCwd` 不可用，回复错误信息让用户重新 `/cd`

### 7.6 群聊中的引用回复

群聊中引用回复和私聊的结构相同（`group_id` 不为空），admission policy 检查一致。gateway 的 `route_to_agent` 应复用餐前的 `is_group_allowed` 检查。

### 7.7 多条 item_list 中的 ref_msg

当前只取 `item_list[0].ref_msg`，因为微信一条消息通常只有一个 item。如果以后出现多条 item 且都有 `ref_msg` 的复杂场景，暂不处理，按第一个 item 的引用为准。

## 8. 测试用例

### 8.1 Rust 单元测试

#### 8.1.1 SendMessageResponse message_id 解析

**文件：** `gateway/src/ilink/types.rs`

| 标题 | 描述 | 预期 |
|------|------|------|
| `test_send_message_response_with_message_id` | iLink 返回含 message_id 的响应 | `message_id` 为 `Some(7484409538682029960)` |
| `test_send_message_response_without_message_id` | 错误响应不含 message_id | `message_id` 为 `None` |

```rust
#[test]
fn test_send_message_response_with_message_id() {
    let json = r#"{"ret": 0, "message_id": 7484409538682029960}"#;
    let resp: SendMessageResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.message_id, Some(7484409538682029960));
}

#[test]
fn test_send_message_response_without_message_id() {
    let json = r#"{"errcode": -2}"#;
    let resp: SendMessageResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.message_id, None);
}
```

#### 8.1.2 RefMsg 反序列化

**文件：** `gateway/src/ilink/types.rs`

| 标题 | 描述 | 预期 |
|------|------|------|
| `test_message_item_with_ref_msg` | 引用回复报文中的 ref_msg 能正确反序列化 | `item.ref_msg.message_item.msg_id` 为 `"7484409538682029960"` |
| `test_message_item_without_ref_msg` | 普通消息没有 ref_msg | `item.ref_msg` 为 `None` |

```rust
#[test]
fn test_message_item_with_ref_msg() {
    let json = r#"{
        "type": 1,
        "text_item": {"text": "这是真的吗？"},
        "ref_msg": {
            "message_item": {
                "msg_id": "7484409538682029960",
                "type": 0
            }
        }
    }"#;
    let item: MessageItem = serde_json::from_str(json).unwrap();
    let ref_msg = item.ref_msg.unwrap();
    assert_eq!(ref_msg.message_item.msg_id, "7484409538682029960");
}

#[test]
fn test_message_item_without_ref_msg() {
    let json = r#"{"type": 1, "text_item": {"text": "hello"}}"#;
    let item: MessageItem = serde_json::from_str(json).unwrap();
    assert!(item.ref_msg.is_none());
}
```

#### 8.1.3 AgentReply agent_context 序列化/反序列化

**文件：** `gateway/src/ilink/types.rs`

| 标题 | 描述 | 预期 |
|------|------|------|
| `test_agent_reply_with_agent_context` | agent_context 存在时序列化/反序列化正常 | 往返正确 |
| `test_agent_reply_without_agent_context` | agent_context 为 None 时序列化不包含该字段（向后兼容） | JSON 中无 `agent_context` 字段 |

```rust
#[test]
fn test_agent_reply_with_agent_context() {
    let reply = AgentReply {
        reply_to_id: "msg-1".to_string(),
        text: "hello back".to_string(),
        media_paths: vec![],
        to_user: None,
        context_token: None,
        agent_context: Some(r#"{"agent":"claude","workspace":"my-project"}"#.to_string()),
    };
    let json = serde_json::to_string(&reply).unwrap();
    let deserialized: AgentReply = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.agent_context.unwrap(), r#"{"agent":"claude","workspace":"my-project"}"#);
}

#[test]
fn test_agent_reply_without_agent_context() {
    let reply = AgentReply {
        reply_to_id: "msg-1".to_string(),
        text: "hello back".to_string(),
        media_paths: vec![],
        to_user: None,
        context_token: None,
        agent_context: None,
    };
    let json = serde_json::to_string(&reply).unwrap();
    let deserialized: AgentReply = serde_json::from_str(&json).unwrap();
    assert!(deserialized.agent_context.is_none());
    // 验证向后兼容：旧版客户端发来的报文（不含 agent_context）也能解析
    let old_json = r#"{"reply_to_id":"msg-1","text":"hello back"}"#;
    let deserialized_old: AgentReply = serde_json::from_str(old_json).unwrap();
    assert!(deserialized_old.agent_context.is_none());
    assert_eq!(deserialized_old.reply_to_id, "msg-1");
}
```

#### 8.1.4 SQLite msg_agent_context 操作

**文件：** `gateway/src/storage/sqlite_store.rs`

| 标题 | 描述 | 预期 |
|------|------|------|
| `test_save_and_get_msg_agent_context` | 保存后能正确查询到 | `get_msg_agent_context` 返回保存的值 |
| `test_get_msg_agent_context_not_found` | 不存在的 msg_id 返回 None | 返回 `None` |
| `test_msg_agent_context_max_limit` | 超过 200 条时自动淘汰最早那条 | 保存第 201 条后，最旧的那条被删除 |
| `test_msg_agent_context_update_existing` | 相同 msg_id 更新覆盖 | 再次 save 后内容更新 |

```rust
#[test]
fn test_save_and_get_msg_agent_context() {
    let file = NamedTempFile::new().unwrap();
    let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();
    store.save_msg_agent_context("7484409538682029960", r#"{"agent":"claude"}"#).unwrap();
    let result = store.get_msg_agent_context("7484409538682029960").unwrap();
    assert_eq!(result.unwrap(), r#"{"agent":"claude"}"#);
}

#[test]
fn test_get_msg_agent_context_not_found() {
    let file = NamedTempFile::new().unwrap();
    let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();
    let result = store.get_msg_agent_context("nonexistent").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_msg_agent_context_max_limit() {
    let file = NamedTempFile::new().unwrap();
    let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();
    // 插入 200 条
    for i in 0..200 {
        store.save_msg_agent_context(&format!("msg-{i}"), r#"{"agent":"test"}"#).unwrap();
    }
    // 再插入一条新的，应该是第 201 条
    store.save_msg_agent_context("msg-200", r#"{"agent":"new"}"#).unwrap();
    // 最早那条 msg-0 应该被删除
    assert!(store.get_msg_agent_context("msg-0").unwrap().is_none());
    // 最新的那条可以查到
    assert!(store.get_msg_agent_context("msg-200").unwrap().is_some());
}

#[test]
fn test_msg_agent_context_update_existing() {
    let file = NamedTempFile::new().unwrap();
    let store = SqliteStore::new(file.path().to_str().unwrap()).unwrap();
    store.save_msg_agent_context("msg-1", r#"{"agent":"old"}"#).unwrap();
    store.save_msg_agent_context("msg-1", r#"{"agent":"new"}"#).unwrap();
    let result = store.get_msg_agent_context("msg-1").unwrap().unwrap();
    assert_eq!(result, r#"{"agent":"new"}"#);
}
```

#### 8.1.5 Router route_to_agent

**文件：** `gateway/src/router/router.rs`

| 标题 | 描述 | 预期 |
|------|------|------|
| `test_route_to_agent_routes_to_specified_agent` | 路由到指定 agent，消息进入该 agent 队列 | 目标 agent 有 pending 消息 |
| `test_route_to_agent_unknown_agent_returns_error` | 指定 agent 不存在时返回错误 | 返回 `AgentNotFound` 错误 |
| `test_route_to_agent_dm_policy_check` | DM policy 为 disabled 时丢弃 | 消息不被入队 |

```rust
#[test]
fn test_route_to_agent_routes_to_specified_agent() {
    let mut router = setup();
    register_agent(&mut router, "hermes");
    register_agent(&mut router, "zeus");
    // active_agent 是 hermes，但 route_to_agent 到 zeus
    let msg = make_text_msg("引用回复到 zeus");
    router.route_to_agent(&msg, "zeus", None).unwrap();
    assert!(!router.queue().has_pending("hermes"));
    assert!(router.queue().has_pending("zeus"));
}

#[test]
fn test_route_to_agent_unknown_agent_returns_error() {
    let mut router = setup();
    let msg = make_text_msg("hello");
    let result = router.route_to_agent(&msg, "nobody", None);
    assert!(result.is_err());
    match result {
        Err(GatewayError::AgentNotFound(name)) => assert_eq!(name, "nobody"),
        _ => panic!("expected AgentNotFound"),
    }
}
```

#### 8.1.6 resilient_send 返回 message_id

**文件：** `gateway/src/main.rs`

| 标题 | 描述 | 预期 |
|------|------|------|
| `test_resilient_send_returns_message_id` | 模拟 iLink 返回 message_id 时 | `resilient_send` 返回 `Ok(Some(id_string))` |
| `test_resilient_send_returns_none` | 响应没有 message_id 时 | 返回 `Ok(None)` |

### 8.2 TypeScript 单元测试

#### 8.2.1 AgentMessage 兼容性

**文件：** `client/claude-code-adapter/src/gateway-client.test.ts`

| 标题 | 描述 | 预期 |
|------|------|------|
| AgentMessage 无 agent_context 时解析正常 | gateway 旧版下发的消息 | `msg.agent_context` 为 `undefined` |
| AgentMessage 有 agent_context 时解析正常 | gateway 新版下发的引用回复 | `msg.agent_context` 为 JSON 字符串 |

#### 8.2.2 handleMessage 二次路由

**文件：** `client/claude-code-adapter/src/index.test.ts`

| 标题 | 描述 | 预期 |
|------|------|------|
| 引用回复路由到指定 workspace | agent_context 中 workspace 和当前不同 | 按目标 workspace 创建 session 并处理 |
| 引用回复的 workspace 就是当前 workspace | agent_context 中 workspace 和当前一致 | 走正常流程 |
| agent_context 解析失败 | 非法 JSON | 走 fallback，按当前 workspace 处理 |

## 9. 开发顺序

建议按以下顺序实现，每个阶段可独立验证：

1. **Rust 数据结构定义**（任务 1-5）— 类型安全，不改变逻辑
2. **SQLite 存储**（任务 6）— 独立单元测试
3. **Router route_to_agent**（任务 8）— 独立单元测试
4. **main.rs 引用回复路由**（任务 7 + 9）— 需要 mock 测试
5. **TS 端改动**（任务 10-12）— 端到端验证

## 10. 回滚方案

如果实施后出现问题，各部分的回滚独立：

- **gateway 侧：** 移除 `ref_msg` 相关逻辑，`MessageItem.extra` 会兜底，不会影响反序列化
- **adapter 侧：** 忽略 `agent_context` 字段，消息按当前 workspace 处理（退化为现有行为）
- **SQLite 表：** `msg_agent_context` 表可以安全存在不被使用
- 所有新增字段都是 `Option`/可选，向前兼容
