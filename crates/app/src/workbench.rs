//! M2 工作台 —— 三段式渐进面板（设计方案 3.3）：人话层（默认）→ 结构层（铁路图）→ 引擎层（M3）。
//!
//! 悬停双向联动（设计 7.5/9 的落地体验）：
//! - 悬停铁路图节点 → 联动行高亮对应正则片段
//! - 悬停人话分段 → 同上
//!
//! 联动的地基：铁路图 Shape 携带源码 span（regex-diagram）+ 人话分段携带 span（regex-plain）。
//!
//! M1 行为保持：边输入边宽松解析（画到能画的部分）、后台线程扫描（防抖+代次守卫）、
//! 解析+布局 <30ms 预算灯、引擎矩阵（regex 真身 / fancy-regex 近似）。

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use egui::{Color32, FontId, Pos2, Sense, Stroke, TextFormat, Vec2};
use regex_diagram::{Diagram, LineStyle, NodeStyle, Shape};
use regex_ir::Span;

use crate::spike_highlight::colorize_regex;

// ---- 配色（与 SVG 端 palette 语义一致） ----
const STROKE_C: Color32 = Color32::from_rgb(73, 80, 87);
const TEXT_C: Color32 = Color32::from_rgb(33, 37, 41);
const CANVAS_BG: Color32 = Color32::from_rgb(252, 252, 253);
const FILL_LITERAL: Color32 = Color32::from_rgb(255, 255, 255);
const FILL_CLASS: Color32 = Color32::from_rgb(231, 241, 255);
const FILL_GROUP: Color32 = Color32::from_rgb(233, 247, 239);
const FILL_DIAMOND: Color32 = Color32::from_rgb(255, 243, 205);
const FILL_ACTIVE: Color32 = Color32::from_rgb(255, 224, 138);
const BYPASS_C: Color32 = Color32::from_rgb(134, 142, 158);
const FG: Color32 = Color32::from_rgb(216, 220, 226);
const GOOD: Color32 = Color32::from_rgb(134, 239, 172);
const WARN: Color32 = Color32::from_rgb(255, 193, 7);
const BAD: Color32 = Color32::from_rgb(255, 128, 128);
const DIM: Color32 = Color32::from_rgb(139, 148, 158);
const CHIP: Color32 = Color32::from_rgb(97, 200, 255);

const EXAMPLES: &[(&str, &str)] = &[
    ("示例1 日期", r"\b(\d{4})-(\d{2})-(\d{2})\b"),
    ("贪婪", r#""(.+)""#),
    ("懒惰", r#""(.+?)""#),
    ("示例4 重复词", r"\b(\w+) \1\b"),
    ("前瞻", r"\d+(?=\.)"),
    ("命名组", r"(?<k>\w+)=(?<v>.+)"),
    ("ReDoS 危险", "(a+)+b"),
];

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    /// ① 人话层（默认打开，新手入口）
    Plain,
    /// ② 结构层：铁路图
    Railroad,
    /// ③ 引擎层：M3 回溯时间轴（教学解释器 + 步迹回放）
    Engine,
}

/// 后台扫描线程回传的结果（代次守卫：gen 不匹配即丢弃）
struct ScanMsg {
    gen: u64,
    engine: Option<(&'static str, bool)>, // (label, approximate)
    error: Option<String>,
    per_line: Vec<Vec<(usize, usize)>>,
    total: usize,
    ms: f32,
}

pub struct State {
    regex_src: String,
    sample: String,

    view: View,
    /// 当前悬停的正则片段（联动核心状态）
    hovered: Option<Span>,

    // 人话层
    plain: Vec<regex_plain::Segment>,
    plain_summary: String,

    // 解析 + 布局（UI 线程，正则变更时每键重算——预算内）
    diagram: Option<Diagram>,
    parse_err: Option<regex_parser::ParseError>,
    parse_us: u128,
    layout_us: u128,
    /// 最近一次解析的 IR（引擎层教学解释器/风险检测输入）
    last_node: Option<regex_ir::Node>,
    /// 灾难性回溯静态检测结果（M3 danger）
    danger: Vec<regex_exec::danger::Risk>,

    // 引擎层（M3）：教学解释器 + 回溯时间轴
    test_text: String,
    budget_idx: usize,
    interp: Option<Result<regex_exec::interp::InterpResult, String>>,
    scrub: usize,

    // 后台扫描
    gen: u64,
    rx: Receiver<ScanMsg>,
    result: Option<ScanMsg>,
    pending_scan: bool,
    scan_due: bool,
    last_edit: f64,

    // 画布视图
    zoom: f32,
    offset: Vec2,

    // 高亮视图 galley 缓存（代次守卫）
    galleys: HashMap<usize, (u64, std::sync::Arc<egui::Galley>)>,
    svg_msg: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            regex_src: r"\b(\d{4})-(\d{2})-(\d{2})\b".to_string(),
            sample: "Contract signed on 2024-08-15, amended 2025-01-01 (see 2025/02/03 doc).\n\
                     He said \"hello\", then \"bye\".\n\
                     the the cat cat dog\n\
                     2026-11-17 worker-3 处理订单 order=ORD-2026-000042 user=张伟 ip=10.20.30.40 123ms\n\
                     price 1234. at end."
                .to_string(),
            view: View::Plain,
            hovered: None,
            plain: Vec::new(),
            plain_summary: String::new(),
            diagram: None,
            parse_err: None,
            parse_us: 0,
            layout_us: 0,
            last_node: None,
            danger: Vec::new(),
            test_text: format!("{}!", "a".repeat(32)),
            budget_idx: 1,
            interp: None,
            scrub: 0,
            gen: 0,
            rx: channel().1,
            result: None,
            pending_scan: false,
            scan_due: true,
            last_edit: f64::NEG_INFINITY,
            zoom: 1.0,
            offset: Vec2::new(36.0, 42.0),
            galleys: HashMap::new(),
            svg_msg: None,
        }
    }
}

