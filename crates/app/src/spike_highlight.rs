//! Spike 2：富文本高亮 —— 正则输入框语法着色 + 大文本命中高亮 + 万行滚动性能。
//!
//! 验收点：
//!   a) 正则输入框里 `\d`、量词、分组、`|` 分色（LayoutJob 手拼，验证这条路可行）
//!   b) 样本文本命中段有背景色；换正则 300ms 防抖后全量重扫
//!   c) 1万 / 10万行滚动不卡（每行 galley 按需构建 + 缓存，只画可视区）
//!   d) 引擎矩阵可见：环视/反向引用时自动落到 fancy-regex 并标注「近似」

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use egui::{Color32, FontId, Pos2, TextFormat};

const FG: Color32 = Color32::from_rgb(216, 220, 226);
const FG_DIM: Color32 = Color32::from_rgb(139, 148, 158);
/// 命中段背景（非 const fn，内联用）
#[allow(non_snake_case)]
#[inline(always)]
fn HIT_BG() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 213, 79, 110)
}

pub struct State {
    regex_src: String,
    applied_src: String,
    last_edit: f64,
    /// 每行的命中区间（字节）
    matches_per_line: Vec<Vec<(usize, usize)>>,
    engine_label: Option<String>,
    approx: bool,
    compile_error: Option<String>,
    total_hits: usize,
    last_scan_ms: f32,
    lines: Vec<String>,
    /// galley 缓存：行号 → (代次, galley)；文本或命中变化时代次+1 全部失效
    gen: u64,
    galleys: HashMap<usize, (u64, Arc<egui::Galley>)>,
    row_h: f32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            regex_src: r"\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}".to_string(),
            applied_src: String::new(),
            last_edit: f64::NEG_INFINITY,
            matches_per_line: Vec::new(),
            engine_label: None,
            approx: false,
            compile_error: None,
            total_hits: 0,
            last_scan_ms: 0.0,
            lines: gen_lines(10_000, 42),
            gen: 0,
            galleys: HashMap::new(),
            row_h: 20.0,
        }
    }
}

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    ui.heading("Spike 2｜匹配高亮 + 大文本滚动（引擎矩阵可见）");
    ui.add_space(4.0);

    // ---- 正则输入（语法着色，验证 LayoutJob 路线） ----
    let ctx = ui.ctx().clone();
    let mut layouter = move |_ui: &egui::Ui, s: &str, _wrap: f32| -> Arc<egui::Galley> {
        let job = colorize_regex(s);
        ctx.fonts(|f| f.layout_job(job))
    };
    let edit = ui.add(
        egui::TextEdit::singleline(&mut st.regex_src)
            .hint_text(r"输入正则，如 \d+\.\d+ 或 ERROR|WARN 或 (?=x)")
            .desired_width(420.0)
            .layouter(&mut layouter),
    );
    if edit.changed() {
        st.last_edit = ui.input(|i| i.time);
    }

    // 300ms 防抖后应用（M1 挪后台线程；M0 故意留在线程内测耗时）
    let now = ui.input(|i| i.time);
    if st.regex_src != st.applied_src && now - st.last_edit > 0.3 {
        apply(st, ui.ctx());
    }

    // ---- 状态行：引擎 / 命中数 / 耗时 / 错误 ----
    ui.horizontal(|ui| {
        match (&st.engine_label, &st.compile_error) {
            (Some(label), None) => {
                let rich = if st.approx {
                    egui::RichText::new(format!("引擎：{label}")).color(Color32::from_rgb(255, 193, 7))
                } else {
                    egui::RichText::new(format!("引擎：{label}")).color(Color32::from_rgb(134, 239, 172))
                };
                ui.label(rich);
                ui.label(format!("命中 {} 处", st.total_hits));
                ui.weak(format!("重扫 {:.1} ms（{} 行，UI 线程同步）", st.last_scan_ms, st.lines.len()));
            }
            (_, Some(err)) => {
                ui.label(egui::RichText::new("两个引擎都编译失败：").color(Color32::from_rgb(255, 128, 128)));
                ui.monospace(err.lines().next().unwrap_or("")).on_hover_text(err.clone());
                // hover 才看全量报错
            }
            (None, None) => {
                ui.weak("尚未应用");
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label("样本量：");
        for (n, seed) in [(1_000usize, 7u64), (10_000, 42), (100_000, 99)] {
            if ui.small_button(fmt_thousands(n)).clicked() {
                st.lines = gen_lines(n, seed);
                apply(st, ui.ctx());
            }
        }
        if ui.small_button("换一批").clicked() {
            let seed = st.lines.len() as u64 + st.gen;
            st.lines = gen_lines(st.lines.len(), seed);
            apply(st, ui.ctx());
        }
    });

    ui.add_space(4.0);
    ui.separator();

    // ---- 样本文本：可视区才建 galley / 绘制 ----
    let n = st.lines.len();
    let total_h = n as f32 * st.row_h;
    let paint_ctx = ui.ctx().clone();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, vp| {
            ui.set_min_height(total_h.max(vp.height()));
            let origin = ui.min_rect().min;
            let first = ((vp.min.y - origin.y) / st.row_h).floor().max(0.0) as usize;
            let last = (((vp.max.y - origin.y) / st.row_h).ceil() as usize).min(n);
            let painter = ui.painter();
            for i in first..last {
                let galley = galley_for(st, &paint_ctx, i);
                let pos = Pos2::new(origin.x + 8.0, origin.y + i as f32 * st.row_h);
                painter.galley(pos, galley, FG);
            }
        });
}

