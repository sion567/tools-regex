#!/usr/bin/env bash
# 浏览器版构建（设计方案 9.4 探索项）：wasm32 + trunk
# 前置：rustup target add wasm32-unknown-unknown && cargo install trunk wasm-bindgen-cli
#       （wasm-bindgen-cli 版本须与 Cargo.lock 的 wasm-bindgen 一致，见 crates/app/Trunk.toml 注释）
# 用法：bash scripts/build_web.sh   → 产物 crates/app/dist/（index.html + wasm）
# 本地预览：cd crates/app && trunk serve
set -euo pipefail
cd "$(dirname "$0")/../crates/app"
echo "==> trunk build --release"
trunk build --release
echo "PASS  浏览器版构建完成：crates/app/dist/（本地预览：trunk serve）"
