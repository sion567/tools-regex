//! 正则速查表 —— 参照 debuggex.com 语法卡风格，按方言各一份（本项目支持的语言全覆盖）。
//!
//! 每行：构造分组 / 写法 / 说明 / 支持状态。与「代码导出」的方言矩阵（设计方案 7.5）
//! 同源认知：移植前先查这，知道哪些写法过去、哪些要改写。
//! 状态取自各方言官方文档（RE2 syntax、Python re、Java Pattern、ECMAScript、PCRE、.NET、regex crate）。

use egui::Color32;
use regex_ir::Dialect;

#[derive(Clone, Copy)]
enum Status {
    Yes,
    Warn,
    No,
}

struct Row {
    group: &'static str,
    syntax: &'static str,
    desc: &'static str,
    status: Status,
}

const C_YES: Color32 = Color32::from_rgb(134, 239, 172);
const C_WARN: Color32 = Color32::from_rgb(255, 193, 7);
const C_NO: Color32 = Color32::from_rgb(255, 128, 128);
const C_GROUP: Color32 = Color32::from_rgb(97, 200, 255);
const C_DIM: Color32 = Color32::from_rgb(139, 148, 158);

/// 顶部方言按钮（短名）
fn short(d: Dialect) -> &'static str {
    match d {
        Dialect::Pcre => "PCRE",
        Dialect::Java => "Java",
        Dialect::JavaScript => "JavaScript",
        Dialect::DotNet => ".NET",
        Dialect::Python => "Python",
        Dialect::Go => "Go",
        Dialect::Rust => "Rust",
    }
}

const DIALECTS: [Dialect; 7] = [
    Dialect::JavaScript,
    Dialect::Python,
    Dialect::Java,
    Dialect::Go,
    Dialect::Rust,
    Dialect::Pcre,
    Dialect::DotNet,
];

fn rows(d: Dialect) -> Vec<Row> {
    match d {
        Dialect::JavaScript => js(),
        Dialect::Python => python(),
        Dialect::Java => java(),
        Dialect::Go => go(),
        Dialect::Rust => rust(),
        Dialect::Pcre => pcre(),
        Dialect::DotNet => dotnet(),
    }
}

const fn y(group: &'static str, syntax: &'static str, desc: &'static str) -> Row {
    Row { group, syntax, desc, status: Status::Yes }
}
const fn w(group: &'static str, syntax: &'static str, desc: &'static str) -> Row {
    Row { group, syntax, desc, status: Status::Warn }
}
const fn n(group: &'static str, syntax: &'static str, desc: &'static str) -> Row {
    Row { group, syntax, desc, status: Status::No }
}

fn js() -> Vec<Row> {
    vec![
        y("字符", "abc", "字面量，原样匹配"),
        y("字符", r"\n \r \t", "换行 / 回车 / 制表符"),
        y("字符", r"\f \v \0", "换页 / 垂直制表 / NUL"),
        y("字符", r"\xHH", "两位十六进制字符（\\x41 → A）"),
        y("字符", r"\uHHHH", "四位 Unicode 码点"),
        w("字符", r"\u{HHHH}", "任意码点——需 u 标志"),
        y("字符类", "[abc]", "字符集合：a、b 或 c"),
        y("字符类", "[^abc]", "取反：除 a、b、c 之外"),
        y("字符类", "[a-z]", "范围：a 到 z"),
        w("字符类", ".", "除行终止符外任意字符——s 标志才含 \\n"),
        y("字符类", r"\d \D", "数字 / 非数字"),
        y("字符类", r"\w \W", "单词字符 [A-Za-z0-9_] / 取反"),
        y("字符类", r"\s \S", "空白 / 非空白"),
        w("字符类", r"\p{...}", "Unicode 属性——需 u 标志（如 \\p{L}）"),
        y("锚点", "^ $", "串首 / 串尾——m 标志下按行"),
        y("锚点", r"\b \B", "单词边界 / 非边界"),
        n("锚点", r"\A \z", "JavaScript 不支持绝对首尾锚"),
        y("量词", "* + ? {n} {n,} {n,m}", "标准量词（贪婪）"),
        y("量词", "*? +? ?? {n,m}?", "懒惰（非贪婪）变体"),
        n("量词", "*+ ++", "占有量词——不支持"),
        y("分组", "(...)", "捕获组"),
        y("分组", "(?:...)", "非捕获组"),
        y("分组", "(?<name>...)", "命名捕获组（ES2018+）"),
        n("分组", "(?P<name>...)", "JS 不认 (?P< 写法"),
        y("分组", r"\1 … \9", "反向引用编号组"),
        y("标志", "g i m", "全局 / 忽略大小写 / 多行"),
        w("标志", "s u y d", "dotAll / Unicode / sticky / indices（ES2018~2022）"),
        y("环视", "(?=...) (?!...)", "前瞻 / 负前瞻"),
        w("环视", "(?<=...) (?<!...)", "后顾 / 负后顾——ES2018+"),
        n("特色", "(?>...)", "原子组——不支持"),
        n("特色", r"\K", "重置匹配起点——不支持"),
    ]
}

