//! 统一正则 IR —— 全仓唯一权威类型层。
//!
//! 工程纪律（设计方案 6.4）：本 crate **零依赖**。
//! 每个节点携带 byte span，是"正则文本 ↔ 铁路图 ↔ 人话层"三层悬停联动的地基。

/// 源码中的字节区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start: start as u32, end: end as u32 }
    }

    /// 错误提示用：换算成 1-based 列号（按字节近似，M1 换 char 精确列）。
    pub fn col(&self) -> usize {
        self.start as usize + 1
    }
}

/// Perl 风格预定义类：\d \D \w \W \s \S
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerlClass {
    Digit,
    NotDigit,
    Word,
    NotWord,
    Space,
    NotSpace,
}

/// 字符类成员
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassItem {
    /// 单字符，如 `a`
    Char(char),
    /// 区间，如 `a-z`
    Range(char, char),
    /// Perl 类，如 `\d`
    Perl(PerlClass),
    /// POSIX 类，如 `[:alpha:]`（类内语法）
    Posix { name: String, negated: bool },
    /// Unicode 属性，如 `\p{L}`（M1 借 regex-syntax 公开表展开）
    Unicode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorKind {
    /// `^`（是否按行解释由内联标志 `(?m)` 决定，引擎层处理）
    StartLine,
    /// `$`
    EndLine,
    /// `\A`
    StartText,
    /// `\z` / `\Z`（两者差异——`\Z` 允许末尾换行——当前统一按 `\z` 处理）
    EndText,
    /// `\b`
    WordBoundary,
    /// `\B`
    NotWordBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Capture,
    Named,
    NonCapturing,
    Lookahead { negative: bool },
    Lookbehind { negative: bool },
    /// `(?>...)` 原子组，DFA 家族不支持
    Atomic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackrefKind {
    /// `\1` 编号引用
    Index(u32),
    /// `\k<name>` / `(?P=name)` 命名引用
    Name(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepKind {
    Greedy,
    Lazy,
    Possessive,
}

/// 统一 IR 节点。以 PCRE 为超集基准（设计方案 7.5）。
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// 空分支（如 `a|` 的右支）
    Empty,
    /// 字面量（连续普通字符合并，减少碎片）
    Literal { text: String, span: Span },
    /// 字符类；`.` 表示为 negated + [Char('\n')]（即"非换行的任意字符"）
    Class { negated: bool, items: Vec<ClassItem>, span: Span },
    Anchor { kind: AnchorKind, span: Span },
    /// 分组。index 仅捕获/命名组有值（按 `(` 出现顺序从 1 编号）；
    /// flags 记录 `(?i:...)` 作用域前缀原文（如 "i"、"im-s"）
    Group { kind: GroupKind, name: Option<String>, index: Option<u32>, flags: Option<String>, child: Box<Node>, span: Span },
    Repeat { child: Box<Node>, min: u32, max: Option<u32>, rep: RepKind, span: Span },
    Concat(Vec<Node>),
    Alternation(Vec<Node>),
    /// 全局内联标志 `(?i)` / `(?im-s)`（零宽；作用域版用 Group.flags 表示）
    Flags { set: String, unset: String, span: Span },
    /// 反向引用（回溯家族专属，DFA 家族在方言层拦截）
    Backref { kind: BackrefKind, span: Span },
}

impl Node {
    /// 量词的人话短标签（铁路图徽标 / 错误提示共用）
    pub fn quant_label(min: u32, max: Option<u32>, rep: RepKind) -> String {
        let base = match (min, max) {
            (0, Some(1)) => "?".to_string(),
            (0, None) => "*".to_string(),
            (1, None) => "+".to_string(),
            (m, Some(n)) if m == n => format!("{{{m}}}"),
            (m, None) => format!("{{{m},}}"),
            (m, Some(n)) => format!("{{{m},{n}}}"),
        };
        match rep {
            RepKind::Greedy => base,
            RepKind::Lazy => format!("{base}?"),
            RepKind::Possessive => format!("{base}+"),
        }
    }
}

// ---------------------------------------------------------------------------
// 方言特性矩阵（设计方案 7.2 / 7.4 的工程化雏形，M1 补全并外置配置表）
// ---------------------------------------------------------------------------

/// 7 大引擎家族代表方言
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    Pcre,
    Java,
    JavaScript,
    DotNet,
    Python,
    Go,
    Rust,
}

impl Dialect {
    /// 代码导出用的语言标签（M4 codegen）
    pub fn lang_label(&self) -> &'static str {
        match self {
            Dialect::Java => "Java（java.util.regex）",
            Dialect::Python => "Python（re）",
            Dialect::JavaScript => "JavaScript（ES2018）",
            Dialect::Go => "Go（regexp）",
            Dialect::Rust => "Rust（regex crate）",
            Dialect::Pcre => "PCRE（超集基准）",
            Dialect::DotNet => ".NET",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Dialect::Pcre => "PCRE 家族（Perl/PHP/Ruby）",
            Dialect::Java => "Java 家族",
            Dialect::JavaScript => "JavaScript 家族",
            Dialect::DotNet => ".NET 家族",
            Dialect::Python => "Python 家族",
            Dialect::Go => "Go（DFA）",
            Dialect::Rust => "Rust regex（DFA）",
        }
    }
}

/// 特性开关矩阵关心的高频特性
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// `\1` 反向引用
    Backreference,
    /// `(?=)` / `(?! )`
    Lookahead,
    /// `(?<=)` / `(?<! )`
    Lookbehind,
    /// `a*+`
    PossessiveQuant,
    /// `(?>...)`
    AtomicGroup,
    /// `(?R)` / `(?1)` 递归
    Recursion,
    /// `(?P<name>...)` 写法
    NamedGroupP,
    /// `(?<name>...)` 写法
    NamedGroupAngleQ,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Yes,
    No,
    /// 支持但有边界（附一句话说明，UI 黄色告警用）
    Partial(&'static str),
}

/// 遍历 IR 收集用到的特性（方言门禁用：目标方言 No 的特性 = 拒绝导出/告警）
pub fn used_features(node: &Node) -> std::collections::BTreeSet<Feature> {
    let mut out = std::collections::BTreeSet::new();
    walk_features(node, &mut out);
    out
}

fn walk_features(node: &Node, out: &mut std::collections::BTreeSet<Feature>) {
    match node {
        Node::Backref { .. } => {
            out.insert(Feature::Backreference);
        }
        Node::Group { kind, child, .. } => {
            match kind {
                GroupKind::Lookahead { .. } => {
                    out.insert(Feature::Lookahead);
                }
                GroupKind::Lookbehind { .. } => {
                    out.insert(Feature::Lookbehind);
                }
                GroupKind::Atomic => {
                    out.insert(Feature::AtomicGroup);
                }
                _ => {}
            }
            walk_features(child, out);
        }
        Node::Repeat { child, rep, .. } => {
            if *rep == RepKind::Possessive {
                out.insert(Feature::PossessiveQuant);
            }
            walk_features(child, out);
        }
        Node::Concat(v) => v.iter().for_each(|c| walk_features(c, out)),
        Node::Alternation(v) => v.iter().for_each(|c| walk_features(c, out)),
        _ => {}
    }
}

impl Feature {
    pub fn label(&self) -> &'static str {
        match self {
            Feature::Backreference => "反向引用 \\1",
            Feature::Lookahead => "前瞻环视 (?=)",
            Feature::Lookbehind => "后顾环视 (?<=)",
            Feature::PossessiveQuant => "占有量词 a*+",
            Feature::AtomicGroup => "原子组 (?>)",
            Feature::Recursion => "递归 (?R)",
            Feature::NamedGroupP => "命名组写法 (?P<n>)",
            Feature::NamedGroupAngleQ => "命名组写法 (?<n>)",
        }
    }
}

