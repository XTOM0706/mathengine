//! 数学 AST 与 Parser（token 流 → 语法树）。
//!
//! 只构建**结构**，不做布局。AST 语义对齐 TeX 数学模式：
//!   - 原子序列（`Atom` / `Group`）是基本串联单元
//!   - `_`/`^` 把下一个"原子+可选的上下标"挂到前一个原子/组之上（scripts）
//!   - `\frac`/`\sqrt`/`\binom` 是带参构造
//!   - `\sum`/`\int`/`\prod`… 是"大运算符"，自带 limits
//!   - `\left( ... \right)` 是分隔符组（闭合高度传播在布局层做）
//!   - `\begin{matrix|pmatrix|...} ... \end{...}` 是表格（`&` 分列、`\\` 分行）
//!
//! 未知命令 → `Unknown(cmd)` 原子，**不抛错**（渲染层决定原文回退）。

use crate::token::{Token, tokenize};

/// 数学字体族（`\mathbb` 这类命令）。TeX 里它们把字母换成 Unicode 数学字母区的码点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyle {
    /// `\mathbb`：双线体（ℂ ℕ ℝ …）
    DoubleStruck,
    /// `\mathbf` / `\boldsymbol`：粗体
    Bold,
    /// `\mathcal` / `\mathscr`：花体
    Script,
    /// `\mathfrak`：哥特体
    Fraktur,
    /// `\mathsf`：无衬线
    SansSerif,
    /// `\mathit`：斜体
    Italic,
    /// `\mathrm`：正体（不做码点映射）
    Upright,
}

/// 数学 AST。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// 单个字符（字母/数字/符号）。`cmd` 为 None 表示字面量字符。
    Atom { ch: char, cmd: Option<String> },
    /// 命令名（已知的符号命令归到这里，如 `\alpha` → Symbol("alpha")）。
    Symbol(String),
    /// 原子序列（多个子表达式顺序排列）。
    Row(Vec<Expr>),
    /// 带上下标：`base`、可选 `sub`、可选 `sup`。TeX 语义：先 base 后脚本。
    Scripts {
        base: Box<Expr>,
        sub: Option<Box<Expr>>,
        sup: Option<Box<Expr>>,
    },
    /// `\frac{num}{den}`。
    Frac { num: Box<Expr>, den: Box<Expr> },
    /// `\sqrt[degree]{body}`，`degree` 缺省为 None。
    Sqrt { degree: Option<Box<Expr>>, body: Box<Expr> },
    /// `\binom{n}{k}`（二项式）。
    Binom { n: Box<Expr>, k: Box<Expr> },
    /// `\text{...}`：文本模式（原样字符，非数学斜体）。
    Text(String),
    /// 大运算符（`\sum`/`\int`/`\prod`/`\lim`…）及其上下限。
    Bigop {
        name: String,
        sub: Option<Box<Expr>>,
        sup: Option<Box<Expr>>,
    },
    /// `\left` 分隔符组：`(open, body, close)`，open/close 可能为点（`.`=空分隔符）。
    LeftRight { open: String, body: Box<Expr>, close: String },
    /// 表格环境：`\begin{env}` 里的行列。
    Matrix { env: String, rows: Vec<Vec<Expr>> },
    /// 上标记重音（`\hat{x}`/`\bar{x}`/`\vec{v}`/`\dot{y}`…）：name 是重音命令名（不含 `\`）。
    Accent { name: String, body: Box<Expr> },
    /// 上下划线（`\overline{ab}` / `\underline{ab}`）：name 是 `overline`/`underline`。
    Line { name: String, body: Box<Expr> },
    /// 字体族包裹（`\mathbb{R}`）：布局时把字母映射到对应数学字母区。
    Styled { style: FontStyle, body: Box<Expr> },
    /// `\overset{top}{base}` / `\underset{bottom}{base}` / `\stackrel{top}{base}`。
    Stack {
        base: Box<Expr>,
        top: Option<Box<Expr>>,
        bottom: Option<Box<Expr>>,
    },
    /// `\substack{a\\b}`：多行小字堆叠（常用于求和/并集的上下限）。
    Lines(Vec<Expr>),
    /// 占位：保留尺寸但不绘制。keep_w/keep_h 决定保留哪一维。
    /// phantom 两维都留，hphantom 只留宽，vphantom 只留高深，smash 保留宽度、高度深度归零。
    Phantom {
        body: Box<Expr>,
        keep_w: bool,
        keep_h: bool,
    },
    /// 未识别命令（渲染层原文回退）。
    Unknown(String),
}

