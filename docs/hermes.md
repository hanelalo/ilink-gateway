# Hermes ACP 对接指南

> Hermes Agent 的 ACP（Agent Communication Protocol）协议完整对接文档。
> 版本：基于 Hermes Agent v2.1.0 + ACP schema v0.11.2

---

## 1. 概述

### 1.1 什么是 ACP？

ACP（Agent Communication Protocol）是 Hermes Agent 为编辑器集成（VS Code、Zed、JetBrains）提供的标准协议。客户端通过**启动 `hermes acp` 子进程**，用 **JSON-RPC 2.0 over stdio** 与 Hermes 通信。

### 1.2 传输层

```
客户端 (你的程序)
  ├── stdin  → JSON-RPC 请求    → hermes acp 子进程
  └── stdout ← JSON-RPC 响应/通知 ← hermes acp 子进程
```

- **stdin**：客户端写入 JSON-RPC 请求，每行一个完整的 JSON 对象
- **stdout**：读取 JSON-RPC 响应和通知（通知没有 `id` 字段）
- **stderr**：Hermes 的日志输出（可忽略或重定向到日志文件）

### 1.3 启动命令

```bash
hermes acp
```

或者在代码中：

```rust
// Rust 示例
let mut child = Command::new("hermes")
    .arg("acp")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())
    .spawn()?;
```

---

## 2. JSON-RPC 帧格式

### 2.1 请求帧（客户端 → Hermes）

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientCapabilities": {},
    "clientInfo": {
      "name": "your-client-name",
      "version": "0.1.0"
    }
  }
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `jsonrpc` | `string` | 固定 `"2.0"` |
| `id` | `uint64` | 请求 ID，从 1 开始自增，用于匹配响应 |
| `method` | `string` | ACP 方法名（见 §3） |
| `params` | `object?` | 方法参数，按 ACP 规范使用 camelCase |

### 2.2 响应帧（Hermes → 客户端）

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": 1,
    "agentInfo": { "name": "hermes-agent", "version": "2.1.0" },
    "agentCapabilities": { ... }
  }
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `jsonrpc` | `string` | 固定 `"2.0"` |
| `id` | `uint64` | 对应请求的 ID |
| `result` | `object?` | 成功时的返回值 |
| `error` | `object?` | 失败时的错误对象 `{ code, message, data? }` |

### 2.3 通知帧（Hermes → 客户端，无 `id`）

ACP 支持**流式推送**：在 `session/prompt` 执行期间，Hermes 会持续发送不带 `id` 的通知帧，直到最终响应帧返回。

```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "abc-123",
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": { "type": "text", "text": "正在处理你的请求..." }
    }
  }
}
```

**通知帧判断规则**：JSON 对象中缺少 `id` 字段即为通知。

---

## 3. 完整 API 方法列表

ACP 协议版本：**v0.11.2**，对应 JSON-RPC method 名称如下：

| 方法名 | 类型 | 说明 | 稳定性 |
|--------|------|------|--------|
| `initialize` | Request | 握手，交换能力信息 | **稳定** |
| `authenticate` | Request | 认证（终端 OAuth/provider 凭证） | **稳定** |
| `session/new` | Request | 创建新会话 | **稳定** |
| `session/load` | Request | 加载已有会话（不存在返回 null） | **稳定** |
| `session/resume` | Request | 恢复会话（不存在则创建新的） | ⚠️ unstable |
| `session/fork` | Request | 分支会话（复制历史到新会话） | ⚠️ unstable |
| `session/list` | Request | 列出所有会话 | **稳定** |
| `session/prompt` | Request | 发送用户消息 → AI 回复 | **稳定** |
| `session/close` | Request | 关闭会话 | ⚠️ unstable |
| `session/set_model` | Request | 切换模型 | ⚠️ unstable |
| `session/set_mode` | Request | 切换模式（编辑审批策略） | **稳定** |
| `session/set_config_option` | Request | 设置配置选项 | **稳定** |
| `session/cancel` | Notification | 取消当前正在执行的 turn | **稳定** |
| `session/update` | Notification | 服务端推送（流式响应、工具调用等） | **稳定** |

---

## 4. 协议交互流程

### 4.1 完整生命周期

