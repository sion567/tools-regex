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
        match regex::Regex::new(pattern) {
            Ok(re) => Self {
                engine: Some(Engine::RegexCrate),
                error: None,
                re_regex: Some(re),
                re_fancy: None,
            },
            Err(e1) => match fancy_regex::Regex::new(pattern) {
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
}

/// 便捷入口：一次调用拿全部结果（内部走 Scanner）
pub fn find_all(pattern: &str, text: &str) -> (Option<Engine>, Option<String>, Vec<(usize, usize)>) {
    let sc = Scanner::new(pattern);
    (sc.engine(), sc.error.as_deref().map(str::to_string), sc.find_in(text))
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
}
