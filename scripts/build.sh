#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> Building wechat-gateway (Rust)..."
cargo build --release "$@"

echo "==> Building wechat-claude (Bun)..."

# Use proxy if set for npm dependencies
if [ -n "${HTTPS_PROXY:-}" ] || [ -n "${http_proxy:-}" ]; then
  BUN_ENV="--bunfile=node_modules"
fi

cd "$ROOT/client/claude-code-adapter"

# Ensure dependencies are installed
if [ ! -d node_modules ]; then
  echo "    Installing npm dependencies..."
  npm install
fi

# Compile to standalone binary
bun build --compile --target=bun \
  --outfile="$ROOT/target/release/wechat-claude" \
  src/index.ts

echo ""
echo "==> Build complete!"
echo "    wechat-gateway: $ROOT/target/release/wechat-gateway"
echo "    wechat-claude:  $ROOT/target/release/wechat-claude"
