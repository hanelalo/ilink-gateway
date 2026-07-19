"""
WeChat Gateway Platform Adapter for Hermes Agent.

Connects to the wechat-gateway (Rust) REST API as a platform adapter,
polling for WeChat messages and forwarding replies back through the gateway.

The adapter registers with the gateway's agent registry, polls for pending
messages, converts them to Hermes MessageEvent objects, and routes agent
replies back through the gateway's reply API.

Plugin path: ~/.hermes/plugins/wechat-gateway/
"""

import asyncio
import json
import logging
import os
import re
import time
from typing import Any, Dict, Optional

import aiohttp

from gateway.platforms.base import (
    BasePlatformAdapter,
    SendResult,
    MessageEvent,
    MessageType,
)
from gateway.config import Platform

logger = logging.getLogger(__name__)

# ─── Helpers ──────────────────────────────────────────────────────────────────

DEFAULT_GATEWAY_URL = "http://127.0.0.1:8765"
DEFAULT_AGENT_NAME = "hermes"
DEFAULT_POLL_INTERVAL = 1.0  # seconds

CONTENT_MAX_LENGTH = 2000
SEGMENT_DELAY = 1.5  # seconds between segments
_FENCE_RE = re.compile(r"^```")
_TABLE_ROW_RE = re.compile(r"^\s*\|")


def _str_env(name: str, default: str = "") -> str:
    return os.environ.get(name, "").strip() or default


def _float_env(name: str, default: float) -> float:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        return float(raw)
    except (TypeError, ValueError):
        return default


def check_requirements() -> bool:
    """Return True when aiohttp is available."""
    try:
        import aiohttp  # noqa: F401
        return True
    except ImportError:
        return False


def validate_config(config) -> bool:
    """Return True when the gateway URL is configured."""
    url = _str_env("WECHAT_GATEWAY_URL")
    if url:
        return True
    extra = getattr(config, "extra", {}) or {}
    return bool(extra.get("gateway_url"))


def is_connected(config) -> bool:
    """Return True when WECHAT_GATEWAY_URL is set (adapter can actually connect)."""
    url = _str_env("WECHAT_GATEWAY_URL")
    return bool(url) or bool(getattr(config, "extra", {}) or {}).get("gateway_url")


def _env_enablement() -> Optional[dict]:
    """Seed PlatformConfig.extra from env vars."""
    url = _str_env("WECHAT_GATEWAY_URL")
    if not url:
        return None
    seed = {"gateway_url": url}
    agent = _str_env("WECHAT_GATEWAY_AGENT_NAME")
    if agent:
        seed["agent_name"] = agent
    interval = _float_env("WECHAT_GATEWAY_POLL_INTERVAL", DEFAULT_POLL_INTERVAL)
    seed["poll_interval"] = interval
    return seed


# ─── Message splitting helpers ──────────────────────────────────────────────


def _split_markdown_blocks(text: str) -> list[str]:
    """Split text at blank lines and code fence boundaries.

    Code blocks are kept intact as single units.
    Consecutive table rows (lines starting with ``|``) are kept as single units.
    """
    if not text:
        return []
    blocks: list[str] = []
    lines = text.splitlines()
    current: list[str] = []
    in_code_block = False
    in_table = False

    def _flush() -> None:
        if current:
            blocks.append("\n".join(current).strip())
            current.clear()

    for raw_line in lines:
        line = raw_line.rstrip()

        # Code fence toggles
        if _FENCE_RE.match(line.strip()):
            if not in_code_block:
                _flush()
                in_table = False
            current.append(line)
            in_code_block = not in_code_block
            if not in_code_block:
                _flush()
            continue

        if in_code_block:
            current.append(line)
            continue

        # Blank line = block boundary
        if not line.strip():
            in_table = False
            _flush()
            continue

        # Table row: keep consecutive | lines together
        if _TABLE_ROW_RE.match(line.strip()):
            if not in_table:
                _flush()
                in_table = True
            current.append(line)
            continue

        # Exiting table
        if in_table:
            _flush()
            in_table = False

        # Regular text
        current.append(line)

    _flush()
    return [b for b in blocks if b]


