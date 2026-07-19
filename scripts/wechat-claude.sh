#!/usr/bin/env bash
# wechat-claude — Claude Code adapter for WeChat gateway
#
# This is a standalone binary compiled by Bun (scripts/build.sh). It does
# NOT require Node.js, npm, or any runtime dependencies.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

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

exec "$ROOT/target/release/wechat-claude" "$@"
