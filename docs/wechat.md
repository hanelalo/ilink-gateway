# Hermes Agent 微信接入完整文档

> Hermes Agent（前身 OpenClaw）通过 Gateway 的 `WeixinAdapter` 对接微信个人账号。
> 底层基于腾讯 **iLink Bot API**，扫码登录，无需手动配置 API token。

---

## 1. 整体架构

```
微信用户 ──→ 微信服务器 ──→ iLink Bot API ──→ Hermes Gateway
                                                 │
                                            WeixinAdapter
                                            (long-poll)
                                                 │
                                            Agent Core
                                            (LLM + Tools)
                                                 │
微信用户 ←── 微信服务器 ←── iLink Bot API ←── Hermes Gateway
```

| 概念 | 说明 |
|------|------|
| **Gateway** | Hermes 的消息网关进程，负责所有平台的连接管理 |
| **WeixinAdapter** | 微信平台适配器（`gateway/platforms/weixin.py`，2379 行） |
| **iLink Bot API** | 腾讯提供的个人微信 Bot API（`ilinkai.weixin.qq.com`） |
| **Long Poll** | 消息接收方式：35 秒超时的 HTTP 长轮询 |
| **context_token** | 会话上下文令牌，每次回复必须回传最新的 |
| **AES-128-ECB** | 媒体文件（图片/视频/文件/语音）的 CDN 加密协议 |

---

## 2. 完整交互流程

### 2.1 扫码登录

```
Hermes                  iLink API                  微信App
  │                         │                         │
  │ GET /ilink/bot/get_bot_qrcode?bot_type=3          │
  │────────────────────────>│                         │
  │ {qrcode, qrcode_img_content}                      │
  │<────────────────────────│                         │
  │                         │                         │
  │ 生成二维码PNG           │     用户扫码             │
  │                         │<────────────────────────│
  │                         │                         │
  │ GET /ilink/bot/get_qrcode_status?qrcode=...       │
  │ (轮询，1s 间隔)         │                         │
  │────────────────────────>│                         │
  │                         │                         │
  │ status: "scaned"        │   用户在微信点确认       │
  │<────────────────────────│<────────────────────────│
  │                         │                         │
  │ status: "confirmed"     │                         │
  │ {ilink_bot_id, bot_token, baseurl}                │
  │<────────────────────────│                         │
  │                         │                         │
  │ 保存凭证到磁盘          │                         │
```

**状态机：**

| 状态 | 含义 | 处理 |
|------|------|------|
| `wait` | 等待扫码 | 静默轮询 |
| `scaned` | 已扫码，待确认 | 提示用户在微信确认 |
| `scaned_but_redirect` | 被重定向 | 切换到新 base_url |
| `expired` | 二维码过期 | 自动刷新（最多 3 次） |
| `confirmed` | 登录成功 | 提取 token/account_id，保存 |

- 超时：480 秒（8 分钟）
- 轮询间隔：1 秒

### 2.2 消息接收（Long Poll）

```
_poll_loop():
  │
  ├── POST /ilink/bot/getupdates
  │     Body: { get_updates_buf, base_info: { channel_version: "2.2.0" } }
  │     Headers: Authorization Bearer <token>,  iLink-App-Id: bot
  │     Timeout: 35 秒
  │
  ├── 解析响应:
  │     ├── ret/errcode != 0:
  │     │     ├── -14 → session 过期，暂停 600 秒
  │     │     ├── 连续失败 ≥ 3 → 30 秒退避
  │     │     └── 其他 → 2 秒后重试
  │     │
  │     ├── 更新 sync_buf 并持久化
  │     │
  │     └── 遍历 msgs[]:
  │           ├── 跳过自己发的消息
  │           ├── message_id + 内容 MD5 双重去重（5 分钟 TTL）
  │           ├── 判断聊天类型: room_id 存在 → group，否则 → dm
  │           ├── 策略检查: dm_policy / group_policy
  │           ├── 提取 context_token → 存入 ContextTokenStore
  │           ├── 预取 typing_ticket（异步）
  │           ├── 下载媒体（AES 解密）
  │           └── 文本批处理（3 秒去抖）→ handle_message()
  │
  └── 异常: CancelledError 退出，其他退避重试
```

### 2.3 消息发送

```
send(chat_id, content):
  │
  ├── 1. 取 context_token
  ├── 2. 提取媒体附件 (MEDIA: 标签 / 本地文件)
  ├── 3. 媒体文件先发，文本后发
  ├── 4. 文本拆分（≤2000 字符/条，按 Markdown 块边界）
  ├── 5. 每条间隔 1.5 秒
  │
  └── POST /ilink/bot/sendmessage
        Body: {
          msg: {
            from_user_id: "",
            to_user_id: <chat_id>,
            client_id: "hermes-weixin-<uuid>",
            message_type: 2,       // MSG_TYPE_BOT
            message_state: 2,      // MSG_STATE_FINISH
            context_token: "...",  // 必须！
            item_list: [{ type: 1, text_item: { text: "..." } }]
          },
          base_info: { channel_version: "2.2.0" }
        }
```

### 2.4 媒体发送

