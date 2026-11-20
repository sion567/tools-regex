//! 执行层引擎矩阵（设计方案 9.1，M3 完整版）。
//!
//! 策略：先 `regex` crate（Rust/Go 家族**真身**，线性时间），编译失败再退
//! `fancy-regex`（回溯家族**近似**，支持环视/反向引用——UI 必须标注"近似引擎"）。
//! 两者都失败 → 带上两个引擎的报错返回（"方言差异高亮"的原料）。
//!
//! 子模块：
//! - [`interp`]：教学解释器（限步回溯 VM + 步迹）→ 回溯时间轴
//! - [`danger`]：灾难性回溯静态检测（嵌套量词）
//! - [`stream`]：mmap + rayon 大文件流式扫描 + 剖析报告

pub mod danger;
pub mod interp;
// 大文件流式扫描（mmap + rayon + 文件系统）属桌面 edition——与 wasm 互斥（设计方案 9.4）
#[cfg(not(target_arch = "wasm32"))]
pub mod stream;

/// 引擎选择策略：自动级联（默认）或强制指定。
/// 强制的意义：两个引擎语义不同（regex 线性时间 DFA 家族，fancy-regex 回溯），
/// 个别写法两边命中集合不一致——强制后"所见即所选引擎"，方便对比差异。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnginePick {
    /// 先 `regex`，编译失败再退 `fancy-regex`
    #[default]
    Auto,
    /// 只用 Rust `regex` crate（不支持环视/反向引用，语法不支持就报错，不回退）
    ForceRegexCrate,
    /// 只用 `fancy-regex`（回溯语义，支持环视/反向引用）
    ForceFancyRegex,
}

/// 实际服务本次匹配的引擎
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Rust `regex` crate —— Rust/Go 家族真身，非近似
    RegexCrate,
    /// `fancy-regex` —— 回溯家族近似引擎（环视/反向引用走这条路）
    FancyRegex,
}

impl Engine {
    pub fn label(&self) -> &'static str {
        match self {
            Engine::RegexCrate => "regex（真身）",
            Engine::FancyRegex => "fancy-regex（近似）",
        }
    }

    pub fn approximate(&self) -> bool {
        matches!(self, Engine::FancyRegex)
    }
}

/// 一次编译、多次使用的扫描器（逐键高亮就靠它，别每行重编译）
pub struct Scanner {
    engine: Option<Engine>,
    error: Option<String>,
    re_regex: Option<regex::Regex>,
    re_fancy: Option<fancy_regex::Regex>,
}

impl Scanner {
    pub fn new(pattern: &str) -> Self {
        let effective = strip_js_literal(pattern);
        match regex::Regex::new(&effective) {
            Ok(re) => Self {
                engine: Some(Engine::RegexCrate),
                error: None,
                re_regex: Some(re),
                re_fancy: None,
            },
            Err(e1) => match fancy_regex::Regex::new(&effective) {
                Ok(re) => Self {
                    engine: Some(Engine::FancyRegex),
                    error: None,
                    re_regex: None,
                    re_fancy: Some(re),
                },
                Err(e2) => Self {
                    engine: None,
                    error: Some(format!("[regex] {e1}\n[fancy-regex] {e2}")),
                    re_regex: None,
                    re_fancy: None,
                },
            },
        }
    }