fn python() -> Vec<Row> {
    vec![
        y("字符", "abc", "字面量，原样匹配"),
        y("字符", r"\n \r \t \f \v", "控制字符转义"),
        y("字符", r"\xHH \uHHHH", "十六进制 / 四位码点（\\UHHHHHHHH 八位）"),
        y("字符类", "[abc] [^abc] [a-z]", "集合 / 取反 / 范围"),
        w("字符类", ".", "除 \\n 外——DOTALL/re.S 标志才含"),
        w("字符类", r"\d \w \s", "默认 Unicode 语义——re.A（ASCII）收窄"),
        n("字符类", r"\p{...}", "re 模块不支持 Unicode 属性类（第三方 regex 库支持）"),
        y("锚点", "^ $", "串首尾——re.M 下按行"),
        y("锚点", r"\A \Z", "绝对串首 / 绝对串尾"),
        y("锚点", r"\b \B", "单词边界 / 非边界"),
        y("量词", "* + ? {n} {n,} {n,m}", "标准量词（贪婪）"),
        y("量词", "*? +? ?? {n,m}?", "懒惰变体"),
        w("量词", "*+ ++ (?>...)", "占有量词 / 原子组——3.11+ 才支持"),
        y("分组", "(...)", "捕获组"),
        y("分组", "(?:...)", "非捕获组"),
        y("分组", "(?P<name>...)", "命名捕获组——Python 的标准写法"),
        n("分组", "(?<name>...)", "Python 不认 (?< 写法（7.2 头号坑的另一面）"),
        y("分组", r"\1 \g<1> \g<name>", "反向引用 / 绝对引用 / 命名引用"),
        y("标志", "(?i) (?m) (?s) (?x)", "re.I/M/S/X 的内联写法"),
        y("环视", "(?=...) (?!...) (?<=...) (?<!...)", "四个方向环视全支持"),
        y("特色", "(?#...)", "注释组"),
        y("特色", "(?(1)yes|no)", "条件组"),
        n("特色", r"\K", "不支持重置匹配起点"),
    ]
}

fn java() -> Vec<Row> {
    vec![
        y("字符", "abc", "字面量，原样匹配"),
        y("字符", r"\n \r \t \f \a \e", "控制字符转义（\\a 响铃 \\e ESC）"),
        y("字符", r"\xHH \uHHHH \x{H…}", "十六进制 / 码点（Java 7+ 支持 \\x{…}）"),
        y("字符类", "[abc] [^abc] [a-z]", "集合 / 取反 / 范围"),
        y("字符类", ".", "除行终止符——DOTALL 标志才含"),
        w("字符类", r"\d \w \s", "默认 ASCII——UNICODE_CHARACTER_CLASS 开 Unicode"),
        y("字符类", r"\p{...}", "Unicode 属性（\\p{IsHan}、\\p{Lu} 等）"),
        y("锚点", "^ $", "串首尾——MULTILINE 下按行"),
        y("锚点", r"\A \Z \z", "绝对首 / 尾（尾换行前）/ 绝对尾——三者齐全"),
        y("锚点", r"\b \B \G", "边界 / 上次匹配尾"),
        y("量词", "* + ? {n} {n,} {n,m}", "标准量词（贪婪）"),
        y("量词", "*? +? ?? {n,m}?", "懒惰变体"),
        y("量词", "*+ ++ ?+ {n,m}+", "占有量词——Java 特有强项"),
        y("分组", "(...)", "捕获组"),
        y("分组", "(?:...) (?<name>...)", "非捕获 / 命名组"),
        n("分组", "(?P<name>...)", "Java 不认 (?P< 写法"),
        y("分组", r"\1 \k<name>", "反向引用 / 命名引用"),
        y("量词", "(?>...)", "原子组"),
        y("标志", "(?idmsux) (?i:...)", "嵌入式标志 / 作用域标志"),
        y("环视", "(?=...) (?!...) (?<=...) (?<!...)", "四个方向环视全支持"),
        y("特色", r"\Q…\E", "引号段：其间全部字面（免转义）"),
    ]
}

