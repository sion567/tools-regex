//! 自研 PCRE 超集递归下降解析器（设计方案 6.1 解析层，2026-11-17 定稿）。
//!
//! 支持范围：字面量 / 转义（\d \w \s \b \A \z \n \xHH \x{...} \p{...} ...）/
//! 字符类（含区间、否定、POSIX 类）/ `.` / 分组（捕获、(?:)、(?<n>)、(?P<n>)、(?P=n)、
//! 环视、(?>)）/ 交替 / 量词（* + ? {m,n}，含懒惰/占有）/ 内联标志 (?i)（全局与 (?i:...) 作用域）/
//! 反向引用（\1、\k<name>）
//!
//! 明确不支持：递归 (?R)、条件组 (?(1))、\u \U 码点——走友好错误（带 span + 提示），绝不 panic。
//!
//! 关键约束：错误必须带 byte span——"边输入边报"是产品体验（设计方案 6.4）。

use regex_ir::{AnchorKind, BackrefKind, ClassItem, GroupKind, Node, PerlClass, RepKind, Span};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
    pub hint: Option<String>,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "第 {} 列：{}", self.span.col(), self.message)?;
        if let Some(h) = &self.hint {
            write!(f, "（{}）", h)?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseError {}

/// 解析正则字符串为统一 IR。失败返回带 span 的友好错误。
pub fn parse(pattern: &str) -> Result<Node, ParseError> {
    let mut p = Parser::new(pattern);
    let node = p.parse_alternation()?;
    if let Some((off, ch)) = p.peek() {
        // 交替层级里不该出现的残留（如多余的 `)`）
        return Err(p.err_at(off, ch, format!("多余的 `{ch}`"), Some("括号不匹配，检查是否多了右括号")));
    }
    Ok(node)
}

/// 宽松解析（边输入边解析）：尽最大努力返回"能画的部分"。
/// 未闭合的组/类自动补全、非法残片吞掉记为错误，绝不 panic。
/// 返回（前缀 IR, 首个错误）。错误为 None 表示当前输入完整合法。
pub fn parse_partial(pattern: &str) -> (Node, Option<ParseError>) {
    let mut p = Parser::new_lenient(pattern);
    let node = p.parse_alternation().unwrap_or(Node::Empty);
    if let Some((off, ch)) = p.peek() {
        p.record_error(p.err_at(off, ch, format!("多余的 `{ch}`"), Some("括号不匹配，检查是否多了右括号")));
    }
    (node, p.error)
}

struct Parser {
    /// (byte offset, char)
    chars: Vec<(usize, char)>,
    i: usize,
    end: usize,      // pattern.len()
    src: String,     // 原文（flags 原文截取用）
    /// 宽松模式：边输入边解析，错误记入 error 并吞掉，返回前缀
    lenient: bool,
    /// 首个遇到的错误（宽松模式累积）
    error: Option<ParseError>,
    /// 捕获组编号（按 `(` 出现顺序从 1）
    group_counter: u32,
}

impl Parser {
    fn new(pattern: &str) -> Self {
        Self {
            chars: pattern.char_indices().collect(),
            i: 0,
            end: pattern.len(),
            src: pattern.to_string(),
            lenient: false,
            error: None,
            group_counter: 0,
        }
    }

    fn new_lenient(pattern: &str) -> Self {
        let mut p = Self::new(pattern);
        p.lenient = true;
        p
    }

    /// 只记第一个错误（边输入边解析时最早的错误最有指导意义）
    fn record_error(&mut self, e: ParseError) {
        if self.error.is_none() {
            self.error = Some(e);
        }
    }

    fn peek(&self) -> Option<(usize, char)> {
        self.chars.get(self.i).copied()
    }

    fn peek_at(&self, n: usize) -> Option<(usize, char)> {
        self.chars.get(self.i + n).copied()
    }

    fn bump(&mut self) -> Option<(usize, char)> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }

    /// 当前 char 的字节尾（span.end 用）
    fn off_end(&self, idx: usize) -> usize {
        match self.chars.get(idx) {
            Some((off, ch)) => off + ch.len_utf8(),
            None => self.end,
        }
    }

    fn err_at<S: Into<String>>(&self, off: usize, ch: char, message: String, hint: Option<S>) -> ParseError {
        ParseError {
            span: Span::new(off, (off + ch.len_utf8()).min(self.end.max(off + 1))),
            message,
            hint: hint.map(Into::into),
        }
    }

    fn err_eof<S: Into<String>>(&self, message: String, hint: Option<S>) -> ParseError {
        ParseError { span: Span::new(self.end, self.end), message, hint: hint.map(Into::into) }
    }

    // alt := concat ('|' concat)*
    fn parse_alternation(&mut self) -> Result<Node, ParseError> {
        let mut branches = vec![self.parse_concat()?];
        while let Some((_, '|')) = self.peek() {
            self.bump();
            branches.push(self.parse_concat()?);
        }
        if branches.len() == 1 {
            Ok(branches.pop().unwrap())
        } else {
            Ok(Node::Alternation(branches))
        }
    }

    // concat := repeat*
    fn parse_concat(&mut self) -> Result<Node, ParseError> {
        let mut items: Vec<Node> = Vec::new();
        loop {
            match self.peek() {
                None => break,
                Some((_, '|')) => break,
                Some((_, ')')) => break, // 由上层分组消费
                Some(_) => match self.parse_repeat() {
                    Ok(rep) => items.push(rep),
                    Err(e) => {
                        if self.lenient {
                            // 边输入边解析：记错吞掉，返回已解析前缀
                            self.record_error(e);
                            break;
                        }
                        return Err(e);
                    }
                },
            }
        }
        match items.len() {
            0 => Ok(Node::Empty),
            1 => Ok(items.pop().unwrap()),
            _ => Ok(Node::Concat(items)),
        }
    }

    // repeat := atom 后缀*
    fn parse_repeat(&mut self) -> Result<Node, ParseError> {
        let mut atom = self.parse_atom()?;
        while let Some((off, ch)) = self.peek() {
            match ch {
                '*' | '+' | '?' => {
                    self.bump();
                    let (min, max) = match ch {
                        '*' => (0, None),
                        '+' => (1, None),
                        _ => (0, Some(1)),
                    };
                    let rep = self.parse_rep_kind()?;
                    atom = self.wrap_repeat(atom, min, max, rep, off);
                }
                '{' => {
                    let save = self.i;
                    if let Some((min, max)) = self.try_parse_braces() {
                        if let Some(mx) = max {
                            if mx < min {
                                return Err(self.err_at(
                                    off,
                                    '{',
                                    format!("量词下界 {min} 大于上界 {mx}"),
                                    Some("写法应为 {min,max} 且 min ≤ max"),
                                ));
                            }
                        }
                        let rep = self.parse_rep_kind()?;
                        atom = self.wrap_repeat(atom, min, max, rep, off);
                    } else {
                        // 不是合法量词 → 回退，`{` 作为普通字面量交给下一个 atom
                        self.i = save;
                        break;
                    }
                }
                _ => break,
            }
        }
        Ok(atom)
    }

    /// 后缀 `?`（懒惰）/ `+`（占有）
    fn parse_rep_kind(&mut self) -> Result<RepKind, ParseError> {
        match self.peek() {
            Some((_, '?')) => {
                self.bump();
                Ok(RepKind::Lazy)
            }
            Some((_, '+')) => {
                self.bump();
                Ok(RepKind::Possessive)
            }
            _ => Ok(RepKind::Greedy),
        }
    }

    fn wrap_repeat(&self, child: Node, min: u32, max: Option<u32>, rep: RepKind, off: usize) -> Node {
        let end = self.off_end(self.i.saturating_sub(1));
        Node::Repeat { child: Box::new(child), min, max, rep, span: Span::new(off, end) }
    }

    /// 尝试解析 `{m}` `{m,}` `{m,n}` `{,n}`；失败不消费输入
    fn try_parse_braces(&mut self) -> Option<(u32, Option<u32>)> {
        let save = self.i;
        self.bump(); // '{'
        let mut digits_before = String::new();
        while let Some((_, c)) = self.peek() {
            if c.is_ascii_digit() {
                digits_before.push(c);
                self.bump();
            } else {
                break;
            }
        }
        match self.peek() {
            Some((_, '}')) => {
                if digits_before.is_empty() {
                    self.i = save;
                    return None;
                }
                self.bump();
                let m: u32 = digits_before.parse().ok()?;
                Some((m, Some(m)))
            }
            Some((_, ',')) => {
                self.bump();
                let mut digits_after = String::new();
                while let Some((_, c)) = self.peek() {
                    if c.is_ascii_digit() {
                        digits_after.push(c);
                        self.bump();
                    } else {
                        break;
                    }
                }
                if self.peek().map(|(_, c)| c) != Some('}') {
                    self.i = save;
                    return None;
                }
                self.bump();
                let min: u32 = if digits_before.is_empty() { 0 } else { digits_before.parse().ok()? };
                let max = if digits_after.is_empty() { None } else { Some(digits_after.parse().ok()?) };
                Some((min, max))
            }
            _ => {
                self.i = save;
                None
            }
        }
    }

    /// 宽松模式下把子解析错误记下并吞成 Empty（严格模式原样上抛）
    fn swallow(&mut self, f: impl FnOnce(&mut Self) -> Result<Node, ParseError>) -> Result<Node, ParseError> {
        match f(self) {
            Ok(n) => Ok(n),
            Err(e) => {
                if self.lenient {
                    self.record_error(e);
                    Ok(Node::Empty)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn parse_atom(&mut self) -> Result<Node, ParseError> {
        let Some((off, ch)) = self.peek() else {
            return Err(self.err_eof("表达式不完整".into(), None::<String>));
        };
        match ch {
            '(' => self.swallow(Self::parse_group),
            '[' => self.swallow(Self::parse_class),
            '.' => {
                self.bump();
                Ok(Node::Class {
                    negated: true,
                    items: vec![ClassItem::Char('\n')],
                    span: Span::new(off, self.off_end(self.i - 1)),
                })
            }
            '^' => {
                self.bump();
                Ok(Node::Anchor {
                    kind: AnchorKind::StartLine,
                    span: Span::new(off, self.off_end(self.i - 1)),
                })
            }
            '$' => {
                self.bump();
                Ok(Node::Anchor {
                    kind: AnchorKind::EndLine,
                    span: Span::new(off, self.off_end(self.i - 1)),
                })
            }
            '\\' => self.swallow(Self::parse_escape),
            // 量词字符出现在开头（无前置原子）：报错而不是静默当字面量
            '*' | '+' | '?' => {
                let e = self.err_at(
                    off,
                    ch,
                    format!("量词 `{ch}` 前面没有可重复的内容"),
                    Some("量词要跟在字符/类/分组之后"),
                );
                if self.lenient {
                    self.record_error(e);
                    self.bump();
                    Ok(Node::Empty)
                } else {
                    Err(e)
                }
            }
            _ => {
                // 普通字面量：吃掉连续普通字符合并为一个 Literal
                let mut text = String::new();
                while let Some((_, c)) = self.peek() {
                    match c {
                        '(' | ')' | '[' | '.' | '^' | '$' | '\\' | '|' | '*' | '+' | '?' | '{' | '}' => break,
                        _ => {
                            text.push(c);
                            self.i += 1;
                        }
                    }
                }
                if text.is_empty() {
                    // 走到这里的是"未构成量词的 `{`/`}`"等残留特殊符：
                    // 按字面量吃掉一个字符（PCRE 宽容语义），否则 concat 循环会原地死循环
                    match self.bump() {
                        Some((_, c)) => text.push(c),
                        None => unreachable!("peek 已保证有字符"),
                    }
                }
                let span = Span::new(off, self.off_end(self.i - 1));
                Ok(Node::Literal { text, span })
            }
        }
    }

    fn parse_group(&mut self) -> Result<Node, ParseError> {
        let open_off = self.peek().map(|(o, _)| o).unwrap_or(0);
        self.bump(); // '('

        // (?P=name) 命名反向引用（形如组，实为引用）
        if self.peek().map(|(_, c)| c) == Some('?')
            && self.peek_at(1).map(|(_, c)| c) == Some('P')
            && self.peek_at(2).map(|(_, c)| c) == Some('=')
        {
            self.bump();
            self.bump();
            self.bump();
            let name = self.parse_group_name(')')?; // 连同终止符 `)` 一起消费
            return Ok(Node::Backref {
                kind: BackrefKind::Name(name),
                span: Span::new(open_off, self.off_end(self.i - 1)),
            });
        }

        let mut flags_raw: Option<String> = None;
        let (kind, name) = if self.peek().map(|(_, c)| c) == Some('?') {
            let q_off = self.peek().map(|(o, _)| o).unwrap_or(0);
            self.bump();
            // 内联标志：(?i) 全局 Flags 节点 / (?i:...) 作用域组
            if let Some((set, unset, raw)) = self.try_parse_flags_prefix() {
                match self.peek() {
                    Some((_, ')')) => {
                        self.bump();
                        return Ok(Node::Flags {
                            set,
                            unset,
                            span: Span::new(open_off, self.off_end(self.i - 1)),
                        });
                    }
                    Some((_, ':')) => {
                        self.bump();
                        flags_raw = Some(raw);
                        (GroupKind::NonCapturing, None)
                    }
                    // 标志字符后跟的不是 )/: → 不是标志，回退走普通前缀
                    _ => {
                        self.i = q_off;
                        self.parse_group_after_question(q_off)?
                    }
                }
            } else {
                // 非标志前缀：(?: ) 非捕获组是高频写法，先于通用前缀解析
                match self.peek() {
                    Some((_, ':')) => {
                        self.bump();
                        (GroupKind::NonCapturing, None)
                    }
                    _ => self.parse_group_after_question(q_off)?,
                }
            }
        } else {
            (GroupKind::Capture, None)
        };

        // 捕获/命名组按 `(` 出现顺序编号（先于子组 → 外层号小，同 PCRE）
        let index = if matches!(kind, GroupKind::Capture | GroupKind::Named) {
            self.group_counter += 1;
            Some(self.group_counter)
        } else {
            None
        };

        let child = self.parse_alternation()?;
        match self.peek() {
            Some((_, ')')) => {
                self.bump();
                Ok(Node::Group {
                    kind,
                    name,
                    index,
                    flags: flags_raw,
                    child: Box::new(child),
                    span: Span::new(open_off, self.off_end(self.i - 1)),
                })
            }
            _ if self.lenient => {
                // 边输入边解析：未闭合的组按"自动补全右括号"出图，但记下错误提示
                self.record_error(self.err_eof(
                    "分组没有闭合".into(),
                    Some(format!("缺少与第 {} 列 `(` 配对的 `)`", open_off + 1)),
                ));
                Ok(Node::Group {
                    kind,
                    name,
                    index,
                    flags: flags_raw,
                    child: Box::new(child),
                    span: Span::new(open_off, self.end),
                })
            }
            _ => Err(self.err_eof(
                "分组没有闭合".into(),
                Some(format!("缺少与第 {} 列 `(` 配对的 `)`", open_off + 1)),
            )),
        }
    }

    /// 尝试解析 `(?` 之后的标志前缀：`i` `im` `i-s` `-m` 等。
    /// 仅当标志字符后紧跟 `)` 或 `:` 时提交；否则回滚输入返回 None。
    /// 返回 (set, unset, 原文如 "im-s")
    fn try_parse_flags_prefix(&mut self) -> Option<(String, String, String)> {
        let save = self.i;
        let mut set = String::new();
        let mut unset = String::new();
        let mut neg = false;
        let mut any = false;
        loop {
            match self.peek() {
                Some((_, '-')) if !neg && any => {
                    neg = true;
                    self.bump();
                }
                Some((_, c)) if "imsxU".contains(c) => {
                    self.bump();
                    any = true;
                    if neg {
                        unset.push(c);
                    } else {
                        set.push(c);
                    }
                }
                _ => break,
            }
        }
        if !any {
            self.i = save;
            return None;
        }
        // 提交条件：紧跟 ) 或 :
        match self.peek() {
            Some((_, ')')) | Some((_, ':')) => {
                let b0 = self.chars[save].0;
                let b1 = self.off_end(self.i - 1);
                let raw = self.src[b0..b1].to_string();
                Some((set, unset, raw))
            }
            _ => {
                self.i = save;
                None
            }
        }
    }

    /// 已吃掉 `(` 和 `?`，解析剩余的组前缀：`=`,`!`,`>`,`<=`,`<!`,`<name>`,`P<name>`,`P=name`,`R`,`1` 等
    fn parse_group_after_question(&mut self, _q_off: usize) -> Result<(GroupKind, Option<String>), ParseError> {
        match self.peek() {
            Some((_, '=')) => {
                self.bump();
                Ok((GroupKind::Lookahead { negative: false }, None))
            }
            Some((_, '!')) => {
                self.bump();
                Ok((GroupKind::Lookahead { negative: true }, None))
            }
            Some((_, '>')) => {
                self.bump();
                Ok((GroupKind::Atomic, None))
            }
            // (?<= / (?<!
            Some((_, '<')) if matches!(self.peek_at(1).map(|(_, c)| c), Some('=') | Some('!')) => {
                let neg = self.peek_at(1).map(|(_, c)| c) == Some('!');
                self.bump();
                self.bump();
                Ok((GroupKind::Lookbehind { negative: neg }, None))
            }
            // (?<name>
            Some((_, '<')) => {
                self.bump();
                let name = self.parse_group_name('>')?;
                Ok((GroupKind::Named, Some(name)))
            }
            // (?P<name>（(?P=name 已在 parse_group 入口预检为命名反向引用，不会走到这）
            Some((off, 'P')) => {
                self.bump();
                match self.peek() {
                    Some((_, '<')) => {
                        self.bump();
                        let name = self.parse_group_name('>')?;
                        Ok((GroupKind::Named, Some(name)))
                    }
                    _ => Err(self.err_at(
                        off,
                        'P',
                        "`(?P` 后只支持 `<name>` 或 `=name`".into(),
                        None::<String>,
                    )),
                }
            }
            // 递归 / 子程序调用
            Some((off, 'R')) => Err(self.err_at(
                off,
                'R',
                "递归 `(?R)` 暂不支持".into(),
                Some("PCRE/Ruby/.NET 特性，M1 明确不支持清单内"),
            )),
            Some((off, c)) if c.is_ascii_digit() => Err(self.err_at(
                off,
                c,
                format!("子程序调用 `(?{c})` 暂不支持"),
                Some("M1 明确不支持清单内"),
            )),
            Some((off, c)) if "imxsU".contains(c) => Err(self.err_at(
                off,
                c,
                "内联标志 `(?i)` 暂不支持".into(),
                Some("M1 支持；当前请在外层配置大小写等选项"),
            )),
            Some((off, c)) if c == '(' => Err(self.err_at(
                off,
                c,
                "条件组 `(?(` 暂不支持".into(),
                Some("M1 明确不支持清单内"),
            )),
            Some((off, c)) => Err(self.err_at(
                off,
                c,
                format!("无法识别的组前缀 `?{c}`"),
                Some("支持：(?:) (?=) (?!) (?<name>) (?P<name>) (?<=) (?<!) (?>)"),
            )),
            None => Err(self.err_eof("组前缀不完整".into(), Some("`?` 后面缺少内容"))),
        }
    }

    /// 组名解析：字母/数字/下划线，以 `close` 字符结束（`(?<n>` 为 `>`，`(?P=n)` 为 `)`）
    fn parse_group_name(&mut self, close: char) -> Result<String, ParseError> {
        let mut name = String::new();
        loop {
            match self.bump() {
                Some((_, c)) if c == close => break,
                Some((_o, c)) if c.is_alphanumeric() || c == '_' => name.push(c),
                Some((off, c)) => {
                    return Err(self.err_at(
                        off,
                        c,
                        format!("组名里出现非法字符 `{c}`"),
                        Some(format!("组名仅允许字母/数字/下划线，以 `{close}` 结束")),
                    ))
                }
                None => {
                    return Err(self.err_eof("组名没有闭合".into(), Some(format!("缺少 `{close}`"))));
                }
            }
        }
        if name.is_empty() {
            return Err(self.err_eof("组名为空".into(), Some("如 `(?<year>\\d{4})`")));
        }
        Ok(name)
    }

    fn parse_escape(&mut self) -> Result<Node, ParseError> {
        let bs_off = self.peek().map(|(o, _)| o).unwrap_or(0);
        self.bump(); // '\'
        let Some((off, ch)) = self.peek() else {
            if self.lenient {
                // 边输入边解析：行尾的 `\` 先按字面反斜杠出图，等下个字符到了再重新解析
                return Ok(Node::Literal { text: "\\".into(), span: Span::new(bs_off, bs_off + 1) });
            }
            return Err(self.err_eof("转义不完整".into(), Some("行尾的 `\\` 后面缺字符，字面反斜杠请写 `\\\\`")));
        };
        let simple = |p: &mut Self, c: char| -> Node {
            p.bump();
            Node::Literal { text: c.to_string(), span: Span::new(bs_off, p.off_end(p.i - 1)) }
        };
        let perl = |p: &mut Self, cls: PerlClass| -> Node {
            p.bump();
            Node::Class {
                negated: false,
                items: vec![ClassItem::Perl(cls)],
                span: Span::new(bs_off, p.off_end(p.i - 1)),
            }
        };
        let node = match ch {
            'd' => perl(self, PerlClass::Digit),
            'D' => perl(self, PerlClass::NotDigit),
            'w' => perl(self, PerlClass::Word),
            'W' => perl(self, PerlClass::NotWord),
            's' => perl(self, PerlClass::Space),
            'S' => perl(self, PerlClass::NotSpace),
            'b' => {
                self.bump();
                Node::Anchor {
                    kind: AnchorKind::WordBoundary,
                    span: Span::new(bs_off, self.off_end(self.i - 1)),
                }
            }
            'B' => {
                self.bump();
                Node::Anchor {
                    kind: AnchorKind::NotWordBoundary,
                    span: Span::new(bs_off, self.off_end(self.i - 1)),
                }
            }
            'A' => {
                self.bump();
                Node::Anchor {
                    kind: AnchorKind::StartText,
                    span: Span::new(bs_off, self.off_end(self.i - 1)),
                }
            }
            'z' | 'Z' => {
                self.bump();
                Node::Anchor {
                    kind: AnchorKind::EndText,
                    span: Span::new(bs_off, self.off_end(self.i - 1)),
                }
            }
            'n' => simple(self, '\n'),
            'r' => simple(self, '\r'),
            't' => simple(self, '\t'),
            'f' => simple(self, '\u{0C}'),
            'v' => simple(self, '\u{0B}'),
            '0' => simple(self, '\0'),
            'a' => simple(self, '\u{07}'),
            'e' => simple(self, '\u{1B}'),
            c if c.is_ascii_digit() => {
                self.bump();
                let idx = c.to_digit(10).unwrap();
                Node::Backref { kind: BackrefKind::Index(idx), span: Span::new(bs_off, self.off_end(self.i - 1)) }
            }
            // \k<name> / \k'name'
            'k' => {
                self.bump();
                let (close, close_desc) = match self.peek() {
                    Some((_, '<')) => ('>', "`>`"),
                    Some((_, '\'')) => ('\'', "`'`"),
                    _ => {
                        return Err(self.err_at(
                            off,
                            'k',
                            "`\\k` 后应跟 `<name>` 或 `'name'`".into(),
                            Some("命名反向引用写法：`\\k<year>`"),
                        ))
                    }
                };
                self.bump();
                let mut name = String::new();
                loop {
                    match self.bump() {
                        Some((_, c)) if c == close => break,
                        Some((_, c)) if c.is_alphanumeric() || c == '_' => name.push(c),
                        Some((o, c)) => {
                            return Err(self.err_at(
                                o,
                                c,
                                format!("组名里出现非法字符 `{c}`"),
                                Some(format!("组名仅允许字母/数字/下划线，以 {close_desc} 结束")),
                            ))
                        }
                        None => return Err(self.err_eof("命名反向引用没有闭合".into(), Some(format!("缺少 {close_desc}")))),
                    }
                }
                if name.is_empty() {
                    return Err(self.err_at(off, 'k', "反向引用组名为空".into(), Some("如 `\\k<year>`")));
                }
                Node::Backref { kind: BackrefKind::Name(name), span: Span::new(bs_off, self.off_end(self.i - 1)) }
            }
            // \xHH 或 \x{HHH} 码点
            'x' => {
                self.bump();
                let (num, n_off): (u32, usize) = if self.peek().map(|(_, c)| c) == Some('{') {
                    self.bump();
                    let mut digits = String::new();
                    loop {
                        match self.bump() {
                            Some((_, '}')) => break,
                            Some((_, c)) if c.is_ascii_hexdigit() => {
                                digits.push(c);
                            }
                            Some((o, c)) => {
                                return Err(self.err_at(o, c, "十六进制码点非法".into(), Some("如 `\\x{1F600}`")))
                            }
                            None => return Err(self.err_eof("码点转义没有闭合".into(), Some("缺少 `}`"))),
                        }
                    }
                    (u32::from_str_radix(&digits, 16).map_err(|_| {
                        self.err_eof("码点超出范围".into(), Some("最多 6 位十六进制，≤ U+10FFFF"))
                    })?, off)
                } else {
                    let mut digits = String::new();
                    for _ in 0..2 {
                        match self.peek() {
                            Some((_, c)) if c.is_ascii_hexdigit() => {
                                digits.push(c);
                                self.bump();
                            }
                            _ => break,
                        }
                    }
                    if digits.len() < 2 {
                        return Err(self.err_at(
                            off,
                            'x',
                            "`\\x` 需要两位十六进制".into(),
                            Some("如 `\\x41` 或 `\\x{1F600}`"),
                        ));
                    }
                    (u32::from_str_radix(&digits, 16).unwrap(), off)
                };
                let c = char::from_u32(num).ok_or_else(|| {
                    self.err_at(n_off, 'x', format!("码点 U+{num:X} 不是合法字符"), None::<String>)
                })?;
                Node::Literal { text: c.to_string(), span: Span::new(bs_off, self.off_end(self.i - 1)) }
            }
            'u' | 'U' => {
                return Err(self.err_at(
                    off,
                    ch,
                    format!("码点转义 `\\{ch}` 暂不支持"),
                    Some("那是 Java/JS 字符串字面量语法；正则层面请用 `\\x{...}`"),
                ))
            }
            'p' | 'P' => {
                self.bump();
                // \p{L} / \pL
                let name = if self.peek().map(|(_, c)| c) == Some('{') {
                    self.bump();
                    let mut s = String::new();
                    loop {
                        match self.bump() {
                            Some((_, '}')) => break,
                            Some((_, c)) if c.is_alphanumeric() => s.push(c),
                            Some((o, c)) => {
                                return Err(self.err_at(o, c, "Unicode 属性名非法".into(), Some("如 `\\p{L}`")))
                            }
                            None => return Err(self.err_eof("Unicode 属性没有闭合".into(), Some("缺少 `}`"))),
                        }
                    }
                    s
                } else {
                    match self.bump() {
                        Some((_, c)) if c.is_alphabetic() => c.to_string(),
                        Some((o, c)) => {
                            return Err(self.err_at(o, c, "单字母 Unicode 属性非法".into(), Some("如 `\\pL` 或 `\\p{L}`")))
                        }
                        None => return Err(self.err_eof("Unicode 属性不完整".into(), None::<String>)),
                    }
                };
                if name.is_empty() {
                    return Err(self.err_at(off, ch, "Unicode 属性名为空".into(), Some("如 `\\p{L}`")));
                }
                Node::Class {
                    negated: ch == 'P',
                    items: vec![ClassItem::Unicode(name)],
                    span: Span::new(bs_off, self.off_end(self.i - 1)),
                }
            }
            c if !c.is_alphanumeric() => simple(self, c),
            c => {
                return Err(self.err_at(
                    off,
                    c,
                    format!("无法识别的转义 `\\{c}`"),
                    Some("已知：\\d \\w \\s \\b \\n \\t；标点转义如 `\\.` `\\(` 均可"),
                ))
            }
        };
        Ok(node)
    }

    fn parse_class(&mut self) -> Result<Node, ParseError> {
        let open_off = self.peek().map(|(o, _)| o).unwrap_or(0);
        self.bump(); // '['
        let negated = if self.peek().map(|(_, c)| c) == Some('^') {
            self.bump();
            true
        } else {
            false
        };
        let mut items: Vec<ClassItem> = Vec::new();
        // 首个 ']' 是字面量（PCRE 语义）
        let mut first = true;
        loop {
            let Some((off, ch)) = self.peek() else {
                if self.lenient {
                    // 边输入边解析：未闭合的类按"自动补全 ]"出图
                    break;
                }
                return Err(self.err_eof("字符类没有闭合".into(), Some(format!("缺少与第 {} 列 `[` 配对的 `]`", open_off + 1))));
            };
            if ch == ']' && !first {
                self.bump();
                break;
            }
            first = false;
            // 单成员
            let lo = self.parse_class_member(off)?;
            // 区间？
            if self.peek().map(|(_, c)| c) == Some('-')
                && self.peek_at(1).map(|(_, c)| c) != Some(']')
                && self.peek_at(1).is_some()
            {
                self.bump(); // '-'
                let hi_off = self.peek().map(|(o, _)| o).unwrap_or(self.end);
                let hi = self.parse_class_member(hi_off)?;
                let (lo_c, hi_c) = (class_char(&lo), class_char(&hi));
                match (lo_c, hi_c) {
                    (Some(a), Some(b)) if a <= b => items.push(ClassItem::Range(a, b)),
                    (Some(a), Some(b)) => {
                        return Err(self.err_at(
                            hi_off,
                            '-',
                            format!("区间 `{a}-{b}` 上界小于下界"),
                            Some("字符区间需从小到大，如 `a-z`"),
                        ));
                    }
                    _ => {
                        // 区间端点是 Perl 类等复杂成员：降级为两个独立成员
                        items.push(lo);
                        items.push(hi);
                    }
                }
            } else {
                items.push(lo);
            }
        }
        Ok(Node::Class { negated, items, span: Span::new(open_off, self.off_end(self.i - 1)) })
    }

    /// 类成员：普通字符 / POSIX 类 / 转义（含 \d \w \s \p）
    fn parse_class_member(&mut self, _off: usize) -> Result<ClassItem, ParseError> {
        let Some((_o, ch)) = self.peek() else {
            return Err(self.err_eof("字符类不完整".into(), None::<String>));
        };
        // [:alpha:] / [:^alpha:] POSIX 类（类内语法）
        if ch == '[' && self.peek_at(1).map(|(_, c)| c) == Some(':') {
            self.bump();
            self.bump();
            let negated = if self.peek().map(|(_, c)| c) == Some('^') {
                self.bump();
                true
            } else {
                false
            };
            let mut name = String::new();
            loop {
                match self.bump() {
                    Some((_, ':')) => break,
                    Some((_, c)) if c.is_ascii_alphabetic() => name.push(c),
                    Some((o, c)) => {
                        return Err(self.err_at(
                            o,
                            c,
                            "POSIX 类名非法".into(),
                            Some("如 `[:alpha:]` `[:digit:]`；以 `:]` 结束"),
                        ))
                    }
                    None => return Err(self.err_eof("POSIX 类没有闭合".into(), Some("缺少 `:]`"))),
                }
            }
            if self.peek().map(|(_, c)| c) != Some(']') {
                return Err(self.err_eof(
                    "POSIX 类写法不完整".into(),
                    Some("POSIX 类以 `:]` 结束，如 `[:digit:]`"),
                ));
            }
            self.bump();
            return Ok(ClassItem::Posix { name, negated });
        }
        if ch != '\\' {
            self.bump();
            return Ok(ClassItem::Char(ch));
        }
        self.bump(); // '\'
        let Some((off, ch)) = self.peek() else {
            return Err(self.err_eof("转义不完整".into(), Some("`]` 前的 `\\` 缺字符")));
        };
        let item = match ch {
            'd' => ClassItem::Perl(PerlClass::Digit),
            'D' => ClassItem::Perl(PerlClass::NotDigit),
            'w' => ClassItem::Perl(PerlClass::Word),
            'W' => ClassItem::Perl(PerlClass::NotWord),
            's' => ClassItem::Perl(PerlClass::Space),
            'S' => ClassItem::Perl(PerlClass::NotSpace),
            'n' => ClassItem::Char('\n'),
            'r' => ClassItem::Char('\r'),
            't' => ClassItem::Char('\t'),
            'f' => ClassItem::Char('\u{0C}'),
            'v' => ClassItem::Char('\u{0B}'),
            '0' => ClassItem::Char('\0'),
            'b' => ClassItem::Char('\u{08}'), // 类内 \b 是退格（PCRE 语义）
            'p' | 'P' => {
                self.bump();
                let name = if self.peek().map(|(_, c)| c) == Some('{') {
                    self.bump();
                    let mut s = String::new();
                    loop {
                        match self.bump() {
                            Some((_, '}')) => break,
                            Some((_, c)) if c.is_alphanumeric() => s.push(c),
                            Some((o, c)) => {
                                return Err(self.err_at(o, c, "Unicode 属性名非法".into(), Some("如 `[\\p{L}]`")))
                            }
                            None => return Err(self.err_eof("Unicode 属性没有闭合".into(), Some("缺少 `}`"))),
                        }
                    }
                    s
                } else {
                    match self.bump() {
                        Some((_, c)) if c.is_alphabetic() => c.to_string(),
                        Some((o, c)) => {
                            return Err(self.err_at(o, c, "单字母 Unicode 属性非法".into(), Some("如 `[\\pL]`")))
                        }
                        None => return Err(self.err_eof("Unicode 属性不完整".into(), None::<String>)),
                    }
                };
                if name.is_empty() {
                    return Err(self.err_at(off, ch, "Unicode 属性名为空".into(), None::<String>));
                }
                return Ok(ClassItem::Unicode(name));
            }
            c if !c.is_alphanumeric() => ClassItem::Char(c), // \] \- \\ \^ 等
            c => {
                return Err(self.err_at(
                    off,
                    c,
                    format!("类内无法识别的转义 `\\{c}`"),
                    Some("类内不支持反向引用与锚点"),
                ))
            }
        };
        self.bump();
        Ok(item)
    }
}

/// 若成员是单字符则返回之（区间判定用）
fn class_char(item: &ClassItem) -> Option<char> {
    match item {
        ClassItem::Char(c) => Some(*c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Node {
        parse(s).unwrap_or_else(|e| panic!("`{s}` 应解析成功，却得到：{e}"))
    }

    #[test]
    fn literal_merge() {
        let n = ok("abc");
        match n {
            Node::Literal { text, .. } => assert_eq!(text, "abc"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn alternation_and_quantifiers() {
        let n = ok("a(b|cd)?e*");
        let Node::Concat(items) = n else { panic!() };
        assert_eq!(items.len(), 3);
        // a
        assert!(matches!(&items[0], Node::Literal { text, .. } if text == "a"));
        // (b|cd)? → Repeat(0..1, Group(Alternation))
        match &items[1] {
            Node::Repeat { min, max, child, .. } => {
                assert_eq!((*min, *max), (0, Some(1)));
                assert!(matches!(&**child, Node::Group { kind: GroupKind::Capture, .. }));
            }
            other => panic!("{other:?}"),
        }
        // e*
        assert!(matches!(&items[2], Node::Repeat { min: 0, max: None, .. }));
    }

    #[test]
    fn named_group_two_syntaxes() {
        for pat in [r"(?<year>\d{4})", r"(?P<year>\d{4})"] {
            match ok(pat) {
                Node::Group { kind: GroupKind::Named, name: Some(n), child, .. } => {
                    assert_eq!(n, "year");
                    assert!(matches!(&*child, Node::Repeat { min: 4, max: Some(4), .. }));
                }
                other => panic!("{pat} => {other:?}"),
            }
        }
    }

    #[test]
    fn used_features_end_to_end() {
        // used_features 本体在 regex-ir（零依赖，无法自测解析）；端到端断言放这里
        use regex_ir::{used_features, Feature};
        let set = |p: &str| used_features(&parse(p).unwrap());
        assert!(set(r"\b(\w+) \1\b").contains(&Feature::Backreference));
        assert!(set(r"\d+(?=\.)").contains(&Feature::Lookahead));
        assert!(set(r"(?<=\$)\d").contains(&Feature::Lookbehind));
        assert!(set("a++").contains(&Feature::PossessiveQuant));
        assert!(set("(?>a+)b").contains(&Feature::AtomicGroup));
        assert!(set(r"\b(\d{4})-(\d{2})\b").is_empty());
    }

    #[test]
    fn non_capturing_group_still_works() {
        // M1 重写时曾把 (?: 分支弄丢（实测 danger 测试抓出）——回归锁
        match ok("(?:ab)+c") {
            Node::Concat(items) => {
                assert!(matches!(&items[0], Node::Repeat { child, .. }
                    if matches!(&**child, Node::Group { kind: GroupKind::NonCapturing, .. })));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lookaround_parse() {
        assert!(matches!(ok(r"(?=\.)"), Node::Group { kind: GroupKind::Lookahead { negative: false }, .. }));
        assert!(matches!(ok(r"(?<!\$)\d+"), Node::Concat(_)));
    }

    #[test]
    fn backref_and_dot() {
        let n = ok(r"\b(\w+)\s+\1\b");
        let Node::Concat(items) = n else { panic!() };
        assert!(matches!(&items[3], Node::Backref { kind: BackrefKind::Index(1), .. }));
    }

    #[test]
    fn inline_flags_global_and_scoped() {
        // 全局 (?i)
        match ok("(?i)abc") {
            Node::Concat(items) => {
                assert!(matches!(&items[0], Node::Flags { set, unset, .. } if set == "i" && unset.is_empty()));
            }
            other => panic!("{other:?}"),
        }
        // unset：(?i-m)
        match ok("(?i-m)x") {
            Node::Concat(items) => {
                assert!(matches!(&items[0], Node::Flags { set, unset, .. } if set == "i" && unset == "m"));
            }
            other => panic!("{other:?}"),
        }
        // 作用域 (?i:ab)c
        match ok("(?i:ab)c") {
            Node::Concat(items) => {
                assert!(matches!(
                    &items[0],
                    Node::Group { kind: GroupKind::NonCapturing, flags: Some(f), .. } if f == "i"
                ));
            }
            other => panic!("{other:?}"),
        }
        // 不是标志的 `?` 前缀不受影响
        assert!(matches!(ok("(?=a)"), Node::Group { kind: GroupKind::Lookahead { .. }, .. }));
    }

    #[test]
    fn named_backrefs() {
        // \k<name>
        match ok(r"\k<year>") {
            Node::Backref { kind: BackrefKind::Name(n), .. } => assert_eq!(n, "year"),
            other => panic!("{other:?}"),
        }
        // (?P=name)
        match ok("(?P=y)") {
            Node::Backref { kind: BackrefKind::Name(n), .. } => assert_eq!(n, "y"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn hex_escapes() {
        match ok(r"\x41") {
            Node::Literal { text, .. } => assert_eq!(text, "A"),
            other => panic!("{other:?}"),
        }
        match ok(r"\x{1F600}") {
            Node::Literal { text, .. } => assert_eq!(text, "😀"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn posix_class_member() {
        match ok("[[:digit:]a-f]") {
            Node::Class { negated: false, items, .. } => {
                assert_eq!(items[0], ClassItem::Posix { name: "digit".into(), negated: false });
                assert_eq!(items[1], ClassItem::Range('a', 'f'));
            }
            other => panic!("{other:?}"),
        }
        match ok("[[:^alpha:]]") {
            Node::Class { items, .. } => {
                assert_eq!(items[0], ClassItem::Posix { name: "alpha".into(), negated: true });
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn capture_group_numbering() {
        // 编号按 `(` 出现顺序：外层先于内层
        let n = ok("(a)(b(?<x>c))");
        let Node::Concat(items) = n else { panic!() };
        let g1 = &items[0];
        let g2 = &items[1];
        let (Node::Group { index: Some(1), .. }, Node::Group { index: Some(2), child, .. }) = (g1, g2)
        else {
            panic!("编号不符：{items:?}")
        };
        let Node::Concat(inner) = &**child else { panic!() };
        let Node::Group { index: Some(3), name: Some(nm), .. } = &inner[1] else {
            panic!("命名组编号不符：{inner:?}")
        };
        assert_eq!(nm, "x");
    }

    #[test]
    fn partial_parse_prefix_and_error() {
        // 未闭合组：宽松下自动补全出图 + 记错
        let (node, err) = parse_partial("(ab|c");
        assert!(err.is_some());
        assert!(matches!(node, Node::Group { kind: GroupKind::Capture, .. }));

        // 行尾反斜杠：宽松按字面量
        let (node, err) = parse_partial("ab\\");
        assert!(err.is_none());
        assert!(matches!(node, Node::Concat(_)));

        // 完全合法的输入：宽松 = 严格，无错
        let (node, err) = parse_partial("a(b|c)?d");
        assert!(err.is_none());
        assert_eq!(node, ok("a(b|c)?d"));

        // 严重残片：记错但不 panic
        let (_, err) = parse_partial("(?P=");
        assert!(err.is_some());

        // 多余右括号：记错
        let (_, err) = parse_partial("a)b");
        assert!(err.is_some());
    }

    #[test]
    fn class_with_range_and_negation() {
        match ok("[^a-z0-9_]") {
            Node::Class { negated: true, items, .. } => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], ClassItem::Range('a', 'z'));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn friendly_errors_with_span() {
        // 未闭合分组 → 指出 `(` 的列
        let e = parse("(abc").unwrap_err();
        assert!(e.message.contains("闭合"));
        assert!(e.hint.as_deref().unwrap().contains("第 1 列"));

        // 行尾反斜杠
        let e = parse("a\\").unwrap_err();
        assert!(e.message.contains("转义"));

        // 量词开头
        let e = parse("*a").unwrap_err();
        assert!(e.message.contains("量词"));
        assert_eq!(e.span.col(), 1);

        // 递归 → 暂不支持
        let e = parse("(?R)").unwrap_err();
        assert!(e.message.contains("递归"));

        // 区间倒置
        let e = parse("[z-a]").unwrap_err();
        assert!(e.message.contains("上界小于下界"));

        // 多余右括号（顶层残留）
        let e = parse("a)b").unwrap_err();
        assert!(e.message.contains("多余的"));
    }

    #[test]
    fn leading_brace_is_literal() {
        // 非法量词形式的 `{` 按字面量处理（PCRE 宽容语义）。
        // 曾是死循环 bug：空字面量不消费字符，concat 循环原地打转（M0 实测抓出）
        assert!(matches!(ok("a{,2"), Node::Concat(_)));
        assert!(matches!(ok("a}b"), Node::Concat(_)));
        assert!(matches!(ok("a{"), Node::Concat(_)));
    }
}
