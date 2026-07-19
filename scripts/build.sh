#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> Building wechat-gateway (Rust)..."
cargo build --release "$@"

echo "==> Building wechat-claude (Bun)..."

cd "$ROOT/client/claude-code-adapter"

# Ensure dependencies are installed
if [ ! -d node_modules ]; then
  echo "    Installing npm dependencies..."
  npm install
fi

# Compile to standalone binary (NOT --target=bun — that causes a segfault on macOS)
bun build --compile \
  --outfile="$ROOT/target/release/wechat-claude" \
  src/index.ts

echo ""
echo "==> Build complete!"
echo "    wechat-gateway: $ROOT/target/release/wechat-gateway"
echo "    wechat-claude:  $ROOT/target/release/wechat-claude"