```
客户端                          hermes acp
  │                                  │
  │────── initialize ───────────────→│  ① 握手
  │←───── InitializeResponse ───────│
  │                                  │
  │────── session/new ──────────────→│  ② 创建会话
  │←───── NewSessionResponse ───────│     (返回 sessionId)
  │                                  │
  │────── session/prompt ───────────→│  ③ 发送消息
  │←───── session/update ───────────│     (流式推送，多次)
  │←───── session/update ───────────│     (agent_message_chunk)
  │←───── session/update ───────────│     (tool_call_start)
  │←───── session/update ───────────│     (tool_call_update)
  │←───── session/update ───────────│     (agent_thought_chunk)
  │←───── PromptResponse ───────────│     最终响应 (带 id)
  │                                  │
  │────── session/close ───────────→│  ④ 关闭会话
  │←───── CloseSessionResponse ─────│
```

### 4.2 多轮对话

同一 `sessionId` 下可以多次调用 `session/prompt`，Hermes 内部维护完整对话历史：

```
  │────── session/prompt ("你好") ──→│
  │←───── ...更新... + PromptResponse│
  │                                  │
  │────── session/prompt ("继续") ──→│   ← 同一 sessionId
  │←───── ...更新... + PromptResponse│     历史自动保留
```

---

## 5. 各方法详解

### 5.1 `initialize` — 握手

**请求参数**：

```jsonc
{
  "protocolVersion": 1,          // ACP 协议版本，当前为 1
  "clientCapabilities": {},      // 客户端能力声明（可为空对象）
  "clientInfo": {
    "name": "my-app",
    "version": "1.0.0"
  }
}
```

**响应**：

```jsonc
{
  "protocolVersion": 1,
  "agentInfo": {
    "name": "hermes-agent",
    "version": "2.1.0"
  },
  "agentCapabilities": {
    "loadSession": true,
    "promptCapabilities": { "image": true },
    "sessionCapabilities": {
      "fork": {},
      "list": {},
      "resume": {}
    }
  }
  // "authMethods": [...]   // 如果配置了认证
}
```

> **注意**：`initialize` 必须在任何其他请求之前调用。

---

### 5.2 `session/new` — 创建新会话

**请求参数**：

```jsonc
{
  "cwd": "/absolute/path/to/workspace",   // 工作目录（必填，绝对路径）
  "mcpServers": []                         // MCP 服务器配置（可选）
}
```

**响应**：

```jsonc
{
  "sessionId": "550e8400-e29b-41d4-a716-446655440000",  // UUID v4
  "models": {                              // 可用模型列表
    "availableModels": [
      {
        "modelId": "openrouter:anthropic/claude-sonnet-4",
        "name": "anthropic/claude-sonnet-4",
        "description": "Provider: OpenRouter • current"
      }
    ],
    "currentModelId": "openrouter:anthropic/claude-sonnet-4"
  },
  "modes": {                               // 可用模式
    "availableModes": [
      {
        "id": "default",
        "name": "Default",
        "description": "Ask before edits."
      },
      {
        "id": "accept_edits",
        "name": "Accept Edits",
        "description": "Auto-allow workspace and /tmp edits..."
      },
      {
        "id": "dont_ask",
        "name": "Don't Ask",
        "description": "Auto-allow file edits..."
      }
    ],
    "currentModeId": "default"
  }
}
```

**关键字段**：
- `sessionId`：后续所有会话操作（prompt/close/load）都需要传递此 ID
- `cwd`：必须是**绝对路径**，定义了 Hermes 的操作上下文
- `mcpServers`：可选的 MCP 服务器列表（stdio/http/sse），Hermes 启动后自动注册

---

### 5.3 `session/prompt` — 发送消息（核心方法）

这是**最复杂也最常用**的方法。它是**流式**的——Hermes 在执行期间持续推送更新通知。

**请求参数**：

```jsonc
{
  "sessionId": "550e8400-...",
  "prompt": [
    {
      "type": "text",
      "text": "帮我写一个 Python 脚本"
    }
  ]
}
```

**prompt 内容块类型**：

| type | 说明 | 额外字段 |
|------|------|----------|
| `"text"` | 纯文本 | `text: string` |
| `"image"` | 图片 | `data: string` (base64) 或 `uri: string`，`mimeType: string` |
| `"resource_link"` | 文件引用 | `uri: string`，`name?: string`，`mimeType?: string` |
| `"resource"` | 嵌入式资源 | `resource: { uri, text? \| blob? }` |

**流式推送通知类型**（`session/update` 的 `update.sessionUpdate` 字段）：