/// Parse 失败：不抛错，返回带剩余位置的错误（调用方可转原文回退）。
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError(pub String);

/// 收集 AST 里所有未识别的命令名（去重、保持首次出现顺序）。
///
/// 宽容解析会把不认识的命令留成 [`Expr::Unknown`]，布局时变成空盒——
/// 于是「`\textcolor{red}{x}`」会静默显示成 `redx`。调用方拿到这份清单就能提示用户
/// 哪些命令其实没被支持，而不是让他对着画错的公式猜。
pub fn unknown_commands(e: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    walk_unknown(e, &mut out);
    out.sort();
    out.dedup();
    out
}

fn walk_unknown(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Unknown(name) => out.push(name.clone()),
        Expr::Row(v) => v.iter().for_each(|e| walk_unknown(e, out)),
        Expr::Scripts { base, sub, sup } => {
            walk_unknown(base, out);
            if let Some(s) = sub { walk_unknown(s, out); }
            if let Some(s) = sup { walk_unknown(s, out); }
        }
        Expr::Frac { num, den } => {
            walk_unknown(num, out);
            walk_unknown(den, out);
        }
        Expr::Sqrt { degree, body } => {
            if let Some(d) = degree { walk_unknown(d, out); }
            walk_unknown(body, out);
        }
        Expr::Binom { n, k } => {
            walk_unknown(n, out);
            walk_unknown(k, out);
        }
        Expr::Bigop { sub, sup, .. } => {
            if let Some(s) = sub { walk_unknown(s, out); }
            if let Some(s) = sup { walk_unknown(s, out); }
        }
        Expr::LeftRight { body, .. } => walk_unknown(body, out),
        Expr::Matrix { rows, .. } => {
            for row in rows {
                for cell in row { walk_unknown(cell, out); }
            }
        }
        Expr::Accent { body, .. } | Expr::Line { body, .. } | Expr::Styled { body, .. } => {
            walk_unknown(body, out)
        }
        Expr::Stack { base, top, bottom } => {
            walk_unknown(base, out);
            if let Some(t) = top { walk_unknown(t, out); }
            if let Some(b) = bottom { walk_unknown(b, out); }
        }
        Expr::Lines(v) => v.iter().for_each(|e| walk_unknown(e, out)),
        Expr::Phantom { body, .. } => walk_unknown(body, out),
        // 叶子节点，不会有 Unknown
        Expr::Atom { .. } | Expr::Symbol(_) | Expr::Text(_) => {}
    }
}

pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let toks = tokenize(src);
    let mut p = Parser { toks, pos: 0 };
    let e = p.parse_row(true)?;
    if p.pos < p.toks.len() {
        return Err(ParseError(format!(
            "unexpected trailing tokens at {}: {:?}",
            p.pos, p.toks[p.pos]
        )));
    }
    Ok(e)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> { self.toks.get(self.pos) }
    fn next(&mut self) -> Option<Token> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() { self.pos += 1; }
        t
    }

    /// 解析一个"原子"：单个 Atom/Symbol/组/构造/环境 均可，**不含**上下标。
    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.peek().cloned() {
            None => Err(ParseError("unexpected end of input".into())),
            Some(Token::BeginGroup) => self.parse_group_body(),   // 组 = 内联行（渲染期视为原子）
            Some(Token::EndGroup) => Err(ParseError("unmatched }".into())),
            Some(Token::Sub) => Err(ParseError("stray _".into())),
            Some(Token::Sup) => Err(ParseError("stray ^".into())),
            Some(Token::Amp) => { self.next(); Ok(Expr::Row(vec![])) }   // & 在矩阵外当空
            Some(Token::Eol) => { self.next(); Ok(Expr::Row(vec![])) }
            Some(Token::Char(c)) => {
                self.next();
                Ok(Expr::Atom { ch: c, cmd: None })
            }
            Some(Token::ControlSeq(name)) => {
                self.next();   // 消费命令名 token 本身，parse_control_seq 从参数开始
                self.parse_control_seq(name)
            }
        }
    }

    /// `\name` 分派：已知构造/符号 → AST；未知 → Unknown。
    fn parse_control_seq(&mut self, name: String) -> Result<Expr, ParseError> {
        match name.as_str() {
            // ---- 双参构造 ----
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let num = self.parse_required_group("frac numerator")?;
                let den = self.parse_required_group("frac denominator")?;
                Ok(Expr::Frac { num: Box::new(num), den: Box::new(den) })
            }
            "binom" | "dbinom" | "tbinom" => {
                let n = self.parse_required_group("binom top")?;
                let k = self.parse_required_group("binom bottom")?;
                Ok(Expr::Binom { n: Box::new(n), k: Box::new(k) })
            }
            // ---- 上标记重音（词法：\hat{body}，body 是必选组）----
            "hat" | "check" | "breve" | "acute" | "grave" | "tilde" | "bar" | "vec"
            | "dot" | "ddot" | "widehat" | "widetilde" => {
                let body = self.parse_required_group("accent body")?;
                Ok(Expr::Accent { name, body: Box::new(body) })
            }
            // ---- 占位：保留尺寸但不绘制 ----
            "phantom" | "hphantom" | "vphantom" | "smash" => {
                let (keep_w, keep_h) = match name.as_str() {
                    "phantom" => (true, true),
                    "hphantom" => (true, false),
                    "vphantom" => (false, true),
                    _ => (true, false),   // smash：保留宽度，高度深度归零
                };
                let body = self.parse_required_group("phantom body")?;
                Ok(Expr::Phantom { body: Box::new(body), keep_w, keep_h })
            }
            // ---- 堆叠：\\overset{top}{base} / \\underset{bottom}{base} / \\stackrel{top}{base} ----
            "overset" | "stackrel" => {
                let top = self.parse_required_group("overset top")?;
                let base = self.parse_required_group("overset base")?;
                Ok(Expr::Stack { base: Box::new(base), top: Some(Box::new(top)), bottom: None })
            }
            "underset" => {
                let bottom = self.parse_required_group("underset bottom")?;
                let base = self.parse_required_group("underset base")?;
                Ok(Expr::Stack { base: Box::new(base), top: None, bottom: Some(Box::new(bottom)) })
            }
            "substack" => {
                let mut lines = Vec::new();
                if matches!(self.peek(), Some(Token::BeginGroup)) {
                    self.next();
                    loop {
                        lines.push(self.parse_row(true)?);
                        match self.peek() {
                            Some(Token::Eol) => { self.next(); }
                            Some(Token::EndGroup) => { self.next(); break; }
                            _ => break,
                        }
                    }
                }
                Ok(Expr::Lines(lines))
            }
            // ---- 字体族：\\mathbb{R} 这类，把参数里的字母换成对应数学字母区 ----
            "mathbb" | "mathbf" | "mathcal" | "mathscr" | "mathfrak" | "mathsf" | "mathit"
            | "boldsymbol" => {
                let fs = match name.as_str() {
                    "mathbb" => FontStyle::DoubleStruck,
                    "mathbf" | "boldsymbol" => FontStyle::Bold,
                    "mathcal" | "mathscr" => FontStyle::Script,
                    "mathfrak" => FontStyle::Fraktur,
                    "mathsf" => FontStyle::SansSerif,
                    _ => FontStyle::Italic,
                };
                let body = self.parse_required_group("styled body")?;
                Ok(Expr::Styled { style: fs, body: Box::new(body) })
            }
            // ---- 上下划线 ----
            "overline" | "underline" => {
                let body = self.parse_required_group("line body")?;
                Ok(Expr::Line { name, body: Box::new(body) })
            }
            // ---- 根号 ----
            "sqrt" => {
                // 可选 [degree]：`[` 是字面方括号 token，收集到 `]` 为止（`]` 也是 Char，
                // 不能跑完整 parse_row——它会吃掉 `]` 当普通原子）
                let degree = if self.toks.get(self.pos) == Some(&Token::Char('[')) {
                    self.next();   // 吃掉 `[`
                    let mut d_toks: Vec<Token> = Vec::new();
                    while let Some(t) = self.next() {
                        if t == Token::Char(']') { break; }
                        d_toks.push(t);
                    }
                    let mut sub = Parser { toks: d_toks, pos: 0 };
                    Some(Box::new(sub.parse_row(true).unwrap_or(Expr::Row(vec![]))))
                } else { None };
                let body = self.parse_required_group("sqrt body")?;
                Ok(Expr::Sqrt { degree, body: Box::new(body) })
            }
            "root" => {
                // \root <exp> \of <exp> 老式语法
                let deg = self.parse_row(true)?;
                if self.toks.get(self.pos) == Some(&Token::ControlSeq("of".into())) { self.next(); }
                let body = self.parse_row(true)?;
                Ok(Expr::Sqrt { degree: Some(Box::new(deg)), body: Box::new(body) })
            }
            // ---- 大运算符（自带 limits）----
            "sum" | "int" | "prod" | "bigcup" | "bigcap" | "bigoplus" | "bigotimes" |
            "bigvee" | "bigwedge" | "coprod" | "oint" | "iint" | "iiint" |
            "lim" | "limsup" | "liminf" | "max" | "min" | "sup" | "inf" | "det" | "gcd" => {
                let (sub, sup) = self.parse_scripts_hungry()?;
                Ok(Expr::Bigop { name, sub, sup })
            }
            // ---- 逻辑运算符名（非大运算符，普通符号+可选脚本）----
            "sin" | "cos" | "tan" | "log" | "ln" | "exp" | "sec" | "csc" | "cot" |
            "arcsin" | "arccos" | "arctan" | "sinh" | "cosh" | "tanh" | "arg" | "deg" | "dim" |
            "ker" | "hom" | "Pr" | "mod" | "bmod" | "pmod" | "sgn" => {
                // 逻辑符名：字号稍大、直立（布局层按 symbol 处理），可带脚本
                let (sub, sup) = self.parse_scripts_hungry()?;
                Ok(Expr::Scripts {
                    base: Box::new(Expr::Symbol(name)),
                    sub, sup,
                })
            }
            // ---- limits 修饰符（TeX：告诉 TeX 上下限放运算符上下/右侧；引擎默认上下即 limits 语义）----
            // 吞掉不产生原子——`\sum\limits_{i=1}^{n}` 与 `\sum_{i=1}^{n}` 行为一致
            "limits" | "nolimits" | "displaylimits" => Ok(Expr::Row(vec![])),
            // ---- 分隔符组 ----
            "left" => parse_left_right(self),
            // ---- 文本模式 ----
            "text" | "mbox" | "textrm" | "mathrm" | "textbf" | "textit" | "operatorname" => {
                let s = self.collect_text_group()?;
                Ok(Expr::Text(s))
            }
            // ---- 表格环境 ----
            "begin" => parse_matrix_env(self),
            // ---- 点分隔符 ----
            "." => {
                // \left. 或 \right. 的空分隔符：布局层处理；这里当普通符号占位
                Ok(Expr::Symbol(".".into()))
            }
            // ---- 标点/空格命令：当作符号（布局层给宽度）----
            "," | ";" | ":" | "!" | "quad" | "qquad" | "enspace" | "thinspace" | "negthinspace" => {
                Ok(Expr::Symbol(name))
            }
            // ---- 已知符号命令全集（渲染层查表给 glyph）----
            _ => {
                // 尝试作为单个符号命令；若是 `\alpha` 这类，归 Symbol（解析 sem 上无法区分全部，
                // 一律交给渲染层查表；这里保守给 Symbol，Unknown 只留给明显非符号的构造）。
                if looks_like_greek_or_symbol(&name) {
                    Ok(Expr::Symbol(name))
                } else {
                    Ok(Expr::Unknown(name))
                }
            }
        }
    }

    /// 解析 `_`/`^`（可能两个都出现，顺序任意）。返回 (sub, sup)。
    fn parse_scripts_hungry(&mut self) -> Result<(Option<Box<Expr>>, Option<Box<Expr>>), ParseError> {
        let mut sub = None;
        let mut sup = None;
        loop {
            match self.peek().cloned() {
                Some(Token::Sub) => {
                    self.next();
                    let a = self.parse_script_arg()?;
                    sub = Some(Box::new(a));
                }
                Some(Token::Sup) => {
                    self.next();
                    let a = self.parse_script_arg()?;
                    sup = Some(Box::new(a));
                }
                _ => break,
            }
        }
        Ok((sub, sup))
    }

    fn parse_script_arg(&mut self) -> Result<Expr, ParseError> {
        // 脚本参数是一个原子（字符/命令/组）
        match self.peek().cloned() {
            Some(Token::BeginGroup) => self.parse_group_body(),
            Some(Token::Char(ch)) => { self.next(); Ok(Expr::Atom { ch, cmd: None }) }
            Some(Token::ControlSeq(name)) => {
                self.next();
                let e = self.parse_control_seq(name)?;
                Ok(e)
            }
            _ => Err(ParseError("expected script argument".into())),
        }
    }

    /// 解析一行（原子序列 + 上下标挂接）。`stop_at_env` — 矩阵里每格一行。
    fn parse_row(&mut self, _top: bool) -> Result<Expr, ParseError> {
        let mut items: Vec<Expr> = Vec::new();
        let mut cur: Option<Expr> = None;
        loop {
            match self.peek().cloned() {
                None => break,
                Some(Token::EndGroup) => break,       // 调用方处理
                Some(Token::Amp) => break,            // 调用方（矩阵）处理
                Some(Token::Eol) => break,            // 调用方（矩阵）处理
                Some(Token::Sub) | Some(Token::Sup) => {
                    // 上下标：挂到 cur（若无 cur 则孤立脚本 = 宽容处理）
                    let (sub, sup) = self.parse_scripts_hungry()?;
                    let base = cur.take().unwrap_or(Expr::Row(vec![]));
                    cur = Some(Expr::Scripts {
                        base: Box::new(base),
                        sub: sub.map(|s| Box::new(Expr::Row(vec![*s]))),
                        sup: sup.map(|s| Box::new(Expr::Row(vec![*s]))),
                    });
                }
                _ => {
                    let atom = self.parse_atom()?;
                    // 原子后面紧跟上下标 → 吃掉挂上
                    if matches!(self.peek(), Some(Token::Sub) | Some(Token::Sup)) {
                        let (sub, sup) = self.parse_scripts_hungry()?;
                        cur = Some(Expr::Scripts {
                            base: Box::new(atom),
                            sub: sub.map(|s| Box::new(Expr::Row(vec![*s]))),
                            sup: sup.map(|s| Box::new(Expr::Row(vec![*s]))),
                        });
                    } else {
                        cur = Some(atom);
                    }
                }
            }
            if let Some(c) = cur.take() { items.push(c); }
        }
        // 单个元素 → 直接返回它；多个 → Row
        match items.len() {
            0 => Ok(Expr::Row(vec![])),
            1 => Ok(items.into_iter().next().unwrap()),
            _ => Ok(Expr::Row(items)),
        }
    }

    fn expect_end_group(&mut self) -> Result<(), ParseError> {
        match self.next() {
            Some(Token::EndGroup) => Ok(()),
            _ => Err(ParseError("expected }".into())),
        }
    }

    /// 解析一个 `{...}` 组的内容（开头的 `{` 已被消费前调用方 peek 到）。
    ///
    /// 组里可能有 `\\` 换行（`\\substack{a\\\\b}` 这类），换行本该由环境吞掉；
    /// 这里宽容地当成顺序拼接，而不是让整条公式解析失败。
    fn parse_group_body(&mut self) -> Result<Expr, ParseError> {
        self.next(); // 吃掉 `{`
        let mut parts = vec![self.parse_row(true)?];
        while matches!(self.peek(), Some(Token::Eol)) {
            self.pos += 1;
            if matches!(self.peek(), Some(Token::EndGroup)) {
                break;
            }
            parts.push(self.parse_row(true)?);
        }
        self.expect_end_group()?;
        Ok(if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            Expr::Row(parts)
        })
    }

    fn parse_required_group(&mut self, what: &str) -> Result<Expr, ParseError> {
        match self.peek().cloned() {
            Some(Token::BeginGroup) => self.parse_group_body(),
            // TeX 允许命令参数不带花括号：`\vec F`、`\frac12` 都是合法的，
            // 取紧随其后的单个字符当参数。之前的实现一律报错，真实文档里
            // `\vec F\cdot d\vec S` 这种写法很常见。
            Some(Token::Char(c)) => {
                self.next();
                Ok(Expr::Atom { ch: c, cmd: None })
            }
            _ => Err(ParseError(format!("expected {{}} group for {what}"))),
        }
    }

    /// 收集 `\text{...}` 的原文字符（不做数学解析，保留原样）。
    fn collect_text_group(&mut self) -> Result<String, ParseError> {
        match self.next() {
            Some(Token::BeginGroup) => {
                let mut s = String::new();
                loop {
                    match self.next() {
                        None | Some(Token::EndGroup) => break,
                        Some(Token::Char(c)) => s.push(c),
                        Some(Token::ControlSeq(_c)) => s.push('\\'),
                        _ => {}
                    }
                }
                Ok(s)
            }
            _ => Err(ParseError("expected { } for \\text".into())),
        }
    }
}

