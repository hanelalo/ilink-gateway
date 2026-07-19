#!/usr/bin/env bash
# wechat-claude — Claude Code adapter for WeChat gateway
#
# This is a shell wrapper that launches the adapter via Bun directly,
# without requiring Node.js. We use bun run instead of bun build --compile
# because --compile has a segfault bug on macOS (oven-sh/bun#26843).
set -euo pipefail

SCRIPT="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$0")"
ROOT="$(dirname "$(dirname "$SCRIPT")")"

# Remove log files from previous calendar days (cross-platform, no cron needed)
LOG_DIR="$HOME/.wechat-gateway"
TODAY=$(date +%Y-%m-%d)
for logfile in "$LOG_DIR"/*.log; do
  [ -f "$logfile" ] || continue
  FILE_DAY=$(date -r "$logfile" +%Y-%m-%d 2>/dev/null)
  if [ -n "$FILE_DAY" ] && [ "$FILE_DAY" \< "$TODAY" ]; then
    rm -f "$logfile"
  fi
done

exec /Users/hanelalo/.bun/bin/bun run "$ROOT/client/claude-code-adapter/src/index.ts" "$@"
