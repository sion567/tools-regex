//! 桌面形态入口。UI 全部在 lib（与 wasm 浏览器形态共享，见 lib.rs 注释）。
//! wasm32 上 bin 只是占位（真正入口是 lib 的 #[wasm_bindgen(start)]，trunk 走 cdylib）。

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    app::run_native()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
