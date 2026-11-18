//! SVG golden 快照：锁定"正则 → IR → 布局 → SVG"全链路输出。
//!
//! 布局是确定性的（有专门测试），因此 SVG 文本可整体对比。
//! 刷新快照：`UPDATE_GOLDEN=1 cargo test -p regex-export --test golden`
//! 刷新后务必人工用浏览器打开看一眼再提交——golden 锁回归，不锁设计。

use regex_diagram::layout;
use regex_export::diagram_to_svg;

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("fixtures/golden/{name}.svg"))
}

fn check_golden(name: &str, pattern: &str) {
    let svg = diagram_to_svg(&layout(&regex_parser::parse(pattern).expect("golden 用例应可解析")));
    let path = golden_path(name);
    if std::env::var("UPDATE_GOLDEN").map(|v| v == "1").unwrap_or(false) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &svg).unwrap();
        eprintln!("golden 已更新：{}", path.display());
        return;
    }
    let saved = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "golden 缺失（{}）：{e}\n首次生成请运行 UPDATE_GOLDEN=1 cargo test -p regex-export --test golden",
            path.display()
        )
    });
    assert_eq!(svg, saved, "{name} 的 SVG 输出与 golden 不符（布局回归！）");
}

#[test]
fn golden_example1_date() {
    check_golden("example1_date", r"\b(\d{4})-(\d{2})-(\d{2})\b");
}

#[test]
fn golden_alternation() {
    check_golden("alternation", "a|bb|c");
}

#[test]
fn greedy_lazy() {
    check_golden("greedy_lazy", r#"(".+?")"#);
}

#[test]
fn lookahead_and_flags() {
    check_golden("lookahead_flags", r"(?i)\d+(?=\.)");
}