    /// 按指定策略编译（UI 引擎切换用）。[`Scanner::new`] 等价于 `with_pick(Auto, …)`。
    /// 强制引擎失败就报错——报错里只带被强制引擎的名字，不静默回退。
    pub fn with_pick(pick: EnginePick, pattern: &str) -> Self {
        match pick {
            EnginePick::Auto => Self::new(pattern),
            EnginePick::ForceRegexCrate => {
                let effective = strip_js_literal(pattern);
                match regex::Regex::new(&effective) {
                    Ok(re) => Self {
                        engine: Some(Engine::RegexCrate),
                        error: None,
                        re_regex: Some(re),
                        re_fancy: None,
                    },
                    Err(e) => Self {
                        engine: None,
                        error: Some(format!("[regex] {e}")),
                        re_regex: None,
                        re_fancy: None,
                    },
                }
            }
            EnginePick::ForceFancyRegex => {
                let effective = strip_js_literal(pattern);
                match fancy_regex::Regex::new(&effective) {
                    Ok(re) => Self {
                        engine: Some(Engine::FancyRegex),
                        error: None,
                        re_regex: None,
                        re_fancy: Some(re),
                    },
                    Err(e) => Self {
                        engine: None,
                        error: Some(format!("[fancy-regex] {e}")),
                        re_regex: None,
                        re_fancy: None,
                    },
                }
            }
        }
    }

    pub fn ok(&self) -> bool {
        self.engine.is_some()
    }

    pub fn engine(&self) -> Option<Engine> {
        self.engine
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// 单段文本内找全部命中（字节偏移；空命中已跳过）。
    /// 调用方决定线程策略：workbench 已在后台线程跑（防抖 + 代次守卫）。
    pub fn find_in(&self, text: &str) -> Vec<(usize, usize)> {
        if let Some(re) = &self.re_regex {
            re.find_iter(text)
                .filter(|m| !m.as_str().is_empty())
                .map(|m| (m.start(), m.end()))
                .collect()
        } else if let Some(re) = &self.re_fancy {
            collect_fancy(re, text)
        } else {
            Vec::new()
        }
    }

    /// 替换预览用：整段文本替换，模板支持 $1/$name 引用（regex/fancy-regex 同语法）。
    /// 无命中时原样返回；两引擎都编译失败返回 None。
    pub fn replace_all_in(&self, text: &str, rep: &str) -> Option<String> {
        if let Some(re) = &self.re_regex {
            Some(re.replace_all(text, rep).into_owned())
        } else if let Some(re) = &self.re_fancy {
            Some(re.replace_all(text, rep).into_owned())
        } else {
            None
        }
    }
}

/// 便捷入口：一次调用拿全部结果（内部走 Scanner）
pub fn find_all(pattern: &str, text: &str) -> (Option<Engine>, Option<String>, Vec<(usize, usize)>) {
    let sc = Scanner::new(pattern);
    (sc.engine(), sc.error.as_deref().map(str::to_string), sc.find_in(text))
}

/// JS 风格字面量 `/pattern/flags` 预处理——用户从 JS/Regex101 习惯带来的写法，
/// 直接当正则编译会把 `/` 和 `/g` 当字面字符（实测 `/([A-Z])\w+/g` 一条都搜不出）。
///
/// 规则（保守：只在高度确信是字面量形式时才剥）：
/// - 以 `/` 开头、最后一个 `/` 之后的 flags 是合法 JS flag 子集（`dgimsuvy`，可空）；
/// - 映射：`i/m/s` → 内联标志前缀（两引擎都认，语义与 JS 对应）；`g`（全局，本来就
///   是）/`u`/`d`（Unicode/索引细节，Rust 默认即此行为）忽略；`y`（sticky）无对应
///   语义，直接拒剥——让编译报错明示，绝不静默改变含义。
///
/// 副作用：想匹配字面 `/a/g` 文本请写 `\/a\/g`——与 JS 一致。
fn strip_js_literal(pattern: &str) -> String {
    if !pattern.starts_with('/') || pattern.len() < 3 {
        return pattern.to_string();
    }
    let Some(last) = pattern.rfind('/') else { return pattern.to_string() };
    if last == 0 {
        return pattern.to_string();
    }
    let (inner, flags) = (&pattern[1..last], &pattern[last + 1..]);
    // flags 白名单校验：防误伤以 / 开头的普通正则（如 /home/user 的 "user" 含非 flag 字母）
    if flags.len() > 6 || !flags.bytes().all(|b| b"dgimsuvy".contains(&b)) {
        return pattern.to_string();
    }
    if flags.contains('y') {
        return pattern.to_string(); // sticky 无等价内联标志，宁报错不变形
    }
    let mut prefix = String::new();
    for f in flags.chars() {
        match f {
            'i' | 'm' | 's' => prefix.push_str(&format!("(?{f})")),
            _ => {}
        }
    }
    format!("{prefix}{inner}")
}

/// 一次命中的完整捕获信息（含编号捕获组；groups[0] 对应整段）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRanges {
    pub whole: (usize, usize),
    /// 编号组 1..n 的区间；未参与的组为 None
    pub groups: Vec<Option<(usize, usize)>>,
}

