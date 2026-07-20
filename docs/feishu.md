# 飞书开放平台对接指南

## 应用类型选择

飞书有三种应用类型，决定了能用什么 API：

| 类型 | 说明 | 适用场景 |
|------|------|---------|
| **企业自建应用** | 只能在自己企业内使用 | 内部 bot、自动化工具 ← 最常见 |
| **应用商店应用** | 可发布到飞书应用市场，多企业使用 | SaaS 产品 |
| **ISV 应用** | 服务商模式，代为管理多企业 | 定制开发服务商 |

一般选**企业自建应用**。

---

## 用户身份体系

飞书有三层用户 ID：

| ID 类型 | 前缀 | 作用域 | 稳定性 |
|---------|------|--------|:---:|
| `open_id` | `ou_xxx` | 应用级别 — 同一个人在两个飞书应用中 open_id 不同 | ⚠️ |
| `user_id` | `u_xxx` | 租户级别 — 公司内唯一 | 较高 |
| `union_id` | `on_xxx` | 开发者级别 — 同一开发者所有应用共享 | ★最稳定 |

---

## 认证鉴权

飞书 API 几乎全部需要 `tenant_access_token`，过期时间 2 小时（7200s），需要提前刷新（建议预留 60-120s buffer）。

### 获取 token

```
POST https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal
Content-Type: application/json

{
    "app_id": "cli_xxxxxxxx",
    "app_secret": "xxxxxx"
}
```

响应：

```json
{
    "code": 0,
    "msg": "ok",
    "tenant_access_token": "t-xxxxxxxxxxxxx",
    "expire": 7200
}
```

### 调用 API 携带 token

```
GET /open-apis/xxx
Authorization: Bearer t-xxxxxxxxxxxxx
```

### SDK 自动管理

官方 SDK 内置了 token 自动刷新，推荐使用：

```bash
pip install lark-oapi
```

```python
from lark_oapi import Client

client = Client.builder() \
    .app_id("cli_xxx") \
    .app_secret("xxx") \
    .build()

# SDK 内部自动获取和刷新 tenant_access_token
# 之后所有 API 调用都通过 client 进行，不用手动管理 token
```

---

## 接收消息

### 方式 1：WebSocket 长连接（推荐，不需要公网 URL）

```python
import lark_oapi as lark

# 1. 定义事件处理器（v2.0 事件用 register_p2_xxx）
def do_p2_im_message_receive_v1(data: lark.im.v1.P2ImMessageReceiveV1) -> None:
    print(data)

event_handler = lark.EventDispatcherHandler.builder("", "") \
    .register_p2_im_message_receive_v1(do_p2_im_message_receive_v1) \
    .build()

# 2. 启动 WebSocket 长连接
ws = lark.ws.Client(
    "cli_xxx",
    "xxx",
    event_handler=event_handler,
    log_level=lark.LogLevel.DEBUG,
)
ws.start()
```

**群聊 @机器人事件能否收到**，取决于三件事：①后台「事件与回调」订阅了 `im.message.receive_v1`；②订阅方式选「使用长连接接收事件」并保存（保存时本地 client **必须已在线**，否则保存失败）；③机器人被拉进群且有 `im:message` 权限。与 User-Agent / `extra_ua_tags` 等参数无关。

**长连接限制**：
- 仅支持**企业自建应用**，商店应用必须用 Webhook
- 每应用最多 **50 个**长连接
- 收到消息后必须在 **3 秒**内处理完成（不抛异常），否则触发超时重推
- **集群模式**：同一 app_id 多个 client 只有**随机一个**收到消息，不是广播也不是重复

### 方式 2：Webhook 回调

需要**公网可达**的 HTTP 服务，在飞书后台配置回调地址。

```
POST https://your-server.com/feishu/webhook
Content-Type: application/json

{
    "schema": "2.0",
    "header": {
        "event_id": "...",
        "event_type": "im.message.receive_v1",
        "token": "***"
    },
    "event": { ... }
}
```

**Webhook 验签流程**：飞书会先发一个 URL 验证请求（challenge），你解密后原样返回 `challenge` 字段即可通过验证。分两种模式：