/// `\left( ... \right)`：扫描到匹配的 `\right`。调用点（parse_control_seq）已消费 `\left`，
/// 这里直接从分隔符开始。
fn parse_left_right(p: &mut Parser) -> Result<Expr, ParseError> {
    let open = parse_delim(p)?;
    let mut depth = 1usize;
    let mut body_toks: Vec<Token> = Vec::new();
    while let Some(t) = p.next() {
        match &t {
            Token::ControlSeq(n) if n == "left" => { depth += 1; body_toks.push(t); }
            Token::ControlSeq(n) if n == "right" => {
                depth -= 1;
                if depth == 0 { break; }
                body_toks.push(t);
            }
            _ => body_toks.push(t),
        }
    }
    let close = parse_delim(p)?;
    // 用子 Parser 解析 body
    let mut sub = Parser { toks: body_toks, pos: 0 };
    let body = sub.parse_row(true).unwrap_or(Expr::Row(vec![]));
    Ok(Expr::LeftRight { open, body: Box::new(body), close })
}

/// `\left`/`\right` 后面的分隔符：一个字符（`( [ | .`…）或符号命令。
fn parse_delim(p: &mut Parser) -> Result<String, ParseError> {
    match p.next() {
        Some(Token::Char(c)) => Ok(c.to_string()),
        Some(Token::ControlSeq(c)) => Ok(c),
        _ => Err(ParseError("expected delimiter after \\left/\\right".into())),
    }
}

