//! tools-regex —— egui 壳（全仓唯一允许依赖 egui 的地方，工程纪律见 6.4 / scripts/lint_deps.sh）。
//!
//! 双形态共享同一套 UI（设计方案 9.4）：
//! - 桌面（src/main.rs → [`run_native`]）：完整能力，含大文件扫描
//! - 浏览器（wasm32，`#[wasm_bindgen(start)]`）：三段式工作台/闯关/导出可用；
//!   大文件扫描属桌面 edition（mmap/rayon 与 wasm 互斥），界面上显式说明

pub mod cheatsheet;
pub mod codegen_ui;
pub mod file_scan;
pub mod fonts;
pub mod lessons;
pub mod logging;
pub mod reverse;
mod spike_highlight;
mod spike_ime;
pub mod workbench;

use eframe::egui;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Workbench,
    Reverse,
    Cheatsheet,
    FileScan,
    Lessons,
    Export,
    Ime,
    Highlight,
}

impl Tab {
    fn title(&self) -> &'static str {
        match self {
            Tab::Workbench => "工作台",
            Tab::Reverse => "反推正则",
            Tab::Cheatsheet => "速查表",
            Tab::FileScan => "大文件扫描",
            Tab::Lessons => "闯关教学",
            Tab::Export => "代码导出",
            Tab::Ime => "IME 输入验证",
            Tab::Highlight => "高亮·10 万行性能",
        }
    }
}

#[derive(Default)]
struct App {
    tab: Option<Tab>,
    wb: workbench::State,
    rev: reverse::State,
    cs: cheatsheet::State,
    fs: file_scan::State,
    ls: lessons::State,
    ex: codegen_ui::State,
    ime: spike_ime::State,
    hl: spike_highlight::State,
}

impl eframe::App for App {
    // 0.37 起 App::update(ctx) 改为 App::ui(ui)：根 Ui 直接递进来，面板挂在它下面
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for tab in [
                    Tab::Workbench,
                    Tab::Reverse,
                    Tab::Cheatsheet,
                    Tab::FileScan,
                    Tab::Lessons,
                    Tab::Export,
                ] {
                    if ui.selectable_label(self.tab == Some(tab), tab.title()).clicked() {
                        logging::info(&format!("[tab] 切换 → {}", tab.title()));
                        self.tab = Some(tab);
                    }
                }
                // spike 是验证页（验收完退居二线）：折进下拉，点开才显示，不占标签栏
                ui.separator();
                ui.menu_button("spike 验证 ▾", |ui| {
                    for tab in [Tab::Ime, Tab::Highlight] {
                        if ui.button(tab.title()).clicked() {
                            logging::info(&format!("[tab] 切换 → {}", tab.title()));
                            self.tab = Some(tab);
                        }
                    }
                });
            });
            ui.add_space(2.0);
        });

        egui::CentralPanel::default_margins().show(ui, |ui| {
            match self.tab.unwrap_or(Tab::Workbench) {
                Tab::Workbench => workbench::show(ui, &mut self.wb),
                Tab::Reverse => {
                    // 反推正则 → 工作台交接：设正则 + 立即重解析重扫 + 切页
                    if let Some(pat) = reverse::show(ui, &mut self.rev) {
                        workbench::set_regex(&mut self.wb, pat);
                        logging::info("[tab] 反推 → 工作台（正则已交接）");
                        self.tab = Some(Tab::Workbench);
                    }
                }
                Tab::Cheatsheet => cheatsheet::show(ui, &mut self.cs),
                Tab::FileScan => file_scan::show(ui, &mut self.fs),
                Tab::Lessons => lessons::show(ui, &mut self.ls),
                Tab::Export => codegen_ui::show(ui, &mut self.ex),
                Tab::Ime => spike_ime::show(ui, &mut self.ime),
                Tab::Highlight => spike_highlight::show(ui, &mut self.hl),
            }
        });
    }
}

