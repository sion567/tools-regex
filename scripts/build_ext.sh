#!/usr/bin/env bash
# VSCode 插件的 wasm 重建（crates/wasm-api → vscode-ext/media/）
# 前置：rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli@<Cargo.lock 版本>
set -euo pipefail
cd "$(dirname "$0")/.."
echo "==> cargo build wasm32 (wasm-api)"
cargo build --release --target wasm32-unknown-unknown -p wasm-api
echo "==> wasm-bindgen"
wasm-bindgen target/wasm32-unknown-unknown/release/wasm_api.wasm \
  --out-dir vscode-ext/media --target web
echo "PASS  wasm 已更新：vscode-ext/media/（再到 vscode-ext 目录 npm run compile + vsce package 打包）"
