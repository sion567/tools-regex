//! tools-regex —— egui 壳（全仓唯一允许依赖 egui 的地方，工程纪律见 6.4 / scripts/lint_deps.sh）。
//!
//! 双形态共享同一套 UI（设计方案 9.4）：
//! - 桌面（src/main.rs → [`run_native`]）：完整能力，含大文件扫描
//! - 浏览器（wasm32，`#[wasm_bindgen(start)]`）：三段式工作台/闯关/导出可用；
//!   大文件扫描属桌面 edition（mmap/rayon 与 wasm 互斥），界面上显式说明

pub mod codegen_ui;
pub mod file_scan;
pub mod fonts;
pub mod lessons;
mod spike_highlight;
mod spike_ime;
mod spike_railroad;
pub mod workbench;

use eframe::egui;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Workbench,
    FileScan,
    Lessons,
    Export,
    Ime,
    Highlight,
    Railroad,
}

impl Tab {
    fn title(&self) -> &'static str {
        match self {
            Tab::Workbench => "工作台 (M2)",
            Tab::FileScan => "大文件扫描 (M3·桌面)",
            Tab::Lessons => "闯关教学 (M4)",
            Tab::Export => "代码导出 (M4)",
            Tab::Ime => "spike① IME",
            Tab::Highlight => "spike② 高亮",
            Tab::Railroad => "spike③ 铁路图",
        }
    }
}

#[derive(Default)]
struct App {
    tab: Option<Tab>,
    wb: workbench::State,
    fs: file_scan::State,
    ls: lessons::State,
    ex: codegen_ui::State,
    ime: spike_ime::State,
    hl: spike_highlight::State,
    rr: spike_railroad::State,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // wasm 探针：渲染循环是否真的在跑（RUNNER-START-OK 之后的下一站）
        #[cfg(target_arch = "wasm32")]
        {
            use std::sync::atomic::{AtomicU32, Ordering};
            static FRAMES: AtomicU32 = AtomicU32::new(0);
            let n = FRAMES.fetch_add(1, Ordering::Relaxed);
            if n == 0 || n == 30 {
                web_sys::console::log_1(&format!("UPDATE-FRAME {n}").into());
            }
        }
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("tools-regex").strong());
                ui.separator();
                for tab in [
                    Tab::Workbench,
                    Tab::FileScan,
                    Tab::Lessons,
                    Tab::Export,
                    Tab::Ime,
                    Tab::Highlight,
                    Tab::Railroad,
                ] {
                    if ui.selectable_label(self.tab == Some(tab), tab.title()).clicked() {
                        self.tab = Some(tab);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak("设计方案 docs/ · 验收清单 README");
                });
            });
            ui.add_space(2.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.tab.unwrap_or(Tab::Workbench) {
                Tab::Workbench => workbench::show(ui, &mut self.wb),
                Tab::FileScan => file_scan::show(ui, &mut self.fs),
                Tab::Lessons => lessons::show(ui, &mut self.ls),
                Tab::Export => codegen_ui::show(ui, &mut self.ex),
                Tab::Ime => spike_ime::show(ui, &mut self.ime),
                Tab::Highlight => spike_highlight::show(ui, &mut self.hl),
                Tab::Railroad => spike_railroad::show(ui, &mut self.rr),
            }
        });
    }
}

/// 桌面形态入口（src/main.rs 调用）——NativeOptions 仅存在于原生 target
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 860.0])
            .with_min_inner_size([1000.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "tools-regex · Rust 正则可视化与调试工具",
        options,
        Box::new(|cc| {
            if let Err(e) = fonts::install_cjk(&cc.egui_ctx) {
                eprintln!("[fonts] CJK 字体加载失败：{e}（中文将显示为豆腐块）");
            }
            Box::new(App::default())
        }),
    )
}

/// 浏览器形态入口（设计方案 9.4 探索项；index.html + wasm-bindgen 构建）
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::prelude::JsValue> {
    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async move {
        eframe::WebRunner::new()
            .start(
                "the_canvas_id",
                web_options,
                Box::new(|cc| {
                    // 浏览器无 Windows 字体路径：中文降级为默认字体（探索版已知边界）
                    let _ = fonts::install_cjk(&cc.egui_ctx);
                    Box::new(App::default())
                }),
            )
            .await
            .expect("eframe wasm 启动失败");
        web_sys::console::log_1(&"RUNNER-START-OK".into());
    });
    Ok(())
}
