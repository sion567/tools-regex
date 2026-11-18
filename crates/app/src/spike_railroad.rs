//! Spike 3：铁路图 —— epaint 绘制 + 滚轮缩放 + 拖拽平移 + SVG 导出一致性。
//!
//! 链路是 M1 主链路的预演：parser → IR → diagram 布局 →（egui 画笔 | SVG）双后端。
//! 验收点：逐键重解析（含错误态红字）、缩放平移手感、导出的 SVG 与屏幕同构。

use std::sync::Arc;
use std::time::Instant;

use egui::{Color32, Pos2, Sense, Stroke, Vec2};
use regex_diagram::{Diagram, LineStyle, NodeStyle, Shape};
use regex_export::palette;

use crate::spike_highlight::colorize_regex;

const STROKE_C: Color32 = Color32::from_rgb(73, 80, 87);
const TEXT_C: Color32 = Color32::from_rgb(33, 37, 41);
const BG_C: Color32 = Color32::from_rgb(252, 252, 253);
const FILL_LITERAL: Color32 = Color32::from_rgb(255, 255, 255);
const FILL_CLASS: Color32 = Color32::from_rgb(231, 241, 255);
const FILL_GROUP: Color32 = Color32::from_rgb(233, 247, 239);
const FILL_DIAMOND: Color32 = Color32::from_rgb(255, 243, 205);
const BYPASS_C: Color32 = Color32::from_rgb(134, 142, 150);

const EXAMPLES: &[&str] = &[
    "a(b|c)?d",
    r"^\d{4}-\d{2}-\d{2}\b",
    r#"(".+?")"#,
    r"\b(\w+)\s+\1\b",
    "(a+)+b",
];

