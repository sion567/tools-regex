//! 多语言代码导出 = 方言翻译，不是复制（设计方案 7.5 / 示例 12）。
//!
//! 三件事：
//! 1. **特性门禁**：目标方言 No 的特性 → 拒绝生成"看似能跑实则报错"的代码，
//!    给改写建议——这正是区别于 Regex101"只标红报错"的专业卖点；
//! 2. **命名组语法翻译**：`(?<n>)` ↔ `(?P<n>)` 逐方言改写（Java/JS 与 Python/Go 相反，
//!    是跨语言移植头号低级错误，见示例 3）；
//! 3. **字符串转义翻译**：Java 双重转义、Python raw string、JS `/` 转义、Go raw/引号双路。
//!
//! 局部近似（诚实声明）：Partial 特性（如 JS 后顾 ES2018+）在代码头部注释提醒，不阻断。

use regex_ir::{Dialect, Node, Support};

pub const TARGETS: [Dialect; 6] = [
    Dialect::Java,
    Dialect::Python,
    Dialect::JavaScript,
    Dialect::Go,
    Dialect::Rust,
    Dialect::DotNet,
];

/// 主入口：IR 特性门禁 + 方言改写 → 即用代码片段
pub fn export_code(node: &Node, pattern: &str, dialect: Dialect) -> Result<String, String> {
    // 1) 特性门禁：No → 拒绝；Partial → 提醒
    let mut warnings = Vec::new();
    for f in regex_ir::used_features(node) {
        match regex_ir::supports(dialect, f) {
            Support::No => {
                return Err(format!(
                    "目标方言（{}）不支持 {}。移植向导拒绝生成会报错的代码——请改写：{}",
                    dialect.label(),
                    f.label(),
                    rewrite_advice(f)
                ));
            }
            Support::Partial(note) => warnings.push(format!("{}：{}", f.label(), note)),
            Support::Yes => {}
        }
    }

    // 2) 组信息（编号访问 + 命名访问示例）
    let groups = collect_groups(node);
    let first_named = groups.iter().find_map(|(i, n)| n.clone().map(|n| (*i, n)));

    // 3) 方言改写
    let code = match dialect {
        Dialect::Java => java_snippet(pattern, &groups, first_named.as_ref(), &warnings),
        Dialect::Python => python_snippet(pattern, &groups, first_named.as_ref(), &warnings),
        Dialect::JavaScript => js_snippet(pattern, &groups, first_named.as_ref(), &warnings),
        Dialect::Go => go_snippet(pattern, &groups, first_named.as_ref(), &warnings),
        Dialect::Rust => rust_snippet(pattern, &groups, first_named.as_ref(), &warnings),
        Dialect::DotNet => dotnet_snippet(pattern, &groups, first_named.as_ref(), &warnings),
        // PCRE 是超集基准（无需翻译）
        Dialect::Pcre => {
            return Err(format!("{}：超集基准，无需导出", dialect.label()))
        }
    };
    Ok(code)
}

fn rewrite_advice(f: regex_ir::Feature) -> &'static str {
    match f {
        regex_ir::Feature::Backreference => "捕获后用代码比较（如 Java m.group(1).equals(...)），或改用 DFA 兼容写法",
        regex_ir::Feature::Lookahead => "把断言并入外层匹配后用代码校验，或改用回溯引擎（如 Rust 的 fancy-regex）",
        regex_ir::Feature::Lookbehind => "改为捕获前缀、在代码里截取，或用回溯引擎",
        regex_ir::Feature::PossessiveQuant => "改为普通量词 + 预防灾难回溯的写法（原子组同理，见设计方案示例 8）",
        regex_ir::Feature::AtomicGroup => "改为普通分组或重写结构（见设计方案示例 8 的修复建议）",
        regex_ir::Feature::Recursion => "递归匹配无直接移植——改用外部循环/栈处理嵌套结构",
        regex_ir::Feature::NamedGroupP => "改用 (?<name>) 写法",
        regex_ir::Feature::NamedGroupAngleQ => "改用 (?P<name>) 写法",
    }
}

/// (编号, 命名) 列表，按编号排序
fn collect_groups(node: &Node) -> Vec<(u32, Option<String>)> {
    let mut out = Vec::new();
    walk_groups(node, &mut out);
    out.sort_by_key(|(i, _)| *i);
    out
}

fn walk_groups(node: &Node, out: &mut Vec<(u32, Option<String>)>) {
    match node {
        Node::Group { kind, name, index, child, .. } => {
            if matches!(kind, regex_ir::GroupKind::Capture | regex_ir::GroupKind::Named) {
                if let Some(i) = index {
                    out.push((*i, name.clone()));
                }
            }
            walk_groups(child, out);
        }
        Node::Concat(v) => v.iter().for_each(|c| walk_groups(c, out)),
        Node::Alternation(v) => v.iter().for_each(|c| walk_groups(c, out)),
        Node::Repeat { child, .. } => walk_groups(child, out),
        _ => {}
    }
}

