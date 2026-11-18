//! 灾难性回溯静态检测（设计方案 5.3 安全拦截 / M3）。
//!
//! 规则：无界量词（*、+、{m,}、{m,n} 大上界）的子树里再出现无界量词 → 风险。
//! 这是 `(a+)+`、`(.*)*`、`((a+b)+c)+` 一族 ReDoS 的结构性特征。
//! 已知盲区（诚实声明）：分支重叠型 `(?:a|aa)+` 无嵌套量词，检测不到——
//! 由执行期"步数预算打满"兜底（教学解释器/时间轴会显式展示）。

use regex_ir::{GroupKind, Node, Span};

#[derive(Debug, Clone, PartialEq)]
pub struct Risk {
    /// 外层量词 span
    pub outer: Span,
    /// 内层量词 span
    pub inner: Span,
    pub desc: String,
}

pub fn check(node: &Node) -> Vec<Risk> {
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

/// 外层"放大器"：无界（None）或大上界（>16，有限常数倍不构成指数放大）
fn is_amplifier(max: Option<u32>) -> bool {
    match max {
        None => true,
        Some(n) => n > 16,
    }
}

/// 内层"可变量"：能变长（None 或 n>1）就可能与外层做重复分配
fn is_variable(max: Option<u32>) -> bool {
    match max {
        None => true,
        Some(n) => n > 1,
    }
}

fn walk(node: &Node, out: &mut Vec<Risk>) {
    if let Node::Repeat { child, max, span, .. } = node {
        if is_amplifier(*max) {
            // 子树里找第一个可变量词（穿过分组/拼接）
            if let Some(inner) = first_variable(child) {
                out.push(Risk {
                    outer: *span,
                    inner,
                    desc: "嵌套量词：外层无界量词之内还有可变量词——回溯次数随输入长度指数增长（ReDoS 风险）。建议：改写为占有量词/原子组，或拆分模式".to_string(),
                });
            }
        }
        walk(child, out);
    } else if let Node::Group { child, kind, .. } = node {
        // 环视内部是独立的匹配上下文，不算外层的回溯子树
        if matches!(kind, GroupKind::NonCapturing | GroupKind::Capture | GroupKind::Named) {
            walk(child, out);
        }
    } else if let Node::Concat(v) = node {
        v.iter().for_each(|c| walk(c, out));
    } else if let Node::Alternation(v) = node {
        v.iter().for_each(|c| walk(c, out));
    }
}

/// 子树中第一个可变量词的 span（穿过普通分组与拼接）
fn first_variable(node: &Node) -> Option<Span> {
    match node {
        Node::Repeat { child, max, span, .. } => {
            if is_variable(*max) {
                Some(*span)
            } else {
                first_variable(child)
            }
        }
        Node::Group { kind, child, .. } => {
            if matches!(kind, GroupKind::NonCapturing | GroupKind::Capture | GroupKind::Named) {
                first_variable(child)
            } else {
                None
            }
        }
        Node::Concat(v) => v.iter().find_map(first_variable),
        Node::Alternation(v) => v.iter().find_map(first_variable),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex_parser::parse;

    fn risks(pattern: &str) -> Vec<Risk> {
        check(&parse(pattern).unwrap())
    }

    #[test]
    fn classic_nesting_detected() {
        // ((a+b)+c)+d 嵌套两层：外层×中层、中层×内层各报一处，属预期
        for pat in ["(a+)+b", "(a*)*", "(?:\\d+)+x", "(?<g>a+)+b"] {
            let r = risks(pat);
            assert_eq!(r.len(), 1, "{pat} 应检出 1 处，实际 {}", r.len());
        }
        let r = risks("((a+b)+c)+d");
        assert!(!r.is_empty(), "嵌套两层应至少检出外层一对");
    }

    #[test]
    fn safe_patterns_pass() {
        for pat in ["a+b", "a*b", "(a+)?b", "a{2,4}", "[a-z]+\\d*", "(a|b)+c", "^\\d+$", "(a{2}){3}"] {
            let r = risks(pat);
            assert!(r.is_empty(), "{pat} 不应有风险，实际 {:?}", r.iter().map(|x| &x.desc).collect::<Vec<_>>());
        }
    }

    #[test]
    fn lookahead_isolated() {
        // 环视内的量词不属于外层回溯子树
        assert!(risks("(?=a+)b").is_empty());
    }
}
