//! IR → 铁路图布局（设计方案 6.1 布局层）。
//!
//! 零 UI 依赖：产出抽象几何（逻辑坐标，y 向下），由 app 的 egui 画笔与
//! regex-export 的 SVG 后端共同消费——两边看到的是同一份坐标（双后端一致性）。
//!
//! 已落地：主线 / 交替平行轨 / 可选旁路 / 回环 / 圆角连接线（round_polyline）/ 汇入箭头 /
//! {m,n} 徽标贴回环轨。坐标确定性有专门测试（layout_is_deterministic）。
//! 已知边界：嵌套重复不做折叠（深层嵌套图会变宽，设计方案未列入 v1）。

use regex_ir::{AnchorKind, BackrefKind, ClassItem, GroupKind, Node, PerlClass, Span};

// ---- 视觉常量（egui 画笔与 SVG 后端共用，保证两边一致） ----
pub const CHAR_W: f32 = 8.5;
pub const CHAR_W_CJK: f32 = 16.0;
pub const NODE_H: f32 = 30.0;
pub const PAD_X: f32 = 11.0;
/// 顺序节点之间的水平间隙
pub const GAP: f32 = 16.0;
/// 交替/回环上下车道额外间距
pub const LANE_GAP: f32 = 18.0;
/// 交替分叉/汇合竖线留出的水平空间（左右各一份）
pub const FORK_GAP: f32 = 22.0;
/// 旁路/回环车道到子图边缘的距离
pub const BYPASS_DROP: f32 = 22.0;
pub const LOOP_RISE: f32 = 20.0;
pub const MARGIN: f32 = 24.0;
/// 徽标（量词/组名）字号
pub const BADGE_SIZE: f32 = 11.0;
pub const BADGE_PAD: f32 = 9.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStyle {
    Literal,
    Class,
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineStyle {
    /// 主轨道
    Main,
    /// 跳过轨道（`?`/`*` 的直通旁路），虚线
    Bypass,
    /// 回环轨道（`*`/`+` 的重复回路）
    Loop,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Box {
        pos: Pt,
        size: Pt,
        label: String,
        style: NodeStyle,
        /// 对应正则源码的字节区间（悬停联动用；None = 复合节点装饰）
        src: Option<Span>,
    },
    /// 菱形（锚点/断言/标志）
    Diamond {
        pos: Pt,
        size: Pt,
        label: String,
        src: Option<Span>,
    },
    Polyline {
        pts: Vec<Pt>,
        style: LineStyle,
    },
    /// 箭头（ tip 处、指向 +x）——标记"线流入节点"的汇入点
    Arrow { tip: Pt },
    /// 左上角锚定的徽标文本（量词/组名/环视说明）
    Text {
        pos: Pt,
        label: String,
        size: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagram {
    pub width: f32,
    pub height: f32,
    pub shapes: Vec<Shape>,
}

/// 布局中间产物：子图分片。约定 entry/exit 都在 (0|width, main_y)，
/// 且所有分片保持上下对称（main_y == height/2），拼接时中线自动对齐。
struct Fragment {
    width: f32,
    height: f32,
    shapes: Vec<Shape>,
}

impl Fragment {
    fn main_y(&self) -> f32 {
        self.height / 2.0
    }
}

/// 对外入口：IR → 带外边距的完整图
pub fn layout(root: &Node) -> Diagram {
    let frag = layout_node(root);
    let mut shapes = frag.shapes;
    for s in &mut shapes {
        translate_shape(s, MARGIN, MARGIN);
    }
    Diagram {
        width: frag.width + 2.0 * MARGIN,
        height: frag.height + 2.0 * MARGIN,
        shapes,
    }
}

fn translate_shape(s: &mut Shape, dx: f32, dy: f32) {
    match s {
        Shape::Box { pos, .. } | Shape::Diamond { pos, .. } | Shape::Text { pos, .. } => {
            pos.x += dx;
            pos.y += dy;
        }
        Shape::Polyline { pts, .. } => {
            for p in pts {
                p.x += dx;
                p.y += dy;
            }
        }
        Shape::Arrow { tip } => {
            tip.x += dx;
            tip.y += dy;
        }
    }
}

/// 文本宽度估算（ASCII / CJK 两档；M1 接真实字体度量）
fn text_w(label: &str) -> f32 {
    let inner: f32 = label
        .chars()
        .map(|c| if c.is_ascii() { CHAR_W } else { CHAR_W_CJK })
        .sum();
    inner + 2.0 * PAD_X
}

fn label_width(label: &str) -> f32 {
    label
        .chars()
        .map(|c| if c.is_ascii() { CHAR_W } else { CHAR_W_CJK })
        .sum::<f32>()
        .max(14.0)
}

fn solo_box(label: String, style: NodeStyle, src: Option<Span>) -> Fragment {
    let w = text_w(&label).max(34.0);
    Fragment {
        width: w,
        height: NODE_H,
        shapes: vec![Shape::Box {
            pos: Pt::new(0.0, 0.0),
            size: Pt::new(w, NODE_H),
            label,
            style,
            src,
        }],
    }
}

fn anchor_diamond(label: String, src: Option<Span>) -> Fragment {
    let w = label_width(&label) + 18.0;
    Fragment {
        width: w,
        height: NODE_H,
        shapes: vec![Shape::Diamond {
            pos: Pt::new(0.0, 0.0),
            size: Pt::new(w, NODE_H),
            label,
            src,
        }],
    }
}

fn layout_node(node: &Node) -> Fragment {
    match node {
        Node::Empty => Fragment { width: 0.0, height: NODE_H, shapes: vec![] },

        Node::Literal { text, span } => solo_box(text.clone(), NodeStyle::Literal, Some(*span)),

        Node::Class { negated, items, span } => solo_box(class_label(*negated, items), NodeStyle::Class, Some(*span)),

        Node::Anchor { kind, span } => anchor_diamond(anchor_label(*kind), Some(*span)),

        Node::Backref { kind, span } => solo_box(backref_label(kind), NodeStyle::Group, Some(*span)),

        Node::Concat(children) => layout_concat(children),

        Node::Alternation(branches) => layout_alternation(branches),

        Node::Flags { set, unset, span } => {
            let label = if unset.is_empty() {
                format!("(?{set})")
            } else {
                format!("(?{set}-{unset})")
            };
            anchor_diamond(label, Some(*span))
        }

        Node::Group { kind, name, index, flags, child, .. } => {
            layout_group(*kind, name.as_deref(), *index, flags.as_deref(), child)
        }

        Node::Repeat { child, min, max, rep, .. } => layout_repeat(child, *min, *max, *rep),
    }
}

// ---- 顺序拼接：全部子片按中线对齐横向排布，段间以主线相连 ----
fn layout_concat(children: &[Node]) -> Fragment {
    let frags: Vec<Fragment> = children.iter().map(layout_node).collect();
    let max_h = frags.iter().map(|f| f.height).fold(NODE_H, f32::max);
    let cy = max_h / 2.0;

    let mut shapes = Vec::new();
    let mut x = 0.0f32;
    let mut prev_end: Option<Pt> = None;
    for f in &frags {
        let dy = cy - f.main_y();
        let is_empty = f.width <= 0.0;
        let entry = Pt::new(x, cy);
        if let Some(pe) = prev_end {
            if !is_empty || f.width > 0.0 {
                // 主线：上一片出口 → 本片入口；流入非空节点处加箭头
                shapes.push(Shape::Polyline {
                    pts: vec![pe, entry],
                    style: LineStyle::Main,
                });
                if !is_empty {
                    shapes.push(Shape::Arrow { tip: entry });
                }
            }
        }
        let mut child_shapes = f.shapes.clone();
        for s in &mut child_shapes {
            translate_shape(s, x, dy);
        }
        shapes.extend(child_shapes);
        if is_empty {
            prev_end = Some(entry); // 空片直通，出口=入口
        } else {
            prev_end = Some(Pt::new(x + f.width, cy));
            x += f.width + GAP;
        }
    }
    // 收掉最后一个 GAP
    if x > 0.0 {
        x -= GAP;
    }
    Fragment { width: x.max(0.0), height: max_h, shapes }
}

// ---- 交替：fork 竖线 → 平行车道 → merge 竖线 ----
fn layout_alternation(branches: &[Node]) -> Fragment {
    let frags: Vec<Fragment> = branches.iter().map(layout_node).collect();
    if frags.len() == 1 {
        return frags.into_iter().next().unwrap();
    }

    let lane_max_w = frags.iter().map(|f| f.width).fold(0.0f32, f32::max).max(FORK_GAP);
    let total_inner_h: f32 = frags.iter().map(|f| f.height).sum::<f32>() + LANE_GAP * (frags.len() - 1) as f32;
    let width = FORK_GAP + lane_max_w + FORK_GAP;
    let mut shapes = Vec::new();

    // 各车道中心 y（自上而下）
    let mut lane_cy = Vec::with_capacity(frags.len());
    let mut y = 0.0f32;
    for f in &frags {
        lane_cy.push(y + f.height / 2.0);
        y += f.height + LANE_GAP;
    }
    let main_y = total_inner_h / 2.0;

    let fork_x = FORK_GAP;
    let merge_x = FORK_GAP + lane_max_w + FORK_GAP;

    // 入线 + 分叉竖线
    shapes.push(Shape::Polyline {
        pts: vec![Pt::new(0.0, main_y), Pt::new(fork_x, main_y)],
        style: LineStyle::Main,
    });
    shapes.push(Shape::Polyline {
        pts: vec![Pt::new(fork_x, lane_cy[0]), Pt::new(fork_x, *lane_cy.last().unwrap())],
        style: LineStyle::Main,
    });
    // 汇合竖线 + 出线
    shapes.push(Shape::Polyline {
        pts: vec![Pt::new(merge_x, lane_cy[0]), Pt::new(merge_x, *lane_cy.last().unwrap())],
        style: LineStyle::Main,
    });
    shapes.push(Shape::Polyline {
        pts: vec![Pt::new(merge_x, main_y), Pt::new(width, main_y)],
        style: LineStyle::Main,
    });

    for (f, &cy) in frags.iter().zip(&lane_cy) {
        let x0 = FORK_GAP + (lane_max_w - f.width) / 2.0; // 车道内容居中
        shapes.push(Shape::Polyline {
            pts: vec![Pt::new(fork_x, cy), Pt::new(x0, cy)],
            style: LineStyle::Main,
        });
        if f.width > 0.0 {
            shapes.push(Shape::Arrow { tip: Pt::new(x0, cy) });
        }
        let mut child_shapes = f.shapes.clone();
        for s in &mut child_shapes {
            translate_shape(s, x0, cy - f.main_y());
        }
        shapes.extend(child_shapes);
        let x1 = x0 + f.width;
        shapes.push(Shape::Polyline {
            pts: vec![Pt::new(x1, cy), Pt::new(merge_x, cy)],
            style: LineStyle::Main,
        });
    }

    Fragment { width, height: total_inner_h, shapes }
}

// ---- 分组：子图 + 上方徽标（组号/组名/环视说明/作用域标志），上下对称加 pad ----
fn layout_group(kind: GroupKind, name: Option<&str>, index: Option<u32>, flags: Option<&str>, child: &Node) -> Fragment {
    let inner = layout_node(child);
    let badge = group_badge(kind, name, index, flags);
    let pad = if badge.is_some() { 2.0 * BADGE_PAD } else { 0.0 };
    let height = inner.height + pad;
    let width = inner.width;

    let mut shapes = inner.shapes;
    for s in &mut shapes {
        translate_shape(s, 0.0, pad / 2.0);
    }
    if let Some(label) = badge {
        shapes.push(Shape::Text { pos: Pt::new(0.0, 2.0), label, size: BADGE_SIZE });
    }
    Fragment { width, height, shapes }
}

fn group_badge(kind: GroupKind, name: Option<&str>, index: Option<u32>, flags: Option<&str>) -> Option<String> {
    match kind {
        GroupKind::Capture => index.map(|i| format!("«{i}»")),
        GroupKind::NonCapturing => flags.map(|f| format!("(?{f})")),
        GroupKind::Named => Some(match name {
            Some(n) => format!("«{n}»"),
            None => "«?»".into(),
        }),
        GroupKind::Lookahead { negative: false } => Some("前瞻 (?=)".into()),
        GroupKind::Lookahead { negative: true } => Some("负前瞻 (?!)".into()),
        GroupKind::Lookbehind { negative: false } => Some("后顾 (?<=".into()),
        GroupKind::Lookbehind { negative: true } => Some("负后顾 (?<!)".into()),
        GroupKind::Atomic => Some("原子 (?>)".into()),
    }
}

fn backref_label(kind: &BackrefKind) -> String {
    match kind {
        BackrefKind::Index(i) => format!("\\{i}"),
        BackrefKind::Name(n) => format!("\\k<{n}>"),
    }
}

// ---- 重复：子图 + 上回环轨 / 下旁路轨 + 量词徽标 ----
fn layout_repeat(child: &Node, min: u32, max: Option<u32>, rep: regex_ir::RepKind) -> Fragment {
    let inner = layout_node(child);
    let bypass = min == 0;                                   // `?` / `*` 可直通
    let loop_back = max.map(|m| m > min).unwrap_or(true);    // `*` / `+` / {m,n} 可重复
    let badge = regex_ir::Node::quant_label(min, max, rep);

    let top_pad = if loop_back { LOOP_RISE + 6.0 } else { 4.0 };
    let bot_pad = if bypass { BYPASS_DROP + 6.0 } else { 4.0 } + 14.0; // +14 给徽标
    let height = top_pad + inner.height + bot_pad;
    let width = inner.width.max(FORK_GAP); // 给回环横线一点最小宽度
    let main_y = height / 2.0;

    let mut shapes = inner.shapes;
    for s in &mut shapes {
        // 子图水平居中
        translate_shape(s, (width - inner.width) / 2.0, top_pad);
    }

    let x0 = (width - inner.width) / 2.0;
    let x1 = x0 + inner.width;

    if loop_back {
        let loop_y = top_pad / 2.0;
        shapes.push(Shape::Polyline {
            pts: round_polyline(
                &[
                    Pt::new(x1, main_y),
                    Pt::new(x1, loop_y),
                    Pt::new(x0, loop_y),
                    Pt::new(x0, main_y),
                ],
                10.0,
            ),
            style: LineStyle::Loop,
        });
        // 回流再入子图：箭头（正式回环语义：走一遍回到起点再来）
        shapes.push(Shape::Arrow { tip: Pt::new(x0, main_y) });
    }
    if bypass {
        let by_y = top_pad + inner.height + BYPASS_DROP / 2.0;
        shapes.push(Shape::Polyline {
            pts: round_polyline(
                &[
                    Pt::new(x0, main_y),
                    Pt::new(x0, by_y),
                    Pt::new(x1, by_y),
                    Pt::new(x1, main_y),
                ],
                10.0,
            ),
            style: LineStyle::Bypass,
        });
    }
    // 量词徽标：有回环 → 贴回环轨上缘（{m,n} 次数标注正式化）；仅旁路 → 子图下方
    let bw = label_width(&badge);
    let by = if loop_back { 1.0 } else { top_pad + inner.height + 4.0 };
    shapes.push(Shape::Text { pos: Pt::new((width - bw) / 2.0, by), label: badge, size: BADGE_SIZE });

    Fragment { width, height, shapes }
}

/// 折线圆角化：每个拐角用二次贝塞尔近似替换（双后端共用同一几何，坐标一致性不破坏）
fn round_polyline(pts: &[Pt], r: f32) -> Vec<Pt> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let dist = |a: Pt, b: Pt| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    let mut out = vec![pts[0]];
    for w in pts.windows(3) {
        let (p0, c, p1) = (w[0], w[1], w[2]);
        let (l1, l2) = (dist(p0, c), dist(c, p1));
        if l1 < 1e-3 || l2 < 1e-3 {
            out.push(c);
            continue;
        }
        let rr = r.min(l1 * 0.5).min(l2 * 0.5);
        let a = Pt::new(c.x + (p0.x - c.x) / l1 * rr, c.y + (p0.y - c.y) / l1 * rr);
        let b = Pt::new(c.x + (p1.x - c.x) / l2 * rr, c.y + (p1.y - c.y) / l2 * rr);
        const N: usize = 5;
        for i in 0..=N {
            let t = i as f32 / N as f32;
            let mt = 1.0 - t;
            out.push(Pt::new(
                mt * mt * a.x + 2.0 * mt * t * c.x + t * t * b.x,
                mt * mt * a.y + 2.0 * mt * t * c.y + t * t * b.y,
            ));
        }
    }
    out.push(*pts.last().unwrap());
    out
}

fn anchor_label(kind: AnchorKind) -> String {
    match kind {
        AnchorKind::StartLine => "^".into(),
        AnchorKind::EndLine => "$".into(),
        AnchorKind::StartText => "\\A".into(),
        AnchorKind::EndText => "\\z".into(),
        AnchorKind::WordBoundary => "\\b".into(),
        AnchorKind::NotWordBoundary => "\\B".into(),
    }
}

fn class_label(negated: bool, items: &[ClassItem]) -> String {
    // `.` 的 IR 表示：negated + [Char('\n')]
    if negated && items.len() == 1 && matches!(items[0], ClassItem::Char('\n')) {
        return "任意字符".into();
    }
    let mut s = String::from("[");
    if negated {
        s.push('^');
    }
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        match item {
            ClassItem::Char(c) => {
                if matches!(c, ']' | '\\' | '^' | '-') {
                    s.push('\\');
                }
                s.push(*c);
            }
            ClassItem::Range(a, b) => {
                s.push(*a);
                s.push('-');
                s.push(*b);
            }
            ClassItem::Perl(p) => s.push_str(perl_label(p)),
            ClassItem::Posix { name, negated } => {
                s.push_str(if *negated { "[:^" } else { "[:" });
                s.push_str(name);
                s.push_str(":]");
            }
            ClassItem::Unicode(name) => {
                s.push_str("\\p{");
                s.push_str(name);
                s.push('}');
            }
        }
    }
    s.push(']');
    s
}

fn perl_label(p: &PerlClass) -> &'static str {
    match p {
        PerlClass::Digit => "\\d",
        PerlClass::NotDigit => "\\D",
        PerlClass::Word => "\\w",
        PerlClass::NotWord => "\\W",
        PerlClass::Space => "\\s",
        PerlClass::NotSpace => "\\S",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex_parser::parse;

    fn diag(src: &str) -> Diagram {
        layout(&parse(src).unwrap())
    }

    #[test]
    fn simple_literal_diagram() {
        let d = diag("abc");
        assert!(d.width > 30.0 && d.height > 30.0);
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Box { label, .. } if label == "abc")));
    }

    #[test]
    fn alternation_makes_lanes_and_spines() {
        let d = diag("a|b");
        let boxes = d.shapes.iter().filter(|s| matches!(s, Shape::Box { .. })).count();
        let lines = d.shapes.iter().filter(|s| matches!(s, Shape::Polyline { .. })).count();
        assert_eq!(boxes, 2, "两条车道各一个字面量框");
        assert!(lines >= 6, "入线+出线+两条叉线+分叉/汇合竖线，实际 {lines}");
    }

    #[test]
    fn optional_makes_bypass_lane() {
        let d = diag("ab?");
        assert!(d
            .shapes
            .iter()
            .any(|s| matches!(s, Shape::Polyline { style: LineStyle::Bypass, .. })));
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Text { label, .. } if label == "?")));
    }

    #[test]
    fn star_makes_bypass_and_loop() {
        let d = diag("a*");
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Polyline { style: LineStyle::Bypass, .. })));
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Polyline { style: LineStyle::Loop, .. })));
    }

    #[test]
    fn anchors_are_diamonds() {
        let d = diag(r"^ab$");
        assert_eq!(d.shapes.iter().filter(|s| matches!(s, Shape::Diamond { .. })).count(), 2);
    }

    #[test]
    fn named_group_badge() {
        let d = diag(r"(?<year>\d{4})");
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Text { label, .. } if label.contains("year"))));
    }

    #[test]
    fn capture_index_badge() {
        // 第 2 个捕获组显示 «2»
        let d = diag("(a)(b)");
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Text { label, .. } if label == "«1»")));
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Text { label, .. } if label == "«2»")));
    }

    #[test]
    fn flags_diamond_and_scoped_badge() {
        let d = diag("(?i)ab");
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Diamond { label, .. } if label == "(?i)")));
        let d = diag("(?im-s:x)");
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Text { label, .. } if label == "(?im-s)")));
    }

    #[test]
    fn posix_and_named_backref_labels() {
        let d = diag("[[:digit:]-]");
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Box { label, .. } if label.contains("[:digit:]"))));
        let d = diag(r"\k<year>");
        assert!(d.shapes.iter().any(|s| matches!(s, Shape::Box { label, .. } if label == "\\k<year>")));
    }

    #[test]
    fn shapes_carry_source_spans() {
        let d = diag(r"\b(\d{4})-\d{2}\b");
        let spans: Vec<_> = d
            .shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Box { src, .. } | Shape::Diamond { src, .. } => *src,
                _ => None,
            })
            .collect();
        assert!(spans.len() >= 4, "至少 4 个带源码 span 的节点，实际 {}", spans.len());
        for sp in &spans {
            assert!(sp.end > sp.start, "span 应非空：{sp:?}");
        }
    }

    #[test]
    fn loop_track_is_rounded() {
        let d = diag("a*");
        let loops: Vec<_> = d
            .shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Polyline { pts, style: LineStyle::Loop } => Some(pts),
                _ => None,
            })
            .collect();
        assert_eq!(loops.len(), 1);
        assert!(loops[0].len() > 8, "圆角化后应远多于 4 个原始顶点，实际 {}", loops[0].len());
    }

    #[test]
    fn entry_arrows_on_concat() {
        // "abc" 会被字面量合并成单节点（无连接线）；用 a.c 强制三段
        let d = diag("a.c");
        assert_eq!(d.shapes.iter().filter(|s| matches!(s, Shape::Arrow { .. })).count(), 2);
    }

    #[test]
    fn loop_reentry_arrow() {
        let d = diag("a*");
        assert_eq!(d.shapes.iter().filter(|s| matches!(s, Shape::Arrow { .. })).count(), 1);
    }

    #[test]
    fn alternation_lane_arrows() {
        let d = diag("a|b");
        assert_eq!(d.shapes.iter().filter(|s| matches!(s, Shape::Arrow { .. })).count(), 2);
    }

    #[test]
    fn repeat_badge_rides_loop_lane() {
        // {m,n} 正式化：徽标贴回环轨上缘（frag 内 y < 回环线 y=13；外层 layout() 已加 MARGIN=24）
        let d = diag("a{2,4}");
        let badge = d
            .shapes
            .iter()
            .find_map(|s| match s {
                Shape::Text { pos, label, .. } if label.contains("{2,4}") => Some(*pos),
                _ => None,
            })
            .expect("徽标存在");
        assert!(badge.y - MARGIN < LOOP_RISE / 2.0, "徽标应在回环轨上缘，实际 frag 内 y={}", badge.y - MARGIN);
    }

    #[test]
    fn layout_is_deterministic() {
        assert_eq!(diag("a(b|c)?d"), diag("a(b|c)?d"));
    }
}
