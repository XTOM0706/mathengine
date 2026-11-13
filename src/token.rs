//! LaTeX 数学模式 Tokenizer。
//!
//! 输入一段 math-mode LaTeX（如 `\frac{a}{b} + \sum_{i=0}^{n} x_i^2`），输出 token 流。
//! 只做**切分**不解释语义——把字符串切成：
//!   - `ControlSeq`（`\frac` 这类命令，含 `\alpha`、`\,` 等符号命令）
//!   - `Char`（单个字母/数字/符号字面量）
//!   - `BeginGroup` / `EndGroup`（`{` / `}`）
//!   - `Sub` / `Sup`（`_` / `^`，数学模式的上下标）
//!   - `Amp`（`&`，矩阵/对齐环境的列分隔）
//!   - `Eol`（`\\`，矩阵/对齐环境的换行）
//!   - `Space`（空白，数学模式里忽略，但保留给需要的位置）
//!
//! 命令名解析规则（对齐 TeX）：`\` 后跟一个或多个字母 = 长命令名；
//! `\` 后跟一个非字母 = 单字符控制符号（`\(`、`\,`、`\\`、`\%`…）。
//! 未知命令不做非法处理——交给 parser 层决定（保留原文回退是渲染器的事）。

/// 一个数学 token。
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// `\name` 或 `\symbol`。`name` 不含反斜杠。
    ControlSeq(String),
    /// 单个字面字符（字母/数字/标点/运算符符号）。
    Char(char),
    /// `{`
    BeginGroup,
    /// `}`
    EndGroup,
    /// `_`
    Sub,
    /// `^`
    Sup,
    /// `&`（矩阵行内列分隔）
    Amp,
    /// `\\`（矩阵/对齐环境换行）
    Eol,
}

/// Tokenizer：把 math-mode LaTeX 源码切成一串 [Token]。
///
/// 不做递归（不处理嵌套），纯粹线性扫描。暂停在 `\` 后的命令名边界、
/// 分组、上下标、特殊字符上。空白被跳过（数学模式空白无意义）。
pub fn tokenize(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                // 反斜杠：命令名 or 控制符号
                let mut j = i + 1;
                if j >= chars.len() {
                    // 末尾孤立反斜杠：当作字面 `\` 字符（宽容）
                    out.push(Token::Char('\\'));
                    i = j;
                    continue;
                }
                if chars[j].is_alphabetic() {
                    // 长命令名：字母序列
                    let start = j;
                    while j < chars.len() && chars[j].is_alphabetic() { j += 1; }
                    out.push(Token::ControlSeq(chars[start..j].iter().collect()));
                    i = j;
                } else {
                    // 单字符控制符号：`\\`、`\(`、`\,`、`\%`…
                    // TeX 特殊：`\\` 是矩阵/对齐环境的换行（Eol），其余按符号命令
                    let sym = chars[j].to_string();
                    if chars[j] == '\\' {
                        out.push(Token::Eol);
                    } else {
                        out.push(Token::ControlSeq(sym));
                    }
                    i = j + 1;
                }
            }
            '{' => { out.push(Token::BeginGroup); i += 1; }
            '}' => { out.push(Token::EndGroup); i += 1; }
            '_' => { out.push(Token::Sub); i += 1; }
            '^' => { out.push(Token::Sup); i += 1; }
            '&' => { out.push(Token::Amp); i += 1; }
            c if c.is_whitespace() => { i += 1; }   // 数学模式空白跳过
            _ => { out.push(Token::Char(c)); i += 1; }
        }
    }
    out
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    fn seqs(toks: &[Token]) -> Vec<String> {
        toks.iter().filter_map(|t| match t {
            Token::ControlSeq(s) => Some(s.clone()),
            _ => None,
        }).collect()
    }

    #[test]
    fn 简单公式的token流() {
        let toks = tokenize("x^2 + y_1");
        // x ^ 2 空格 + 空格 y _ 1（空白被跳过）
        assert_eq!(toks, vec![
            Token::Char('x'), Token::Sup, Token::Char('2'),
            Token::Char('+'),
            Token::Char('y'), Token::Sub, Token::Char('1'),
        ]);
    }

    #[test]
    fn 长命令名解析() {
        let toks = tokenize(r"\frac{1}{2}");
        assert_eq!(seqs(&toks), vec!["frac".to_string()]);
        assert_eq!(toks[0], Token::ControlSeq("frac".to_string()));
        assert_eq!(toks[1], Token::BeginGroup);
        assert_eq!(toks[2], Token::Char('1'));
        assert_eq!(toks[3], Token::EndGroup);
    }

    #[test]
    fn 单字符控制符号() {
        // \, 是控制符号（thin space）；\\ 是换行；\( 是左括号控制符
        let toks = tokenize(r"a\,b\\c");
        assert_eq!(seqs(&toks), vec![",".to_string()]);
        assert!(toks.contains(&Token::Eol));
    }

    #[test]
    fn 希腊字母命令是长命令名() {
        let toks = tokenize(r"\alpha+\beta");
        assert_eq!(seqs(&toks), vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn 矩阵分隔符() {
        let toks = tokenize(r"a & b \\ c & d");
        assert!(toks.contains(&Token::Amp));
        assert!(toks.contains(&Token::Eol));
    }

    #[test]
    fn 未知命令宽容处理_不抛错() {
        let toks = tokenize(r"\unknowncmd{x}");
        assert_eq!(seqs(&toks), vec!["unknowncmd".to_string()]);
        assert_eq!(toks.len(), 4); // cmd + { + x + }
    }

    #[test]
    fn 孤立反斜杠宽容() {
        let toks = tokenize("a\\");
        assert_eq!(toks, vec![Token::Char('a'), Token::Char('\\')]);
    }

    #[test]
    fn 中文等非ASCII字符当作字面字符() {
        let toks = tokenize("面积");
        assert_eq!(toks, vec![Token::Char('面'), Token::Char('积')]);
    }
}