fn go() -> Vec<Row> {
    vec![
        y("字符", "abc", "字面量，原样匹配"),
        y("字符", r"\n \r \t \f \v \a", "控制字符转义"),
        y("字符", r"\xHH \x{HHHH} \uHHHH", "十六进制 / 码点"),
        y("字符类", "[abc] [^abc] [a-z]", "集合 / 取反 / 范围"),
        w("字符类", ".", "默认不含 \\n——(?s) 才含"),
        y("字符类", r"\d \w \s", "Unicode 语义（全角数字也算 \\d）"),
        y("字符类", r"\p{...} \p{Han}", "Unicode 属性类（RE2 强项）"),
        y("锚点", "^ $", "串首尾——(?m) 下按行"),
        y("锚点", r"\A \z", "绝对首 / 绝对尾"),
        y("锚点", r"\b \B", "ASCII 单词边界"),
        n("锚点", r"\Z", "RE2 无「尾换行前」锚——用 \\z 或先 trim"),
        y("量词", "* + ? {n} {n,} {n,m}", "标准量词"),
        y("量词", "*? +? ?? {n,m}?", "懒惰变体"),
        n("量词", "*+ ++ (?>...)", "占有/原子——不支持（无回溯，也不需要）"),
        y("分组", "(...) (?:...)", "捕获 / 非捕获组"),
        y("分组", "(?P<name>...)", "命名组——Go 传统写法"),
        w("分组", "(?<name>...)", "Go 1.22+ 也认这种写法"),
        n("分组", r"\1 \k<name>", "反向引用——RE2 不支持（线性时间代价）"),
        y("标志", "(?i) (?m) (?s) (?U)", "i/m/s/非贪婪（U）"),
        n("环视", "(?=...) (?<=...)", "环视全不支持——改捕获+代码校验或换回溯引擎"),
        y("特色", "线性时间", "无回溯：不怕灾难性回溯，性能可预测"),
    ]
}

fn rust() -> Vec<Row> {
    vec![
        y("字符", "abc", "字面量，原样匹配"),
        y("字符", r"\n \r \t \f \v \0", "控制字符转义"),
        y("字符", r"\xHH \x{HHHH} \uHHHH \u{HHHH}", "十六进制 / 码点写法齐全"),
        y("字符类", "[abc] [^abc] [a-z&&[^c]]", "集合 / 取反 / 交集（嵌套类是特色）"),
        w("字符类", ".", "默认不含 \\n——(?s) 才含"),
        y("字符类", r"\d \w \s", "Unicode 语义（re.ASCII 标志收窄）"),
        y("字符类", r"\p{...}", "Unicode 属性类（\\p{Han} 等）"),
        y("锚点", "^ $", "串首尾——(?m) 下按行"),
        y("锚点", r"\A \z", "绝对首 / 绝对尾"),
        y("锚点", r"\b \B", "Unicode 单词边界"),
        n("锚点", r"\Z", "regex crate 无「尾换行前」锚"),
        y("量词", "* + ? {n} {n,} {n,m}", "标准量词"),
        y("量词", "*? +? ?? {n,m}?", "懒惰变体"),
        n("量词", "*+ ++ (?>...)", "占有/原子——regex crate 不支持"),
        w("量词", "(?>...)", "fancy-regex 补原子组（近似引擎，见引擎层）"),
        y("分组", "(...) (?:...)", "捕获 / 非捕获组"),
        y("分组", "(?P<name>...)", "命名组——推荐写法"),
        w("分组", "(?<name>...)", "regex 1.9+ 也认"),
        n("分组", r"\1 \k<name>", "反向引用——不支持（线性时间代价）"),
        y("标志", "(?i) (?m) (?s) (?x) (?U)", "i/m/s/扩展/非贪婪"),
        n("环视", "(?=...) (?<=...)", "环视不支持——fancy-regex 近似支持"),
        y("特色", "线性时间", "无回溯：不怕灾难性回溯"),
    ]
}