const DEBOUNCE: f64 = 0.3;

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    let ctx = ui.ctx().clone();
    let now = ui.input(|i| i.time);

    let mut regex_edited = false;
    egui::TopBottomPanel::top("wb_top").show_inside(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("正则：");
            let ctx2 = ui.ctx().clone();
            let mut layouter = move |_ui: &egui::Ui, s: &str, _w: f32| -> std::sync::Arc<egui::Galley> {
                let job = colorize_regex(s);
                ctx2.fonts(|f| f.layout_job(job))
            };
            let resp = ui.add(
                egui::TextEdit::singleline(&mut st.regex_src)
                    .desired_width(380.0)
                    .font(FontId::monospace(14.0))
                    .layouter(&mut layouter),
            );
            if resp.changed() {
                regex_edited = true;
                // 作废在途扫描：旧正则的扫描结果回来时 gen 已不匹配（防内容错位）
                st.gen += 1;
            }
            ui.separator();
            ui.label("示例：");
            for (name, pat) in EXAMPLES {
                if ui.small_button(*name).clicked() {
                    st.regex_src = (*pat).to_string();
                    regex_edited = true;
                }
            }
        });

        // 解析错误（宽松：红字 + 列号，同时铁路图画前缀）
        if let Some(e) = &st.parse_err {
            let msg = egui::RichText::new(format!("✗ {e}")).color(BAD).strong();
            let resp = ui.label(msg);
            if let Some(h) = &e.hint {
                resp.on_hover_text(h.clone());
            }
        }

        // 联动行：悬停目标（铁路图节点/人话分段）对应的高亮正则片段
        ui.horizontal(|ui| {
            ui.label("联动：");
            let job = linkage_job(&st.regex_src, st.hovered);
            let gal = ui.ctx().fonts(|f| f.layout_job(job));
            ui.add(egui::Label::new(egui::WidgetText::Galley(gal)));
            if st.hovered.is_none() {
                ui.weak("（悬停铁路图节点或人话分段，这里高亮对应片段）");
            }
        });

        // 三段式视图切换（设计方案 3.3：默认落人话层）
        ui.horizontal(|ui| {
            ui.label("视图：");
            for (v, label) in [
                (View::Plain, "① 人话层"),
                (View::Railroad, "② 结构层 · 铁路图"),
                (View::Engine, "③ 引擎层 (M3)"),
            ] {
                if ui.selectable_label(st.view == v, label).clicked() {
                    st.view = v;
                }
            }
            ui.separator();
            if ui
                .add_enabled(st.diagram.is_some(), egui::Button::new("导出 SVG"))
                .clicked()
            {
                if let Some(d) = &st.diagram {
                    st.svg_msg = export_svg(d);
                }
            }
            if let Some(m) = &st.svg_msg {
                ui.label(egui::RichText::new(m).color(GOOD));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let total_us = st.parse_us + st.layout_us;
                let budget_ok = total_us < 30_000;
                let t = egui::RichText::new(format!(
                    "解析 {} µs + 布局 {} µs {}",
                    st.parse_us,
                    st.layout_us,
                    if budget_ok { "✓ 预算内(<30ms)" } else { "✗ 超预算" }
                ))
                .color(if budget_ok { DIM } else { BAD })
                .monospace();
                ui.label(t);
                if !st.danger.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("⚠ 回溯风险 {} 处（引擎层可查）", st.danger.len()))
                            .color(BAD)
                            .strong(),
                    );
                }
            });
        });
        ui.add_space(2.0);
    });

    // ---- 正则每键重解析（预算内即时出图）；扫描走防抖 ----
    if regex_edited {
        reparse(st);
        st.scan_due = true;
        st.last_edit = now;
    }
    if st.scan_due && now - st.last_edit > DEBOUNCE {
        spawn_scan(st);
        st.scan_due = false;
    }

    // ---- 收后台结果（代次守卫） ----
    let mut fresh = false;
    while let Ok(msg) = st.rx.try_recv() {
        if msg.gen == st.gen {
            st.result = Some(msg);
            st.galleys.clear();
            fresh = true;
        }
    }
    if st.pending_scan {
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }

    // ---- 右侧样本文本：编辑框 + 高亮视图 ----
    egui::SidePanel::right("wb_sample")
        .min_width(430.0)
        .default_width(520.0)
        .resizable(true)
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("样本文本：");
                if st.pending_scan {
                    ui.label(egui::RichText::new("扫描中…").weak());
                } else if let Some(r) = &st.result {
                    match (&r.engine, &r.error) {
                        (Some((label, approx)), None) => {
                            let t = egui::RichText::new(format!("引擎 {label}"))
                                .color(if *approx { WARN } else { GOOD });
                            ui.label(t);
                            ui.label(format!("命中 {} 处", r.total));
                            ui.weak(format!("{:.1} ms", r.ms));
                        }
                        (_, Some(_)) => {
                            ui.label(egui::RichText::new("编译失败（见上方红字）").color(BAD));
                        }
                        (None, None) => {
                            ui.weak("无结果");
                        }
                    }
                } else {
                    ui.weak("等待扫描");
                }
                if ui.small_button("清空").clicked() {
                    st.sample.clear();
                    st.last_edit = now;
                    st.scan_due = true;
                    // 与编辑一致：作废在途扫描，防旧偏移套到新文本上
                    st.gen += 1;
                    st.result = None;
                    st.galleys.clear();
                }
            });
            let resp = ui.add(
                egui::TextEdit::multiline(&mut st.sample)
                    .desired_rows(4)
                    .font(FontId::monospace(12.5))
                    .desired_width(f32::INFINITY),
            );
            if resp.changed() {
                st.last_edit = now;
                st.scan_due = true;
                // 文本变了：立即失效旧高亮，防止错位显示；
                // 并作废在途扫描（旧消息的字节偏移对齐的是编辑前的文本，套回新文本可能 panic）
                st.gen += 1;
                st.result = None;
                st.galleys.clear();
            }
            ui.separator();
            ui.label(egui::RichText::new("匹配高亮（只读视图）：").weak());

            let lines: Vec<String> = st.sample.lines().map(str::to_string).collect();
            let gen = st.result.as_ref().map(|r| r.gen).unwrap_or(u64::MAX);
            let per_line: Vec<Vec<(usize, usize)>> = st
                .result
                .as_ref()
                .map(|r| r.per_line.clone())
                .unwrap_or_default();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let n = lines.len();
                    for (i, line) in lines.iter().enumerate() {
                        let empty: Vec<(usize, usize)> = Vec::new();
                        let hits = per_line.get(i).unwrap_or(&empty);
                        let galley = galley_for(st, ui.ctx(), gen, i, line, hits);
                        let resp = ui.add(egui::Label::new(egui::WidgetText::Galley(galley)));
                        if fresh && !hits.is_empty() {
                            resp.highlight();
                        }
                    }
                    if n == 0 {
                        ui.weak("(空)");
                    }
                });
        });

    // ---- 中央：按视图 ----
    egui::CentralPanel::default().show_inside(ui, |ui| match st.view {
        View::Plain => plain_view(ui, st),
        View::Railroad => railroad_view(ui, st),
        View::Engine => engine_view(ui, st),
    });
}