def _truncate_block(block: str, max_len: int, seg_index: int, seg_total: int) -> list[str]:
    """Split a single oversized block at character boundary with [N/M] prefix."""
    prefix = f"[{seg_index}/{seg_total}] "
    usable = max_len - len(prefix)
    if usable <= 0:
        return [block[:max_len]]
    result: list[str] = []
    remaining = block
    while remaining:
        if len(remaining) <= usable:
            result.append(f"{prefix}{remaining}")
            break
        result.append(f"{prefix}{remaining[:usable]}")
        remaining = remaining[usable:]
        seg_total = max(seg_total, len(result))
        prefix = f"[{len(result)}/{seg_total}] "
        usable = max_len - len(prefix)
    return result


def _pack_blocks(blocks: list[str], max_len: int) -> list[str]:
    """Greedily pack blocks into segments under max_len.

    Single blocks exceeding max_len are truncated at character boundary
    with [N/M] prefix.
    """
    if not blocks:
        return []
    packed: list[str] = []
    current = ""
    oversized: list[str] = []

    for block in blocks:
        candidate = block if not current else f"{current}\n\n{block}"
        if len(candidate) <= max_len:
            current = candidate
            continue
        if current:
            packed.append(current)
            current = ""
        if len(block) <= max_len:
            current = block
            continue
        oversized.append(block)

    if current:
        packed.append(current)

    if oversized:
        seg_total = len(packed) + len(oversized)
        for block in oversized:
            packed.extend(_truncate_block(block, max_len, len(packed) + 1, seg_total))

    return packed


def _split_content(text: str, max_len: int) -> list[str]:
    """Split text into segments respecting markdown block boundaries."""
    if not text:
        return []
    if len(text) <= max_len:
        return [text]
    blocks = _split_markdown_blocks(text)
    return _pack_blocks(blocks, max_len)


# ─── Adapter ────────────────────────────────────────────────────────────────


