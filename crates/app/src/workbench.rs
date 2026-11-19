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
    ("日期", r"\b(\d{4})-(\d{2})-(\d{2})\b"),
    ("时间", r"([01]\d|2[0-3]):[0-5]\d"),
    ("IP 地址", r"\b\d{1,3}(\.\d{1,3}){3}\b"),
    ("手机号", r"\b1[3-9]\d{9}\b"),
    ("邮箱", r"[\w.+-]+@[\w-]+\.[\w.]+"),
    ("URL", r"https?://[^\s,，)]+"),
    ("汉字", r"\p{Han}+"),
    ("浮点数", r"-?\d+\.\d+"),
    ("十六进制色", r"#[0-9a-fA-F]{6}\b"),
    ("HTML 标签", r"</?[a-zA-Z][^>]*>"),
    ("贪婪", r#""(.+)""#),
    ("懒惰", r#""(.+?)""#),
    ("重复词", r"\b(\w+) \1\b"),
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

/// 后台扫描线程回传的结果（代次守卫：gen 不匹配即丢弃）。
/// per_sample：每个样本文本 → 其内每行 → 命中区间（样本文本之间不做字节偏移拼接）
struct ScanMsg {
    gen: u64,
    engine: Option<(&'static str, bool)>, // (label, approximate)
    error: Option<String>,
    per_sample: Vec<Vec<Vec<(usize, usize)>>>,
    total: usize,
    ms: f32,
    /// 对比正则 B 的结果（cmp_on 时才有）
    cmp: Option<CmpResult>,
}

/// 两正则对比的正则 B 侧结果（命中数由 span 集合现算，不存字段）
struct CmpResult {
    error: Option<String>,
    per_sample: Vec<Vec<Vec<(usize, usize)>>>,
}

/// 样本文本形态：TEXT 单块（原样）/ TEXTS 多样本（可加多条，逐条扫描高亮）
#[derive(Clone, Copy, PartialEq)]
pub enum SampleMode {
    Text,
    Texts,
}

/// 历史快照（会话内存版；恢复 = 正则 + 标志 + 样本整体回滚）
struct HistoryEntry {
    time: String,
    regex: String,
    flags: [bool; 4],
    samples: Vec<String>,
}

pub struct State {
    regex_src: String,
    /// TEXT 模式恒 1 条；TEXTS 模式可多条。空列表不允许（UI 兜底补一条）
    samples: Vec<String>,
    mode: SampleMode,

    // RegexBuddy 式标志开关：生效 = 正则前拼内联标志喂给引擎/替换/对比/解释器
    flag_i: bool,
    flag_s: bool,
    flag_m: bool,
    flag_x: bool,

    // History / 替换预览 / 两正则对比
    history: Vec<HistoryEntry>,
    repl_on: bool,
    repl_template: String,
    /// (gen, 模板, 每样本替换结果)——gen 或模板变了重算
    repl_cache: Option<(u64, String, Vec<String>)>,
    cmp_on: bool,
    cmp_src: String,

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
    /// 高亮视图空白可视化：空格→· Tab→→ 行尾→↵
    show_ws: bool,
}

impl Default for State {
    fn default() -> Self {
        let mut st = Self {
            regex_src: r"\b(\d{4})-(\d{2})-(\d{2})\b".to_string(),
            samples: vec!["Contract signed on 2024-08-15, amended 2025-01-01 (see 2025/02/03 doc).\n\
                     He said \"hello\", then \"bye\".\n\
                     the the cat cat dog\n\
                     2026-11-17 worker-3 处理订单 order=ORD-2026-000042 user=张伟 ip=10.20.30.40 123ms\n\
                     price 1234. at end."
                .to_string()],
            mode: SampleMode::Text,
            flag_i: false,
            flag_s: false,
            flag_m: false,
            flag_x: false,
            history: Vec::new(),
            repl_on: false,
            repl_template: "[$1]".to_string(),
            repl_cache: None,
            cmp_on: false,
            cmp_src: String::new(),
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
            show_ws: true,
        };
        // 默认正则必须首帧就解析：否则打开应用后三视图空白，
        // 要敲任意一个键触发 changed() 才有内容（实测：首帧有正则、视图不显示）
        reparse(&mut st);
        st
    }
}

const DEBOUNCE: f64 = 0.3;

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    let ctx = ui.ctx().clone();
    let now = ui.input(|i| i.time);

    let mut regex_edited = false;
    egui::Panel::top("wb_top").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("正则：");
            let ctx2 = ui.ctx().clone();
            let mut layouter = move |_ui: &egui::Ui, s: &dyn egui::TextBuffer, _w: f32| -> std::sync::Arc<egui::Galley> {
                let job = colorize_regex(s.as_str());
                ctx2.fonts_mut(|f| f.layout_job(job))
            };
            let out = egui::TextEdit::multiline(&mut st.regex_src)
                .desired_rows(2)
                .desired_width(380.0)
                .font(FontId::monospace(14.0))
                .layouter(&mut layouter)
                .show(ui);
            if out.response.changed() {
                regex_edited = true;
                // 作废在途扫描：旧正则的扫描结果回来时 gen 已不匹配（防内容错位）
                st.gen += 1;
            }
            // RegexBuddy Insert Token 式插入器（光标处插入 / 包裹选区）
            insert_menu(ui, st, &out, &mut regex_edited);
        });
        // 示例单独一行 + 可换行：16 个按钮一行放不下（horizontal 不换行会被裁掉）
        ui.horizontal_wrapped(|ui| {
            ui.label("示例：");
            for (name, pat) in EXAMPLES {
                if ui.small_button(*name).clicked() {
                    st.regex_src = (*pat).to_string();
                    regex_edited = true;
                }
            }
        });
        // 方言说明：这里没有"选语言"——匹配固定走 Rust regex 真身 + fancy-regex 近似
        ui.weak(
            "方言 = Rust regex（Rust/Go 家族·线性时间）；含环视/反向引用自动落 fancy-regex（回溯家族·近似）；\
             支持 JS 风格 /pattern/flags（g/i/m/s）",
        );

        // RegexBuddy 式标志开关：拼内联标志喂给引擎/替换/对比/解释器
        ui.horizontal(|ui| {
            ui.label("标志：");
            for (on, label, tip) in [
                (&mut st.flag_i, "i 忽略大小写", "(?i)"),
                (&mut st.flag_s, "s 单行(.)含换行", "(?s)"),
                (&mut st.flag_m, "m 多行(^$按行)", "(?m)"),
                (&mut st.flag_x, "x 自由格式", "(?x)"),
            ] {
                if ui
                    .checkbox(on, label)
                    .on_hover_text(format!("等价于给正则前加 {tip}"))
                    .changed()
                {
                    st.gen += 1;
                    st.result = None;
                    st.scan_due = true;
                    st.last_edit = now;
                }
            }
            ui.weak("（铁路图/人话按你输入的原文展示，不拼标志前缀）");
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
            let gal = ui.ctx().fonts_mut(|f| f.layout_job(job));
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
            ui.separator();
            if ui.selectable_label(st.cmp_on, "对比").clicked() {
                st.cmp_on = !st.cmp_on;
                st.gen += 1; // 扫描消息里是否带 B 结果变了，换代重扫
                st.scan_due = true;
                st.last_edit = now;
                st.result = None;
            }
            history_menu(ui, st);
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

        // 两正则对比（RegexBuddy Compare 式）：同一样本上 A/B 命中集合 diff
        if st.cmp_on {
            ui.horizontal(|ui| {
                ui.label("对比正则 B：");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut st.cmp_src)
                        .desired_width(380.0)
                        .font(FontId::monospace(13.0))
                        .hint_text("输入第二个正则（吃同一套标志），比两边的命中集合"),
                );
                if resp.changed() {
                    st.gen += 1;
                    st.result = None;
                    st.scan_due = true;
                    st.last_edit = now;
                }
            });
            if let Some(r) = &st.result {
                if let Some(cmp) = &r.cmp {
                    match &cmp.error {
                        Some(e) => {
                            let first = e.lines().next().unwrap_or("").to_string();
                            ui.label(
                                egui::RichText::new(format!("✗ 正则 B 编译失败：{first}")).color(BAD),
                            );
                        }
                        None => {
                            let set_a = span_set(&r.per_sample);
                            let set_b = span_set(&cmp.per_sample);
                            let both = set_a.intersection(&set_b).count();
                            let a_only: Vec<_> = set_a.difference(&set_b).copied().collect();
                            let b_only: Vec<_> = set_b.difference(&set_a).copied().collect();
                            ui.monospace(format!(
                                "A 命中 {}｜B 命中 {}｜共有 {}｜仅 A {}｜仅 B {}",
                                set_a.len(),
                                set_b.len(),
                                both,
                                a_only.len(),
                                b_only.len()
                            ));
                            egui::ScrollArea::vertical()
                                .max_height(110.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let mut shown = 0usize;
                                    for (tag, color, list) in
                                        [("仅A", WARN, &a_only), ("仅B", CHIP, &b_only)]
                                    {
                                        for &(si, li, s, e) in list.iter() {
                                            if shown >= 50 {
                                                break;
                                            }
                                            let text = st
                                                .samples
                                                .get(si)
                                                .and_then(|sm| sm.lines().nth(li))
                                                .and_then(|l| l.get(s..e))
                                                .unwrap_or("");
                                            ui.monospace(egui::RichText::new(format!(
                                                "{tag} S{}L{} [{s}..{e}) {text}",
                                                si + 1,
                                                li + 1
                                            )).color(color));
                                            shown += 1;
                                        }
                                    }
                                    if a_only.len() + b_only.len() > 50 {
                                        ui.weak("…其余省略");
                                    }
                                });
                        }
                    }
                }
            }
        }
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

    // ---- 右侧样本文本：TEXT 单块 / TEXTS 多样本 + 高亮视图 + 命中列表 ----
    // 布局规则（0.37 面板）：底部面板按代码序自下而上堆叠——命中列表最底，其上
    // 替换预览，其上样本编辑区；顶部剩余给状态行 + 高亮。编辑区面板定高 + 内部
    // 滚动，内容再多也只在面板内滚（旧版编辑区溢出压高亮区的 bug 根源在此）。
    egui::Panel::right("wb_sample")
        .min_size(430.0)
        .default_size(520.0)
        .resizable(true)
        .show(ui, |ui| {
            let per_sample_lines: Vec<Vec<String>> = st
                .samples
                .iter()
                .map(|s| s.lines().map(str::to_string).collect())
                .collect();
            let gen = st.result.as_ref().map(|r| r.gen).unwrap_or(u64::MAX);
            let per_sample: Vec<Vec<Vec<(usize, usize)>>> = st
                .result
                .as_ref()
                .map(|r| r.per_sample.clone())
                .unwrap_or_default();

            // ① 命中列表（最底）：S=样本序号 L=行号；TEXT 模式省略 S 前缀
            egui::Panel::bottom("wb_hit_list")
                .max_size(190.0)
                .show(ui, |ui| {
                    let total_all: usize = per_sample
                        .iter()
                        .map(|s| s.iter().map(|l| l.len()).sum::<usize>())
                        .sum();
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("命中列表").strong());
                        ui.weak(format!("{total_all} 条"));
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if st.result.is_none() {
                                ui.weak("（等待扫描…）");
                                return;
                            }
                            let multi = st.samples.len() > 1;
                            let mut shown = 0usize;
                            'outer: for (si, per_lines) in per_sample.iter().enumerate() {
                                for (li, hits) in per_lines.iter().enumerate() {
                                    for &(s, e) in hits {
                                        let text = per_sample_lines
                                            .get(si)
                                            .and_then(|ls| ls.get(li))
                                            .and_then(|l| l.get(s..e))
                                            .unwrap_or("⚠区间越界");
                                        let mut text = text.to_string();
                                        if text.chars().count() > 48 {
                                            text = format!(
                                                "{}…",
                                                text.chars().take(48).collect::<String>()
                                            );
                                        }
                                        let tag = if multi {
                                            format!("S{}L{} [{s}..{e})  {text}", si + 1, li + 1)
                                        } else {
                                            format!("L{} [{s}..{e})  {text}", li + 1)
                                        };
                                        ui.monospace(tag);
                                        shown += 1;
                                        if shown >= 400 {
                                            break 'outer;
                                        }
                                    }
                                }
                            }
                            if total_all == 0 {
                                ui.weak("（无命中——检查正则与样本文本）");
                            } else if shown < total_all {
                                ui.weak(format!("…其余 {} 条省略", total_all - shown));
                            }
                        });
                });

            // ② 替换预览（命中列表上方；开关在高亮行）
            if st.repl_on {
                egui::Panel::bottom("wb_repl")
                    .max_size(150.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("替换预览").strong());
                            ui.label("模板：");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut st.repl_template)
                                        .desired_width(160.0)
                                        .font(FontId::monospace(12.5)),
                                )
                                .changed()
                            {
                                st.repl_cache = None;
                            }
                            ui.weak("$1 / $name 引用捕获组");
                        });
                        let template = st.repl_template.clone();
                        let gen_now = st.result.as_ref().map(|r| r.gen).unwrap_or(u64::MAX);
                        let stale = st
                            .repl_cache
                            .as_ref()
                            .map_or(true, |(g, t, _)| *g != gen_now || *t != template);
                        if stale {
                            if let Some(r) = &st.result {
                                if r.error.is_none() {
                                    let sc = regex_exec::Scanner::new(&effective_pattern(st));
                                    let outs: Vec<String> = st
                                        .samples
                                        .iter()
                                        .map(|s| {
                                            s.lines()
                                                .map(|l| {
                                                    sc.replace_all_in(l, &template)
                                                        .unwrap_or_else(|| l.to_string())
                                                })
                                                .collect::<Vec<_>>()
                                                .join("\n")
                                        })
                                        .collect();
                                    st.repl_cache = Some((gen_now, template, outs));
                                }
                            }
                        }
                        if let Some((_, _, outs)) = &st.repl_cache {
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let multi = st.samples.len() > 1;
                                    for (i, o) in outs.iter().enumerate() {
                                        if multi {
                                            ui.weak(format!("── 替换后 样本 {} ──", i + 1));
                                        }
                                        ui.monospace(o);
                                    }
                                });
                        }
                    });
            }

            // ③ 样本编辑区（定高 + 内部滚动；编辑任何一条都触发重扫）
            egui::Panel::bottom("wb_editor")
                .default_size(if st.mode == SampleMode::Texts { 240.0 } else { 170.0 })
                .min_size(110.0)
                .max_size(340.0)
                .resizable(true)
                .show(ui, |ui| {
                    let mut sample_changed = false;
                    match st.mode {
                        SampleMode::Text => {
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let resp = ui.add(
                                        egui::TextEdit::multiline(&mut st.samples[0])
                                            .desired_rows(4)
                                            .font(FontId::monospace(12.5))
                                            .desired_width(f32::INFINITY),
                                    );
                                    sample_changed = resp.changed();
                                });
                        }
                        SampleMode::Texts => {
                            ui.horizontal(|ui| {
                                ui.weak(format!(
                                    "{} 个样本文本（逐条独立扫描）",
                                    st.samples.len()
                                ));
                                if ui.small_button("＋ 添加").clicked() {
                                    st.samples.push(String::new());
                                    sample_changed = true;
                                }
                            });
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    'smp: for i in 0..st.samples.len() {
                                        let mut removed = false;
                                        ui.horizontal(|ui| {
                                            ui.monospace(
                                                egui::RichText::new(format!("样本 {}", i + 1))
                                                    .color(DIM),
                                            );
                                            if ui
                                                .add_enabled(
                                                    st.samples.len() > 1,
                                                    egui::Button::new("✕").small(),
                                                )
                                                .clicked()
                                            {
                                                st.samples.remove(i);
                                                removed = true;
                                            }
                                        });
                                        if removed {
                                            sample_changed = true;
                                            break 'smp; // 索引已移位，本轮到此为止（下一帧重画）
                                        }
                                        let resp = ui.add(
                                            egui::TextEdit::multiline(&mut st.samples[i])
                                                .desired_rows(2)
                                                .font(FontId::monospace(12.5))
                                                .desired_width(f32::INFINITY),
                                        );
                                        if resp.changed() {
                                            sample_changed = true;
                                        }
                                        ui.add_space(2.0);
                                    }
                                });
                        }
                    }
                    if sample_changed {
                        st.last_edit = now;
                        st.scan_due = true;
                        // 文本变了：立即失效旧高亮，防止错位显示；
                        // 并作废在途扫描（旧消息的字节偏移对齐的是编辑前的文本，套回新文本可能 panic）
                        st.gen += 1;
                        st.result = None;
                        st.galleys.clear();
                    }
                });

            // ④ 顶部剩余：状态行 + 高亮视图
            ui.horizontal(|ui| {
                // 形态切换：TEXT 单样本（原样）/ TEXTS 多样本可增删
                if ui.selectable_label(st.mode == SampleMode::Text, "TEXT").clicked() {
                    st.mode = SampleMode::Text;
                    st.gen += 1;
                    st.galleys.clear(); // 行号映射变了，缓存必须换代
                }
                if ui.selectable_label(st.mode == SampleMode::Texts, "TEXTS").clicked() {
                    st.mode = SampleMode::Texts;
                    st.gen += 1;
                    st.galleys.clear();
                }
                ui.separator();
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
                    st.samples = vec![String::new()];
                    st.last_edit = now;
                    st.scan_due = true;
                    // 与编辑一致：作废在途扫描，防旧偏移套到新文本上
                    st.gen += 1;
                    st.result = None;
                    st.galleys.clear();
                }
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("匹配高亮（只读视图）：").weak());
                let btn = if st.show_ws { "空白可视化：开" } else { "空白可视化：关" };
                if ui.small_button(btn).clicked() {
                    st.show_ws = !st.show_ws;
                    st.galleys.clear();
                }
                ui.separator();
                if ui.checkbox(&mut st.repl_on, "替换预览").changed() {
                    st.repl_cache = None;
                }
            });
            // 高亮：TEXTS 多样本逐块展示（样本头 + 行）；TEXT 单块直出
            let multi = st.mode == SampleMode::Texts && st.samples.len() > 1;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut gidx = 0usize; // galley 缓存键：跨样本全局行号（换 mode 已换代）
                    for (si, lines) in per_sample_lines.iter().enumerate() {
                        if multi {
                            let hits_n: usize = per_sample
                                .get(si)
                                .map(|s| s.iter().map(|l| l.len()).sum::<usize>())
                                .unwrap_or(0);
                            ui.weak(format!("── 样本 {}（{} 处命中）──", si + 1, hits_n));
                        }
                        for (li, line) in lines.iter().enumerate() {
                            let empty: Vec<(usize, usize)> = Vec::new();
                            let hits = per_sample
                                .get(si)
                                .and_then(|s| s.get(li))
                                .unwrap_or(&empty);
                            let galley = galley_for(st, ui.ctx(), gen, gidx, line, hits);
                            let resp = ui.add(egui::Label::new(egui::WidgetText::Galley(galley)));
                            if fresh && !hits.is_empty() {
                                resp.highlight();
                            }
                            gidx += 1;
                        }
                        if multi {
                            ui.add_space(4.0);
                        }
                    }
                    if gidx == 0 {
                        ui.weak("(空)");
                    }
                });
        });

    // ---- 中央：按视图 ----
    egui::CentralPanel::default_margins().show(ui, |ui| match st.view {
        View::Plain => plain_view(ui, st),
        View::Railroad => railroad_view(ui, st),
        View::Engine => engine_view(ui, st),
    });
}