// ---- ① 人话层 ----
fn plain_view(ui: &mut egui::Ui, st: &mut State) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("人话层").strong());
        ui.weak("这段正则在找什么——悬停任一分段，上方「联动」行高亮对应片段");
    });
    ui.separator();

    if st.parse_err.is_some() {
        ui.label(
            egui::RichText::new("正则当前未通过解析，以下为已解析部分的人话。").color(WARN),
        );
    }

    if st.plain.is_empty() {
        ui.weak("（输入正则后这里给逐段中文解释）");
        st.hovered = None;
        return;
    }

    ui.label(egui::RichText::new(format!("整体流程：{}", st.plain_summary)).monospace());
    ui.add_space(2.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut hov = None;
            for seg in &st.plain {
                let slice = safe_slice(&st.regex_src, seg.span);
                let is_hover = st.hovered == Some(seg.span);
                let resp = ui
                    .horizontal(|ui| {
                        // 空说明段（交替引导）：只显示文字
                        if slice.is_empty() {
                            ui.label(egui::RichText::new(&seg.text).color(DIM));
                        } else {
                            let chip = if is_hover {
                                egui::RichText::new(format!("「{slice}」"))
                                    .monospace()
                                    .background_color(WARN)
                                    .color(Color32::from_rgb(30, 30, 30))
                            } else {
                                egui::RichText::new(format!("「{slice}」")).monospace().color(CHIP)
                            };
                            ui.label(chip);
                            let txt = egui::RichText::new(&seg.text);
                            ui.label(if is_hover { txt.strong() } else { txt });
                        }
                    })
                    .response;
                if resp.hovered() {
                    hov = Some(seg.span);
                }
            }
            st.hovered = hov;
        });
}

