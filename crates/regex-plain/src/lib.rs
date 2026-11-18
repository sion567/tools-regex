//! 人话层：正则 → 中文分段解释（设计方案第三章"人话层"，M2 必做核心）。
//!
//! 零 UI 依赖：产出带源码 span 的分段文本，供 UI 做"人话 ↔ 正则文本 ↔ 铁路图"三层悬停联动。
//! 每段 span 指向正则源码的字节区间（解析器保证 char 边界对齐），UI 用它切片高亮。
//!
//! 两个出口：
//! - `segments(node)`：逐段解释，阅读顺序 = 正则书写顺序（DFS 前序）
//! - `summary(node)`：顶层一句话流程（"词边界 → 数字×4 → 「-」 → …"）

use regex_ir::{AnchorKind, BackrefKind, ClassItem, GroupKind, Node, PerlClass, RepKind, Span};

/// 一段解释：对应正则源码 [span.start..span.end) 的一段内容
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub span: Span,
    pub text: String,
}

/// 逐段解释。顺序 = 正则书写顺序；每段 span 可用于 UI 高亮联动。
pub fn segments(node: &Node) -> Vec<Segment> {
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

/// 顶层一句话流程摘要（"词边界 → 数字×4 → 「-」 → …"）
pub fn summary(node: &Node) -> String {
    let parts: Vec<String> = match node {
        Node::Concat(children) => children.iter().map(short).collect(),
        Node::Alternation(branches) => branches.iter().map(short).collect(),
        _ => vec![short(node)],
    };
    parts.join(" → ")
}

fn walk(node: &Node, out: &mut Vec<Segment>) {
    match node {
        Node::Empty => {}

        Node::Literal { text, span } => out.push(Segment {
            span: *span,
            text: format!("字面文本「{text}」"),
        }),

        Node::Class { negated, items, span } => out.push(Segment {
            span: *span,
            text: class_text(*negated, items),
        }),

        Node::Anchor { kind, span } => out.push(Segment {
            span: *span,
            text: anchor_text(*kind).to_string(),
        }),

        Node::Flags { set, unset, span } => out.push(Segment {
            span: *span,
            text: flags_text(set, unset),
        }),

        Node::Backref { kind, span } => out.push(Segment {
            span: *span,
            text: match kind {
                BackrefKind::Index(i) => format!("反向引用第 {i} 组实际抓到的内容"),
                BackrefKind::Name(n) => format!("反向引用命名组「{n}」实际抓到的内容"),
            },
        }),

        Node::Group { kind, name, index, child, span, .. } => {
            // 环视/原子组有独立的"包"语义，先给一句说明（span 覆盖整个组）
            let intro = match kind {
                GroupKind::Lookahead { negative: false } => {
                    Some("前瞻：要求右侧匹配下面这些内容（零宽，不消费字符）".to_string())
                }
                GroupKind::Lookahead { negative: true } => {
                    Some("负前瞻：要求右侧**不是**下面这些内容（零宽，不消费字符）".to_string())
                }
                GroupKind::Lookbehind { negative: false } => {
                    Some("后顾：要求左侧匹配下面这些内容（零宽，不消费字符）".to_string())
                }
                GroupKind::Lookbehind { negative: true } => {
                    Some("负后顾：要求左侧**不是**下面这些内容（零宽，不消费字符）".to_string())
                }
                GroupKind::Atomic => {
                    Some("原子组：内部匹配成功后不交还字符（防回溯重试）".to_string())
                }
                GroupKind::Capture => index.map(|i| format!("捕获第 {i} 组：")),
                GroupKind::Named => Some(match name.as_deref() {
                    Some(n) => format!("捕获命名组「{n}」："),
                    None => "捕获命名组：".to_string(),
                }),
                GroupKind::NonCapturing => None, // 纯分组不说话，子内容自己说
            };
            if let Some(t) = intro {
                out.push(Segment { span: *span, text: t });
            }
            walk(child, out);
        }

        Node::Repeat { child, min, max, rep, span } => {
            walk(child, out);
            out.push(Segment {
                span: *span,
                text: repeat_text(*min, *max, *rep),
            });
        }

        Node::Concat(children) => {
            for c in children {
                walk(c, out);
            }
        }

        Node::Alternation(branches) => {
            out.push(Segment {
                span: Span::new(0, 0), // 交替整体没有独立 span（M3 可补），此处用空 span 标记
                text: format!("以下内容二选一/多选一（共 {} 支）：", branches.len()),
            });
            for b in branches {
                walk(b, out);
            }
        }
    }
}

// ---- 各类节点的文案 ----

fn repeat_text(min: u32, max: Option<u32>, rep: RepKind) -> String {
    let rep_desc = match rep {
        RepKind::Greedy => "贪婪：尽量多拿",
        RepKind::Lazy => "懒惰：尽量少拿，够就走",
        RepKind::Possessive => "占有：拿到绝不交还",
    };
    let base = match (min, max) {
        (0, Some(1)) => "出现 0 或 1 次".to_string(),
        (0, None) => "重复任意次（包括 0 次）".to_string(),
        (1, None) => "重复至少 1 次".to_string(),
        (m, Some(n)) if m == n => format!("重复 {m} 次"),
        (m, None) => format!("重复至少 {m} 次"),
        (m, Some(n)) => format!("重复 {m} 到 {n} 次"),
    };
    format!("{base}（{rep_desc}）")
}

fn anchor_text(kind: AnchorKind) -> &'static str {
    match kind {
        AnchorKind::StartLine => "行首锚点 ^（零宽，不消费字符）",
        AnchorKind::EndLine => "行尾锚点 $（零宽，不消费字符）",
        AnchorKind::StartText => "整段文本开头 \\A（零宽）",
        AnchorKind::EndText => "整段文本末尾 \\z（零宽）",
        AnchorKind::WordBoundary => "词边界 \\b：一侧是单词字符、另一侧不是（零宽）",
        AnchorKind::NotWordBoundary => "非词边界 \\B（零宽）",
    }
}