// ---- ① 人话层（Explain Token 树：按组嵌套缩进，RegexBuddy 式） ----
fn plain_view(ui: &mut egui::Ui, st: &mut State) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("人话层").strong());
        ui.weak("按组嵌套逐 token 解释——悬停任一分段，上方「联动」行高亮对应片段");
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

    // 树深度 = 分段被多少个组节点包裹（IR 组 span 精确包含，正则原文无需再数括号）
    let mut groups: Vec<Span> = Vec::new();
    if let Some(node) = &st.last_node {
        collect_group_spans(node, &mut groups);
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut hov = None;
            for seg in &st.plain {
                let depth = groups
                    .iter()
                    .filter(|g| seg.span.start >= g.start && seg.span.end <= g.end)
                    .count();
                let slice = safe_slice(&st.regex_src, seg.span);
                let is_hover = st.hovered == Some(seg.span);
                let indent = if depth == 0 {
                    String::new()
                } else {
                    format!("{}├─ ", "│  ".repeat(depth - 1))
                };
                let resp = ui
                    .horizontal(|ui| {
                        ui.monospace(egui::RichText::new(&indent).color(DIM));
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

    // ---- 教学解释器运行区（两行布局：宽输入独占一行，控件一行，避免横向溢出挤坏面板） ----
    ui.horizontal(|ui| {
        ui.label("测试文本：");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut st.test_text)
                .desired_width(f32::INFINITY)
                .font(FontId::monospace(13.0)),
        );
        if resp.changed() {
            st.interp = None; // 文本变了，旧轨迹失效
        }
    });
    ui.horizontal(|ui| {
        ui.label("步数预算：");
        const BUDGETS: [u32; 3] = [2_000, 10_000, 50_000];
        for (i, b) in BUDGETS.iter().enumerate() {
            if ui.selectable_label(st.budget_idx == i, fmt_steps(*b)).clicked() {
                st.budget_idx = i;
                st.interp = None;
            }
        }
        ui.separator();
        if ui.add_enabled(
            st.last_node.is_some(),
            egui::Button::new(egui::RichText::new("▶ 运行教学解释器").strong()),
        )
        .clicked()
        {
            // 标志开关生效：按 (?i)… 前缀重解析再跑（Flags 节点在解释器里是零宽操作）
            let (node, _) = regex_parser::parse_partial(&effective_pattern(st));
            let r = regex_exec::interp::run(&node, &st.test_text, BUDGETS[st.budget_idx]);
            st.scrub = 0;
            st.interp = Some(r);
        }
        if ui.small_button("用右侧样本文本").clicked() {
            st.test_text = st.samples.join("\n");
            st.interp = None;
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

// ---- 后台扫描（代次守卫 + 防抖；逐样本独立扫，样本间不拼接偏移）----
fn spawn_scan(st: &mut State) {
    st.gen += 1;
    let gen = st.gen;
    let pattern = effective_pattern(st);
    let cmp_on = st.cmp_on && !st.cmp_src.trim().is_empty();
    let cmp_pattern = st.cmp_src.clone();
    let samples = st.samples.clone();
    // History 式快照：正则/样本稳定下来（防抖到期）就落一条，去重相邻重复，上限 30
    {
        let entry = HistoryEntry {
            time: crate::logging::now_hms(),
            regex: st.regex_src.clone(),
            flags: [st.flag_i, st.flag_s, st.flag_m, st.flag_x],
            samples: st.samples.clone(),
        };
        let differs = st
            .history
            .last()
            .map(|h| h.regex != entry.regex || h.samples != entry.samples || h.flags != entry.flags)
            .unwrap_or(true);
        if differs {
            st.history.push(entry);
            if st.history.len() > 30 {
                st.history.remove(0);
            }
        }
    }
    let (tx, rx) = channel();
    st.rx = rx;
    st.pending_scan = true;
    st.result = None;
    st.galleys.clear();
    st.repl_cache = None;
    let scan = move || -> ScanMsg {
        let t0 = Instant::now();
        let sc = regex_exec::Scanner::new(&pattern);
        let engine = sc.engine().map(|e| (e.label(), e.approximate()));
        let error = sc.error().map(str::to_string);
        let mut per_sample = Vec::with_capacity(samples.len());
        let mut total = 0usize;
        for s in &samples {
            let per_line: Vec<Vec<(usize, usize)>> = s.lines().map(|l| sc.find_in(l)).collect();
            total += per_line.iter().map(|m| m.len()).sum::<usize>();
            per_sample.push(per_line);
        }
        // 对比正则 B：同一样本、同一套标志
        let cmp = if cmp_on {
            let scb = regex_exec::Scanner::new(&cmp_pattern);
            let c_engine = scb.engine().map(|e| (e.label(), e.approximate()));
            let c_error = scb.error().map(str::to_string);
            let mut c_per = Vec::with_capacity(samples.len());
            let mut c_total = 0usize;
            for s in &samples {
                let per_line: Vec<Vec<(usize, usize)>> = s.lines().map(|l| scb.find_in(l)).collect();
                c_total += per_line.iter().map(|m| m.len()).sum::<usize>();
                c_per.push(per_line);
            }
            let _ = (c_engine, c_total); Some(CmpResult { error: c_error, per_sample: c_per })
        } else {
            None
        };
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        ScanMsg { gen, engine, error, per_sample, total, ms, cmp }
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let _ = tx.send(scan());
    });
    // 浏览器版（wasm）：std::thread 不可用——同步扫（样本量小；大文件属桌面 edition）
    #[cfg(target_arch = "wasm32")]
    {
        let _ = tx.send(scan());
    }
}

/// 外部入口（反推正则页「发送到工作台」）：设正则并立即重解析 + 触发重扫
pub fn set_regex(st: &mut State, src: String) {
    st.regex_src = src;
    st.gen += 1;
    st.result = None;
    st.galleys.clear();
    reparse(st);
    st.scan_due = true;
    st.last_edit = f64::NEG_INFINITY; // 立即到期，不等防抖
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
// 命中配色：实底琥珀 + 深字（原半透明底叠浅字发灰、对比度差）。
// 空白可视化：空格→· Tab→→ 行尾→↵。映射是 1 字符↔1 字符，命中区间判定不受影响。
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
        color: Color32::from_rgb(43, 30, 0),
        background: Color32::from_rgba_unmultiplied(255, 193, 7, 210),
        ..Default::default()
    };
    // 逐字符分段：命中归属按「字符起始字节 ∈ [s,e)」判定（引擎给的边界都是字符边界）
    let mut seg = String::new();
    let mut seg_hit = false;
    let flush = |job: &mut egui::text::LayoutJob, seg: &mut String, hit_flag: bool| {
        if !seg.is_empty() {
            let fmt = if hit_flag { hit.clone() } else { plain.clone() };
            job.append(seg.as_str(), 0.0, fmt);
            seg.clear();
        }
    };
    for (b, c) in line.char_indices() {
        let is_hit = hits.iter().any(|&(s, e)| b >= s && b < e);
        if is_hit != seg_hit {
            flush(&mut job, &mut seg, seg_hit);
            seg_hit = is_hit;
        }
        match c {
            ' ' if st.show_ws => seg.push('·'),
            '\t' if st.show_ws => seg.push('→'),
            _ => seg.push(c),
        }
    }
    flush(&mut job, &mut seg, seg_hit);
    if st.show_ws {
        // lines() 已剥离 \n：行尾补一个 ↵，空行也因此可见
        job.append("↵", 0.0, plain);
    }
    let gal = ctx.fonts_mut(|f| f.layout_job(job));
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
                    egui::CornerRadius::same((7.0 * zoom).clamp(1.0, 10.0) as u8),
                    fill,
                    Stroke::new(
                        if active { stroke_w + 0.8 } else { stroke_w },
                        if active { WARN } else { STROKE_C },
                    ),
                    egui::StrokeKind::Middle,
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

// ---- RegexBuddy 参照组：插入器 / 标志 / 历史 / 替换 / 对比 ----

/// 标志开关的生效形式：内联标志前缀 + 用户正则。只喂引擎/替换/对比/解释器；
/// 铁路图/人话仍按原文解析——span 必须与用户看到的文本对齐。
fn effective_pattern(st: &State) -> String {
    let mut p = String::new();
    if st.flag_i {
        p.push_str("(?i)");
    }
    if st.flag_s {
        p.push_str("(?s)");
    }
    if st.flag_m {
        p.push_str("(?m)");
    }
    if st.flag_x {
        p.push_str("(?x)");
    }
    p.push_str(&st.regex_src);
    p
}

/// 一个可插入 token：before+选区+after。inside=光标停在包裹内容尾部（组/环视）
#[derive(Clone, Copy, Debug)]
struct Token {
    label: &'static str,
    before: &'static str,
    after: &'static str,
    inside: bool,
}

const fn t(label: &'static str, before: &'static str, after: &'static str, inside: bool) -> Token {
    Token { label, before, after, inside }
}

/// RegexBuddy Insert Token 菜单的本土化版（按本仓 parser/引擎支持裁剪）
const TOKEN_MENU: &[(&str, &[Token])] = &[
    (
        "简写字符类",
        &[
            t(r"\d 数字", r"\d", "", false),
            t(r"\D 非数字", r"\D", "", false),
            t(r"\w 单词字符", r"\w", "", false),
            t(r"\W 非单词", r"\W", "", false),
            t(r"\s 空白", r"\s", "", false),
            t(r"\S 非空白", r"\S", "", false),
        ],
    ),
    (
        "锚点",
        &[
            t("^ 串首", "^", "", false),
            t("$ 串尾", "$", "", false),
            t(r"\b 单词边界", r"\b", "", false),
            t(r"\B 非边界", r"\B", "", false),
            t(r"\A 绝对首", r"\A", "", false),
            t(r"\z 绝对尾", r"\z", "", false),
        ],
    ),
    (
        "量词（有选区时包裹它）",
        &[
            t("* 0+", "*", "", false),
            t("+ 1+", "+", "", false),
            t("? 0/1", "?", "", false),
            t("{2} 恰好", "{2}", "", false),
            t("{2,} 至少", "{2,}", "", false),
            t("{2,5} 区间", "{2,5}", "", false),
            t("*? 懒惰", "*?", "", false),
            t("+? 懒惰", "+?", "", false),
            t("?? 懒惰", "??", "", false),
        ],
    ),
    (
        "组与引用",
        &[
            t("捕获组 (…)", "(", ")", true),
            t("非捕获组 (?:…)", "(?:", ")", true),
            t("命名组 (?<name>…)", "(?<name>", ")", true),
            t(r"反向引用 \1", r"\1", "", false),
        ],
    ),
    (
        "环视",
        &[
            t("前瞻 (?=…)", "(?=", ")", true),
            t("负前瞻 (?!…)", "(?!", ")", true),
            t("后顾 (?<=…)", "(?<=", ")", true),
            t("负后顾 (?<!…)", "(?<!", ")", true),
        ],
    ),
    (
        "不可见字符",
        &[
            t(r"\n 换行", r"\n", "", false),
            t(r"\t 制表", r"\t", "", false),
            t(r"\r 回车", r"\r", "", false),
        ],
    ),
    (
        "标志",
        &[
            t("(?i) 忽略大小写", "(?i)", "", false),
            t("(?s) 单行", "(?s)", "", false),
            t("(?m) 多行", "(?m)", "", false),
            t("(?x) 自由格式", "(?x)", "", false),
        ],
    ),
];

/// 插入器：光标/选区处插入；选区被 before/after 包裹（量词、组都适用）
fn insert_at_cursor(
    src: &mut String,
    sel: Option<egui::text::CCursorRange>,
    before: &str,
    after: &str,
    inside: bool,
) -> usize {
    let chars: Vec<char> = src.chars().collect();
    let (s, e) = sel
        .map(|r| {
            let p = usize::from(r.primary.index);
            let sec = usize::from(r.secondary.index);
            let a = p.min(sec).min(chars.len());
            let b = p.max(sec).min(chars.len());
            (a, b)
        })
        .unwrap_or((chars.len(), chars.len()));
    let mut out = String::with_capacity(src.len() + before.len() + after.len());
    out.extend(&chars[..s]);
    out.push_str(before);
    out.extend(&chars[s..e]);
    out.push_str(after);
    out.extend(&chars[e..]);
    *src = out;
    // 新光标：包裹类停在内容尾部（closing 前）；追加类停在插入物之后
    s + before.chars().count() + (e - s) + if inside { 0 } else { after.chars().count() }
}

/// RegexBuddy Insert Token 式菜单：点选后写回光标（下一帧生效），并触发重解析
fn insert_menu(
    ui: &mut egui::Ui,
    st: &mut State,
    out: &egui::widgets::text_edit::TextEditOutput,
    regex_edited: &mut bool,
) {
    let mut pending: Option<Token> = None;
    ui.menu_button("插入 ▾", |ui| {
        for (cat, items) in TOKEN_MENU {
            ui.menu_button(*cat, |ui| {
                for tok in *items {
                    if ui.button(tok.label).clicked() {
                        pending = Some(*tok);
                    }
                }
            });
        }
    });
    if let Some(tok) = pending {
        let sel = out.state.cursor.char_range();
        let new_cursor = insert_at_cursor(&mut st.regex_src, sel, tok.before, tok.after, tok.inside);
        // show() 内部已 store 过本帧状态——手动改光标必须重存才会在下一帧生效
        let mut state = out.state.clone();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(new_cursor))));
        state.store(ui.ctx(), out.response.id);
        crate::logging::info(&format!(
            "[插入] {} → 光标 {}",
            tok.label, new_cursor
        ));
        *regex_edited = true;
        st.gen += 1;
        st.result = None;
    }
}

/// RegexBuddy History 式菜单：快照点一下整体回滚（正则 + 标志 + 样本）
fn history_menu(ui: &mut egui::Ui, st: &mut State) {
    ui.menu_button(format!("🕘 历史 {}", st.history.len()), |ui| {
        if ui.button("清空").clicked() {
            st.history.clear();
        }
        ui.separator();
        if st.history.is_empty() {
            ui.weak("（暂无——正则/样本稳定后自动记录，最多 30 条）");
        }
        let snapshot: Vec<(String, String, [bool; 4], Vec<String>)> = st
            .history
            .iter()
            .map(|h| (h.time.clone(), h.regex.clone(), h.flags, h.samples.clone()))
            .collect();
        for (time, regex, flags, samples) in snapshot.iter().rev() {
            let label = truncate_one(&regex.replace('\n', "⏎"), 26);
            let tag = format!(
                "[{}{}{}{}]",
                if flags[0] { "i" } else { "" },
                if flags[1] { "s" } else { "" },
                if flags[2] { "m" } else { "" },
                if flags[3] { "x" } else { "" }
            );
            if ui.button(format!("{time} {tag} {label}")).on_hover_text(regex.clone()).clicked() {
                st.regex_src = regex.clone();
                st.flag_i = flags[0];
                st.flag_s = flags[1];
                st.flag_m = flags[2];
                st.flag_x = flags[3];
                st.samples = samples.clone();
                st.gen += 1;
                st.result = None;
                st.galleys.clear();
                st.repl_cache = None;
                reparse(st);
                st.scan_due = true;
                st.last_edit = f64::NEG_INFINITY; // 立即重扫，不等防抖
                crate::logging::info("[历史] 回滚快照");
            }
        }
    });
}

fn truncate_one(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// 组节点 span 集合（人话层树深度：分段被多少个组包裹）
fn collect_group_spans(node: &regex_ir::Node, out: &mut Vec<Span>) {
    match node {
        regex_ir::Node::Group { child, span, .. } => {
            out.push(*span);
            collect_group_spans(child, out);
        }
        regex_ir::Node::Repeat { child, .. } => collect_group_spans(child, out),
        regex_ir::Node::Concat(items) => items.iter().for_each(|n| collect_group_spans(n, out)),
        regex_ir::Node::Alternation(branches) => {
            branches.iter().for_each(|n| collect_group_spans(n, out))
        }
        _ => {}
    }
}

/// 对比用：per_sample 命中铺平成 (样本, 行, 起, 止) 集合
fn span_set(
    per_sample: &[Vec<Vec<(usize, usize)>>],
) -> std::collections::HashSet<(usize, usize, usize, usize)> {
    per_sample
        .iter()
        .enumerate()
        .flat_map(|(si, lines)| {
            lines
                .iter()
                .enumerate()
                .map(move |(li, hits)| hits.iter().map(move |&(s, e)| (si, li, s, e)))
        })
        .flatten()
        .collect()
}
