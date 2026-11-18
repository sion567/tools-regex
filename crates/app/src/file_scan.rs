//! 大文件流式扫描页（设计方案第五章 / M3）：mmap + rayon 并行，剖析报告 + 采样预览。
//!
//! 安全拦截（5.3）：扫描前先跑灾难性回溯静态检测——有风险的大文件直接阻断，
//! 防止把 UI 卡死；环视/反向引用模式由 stream 模块在编译期拒绝（DFA 语法限定）。

// mpsc：桌面版扫描通道；wasm 上无桌面路径，import 由 allow 兜住
#[allow(unused_imports)]
use std::sync::mpsc::{channel, Receiver};

#[cfg(not(target_arch = "wasm32"))]
pub struct State {
    pattern: String,
    path: String,
    running: bool,
    rx: Receiver<Result<regex_exec::stream::StreamReport, String>>,
    report: Option<Result<regex_exec::stream::StreamReport, String>>,
    block_msg: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for State {
    fn default() -> Self {
        Self {
            pattern: r"\bERROR\b.*?code=(\d{5})".to_string(),
            path: String::new(),
            running: false,
            rx: channel().1,
            report: None,
            block_msg: None,
        }
    }
}

/// 浏览器版：mmap/rayon 属桌面 edition——占位状态（显式说明，不提供扫描）
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub struct State {}

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    // 浏览器形态（wasm）：mmap/rayon/文件系统属桌面 edition，显式说明（设计方案 9.4 双 edition）
    #[cfg(target_arch = "wasm32")]
    {
        let _ = st;
        ui.heading("大文件流式扫描");
        ui.label(
            egui::RichText::new("此功能属于桌面版（mmap + rayon 并行依赖本地文件系统，与 wasm 互斥）。")
                .color(egui::Color32::from_rgb(255, 193, 7)),
        );
        ui.weak("设计方案 9.4：桌面与浏览器为两个 edition，共享同一套核心 crates。");
        return;
    }
    #[allow(unused_variables)]
    #[cfg(not(target_arch = "wasm32"))]
    native_show(ui, st)
}

