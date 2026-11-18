//! M1 验收 fixture：设计方案第十二章示例 1/2/4 的可执行化。
//!
//! fixture 位置：<workspace>/fixtures/*.toml；新增示例 = 加一个 toml 文件即进验收。
//! 断言三件事：引擎选择（真身/近似）· 命中区间 · 编号捕获组区间。

use regex_exec::{Engine, Scanner};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    meta: Meta,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Meta {
    name: String,
}

#[derive(Deserialize)]
struct Case {
    #[serde(default)]
    name: String,
    pattern: String,
    sample: String,
    /// "regex"（真身）| "fancy"（fancy-regex 近似）
    expect_engine: String,
    /// 每个命中的整段区间（字节）
    expected_matches: Vec<[usize; 2]>,
    /// 第一个命中的编号捕获组区间（字节）；缺省不断言组
    #[serde(default)]
    expected_groups: Option<Vec<[usize; 2]>>,
}

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

#[test]
fn design_doc_examples_pass() {
    let dir = fixtures_dir();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("fixtures 目录不存在（{:?}）：{e}", dir))
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "toml").unwrap_or(false))
        .collect();
    files.sort_by_key(|e| e.file_name());
    assert!(!files.is_empty(), "fixtures 目录里没有任何 .toml");

    for entry in files {
        let raw = std::fs::read_to_string(entry.path()).unwrap();
        let fx: Fixture = toml::from_str(&raw).unwrap_or_else(|e| panic!("{} 解析失败：{e}", entry.path().display()));
        for case in &fx.cases {
            let label = format!(
                "[{} / {}]",
                fx.meta.name,
                if case.name.is_empty() { "<unnamed>" } else { &case.name }
            );
            let sc = Scanner::new(&case.pattern);
            let engine = sc.engine().unwrap_or_else(|| panic!("{label} 两个引擎都编译失败：{}", sc.error().unwrap_or("")));
            let expected: Engine = match case.expect_engine.as_str() {
                "regex" => Engine::RegexCrate,
                "fancy" => Engine::FancyRegex,
                other => panic!("{label} 未知 expect_engine：{other}"),
            };
            assert_eq!(engine, expected, "{label} 引擎不符（矩阵回退行为变化）");

            let caps = sc.captures_in(&case.sample);
            let wholes: Vec<(usize, usize)> = caps.iter().map(|c| c.whole).collect();
            let expect_wholes: Vec<(usize, usize)> =
                case.expected_matches.iter().map(|m| (m[0], m[1])).collect();
            assert_eq!(wholes, expect_wholes, "{label} 命中区间不符");

            if let Some(groups) = &case.expected_groups {
                let first = caps.first().unwrap_or_else(|| panic!("{label} 无首个命中"));
                let got: Vec<Option<(usize, usize)>> = first.groups.clone();
                let want: Vec<Option<(usize, usize)>> = groups.iter().map(|g| Some((g[0], g[1]))).collect();
                assert_eq!(got, want, "{label} 首个命中的捕获组不符");
            }
        }
        eprintln!("fixture 通过：{}（{} 个 case）", fx.meta.name, fx.cases.len());
    }
}

#[test]
fn invalid_pattern_reports_both_engines() {
    let sc = Scanner::new("[");
    assert!(sc.engine().is_none());
    let err = sc.error().expect("合并报错应存在");
    assert!(err.contains("[regex]") && err.contains("[fancy-regex]"), "报错应含两引擎信息：{err}");
}
