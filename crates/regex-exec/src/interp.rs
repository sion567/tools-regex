//! 教学解释器：限步数的回溯 VM（设计方案 9.1 引擎矩阵第三行 / 第十章 M3）。
//!
//! 定位：**不是性能引擎**，是"看见引擎在做什么"的教学工具——每条指令执行记一步，
//! Split 记选择点、回退记 Backtrack，供回溯时间轴逐帧回放（RegexBuddy 式体验的核心）。
//!
//! 安全边界（设计方案 9.3）：文本 ≤4096 字节、步数预算（默认 2 万步）——预算耗尽即停。
//! 这本身就是 ReDoS 演示的保险丝：灾难性回溯 = 预算打满 + Backtrack 海啸。
//!
//! 语义近似（诚实声明）：
//! - 占有量词 / 原子组：不支持（编译期报错），演示用普通量词
//! - 后顾环视：不支持（逆向匹配超出教学解释器范围），前瞻支持
//! - 作用域标志 `(?i:...)`：按全局近似；`(?i)`/`(?m)` 全局生效
//! - 组编号捕获：支持；命名反向引用经 名→号 映射支持

use std::collections::HashMap;

use regex_ir::{AnchorKind, BackrefKind, ClassItem, GroupKind, Node, PerlClass, RepKind};

/// 默认步数预算（ReDoS 演示的保险丝）
pub const DEFAULT_BUDGET: u32 = 20_000;
/// 教学解释器最大文本（字节）
pub const MAX_TEXT_BYTES: usize = 4096;
/// 量词展开上限（min 份数 / 可选份数各计）
const MAX_EXPAND: u32 = 64;

// ---------- 编译：IR → 字节码 ----------