```
_send_file(chat_id, path):
  │
  ├── 1. 生成 AES key（随机 16 字节）
  ├── 2. AES-128-ECB 加密（PKCS7 填充）
  ├── 3. POST /ilink/bot/getuploadurl
  │     → 获取 upload_full_url 或 upload_param
  ├── 4. POST ciphertext 到 CDN
  │     → 响应头 x-encrypted-param
  ├── 5. POST /ilink/bot/sendmessage
  │     将 encrypt_query_param + aes_key 作为媒体 item 发送
  │
  └── ⚠️ aes_key 格式必须是 base64(hex_string)，不是 base64(raw_bytes)！
```

---

## 3. API 端点总览

| 端点 | 方法 | 用途 |
|------|------|------|
| `ilink/bot/get_bot_qrcode?bot_type=3` | GET | 获取登录二维码 |
| `ilink/bot/get_qrcode_status?qrcode=...` | GET | 轮询扫码状态 |
| `ilink/bot/getupdates` | POST | 长轮询接收消息 |
| `ilink/bot/sendmessage` | POST | 发送消息（文本/媒体） |
| `ilink/bot/sendtyping` | POST | "正在输入"状态 |
| `ilink/bot/getconfig` | POST | 获取 typing_ticket 等配置 |
| `ilink/bot/getuploadurl` | POST | 获取媒体上传 URL |

**基础 URL:**
- API: `https://ilinkai.weixin.qq.com`
- CDN: `https://novac2c.cdn.weixin.qq.com/c2c`

---

## 4. 请求参数详解

### 4.1 通用请求头

```
Content-Type: application/json
Authorization: Bearer <bot_token>
AuthorizationType: ilink_bot_token
Content-Length: <length>
X-WECHAT-UIN: <random_base64>
iLink-App-Id: bot
iLink-App-ClientVersion: 131584    # (2<<16)|(2<<8)|0
```

### 4.2 getupdates

**请求：**
```json
POST /ilink/bot/getupdates
{
  "get_updates_buf": "<sync_buf>",
  "base_info": { "channel_version": "2.2.0" }
}
```

**响应：**
```json
{
  "ret": 0,
  "errcode": 0,
  "get_updates_buf": "<new_buf>",
  "longpolling_timeout_ms": 35000,
  "msgs": [
    {
      "message_id": "...",
      "from_user_id": "wxid_xxx",
      "to_user_id": "...",
      "room_id": "...",
      "msg_type": 1,
      "context_token": "ctx_abc",
      "item_list": [{
        "type": 1,
        "text_item": { "text": "消息内容" }
      }]
    }
  ]
}
```

**错误码：**

| errcode | 含义 | 处理 |
|---------|------|------|
| 0 | 成功 | - |
| -2 | 频率限制 | 退避重试 |
| -14 | Session 过期 | 暂停 600 秒 |
| -2 + errmsg="unknown error" | 陈旧 session | 同 -14 |

### 4.3 sendmessage

**文本消息：**
```json
POST /ilink/bot/sendmessage
{
  "msg": {
    "from_user_id": "",
    "to_user_id": "<receiver>",
    "client_id": "hermes-weixin-<uuid>",
    "message_type": 2,
    "message_state": 2,
    "context_token": "<current_token>",
    "item_list": [{
      "type": 1,
      "text_item": { "text": "回复内容" }
    }]
  },
  "base_info": { "channel_version": "2.2.0" }
}
```

**图片消息：**
```json
POST /ilink/bot/sendmessage
{
  "msg": {
    "from_user_id": "",
    "to_user_id": "<receiver>",
    "client_id": "hermes-weixin-<uuid>",
    "message_type": 2,
    "message_state": 2,
    "context_token": "<current_token>",
    "item_list": [{
      "type": 2,
      "image_item": {
        "media": {
          "encrypt_query_param": "<x-encrypted-param>",
          "aes_key": "<base64(hex_string)>",
          "encrypt_type": 1
        },
        "mid_size": <ciphertext_size>
      }
    }]
  },
  "base_info": { "channel_version": "2.2.0" }
}
```

**消息项类型：**

| type | 常量 | 含义 |
|------|------|------|
| 1 | ITEM_TEXT | 文本 |
| 2 | ITEM_IMAGE | 图片 |
| 3 | ITEM_VOICE | 语音 |
| 4 | ITEM_FILE | 文件 |
| 5 | ITEM_VIDEO | 视频 |

### 4.4 sendtyping

```json
POST /ilink/bot/sendtyping
{
  "ilink_user_id": "<user_id>",
  "typing_ticket": "<ticket>",
  "status": 1,
  "base_info": { "channel_version": "2.2.0" }
}
```

| status | 含义 |
|--------|------|
| 1 | 开始输入 |
| 2 | 停止输入 |

typing_ticket 通过 `getconfig` 获取，TTL 600 秒，过期自动刷新。

### 4.5 getconfig

```json
POST /ilink/bot/getconfig
{
  "ilink_user_id": "<user_id>",
  "context_token": "<token>",
  "base_info": { "channel_version": "2.2.0" }
}
```
→ 返回 `{ typing_ticket: "..." }`