// ---- ② 结构层：铁路图 ----
fn railroad_view(ui: &mut egui::Ui, st: &mut State) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("结构层 · 铁路图").strong());
        ui.weak("滚轮缩放 / 拖拽平移；悬停节点联动上方正则片段");
        if ui.small_button("重置视图").clicked() {
            st.zoom = 1.0;
            st.offset = Vec2::new(36.0, 42.0);
        }
    });
    ui.separator();

    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    painter.rect_filled(resp.rect, 2.0, CANVAS_BG);
    if resp.dragged() {
        st.offset += resp.drag_delta();
    }
    if resp.hovered() {
        let sd = ui.input(|i| i.smooth_scroll_delta.y);
        if sd.abs() > 0.5 {
            let old = st.zoom;
            let new = (old * (1.0 + sd * 0.0015)).clamp(0.15, 10.0);
            if let Some(m) = resp.hover_pos() {
                let m = m.to_vec2();
                st.offset = m - (m - st.offset) * (new / old);
            }
            st.zoom = new;
        }
    }

    if let Some(d) = &st.diagram {
        // 悬停命中测试：逆变换到图形坐标，取最后命中（后画的在上层）
        if let Some(hp) = resp.hover_pos() {
            let px = (hp.x - resp.rect.min.x - st.offset.x) / st.zoom;
            let py = (hp.y - resp.rect.min.y - st.offset.y) / st.zoom;
            let mut found = None;
            for s in &d.shapes {
                match s {
                    Shape::Box { pos, size, src: Some(sp), .. }
                    | Shape::Diamond { pos, size, src: Some(sp), .. }
                        if px >= pos.x
                            && px <= pos.x + size.x
                            && py >= pos.y
                            && py <= pos.y + size.y =>
                    {
                        found = Some(*sp);
                    }
                    _ => {}
                }
            }
            st.hovered = found;
        } else {
            st.hovered = None;
        }
        draw_diagram(&painter, d, resp.rect.min, st.zoom, st.offset, st.hovered);
    } else {
        st.hovered = None;
        painter.text(
            resp.rect.center() - Vec2::new(0.0, 60.0),
            egui::Align2::CENTER_CENTER,
            "输入正则后这里出铁路图",
            FontId::proportional(16.0),
            DIM,
        );
    }
}