impl Scanner {
    /// 找全部命中并捕获编号组（fixture / 属性表 / 人话层联动用）。
    /// 空命中已跳过。
    pub fn captures_in(&self, text: &str) -> Vec<GroupRanges> {
        if let Some(re) = &self.re_regex {
            re.captures_iter(text)
                .filter(|c| c.get(0).map(|m| !m.as_str().is_empty()).unwrap_or(false))
                .map(|c| GroupRanges {
                    whole: {
                        let m = c.get(0).unwrap();
                        (m.start(), m.end())
                    },
                    groups: (1..c.len()).map(|i| c.get(i).map(|m| (m.start(), m.end()))).collect(),
                })
                .collect()
        } else if let Some(re) = &self.re_fancy {
            let mut out = Vec::new();
            let mut pos = 0usize;
            loop {
                match re.captures_from_pos(text, pos) {
                    Ok(Some(caps)) => {
                        let whole = caps.get(0).expect("captures 必有 group 0");
                        let (s, e) = (whole.start(), whole.end());
                        if e > s {
                            out.push(GroupRanges {
                                whole: (s, e),
                                groups: (1..caps.len()).map(|i| caps.get(i).map(|m| (m.start(), m.end()))).collect(),
                            });
                            pos = e;
                        } else {
                            pos = match text[pos..].chars().next() {
                                Some(c) => pos + c.len_utf8(),
                                None => break,
                            };
                        }
                        if pos >= text.len() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            out
        } else {
            Vec::new()
        }
    }
}

/// fancy-regex 没有面向 &str 的 find_iter（0.13），手动推进游标
fn collect_fancy(re: &fancy_regex::Regex, text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        match re.find_from_pos(text, pos) {
            Ok(Some(m)) => {
                let (s, e) = (m.start(), m.end());
                if e > s {
                    out.push((s, e));
                    pos = e;
                } else {
                    // 空命中：前移一个字符（保持 UTF-8 边界）
                    pos = match text[pos..].chars().next() {
                        Some(c) => pos + c.len_utf8(),
                        None => break,
                    };
                }
                if pos >= text.len() {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break, // 匹配期错误（如回溯深度超限）：截断命中列表（已命中的部分保留）
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn true_engine_for_rust_dialect() {
        let (engine, err, ranges) = find_all(r"ab+c", "xx abbbc yy ab");
        assert_eq!(engine, Some(Engine::RegexCrate));
        assert!(err.is_none());
        assert_eq!(ranges, vec![(3, 8)]); // 末尾的 "ab" 缺 b+，不命中
    }

    #[test]
    fn approx_engine_when_lookahead() {
        // regex 拒绝环视 → 回退 fancy-regex（近似）
        let (engine, _, ranges) = find_all(r"x(?=y)", "xy xz xy");
        assert_eq!(engine, Some(Engine::FancyRegex));
        assert!(engine.unwrap().approximate());
        assert_eq!(ranges, vec![(0, 1), (6, 7)]);
    }

    #[test]
    fn both_fail_reports_both_errors() {
        let sc = Scanner::new("[");
        assert!(!sc.ok());
        let err = sc.error().unwrap();
        assert!(err.contains("[regex]") && err.contains("[fancy-regex]"));
    }

    #[test]
    fn force_regex_rejects_lookahead_without_fallback() {
        // 强制 regex：环视不被支持就报错，绝不静默回退到 fancy-regex
        let sc = Scanner::with_pick(EnginePick::ForceRegexCrate, r"x(?=y)");
        assert!(sc.engine().is_none());
        assert!(sc.error().unwrap().starts_with("[regex]"));
        // 平凡 pattern 下强制 regex 与自动一致
        let sc2 = Scanner::with_pick(EnginePick::ForceRegexCrate, r"ab+c");
        assert_eq!(sc2.engine(), Some(Engine::RegexCrate));
    }

    #[test]
    fn force_fancy_serves_lookaround() {
        let sc = Scanner::with_pick(EnginePick::ForceFancyRegex, r"x(?=y)");
        assert_eq!(sc.engine(), Some(Engine::FancyRegex));
        assert_eq!(sc.find_in("xy xz xy"), vec![(0, 1), (6, 7)]);
        let sc2 = Scanner::with_pick(EnginePick::ForceFancyRegex, r"ab+c");
        assert_eq!(sc2.engine(), Some(Engine::FancyRegex)); // 平凡 pattern 也走 fancy（回溯语义）
    }

    #[test]
    fn scanner_reuse_across_texts() {
        let sc = Scanner::new(r"\d+");
        assert_eq!(sc.find_in("a12b345"), vec![(1, 3), (4, 7)]);
        assert_eq!(sc.find_in("no digits"), vec![]);
    }

    #[test]
    fn backref_via_fancy() {
        let (engine, _, ranges) = find_all(r"\b(\w+) \1\b", "the the cat cat dog");
        assert_eq!(engine, Some(Engine::FancyRegex));
        assert_eq!(ranges, vec![(0, 7), (8, 15)]);
    }

    #[test]
    fn js_literal_g_flag() {
        // 用户实测报障的写法：/…/g 不能把定界符当字面字符
        let (engine, err, ranges) = find_all("/([A-Z])\\w+/g", "Contract signed on Tuesday");
        assert!(err.is_none());
        assert_eq!(engine, Some(Engine::RegexCrate));
        assert_eq!(ranges, vec![(0, 8), (19, 26)]); // Contract / Tuesday（小写开头不命中）
    }

    #[test]
    fn js_literal_flags_map_to_inline() {
        // i → 大小写不敏感；m → 多行；空 flags 也剥
        assert_eq!(find_all("/abc/i", "ABC abc").2, vec![(0, 3), (4, 7)]);
        assert_eq!(find_all("/^b/m", "a\nb").2, vec![(2, 3)]);
        assert_eq!(find_all("/b+/", "abbb").2, vec![(1, 4)]);
    }

    #[test]
    fn js_literal_conservative_reject() {
        // flags 不在 JS 白名单 → 原样保留：/home/user 当字面路径匹配，绝不静默变形
        assert_eq!(find_all("/home/user", "see /home/user now").2, vec![(4, 14)]);
        // sticky(y) 无等价内联语义 → 原样（字面）编译，不硬猜
        assert_eq!(find_all("/a/y", "/a/y").2, vec![(0, 4)]);
    }

    #[test]
    fn replace_all_with_groups() {
        let sc = Scanner::new(r"(\d{4})-(\d{2})-(\d{2})");
        assert_eq!(
            sc.replace_all_in("on 2024-08-15 & 2025-01-01", "$2/$3/$1").unwrap(),
            "on 08/15/2024 & 01/01/2025"
        );
        // 无命中原样返回
        assert_eq!(sc.replace_all_in("no date", "$1").unwrap(), "no date");
        // fancy-regex 路径（含环视）
        let sc2 = Scanner::new(r"\d+(?= ms)");
        assert_eq!(sc2.replace_all_in("123 ms, 45 ms", "<T>").unwrap(), "<T> ms, <T> ms");
        // 双引擎失败 → None
        assert!(Scanner::new("[").replace_all_in("x", "y").is_none());
    }
}