- **明文模式**（未配置 Encrypt Key）：用 `Verification Token` 校验请求来源（header 中的 token 字段）
- **加密模式**（配置了 Encrypt Key）：请求体经 **AES-256-CBC** 加密（key = `SHA256(Encrypt Key)`），解密后取出 `challenge` 原样返回

需要配置 `FEISHU_VERIFICATION_TOKEN` 和 `FEISHU_ENCRYPT_KEY`。challenge 验证与事件回调一样要求秒级响应，超时会重试。

### 事件类型

| 事件 | 说明 |
|------|------|
| `im.message.receive_v1` | 收到消息（文本/图片/文件/音频/富文本/卡片/表情） |
| `im.message.message_read_v1` | 消息已读 |
| `card.action.trigger` | 交互卡片按钮点击 |

---

## 发送消息

### 统一端点

```
POST https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type={id_type}
Authorization: Bearer t-xxx
Content-Type: application/json

{
    "receive_id": "{接收者ID}",
    "msg_type": "{消息类型}",
    "content": "{JSON字符串}"
}
```

**⚠️ 关键坑**：`content` 必须是 **JSON 字符串**，不是 JSON 对象。

### `receive_id_type` 参数

| 值 | 含义 | 对应 ID 前缀 |
|----|------|:---:|
| `open_id` | 用户的应用内 ID | `ou_xxx` |
| `user_id` | 企业内用户 ID | 数字或 `u_xxx` |
| `union_id` | 开发者维度唯一用户 ID | `on_xxx` |
| `email` | 邮箱 | - |
| `chat_id` | 群聊 ID | `oc_xxx` |

### 消息类型

`msg_type` 决定 `content` 的结构：

| msg_type | content 结构 | 用途 |
|----------|-------------|------|
| `text` | `{"text":"纯文本"}` | 普通消息 |
| `post` | `{"zh_cn":{"title":"","content":[[...元素]]}}` | 富文本（加粗/斜体/链接/@人/代码块） |
| `interactive` | 飞书卡片 JSON | 交互卡片（按钮、表单） |
| `image` | `{"image_key":"img_xxx"}` | 图片（需先上传） |
| `file` | `{"file_key":"file_xxx"}` | 文件 |
| `audio` | `{"file_key":"file_xxx"}` | 语音 |
| `media` | `{"file_key":"file_xxx"}` | 视频 |
| `share_chat` | `{"chat_id":"oc_xxx"}` | 分享群名片 |

### 发送文本消息

```json
{
    "receive_id": "ou_xxx",
    "msg_type": "text",
    "content": "{\"text\":\"hello world\"}"
}
```

### 发送富文本消息（post）

```json
{
    "receive_id": "ou_xxx",
    "msg_type": "post",
    "content": "{\"zh_cn\":{\"title\":\"\",\"content\":[[{\"tag\":\"text\",\"text\":\"这是 \"},{\"tag\":\"text\",\"text\":\"加粗\",\"style\":[\"bold\"]},{\"tag\":\"text\",\"text\":\" 文字\"}]]}}"
}
```

post 富文本元素：

| tag | 用途 | 格式 |
|-----|------|------|
| `text` | 文本 | `{"tag":"text","text":"内容","style":["bold","italic"]}` |
| `a` | 超链接 | `{"tag":"a","text":"点击","href":"https://..."}` |
| `at` | @提及 | `{"tag":"at","user_id":"ou_xxx","user_name":"张三"}` |
| `img` | 内嵌图片 | `{"tag":"img","image_key":"img_xxx"}` |
| `media` | 内嵌文件 | `{"tag":"media","file_key":"file_xxx"}` |

### 回复消息

```
POST https://open.feishu.cn/open-apis/im/v1/messages/{message_id}/reply
Authorization: Bearer t-xxx
Content-Type: application/json

{
    "content": "{\"text\":\"回复内容\"}",
    "msg_type": "text"
}
```

如果被回复的消息已被撤回或不存在，会返回特定错误码，需要 fallback 到直接发新消息。

---

## 上传文件/图片

### 上传图片

```
POST https://open.feishu.cn/open-apis/im/v1/images
Content-Type: multipart/form-data

image_type=message
image=@/path/to/image.png
```

返回 `{"code":0,"data":{"image_key":"img_xxx"}}`

### 上传文件