// ---- ③ 引擎层（M3）：灾难检测 + 教学解释器 + 回溯时间轴 ----
fn engine_view(ui: &mut egui::Ui, st: &mut State) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("引擎层").strong());
        ui.weak("回答「它为什么慢 / 为什么没匹配上」——自研回溯 VM，步数预算保平安");
    });
    ui.separator();

    // ---- 灾难性回溯静态检测（设计方案 5.3 安全拦截） ----
    if !st.danger.is_empty() {
        for r in &st.danger {
            let slice = safe_slice(&st.regex_src, r.outer);
            ui.label(
                egui::RichText::new(format!("⚠ 灾难性回溯风险：{}", r.desc))
                    .color(BAD)
                    .strong(),
            );
            ui.monospace(egui::RichText::new(format!("  风险构造：「{slice}」")).color(BAD));
        }
    }

    // ---- 教学解释器运行区 ----
    ui.horizontal(|ui| {
        ui.label("测试文本：");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut st.test_text)
                .desired_width(360.0)
                .font(FontId::monospace(13.0)),
        );
        if resp.changed() {
            st.interp = None; // 文本变了，旧轨迹失效
        }
        ui.separator();
        ui.label("步数预算：");
        const BUDGETS: [u32; 3] = [2_000, 10_000, 50_000];
        for (i, b) in BUDGETS.iter().enumerate() {
            if ui.selectable_label(st.budget_idx == i, fmt_steps(*b)).clicked() {
                st.budget_idx = i;
                st.interp = None;
            }
        }
        if ui.add_enabled(
            st.last_node.is_some(),
            egui::Button::new(egui::RichText::new("▶ 运行教学解释器").strong()),
        )
        .clicked()
        {
            if let Some(node) = &st.last_node {
                let r = regex_exec::interp::run(node, &st.test_text, BUDGETS[st.budget_idx]);
                st.scrub = 0;
                st.interp = Some(r);
            }
        }
        if ui.small_button("ReDoS 示例").clicked() {
            st.regex_src = "(a+)+b".into();
            st.test_text = format!("{}!", "a".repeat(32));
            reparse(st);
            st.scan_due = true;
            st.last_edit = ui.input(|i| i.time);
            st.interp = None;
        }
    });

    // ---- 结果 + 时间轴 ----
    if let Some(res) = &st.interp {
        match res {
            Err(e) => {
                ui.label(egui::RichText::new(format!("教学解释器：{e}")).color(WARN));
            }
            Ok(r) => {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    match r.verdict {
                        regex_exec::interp::Verdict::Matched => {
                            let (s, e) = r.match_span.unwrap_or((0, 0));
                            ui.label(egui::RichText::new(format!("✓ 命中 [{s}..{e})（字符下标）")).color(GOOD).strong());
                        }
                        regex_exec::interp::Verdict::NoMatch => {
                            ui.label(egui::RichText::new("✗ 全部起点尝试失败（无匹配）").color(BAD).strong());
                        }
                        regex_exec::interp::Verdict::BudgetExceeded => {
                            ui.label(
                                egui::RichText::new("⛔ 预算打满——灾难性回溯的实锤（真实引擎会在这里卡死）")
                                    .color(BAD)
                                    .strong(),
                            );
                        }
                    }
                    ui.separator();
                    ui.monospace(format!(
                        "步数 {}/{} | 选择 {} | 回退 {} | 最深栈 {} | 重启 {} 次",
                        r.steps.len(),
                        r.budget,
                        r.count_of(regex_exec::interp::Ev::Choice),
                        r.count_of(regex_exec::interp::Ev::Backtrack),
                        r.max_depth,
                        r.restarts.len(),
                    ));
                });

                if r.steps.is_empty() {
                    ui.weak("（空轨迹）");
                    st.hovered = None;
                    return;
                }

                // ---- 时间轴：桶状图，红=回退 蓝=选择 绿=前进 金=命中 ----
                ui.add_space(4.0);
                let width = ui.available_width();
                let (resp, painter) = ui.allocate_painter(egui::vec2(width, 72.0), Sense::click());
                let nb = (width / 4.0).max(20.0) as usize;
                let per = r.steps.len().div_ceil(nb).max(1);
                let ev_color = |ev: regex_exec::interp::Ev| -> Color32 {
                    use regex_exec::interp::Ev::*;
                    match ev {
                        Advance => Color32::from_rgb(76, 175, 80),
                        Choice => Color32::from_rgb(97, 200, 255),
                        Pure => DIM,
                        Lookaround => Color32::from_rgb(200, 150, 255),
                        Backtrack => Color32::from_rgb(229, 57, 53),
                        Match => WARN,
                    }
                };
                let mut max_total = 1usize;
                let mut buckets: Vec<[usize; 6]> = vec![[0; 6]; nb];
                for (i, s) in r.steps.iter().enumerate() {
                    let b = i / per;
                    if b < nb {
                        buckets[b][s.ev as usize] += 1;
                    }
                }
                for b in &buckets {
                    max_total = max_total.max(b.iter().sum::<usize>());
                }
                painter.rect_filled(resp.rect, 2.0, Color32::from_rgb(24, 26, 30));
                for (bi, counts) in buckets.iter().enumerate() {
                    let total: usize = counts.iter().sum();
                    if total == 0 {
                        continue;
                    }
                    let x0 = resp.rect.min.x + bi as f32 * (width / nb as f32);
                    let bw = (width / nb as f32).ceil().max(1.0);
                    let mut y = resp.rect.max.y;
                    // 事件堆叠（底→顶：回退/命中在最底显眼）
                    let stack = [
                        regex_exec::interp::Ev::Backtrack,
                        regex_exec::interp::Ev::Match,
                        regex_exec::interp::Ev::Choice,
                        regex_exec::interp::Ev::Lookaround,
                        regex_exec::interp::Ev::Advance,
                        regex_exec::interp::Ev::Pure,
                    ];
                    for ev in stack {
                        let c = counts[ev as usize];
                        if c == 0 {
                            continue;
                        }
                        let h = (c as f32 / max_total as f32 * 64.0).max(1.0);
                        let rect = egui::Rect::from_min_max(
                            Pos2::new(x0, y - h),
                            Pos2::new(x0 + bw - 0.5, y),
                        );
                        painter.rect_filled(rect, 0.0, ev_color(ev));
                        y -= h;
                    }
                }
                // 点击定位到桶起始步
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let bi = ((p.x - resp.rect.min.x) / (width / nb as f32)) as usize;
                        st.scrub = (bi * per).min(r.steps.len() - 1);
                    }
                }
                // 当前步刻度线
                let cur_b = st.scrub / per;
                let cx = resp.rect.min.x + (cur_b as f32 + 0.5) * (width / nb as f32);
                painter.line_segment(
                    [Pos2::new(cx, resp.rect.min.y), Pos2::new(cx, resp.rect.max.y)],
                    Stroke::new(1.5_f32, WARN),
                );

                // ---- 逐帧回放 ----
                ui.add_space(4.0);
                let max_step = r.steps.len() - 1;
                ui.add(egui::Slider::new(&mut st.scrub, 0..=max_step).text("步"));
                let s = &r.steps[st.scrub];
                ui.horizontal(|ui| {
                    let label = r
                        .labels
                        .get(s.pc as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("pc{}", s.pc));
                    ui.label(egui::RichText::new(format!("第 {} 步：{}", st.scrub, s.ev.label())).strong());
                    ui.separator();
                    ui.monospace(format!("指令 [pc{}] {label}", s.pc));
                    ui.monospace(egui::RichText::new(format!("栈深 {}", s.depth)).color(DIM));
                });
                // 文本游标视图（字符下标）
                let chars: Vec<char> = st.test_text.chars().collect();
                let pos = (s.pos as usize).min(chars.len());
                ui.horizontal(|ui| {
                    ui.label("文本游标：");
                    let pre: String = chars[..pos].iter().collect();
                    let cur: String = chars.get(pos).map(|c| c.to_string()).unwrap_or_else(|| "⏷".into());
                    let post: String = chars[(pos + 1).min(chars.len())..].iter().collect();
                    ui.monospace(egui::RichText::new(pre).color(DIM));
                    ui.monospace(egui::RichText::new(cur).background_color(WARN).color(Color32::from_rgb(30,30,30)).strong());
                    ui.monospace(egui::RichText::new(post).color(FG));
                });
            }
        }
    } else {
        ui.weak("输入测试文本后点「运行教学解释器」，这里给逐步回放；点「ReDoS 示例」看灾难性回溯长什么样。");
    }

    st.hovered = None;
}

