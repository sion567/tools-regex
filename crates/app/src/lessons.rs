//! 交互式实战教学（设计方案第八章 / M4）：渐进闯关 + 方言翻译实战。
//!
//! 两类关卡：
//! - Match：给定文本，用户正则的命中集合必须**精确等于**期望集合（多杀/漏杀都不过）
//! - Port：给定源正则与目标方言——用户改写的正则必须 (a) 不含目标方言不支持的特性
//!   （方言矩阵门禁，设计方案 7.5）(b) 命名组写法符合目标语法 (c) 与源正则在样本上命中等价
//!
//! 自验证：每关内置参考解，测试里保证"参考解必通关"——期望集合抄错会立刻暴露。

use crate::logging;
use regex_ir::{used_features, Dialect, Feature, Support};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// 命中集合精确匹配（字节区间）
    Match { expected: &'static [(usize, usize)] },
    /// 方言移植（named_req: true=要求 (?P< 写法，false=要求 (?< 写法，None=不限）
    Port { source: &'static str, target: Dialect, named_req: Option<bool> },
}

pub struct Level {
    pub id: &'static str,
    pub title: &'static str,
    pub prompt: &'static str,
    pub text: &'static str,
    pub solution: &'static str,
    pub kind: Kind,
}

// 期望集合用 macro 展开成 Level（static 切片引用，无运行时构造）
macro_rules! level_match {
    ($id:literal, $title:literal, $prompt:literal, $text:literal, $solution:literal, $expected:expr) => {
        Level {
            id: $id,
            title: $title,
            prompt: $prompt,
            text: $text,
            solution: $solution,
            kind: Kind::Match { expected: $expected },
        }
    };
}

pub static LEVELS: &[Level] = &[
    level_match!(
        "L0",
        "热身：字面量",
        "在 cat dog cow 里精确匹配单词 dog（别把 cat 和 cow 也抓进来）",
        "cat dog cow",
        "dog",
        &[(4, 7)]
    ),
    level_match!(
        "L1",
        "字符类",
        "找出 education 里的全部元音字母（每个单独一击）",
        "education",
        "[aeiou]",
        &[(0, 1), (2, 3), (4, 5), (6, 7), (7, 8)]
    ),
    level_match!(
        "L2",
        "量词区间",
        "匹配 ab 后跟 2~4 个 b 的串（如 abbb），单杀 ab 和 abbbbb 不算",
        "ab abbb abbbb aab",
        "ab{2,4}",
        &[(3, 7), (8, 13)]
    ),
    level_match!(
        "L3",
        "分组与或",
        "匹配复数宠物：cats、dogs（单数的 cat/dog 不算）",
        "cats dog dogs cat cats",
        "(cat|dog)s",
        &[(0, 4), (9, 13), (18, 22)]
    ),
    level_match!(
        "L4",
        "捕获组",
        "匹配 HH:MM 两位制时间（7:5 这种一位数不算）",
        "start 09:30 end 18:45 bad 7:5",
        r"(\d{2}):(\d{2})",
        &[(6, 11), (16, 21)]
    ),
    level_match!(
        "L5",
        "锚点与多行",
        "匹配每一行开头处（且仅开头处）的 http:// 或 https://（提示：内联多行标志）",
        "http://a.com\nhttps://b.cn\nftp://c.io",
        r"(?m)^https?://",
        &[(0, 7), (13, 21)]
    ),
    level_match!(
        "L6",
        "贪婪转懒惰",
        "逐个匹配全部 HTML 标签（贪婪 <.+> 会一口吞成一个大匹配——差一个 ? 的事）",
        "<a>x</a><b>y</b>",
        "<[^>]*>",
        &[(0, 3), (4, 8), (8, 11), (12, 16)]
    ),
    level_match!(
        "L7",
        "反向引用",
        "找出文本里相邻重复的单词（the the、cat cat）",
        "the the cat cat dog",
        r"(\w+) \1",
        &[(0, 7), (8, 15)]
    ),
    level_match!(
        "L8",
        "环视",
        "只匹配 % 前面的数字（% 本身不要吃进来——零宽断言）",
        "100% off, 50$ and 30% end",
        r"\d+(?=%)",
        &[(0, 3), (18, 20)]
    ),
    level_match!(
        "Boss",
        "终极关卡：修复 ReDoS",
        "把危险正则 (a+)+b 的行为修好：仍要完整匹配 30 个 a + b，但不能再有灾难性回溯风险（提示：拆掉嵌套量词）",
        "", // Boss 文本运行时生成（30 个 a），level_text() 特判
        "a+b",
        &[(0, 31)]
    ),
    Level {
        id: "P1",
        title: "方言翻译：Java → Python",
        prompt: "源正则是 Java 写法（命名组 (?<k>)）。把它移植到 Python：命名组必须改成 (?P<k>) 写法，且在样本上命中等价",
        text: "user=zhang",
        solution: r"(?P<k>\w+)=(?P<v>\w+)",
        kind: Kind::Port {
            source: r"(?<k>\w+)=(?<v>\w+)",
            target: Dialect::Python,
            named_req: Some(true),
        },
    },
    Level {
        id: "P2",
        title: "方言翻译：Python → Java",
        prompt: "源正则是 Python 写法（(?P<y>)）。移植到 Java：必须改成 (?<y>) 写法（这就是 7.2 矩阵里'新手最容易绊倒'的反向坑）",
        text: "since 2024-08 ok",
        solution: r"(?<y>\d{4})-(\d{2})",
        kind: Kind::Port {
            source: r"(?P<y>\d{4})-(\d{2})",
            target: Dialect::Java,
            named_req: Some(false),
        },
    },
    Level {
        id: "P3",
        title: "方言翻译：Java → Go",
        prompt: "把编号捕获组的正则移植到 Go（DFA 家族）：不能用环视/反向引用，编号组跨引擎可用；命中须等价",
        text: "id=42",
        solution: r"(\w+)=(\d+)",
        kind: Kind::Port {
            source: r"(\w+)=(\d+)",
            target: Dialect::Go,
            named_req: None,
        },
    },
];

// Boss 关的 30 个 a 不能写在 const 里 → 运行时拼接
fn format_path() -> String {
    format!("{}b", "a".repeat(30))
}

fn boss_text() -> &'static str {
    // 进程级缓存，避免每帧重建
    use std::sync::OnceLock;
    static BOSS: OnceLock<String> = OnceLock::new();
    BOSS.get_or_init(format_path)
}