#[cfg(not(target_arch = "wasm32"))]
fn native_show(ui: &mut egui::Ui, st: &mut State) {
    use egui::{Color32, FontId};
    const FG: Color32 = Color32::from_rgb(216, 220, 226);
    const GOOD: Color32 = Color32::from_rgb(134, 239, 172);
    const WARN: Color32 = Color32::from_rgb(255, 193, 7);
    const BAD: Color32 = Color32::from_rgb(255, 128, 128);
    const DIM: Color32 = Color32::from_rgb(139, 148, 158);

    let ctx = ui.ctx().clone();
    ui.heading("大文件流式扫描（mmap + rayon 并行）");
    ui.weak("GB 级文件不读进内存；行边界对齐分块并行；只支持 regex 引擎语法（线性时间，永远不炸）。");
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("正则：");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut st.pattern)
                .desired_width(340.0)
                .font(FontId::monospace(13.5)),
        );
        if resp.changed() {
            st.report = None;
            st.block_msg = None;
        }
        // 静态风险提示（仅提示；大文件执行前的硬拦截在扫描按钮里）
        let risks = regex_exec::danger::check(
            &regex_parser::parse_partial(&st.pattern).0,
        );
        if !risks.is_empty() {
            ui.label(
                egui::RichText::new("⚠ 嵌套量词风险——超过 10MB 的文件将被阻断")
                    .color(BAD)
                    .strong(),
            );
        }
    });

    ui.horizontal(|ui| {
        ui.label("文件：");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut st.path)
                .desired_width(520.0)
                .font(FontId::monospace(13.5))
                .hint_text("输入日志文件路径，如 E:/logs/app.log"),
        );
        if resp.changed() {
            st.report = None;
            st.block_msg = None;
        }
        if st.running {
            ui.label(egui::RichText::new("扫描中…").color(WARN).strong());
        } else if ui.add_enabled(!st.path.is_empty(), egui::Button::new("▶ 扫描")).clicked() {
            st.block_msg = None;
            st.report = None;
            match precheck(&st.pattern, &st.path) {
                Err(msg) => st.block_msg = Some(msg), // 安全拦截：不打进后台
                Ok(_) => {
                    let (tx, rx) = channel();
                    st.rx = rx;
                    st.running = true;
                    let (pattern, path) = (st.pattern.clone(), st.path.clone());
                    std::thread::spawn(move || {
                        let r = regex_exec::stream::scan_file(&path, &pattern);
                        let _ = tx.send(r);
                    });
                }
            }
        }
    });
    if let Some(msg) = &st.block_msg {
        ui.label(egui::RichText::new(format!("⛔ 已拦截：{msg}")).color(BAD).strong());
    }

    // 收后台结果
    while let Ok(r) = st.rx.try_recv() {
        st.report = Some(r);
        st.running = false;
    }
    if st.running {
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }

    // ---- 报告 ----
    if let Some(res) = &st.report {
        match res {
            Err(e) => {
                ui.label(egui::RichText::new(format!("扫描失败：{e}")).color(BAD));
            }
            Ok(r) => {
                ui.add_space(6.0);
                ui.separator();
                ui.label(egui::RichText::new("性能剖析").strong());
                egui::Grid::new("scan_report").striped(true).show(ui, |ui| {
                    ui.label("文件"); ui.monospace(&r.path); ui.end_row();
                    ui.label("大小"); ui.monospace(fmt_bytes(r.bytes)); ui.end_row();
                    ui.label("耗时"); ui.monospace(format!("{:.0} ms", r.elapsed_ms)); ui.end_row();
                    ui.label("吞吐"); ui.monospace(format!("{:.0} MB/s", r.throughput_mbs)); ui.end_row();
                    ui.label("并行块"); ui.monospace(format!("{} 块（行边界对齐）", r.chunks)); ui.end_row();
                    ui.label("内存"); ui.monospace("映射按需分页（mmap），常驻由 OS 管理——不整读进内存"); ui.end_row();
                    ui.label("回溯触发"); ui.monospace(egui::RichText::new("0 次（DFA 安全引擎，线性时间）").color(GOOD)); ui.end_row();
                    ui.label("命中"); ui.monospace(format!("{} 行 / {} 处", r.matched_lines, r.total_matches)); ui.end_row();
                });
                ui.add_space(6.0);
                ui.label(egui::RichText::new(format!(
                    "采样预览（前 {} 条，设计方案 5.3 上下文采样——不刷爆界面）：",
                    r.preview.len()
                )).strong());
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for h in &r.preview {
                            ui.horizontal(|ui| {
                                ui.monospace(egui::RichText::new(format!("L{:>6}", h.line_no)).color(DIM));
                                ui.monospace(egui::RichText::new(&h.preview).color(FG));
                                if h.match_count > 1 {
                                    ui.label(egui::RichText::new(format!("×{}", h.match_count)).color(WARN));
                                }
                            });
                        }
                        if r.preview.is_empty() {
                            ui.weak("（无命中）");
                        }
                    });
            }
        }
    }
}

/// 扫描前拦截（设计方案 5.3 安全拦截机制）：
/// 1) 嵌套量词风险 + 文件 >10MB → 阻断
/// 2) 文件不存在/读不了 → 阻断
#[cfg(not(target_arch = "wasm32"))]
fn precheck(pattern: &str, path: &str) -> Result<(), String> {
    let risks = regex_exec::danger::check(&regex_parser::parse_partial(pattern).0);
    if !risks.is_empty() {
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > 10 * 1024 * 1024 {
                return Err(format!(
                    "正则含灾难性回溯风险（嵌套量词），目标文件 {:.1} MB > 10 MB——已阻断执行。请改写正则（占有量词/原子组/拆分）",
                    meta.len() as f64 / 1e6
                ));
            }
        }
    }
    std::fs::metadata(path).map_err(|e| format!("文件不可访问：{e}"))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn fmt_bytes(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.2} GB", n as f64 / (1 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} KB", n as f64 / (1 << 10) as f64)
    } else {
        format!("{n} B")
    }
}
