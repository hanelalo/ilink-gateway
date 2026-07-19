#!/usr/bin/env bash
# wechat-claude — Claude Code adapter for WeChat gateway
#
# This is a shell wrapper that launches the adapter via Bun directly,
# without requiring Node.js. We use bun run instead of bun build --compile
# because --compile has a segfault bug on macOS (oven-sh/bun#26843).
set -euo pipefail

SCRIPT="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$0")"
ROOT="$(dirname "$(dirname "$SCRIPT")")"
exec /Users/hanelalo/.bun/bin/bun run "$ROOT/client/claude-code-adapter/src/index.ts" "$@"