class WeChatGatewayAdapter(BasePlatformAdapter):
    """Platform adapter that polls the wechat-gateway Rust API."""

    def __init__(self, config, **kwargs):
        platform = Platform("wechat_gateway")
        super().__init__(config=config, platform=platform)

        extra = getattr(config, "extra", {}) or {}

        self.gateway_url = (
            os.environ.get("WECHAT_GATEWAY_URL")
            or extra.get("gateway_url")
            or DEFAULT_GATEWAY_URL
        ).rstrip("/")
        self.agent_name = (
            os.environ.get("WECHAT_GATEWAY_AGENT_NAME")
            or extra.get("agent_name")
            or DEFAULT_AGENT_NAME
        )
        self.poll_interval = _float_env(
            "WECHAT_GATEWAY_POLL_INTERVAL",
            extra.get("poll_interval", DEFAULT_POLL_INTERVAL),
        )

        # Runtime state
        self._session: Optional[aiohttp.ClientSession] = None
        self._poll_task: Optional[asyncio.Task] = None
        self._running = False
        self._registered = False

    # ── Connection lifecycle ────────────────────────────────────────────

    async def connect(self, *, is_reconnect: bool = False) -> bool:
        """Register with the gateway and start the poll loop."""
        if self._running:
            logger.warning("Already connected")
            return True

        self._session = aiohttp.ClientSession()
        self._running = True

        # Register with the gateway
        if not await self._register():
            logger.error("Failed to register with gateway")
            self._running = False
            return False

        # Start the poll loop
        self._poll_task = asyncio.create_task(self._poll_loop())
        self._mark_connected()
        logger.info(
            "Connected to gateway %s as agent %s",
            self.gateway_url,
            self.agent_name,
        )
        return True

    async def disconnect(self) -> None:
        """Stop polling and disconnect from the gateway."""
        self._running = False
        self._mark_disconnected()
        if self._poll_task:
            self._poll_task.cancel()
            try:
                await self._poll_task
            except asyncio.CancelledError:
                pass
            self._poll_task = None
        if self._session:
            await self._session.close()
            self._session = None
        logger.info("Disconnected from gateway")

    # ── Send ────────────────────────────────────────────────────────────

    async def send(
        self,
        chat_id: str,
        content: str,
        reply_to: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> SendResult:
        """Send a reply back through the gateway API."""
        if reply_to:
            return await self._send_reply(reply_to, content)
        # Proactive send (pairing code, notifications): use chat_id as from_user
        return await self._send_proactive(chat_id, content)

    async def get_chat_info(self, chat_id: str) -> Dict[str, Any]:
        """Return basic chat info."""
        return {"name": chat_id, "type": "dm"}

    # ── Internal: Register ──────────────────────────────────────────────

    def _ensure_header(self, text: str) -> str:
        """Prepend **<agent_name>** header if not already present."""
        if text.startswith("**"):
            return text
        return f"**{self.agent_name}**\n---\n{text}"

    async def _register(self) -> bool:
        """POST /api/agents/register with this agent's name."""
        url = f"{self.gateway_url}/api/agents/register"
        body = {"name": self.agent_name, "capabilities": ["text"]}
        try:
            async with self._session.post(url, json=body) as resp:
                if resp.status != 200:
                    text = await resp.text()
                    logger.error("Register returned %s: %s", resp.status, text)
                    return False
                data = await resp.json()
                if not data.get("ok"):
                    logger.error("Register returned ok=false")
                    return False
                self._registered = True
                active = data.get("active_agent")
                if active:
                    logger.info("Active agent on gateway: %s", active)
                return True
        except (aiohttp.ClientError, asyncio.TimeoutError) as e:
            logger.error("Register HTTP error: %s", e)
            return False

    # ── Internal: Poll loop ─────────────────────────────────────────────

    async def _poll_loop(self) -> None:
        """Poll the gateway for pending messages in a loop."""
        while self._running:
            try:
                messages = await self._poll()
                for msg in messages:
                    await self._handle_message(msg)
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Poll loop error: %s", e)

            await asyncio.sleep(self.poll_interval)

    async def _poll(self) -> list[dict]:
        """GET /api/agents/{name}/poll, return list of message dicts."""
        url = f"{self.gateway_url}/api/agents/{self.agent_name}/poll"
        try:
            async with self._session.get(url) as resp:
                if resp.status == 200:
                    data = await resp.json()
                    return data.get("messages", [])
                elif resp.status == 404:
                    # Agent not registered — re-register
                    logger.warning("Agent not found on gateway (404), re-registering...")
                    self._registered = False
                    if await self._register():
                        logger.info("Re-registered with gateway after 404")
                    else:
                        # Don't spam: sleep extra before retry
                        await asyncio.sleep(5)
                    return []
                else:
                    text = await resp.text()
                    logger.warning("Poll returned %s: %s", resp.status, text)
                    return []
        except (aiohttp.ClientError, asyncio.TimeoutError) as e:
            logger.debug("Poll HTTP error: %s", e)
            return []

    # ── Internal: Handle inbound message ────────────────────────────────

    async def _handle_message(self, msg: dict) -> None:
        """Convert a gateway message to a MessageEvent and forward to Hermes."""
        msg_id = msg.get("id", "")
        from_user = msg.get("from_user", "")
        text = msg.get("text", "")
        msg_type_str = msg.get("message_type", "text")

        # Build the source info
        source = self.build_source(
            chat_id=from_user,
            chat_name=from_user,
            chat_type="dm",
            user_id=from_user,
            user_name=from_user,
            message_id=msg_id,
        )

        # Map type
        message_type = MessageType.TEXT
        if msg_type_str == "image":
            message_type = MessageType.PHOTO
        elif msg_type_str == "voice":
            message_type = MessageType.VOICE
        elif msg_type_str == "video":
            message_type = MessageType.VIDEO
        elif msg_type_str == "file":
            message_type = MessageType.DOCUMENT

        # Check for media files
        media_urls: list[str] = []
        media_types: list[str] = []
        for item in msg.get("media", []):
            media_path = item.get("local_path", "")
            if media_path:
                media_urls.append(media_path)
                media_types.append(item.get("media_type", "file"))

        event = MessageEvent(
            text=text,
            message_type=message_type,
            source=source,
            message_id=msg_id,
            media_urls=media_urls,
            media_types=media_types,
        )

        # Forward to Hermes — this triggers the full agent flow,
        # including all slash commands like /new
        logger.debug("Handling message %s from %s", msg_id, from_user)
        await self.handle_message(event)

    # ── Internal: Segment send helper ────────────────────────────────────────

    async def _send_segments(self, segments: list[str], send_fn) -> SendResult:
        """Send multiple segments sequentially with delay."""
        for i, segment in enumerate(segments):
            result = await send_fn(segment)
            if not result.success and i == 0:
                return result
            if i < len(segments) - 1:
                await asyncio.sleep(SEGMENT_DELAY)
        return SendResult(success=True, message_id="")

    # ── Internal: Proactive send ──────────────────────────────────────────

    async def _send_proactive(self, chat_id: str, text: str) -> SendResult:
        """POST /api/agents/{name}/reply with to_user for proactive sends.

        Used for pairing codes, notifications, etc. where there is no
        pre-existing message context.
        """
        text = self._ensure_header(text)
        segments = _split_content(text, CONTENT_MAX_LENGTH)

        async def _send_one(content: str) -> SendResult:
            url = f"{self.gateway_url}/api/agents/{self.agent_name}/reply"
            body = {
                "reply_to_id": "",
                "text": content,
                "media_paths": [],
                "to_user": chat_id,
                "context_token": "",
            }
            try:
                async with self._session.post(url, json=body) as resp:
                    if resp.status != 200:
                        err_text = await resp.text()
                        logger.error("Proactive send returned %s: %s", resp.status, err_text)
                        return SendResult(success=False, message_id="")
                    data = await resp.json()
                    success = data.get("ok", False)
                    return SendResult(success=success, message_id="")
            except (aiohttp.ClientError, asyncio.TimeoutError) as e:
                logger.error("Proactive send HTTP error: %s", e)
                return SendResult(success=False, message_id="")

        return await self._send_segments(segments, _send_one)

    # ── Internal: Send reply ────────────────────────────────────────────

    async def _send_reply(self, reply_to_id: str, text: str) -> SendResult:
        """POST /api/agents/{name}/reply with the response."""
        text = self._ensure_header(text)
        segments = _split_content(text, CONTENT_MAX_LENGTH)

        async def _send_one(content: str) -> SendResult:
            url = f"{self.gateway_url}/api/agents/{self.agent_name}/reply"
            body = {
                "reply_to_id": reply_to_id,
                "text": content,
                "media_paths": [],
                "agent_context": json.dumps({"agent": self.agent_name}),
            }
            try:
                async with self._session.post(url, json=body) as resp:
                    if resp.status != 200:
                        err_text = await resp.text()
                        logger.error("Reply returned %s: %s", resp.status, err_text)
                        return SendResult(success=False, message_id="")
                    data = await resp.json()
                    success = data.get("ok", False)
                    return SendResult(success=success, message_id=reply_to_id)
            except (aiohttp.ClientError, asyncio.TimeoutError) as e:
                logger.error("Reply HTTP error: %s", e)
                return SendResult(success=False, message_id="")

        return await self._send_segments(segments, _send_one)


# ─── Plugin entry point ───────────────────────────────────────────────────────


def register(ctx):
    """Plugin entry point — called by the Hermes plugin system."""
    ctx.register_platform(
        name="wechat_gateway",
        label="WeChat Gateway",
        adapter_factory=lambda cfg: WeChatGatewayAdapter(cfg),
        check_fn=check_requirements,
        validate_config=validate_config,
        is_connected=is_connected,
        required_env=["WECHAT_GATEWAY_URL"],
        install_hint="pip install aiohttp",
        env_enablement_fn=_env_enablement,
        allowed_users_env="WECHAT_GATEWAY_ALLOWED_USERS",
        allow_all_env="WECHAT_GATEWAY_ALLOW_ALL_USERS",
        max_message_length=2000,
        platform_hint=(
            "You are chatting via WeChat. "
            "You can respond with formatted text."
        ),
        emoji="💬",
    )
