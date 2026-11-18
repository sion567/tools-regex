//! 双后端之 SVG（设计方案 6.1 呈现层）+ 多语言代码导出（M4，见 codegen 模块）。
//!
//! 与 app 的 egui 画笔消费同一份 `Diagram`，坐标语义完全一致——
//! 屏幕上看到什么，导出的 SVG 就是同一张图（spike 3 的一致性验收依据）。

pub mod codegen;

use regex_diagram::{Diagram, LineStyle, NodeStyle, Shape};

/// 颜色/描边常量（与 app 画笔侧保持一致，集中在这里供两边引用对齐）
pub mod palette {
    pub const STROKE: &str = "#495057";
    pub const TEXT: &str = "#212529";
    pub const FILL_LITERAL: &str = "#ffffff";
    pub const FILL_CLASS: &str = "#e7f1ff";
    pub const FILL_GROUP: &str = "#e9f7ef";
    pub const FILL_DIAMOND: &str = "#fff3cd";
    pub const BYPASS_STROKE: &str = "#868e96";
}

pub fn diagram_to_svg(d: &Diagram) -> String {
    let mut s = String::with_capacity(4096);
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\" font-family=\"Segoe UI, Microsoft YaHei, sans-serif\">\n",
        w = d.width.max(1.0),
        h = d.height.max(1.0),
    ));
    s.push_str(&format!(
        "<rect x=\"0\" y=\"0\" width=\"{w:.0}\" height=\"{h:.0}\" fill=\"#fcfcfd\"/>\n",
        w = d.width.max(1.0),
        h = d.height.max(1.0),
    ));
    // 先画线再画框，避免线压住框
    let (mut lines, mut nodes) = (String::new(), String::new());
    for shape in &d.shapes {
        match shape {
            Shape::Polyline { pts, style } => {
                let mut path = String::new();
                for (i, p) in pts.iter().enumerate() {
                    if i == 0 {
                        path.push_str(&format!("M {x:.1} {y:.1}", x = p.x, y = p.y));
                    } else {
                        path.push_str(&format!(" L {x:.1} {y:.1}", x = p.x, y = p.y));
                    }
                }
                let (stroke, dash) = match style {
                    LineStyle::Main => (palette::STROKE, ""),
                    LineStyle::Bypass => (palette::BYPASS_STROKE, " stroke-dasharray=\"7 5\""),
                    LineStyle::Loop => (palette::BYPASS_STROKE, " stroke-dasharray=\"2 4\""),
                };
                lines.push_str(&format!(
                    "<path d=\"{path}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.6\"{dash} stroke-linejoin=\"round\"/>\n"
                ));
            }
            Shape::Box { pos, size, label, style, .. } => {
                let fill = match style {
                    NodeStyle::Literal => palette::FILL_LITERAL,
                    NodeStyle::Class => palette::FILL_CLASS,
                    NodeStyle::Group => palette::FILL_GROUP,
                };
                nodes.push_str(&format!(
                    "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" rx=\"7\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.6\"/>\n",
                    x = pos.x,
                    y = pos.y,
                    w = size.x,
                    h = size.y,
                    stroke = palette::STROKE,
                ));
                nodes.push_str(&format!(
                    "<text x=\"{cx:.1}\" y=\"{cy:.1}\" text-anchor=\"middle\" dominant-baseline=\"central\" font-size=\"14\" fill=\"{c}\">{t}</text>\n",
                    cx = pos.x + size.x / 2.0,
                    cy = pos.y + size.y / 2.0,
                    c = palette::TEXT,
                    t = esc(label),
                ));
            }
            Shape::Diamond { pos, size, label, .. } => {
                let (cx, cy) = (pos.x + size.x / 2.0, pos.y + size.y / 2.0);
                nodes.push_str(&format!(
                    "<polygon points=\"{cx:.1},{t:.1} {r:.1},{cy:.1} {cx:.1},{b:.1} {l:.1},{cy:.1}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.6\"/>\n",
                    t = cy - size.y / 2.0,
                    b = cy + size.y / 2.0,
                    l = cx - size.x / 2.0,
                    r = cx + size.x / 2.0,
                    fill = palette::FILL_DIAMOND,
                    stroke = palette::STROKE,
                ));
                nodes.push_str(&format!(
                    "<text x=\"{cx:.1}\" y=\"{cy:.1}\" text-anchor=\"middle\" dominant-baseline=\"central\" font-size=\"13\" fill=\"{c}\">{t}</text>\n",
                    c = palette::TEXT,
                    t = esc(label),
                ));
            }
            Shape::Arrow { tip } => {
                // 实心三角：指向 +x
                nodes.push_str(&format!(
                    "<polygon points=\"{x:.1},{y:.1} {bx:.1},{t:.1} {bx:.1},{b:.1}\" fill=\"{c}\"/>\n",
                    x = tip.x,
                    y = tip.y,
                    bx = tip.x - 7.0,
                    t = tip.y - 3.5,
                    b = tip.y + 3.5,
                    c = palette::STROKE,
                ));
            }
            Shape::Text { pos, label, size } => {
                nodes.push_str(&format!(
                    "<text x=\"{x:.1}\" y=\"{y:.1}\" font-size=\"{fs:.0}\" fill=\"#6c757d\">{t}</text>\n",
                    x = pos.x,
                    y = pos.y + size,
                    fs = size,
                    t = esc(label),
                ));
            }
        }
    }
    s.push_str(&lines);
    s.push_str(&nodes);
    s.push_str("</svg>\n");
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex_diagram::layout;
    use regex_parser::parse;

    #[test]
    fn svg_contains_nodes_and_size() {
        let svg = diagram_to_svg(&layout(&parse("ab?").unwrap()));
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<rect"));
        assert!(svg.contains(">a</text>") || svg.contains(">ab</text>"));
        assert!(svg.contains("dasharray")); // 旁路虚线
        assert!(svg.contains("<polygon")); // 汇入箭头
    }

    #[test]
    fn escapes_special_chars() {
        let svg = diagram_to_svg(&layout(&parse("a&b<c").unwrap()));
        assert!(svg.contains("a&amp;b&lt;c"));
        assert!(!svg.contains("a&b<c"));
    }

    #[test]
    fn same_diagram_same_svg() {
        let d = layout(&parse("a(b|c)?d").unwrap());
        assert_eq!(diagram_to_svg(&d), diagram_to_svg(&d));
    }
}