/// 命名组语法翻译：to_p=true → (?P<n>；false → (?<n>。
/// 注意 (?<= / (?<! 是后顾环视，不能误改——逐位扫描识别组名形式。
fn convert_named(pattern: &str, to_p: bool) -> String {
    let b: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == '(' && i + 2 < b.len() && b[i + 1] == '?' {
            if !to_p && b[i + 2] == 'P' && i + 3 < b.len() && b[i + 3] == '<' {
                // (?P<name> → (?<name>
                let mut j = i + 4;
                while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_') {
                    j += 1;
                }
                if j < b.len() && b[j] == '>' {
                    out.push_str("(?<");
                    out.extend(&b[i + 4..j]);
                    out.push('>');
                    i = j + 1;
                    continue;
                }
            } else if to_p && b[i + 2] == '<' {
                // (?<name> → (?P<name>（(?<= / (?<! 是环视，名字位是 = / !，天然不命中）
                let mut j = i + 3;
                while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_') {
                    j += 1;
                }
                if j < b.len() && b[j] == '>' {
                    out.push_str("(?P<");
                    out.extend(&b[i + 3..j]);
                    out.push('>');
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

// ---- 各方言转义 ----

fn java_escape(p: &str) -> String {
    // Java 字符串字面量：\ → \\、" → \"
    let mut out = String::with_capacity(p.len() * 2);
    for c in p.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

/// Python：raw string 引号选择；模式里同时有两种引号时退回普通字符串
fn python_quote(p: &str) -> String {
    let has_dq = p.contains('"');
    let has_sq = p.contains('\'');
    if has_dq && has_sq {
        // 双逃逸普通字符串
        let mut out = String::from("\"");
        for c in p.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    } else if has_dq {
        format!("r'{p}'")
    } else {
        format!("r\"{p}\"")
    }
}

fn js_escape(p: &str) -> String {
    // 正则字面量 /…/ 内：/ → \/（行内字面量法）
    let mut out = String::with_capacity(p.len() + 4);
    for c in p.chars() {
        match c {
            '/' => out.push_str("\\/"),
            _ => out.push(c),
        }
    }
    out
}

/// Go：无反引号 → raw；有反引号 → 解释字符串（\、" 逃逸）
fn go_quote(p: &str) -> String {
    if p.contains('`') {
        let mut out = String::from("\"");
        for c in p.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    } else {
        format!("`{p}`")
    }
}

/// C#：优先 verbatim 字符串 @"..."（反斜杠保持单道）；含引号时退回普通转义
fn dotnet_quote(p: &str) -> String {
    if p.contains('"') {
        let mut out = String::from("\"");
        for c in p.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    } else {
        format!("@\"{p}\"")
    }
}

// ---- 各方言片段 ----

fn group_access_java(named: Option<&(u32, String)>, count: usize) -> String {
    match named {
        Some((i, n)) => format!("String {} = m.group(\"{}\");", n, n)
            + &format!(" // 或 m.group({i})", i = i),
        None if count > 0 => "String g1 = m.group(1);".into(),
        None => String::new(),
    }
}

fn java_snippet(pattern: &str, groups: &[(u32, Option<String>)], named: Option<&(u32, String)>, warnings: &[String]) -> String {
    let pat = java_escape(&convert_named(pattern, false));
    let mut s = String::new();
    for w in warnings {
        s.push_str(&format!("// 注意：{w}\n"));
    }
    s.push_str("// Java（java.util.regex）—— 字符串里 \\ 要写成 \\\\（Java 同学专属痛点，见示例 7）\n");
    s.push_str(&format!("Pattern p = Pattern.compile(\"{pat}\");\n"));
    s.push_str("Matcher m = p.matcher(text);\n");
    s.push_str("while (m.find()) {\n");
    let access = group_access_java(named, groups.len());
    if !access.is_empty() {
        s.push_str(&format!("    {access}\n"));
    }
    s.push('}');
    s
}

fn python_snippet(pattern: &str, groups: &[(u32, Option<String>)], named: Option<&(u32, String)>, warnings: &[String]) -> String {
    let pat = python_quote(&convert_named(pattern, true)); // Python 只认 (?P<n>
    let mut s = String::new();
    for w in warnings {
        s.push_str(&format!("# 注意：{w}\n"));
    }
    s.push_str("# Python（re）——命名组只认 (?P<n>) 写法（拷去 Java 要改成 (?<n>)）\n");
    s.push_str("import re\n\n");
    s.push_str(&format!("p = re.compile({pat})\n"));
    s.push_str("for m in p.finditer(text):\n");
    match named {
        Some((i, n)) => s.push_str(&format!("    {n} = m.group(\"{n}\")  # 或 m.group({i})\n")),
        None if !groups.is_empty() => s.push_str("    g1 = m.group(1)\n"),
        None => s.push_str("    print(m.group(0))\n"),
    }
    s
}

fn js_snippet(pattern: &str, groups: &[(u32, Option<String>)], named: Option<&(u32, String)>, warnings: &[String]) -> String {
    let pat = js_escape(&convert_named(pattern, false));
    let mut s = String::new();
    for w in warnings {
        s.push_str(&format!("// 注意：{w}\n"));
    }
    s.push_str("// JavaScript（ES2018）——无占有量词/原子组；后顾需 ES2018+\n");
    s.push_str(&format!("const re = /{pat}/g;\n"));
    s.push_str("for (const m of text.matchAll(re)) {\n");
    match named {
        Some((_, n)) => s.push_str(&format!("    const {n} = m.groups.{n};\n")),
        None if !groups.is_empty() => s.push_str("    const g1 = m[1];\n"),
        None => s.push_str("    console.log(m[0]);\n"),
    }
    s.push('}');
    s
}

fn go_snippet(pattern: &str, groups: &[(u32, Option<String>)], named: Option<&(u32, String)>, warnings: &[String]) -> String {
    let pat = go_quote(&convert_named(pattern, true)); // Go 只认 (?P<n>
    let mut s = String::new();
    for w in warnings {
        s.push_str(&format!("// 注意：{w}\n"));
    }
    s.push_str("// Go（regexp）——DFA 引擎：无环视/无反向引用；命名组用 (?P<n>)\n");
    s.push_str(&format!("re := regexp.MustCompile({pat})\n\n"));
    s.push_str("m := re.FindStringSubmatch(text)\n");
    s.push_str("if m != nil {\n");
    match named {
        // FindStringSubmatch 的下标就是组号：m[0] 整段，m[i] 第 i 组
        Some((i, n)) => {
            s.push_str(&format!("    {n} := m[{}]\n", *i));
            s.push_str("    // 需要字节区间时：loc := re.FindStringSubmatchIndex(text)，第 i 组在 loc[2i..2i+2]\n");
        }
        None if !groups.is_empty() => s.push_str("    g1 := m[1]\n"),
        None => s.push_str("    fmt.Println(m[0])\n"),
    }
    s.push('}');
    s
}

fn rust_snippet(pattern: &str, groups: &[(u32, Option<String>)], named: Option<&(u32, String)>, warnings: &[String]) -> String {
    let pat = go_quote(&convert_named(pattern, true)); // Rust 两种写法都收，(?P<n> 与 raw string 兼容
    let mut s = String::new();
    for w in warnings {
        s.push_str(&format!("// 注意：{w}\n"));
    }
    s.push_str("// Rust（regex crate）——DFA 引擎：无环视/无反向引用；命名组 (?<n> 与 (?P<n> 均可\n");
    s.push_str("use regex::Regex;\n\n");
    s.push_str(&format!("let re = Regex::new({pat}).unwrap();\n\n"));
    s.push_str("if let Some(caps) = re.captures(text) {\n");
    match named {
        Some((i, n)) => {
            s.push_str(&format!("    let {n} = caps.name(\"{n}\").unwrap().as_str();"));
            s.push_str(&format!(" // 或 caps.get({i})\n"));
        }
        None if !groups.is_empty() => s.push_str("    let g1 = caps.get(1).unwrap().as_str();\n"),
        None => s.push_str("    println!(\"{}\", &caps[0]);\n"),
    }
    s.push('}');
    s
}

fn dotnet_snippet(pattern: &str, groups: &[(u32, Option<String>)], named: Option<&(u32, String)>, warnings: &[String]) -> String {
    let pat = dotnet_quote(&convert_named(pattern, false)); // C# 认 (?<n> 写法
    let mut s = String::new();
    for w in warnings {
        s.push_str(&format!("// 注意：{w}\n"));
    }
    s.push_str("// C# / .NET —— 独有可变长后顾与平衡组；无占有量词语法（防回溯可用原子组或 NonBacktracking）\n");
    s.push_str("using System.Text.RegularExpressions;\n\n");
    s.push_str(&format!("var re = new Regex({pat});\n\n"));
    s.push_str("foreach (Match m in re.Matches(text))\n{\n");
    match named {
        Some((i, n)) => {
            s.push_str(&format!("    var {n} = m.Groups[\"{n}\"].Value;"));
            s.push_str(&format!(" // 或 m.Groups[{i}].Value\n"));
        }
        None if !groups.is_empty() => s.push_str("    var g1 = m.Groups[1].Value;\n"),
        None => s.push_str("    Console.WriteLine(m.Value);\n"),
    }
    s.push('}');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex_parser::parse;

    fn code(pattern: &str, d: Dialect) -> String {
        export_code(&parse(pattern).unwrap(), pattern, d).unwrap()
    }

    fn err(pattern: &str, d: Dialect) -> String {
        export_code(&parse(pattern).unwrap(), pattern, d).unwrap_err()
    }

    #[test]
    fn java_double_escaping() {
        let c = code(r"\b(\d{4})-(\d{2})-(\d{2})\b", Dialect::Java);
        // Java 字符串里 \b 必须写成 \\b（源码即示例 7 的翻译表）
        assert!(c.contains("Pattern.compile(\"\\\\b(\\\\d{4})-"), "{c}");
        assert!(c.contains("m.group(1)"));
    }

    #[test]
    fn python_raw_string() {
        let c = code(r"\b(\d{4})\b", Dialect::Python);
        assert!(c.contains("re.compile(r\"\\b(\\d{4})\\b\")"), "{c}");
        assert!(c.contains("m.group(1)"));
    }

    #[test]
    fn named_group_syntax_translation() {
        let p = r"(?P<k>\w+)=(?<v>\w+)";
        // Java/JS 认 (?<n>，不认 (?P<n>
        let j = code(p, Dialect::Java);
        assert!(j.contains("(?<k>") && j.contains("(?<v>"), "{j}");
        assert!(!j.contains("(?P<"), "{j}");
        assert!(j.contains("m.group(\"k\")"), "{j}");
        // Python/Go 认 (?P<n>，不认 (?<n>
        let py = code(p, Dialect::Python);
        assert!(py.contains("(?P<k>") && py.contains("(?P<v>"), "{py}");
        assert!(!py.contains("(?<v>"), "{py}");
        assert!(py.contains("m.group(\"k\")"));
        let g = code(p, Dialect::Go);
        assert!(g.contains("(?P<k>") && !g.contains("(?<v>"), "{g}");
        // 组 1 的下标就是 1（FindStringSubmatch 语义，曾误写成 m[2*i] 回归锁）
        assert!(g.contains("k := m[1]"), "{g}");
        // JS：m.groups.k
        let js = code(p, Dialect::JavaScript);
        assert!(js.contains("(?<k>") && js.contains("m.groups.k"), "{js}");
        // Rust：caps.name("k")
        let r = code(p, Dialect::Rust);
        assert!(r.contains("caps.name(\"k\")"), "{r}");
    }

    #[test]
    fn dfa_family_refusal() {
        // 反向引用 → Go/Rust 拒绝（示例 4 的红字告警）
        let e = err(r"\b(\w+) \1\b", Dialect::Go);
        assert!(e.contains("反向引用") && e.contains("Go"), "{e}");
        // 环视 → Rust 拒绝（示例 5）
        let e = err(r"\d+(?=\.)", Dialect::Rust);
        assert!(e.contains("前瞻") && e.contains("Rust"), "{e}");
        // 回溯家族放行
        assert!(code(r"\b(\w+) \1\b", Dialect::Java).contains("Pattern.compile"));
    }

    #[test]
    fn js_slash_escaping() {
        let c = code("a/b", Dialect::JavaScript);
        assert!(c.contains("/a\\/b/g"), "{c}");
    }

    #[test]
    fn partial_feature_warns_not_blocks() {
        // JS 后顾 ES2018+ → Partial：生成但带提醒注释（不阻断）
        let c = code(r"(?<=\$)\d+", Dialect::JavaScript);
        assert!(c.contains("注意") && c.contains("ES2018"), "{c}");
        // 命名组写法是源语法层概念，IR 不保留——提醒由 UI 层负责
        let c = code(r"(?<k>\w+)", Dialect::JavaScript);
        assert!(c.contains("m.groups.k"), "{c}");
    }

    #[test]
    fn no_groups_fallback() {
        let c = code("dog", Dialect::Rust);
        assert!(c.contains("caps[0]"), "{c}");
    }

    #[test]
    fn dotnet_named_and_verbatim() {
        let c = code(r"(?<k>\w+)=(?<v>\w+)", Dialect::DotNet);
        // verbatim 字符串：反斜杠保持单道；命名组 (?<n> 原生；Groups["k"] 访问
        assert!(c.contains("new Regex(@\"(?<k>\\w+)=(?<v>\\w+)\")"), "{c}");
        assert!(c.contains("m.Groups[\"k\"].Value"), "{c}");
    }

    #[test]
    fn dotnet_possessive_refused() {
        // .NET 无占有量词语法 → 拒绝 + 建议（7.2 矩阵勘误后的门禁）
        let e = err("a++", Dialect::DotNet);
        assert!(e.contains("占有量词") && e.contains(".NET"), "{e}");
        // 但原子组放行（.NET 原生支持 (?>)）
        assert!(code("(?>a+)b", Dialect::DotNet).contains("new Regex"));
    }
}