/// 应用当前正则：全量重扫 + 失效 galley 缓存
fn apply(st: &mut State, ctx: &egui::Context) {
    let t0 = Instant::now();
    st.applied_src = st.regex_src.clone();
    let scanner = regex_exec::Scanner::new(&st.regex_src);
    match scanner.engine() {
        Some(e) => {
            st.engine_label = Some(e.label().to_string());
            st.approx = e.approximate();
            st.compile_error = None;
            st.matches_per_line = st.lines.iter().map(|l| scanner.find_in(l)).collect();
            st.total_hits = st.matches_per_line.iter().map(|m| m.len()).sum();
        }
        None => {
            st.engine_label = None;
            st.approx = false;
            st.compile_error = scanner.error().map(str::to_string);
            st.matches_per_line = vec![Vec::new(); st.lines.len()];
            st.total_hits = 0;
        }
    }
    st.gen += 1;
    st.galleys.clear();
    st.last_scan_ms = t0.elapsed().as_secs_f32() * 1000.0;

    // 用第一行探一次行高
    let job = line_job(&st.lines[0], &[]);
    let g = ctx.fonts(|f| f.layout_job(job));
    st.row_h = (g.size().y + 4.0).max(16.0);
}

/// 行 galley：带代次的按需缓存（只缓存可视行）
fn galley_for(st: &mut State, ctx: &egui::Context, i: usize) -> Arc<egui::Galley> {
    if let Some((gen, g)) = st.galleys.get(&i) {
        if *gen == st.gen {
            return g.clone();
        }
    }
    let job = line_job(&st.lines[i], &st.matches_per_line[i]);
    let g = ctx.fonts(|f| f.layout_job(job));
    st.galleys.insert(i, (st.gen, g.clone()));
    g
}