#[derive(Debug, Clone)]
#[allow(dead_code)] // end 字段仅编译期使用（子程序边界），运行期读 start 即可
enum Inst {
    Char(char),
    Class { negated: bool, items: Vec<ClassItem> },
    Split { x: usize, y: usize },
    Jmp(usize),
    Save(usize),
    Assert(AssertKind),
    Backref(usize),
    Look { start: usize, end: usize, negated: bool },
    Match,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssertKind {
    StartText,
    EndText,
    StartLine,
    EndLine,
    WordB,
    NotWordB,
}

/// 编译产物（含逐条指令的可读标签，时间轴回放用）
pub struct Prog {
    insts: Vec<Inst>,
    pub labels: Vec<String>,
    n_slots: usize,
    icase: bool,
}

fn collect_names(node: &Node, map: &mut HashMap<String, u32>) {
    match node {
        Node::Group { kind, name, index, child, .. } => {
            if *kind == GroupKind::Named {
                if let (Some(n), Some(i)) = (name, index) {
                    map.insert(n.clone(), *i);
                }
            }
            collect_names(child, map);
        }
        Node::Concat(v) => v.iter().for_each(|c| collect_names(c, map)),
        Node::Alternation(v) => v.iter().for_each(|c| collect_names(c, map)),
        Node::Repeat { child, .. } => collect_names(child, map),
        _ => {}
    }
}

fn scan_flags(node: &Node, icase: &mut bool, multiline: &mut bool) {
    match node {
        Node::Flags { set, unset, .. } => {
            // 全局近似：set 与 unset 都按"最后声明者生效"折算成全局开关
            for c in set.chars() {
                apply_flag(c, false, icase, multiline);
            }
            for c in unset.chars() {
                apply_flag(c, true, icase, multiline);
            }
        }
        Node::Group { flags, child, .. } => {
            // 作用域标志 (?i:...) 按全局近似（头注释声明）：原文 "im-s" 里 '-' 后为 unset
            if let Some(raw) = flags {
                let mut neg = false;
                for c in raw.chars() {
                    match c {
                        '-' => neg = true,
                        _ => apply_flag(c, neg, icase, multiline),
                    }
                }
            }
            scan_flags(child, icase, multiline);
        }
        Node::Concat(v) => v.iter().for_each(|c| scan_flags(c, icase, multiline)),
        Node::Alternation(v) => v.iter().for_each(|c| scan_flags(c, icase, multiline)),
        Node::Repeat { child, .. } => scan_flags(child, icase, multiline),
        _ => {}
    }
}

/// 单个标志字符落到全局开关（unset=true 表示 `(?-i)` / `-` 之后的字母）
fn apply_flag(c: char, unset: bool, icase: &mut bool, multiline: &mut bool) {
    match c {
        'i' => *icase = !unset,
        'm' => *multiline = !unset,
        _ => {}
    }
}

struct Ctx {
    insts: Vec<Inst>,
    labels: Vec<String>,
    max_group: u32,
    names: HashMap<String, u32>,
    multiline: bool,
}

impl Ctx {
    fn push(&mut self, inst: Inst, label: String) -> usize {
        self.insts.push(inst);
        self.labels.push(label);
        self.insts.len() - 1
    }
}

fn compile(node: &Node) -> Result<Prog, String> {
    let mut icase = false;
    let mut multiline = false;
    scan_flags(node, &mut icase, &mut multiline);
    let mut names = HashMap::new();
    collect_names(node, &mut names);
    let mut c = Ctx { insts: Vec::new(), labels: Vec::new(), max_group: 0, names, multiline };
    emit(&mut c, node)?;
    c.push(Inst::Match, "Match".into());
    let n_slots = (2 + 2 * c.max_group) as usize;
    Ok(Prog { insts: c.insts, labels: c.labels, n_slots, icase })
}

fn emit(c: &mut Ctx, node: &Node) -> Result<(), String> {
    match node {
        Node::Empty => {}

        Node::Literal { text, .. } => {
            for ch in text.chars() {
                c.push(Inst::Char(ch), format!("Char {ch:?}"));
            }
        }

        Node::Class { negated, items, .. } => {
            c.push(Inst::Class { negated: *negated, items: items.clone() }, "字符类".into());
        }

        Node::Anchor { kind, .. } => {
            let k = anchor_to_assert(*kind, c.multiline);
            let label = match k {
                AssertKind::StartText => "^ 文本开头".into(),
                AssertKind::EndText => "$ 文本末尾".into(),
                AssertKind::StartLine => "^ 行首".into(),
                AssertKind::EndLine => "$ 行尾".into(),
                AssertKind::WordB => "\\b 词边界".into(),
                AssertKind::NotWordB => "\\B 非词边界".into(),
            };
            c.push(Inst::Assert(k), label);
        }

        Node::Flags { .. } => {} // 全局近似，已在 compile 扫描

        Node::Backref { kind, .. } => {
            let idx = match kind {
                BackrefKind::Index(i) => *i,
                BackrefKind::Name(n) => c
                    .names
                    .get(n)
                    .copied()
                    .ok_or_else(|| format!("反向引用了不存在的组「{n}」"))?,
            };
            if idx > c.max_group {
                return Err(format!("反向引用 \\{idx} 超出组数"));
            }
            c.push(Inst::Backref(idx as usize), format!("Backref \\{idx}"));
        }

        Node::Group { kind, index, child, .. } => match kind {
            GroupKind::Atomic => {
                return Err("教学解释器暂不支持原子组 (?>...)——请用普通分组演示回溯".into());
            }
            GroupKind::Lookbehind { .. } => {
                return Err("教学解释器暂不支持后顾环视（逆向匹配）——前瞻 (?=)/(?!) 可用".into());
            }
            GroupKind::Capture | GroupKind::Named => {
                let i = index.ok_or("捕获组缺编号")?;
                c.max_group = c.max_group.max(i);
                c.push(Inst::Save((2 * i) as usize), format!("开组 {i}"));
                emit(c, child)?;
                c.push(Inst::Save((2 * i + 1) as usize), format!("闭组 {i}"));
            }
            GroupKind::NonCapturing => emit(c, child)?,
            GroupKind::Lookahead { negative } => {
                let idx = c.insts.len();
                c.push(Inst::Jmp(0), "?".into()); // 占位 → Look
                let j = c.push(Inst::Jmp(0), "?".into()); // 占位 → 跳过子程序体（主流程续点）
                let s = c.insts.len();
                emit(c, child)?;
                c.push(Inst::Match, "环视内 Match".into());
                let e = c.insts.len();
                c.insts[idx] = Inst::Look { start: s, end: e, negated: *negative };
                c.labels[idx] = if *negative { "负前瞻 (?!)".into() } else { "前瞻 (?=)".into() };
                c.insts[j] = Inst::Jmp(e);
                c.labels[j] = "跳过子程序".into();
            }
        },

        Node::Repeat { child, min, max, rep, .. } => {
            if *rep == RepKind::Possessive {
                return Err("教学解释器暂不支持占有量词 a*+——请用普通量词演示回溯".into());
            }
            let min = *min;
            let max = *max;
            if min > MAX_EXPAND {
                return Err(format!("重复下界 {min} 超出教学解释器上限 {MAX_EXPAND}"));
            }
            let lazy = *rep == RepKind::Lazy;
            for _ in 0..min {
                emit(c, child)?;
            }
            match max {
                None => {
                    // 星号循环：Split(body, exit) 贪婪 / Split(exit, body) 懒惰
                    let l1 = c.insts.len();
                    c.push(Inst::Jmp(0), "?".into());
                    let body = c.insts.len();
                    emit(c, child)?;
                    c.push(Inst::Jmp(l1), format!("Jmp {l1}"));
                    let exit = c.insts.len();
                    c.insts[l1] = if lazy {
                        Inst::Split { x: exit, y: body }
                    } else {
                        Inst::Split { x: body, y: exit }
                    };
                    c.labels[l1] = format!(
                        "Split {}|{}（{}）",
                        if lazy { exit } else { body },
                        if lazy { body } else { exit },
                        if lazy { "懒" } else { "贪" }
                    );
                }
                Some(n) => {
                    let extra = n.saturating_sub(min);
                    if extra > MAX_EXPAND {
                        return Err(format!("重复上界 {n} 超出教学解释器上限 {MAX_EXPAND}"));
                    }
                    emit_optional(c, child, extra, lazy)?;
                }
            }
        }

        Node::Concat(children) => {
            for ch in children {
                emit(c, ch)?;
            }
        }

        Node::Alternation(branches) => {
            let mut jmps = Vec::new();
            for (i, b) in branches.iter().enumerate() {
                if i + 1 < branches.len() {
                    let sp = c.insts.len();
                    c.push(Inst::Jmp(0), "?".into());
                    emit(c, b)?;
                    let j = c.push(Inst::Jmp(0), "?".into());
                    jmps.push(j);
                    let next = c.insts.len();
                    c.insts[sp] = Inst::Split { x: sp + 1, y: next };
                    c.labels[sp] = format!("Split 分支{}|分支{}", i + 1, i + 2);
                } else {
                    emit(c, b)?;
                }
            }
            let end = c.insts.len();
            for j in jmps {
                c.insts[j] = Inst::Jmp(end);
                c.labels[j] = format!("Jmp {end}（汇合）");
            }
        }
    }
    Ok(())
}

/// 嵌套可选段：e{m,n} 的 (n-m) 个可选拷贝，任一层可全部跳过。
/// Split 的两个去向：body 起点（sp+1）与 exit；body 走完自然落到下一层/exit。
fn emit_optional(c: &mut Ctx, child: &Node, count: u32, lazy: bool) -> Result<usize, String> {
    if count == 0 {
        return Ok(c.insts.len());
    }
    let sp = c.insts.len();
    c.push(Inst::Jmp(0), "?".into());
    let body = c.insts.len();
    emit(c, child)?;
    let exit = emit_optional(c, child, count - 1, lazy)?;
    c.insts[sp] = if lazy {
        Inst::Split { x: exit, y: body }
    } else {
        Inst::Split { x: body, y: exit }
    };
    c.labels[sp] = format!(
        "Split {}|{}（{}）",
        if lazy { exit } else { body },
        if lazy { body } else { exit },
        if lazy { "懒" } else { "贪" }
    );
    Ok(exit)
}

/// 锚点 → 断言。非多行模式下 `^`/`$` 退化为文本首/尾（与 (?m) 语义一致）。
fn anchor_to_assert(kind: AnchorKind, multiline: bool) -> AssertKind {
    match kind {
        AnchorKind::StartLine if !multiline => AssertKind::StartText,
        AnchorKind::EndLine if !multiline => AssertKind::EndText,
        AnchorKind::StartLine => AssertKind::StartLine,
        AnchorKind::EndLine => AssertKind::EndLine,
        AnchorKind::StartText => AssertKind::StartText,
        AnchorKind::EndText => AssertKind::EndText,
        AnchorKind::WordBoundary => AssertKind::WordB,
        AnchorKind::NotWordBoundary => AssertKind::NotWordB,
    }
}

// ---------- 匹配原语 ----------

fn char_eq(a: char, b: char, icase: bool) -> bool {
    a == b || (icase && a.to_lowercase().eq(b.to_lowercase()))
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn perl_match(p: PerlClass, c: char) -> bool {
    match p {
        PerlClass::Digit => c.is_numeric(),
        PerlClass::NotDigit => !c.is_numeric(),
        PerlClass::Word => is_word(c),
        PerlClass::NotWord => !is_word(c),
        PerlClass::Space => c.is_whitespace(),
        PerlClass::NotSpace => !c.is_whitespace(),
    }
}

fn item_match(item: &ClassItem, ch: char, icase: bool) -> bool {
    match item {
        ClassItem::Char(x) => char_eq(*x, ch, icase),
        ClassItem::Range(a, b) => {
            (*a <= ch && ch <= *b)
                || (icase
                    && ch.to_lowercase().chain(ch.to_uppercase()).any(|f| *a <= f && f <= *b))
        }
        ClassItem::Perl(p) => perl_match(*p, ch),
        ClassItem::Posix { name, negated } => {
            let hit = match name.as_str() {
                "alpha" => ch.is_alphabetic(),
                "digit" => ch.is_numeric(),
                "alnum" => ch.is_alphanumeric(),
                "space" => ch.is_whitespace(),
                "upper" => ch.is_uppercase(),
                "lower" => ch.is_lowercase(),
                "punct" => ch.is_ascii_punctuation(),
                "xdigit" => ch.is_ascii_hexdigit(),
                _ => false,
            } != *negated;
            hit
        }
        ClassItem::Unicode(name) => match name.as_str() {
            "L" => ch.is_alphabetic(),
            "N" => ch.is_numeric(),
            "Lu" => ch.is_uppercase(),
            "Ll" => ch.is_lowercase(),
            _ => false, // 近似：教学演示覆盖常用类
        },
    }
}

fn class_match(items: &[ClassItem], negated: bool, ch: char, icase: bool) -> bool {
    let mut hit = items.iter().any(|it| item_match(it, ch, icase));
    if !hit && icase {
        let mut alt = ch.to_lowercase().chain(ch.to_uppercase());
        hit = alt.any(|f| items.iter().any(|it| item_match(it, f, icase)));
    }
    hit != negated
}

fn assert_ok(k: AssertKind, chars: &[char], pos: usize) -> bool {
    let n = chars.len();
    // 词边界两侧：文本开头/结尾视作非词字符（M1 实测修过：pos=0 的左邻不能取当前字符）
    let left = pos > 0 && is_word(chars[pos - 1]);
    let right = pos < n && is_word(chars[pos]);
    match k {
        AssertKind::StartText => pos == 0,
        AssertKind::EndText => pos == n,
        AssertKind::StartLine => pos == 0 || chars[pos - 1] == '\n',
        AssertKind::EndLine => pos == n || chars[pos] == '\n',
        AssertKind::WordB => left != right,
        AssertKind::NotWordB => left == right,
    }
}

// ---------- 执行 + 步迹 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ev {
    /// 消费了一个字符（Char/Class）
    Advance,
    /// 遇到选择点（Split）
    Choice,
    /// 纯指令（Save/Jmp/断言/引用）
    Pure,
    /// 进入环视子执行
    Lookaround,
    /// 回退（从选择栈恢复）
    Backtrack,
    /// 整体匹配成功
    Match,
}

impl Ev {
    pub fn label(&self) -> &'static str {
        match self {
            Ev::Advance => "前进",
            Ev::Choice => "选择",
            Ev::Pure => "指令",
            Ev::Lookaround => "环视",
            Ev::Backtrack => "回退",
            Ev::Match => "命中",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub pc: u32,
    /// 文本位置（char 下标）
    pub pos: u32,
    /// 回溯栈深（含环视嵌套层）
    pub depth: u32,
    pub ev: Ev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Matched,
    NoMatch,
    /// 预算耗尽（灾难性回溯的表现形式）
    BudgetExceeded,
}

#[derive(Debug, Clone)]
pub struct InterpResult {
    pub verdict: Verdict,
    pub steps: Vec<Step>,
    pub exceeded_budget: bool,
    /// 每次尝试（起始位置，消耗步数）——"为什么没匹配上"的答案
    pub restarts: Vec<(usize, u32)>,
    /// 命中区间（char 下标）
    pub match_span: Option<(usize, usize)>,
    /// 编号捕获组区间（char 下标）；[0] 为整段
    pub slots: Vec<Option<(usize, usize)>>,
    pub max_depth: u32,
    pub budget: u32,
    /// 逐条指令可读标签（时间轴回放显示"当前在哪条指令"）
    pub labels: Vec<String>,
}

impl InterpResult {
    pub fn count_of(&self, ev: Ev) -> usize {
        self.steps.iter().filter(|s| s.ev == ev).count()
    }
}

enum Attempt {
    Matched(Vec<Option<usize>>),
    Failed,
    Budget,
}

struct Thread {
    pc: usize,
    pos: usize,
    slots: Vec<Option<usize>>,
}

/// 运行教学解释器：从每个起始位置尝试（非锚定语义），共享步数预算。
pub fn run(node: &Node, text: &str, budget: u32) -> Result<InterpResult, String> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "教学解释器仅支持 ≤{MAX_TEXT_BYTES} 字节文本（当前 {} 字节）",
            text.len()
        ));
    }
    let prog = compile(node)?;
    let chars: Vec<char> = text.chars().collect();
    let mut steps = Vec::new();
    let mut budget_left = budget;
    let mut restarts = Vec::new();
    let mut max_depth = 0u32;

    for start in 0..=chars.len() {
        let before = steps.len();
        match attempt(&prog, &chars, 0, start, &mut steps, &mut budget_left, &mut max_depth, 0) {
            Attempt::Matched(slots) => {
                restarts.push((start, (steps.len() - before) as u32));
                let end = slots.get(1).copied().flatten().unwrap_or(start);
                let mut sp = Vec::new();
                let mut i = 0;
                while 2 * i + 1 < slots.len() {
                    sp.push(pair(&slots, 2 * i));
                    i += 1;
                }
                return Ok(InterpResult {
                    verdict: Verdict::Matched,
                    steps,
                    exceeded_budget: false,
                    restarts,
                    match_span: Some((start, end)),
                    slots: sp,
                    max_depth,
                    budget,
                    labels: prog.labels.clone(),
                });
            }
            Attempt::Failed => restarts.push((start, (steps.len() - before) as u32)),
            Attempt::Budget => {
                restarts.push((start, (steps.len() - before) as u32));
                return Ok(InterpResult {
                    verdict: Verdict::BudgetExceeded,
                    steps,
                    exceeded_budget: true,
                    restarts,
                    match_span: None,
                    slots: vec![],
                    max_depth,
                    budget,
                    labels: prog.labels.clone(),
                });
            }
        }
    }
    Ok(InterpResult {
        verdict: Verdict::NoMatch,
        steps,
        exceeded_budget: false,
        restarts,
        match_span: None,
        slots: vec![],
        max_depth,
        budget,
        labels: prog.labels.clone(),
    })
}

