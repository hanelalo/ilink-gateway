#!/usr/bin/env bash
# wechat-claude — Claude Code adapter for WeChat gateway
#
# Log cleanup is handled inside the adapter. Uses bun run to ensure
# native binaries (Claude Code CLI) load correctly from the real
# node_modules — bun build --compile cannot bundle native binary deps.
set -euo pipefail

ROOT="/Users/hanelalo/develop/wechat-gateway"
exec /Users/hanelalo/.bun/bin/bun run "$ROOT/client/claude-code-adapter/src/index.ts" "$@"