```
POST https://open.feishu.cn/open-apis/im/v1/files
Content-Type: multipart/form-data

file_type=stream
file_name=test.pdf
file=@/path/to/file
```

`file_type` 映射：

| 扩展名 | file_type |
|--------|-----------|
| .opus/.ogg | `opus` |
| .mp4 | `mp4` |
| .pdf | `pdf` |
| .doc | `doc` |
| .docx | `docx` |
| .xls | `xls` |
| .xlsx | `xlsx` |
| .ppt | `ppt` |
| .pptx | `pptx` |
| 其他 | `stream` |

返回 `{"code":0,"data":{"file_key":"file_xxx"}}`

---

## 其他常用 API

### 获取用户信息

```
GET /open-apis/contact/v3/users/{user_id}?user_id_type=open_id
Authorization: Bearer t-xxx
```

返回姓名、邮箱、部门等。

需要 scope: `contact:user.employee_id:readonly` 或 `contact:user.base:readonly`

### 获取机器人信息

```
GET /open-apis/bot/v3/info
Authorization: Bearer t-xxx
```

返回 `{"bot":{"app_name":"机器人名称","open_id":"ou_xxx"}}`

用于群聊 @提及匹配——对比事件 `mentions` 中的 `open_id` 是否等于 bot 的 `open_id`。

### 批量获取 Bot 信息

```
GET /open-apis/bot/v3/bots/basic_batch?bot_ids=ou_xxx&bot_ids=ou_yyy
Authorization: Bearer t-xxx
```

### 获取群聊信息

```
GET /open-apis/im/v1/chats/{chat_id}
Authorization: Bearer t-xxx
```

返回群名、群类型（`p2p` 私聊 / `private` 内部群 / `public` 公开群）。

### 获取消息内容

```
GET /open-apis/im/v1/messages/{message_id}
Authorization: Bearer t-xxx
```

### 下载消息中的文件/图片

```
GET /open-apis/im/v1/messages/{message_id}/resources/{file_key}?type=image
Authorization: Bearer t-xxx
```

`type` 可选值：`image` / `file`

---

## 群聊 @提及匹配

当用户 @机器人时，飞书事件 `mentions` 数组包含被 @用户的 ID：

```json
{
    "event": {
        "message": {
            "mentions": [
                {
                    "key": "@_user_1",
                    "id": {"open_id": "ou_bot_xxx"},
                    "name": "MyBot"
                }
            ]
        }
    }
}
```

匹配逻辑：

```python
def is_bot_mentioned(mentions, bot_open_id, bot_user_id=None, bot_name=None):
    """判断机器人是否被 @提及，支持三路 fallback"""
    for m in mentions:
        mid = m.get("id", {})
        # 第一路：open_id 精确匹配
        if bot_open_id and mid.get("open_id") == bot_open_id:
            return True
        # 第二路：user_id 匹配
        if bot_user_id and mid.get("user_id") == bot_user_id:
            return True
        # 第三路：bot_name 匹配
        if bot_name and m.get("name") == bot_name:
            return True
    return False
```

**⚠️ 提醒**：`mentions[].id` 里通常只可靠地包含 `open_id`，`user_id` / `union_id` 可能为空——优先用 `open_id` 匹配，其余两路作 fallback。事件能否推送见上文「接收消息」部分。

---

## 交互卡片

飞书卡片是 JSON Schema，支持按钮、表单、图片等。发送时 `msg_type="interactive"`。

### 审批卡片示例

```json
{
    "config": {"wide_screen_mode": true},
    "header": {
        "title": {"content": "⚠️ 需要确认", "tag": "plain_text"},
        "template": "orange"
    },
    "elements": [
        {
            "tag": "markdown",
            "content": "确认执行以下命令？\n```\nrm -rf /\n```"
        },
        {
            "tag": "action",
            "actions": [
                {
                    "tag": "button",
                    "text": {"tag": "plain_text", "content": "✅ 确认"},
                    "type": "primary",
                    "value": {"action": "approve", "id": "123"}
                },
                {
                    "tag": "button",
                    "text": {"tag": "plain_text", "content": "❌ 取消"},
                    "type": "danger",
                    "value": {"action": "deny", "id": "123"}
                }
            ]
        }
    ]
}
```