fn fmt_steps(n: u32) -> String {
    if n >= 1000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

// ---- 重算：宽松解析 + 布局 + 人话（UI 线程，预算内） ----
fn reparse(st: &mut State) {
    let t0 = Instant::now();
    let (node, err) = regex_parser::parse_partial(&st.regex_src);
    st.parse_us = t0.elapsed().as_micros();
    st.parse_err = err;

    let t1 = Instant::now();
    st.diagram = if matches!(node, regex_ir::Node::Empty) && st.parse_err.is_some() {
        None // 无前缀可画
    } else {
        Some(regex_diagram::layout(&node))
    };
    st.layout_us = t1.elapsed().as_micros();

    st.plain = regex_plain::segments(&node);
    st.plain_summary = regex_plain::summary(&node);
    st.last_node = Some(node.clone());
    st.danger = regex_exec::danger::check(&node);
    st.hovered = None;
}

// ---- 后台扫描（代次守卫 + 防抖）----
fn spawn_scan(st: &mut State) {
    st.gen += 1;
    let gen = st.gen;
    let pattern = st.regex_src.clone();
    let sample = st.sample.clone();
    let (tx, rx) = channel();
    st.rx = rx;
    st.pending_scan = true;
    st.result = None;
    st.galleys.clear();
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let sc = regex_exec::Scanner::new(&pattern);
        let engine = sc.engine().map(|e| (e.label(), e.approximate()));
        let error = sc.error().map(str::to_string);
        let per_line: Vec<Vec<(usize, usize)>> = sample.lines().map(|l| sc.find_in(l)).collect();
        let total = per_line.iter().map(|m| m.len()).sum();
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        let _ = tx.send(ScanMsg { gen, engine, error, per_line, total, ms });
    });
    // 浏览器版（wasm）：std::thread 不可用——同步扫（样本量小；大文件属桌面 edition）
    #[cfg(target_arch = "wasm32")]
    {
        let t0 = Instant::now();
        let sc = regex_exec::Scanner::new(&pattern);
        let engine = sc.engine().map(|e| (e.label(), e.approximate()));
        let error = sc.error().map(str::to_string);
        let per_line: Vec<Vec<(usize, usize)>> = sample.lines().map(|l| sc.find_in(l)).collect();
        let total = per_line.iter().map(|m| m.len()).sum();
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        let _ = tx.send(ScanMsg { gen, engine, error, per_line, total, ms });
    }
}