fn perl_text(p: PerlClass) -> &'static str {
    match p {
        PerlClass::Digit => "数字（0-9）",
        PerlClass::NotDigit => "非数字",
        PerlClass::Word => "单词字符（字母/数字/下划线）",
        PerlClass::NotWord => "非单词字符",
        PerlClass::Space => "空白字符",
        PerlClass::NotSpace => "非空白字符",
    }
}

fn item_text(item: &ClassItem) -> String {
    match item {
        ClassItem::Char(c) => match c {
            '\n' => "换行符".into(),
            '\r' => "回车符".into(),
            '\t' => "制表符".into(),
            ' ' => "空格".into(),
            c => c.to_string(),
        },
        ClassItem::Range(a, b) => format!("{a}~{b}"),
        ClassItem::Perl(p) => perl_text(*p).to_string(),
        ClassItem::Posix { name, .. } => format!("POSIX {name}"),
        ClassItem::Unicode(name) => format!("Unicode 类别 {name}"),
    }
}

fn class_text(negated: bool, items: &[ClassItem]) -> String {
    // `.` 的 IR 表示：negated + [Char('\n')]
    if negated && items.len() == 1 && matches!(items[0], ClassItem::Char('\n')) {
        return "任意一个字符（默认不含换行）".to_string();
    }
    let names: Vec<String> = items.iter().map(item_text).collect();
    let listed = if names.len() > 6 {
        format!("{}…等 {} 项", names[..6].join("、"), names.len())
    } else {
        names.join("、")
    };
    if negated {
        format!("任意一个字符，但**不是**：{listed}")
    } else {
        format!("一个字符，取以下之一：{listed}")
    }
}

fn flags_text(set: &str, unset: &str) -> String {
    let mut parts = Vec::new();
    for c in set.chars() {
        if let Some(d) = flag_desc(c) {
            parts.push(format!("开启{}", d));
        }
    }
    for c in unset.chars() {
        if let Some(d) = flag_desc(c) {
            parts.push(format!("关闭{}", d));
        }
    }
    format!("内联标志（{}，从此处起生效）", parts.join("，"))
}

fn flag_desc(c: char) -> Option<&'static str> {
    match c {
        'i' => Some("忽略大小写"),
        'm' => Some("多行模式（^$ 按行）"),
        's' => Some("点号匹配换行"),
        'x' => Some("宽松模式（忽略空白）"),
        'U' => Some("贪婪反转"),
        _ => None,
    }
}

// ---- 顶层摘要用的紧凑短标签 ----

