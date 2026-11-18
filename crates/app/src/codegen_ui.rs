//! 代码导出页（设计方案示例 12 的总集成）：一段正则 → 六方言即用代码。
//! 门禁可视化：目标方言不支持的特性直接拒绝 + 改写建议（专业卖点，见 7.5）。

use egui::{Color32, FontId};
use regex_ir::Dialect;
use regex_export::codegen::{export_code, TARGETS};

const FG: Color32 = Color32::from_rgb(216, 220, 226);
const GOOD: Color32 = Color32::from_rgb(134, 239, 172);
const BAD: Color32 = Color32::from_rgb(255, 128, 128);
const DIM: Color32 = Color32::from_rgb(139, 148, 158);
const CHIP: Color32 = Color32::from_rgb(97, 200, 255);

pub struct State {
    pattern: String,
    node: Option<regex_ir::Node>,
    parse_err: Option<String>,
    results: Vec<(Dialect, Result<String, String>)>,
}

impl Default for State {
    fn default() -> Self {
        let mut s = Self {
            pattern: r"(?P<k>\w+)=(?P<v>\w+)".to_string(),
            node: None,
            parse_err: None,
            results: Vec::new(),
        };
        s.recompute();
        s
    }
}

impl State {
    fn recompute(&mut self) {
        match regex_parser::parse(&self.pattern) {
            Ok(node) => {
                self.parse_err = None;
                self.node = Some(node.clone());
                self.results = TARGETS
                    .iter()
                    .map(|d| (*d, export_code(&node, &self.pattern, *d)))
                    .collect();
            }
            Err(e) => {
                self.parse_err = Some(e.to_string());
                self.node = None;
                self.results.clear();
            }
        }
    }
}

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    ui.heading("代码导出 = 方言翻译，不是复制");
    ui.weak("命名组写法逐方言改写（(?<n>) ↔ (?P<n>)）、Java 双重转义、Python raw string、Go raw/引号；目标方言不支持的特性 → 拒绝生成 + 改写建议。");
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("正则：");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut st.pattern)
                .desired_width(380.0)
                .font(FontId::monospace(14.0)),
        );
        if resp.changed() {
            st.recompute();
        }
        ui.separator();
        ui.label("快速示例：");
        for (name, pat) in [
            ("命名组", r"(?P<k>\w+)=(?P<v>\w+)"),
            ("日期", r"\b(\d{4})-(\d{2})-(\d{2})\b"),
            ("环视", r"\d+(?=\.)"),
            ("反向引用", r"\b(\w+) \1\b"),
        ] {
            if ui.small_button(name).clicked() {
                st.pattern = pat.to_string();
                st.recompute();
            }
        }
    });

    if let Some(e) = &st.parse_err {
        ui.label(egui::RichText::new(format!("✗ 解析失败：{e}")).color(BAD).strong());
        return;
    }

    ui.add_space(4.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (d, res) in &st.results {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(d.lang_label()).strong());
                match res {
                    Ok(code) => {
                        ui.label(egui::RichText::new("✓ 可导出").color(GOOD));
                        if ui.small_button("复制").clicked() {
                            ui.ctx().copy_text(code.clone());
                        }
                    }
                    Err(e) => {
                        ui.label(egui::RichText::new(format!("✗ 拒绝：{e}")).color(BAD));
                    }
                }
            });
            if let Ok(code) = res {
                // 代码块：等宽 + 浅底
                egui::Frame::default()
                    .fill(Color32::from_rgb(24, 26, 30))
                    .inner_margin(8.0)
                    .rounding(6.0)
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        ui.monospace(egui::RichText::new(code.clone()).color(FG).monospace());
                    });
            }
        }
        let _ = (DIM, CHIP); // 保留配色常量语义
    });
}