// ---- 联动行：整条正则，悬停片段高亮 ----
fn linkage_job(src: &str, hovered: Option<Span>) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping::no_max_width(),
        ..Default::default()
    };
    let mono = || FontId::monospace(14.0);
    let plain = TextFormat { font_id: mono(), color: FG, ..Default::default() };
    let hit = TextFormat {
        font_id: mono(),
        color: Color32::from_rgb(30, 30, 30),
        background: WARN,
        ..Default::default()
    };
    match hovered {
        Some(sp) => {
            let (s, e) = (sp.start as usize, sp.end as usize);
            if s > 0 {
                job.append(&src[..s], 0.0, plain.clone());
            }
            if e > s {
                job.append(&src[s..e], 0.0, hit);
            }
            if e < src.len() {
                job.append(&src[e..], 0.0, plain);
            }
        }
        None => job.append(src, 0.0, plain),
    }
    job
}

// ---- 高亮行 galley（代次缓存） ----
fn galley_for(
    st: &mut State,
    ctx: &egui::Context,
    gen: u64,
    i: usize,
    line: &str,
    hits: &[(usize, usize)],
) -> std::sync::Arc<egui::Galley> {
    if let Some((g, gal)) = st.galleys.get(&i) {
        if *g == gen {
            return gal.clone();
        }
    }
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping::no_max_width(),
        ..Default::default()
    };
    let mono = || FontId::monospace(12.5);
    let plain = TextFormat { font_id: mono(), color: FG, ..Default::default() };
    let hit = TextFormat {
        font_id: mono(),
        color: FG,
        background: Color32::from_rgba_unmultiplied(255, 213, 79, 110),
        ..Default::default()
    };
    let mut cursor = 0usize;
    for &(s, e) in hits {
        if s > cursor {
            job.append(&line[cursor..s], 0.0, plain.clone());
        }
        job.append(&line[s..e], 0.0, hit.clone());
        cursor = e;
    }
    if cursor < line.len() {
        job.append(&line[cursor..], 0.0, plain.clone());
    }
    let gal = ctx.fonts(|f| f.layout_job(job));
    st.galleys.insert(i, (gen, gal.clone()));
    gal
}