pub fn level_text(level: &Level) -> &str {
    if level.id == "Boss" {
        boss_text()
    } else {
        level.text
    }
}

// ---- 检查器 ----

/// 返回 (通过, 说明)
pub fn check(level: &Level, user: &str) -> (bool, String) {
    if user.trim().is_empty() {
        return (false, "还没输入正则".into());
    }
    match &level.kind {
        Kind::Match { expected } => {
            let sc = regex_exec::Scanner::new(user);
            let Some(engine) = sc.engine() else {
                return (false, format!("两个引擎都编译失败：{}", sc.error().unwrap_or("")));
            };
            let got = sc.find_in(level_text(level));
            let want: Vec<(usize, usize)> = expected.to_vec();
            // Boss 关附加约束：不允许残留灾难性回溯风险（设计方案 8.3 的动手修复）
            if level.id == "Boss" {
                let node = regex_parser::parse_partial(user).0;
                let risks = regex_exec::danger::check(&node);
                if !risks.is_empty() {
                    return (
                        false,
                        "功能对了但正则仍带灾难性回溯风险（嵌套量词）——试试拆掉外层量词".into(),
                    );
                }
            }
            if got == want {
                (
                    true,
                    format!(
                        "命中 {} 处，与期望完全一致（引擎：{}）",
                        got.len(),
                        engine.label()
                    ),
                )
            } else {
                let mut why = format!(
                    "命中集合不符。\n期望 {want:?}\n实际 {got:?}（引擎：{}）",
                    engine.label()
                );
                if got.len() > want.len() {
                    why.push_str("\n多杀了：检查是否把不该匹配的也吞进来了（试试收窄类/加锚点）");
                } else if got.len() < want.len() {
                    why.push_str("\n漏了：检查量词区间/是否漏了变体");
                } else {
                    why.push_str("\n位置不对：检查是否多吃了或少吃了边界");
                }
                (false, why)
            }
        }
        Kind::Port { source, target, named_req } => {
            // 1) 命名组写法要求（源语法层检查，IR 不保留写法）
            if let Some(req_p) = named_req {
                let (has_p, has_angle) = (uses_p_named(user), uses_angle_named(user));
                if *req_p && has_angle && !has_p {
                    return (false, "Python/Go 只认 (?P<name>) 写法——把 (?<name>) 改过来（设计方案 7.2 头号坑）".into());
                }
                if !*req_p && has_p {
                    return (false, "Java/JS 只认 (?<name>) 写法——把 (?P<name>) 改过来".into());
                }
            }
            // 2) 方言特性门禁：目标方言 No 的特性 → 拒绝（设计方案 7.5 第 3 条）
            let node = match regex_parser::parse_partial(user) {
                (n, None) => n,
                (n, Some(_err)) => {
                    // 先按宽松结果查特性，但语法错误本身要报
                    if let Err(strict) = regex_parser::parse(user) {
                        return (false, format!("解析失败：{strict}"));
                    }
                    n
                }
            };
            for f in used_features(&node) {
                if let Support::No = regex_ir::supports(*target, f) {
                    return (
                        false,
                        format!(
                            "{}（{}）不支持 {}——这不是语法错误，是引擎家族限制。{}",
                            target.label(),
                            "DFA/回溯家族",
                            f.label(),
                            rewrite_hint(f)
                        ),
                    );
                }
            }
            // 3) 语义等价：与源正则命中集合一致
            let src = regex_exec::Scanner::new(source);
            let usr = regex_exec::Scanner::new(user);
            let (a, b) = (src.find_in(level_text(level)), usr.find_in(level_text(level)));
            if a == b {
                (true, format!("移植成功：与源正则在样本上命中等价（{} 处）", a.len()))
            } else {
                (false, format!("命中不等价。\n源 {source} → {a:?}\n你的 → {b:?}"))
            }
        }
    }
}