fn short(node: &Node) -> String {
    match node {
        Node::Empty => "(空)".into(),
        Node::Literal { text, .. } => format!("「{text}」"),
        Node::Class { negated, items, .. } => {
            if *negated && items.len() == 1 && matches!(items[0], ClassItem::Char('\n')) {
                "任意字符".into()
            } else if items.len() == 1 {
                match &items[0] {
                    ClassItem::Perl(p) => match p {
                        PerlClass::Digit => "数字".into(),
                        PerlClass::Word => "单词字符".into(),
                        PerlClass::Space => "空白".into(),
                        _ => class_text(*negated, items),
                    },
                    ClassItem::Unicode(_) => "\\p{…}".into(),
                    ClassItem::Posix { name, .. } => format!("[[:{name}:]]"),
                    _ => class_text(*negated, items),
                }
            } else {
                class_text(*negated, items)
            }
        }
        Node::Anchor { kind, .. } => match kind {
            AnchorKind::StartLine => "行首".into(),
            AnchorKind::EndLine => "行尾".into(),
            AnchorKind::StartText => "文本开头".into(),
            AnchorKind::EndText => "文本末尾".into(),
            AnchorKind::WordBoundary => "词边界".into(),
            AnchorKind::NotWordBoundary => "非词边界".into(),
        },
        Node::Flags { set, .. } => format!("(?{set})"),
        Node::Backref { kind, .. } => match kind {
            BackrefKind::Index(i) => format!("引用组{i}"),
            BackrefKind::Name(n) => format!("引用组「{n}」"),
        },
        Node::Group { kind, name, child, .. } => {
            let inner = short(child);
            match kind {
                GroupKind::Named => match name.as_deref() {
                    Some(n) => format!("组«{n}»={inner}"),
                    None => inner,
                },
                GroupKind::Capture => inner,
                GroupKind::Lookahead { negative: false } => format!("前瞻({inner})"),
                GroupKind::Lookahead { negative: true } => format!("负前瞻(非{inner})"),
                GroupKind::Lookbehind { negative: false } => format!("后顾({inner})"),
                GroupKind::Lookbehind { negative: true } => format!("负后顾(非{inner})"),
                GroupKind::Atomic => format!("原子[{inner}]"),
                GroupKind::NonCapturing => inner,
            }
        }
        Node::Repeat { child, min, max, rep, .. } => {
            let q = match (*min, max, rep) {
                (0, Some(1), _) => "×0~1".to_string(),
                (0, None, RepKind::Lazy) => "×0+(懒)".to_string(),
                (0, None, _) => "×0+".to_string(),
                (1, None, RepKind::Lazy) => "×1+(懒)".to_string(),
                (1, None, _) => "×1+".to_string(),
                (m, Some(n), RepKind::Lazy) if m != *n => format!("×{m}~{n}(懒)"),
                (m, Some(n), _) if m == *n => format!("×{m}"),
                (m, Some(n), _) => format!("×{m}~{n}"),
                (m, None, RepKind::Lazy) => format!("×{m}+(懒)"),
                (m, None, _) => format!("×{m}+"),
            };
            format!("{}{}", short(child), q)
        }
        Node::Concat(children) => children.iter().map(short).collect::<Vec<_>>().join(""),
        Node::Alternation(branches) => branches
            .iter()
            .map(short)
            .collect::<Vec<_>>()
            .join(" 或 "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex_parser::parse;

    fn segs(pattern: &str) -> Vec<Segment> {
        segments(&parse(pattern).unwrap())
    }

    #[test]
    fn example1_explained_with_spans() {
        let s = segs(r"\b(\d{4})-(\d{2})-(\d{2})\b");
        let joined: String = s.iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join(" | ");
        assert!(joined.contains("词边界"), "{joined}");
        assert!(joined.contains("捕获第 1 组"), "{joined}");
        assert!(joined.contains("重复 4 次"), "{joined}");
        assert!(joined.contains("字面文本「-」"), "{joined}");
        // span 切回源码必须落在 char 边界且内容非空
        for seg in &s {
            let slice = &r"\b(\d{4})-(\d{2})-(\d{2})\b"[seg.span.start as usize..seg.span.end as usize];
            assert!(!slice.is_empty(), "span {:?} 切出空串", seg.span);
        }
    }

    #[test]
    fn summary_example1() {
        let s = summary(&parse(r"\b(\d{4})-(\d{2})-(\d{2})\b").unwrap());
        assert!(s.contains("词边界"), "{s}");
        assert!(s.contains("数字×4"), "{s}");
        assert!(s.contains("→"), "{s}");
    }

    #[test]
    fn greedy_vs_lazy_wording() {
        let greedy = segs(r#""(.+)""#);
        let lazy = segs(r#""(.+?)""#);
        assert!(greedy.iter().any(|x| x.text.contains("贪婪")));
        assert!(lazy.iter().any(|x| x.text.contains("懒惰")));
        // 量词段 span 指向量词记号本身（`+` 与 `+?`）
        let q = lazy.iter().find(|x| x.text.contains("懒惰")).unwrap();
        assert_eq!(&r#""(.+?)""#[q.span.start as usize..q.span.end as usize], "+?");
    }

    #[test]
    fn lookaround_and_backref_wording() {
        let s = segs(r"\d+(?=\.)");
        assert!(s.iter().any(|x| x.text.contains("前瞻") && x.text.contains("零宽")));
        let s = segs(r"\b(\w+) \1\b");
        assert!(s.iter().any(|x| x.text.contains("反向引用第 1 组")));
        let s = segs(r"(?P<y>\d)\k<y>");
        assert!(s.iter().any(|x| x.text.contains("反向引用命名组「y」")));
    }

    #[test]
    fn flags_and_posix_wording() {
        let s = segs("(?i)abc");
        assert!(s.iter().any(|x| x.text.contains("忽略大小写")));
        let s = segs("[[:digit:]a-f]");
        assert!(s.iter().any(|x| x.text.contains("POSIX digit")));
        let s = segs(r"\p{L}");
        assert!(s.iter().any(|x| x.text.contains("Unicode 类别 L")));
        let s = segs("[^a-z]");
        assert!(s.iter().any(|x| x.text.contains("但**不是**")));
    }

    #[test]
    fn alternation_intro_then_branches() {
        let s = segs("cat|dog");
        assert!(s[0].text.contains("多选一"));
        assert!(s.iter().any(|x| x.text.contains("「cat」")));
        assert!(s.iter().any(|x| x.text.contains("「dog」")));
    }

    #[test]
    fn dot_wording() {
        let s = segs("a.c");
        assert!(s.iter().any(|x| x.text.contains("任意一个字符（默认不含换行）")));
    }
}