`value` 中的自定义字段会原样出现在 `card.action.trigger` 回调中。

### 卡片按钮回调

用户点击按钮 → 飞书发送 `card.action.trigger` 事件 → 你需要返回 `P2CardActionTriggerResponse` 来替换卡片：

```python
# 响应可选的 CallBackCard 来内联替换卡片
response = P2CardActionTriggerResponse()
response.card = CallBackCard({"config": {...}, "elements": [...]})
```

如果不返回，卡片保持不变。

---

## 权限 Scope

在飞书开放平台后台「权限管理」中配置 API 权限，常用 scope：

| Scope | 用途 | 必需？ |
|-------|------|:---:|
| `im:message:send_as_bot` | 以机器人身份**发送**消息 | ✅ 发消息必需 |
| `im:message` | 读取用户发给机器人的消息内容（配合事件订阅） | ✅ 收消息必需 |
| `im:resource` | 上传/下载图片和文件 | 按需 |
| `contact:user.employee_id:readonly` | 读取员工工号 | 按需 |
| `contact:user.base:readonly` | 读取用户基本信息（姓名等） | 按需 |
| `admin:app.info:readonly` | 读取应用信息 | 按需 |
| `application:application:self_manage` | 读取自身应用信息 | 按需 |

---

## 完整对接步骤

```
1. 创建应用 → 飞书开放平台 → 企业自建应用 → 获取 app_id + app_secret
2. 配置权限 → 后台添加所需 scope（至少 im:message + im:message:send_as_bot）
3. 发布版本 → 创建版本并发布（企业自建应用也需发布，可指定可见范围）
4. 获取 token → POST /auth/v3/tenant_access_token/internal
5. 接收消息 → WebSocket 长连接 或 Webhook 回调
6. 处理消息 → 去重 + 准入检查 + @提及匹配 + 消息类型分支处理
7. 发送消息 → POST /im/v1/messages（注意 content 是 JSON 字符串）
8. 上传文件 → POST /im/v1/files（multipart）→ 获取 file_key → 发送
```

---

## 踩坑清单

| 坑 | 说明 |
|----|------|
| **content 是 JSON 字符串** | `"{\"text\":\"hello\"}"` 不是 `{"text":"hello"}` |
| **群聊 @mention 不推送** | WebSocket 必须带 `extra_ua_tags=["channel"]` |
| **post 不渲染表格** | 飞书 post 类型不支持 markdown 表格，遇表格需 fallback 到 text |
| **WebSocket 断开残留** | 断开时要发 CLOSE 帧，否则飞书服务端 CLOSE-WAIT 可能持续几分钟到几小时 |
| **token 过期** | 7200s，不要在临界点刷新，建议提前 60-120s |
| **Webhook URL 验证** | 必须在 1 秒内返回解密后的 challenge |
| **同 app_id 多连接（集群模式）** | 多个 client 只有**随机一个**收到消息（不是重复推送）；每应用最多 50 个连接 |
| **消息重复** | 飞书可能推送重复消息，需要基于 `message_id` 做去重 |
| **回复已撤回消息** | 回复被撤回消息会失败，需要 fallback 到发送新消息 |
| **文件上传 file_type** | 不同扩展名对应不同 `file_type`，用错会导致上传失败（如 .opus 必须用 `opus` 而不是 `stream`） |
| **图片消息裁剪** | 图片默认 800px 宽，通过 `/im/v1/images` 上传时支持设置 `width`/`height` |
| **发消息频率限制** | 单应用发消息有 QPS 上限（详见后台「频率限制」），批量发送会触发 429，需客户端限流 |
| **消息体大小** | `post` 富文本约 30KB 上限（`text` 类型上限更大），长输出需分片发送 |

---

## 官方文档

- 飞书开放平台：[https://open.feishu.cn/document](https://open.feishu.cn/document)
- 服务端 SDK：[lark-oapi](https://github.com/larksuite/oapi-sdk-python)
- 消息卡片搭建工具：[https://open.feishu.cn/tool/cardbuilder](https://open.feishu.cn/tool/cardbuilder)
- 用户身份说明：[https://open.feishu.cn/document/home/user-identity-introduction/introduction](https://open.feishu.cn/document/home/user-identity-introduction/introduction)