fn pair(slots: &[Option<usize>], i: usize) -> Option<(usize, usize)> {
    let a = slots.get(i).copied().flatten()?;
    let b = slots.get(i + 1).copied().flatten()?;
    if a <= b {
        Some((a, b))
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn attempt(
    prog: &Prog,
    chars: &[char],
    entry: usize,
    start: usize,
    steps: &mut Vec<Step>,
    budget_left: &mut u32,
    max_depth: &mut u32,
    depth: u32,
) -> Attempt {
    let n = chars.len();
    let mut t = Thread { pc: entry, pos: start, slots: vec![None; prog.n_slots] };
    t.slots[0] = Some(start);
    let mut stack: Vec<Thread> = Vec::new();

    loop {
        if *budget_left == 0 {
            return Attempt::Budget;
        }
        *budget_left -= 1;
        let d = stack.len() as u32 + depth;
        if d > *max_depth {
            *max_depth = d;
        }
        let (pc, pos) = (t.pc, t.pos);
        macro_rules! step {
            ($ev:expr) => {
                steps.push(Step { pc: pc as u32, pos: pos as u32, depth: d, ev: $ev })
            };
        }
        macro_rules! fail {
            () => {{
                if let Some(prev) = stack.pop() {
                    steps.push(Step {
                        pc: prev.pc as u32,
                        pos: prev.pos as u32,
                        depth: stack.len() as u32 + depth,
                        ev: Ev::Backtrack,
                    });
                    t = prev;
                } else {
                    return Attempt::Failed;
                }
            }};
        }
        match &prog.insts[pc] {
            Inst::Char(ch) => {
                step!(Ev::Advance);
                if pos < n && char_eq(chars[pos], *ch, prog.icase) {
                    t.pos += 1;
                    t.pc += 1;
                } else {
                    fail!();
                }
            }
            Inst::Class { negated, items } => {
                step!(Ev::Advance);
                if pos < n && class_match(items, *negated, chars[pos], prog.icase) {
                    t.pos += 1;
                    t.pc += 1;
                } else {
                    fail!();
                }
            }
            Inst::Split { x, y } => {
                step!(Ev::Choice);
                stack.push(Thread { pc: *y, pos, slots: t.slots.clone() });
                t.pc = *x;
            }
            Inst::Jmp(x) => {
                step!(Ev::Pure);
                t.pc = *x;
            }
            Inst::Save(s) => {
                step!(Ev::Pure);
                if *s < t.slots.len() {
                    t.slots[*s] = Some(pos);
                }
                t.pc += 1;
            }
            Inst::Assert(k) => {
                step!(Ev::Pure);
                if assert_ok(*k, chars, pos) {
                    t.pc += 1;
                } else {
                    fail!();
                }
            }
            Inst::Backref(i) => {
                step!(Ev::Pure);
                let open = t.slots.get(2 * i).copied().flatten();
                let close = t.slots.get(2 * i + 1).copied().flatten();
                match (open, close) {
                    (Some(a), Some(b)) if a <= b => {
                        let len = b - a;
                        if pos + len <= n
                            && (0..len).all(|k| char_eq(chars[pos + k], chars[a + k], prog.icase))
                        {
                            t.pos += len;
                            t.pc += 1;
                        } else {
                            fail!();
                        }
                    }
                    _ => fail!(), // 组未参与匹配 → 失败（PCRE 语义）
                }
            }
            Inst::Look { start: s, negated, .. } => {
                step!(Ev::Lookaround);
                match attempt(prog, chars, *s, pos, steps, budget_left, max_depth, depth + 1) {
                    Attempt::Budget => return Attempt::Budget,
                    Attempt::Matched(sub) => {
                        if *negated {
                            fail!();
                        } else {
                            // 环视内的捕获组并入主线程槽位（槽 0/1 属主流程，不动）
                            for (k, v) in sub.iter().enumerate().skip(2) {
                                if v.is_some() {
                                    t.slots[k] = *v;
                                }
                            }
                            t.pc += 1;
                        }
                    }
                    Attempt::Failed => {
                        if *negated {
                            t.pc += 1;
                        } else {
                            fail!();
                        }
                    }
                }
            }
            Inst::Match => {
                step!(Ev::Match);
                t.slots[1] = Some(pos);
                return Attempt::Matched(std::mem::take(&mut t.slots));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex_parser::parse;

    fn go(pattern: &str, text: &str) -> InterpResult {
        run(&parse(pattern).unwrap(), text, DEFAULT_BUDGET).unwrap()
    }

    #[test]
    fn literal_and_restart() {
        let r = go("abc", "xxabcxx");
        assert_eq!(r.verdict, Verdict::Matched);
        assert_eq!(r.match_span, Some((2, 5)));
        assert_eq!(r.restarts.len(), 3); // 0 失败、1 失败、2 命中
    }

    #[test]
    fn alternation_leftmost_first() {
        // 左优先语义：`a|ab` 命中 "a" 而非 "ab"
        let r = go("a|ab", "ab");
        assert_eq!(r.match_span, Some((0, 1)));
    }

    #[test]
    fn greedy_star_and_lazy() {
        let r = go("a*", "aaa");
        assert_eq!(r.match_span, Some((0, 3)));
        // 懒惰：够就走
        let r = go("a+?", "aaa");
        assert_eq!(r.match_span, Some((0, 1)));
    }

    #[test]
    fn captures_and_backref() {
        let r = go(r"\b(\w+) \1\b", "the the");
        assert_eq!(r.verdict, Verdict::Matched);
        assert_eq!(r.match_span, Some((0, 7)));
        assert_eq!(r.slots[1], Some((0, 3)));
        // 反向引用不匹配 → 后面重启也失败
        let r = go(r"\b(\w+) \1\b", "the cat");
        assert_eq!(r.verdict, Verdict::NoMatch);
    }

    #[test]
    fn lookahead_zero_width() {
        let r = go(r"\d+(?=\.)", "123.x");
        assert_eq!(r.match_span, Some((0, 3))); // 不吃掉点
        let r = go(r"x(?=y)", "xy xz");
        assert_eq!(r.match_span, Some((0, 1)));
    }

    #[test]
    fn repeat_range_expand() {
        let r = go("a{2,4}", "aaaaa");
        assert_eq!(r.match_span, Some((0, 4))); // 贪婪拿满 4
        let r = go("a{2,4}?", "aaaaa");
        assert_eq!(r.match_span, Some((0, 2))); // 懒惰拿 2
        let r = go("a{3}", "aa");
        assert_eq!(r.verdict, Verdict::NoMatch);
    }

    #[test]
    fn catastrophic_backtracking_hits_budget() {
        // (a+)+b 喂 30 个 a 无 b：指数回溯 → 预算打满（ReDoS 教学演示）
        let text = "a".repeat(30);
        let r = go("(a+)+b", &text);
        assert_eq!(r.verdict, Verdict::BudgetExceeded);
        assert!(r.exceeded_budget);
        // 大量 Backtrack 步
        assert!(r.count_of(Ev::Backtrack) > 1000);
    }

    #[test]
    fn catastrophic_pattern_matches_when_input_kind() {
        let text = format!("{}b", "a".repeat(20));
        let r = go("(a+)+b", &text);
        assert_eq!(r.verdict, Verdict::Matched);
        assert_eq!(r.match_span, Some((0, 21)));
    }

    #[test]
    fn unsupported_constructs_error() {
        assert!(run(&parse("(?>a+)b").unwrap(), "ab", 100).is_err());
        assert!(run(&parse("a*+b").unwrap(), "ab", 100).is_err());
        assert!(run(&parse("(?<=a)b").unwrap(), "ab", 100).is_err());
    }

    #[test]
    fn text_cap() {
        let big = "a".repeat(5000);
        let r = run(&parse("a").unwrap(), &big, 100);
        assert!(r.is_err());
    }

    #[test]
    fn icase_and_multiline() {
        let r = go("(?i)abc", "xABCx");
        assert_eq!(r.match_span, Some((1, 4)));
        let r = go("(?m)^b", "a\nb");
        assert_eq!(r.match_span, Some((2, 3)));
        // 无 (?m)：^ 只认文本开头（此前 ^ 恒为行首，与 L5 教学语义矛盾——回归锁）
        let r = go("^b", "a\nb");
        assert_eq!(r.verdict, Verdict::NoMatch);
        let r = go("^a", "ab");
        assert_eq!(r.match_span, Some((0, 1)));
    }

    #[test]
    fn scoped_flags_approximated_globally() {
        // 作用域标志按全局近似（头注释声明）：(?i:ab) 里的 i 生效
        let r = go("(?i:ab)c", "ABc");
        assert_eq!(r.match_span, Some((0, 3)));
        // unset 部分同样生效：(?i-m) 显式关掉多行 → ^ 不再认行中（旧行为会命中 (2,3)）
        let r = go("(?i-m)^b", "a\nb");
        assert_eq!(r.verdict, Verdict::NoMatch);
        // 作用域形式 (?i-m:x) 的 raw 原文同样被扫描（旧行为：^ 恒行首，"(?i-m:x)^b" 在 "\nx\nb" 会命中）
        let r = go("(?i-m:x)^b", "\nx\nb");
        assert_eq!(r.verdict, Verdict::NoMatch);
    }
}
