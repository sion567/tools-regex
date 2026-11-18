//! 确定性属性 fuzz（设计方案 6.4 测试纪律：随机输入绝不 panic）。
//!
//! 说明：cargo-fuzz/libFuzzer 在 Windows-MSVC 不可用，这里用固定种子的
//! 伪随机生成器做进程内 fuzz（CI 上 Linux 可再加 libFuzzer harness）。
//! 断言：任意字节序列喂给 解析 →（宽松/严格）→ 布局，只允许 Ok 或带 span 的 Err。

use regex_diagram::layout;
use regex_parser::{parse, parse_partial};

/// 固定种子 xorshift：两台机器跑出同一批输入（可复现失败用例）
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// 正则语法字符池 + 少量普通字符/汉字（覆盖 UTF-8 多字节路径）
const POOL: &[&str] = &[
    "a", "b", "c", "0", "9", "_", "中", "文", "(", ")", "[", "]", "{", "}", "|", "*", "+", "?", ".",
    "^", "$", "\\", "-", "<", ">", "=", "!", ":", "P", "p", "k", "i", "m", "s", "x", "d", "w", "b",
    "1", "2", "{2,4}", "(?:", "(?<", "(?P", "(?=", "(?!", "(?<=", "(?<!", "(?>", "(?i", "\\d", "\\w",
    "\\s", "\\b", "\\p{L}", "\\x41", "\\k<", "\\1", "\\Q", "[[:alpha:]]", "\\u", "\x00",
];

fn gen(rng: &mut Rng) -> String {
    let n = (rng.next() % 40) as usize;
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(POOL[(rng.next() % POOL.len() as u64) as usize]);
    }
    s
}

#[test]
fn parse_never_panics_on_random_input() {
    let mut rng = Rng(0xDEADBEEF);
    for i in 0..20_000u32 {
        let s = gen(&mut rng);
        // 严格：Ok 或带 span 的 Err，不许 panic
        let _ = parse(&s);
        // 宽松：必须成功返回（可带错误），且前缀可布局
        let (node, _err) = parse_partial(&s);
        let _ = layout(&node);
        if i % 5_000 == 0 {
            eprintln!("fuzz 进度 {i}：{}", s.escape_debug());
        }
    }
}

#[test]
fn known_nasty_cases_stay_benign() {
    // M0 死循环 bug 的近亲 + 边界残片
    let cases = [
        "a{,2", "a}", "a{", "{", "}", "(?P=", "(?", "(?<", "[", "[^", "\\", "\\x", "\\x{", "\\p{",
        "(?i-", "(a|", "|a", "||", "a||b", "((", "))", "[[:", "[[:x", "(?>", "(?<=", "\\k<",
        "\\k<>", "\\x{}", "\\p{}", "{9999999999999}", "(?P=a",
    ];
    for c in cases {
        let _ = parse(c); // 严格：不 panic 即可
        let (node, _) = parse_partial(c);
        let _ = layout(&node); // 宽松前缀可布局
    }
}

#[test]
fn partial_output_is_always_valid_ir() {
    // 宽松输出的 IR 必须能被布局接受（即 IR 自身良构：无悬空结构）
    let mut rng = Rng(0xCAFE);
    for _ in 0..5_000u32 {
        let s = gen(&mut rng);
        let (node, _) = parse_partial(&s);
        // 布局 panic = IR 良构性破坏
        let d = layout(&node);
        assert!(d.width >= 0.0 && d.height >= 0.0 && d.width.is_finite() && d.height.is_finite());
    }
}