fn pcre() -> Vec<Row> {
    vec![
        y("字符", "abc", "字面量，原样匹配"),
        y("字符", r"\n \r \t \f \v \0 \e", "控制字符转义"),
        y("字符", r"\xHH \x{HHHH}", "十六进制（任意位花括号形式）"),
        y("字符类", "[abc] [^abc] [a-z]", "集合 / 取反 / 范围"),
        w("字符类", ".", "默认不含 \\n——(?s) 才含"),
        y("字符类", r"\d \w \s", "⚠ PCRE 默认 ASCII，(*UTF) / u 修饰符开 Unicode"),
        y("字符类", r"\p{...}", "Unicode 属性类"),
        y("锚点", "^ $", "串首尾——(?m) 下按行"),
        y("锚点", r"\A \Z \z \G", "绝对首 / 尾换行前 / 绝对尾 / 上次匹配尾"),
        y("锚点", r"\b \B \K", "边界 / 重置匹配起点"),
        y("量词", "* + ? {n} {n,} {n,m}", "标准量词"),
        y("量词", "*? +? ?? {n,m}?", "懒惰变体"),
        y("量词", "*+ ++ ?+ {n,m}+", "占有量词——PCRE 强项"),
        y("分组", "(...) (?:...)", "捕获 / 非捕获组"),
        y("分组", "(?P<name>) (?<name>) (?'name')", "命名组三种写法全认"),
        y("分组", r"\1 \k<name> \g{1}", "反向引用多种写法"),
        y("量词", "(?>...)", "原子组"),
        y("标志", "(?i) (?m) (?s) (?x) (?J)", "i/m/s/扩展/允许重名组"),
        y("环视", "(?=...) (?!...) (?<=...) (?<!...)", "四个方向环视全支持"),
        y("特色", "(?R) (?1)", "递归 / 子程序调用——PCRE 独门"),
        y("特色", "(?(1)yes|no)", "条件组"),
    ]
}

fn dotnet() -> Vec<Row> {
    vec![
        y("字符", "abc", "字面量，原样匹配"),
        y("字符", r"\n \r \t \f \v \a \e", "控制字符转义"),
        y("字符", r"\xHH \uHHHH", "十六进制 / 码点"),
        y("字符类", "[abc] [^abc] [a-z]", "集合 / 取反 / 范围"),
        w("字符类", ".", "默认不含 \\n——(?s) 才含"),
        y("字符类", r"\d \w \s", "默认 Unicode 语义（ECMAScript 选项收窄）"),
        y("字符类", r"\p{...}", "Unicode 属性类"),
        y("锚点", "^ $", "串首尾——(?m) 下按行"),
        y("锚点", r"\A \Z \z \G", "绝对首 / 尾换行前 / 绝对尾 / 上次匹配尾"),
        y("锚点", r"\b \B", "单词边界"),
        y("量词", "* + ? {n} {n,} {n,m}", "标准量词"),
        y("量词", "*? +? ?? {n,m}?", "懒惰变体"),
        n("量词", "*+ ++", "占有量词写法不支持——用原子组替代"),
        y("量词", "(?>...)", "原子组"),
        y("分组", "(...)", "捕获组"),
        y("分组", "(?<name>) (?'name')", "命名组两种写法"),
        n("分组", "(?P<name>)", ".NET 不认 (?P< 写法"),
        y("分组", r"\1 \k<name>", "反向引用 / 命名引用"),
        y("标志", "(?imnsx) (?i:...)", "嵌入式 / 作用域标志"),
        y("环视", "(?=...) (?<=...)", "四个方向环视全支持"),
        y("特色", "(?<=...)", "后顾可变长——.NET 独有强项（其余引擎基本要求等长）"),
        y("特色", "(?<x-y>...)", "平衡组——栈式配对，.NET 独门"),
    ]
}

pub struct State {
    d: Dialect,
}

impl Default for State {
    fn default() -> Self {
        Self { d: Dialect::JavaScript }
    }
}

pub fn show(ui: &mut egui::Ui, st: &mut State) {
    ui.heading("正则速查表");
    ui.weak("同一构造在各方言下的写法与支持状态——移植前先查这（与「代码导出」方言矩阵同源认知）");
    ui.separator();

    ui.horizontal_wrapped(|ui| {
        for d in DIALECTS {
            if ui.selectable_label(st.d == d, short(d)).clicked() {
                st.d = d;
            }
        }
    });
    ui.add_space(2.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("cheat_grid")
                .striped(true)
                .num_columns(3)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    let mut last_group = "";
                    for r in rows(st.d) {
                        if r.group != last_group {
                            ui.label(egui::RichText::new(r.group).strong().color(C_GROUP));
                            ui.label(egui::RichText::new("").color(C_DIM));
                            ui.label(egui::RichText::new("").color(C_DIM));
                            ui.end_row();
                            last_group = r.group;
                        }
                        ui.monospace(egui::RichText::new(r.syntax).size(13.0));
                        ui.label(egui::RichText::new(r.desc).color(C_DIM));
                        let (mark, color) = match r.status {
                            Status::Yes => ("✓", C_YES),
                            Status::Warn => ("⚠", C_WARN),
                            Status::No => ("✗", C_NO),
                        };
                        ui.label(egui::RichText::new(mark).strong().color(color));
                        ui.end_row();
                    }
                });
        });
}