fn rewrite_hint(f: Feature) -> &'static str {
    match f {
        Feature::Backreference => "重复词类需求在 Go/Rust 里只能改用代码比较或换回溯引擎",
        Feature::Lookahead | Feature::Lookbehind => "把断言改成捕获 + 代码校验，或用 fancy-regex（Rust）/回溯引擎",
        _ => "见设计方案 7.5 的改写建议",
    }
}

fn uses_p_named(p: &str) -> bool {
    p.contains("(?P<")
}

/// "(?<name>" 形式（排除 (?<= / (?<! 环视）
fn uses_angle_named(p: &str) -> bool {
    let b = p.as_bytes();
    for i in 0..b.len().saturating_sub(3) {
        if &b[i..i + 3] == b"(?<" {
            let c = b[i + 3] as char;
            if c.is_ascii_alphabetic() || c == '_' {
                return true;
            }
        }
    }
    false
}

// ---- UI ----

const GOOD: egui::Color32 = egui::Color32::from_rgb(134, 239, 172);
const BAD: egui::Color32 = egui::Color32::from_rgb(255, 128, 128);
const WARN: egui::Color32 = egui::Color32::from_rgb(255, 193, 7);

pub struct State {
    cur: usize,
    input: String,
    verdict: Option<(bool, String)>,
    solved: Vec<bool>,
}