fn line_job(line: &str, hits: &[(usize, usize)]) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping::no_max_width(), // 不换行，超宽裁剪
        ..Default::default()
    };
    let mono = || FontId::monospace(14.0);
    let fmt_plain = TextFormat { font_id: mono(), color: FG, ..Default::default() };
    let fmt_hit = TextFormat {
        font_id: mono(),
        color: FG,
        background: HIT_BG(),
        ..Default::default()
    };
    let mut cursor = 0usize;
    for &(s, e) in hits {
        if s > cursor {
            job.append(&line[cursor..s], 0.0, fmt_plain.clone());
        }
        job.append(&line[s..e], 0.0, fmt_hit.clone());
        cursor = e;
    }
    if cursor < line.len() {
        job.append(&line[cursor..], 0.0, fmt_plain.clone());
    }
    job
}

/// 正则输入框的简单语法着色（状态机逐字符分类；M1 换成基于 parser 的 span 着色）
pub fn colorize_regex(src: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping::no_max_width(),
        ..Default::default()
    };
    let font = || FontId::monospace(15.0);

    let c_default = TextFormat { font_id: font(), color: FG, ..Default::default() };
    let c_class = TextFormat { font_id: font(), color: Color32::from_rgb(97, 200, 255), ..Default::default() };
    let c_quant = TextFormat { font_id: font(), color: Color32::from_rgb(255, 170, 60), ..Default::default() };
    let c_group = TextFormat { font_id: font(), color: Color32::from_rgb(255, 222, 100), ..Default::default() };
    let c_alt = TextFormat { font_id: font(), color: Color32::from_rgb(255, 120, 120), ..Default::default() };
    let c_anchor = TextFormat { font_id: font(), color: Color32::from_rgb(200, 150, 255), ..Default::default() };
    let c_esc_lit = TextFormat { font_id: font(), color: FG_DIM, ..Default::default() };

    let mut in_class = false;
    let mut escaped = false;
    for c in src.chars() {
        let fmt = if escaped {
            escaped = false;
            match c {
                'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'p' | 'P' => &c_class,
                'b' | 'B' | 'A' | 'z' | 'Z' => &c_anchor,
                _ => &c_esc_lit,
            }
        } else if c == '\\' {
            escaped = true;
            &c_class
        } else if c == '[' {
            in_class = true;
            &c_class
        } else if c == ']' {
            in_class = false;
            &c_class
        } else if in_class {
            &c_class
        } else {
            match c {
                '*' | '+' | '?' | '{' | '}' => &c_quant,
                '(' | ')' => &c_group,
                '|' => &c_alt,
                '^' | '$' | '.' => &c_anchor,
                _ => &c_default,
            }
        };
        job.append(&c.to_string(), 0.0, fmt.clone());
    }
    job
}

fn fmt_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 合成日志样本（LCG 伪随机，零依赖）：中英混排 + IP + 等级 + 订单号
fn gen_lines(n: usize, seed: u64) -> Vec<String> {
    let mut next = {
        let mut x = seed | 1;
        move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        }
    };
    let levels = ["INFO", "INFO", "INFO", "WARN", "WARN", "ERROR"];
    let users = ["张伟", "李娜", "王芳", "刘强", "陈静", "赵磊"];
    let actions = ["处理订单", "库存同步", "支付回调", "风控校验", "消息推送", "对账拉取"];
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        let r = next();
        let level = levels[(r % 6) as usize];
        let user = users[((r >> 3) % 6) as usize];
        let action = actions[((r >> 6) % 6) as usize];
        let ip = format!(
            "{}.{}.{}.{}",
            10 + (r % 200),
            (r >> 8) % 256,
            (r >> 16) % 256,
            ((r >> 24) % 254) + 1
        );
        let ms = (r % 900) + 12;
        let line = format!(
            "2026-11-17 {:02}:{:02}:{:02}.{:03} {level:<5} [worker-{}] {action} order=ORD-2026-{:06} user={user} ip={ip} {ms}ms{}",
            r % 24,
            (r >> 5) % 60,
            (r >> 11) % 60,
            r % 1000,
            (r % 8) + 1,
            r % 1_000_000,
            if r % 37 == 0 { " ⚠重试" } else { "" },
        );
        lines.push(line);
    }
    lines
}
