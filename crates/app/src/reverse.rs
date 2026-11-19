//! 反向推演：给一组样例串，启发式猜一个能整串命中它们的正则。
//!
//! 这是"建议生成器"不是魔法——覆盖列出的样例 ≠ 覆盖真实意图，输出必须经自测
//! 和工作台验证。策略由精到粗，取第一条可用路径：
//! 1. 全部相同 → 字面量（转义）；
//! 2. 公共前后缀剥离 + 变体中段做"字符类 run 对齐"：每条中段切成
//!    数字/小写/大写/Unicode字母/空白/下划线/标点 run，run 数与类序列完全一致 →
//!    逐位合并（计数一致→{n}，不一致→{min,max}，如 IP 的 `\d{1,3}`）；
//! 3. 等长中段 → 逐列归类（列内同类→shorthand，异类→[..]，再压缩相邻同列）；
//! 4. 都不行 → 中段用最宽可用类 + {min,max} 兜底。

use egui::{Color32, FontId};

const GOOD: Color32 = Color32::from_rgb(134, 239, 172);
const BAD: Color32 = Color32::from_rgb(255, 128, 128);
const DIM: Color32 = Color32::from_rgb(139, 148, 158);

pub struct State {
    examples: Vec<String>,
    /// 整串匹配（^...$ 包裹）。关掉则生成的正则可用于 find（如扫日志）
    anchored: bool,
    result: Option<String>,
    /// 生成后自测：(样例, 是否全串命中)
    checks: Option<Vec<(String, bool)>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            examples: vec![
                "2024-08-15".into(),
                "2025-01-01".into(),
                "2026-11-30".into(),
            ],
            anchored: true,
            result: None,
            checks: None,
        }
    }
}

// ---- 推断核心（纯函数，测试直测） ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cls {
    Digit,
    Lower,
    Upper,
    /// 非 ASCII 字母（中文等）
    Letter,
    Underscore,
    Space,
    Punct,
    Other,
}

fn cls_of(c: char) -> Cls {
    if c.is_ascii_digit() {
        Cls::Digit
    } else if c.is_ascii_lowercase() {
        Cls::Lower
    } else if c.is_ascii_uppercase() {
        Cls::Upper
    } else if c == '_' {
        Cls::Underscore
    } else if c.is_alphabetic() {
        Cls::Letter
    } else if c.is_whitespace() {
        Cls::Space
    } else if c.is_ascii_punctuation() {
        Cls::Punct
    } else {
        Cls::Other
    }
}

fn cls_token(c: Cls) -> &'static str {
    match c {
        Cls::Digit => r"\d",
        Cls::Lower => "[a-z]",
        Cls::Upper => "[A-Z]",
        Cls::Letter => r"\p{L}",
        Cls::Underscore => "_",
        Cls::Space => r"\s",
        Cls::Punct => "", // 标点逐字面处理，不走 token
        Cls::Other => ".",
    }
}

/// 把字符串切成同字符类的 run
fn runs(s: &str) -> Vec<(Cls, String)> {
    let mut out: Vec<(Cls, String)> = Vec::new();
    for c in s.chars() {
        let k = cls_of(c);
        match out.last_mut() {
            Some((k2, buf)) if *k2 == k => buf.push(c),
            _ => out.push((k, c.to_string())),
        }
    }
    out
}