impl Default for State {
    fn default() -> Self {
        // solved 必须与关卡数对齐——派生 Default 会得到空 Vec（越界 panic）
        Self { cur: 0, input: String::new(), verdict: None, solved: vec![false; LEVELS.len()] }
    }
}

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    let solved_n = st.solved.iter().filter(|x| **x).count();
    ui.horizontal(|ui| {
        ui.heading("闯关教学");
        ui.weak(format!(
            "已通关 {solved_n}/{} —— 左文本右任务，命中集合要精确对上（多杀/漏杀都不过）",
            LEVELS.len()
        ));
    });
    ui.separator();

    // 关卡选择
    ui.horizontal_wrapped(|ui| {
        for (i, lv) in LEVELS.iter().enumerate() {
            let label = if st.solved[i] {
                egui::RichText::new(format!("✓ {}", lv.id)).color(GOOD)
            } else {
                egui::RichText::new(lv.id)
            };
            if ui.selectable_label(st.cur == i, label).clicked() {
                logging::info(&format!("[闯关] 选关 → {} {}", lv.id, lv.title));
                st.cur = i;
                st.input.clear();
                st.verdict = None;
            }
        }
    });
    ui.separator();

    let (lv, is_last) = {
        let lv = &LEVELS[st.cur];
        (lv, st.cur + 1 >= LEVELS.len())
    };

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(lv.title).strong());
        ui.weak(lv.id);
    });
    ui.label(egui::RichText::new(lv.prompt).color(egui::Color32::from_rgb(200, 205, 212)));

    // 样本文本（Boss 是运行时生成的长串）
    ui.add_space(2.0);
    ui.label(egui::RichText::new("样本文本：").weak());
    let text = level_text(lv);
    let display: String = if text.len() > 80 {
        format!("{}…（共 {} 字节）…{}", &text[..20], text.len(), &text[text.len() - 20..])
    } else {
        text.to_string()
    };
    ui.monospace(display);

    // 移植关：展示源正则与目标方言
    if let Kind::Port { source, target, .. } = &lv.kind {
        ui.label(egui::RichText::new(format!("源正则：{source}")).monospace());
        ui.label(egui::RichText::new(format!("目标方言：{}", target.label())).color(WARN));
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("你的正则：");
        let resp = ui.add(
            egui::TextEdit::singleline(&mut st.input)
                .desired_width(360.0)
                .font(egui::FontId::monospace(14.0))
                .hint_text("输入后自动检查；多杀/漏杀都过不了"),
        );
        if resp.changed() || ui.button("检查").clicked() {
            check_and_record(st, lv);
        }
        if ui.small_button("卡住了？看参考解").clicked() {
            logging::info(&format!("[闯关] 看参考解 level={}", lv.id));
            st.input = lv.solution.to_string();
            check_and_record(st, lv);
        }
    });

    if let Some((ok, why)) = &st.verdict {
        ui.add_space(2.0);
        let rich = egui::RichText::new(if *ok { format!("✓ 通关：{why}") } else { format!("✗ {why}") })
            .color(if *ok { GOOD } else { BAD })
            .monospace();
        ui.label(rich);
        if *ok && !is_last && ui.button("下一关 →").clicked() {
            logging::info(&format!("[闯关] 下一关 {} → {}", lv.id, LEVELS[st.cur + 1].id));
            st.cur += 1;
            st.input.clear();
            st.verdict = None;
        }
    }
}

/// 检查统一入口：先留痕再改状态——崩溃排查要求每次引擎调用可追溯（触发点：
/// 输入变化/检查按钮/参考解）。check 本身保持纯函数（测试也直接用它）。
fn check_and_record(st: &mut State, lv: &Level) {
    let (ok, why) = check(lv, &st.input);
    logging::info(&format!("[闯关] check level={} 输入={:?} → {}", lv.id, st.input, if ok { "✓" } else { "✗" }));
    if !ok {
        logging::info(&format!("[闯关] 原因：{why}"));
    }
    if ok {
        st.solved[st.cur] = true;
    }
    st.verdict = Some((ok, why));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_solvable_by_reference_solution() {
        for lv in LEVELS {
            let (ok, why) = check(lv, lv.solution);
            assert!(ok, "参考解未通关 [{}] {}：{why}", lv.id, lv.title);
        }
    }

    #[test]
    fn wrong_answers_fail() {
        // L0 多杀
        let lv = &LEVELS[0];
        assert!(!check(lv, "cat|dog|cow").0);
        // L6 贪婪一口吞
        let lv6 = LEVELS.iter().find(|l| l.id == "L6").unwrap();
        assert!(!check(lv6, "<.+>").0);
        // Boss：嵌套量词有风险 → 拒绝
        let boss = LEVELS.iter().find(|l| l.id == "Boss").unwrap();
        assert!(!check(boss, "(a+)+b").0);
    }

    #[test]
    fn port_gates() {
        // P1：写 Java 语法 → 拒
        let p1 = LEVELS.iter().find(|l| l.id == "P1").unwrap();
        assert!(!check(p1, r"(?<k>\w+)=(?<v>\w+)").0);
        assert!(check(p1, r"(?P<k>\w+)=(?P<v>\w+)").0);
        // P3：环视进 Go → 拒
        let p3 = LEVELS.iter().find(|l| l.id == "P3").unwrap();
        assert!(!check(p3, r"(\w+)=(\d+)(?=\.)").0);
    }

    #[test]
    fn syntax_helpers() {
        assert!(uses_angle_named("(?<k>x)"));
        assert!(!uses_angle_named("(?<=x)")); // 环视不是命名组
        assert!(!uses_angle_named("(?<!x)"));
        assert!(uses_p_named("(?P<k>x)"));
    }
}