| sessionUpdate 值 | 携带数据 | 说明 |
|------------------|----------|------|
| `"agent_message_chunk"` | `content: { type: "text", text: "..." }` | AI 回复的文本片段 |
| `"agent_thought_chunk"` | `content: { type: "text", text: "..." }` | AI 的思考过程（reasoning） |
| `"tool_call_start"` | `toolCallId, name, args, kind` | 工具调用开始 |
| `"tool_call_update"` | `toolCallId, status, rawOutput, content` | 工具调用状态更新 |
| `"plan"` | `entries: [{ content, status, priority }]` | 任务计划更新 |
| `"user_message_chunk"` | `content: { type: "text", text: "..." }` | 历史消息回放 |
| `"usage_update"` | `size, used` | 上下文窗口用量 |
| `"available_commands_update"` | `availableCommands: [...]` | 可用斜杠命令 |
| `"session_info_update"` | `title, updatedAt` | 会话元数据 |

**最终响应**：

```jsonc
{
  "stopReason": "end_turn",         // end_turn | cancelled | max_tokens | refusal
  "usage": {                         // 可能不存在
    "inputTokens": 1234,
    "outputTokens": 567,
    "totalTokens": 1801,
    "thoughtTokens": 0         // reasoning tokens（可选）
  }
}
```

**从 `session/update` 通知中提取 AI 回复文本**：

```rust
// 伪代码
let mut reply_parts = Vec::new();

loop {
    let line = read_line_from_stdout();
    let v: Value = serde_json::from_str(&line)?;

    // 如果是最终响应（有我们的 id），停止
    if v.get("id") == Some(&request_id) {
        break;
    }

    // 收集 agent_message_chunk 中的文本
    if let Some(text) = v["params"]["update"]
        .get("content")
        .and_then(|c| c["text"].as_str())
    {
        reply_parts.push(text);
    }
}

let full_reply = reply_parts.join("");
```

---

### 5.4 `session/load` — 加载已有会话

用于恢复之前创建的会话（跨进程重启）。

**请求参数**：

```jsonc
{
  "sessionId": "之前保存的 sessionId",
  "cwd": "/absolute/path"
}
```

**响应**：与 `session/new` 类似（`models` + `modes`），**同时在响应返回前会通过 `session/update` 回放完整历史对话**。

> **注意**：`session/load` 要求会话存在。如果不存在，返回 JSON-RPC error。若希望"不存在则创建"，使用 `session/resume`（unstable）。

---

### 5.5 `session/close` — 关闭会话

**请求参数**：

```jsonc
{
  "sessionId": "要关闭的 sessionId"
}
```

---

### 5.6 `session/list` — 列出会话

**请求参数**：

```jsonc
{
  "cwd": "/path/to/filter",    // 可选：按工作目录过滤
  "cursor": "lastSeenId"       // 可选：分页游标
}
```

**响应**：

```jsonc
{
  "sessions": [
    {
      "sessionId": "uuid-1",
      "cwd": "/path/to/project",
      "title": "Write Python script",
      "updatedAt": "2026-07-18T10:30:00Z"
    }
  ],
  "nextCursor": null
}
```

---

### 5.7 `session/set_model` — 切换模型

**请求参数**：

```jsonc
{
  "modelId": "openrouter:anthropic/claude-sonnet-4",
  "sessionId": "目标 sessionId"
}
```

> 模型 ID 格式为 `provider:model_name`，如 `openrouter:deepseek/deepseek-chat`。

---

### 5.8 `session/set_mode` — 切换模式

**请求参数**：

```jsonc
{
  "modeId": "accept_edits",
  "sessionId": "目标 sessionId"
}
```

**可用模式**：

| modeId | 说明 |
|--------|------|
| `default` | 执行编辑前询问用户 |
| `accept_edits` | 自动接受工作区和 /tmp 下的编辑 |
| `dont_ask` | 自动接受所有编辑（除敏感路径） |

---

### 5.9 `session/cancel` — 取消当前执行（通知）

这是**通知**（无 `id`），不需要响应：

```jsonc
{
  "jsonrpc": "2.0",
  "method": "session/cancel",
  "params": {
    "sessionId": "目标 sessionId"
  }
}
```

---

### 5.10 `session/fork` — 分支会话

复制一个会话的完整历史到新会话，常用于"从某个点开始不同探索"。