/// 桌面形态入口（src/main.rs 调用）——NativeOptions 仅存在于原生 target
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    // 尽早初始化：panic 钩子装上后，启动期崩溃也能落盘
    logging::init("desktop");
    // 本机实测（Intel 核显 + RDP）：wgpu 默认选 Vulkan 初始化直接段错误，DX12 正常。
    // Windows 上显式限定 DX12——代价是 WGPU_BACKEND 环境变量不再生效（backends 已写死）。
    //
    // surface 获取异常（Outdated/Lost/Timeout/Occluded…）全部落日志：隔夜/息屏/断连
    // 后 GPU 上下文出问题时，regex.log 里会留下痕迹（30 秒节流，防每帧刷屏）。
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_SURFACE_LOG_MS: AtomicU64 = AtomicU64::new(0);
    let on_surface_status: std::sync::Arc<
        dyn Fn(&egui_wgpu::wgpu::CurrentSurfaceTexture) -> egui_wgpu::SurfaceErrorAction
            + Send
            + Sync,
    > = std::sync::Arc::new(|status| {
        use egui_wgpu::SurfaceErrorAction;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_SURFACE_LOG_MS.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) > 30_000
            && LAST_SURFACE_LOG_MS
                .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            crate::logging::warn(&format!(
                "[wgpu] surface 获取异常：{status:?}（30 秒内只记一条）"
            ));
        }
        match status {
            egui_wgpu::wgpu::CurrentSurfaceTexture::Outdated => SurfaceErrorAction::Reconfigure,
            egui_wgpu::wgpu::CurrentSurfaceTexture::Lost => SurfaceErrorAction::RecreateSurface,
            _ => SurfaceErrorAction::SkipFrame,
        }
    });
    #[cfg(target_os = "windows")]
    let wgpu_options = egui_wgpu::WgpuConfiguration {
        on_surface_status,
        wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
            instance_descriptor: {
                let mut d = egui_wgpu::wgpu::InstanceDescriptor::new_without_display_handle();
                d.backends = egui_wgpu::wgpu::Backends::DX12;
                d
            },
            ..egui_wgpu::WgpuSetupCreateNew::without_display_handle()
        }),
        ..Default::default()
    };
    #[cfg(not(target_os = "windows"))]
    let wgpu_options = egui_wgpu::WgpuConfiguration {
        on_surface_status,
        ..Default::default()
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 860.0])
            .with_min_inner_size([1000.0, 620.0]),
        wgpu_options,
        ..Default::default()
    };
    let result = eframe::run_native(
        "tools-regex · Rust 正则可视化与调试工具",
        options,
        Box::new(|cc| {
            if let Err(e) = fonts::install_cjk(&cc.egui_ctx) {
                logging::warn(&format!("[fonts] CJK 字体加载失败：{e}（中文将显示为豆腐块）"));
            }
            Ok(Box::new(App::default()))
        }),
    );
    if let Err(e) = &result {
        logging::error(&format!("[eframe] 异常退出：{e}"));
    }
    result
}

/// 浏览器形态入口（设计方案 9.4 探索项；index.html + wasm-bindgen 构建）
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::prelude::JsValue> {
    use wasm_bindgen::JsCast;
    logging::init("wasm");
    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async move {
        // 0.37 起 start 收 HtmlCanvasElement，不再收 id 字符串
        let document = web_sys::window()
            .expect("No window")
            .document()
            .expect("No document");
        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("找不到 #the_canvas_id")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("canvas 元素类型错误");
        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    // 浏览器无 Windows 字体路径：中文降级为默认字体（探索版已知边界）
                    let _ = fonts::install_cjk(&cc.egui_ctx);
                    Ok(Box::new(App::default()))
                }),
            )
            .await
            .expect("eframe wasm 启动失败");
        logging::info("[wasm] RUNNER-START-OK");
    });
    Ok(())
}