pub struct State {
    src: String,
    zoom: f32,
    offset: Vec2,
    last_us: u128,
    svg_msg: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            src: EXAMPLES[0].to_string(),
            zoom: 1.0,
            offset: Vec2::new(40.0, 50.0),
            last_us: 0,
            svg_msg: None,
        }
    }
}

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    ui.heading("Spike 3｜铁路图（epaint 绘制 + 缩放平移 + SVG 导出）");
    ui.label(
        egui::RichText::new("滚轮缩放（围绕光标）/ 拖拽平移；逐键重解析，非法正则红字报错并指列号。")
            .weak(),
    );

    // ---- 顶部：输入 + 示例 + 操作 ----
    let ctx = ui.ctx().clone();
    let mut layouter = move |_ui: &egui::Ui, s: &str, _wrap: f32| -> Arc<egui::Galley> {
        let job = colorize_regex(s);
        ctx.fonts(|f| f.layout_job(job))
    };
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut st.src)
                .hint_text("输入正则，实时出图")
                .desired_width(320.0)
                .layouter(&mut layouter),
        );
        ui.separator();
        for ex in EXAMPLES {
            if ui.small_button(*ex).clicked() {
                st.src = ex.to_string();
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button("重置视图").clicked() {
            st.zoom = 1.0;
            st.offset = Vec2::new(40.0, 50.0);
        }
        ui.weak(format!("解析+布局：{} µs（预算 < 30 000 µs）", st.last_us));
        if let Some(msg) = &st.svg_msg {
            ui.label(egui::RichText::new(msg).color(Color32::from_rgb(134, 239, 172)));
        }
    });

    // ---- 逐键重解析（M1 主链路预演） ----
    let t0 = Instant::now();
    let parsed = regex_parser::parse(&st.src);
    let (diagram, error) = match parsed {
        Ok(root) => {
            let d = regex_diagram::layout(&root);
            (Some(d), None)
        }
        Err(e) => (None, Some(e)),
    };
    st.last_us = t0.elapsed().as_micros();

    let export_enabled = diagram.is_some();
    ui.horizontal(|ui| {
        if let Some(d) = &diagram {
            ui.weak(format!("图：{:.0}×{:.0}，{} 个图元", d.width, d.height, d.shapes.len()));
        }
        let btn = egui::Button::new("导出 SVG");
        if ui.add_enabled(export_enabled, btn).clicked() {
            if let Some(d) = &diagram {
                st.svg_msg = export_svg(d);
            }
        }
    });
    if let Some(e) = &error {
        ui.label(
            egui::RichText::new(format!("✗ {e}"))
                .color(Color32::from_rgb(255, 128, 128))
                .strong(),
        );
    }

    ui.add_space(4.0);

    // ---- 画布：缩放 / 平移 ----
    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    painter.rect_filled(resp.rect, 2.0, BG_C);

    if resp.dragged() {
        st.offset += resp.drag_delta();
    }
    if resp.hovered() {
            let sd = ui.input(|i| i.smooth_scroll_delta.y);
        if sd.abs() > 0.5 {
            let old = st.zoom;
            let new = (old * (1.0 + sd * 0.0015)).clamp(0.2, 8.0);
            if let Some(m) = resp.hover_pos() {
                // 围绕光标缩放：offset' = m - (m - offset) * (new/old)
                let m = m.to_vec2();
                st.offset = m - (m - st.offset) * (new / old);
            }
            st.zoom = new;
        }
    }

    let Some(d) = &diagram else { return };
    let origin = resp.rect.min;
    let t = |p: regex_diagram::Pt| -> Pos2 {
        Pos2::new(origin.x + st.offset.x + p.x * st.zoom, origin.y + st.offset.y + p.y * st.zoom)
    };
    let stroke_w = (1.6 * st.zoom).clamp(0.6, 4.0);

    for shape in &d.shapes {
        match shape {
            Shape::Polyline { pts, style } => {
                let (color, w) = match style {
                    LineStyle::Main => (STROKE_C, stroke_w),
                    // egui 原生不支持虚线：旁路/回环用浅细线区分，M1 换分段虚线 tessellation
                    LineStyle::Bypass => (BYPASS_C, (0.9 * st.zoom).clamp(0.4, 2.0)),
                    LineStyle::Loop => (BYPASS_C, (0.7 * st.zoom).clamp(0.35, 2.0)),
                };
                let points: Vec<Pos2> = pts.iter().map(|p| t(*p)).collect();
                if points.len() >= 2 {
                    painter.add(egui::Shape::line(points, Stroke::new(w, color)));
                }
            }
            Shape::Box { pos, size, label, style, .. } => {
                let min = t(*pos);
                let max = t(regex_diagram::Pt::new(pos.x + size.x, pos.y + size.y));
                let fill = match style {
                    NodeStyle::Literal => FILL_LITERAL,
                    NodeStyle::Class => FILL_CLASS,
                    NodeStyle::Group => FILL_GROUP,
                };
                let rect = egui::Rect::from_min_max(min, max);
                painter.rect(
                    rect,
                    egui::Rounding::same((7.0 * st.zoom).clamp(1.0, 10.0)),
                    fill,
                    Stroke::new(stroke_w, STROKE_C),
                );
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional((14.0 * st.zoom).clamp(6.0, 40.0)),
                    TEXT_C,
                );
            }
            Shape::Diamond { pos, size, label, .. } => {
                let (cx, cy) = (pos.x + size.x / 2.0, pos.y + size.y / 2.0);
                let c = t(regex_diagram::Pt::new(cx, cy));
                let h = regex_diagram::Pt::new(cx - size.x / 2.0, cy);
                let r = regex_diagram::Pt::new(cx + size.x / 2.0, cy);
                let top = regex_diagram::Pt::new(cx, cy - size.y / 2.0);
                let bot = regex_diagram::Pt::new(cx, cy + size.y / 2.0);
                let pts = vec![t(top), t(r), t(bot), t(h)];
                painter.add(egui::Shape::convex_polygon(pts, FILL_DIAMOND, Stroke::new(stroke_w, STROKE_C)));
                painter.text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional((13.0 * st.zoom).clamp(6.0, 40.0)),
                    TEXT_C,
                );
            }
            Shape::Arrow { tip } => {
                let p = t(*tip);
                let w = (7.0 * st.zoom).clamp(3.0, 16.0);
                let h = (3.5 * st.zoom).clamp(1.5, 8.0);
                painter.add(egui::Shape::convex_polygon(
                    vec![p, Pos2::new(p.x - w, p.y - h), Pos2::new(p.x - w, p.y + h)],
                    STROKE_C,
                    Stroke::NONE,
                ));
            }
            Shape::Text { pos, label, size } => {
                let p = t(*pos);
                painter.text(
                    Pos2::new(p.x, p.y + size * st.zoom),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional((*size * st.zoom).clamp(6.0, 24.0)),
                    egui::Color32::from_rgb(108, 117, 125),
                );
            }
        }
    }
    let _ = palette::STROKE; // 与 SVG 端共享调色板（数值对齐由两边常量保证）
}

fn export_svg(d: &Diagram) -> Option<String> {
    let svg = regex_export::diagram_to_svg(d);
    let path = std::env::current_dir().ok()?.join("m0-railroad.svg");
    std::fs::write(&path, &svg).ok()?;
    Some(format!("已导出 {}（{} 字节）", path.display(), svg.len()))
}