**请求参数**：

```jsonc
{
  "sessionId": "源 sessionId",
  "cwd": "/absolute/path"
}
```

**响应**：

```jsonc
{
  "sessionId": "新 sessionId UUID",
  "models": { ... },
  "modes": { ... }
}
```

---

### 5.11 `session/resume` — 恢复会话

与 `session/load` 类似，但**如果会话不存在则自动创建新的**（fallback 行为）。

---

## 6. 斜杠命令（/commands）

Hermes ACP 支持通过 `session/prompt` 发送斜杠命令，在本地处理而不调用 LLM：

| 命令 | 说明 | 示例 |
|------|------|------|
| `/help` | 列出所有命令 | `/help` |
| `/model` | 查看/切换模型 | `/model` 或 `/model deepseek/deepseek-chat` |
| `/tools` | 列出可用工具 | `/tools` |
| `/context` | 查看上下文使用情况 | `/context` |
| `/reset` | 清空对话历史 | `/reset` |
| `/compact` | 压缩上下文 | `/compact` |
| `/steer` | 注入指导到运行中的 turn | `/steer use async/await` |
| `/queue` | 排队下一条提示 | `/queue 继续重构` |
| `/version` | 显示 Hermes 版本 | `/version` |

> 斜杠命令响应作为 `agent_message_chunk` 直接返回，不会触发 LLM 调用。

---

## 7. 关键回调：`session/update` 推送类型详解

### 7.1 `agent_message_chunk` — AI 回复文本

```jsonc
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "abc-123",
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": {
        "type": "text",
        "text": "好的，我来帮你写一个 Python 脚本..."
      }
    }
  }
}
```

### 7.2 `agent_thought_chunk` — AI 思考过程

```jsonc
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "abc-123",
    "update": {
      "sessionUpdate": "agent_thought_chunk",
      "content": {
        "type": "text",
        "text": "用户想要一个 Python 脚本，我应该先确认需求..."
      }
    }
  }
}
```

### 7.3 `tool_call_start` — 工具调用开始

```jsonc
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "abc-123",
    "update": {
      "sessionUpdate": "tool_call_start",
      "toolCallId": "tc-1",
      "title": "write_file",
      "kind": "edit",            // read | edit | delete | move | search | execute | think | fetch | other
      "args": {                  // JSON 格式的工具参数
        "path": "/path/to/file.py",
        "content": "print('hello')"
      }
    }
  }
}
```

### 7.4 `tool_call_update` — 工具调用完成/状态变更

```jsonc
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "abc-123",
    "update": {
      "sessionUpdate": "tool_call_update",
      "toolCallId": "tc-1",
      "status": "completed",    // pending | in_progress | completed | failed
      "rawOutput": "文件写入成功",
      "content": [              // 可选的结构化展示内容
        {
          "type": "content",
          "content": {"type": "text", "text": "文件写入成功"}
        }
      ]
    }
  }
}
```

### 7.5 `plan` — 任务计划/待办列表

```jsonc
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "abc-123",
    "update": {
      "sessionUpdate": "plan",
      "entries": [
        { "content": "分析需求", "status": "completed", "priority": "high" },
        { "content": "编写代码", "status": "in_progress", "priority": "medium" },
        { "content": "测试验证", "status": "pending", "priority": "medium" }
      ]
    }
  }
}
```

### 7.6 `usage_update` — 上下文窗口用量

```jsonc
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "abc-123",
    "update": {
      "sessionUpdate": "usage_update",
      "size": 200000,    // 模型上下文窗口大小（tokens）
      "used": 45000       // 当前已用 tokens
    }
  }
}
```

---

## 8. 完整对接示例（Rust）

参考 `wechat-gateway/client/hermes/src/acp/client.rs` 的实现：