/// 特性 × 方言支持矩阵（设计方案 7.2 的工程化，含 7.2 勘误：.NET 无占有量词）。
/// 当前为内置表；若要前后端共享配置可再外置（见设计方案 9.2 的设想）。
pub fn supports(dialect: Dialect, feature: Feature) -> Support {
    use Dialect as D;
    use Feature as F;
    use Support as S;

    match (dialect, feature) {
        (D::Pcre, _) => S::Yes,
        (D::Java, F::Lookbehind) => S::Partial("仅有界（定长或有限重复）"),
        (D::Java, F::Recursion) => S::No,
        (D::Java, F::NamedGroupP) => S::No,
        (D::Java, _) => S::Yes,
        (D::JavaScript, F::Lookbehind) => S::Partial("ES2018+"),
        (D::JavaScript, F::NamedGroupAngleQ) => S::Partial("ES2018+"),
        (D::JavaScript, F::PossessiveQuant | F::AtomicGroup | F::Recursion | F::NamedGroupP) => S::No,
        (D::JavaScript, _) => S::Yes,
        (D::DotNet, F::Recursion) => S::Partial("无 (?R)，独有平衡组"),
        (D::DotNet, F::NamedGroupP) => S::No,
        // .NET 无占有量词语法（有原子组 (?>)；防回溯另有 NonBacktracking 模式）
        (D::DotNet, F::PossessiveQuant) => S::No,
        (D::DotNet, _) => S::Yes,
        (D::Python, F::Lookbehind) => S::Partial("仅定长"),
        (D::Python, F::PossessiveQuant | F::AtomicGroup) => S::Partial("3.11+"),
        (D::Python, F::Recursion | F::NamedGroupAngleQ) => S::No,
        (D::Python, _) => S::Yes,
        // DFA 家族：反向引用 / 环视 / 占有语法 / 递归全不支持（设计方案 7.1 的硬断层）
        (D::Go | D::Rust, _) => S::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dfa_family_hard_gaps() {
        for d in [Dialect::Go, Dialect::Rust] {
            assert_eq!(supports(d, Feature::Backreference), Support::No);
            assert_eq!(supports(d, Feature::Lookahead), Support::No);
            assert_eq!(supports(d, Feature::Lookbehind), Support::No);
        }
    }

    #[test]
    fn java_lookbehind_is_bounded_not_fixed() {
        // 7.2 勘误：Java 是"仅有界"而非"仅定长"（文档 2026-11-17 修订）
        assert_eq!(supports(Dialect::Java, Feature::Lookbehind), Support::Partial("仅有界（定长或有限重复）"));
        assert_eq!(supports(Dialect::Python, Feature::Lookbehind), Support::Partial("仅定长"));
    }

    #[test]
    fn quant_labels() {
        assert_eq!(Node::quant_label(0, Some(1), RepKind::Greedy), "?");
        assert_eq!(Node::quant_label(0, None, RepKind::Lazy), "*?");
        assert_eq!(Node::quant_label(2, Some(4), RepKind::Greedy), "{2,4}");
    }
}