// ---- 铁路图绘制（M2：悬停高亮） ----
fn draw_diagram(
    painter: &egui::Painter,
    d: &Diagram,
    origin: Pos2,
    zoom: f32,
    offset: Vec2,
    hovered: Option<Span>,
) {
    let t = |p: regex_diagram::Pt| -> Pos2 {
        Pos2::new(origin.x + offset.x + p.x * zoom, origin.y + offset.y + p.y * zoom)
    };
    let stroke_w = (1.6 * zoom).clamp(0.6, 4.0);
    for shape in &d.shapes {
        match shape {
            Shape::Polyline { pts, style } => {
                let (color, w) = match style {
                    LineStyle::Main => (STROKE_C, stroke_w),
                    LineStyle::Bypass => (BYPASS_C, (0.9 * zoom).clamp(0.4, 2.0)),
                    LineStyle::Loop => (BYPASS_C, (0.7 * zoom).clamp(0.35, 2.0)),
                };
                let points: Vec<Pos2> = pts.iter().map(|p| t(*p)).collect();
                if points.len() >= 2 {
                    painter.add(egui::Shape::line(points, Stroke::new(w, color)));
                }
            }
            Shape::Box { pos, size, label, style, src } => {
                let min = t(*pos);
                let max = t(regex_diagram::Pt::new(pos.x + size.x, pos.y + size.y));
                let active = src.map(|s| Some(s) == hovered).unwrap_or(false);
                let fill = if active {
                    FILL_ACTIVE
                } else {
                    match style {
                        NodeStyle::Literal => FILL_LITERAL,
                        NodeStyle::Class => FILL_CLASS,
                        NodeStyle::Group => FILL_GROUP,
                    }
                };
                let rect = egui::Rect::from_min_max(min, max);
                painter.rect(
                    rect,
                    egui::Rounding::same((7.0 * zoom).clamp(1.0, 10.0)),
                    fill,
                    Stroke::new(
                        if active { stroke_w + 0.8 } else { stroke_w },
                        if active { WARN } else { STROKE_C },
                    ),
                );
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    FontId::proportional((14.0 * zoom).clamp(6.0, 40.0)),
                    TEXT_C,
                );
            }
            Shape::Diamond { pos, size, label, src } => {
                let (cx, cy) = (pos.x + size.x / 2.0, pos.y + size.y / 2.0);
                let c = t(regex_diagram::Pt::new(cx, cy));
                let pts = vec![
                    t(regex_diagram::Pt::new(cx, cy - size.y / 2.0)),
                    t(regex_diagram::Pt::new(cx + size.x / 2.0, cy)),
                    t(regex_diagram::Pt::new(cx, cy + size.y / 2.0)),
                    t(regex_diagram::Pt::new(cx - size.x / 2.0, cy)),
                ];
                let active = src.map(|s| Some(s) == hovered).unwrap_or(false);
                let fill = if active { FILL_ACTIVE } else { FILL_DIAMOND };
                painter.add(egui::Shape::convex_polygon(
                    pts,
                    fill,
                    Stroke::new(
                        if active { stroke_w + 0.8 } else { stroke_w },
                        if active { WARN } else { STROKE_C },
                    ),
                ));
                painter.text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    label,
                    FontId::proportional((13.0 * zoom).clamp(6.0, 40.0)),
                    TEXT_C,
                );
            }
            Shape::Arrow { tip } => {
                // 汇入箭头（实心三角，随缩放）
                let t0 = t(*tip);
                let (mut w, mut h) = (7.0 * zoom, 3.5 * zoom);
                w = w.clamp(3.0, 16.0);
                h = h.clamp(1.5, 8.0);
                painter.add(egui::Shape::convex_polygon(
                    vec![t0, Pos2::new(t0.x - w, t0.y - h), Pos2::new(t0.x - w, t0.y + h)],
                    STROKE_C,
                    Stroke::NONE,
                ));
            }
            Shape::Text { pos, label, size } => {
                let p = t(*pos);
                painter.text(
                    Pos2::new(p.x, p.y + size * zoom),
                    egui::Align2::LEFT_TOP,
                    label,
                    FontId::proportional((*size * zoom).clamp(6.0, 24.0)),
                    DIM,
                );
            }
        }
    }
}

fn export_svg(d: &Diagram) -> Option<String> {
    let svg = regex_export::diagram_to_svg(d);
    let path = std::env::current_dir().ok()?.join("m2-diagram.svg");
    std::fs::write(&path, &svg).ok()?;
    Some(format!("已导出 {}（{} 字节）", path.display(), svg.len()))
}

/// span 切源码（防御式：正则中途变更时旧 span 越界返回空串，不 panic）
fn safe_slice(src: &str, sp: Span) -> &str {
    src.get(sp.start as usize..sp.end as usize).unwrap_or("")
}