```rust
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use serde_json::{json, Value};

// 1. 启动 hermes acp
let mut child = Command::new("hermes")
    .arg("acp")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())
    .spawn()?;

let mut stdin = child.stdin.take().unwrap();
let mut stdout = BufReader::new(child.stdout.take().unwrap());

// 2. 发送 initialize
let init = json!({
    "jsonrpc": "2.0", "id": 1, "method": "initialize",
    "params": {
        "protocolVersion": 1,
        "clientCapabilities": {},
        "clientInfo": { "name": "my-app", "version": "0.1.0" }
    }
});
writeln!(stdin, "{}", init)?;
stdin.flush()?;

// 读取 initialize 响应
let mut line = String::new();
stdout.read_line(&mut line)?;
// line = {"jsonrpc":"2.0","id":1,"result":{...}}

// 3. 创建 session
let msg = json!({
    "jsonrpc": "2.0", "id": 2, "method": "session/new",
    "params": { "cwd": "/home/user/myproject" }
});
writeln!(stdin, "{}", msg)?;
stdin.flush()?;

line.clear();
stdout.read_line(&mut line)?;
let resp: Value = serde_json::from_str(&line)?;
let session_id = resp["result"]["sessionId"].as_str().unwrap();

// 4. 发送 prompt（流式读取）
let msg = json!({
    "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
    "params": {
        "sessionId": session_id,
        "prompt": [{ "type": "text", "text": "写一个 hello world" }]
    }
});
writeln!(stdin, "{}", msg)?;
stdin.flush()?;

let mut reply = String::new();
loop {
    line.clear();
    stdout.read_line(&mut line)?;
    let v: Value = serde_json::from_str(&line)?;

    // 有 id = 最终响应，停止
    if v.get("id").is_some() {
        break;
    }

    // 无 id = 通知，收集 agent_message_chunk 中的 text
    if let Some(text) = v["params"]["update"]["content"]["text"].as_str() {
        reply.push_str(text);
    }
}
println!("AI 回复: {}", reply);

// 5. 关闭 session
let msg = json!({
    "jsonrpc": "2.0", "id": 4, "method": "session/close",
    "params": { "sessionId": session_id }
});
writeln!(stdin, "{}", msg)?;
stdin.flush()?;
```

---

## 9. 多 Session 管理

### 9.1 创建多个 Session

每个 `session/new` 返回独立的 `sessionId`，Hermes 内部通过 `SessionManager` 管理：

```
session/new (cwd="/project-a") → sessionId: "uuid-a"
session/new (cwd="/project-b") → sessionId: "uuid-b"
```

### 9.2 切换 Session

后续 `session/prompt` 请求中传入不同的 `sessionId` 即可切换。无需「激活」操作——每个请求直接指定目标 session：

```json
{ "method": "session/prompt", "params": { "sessionId": "uuid-a", ... } }
// 稍后...
{ "method": "session/prompt", "params": { "sessionId": "uuid-b", ... } }
```

### 9.3 跨进程恢复 Session

Hermes ACP 的 Session 会被持久化到 `~/.hermes/state.db`（SQLite），因此即使 ACP 进程重启，也可以通过 `session/load` 恢复：

```json
{ "method": "session/load", "params": {
    "sessionId": "之前保存的 uuid",
    "cwd": "/原来的工作目录"
}}
```

---

## 10. 重要注意事项

### 10.1 错误处理

- **JSON-RPC 错误码**：`-32700`（Parse error）、`-32601`（Method not found）、`-32602`（Invalid params）、`-32603`（Internal error）
- **stderr** 是日志输出，不要将其与 stdout 的 JSON-RPC 帧混淆
- **stdout 空行**：表示 ACP 进程异常退出

### 10.2 生命周期管理

- 必须先 `initialize`，后 `session/new`
- `session/prompt` 可以多次调用，会话自动维护历史
- 一个 `session/prompt` 执行期间不能在同一 session 上发送第二个 prompt（会被自动排队）
- 使用 `session/cancel` 通知取消正在执行的 prompt

### 10.3 依赖安装

如果 `hermes acp` 报错 "ACP dependencies not installed"：

```bash
pip install -e '.[acp]'
```

### 10.4 与 `hermes chat` 的区别

| | `hermes chat` | `hermes acp` |
|---|--------------|--------------|
| 协议 | 交互式 CLI | JSON-RPC 2.0 over stdio |
| 用途 | 人工使用 | 程序对接 |
| 输出 | 终端 UI | JSON 帧 |
| Session 管理 | 通过 `--resume`/`--continue` | 通过 `sessionId` 参数 |

---

## 参考

- ACP 协议源码：`acp_adapter/`（`~/.hermes/hermes-agent/acp_adapter/`）
- ACP 协议包：`acp` Python 包（`venv/lib/python3.11/site-packages/acp/`）
- Hermes ACP 文档：`https://hermes-agent.nousresearch.com/docs/`
- 实际客户端实现：`wechat-gateway/client/hermes/src/acp/client.rs`