/// 字面量转义（正则元字符）
fn escape_literal(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 字符集 → [..] 写法（内部字符全转义，最稳）
fn char_class(chars: &[char]) -> String {
    let mut out = String::from("[");
    for &c in chars {
        if "\\\\]^-$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out.push(']');
    out
}

/// 相邻相同 piece 压缩成 {n}（piece 须是原子的：单 token / 单字符面 / [..]）
fn compress(pieces: &[String]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < pieces.len() {
        let mut j = i;
        while j < pieces.len() && pieces[j] == pieces[i] {
            j += 1;
        }
        out.push_str(&pieces[i]);
        let n = j - i;
        if n > 1 {
            out.push_str(&format!("{{{n}}}"));
        }
        i = j;
    }
    out
}

/// 计数向量 → 量词后缀：{1}→无；{0,1}→?；其余 {n} / {min,max}
fn quant(min: usize, max: usize) -> String {
    if min == max {
        match min {
            1 => String::new(),
            0 => "{0}".into(), // 不应出现，防御
            n => format!("{{{n}}}"),
        }
    } else if min == 0 && max == 1 {
        "?".into()
    } else {
        format!("{{{min},{max}}}")
    }
}

/// 路径②：run 类序列完全一致 → 逐位合并。
/// 每位优先级：各例该 run 内容完全一致 → 字面量（最忠实）；否则 → 类 token + 量词。
fn try_align(middles: &[&str]) -> Option<String> {
    let all_runs: Vec<Vec<(Cls, String)>> = middles.iter().map(|m| runs(m)).collect();
    let first = all_runs.first()?;
    if first.is_empty() {
        return None; // 有空中段：长度不齐，交下一条路径
    }
    if !all_runs.iter().all(|r| r.len() == first.len()) {
        return None;
    }
    let mut out = String::new();
    for i in 0..first.len() {
        let cls = first[i].0;
        if !all_runs.iter().all(|r| r[i].0 == cls) {
            return None;
        }
        let strs: Vec<&str> = all_runs.iter().map(|r| r[i].1.as_str()).collect();
        if strs.iter().all(|s| *s == strs[0]) {
            out.push_str(&escape_literal(strs[0])); // 全等 → 字面量，无需量词
            continue;
        }
        let counts: Vec<usize> = all_runs.iter().map(|r| r[i].1.chars().count()).collect();
        let (min, max) = (*counts.iter().min()?, *counts.iter().max()?);
        match cls {
            Cls::Punct => {
                let mut chars: Vec<char> = strs.iter().flat_map(|s| s.chars()).collect();
                chars.sort();
                chars.dedup();
                out.push_str(&char_class(&chars));
                out.push_str(&quant(min, max));
            }
            _ => {
                out.push_str(cls_token(cls));
                out.push_str(&quant(min, max));
            }
        }
    }
    Some(out)
}

/// 路径③：中段等长 → 逐列归类 + 压缩
fn per_column(middles: &[&str]) -> Option<String> {
    let lens: Vec<usize> = middles.iter().map(|m| m.chars().count()).collect();
    if lens.is_empty() || !lens.iter().all(|l| *l == lens[0]) {
        return None;
    }
    let n = lens[0];
    let chars: Vec<Vec<char>> = middles.iter().map(|m| m.chars().collect()).collect();
    let mut pieces: Vec<String> = Vec::with_capacity(n);
    for j in 0..n {
        let col: Vec<char> = chars.iter().map(|cs| cs[j]).collect();
        let piece = if col.iter().all(|c| *c == col[0]) {
            escape_literal(&col[0].to_string())
        } else {
            let cls = cls_of(col[0]);
            if !matches!(cls, Cls::Punct | Cls::Other) && col.iter().all(|c| cls_of(*c) == cls) {
                cls_token(cls).to_string()
            } else {
                let mut u: Vec<char> = col.clone();
                u.sort();
                u.dedup();
                if u.len() > 12 {
                    ".".to_string() // 列内太杂，收成 .（宁粗勿错）
                } else {
                    char_class(&u)
                }
            }
        };
        pieces.push(piece);
    }
    Some(compress(&pieces))
}

/// 路径④：兜底——中段取最宽可用类 + {min,max}
fn fallback(middles: &[&str]) -> String {
    let all: Vec<char> = middles.iter().flat_map(|m| m.chars()).collect();
    if all.is_empty() {
        return String::new();
    }
    let min = middles.iter().map(|m| m.chars().count()).min().unwrap_or(0);
    let max = middles.iter().map(|m| m.chars().count()).max().unwrap_or(0);
    let piece = if all.iter().all(|c| cls_of(*c) == Cls::Digit) {
        r"\d"
    } else if all
        .iter()
        .all(|c| matches!(cls_of(*c), Cls::Digit | Cls::Lower | Cls::Upper | Cls::Underscore | Cls::Letter))
    {
        r"\w" // regex/fancy-regex 的 \w 均含 Unicode 字母
    } else {
        "."
    };
    if min == max && min == 1 {
        piece.into()
    } else {
        format!("{piece}{}", quant(min, max))
    }
}

/// 入口：从样例推正则（不含锚点；锚点由 UI 层按「整串匹配」开关加）
pub fn infer(examples: &[String]) -> String {
    let cleaned: Vec<&str> = examples.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    if cleaned.is_empty() {
        return String::new();
    }
    if cleaned.iter().all(|s| *s == cleaned[0]) {
        return escape_literal(cleaned[0]);
    }
    // 路径②先整串对齐（类 run 序列一致时最优雅，如日期 → \d{4}-\d{2}-\d{2}）；
    // 不行再剥公共前后缀降级重试，最后逐列/兜底。
    let whole: Vec<&str> = cleaned.iter().copied().collect();
    if let Some(body) = try_align(&whole) {
        return body;
    }
    // 公共前后缀（char 级，前后缀不重叠）
    let vc: Vec<Vec<char>> = cleaned.iter().map(|s| s.chars().collect()).collect();
    let min_len = vc.iter().map(|cs| cs.len()).min().unwrap_or(0);
    let mut p = 0usize;
    while p < min_len && vc.iter().all(|cs| cs[p] == vc[0][p]) {
        p += 1;
    }
    let mut s = 0usize;
    while s < min_len - p && vc.iter().all(|cs| cs[cs.len() - 1 - s] == vc[0][vc[0].len() - 1 - s]) {
        s += 1;
    }
    let prefix: String = escape_literal(&vc[0][..p].iter().collect::<String>());
    let suffix: String = escape_literal(&vc[0][vc[0].len() - s..].iter().collect::<String>());
    let middles: Vec<String> = vc
        .iter()
        .map(|cs| cs[p..cs.len() - s].iter().collect())
        .collect();
    let mrefs: Vec<&str> = middles.iter().map(|m| m.as_str()).collect();

    let body = try_align(&mrefs)
        .or_else(|| per_column(&mrefs))
        .unwrap_or_else(|| fallback(&mrefs));
    format!("{prefix}{body}{suffix}")
}

/// 整串命中判定（自测用；pattern 可能带 ^...$ 也可能不带，均适用）
fn full_match_ok(pattern: &str, ex: &str) -> bool {
    let sc = regex_exec::Scanner::new(pattern);
    if sc.engine().is_none() {
        return false;
    }
    sc.find_in(ex).iter().any(|&(s, e)| s == 0 && e == ex.len())
}

// ---- UI ----

pub fn show(ui: &mut egui::Ui, st: &mut State) -> Option<String> {
    let mut handoff = None;
    ui.heading("反向推演：样例 → 正则");
    ui.weak("给一组样例串，猜一个能整串命中它们的正则。启发式建议——自测通过 ≠ 覆盖真实意图，请到工作台继续调。");
    ui.separator();

    let mut changed = false;
    ui.horizontal(|ui| {
        ui.weak(format!("{} 条样例", st.examples.len()));
        if ui.small_button("＋ 添加").clicked() {
            st.examples.push(String::new());
            changed = true;
        }
        ui.separator();
        if ui.checkbox(&mut st.anchored, "整串匹配（^…$）").changed() {
            st.result = None;
            st.checks = None;
        }
        if ui
            .add_enabled(
                st.examples.iter().any(|e| !e.trim().is_empty()),
                egui::Button::new(egui::RichText::new("⚡ 生成正则").strong()),
            )
            .clicked()
        {
            let body = infer(&st.examples);
            let pat = if st.anchored && !body.is_empty() {
                format!("^(?:{body})$")
            } else {
                body
            };
            crate::logging::info(&format!(
                "[反推] 生成 {} 条样例 → {pat}",
                st.examples.iter().filter(|e| !e.trim().is_empty()).count()
            ));
            st.checks = if pat.is_empty() {
                None
            } else {
                Some(
                    st.examples
                        .iter()
                        .filter(|e| !e.is_empty())
                        .map(|e| (e.clone(), full_match_ok(&pat, e)))
                        .collect(),
                )
            };
            st.result = if pat.is_empty() { None } else { Some(pat) };
        }
    });

    egui::ScrollArea::vertical()
        .max_height(180.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            'ex: for i in 0..st.examples.len() {
                let mut removed = false;
                ui.horizontal(|ui| {
                    ui.monospace(egui::RichText::new(format!("#{:<2}", i + 1)).color(DIM));
                    if ui
                        .add_enabled(st.examples.len() > 1, egui::Button::new("✕").small())
                        .clicked()
                    {
                        st.examples.remove(i);
                        removed = true;
                    }
                });
                if removed {
                    changed = true;
                    break 'ex;
                }
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut st.examples[i])
                        .desired_width(f32::INFINITY)
                        .font(FontId::monospace(13.0))
                        .hint_text("输入一条样例串"),
                );
                if resp.changed() {
                    changed = true;
                }
            }
        });
    if changed {
        // 样例变了：旧建议与自测已过期
        st.result = None;
        st.checks = None;
    }

    ui.separator();
    if let Some(pat) = &st.result {
        ui.horizontal(|ui| {
            ui.label("建议正则：");
            ui.monospace(egui::RichText::new(pat).strong().color(GOOD));
            if ui.small_button("复制").clicked() {
                ui.output_mut(|o| o.commands.push(egui::OutputCommand::CopyText(pat.clone())));
            }
            if ui.small_button("发送到工作台").clicked() {
                handoff = Some(pat.clone());
            }
        });
        if let Some(checks) = &st.checks {
            ui.add_space(2.0);
            for (ex, ok) in checks {
                ui.monospace(
                    egui::RichText::new(format!("{} {ex}", if *ok { "✓" } else { "✗" }))
                        .color(if *ok { GOOD } else { BAD }),
                );
            }
            let n_ok = checks.iter().filter(|(_, ok)| *ok).count();
            ui.weak(format!("自测：{}/{} 全串命中", n_ok, checks.len()));
        }
    } else {
        ui.weak("（点「⚡ 生成正则」出建议；样例越相似，结果越精确）");
    }
    handoff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(examples: &[&str]) -> String {
        let v: Vec<String> = examples.iter().map(|s| s.to_string()).collect();
        infer(&v)
    }

    #[test]
    fn identical_is_literal() {
        assert_eq!(pat(&["abc", "abc"]), r"abc");
        assert_eq!(pat(&["a.c", "a.c"]), r"a\.c");
    }

    #[test]
    fn dates_aligned() {
        let p = pat(&["2024-08-15", "2025-01-01"]);
        assert_eq!(p, r"\d{4}-\d{2}-\d{2}");
    }

    #[test]
    fn counts_vary_to_range() {
        // IP 形：整串 run 对齐，各段位数不齐 → {min,max}（样例精确值，不硬凑 \d{1,3}）
        let p = pat(&["192.168.1.1", "10.0.0.255"]);
        assert_eq!(p, r"\d{2,3}\.\d{1,3}\.\d\.\d{1,3}");
    }

    #[test]
    fn user_kv() {
        // 恒定段（user=）保字面量，变化段泛化
        let p = pat(&["user=zhang", "user=wang"]);
        assert_eq!(p, "user=[a-z]{4,5}");
    }

    #[test]
    fn suffix_kept() {
        let p = pat(&["id=1;", "id=42;"]);
        assert_eq!(p, r"id=\d{1,2};");
    }

    #[test]
    fn same_len_diff_class_columns() {
        let p = pat(&["a1b", "2x3"]);
        assert_eq!(p, "[2a][1x][3b]");
    }

    #[test]
    fn same_class_vary_len() {
        // run 类序列一致但长度不齐 → {min,max}（不再退化到兜底路径）
        let p = pat(&["abc", "abcdef"]);
        assert_eq!(p, "[a-z]{3,6}");
    }

    #[test]
    fn output_compiles_and_covers() {
        let cases: &[&[&str]] = &[
            &["2024-08-15", "2025-01-01", "2026-11-30"],
            &["192.168.1.1", "10.0.0.255", "172.16.0.1"],
            &["user=zhang", "user=wang", "user=li"],
            &["abc", "abcdef"],
            &["a1b", "2x3"],
        ];
        for case in cases {
            let p = pat(case);
            assert!(regex_exec::Scanner::new(&p).engine().is_some(), "编译失败：{p}");
            for ex in *case {
                assert!(full_match_ok(&p, ex), "未覆盖 {ex}：{p}");
            }
        }
    }

    #[test]
    fn anchored_full_match_semantics() {
        let p = format!("^(?:{})$", pat(&["2024-08-15", "2025-01-01"]));
        assert!(full_match_ok(&p, "2024-08-15"));
        assert!(!full_match_ok(&p, "2024-8-15"));
        assert!(!full_match_ok(&p, "x2024-08-15"));
    }
}