### 4.6 getuploadurl

```json
POST /ilink/bot/getuploadurl
{
  "filekey": "<random_hex_32>",
  "media_type": 1,
  "to_user_id": "<receiver>",
  "rawsize": <original_size>,
  "rawfilemd5": "<md5_hex>",
  "filesize": <padded_size>,
  "no_need_thumb": true,
  "aeskey": "<aes_key_hex>",
  "base_info": { "channel_version": "2.2.0" }
}
```

| media_type | 含义 |
|------------|------|
| 1 | 图片 (MEDIA_IMAGE) |
| 2 | 视频 (MEDIA_VIDEO) |
| 3 | 文件 (MEDIA_FILE) |
| 4 | 语音 (MEDIA_VOICE) |

→ 返回 `{ upload_full_url, upload_param }`，优先用 `upload_full_url`。

---

## 5. 媒体加密

**算法：** AES-128-ECB + PKCS7 填充

```
原始文件 → AES-128-ECB 加密 → POST ciphertext 到 CDN
                                   ↓
                          响应头: x-encrypted-param

发送消息:
  encrypt_query_param = x-encrypted-param
  aes_key_for_api = base64(hex(aes_key).encode())
                     ↑ 注意：是 base64 套 hex，不是 base64(raw_bytes)
```

**解密（入站媒体）：**
```
GET CDN/download?encrypted_query_param=<param>
  → 获取密文 → AES-128-ECB 解密 → 去 PKCS7 填充 → 原始文件
```

---

## 6. context_token 机制

```
用户发消息 → iLink 返回 context_token
                  ↓
       WeixinAdapter 存入 ContextTokenStore
       （内存 + 磁盘 ~/.hermes/weixin/accounts/<id>.context-tokens.json）
                  ↓
       Agent 回复时取出，附带在 sendmessage 的 msg.context_token 中
                  ↓
       服务端返回新 context_token → 更新 Store
```

**降级：** 如果 context_token 导致 errcode=-14，去掉 token 无 context 重试一次。

---

## 7. 配置

### 7.1 config.yaml

```yaml
gateway:
  platforms:
    weixin:
      enabled: true
      token: ""                # 或 WEIXIN_TOKEN
      extra:
        account_id: ""         # 或 WEIXIN_ACCOUNT_ID
        base_url: "https://ilinkai.weixin.qq.com"
        dm_policy: "pairing"   # disabled | pairing | allowlist | open
        group_policy: "disabled"  # disabled | all | allowlist
        allow_from: ""         # 逗号分隔白名单
```

### 7.2 环境变量

| 变量 | 用途 |
|------|------|
| `WEIXIN_TOKEN` | iLink Bot token |
| `WEIXIN_ACCOUNT_ID` | 账号 ID |
| `WEIXIN_BASE_URL` | API 基础 URL |
| `WEIXIN_DM_POLICY` | 私聊策略（默认 `pairing`） |
| `WEIXIN_GROUP_POLICY` | 群聊策略（默认 `disabled`） |
| `WEIXIN_ALLOWED_USERS` | 白名单 |

### 7.3 策略说明

| 策略 | 含义 |
|------|------|
| `disabled` | 完全禁用 |
| `pairing` | 需审批（`hermes pairing approve`） |
| `allowlist` | 白名单模式 |
| `all` / `open` | 全开放 |

---

## 8. 限流与熔断

- iLink 限流 `errcode=-2` → 退避重试（最多 4 次，间隔递增）
- 30 秒滑动窗口内达到阈值（默认 1 次）→ 熔断 30 秒
- 成功发送后自动重置熔断器

---

## 9. 限制

| 限制 | 说明 |
|------|------|
| 单条消息 | 最大 2000 字符 |
| 消息编辑 | 不支持，流式只能走最终结果 |
| 群聊 | iLink Bot（@im.bot）通常不能被拉入普通微信群 |
| 语音发送 | 不成熟，降级为文件附件 |
| 依赖 | `aiohttp` + `cryptography` + `certifi` |

---

## 10. 接入步骤

```bash
# 1. 装依赖
pip install aiohttp cryptography certifi

# 2. 扫码登录
cd ~/.hermes/hermes-agent
python3 -c "
from gateway.platforms.weixin import qr_login
from hermes_constants import get_hermes_home
import asyncio
asyncio.run(qr_login(str(get_hermes_home())))
"

# 3. 配置（如果没自动写入）
hermes config edit
# gateway.platforms.weixin:
#   enabled: true
#   extra:
#     account_id: "<扫码得到的id>"

# 4. 启动
hermes gateway install
hermes gateway start
```

---

## 11. 关键源码

| 文件 | 内容 |
|------|------|
| `gateway/platforms/weixin.py` (2379行) | WeixinAdapter 完整实现：QR登录、long-poll、消息收发、媒体加解密、限流熔断 |
| `gateway/run.py` L8830 | 平台适配器初始化 |
| `gateway/config.py` | Platform.WEIXIN + PlatformConfig |
| `tools/send_message_tool.py` L1775 | send_weixin_direct 独立发送入口 |