/// `\begin{env} ... \end{env}`：解析为矩阵。行由 `\\` 分隔，列由 `&` 分隔。
/// 调用点（parse_control_seq）已消费 `\begin`，这里直接从 `{env}` 开始。
fn parse_matrix_env(p: &mut Parser) -> Result<Expr, ParseError> {
    let env = match p.next() {
        Some(Token::BeginGroup) => {
            let mut s = String::new();
            loop {
                match p.next() {
                    None | Some(Token::EndGroup) => break,
                    Some(Token::Char(c)) => s.push(c),
                    _ => {}
                }
            }
            s
        }
        _ => return Err(ParseError("expected {env} after \\begin".into())),
    };
    let mut rows: Vec<Vec<Expr>> = Vec::new();
    let mut cur_row: Vec<Expr> = Vec::new();
    let mut cur_cell: Vec<Token> = Vec::new();
    let flush_cell = |cur_cell: &mut Vec<Token>, cur_row: &mut Vec<Expr>| {
        if !cur_cell.is_empty() {
            let mut sub = Parser { toks: std::mem::take(cur_cell), pos: 0 };
            let e = sub.parse_row(true).unwrap_or(Expr::Row(vec![]));
            cur_row.push(e);
        }
    };
    loop {
        match p.next() {
            None => break,   // 未闭合环境：宽容
            Some(Token::ControlSeq(n)) if n == "end" => {
                // \end{env}
                // 吃掉 {env}
                match p.next() {
                    Some(Token::BeginGroup) => {
                        loop {
                            match p.next() {
                                None | Some(Token::EndGroup) => break,
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                flush_cell(&mut cur_cell, &mut cur_row);
                if !cur_row.is_empty() || !rows.is_empty() { rows.push(cur_row.clone()); }
                break;
            }
            Some(Token::Amp) => {
                flush_cell(&mut cur_cell, &mut cur_row);
            }
            Some(Token::Eol) => {
                flush_cell(&mut cur_cell, &mut cur_row);
                rows.push(std::mem::take(&mut cur_row));
            }
            other => if let Some(t) = other { cur_cell.push(t); }
        }
    }
    Ok(Expr::Matrix { env, rows })
}

/// 判定一个命令名是否"像符号"（希腊字母/运算符符号/箭头/关系符等）。
/// 列表覆盖常用数学符号命令全集；不在表里的保守走 Symbol（渲染层查表失败再回退）。
fn looks_like_greek_or_symbol(name: &str) -> bool {
    const GREEK: &[&str] = &[
        "alpha","beta","gamma","delta","epsilon","varepsilon","zeta","eta","theta",
        "vartheta","iota","kappa","lambda","mu","nu","xi","pi","varpi","rho","varrho",
        "sigma","varsigma","tau","upsilon","phi","varphi","chi","psi","omega",
        "Gamma","Delta","Theta","Lambda","Xi","Pi","Sigma","Upsilon","Phi","Psi","Omega",
        "ell","aleph","hbar","imath","jmath","wp","Re","Im","partial","nabla","infty",
        "emptyset","varnothing",
    ];
    const SYMS: &[&str] = &[
        "+","-","*","/","=","<",">","|",":",".",",",";","!","(",")","[","]","{","}",
        "times","div","pm","mp","cdot","ast","star","circ","bullet","oplus","otimes",
        "ominus","oslash","odot","dagger","ddagger","amalg","wr",
        "leq","geq","neq","equiv","sim","simeq","approx","cong","propto","prec","succ",
        "le","ge","ne","leqq","geqq","lt","gt","gets","implies",
        "preceq","succeq","ll","gg","subset","supset","subseteq","supseteq","cup","cap",
        "setminus","in","ni","notin","forall","exists","neg","land","lor","implies",
        "to","rightarrow","leftarrow","leftrightarrow","mapsto","Rightarrow","Leftarrow",
        "Leftrightarrow","uparrow","downarrow","updownarrow","hookrightarrow","hookleftarrow",
        "rightharpoonup","leftharpoondown","rightleftharpoons","longrightarrow","longleftarrow",
        "longrightarrow","longleftrightarrow","iff","Rightarrow","Leftarrow",
        "prime","backslash","angle","triangle","surd","succeq","infty","partial",
        "dots","ldots","cdots","vdots","ddots",
        "hat","check","tilde","bar","vec","dot","ddot","breve","acute","grave",
        "widehat","widetilde","overline","underline","overbrace","underbrace",
        "overleftarrow","overrightarrow",
        "big","Big","bigg","Bigg","bigl","bigr","Bigl","Bigr","biggl","biggr","Biggl","Biggr",
        "left","right","middle","mathopen","mathclose","mathrel","mathbin","mathord","mathop",
        // 2026-11-13 按真实文档补齐的符号命令
        "bigodot","bigsqcup","bigstar","biguplus","iiiint","circledast","circledcirc","circleddash",
        "uplus","sqcup","sqcap","smallint","oiint","oiiint","intop","ulcorner","urcorner","llcorner","lrcorner","llbracket","rrbracket","llparenthesis","rrparenthesis",
        "nearrow","searrow","swarrow","nwarrow","leftharpoonup","leftharpoondown","rightharpoonup","rightharpoondown","rightleftharpoons","leftrightharpoons","rightrightarrows","leftleftarrows","rightleftarrows","leftrightarrows","upuparrows","downdownarrows","upharpoonleft","upharpoonright","downharpoonleft","downharpoonright","curvearrowleft","curvearrowright","circlearrowleft","circlearrowright","looparrowleft","looparrowright","multimap","rightsquigarrow","leadsto","dashrightarrow","dashleftarrow","twoheadrightarrow","twoheadleftarrow","Longleftarrow","Longrightarrow","Longleftrightarrow","Uparrow","Downarrow","Updownarrow","nsubseteq","nsupseteq","subsetneq","supsetneq","varsubsetneq","varsupsetneq","sqsubset","sqsupset","sqsubseteq","sqsupseteq","Subset","Supset","owns","complement","smallsetminus","nleq","ngeq","nless","ngtr","nsim","ncong","nparallel","nmid","nprec","nsucc","triangleq","eqsim","approxeq","asymp","smile","frown","models","vDash","Vdash","VDash","nvdash","nVdash","perp","parallel","mid","shortmid","shortparallel","nexists","lnot","wedge","vee","impliedby","iff","blacklozenge","diamond","lozenge","boxplus","boxminus","boxtimes","boxdot","dotplus","divideontimes","ltimes","rtimes","leftthreetimes","rightthreetimes","barwedge","veebar","doublebarwedge","curlywedge","curlyvee","Cup","Cap","intercal","triangle","blacktriangle","blacktriangledown","blacktriangleleft","blacktriangleright","blacksquare","square","Box","Diamond",
    ];
    GREEK.contains(&name) || SYMS.contains(&name)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 原子序列() {
        let e = parse("x+1").unwrap();
        assert_eq!(
            e,
            Expr::Row(vec![
                Expr::Atom { ch: 'x', cmd: None },
                Expr::Atom { ch: '+', cmd: None },
                Expr::Atom { ch: '1', cmd: None },
            ])
        );
    }

    #[test]
    fn 上下标() {
        let e = parse("x^2").unwrap();
        assert_eq!(
            e,
            Expr::Scripts {
                base: Box::new(Expr::Atom { ch: 'x', cmd: None }),
                sub: None,
                sup: Some(Box::new(Expr::Row(vec![Expr::Atom { ch: '2', cmd: None }]))),
            }
        );
    }

    #[test]
    fn 双上下标() {
        let e = parse("a_i^2").unwrap();
        match e {
            Expr::Scripts { base, sub, sup } => {
                assert_eq!(*base, Expr::Atom { ch: 'a', cmd: None });
                assert!(sub.is_some());
                assert!(sup.is_some());
            }
            _ => panic!("expected Scripts"),
        }
    }

    #[test]
    fn 分组作为原子() {
        // {abc} 顶层单元素就地展开为内层序列（parse_row 单元素不套壳）
        let e = parse("{abc}").unwrap();
        assert_eq!(e, Expr::Row(vec![
            Expr::Atom { ch: 'a', cmd: None },
            Expr::Atom { ch: 'b', cmd: None },
            Expr::Atom { ch: 'c', cmd: None },
        ]));
    }

    #[test]
    fn frac() {
        let e = parse(r"\frac{a}{b}").unwrap();
        assert_eq!(
            e,
            Expr::Frac {
                num: Box::new(Expr::Atom { ch: 'a', cmd: None }),
                den: Box::new(Expr::Atom { ch: 'b', cmd: None }),
            }
        );
    }

    #[test]
    fn sqrt_无度() {
        let e = parse(r"\sqrt{x}").unwrap();
        assert_eq!(
            e,
            Expr::Sqrt { degree: None, body: Box::new(Expr::Atom { ch: 'x', cmd: None }) }
        );
    }

    #[test]
    fn sqrt_带度() {
        let e = parse(r"\sqrt[3]{x}").unwrap();
        match e {
            Expr::Sqrt { degree, body } => {
                assert!(degree.is_some());
                assert_eq!(*body, Expr::Atom { ch: 'x', cmd: None });
            }
            _ => panic!("expected Sqrt"),
        }
    }

    #[test]
    fn 希腊字母() {
        let e = parse(r"\alpha").unwrap();
        assert_eq!(e, Expr::Symbol("alpha".into()));
    }

    #[test]
    fn 大运算符带限制() {
        let e = parse(r"\sum_{i=1}^{n}").unwrap();
        match e {
            Expr::Bigop { name, sub, sup } => {
                assert_eq!(name, "sum");
                assert!(sub.is_some());
                assert!(sup.is_some());
            }
            _ => panic!("expected Bigop"),
        }
    }

    #[test]
    fn left_right() {
        let e = parse(r"\left( x \right)").unwrap();
        match e {
            Expr::LeftRight { open, body, close } => {
                assert_eq!(open, "(");
                assert_eq!(close, ")");
                assert_eq!(*body, Expr::Atom { ch: 'x', cmd: None });
            }
            _ => panic!("expected LeftRight"),
        }
    }

    #[test]
    fn left_dot_right_dot() {
        let e = parse(r"\left. x \right|").unwrap();
        match e {
            Expr::LeftRight { open, close, .. } => {
                assert_eq!(open, ".");
                assert_eq!(close, "|");
            }
            _ => panic!("expected LeftRight"),
        }
    }

    #[test]
    fn 矩阵() {
        let e = parse(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}").unwrap();
        match e {
            Expr::Matrix { env, rows } => {
                assert_eq!(env, "pmatrix");
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
                assert_eq!(rows[1].len(), 2);
            }
            _ => panic!("expected Matrix"),
        }
    }

    #[test]
    fn text模式() {
        let e = parse(r"\text{hello}").unwrap();
        assert_eq!(e, Expr::Text("hello".into()));
    }

    #[test]
    fn 未知命令宽容() {
        // 未知命令自身保留为 Unknown，参数照常解析（渲染层决定原文回退）
        let e = parse(r"\fancycmd{x}").unwrap();
        let s = format!("{e:?}");
        assert!(s.contains("Unknown(\"fancycmd\")"), "应保留未知命令: {s}");
    }

    #[test]
    fn 完整公式() {
        // \frac{a}{b} + \sum_{i=1}^{n} x_i^2
        let src = r"\frac{a}{b} + \sum_{i=1}^{n} x_i^2";
        let e = parse(src);
        assert!(e.is_ok(), "parse failed: {:?}", e.err());
        // 结构上：Row[Frac, +, Bigop(sum), Scripts(x, sub=i, sup=2)]
        match e.unwrap() {
            Expr::Row(items) => {
                assert_eq!(items.len(), 4);
            }
            other => panic!("expected Row, got {other:?}"),
        }
    }

    #[test]
    fn 孤立脚本宽容() {
        // 孤立 `^2`（前面无数）：不抛错
        let e = parse("^2").unwrap();
        assert!(matches!(e, Expr::Scripts { .. }));
    }

    #[test]
    fn 不匹配括号宽容处理() {
        // 多一个 }：parse_row 在 EndGroup 停止 → 剩余 } 报 trailing
        let e = parse("a}");
        assert!(e.is_err());
    }
}
