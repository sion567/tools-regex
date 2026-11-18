#!/usr/bin/env bash
# 发布构建（Windows）：产物 target/release/app.exe（单二进制，无 WebView/Node 依赖）
# 用法：bash scripts/build_release.sh
set -euo pipefail
cd "$(dirname "$0")/.."
echo "==> cargo build --release -p app"
cargo build --release -p app
echo "==> 测试（release profile 冒烟：核心 crates）"
cargo test --release -p regex-ir -p regex-parser -p regex-plain -p regex-diagram -p regex-export -p regex-exec --lib >/dev/null
echo "PASS  release 构建完成：target/release/app.exe"
