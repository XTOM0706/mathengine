//! TeX Box 布局：AST → 布局盒树 → 扁平 GlyphRun（Kotlin/Compose 直接绘制）。
//!
//! 布局模型（对齐 Knuth TeX 数学模式）：
//!   - 每个表达式布局成 `Box`：width/height/depth（font units，baseline 为 y=0，向上为正）
//!   - 盒子含若干 `PlacedGlyph`（glyph + 相对 base 的 x, y + 缩放），以及子盒的绝对定位
//!   - 递归：子表达式 → 相对位移 → 合并成 Row/Stack
//!
//! 最终输出 `Layout`：glyph 扁平列表（Kotlin 按 (x, baseline_y) 画）+ 整体宽/高/深 +
//! 绘制辅助（分数线的 rule）。
//!
//! 度量来源全部是 [crate::font::MathFont]（Latin Modern Math 的 cmap/hmtx/MATH 表）。
//! 单位 = font units；Kotlin 侧缩放。

use crate::ast::{Expr, FontStyle};
use crate::font::MathFont;

/// 一个已放置的 glyph（相对布局基线的绝对位置，font units）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    /// Unicode 字符（Kotlin 用 Typeface 渲染，不需 glyph id）。
    pub ch: char,
    /// 相对**基线原点**的水平偏移（布局内绝对 x）。
    pub x: f32,
    /// 相对基线的垂直偏移（向上为正）。
    pub y: f32,
    /// 缩放系数（1.0 正文 / script / scriptscript）。
    pub scale: f32,
}

/// 一根水平线段（分数线/根号横线），Kotlin drawRect。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub thickness: f32,
}

/// 布局结果：flat 指令 + 整体尺寸。
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub glyphs: Vec<PlacedGlyph>,
    pub rules: Vec<Rule>,
    /// 总宽度。
    pub width: f32,
    /// 基线以上最大高度。
    pub height: f32,
    /// 基线以下最大深度。
    pub depth: f32,
}

/// 内部盒：宽/高/深 + glyph 列表（已在基线的相对坐标）。
#[derive(Debug, Clone)]
struct Box {
    width: f32,
    height: f32,
    depth: f32,
    glyphs: Vec<PlacedGlyph>,
    rules: Vec<Rule>,
}

impl Box {
    fn empty() -> Self { Box { width: 0.0, height: 0.0, depth: 0.0, glyphs: vec![], rules: vec![] } }
}

/// 布局风格（TeX 的 text/script/scriptscript 三级，影响字体缩放与脚本规则）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Style {
    Text,
    Script,
    ScriptScript,
}

impl Style {
    fn scale_of(&self, f: &MathFont) -> f32 {
        match self {
            Style::Text => 1.0,
            Style::Script => f.script_scale,
            Style::ScriptScript => f.scriptscript_scale,
        }
    }
}

/// 用给定字体将 AST 布局成 flat 指令。
pub fn layout(font: &MathFont, ast: &Expr) -> Layout {
    let b = layout_box(font, ast, Style::Text);
    // 转 flat：glyph 坐标已是相对基线的绝对值（layout_box 内已绝对化）
    Layout {
        glyphs: b.glyphs,
        rules: b.rules,
        width: b.width,
        height: b.height,
        depth: b.depth,
    }
}

/// 递归布局。返回的 glyph 坐标是**相对本盒基线**的——调用方（Row）负责加水平偏移。
/// 但为简化，「绝对化」在每一层由父盒完成：子盒布局时坐标系以子盒基线为原点，
/// 父盒把子盒整个平移（glyphs 里每个 x += dx，y += dy）。
/// 把字母映射到对应数学字体族的 Unicode 码点。
///
/// 各区块里都有几个字母在 Unicode 中被单独编在别处（花体的 B/E/F/H/I/L/M/R、
/// 哥特体的 C/H/I/R/Z、双线体的 C/H/N/P/Q/R/Z），必须逐个列出来，否则会画出豆腐。

/// 数学斜体码点 → ASCII。用于字体族切换前还原。
fn unitalic(ch: char) -> char {
    let c = ch as u32;
    if (0x1D434..=0x1D44D).contains(&c) {
        return char::from_u32(0x41 + (c - 0x1D434)).unwrap_or(ch);
    }
    if (0x1D44E..=0x1D467).contains(&c) {
        return char::from_u32(0x61 + (c - 0x1D44E)).unwrap_or(ch);
    }
    if c == 0x210E {
        return 'h';
    }
    ch
}

fn map_font_style(fs: FontStyle, ch: char) -> char {
    // 布局原子时字母已被映射成数学斜体码点（TeX 变量默认斜体），
    // 所以这里要先把斜体码点还原成 ASCII，才能套上 `\mathbb` 这类目标字体。
    // 不还原的话，`\mathbb{R}` 里的 R 已经是 𝑅，不在 A-Z 区间，会被整条跳过。
    let ch = unitalic(ch);
    let c = ch as u32;
    let upper = (0x41..=0x5A).contains(&c);
    let lower = (0x61..=0x7A).contains(&c);
    if !upper && !lower {
        return ch;
    }
    let idx = if upper { c - 0x41 } else { c - 0x61 };
    let mapped = match fs {
        FontStyle::Upright => return ch,
        FontStyle::Bold => {
            if upper { 0x1D400 + idx } else { 0x1D41A + idx }
        }
        FontStyle::Italic => match ch {
            'h' => 0x210E,
            _ => if upper { 0x1D434 + idx } else { 0x1D44E + idx },
        },
        FontStyle::Script => match ch {
            'B' => 0x212C, 'E' => 0x2130, 'F' => 0x2131, 'H' => 0x210B,
            'I' => 0x2110, 'L' => 0x2112, 'M' => 0x2133, 'R' => 0x211B,
            'e' => 0x212F, 'g' => 0x210A, 'o' => 0x2134,
            _ => if upper { 0x1D49C + idx } else { 0x1D4B6 + idx },
        },
        FontStyle::Fraktur => match ch {
            'C' => 0x212D, 'H' => 0x210C, 'I' => 0x2111, 'R' => 0x211C, 'Z' => 0x2128,
            _ => if upper { 0x1D504 + idx } else { 0x1D51E + idx },
        },
        FontStyle::DoubleStruck => match ch {
            'C' => 0x2102, 'H' => 0x210D, 'N' => 0x2115, 'P' => 0x2119,
            'Q' => 0x211A, 'R' => 0x211D, 'Z' => 0x2124,
            _ => if upper { 0x1D538 + idx } else { 0x1D552 + idx },
        },
        FontStyle::SansSerif => {
            if upper { 0x1D5A0 + idx } else { 0x1D5BA + idx }
        }
    };
    char::from_u32(mapped).unwrap_or(ch)
}


/// `\overset` / `\underset` / `\stackrel`：base 留在基线，另一个压在它上方或下方。
fn layout_stack(
    font: &MathFont,
    base: &Expr,
    top: Option<&Expr>,
    bottom: Option<&Expr>,
    style: Style,
) -> Box {
    let scale = style.scale_of(font);
    let base_box = layout_box(font, base, style);
    let gap = font.units_per_em() * 0.1 * scale;
    let mut out = base_box.clone();
    if let Some(t) = top {
        let tb = layout_box(font, t, Style::Script);
        let dx = (base_box.width - tb.width) / 2.0;
        let dy = base_box.height + gap + tb.depth;
        merge_at(&mut out, &tb, dx, dy);
        out.height = out.height.max(dy + tb.height);
        out.width = out.width.max(base_box.width);
    }
    if let Some(b) = bottom {
        let bb = layout_box(font, b, Style::Script);
        let dx = (base_box.width - bb.width) / 2.0;
        let dy = -(base_box.depth + gap + bb.height);
        merge_at(&mut out, &bb, dx, dy);
        out.depth = out.depth.max(-dy + bb.depth);
        out.width = out.width.max(base_box.width);
    }
    out
}

/// `\substack{a\\b}`：多行小字垂直堆叠，整体水平居中。用于求和号的下限之类。
fn layout_lines(font: &MathFont, lines: &[Expr], style: Style) -> Box {
    if lines.is_empty() {
        return Box::empty();
    }
    let scale = style.scale_of(font);
    let boxes: Vec<Box> = lines.iter().map(|l| layout_box(font, l, Style::Script)).collect();
    let width = boxes.iter().map(|b| b.width).fold(0.0, f32::max);
    let gap = font.units_per_em() * 0.08 * scale;

    let mut out = Box::empty();
    // 第一行坐在基线上，后面的行依次往下排；整体高度/深度按实际算，
    // 否则整个盒子都沉到基线下、height=0，单独渲染时顶部会被切掉。
    let mut baseline = 0.0f32;
    let mut prev_depth = 0.0f32;
    for (i, b) in boxes.iter().enumerate() {
        if i > 0 {
            baseline -= prev_depth + gap + b.height;
        }
        merge_at(&mut out, b, (width - b.width) / 2.0, baseline);
        prev_depth = b.depth;
    }
    out.width = width;
    out.height = out.height.max(0.0);
    out.depth = out.depth.max(0.0);
    out
}

fn layout_box(font: &MathFont, expr: &Expr, style: Style) -> Box {
    let scale = style.scale_of(font);
    match expr {
        Expr::Row(items) => layout_row(font, items, style),
        Expr::Atom { ch, cmd } => {
            let _ = cmd;
            layout_char(font, *ch, scale, 0.0)
        }
        Expr::Symbol(name) => layout_symbol(font, name, scale),
        Expr::Scripts { base, sub, sup } => layout_scripts(font, base.as_ref(), sub.as_deref(), sup.as_deref(), style),
        Expr::Frac { num, den } => layout_fraction(font, num.as_ref(), den.as_ref(), style),
        Expr::Sqrt { degree, body } => layout_sqrt(font, degree.as_deref(), body.as_ref(), style),
        Expr::Binom { n, k } => {
            // 二项式：上下堆叠（无分数线）+ 圆括号。
            // 旧实现把括号留给「绘制层」，但绘制层拿到的只是字形指令、不知道这里该包括号，
            // 结果 \binom{n}{k} 渲染出来只有上下两个数。这里直接补上。
            let b = layout_stacked(font, n.as_ref(), k.as_ref(), style, false);
            let gap = font.units_per_em() * 0.04 * style.scale_of(font);
            wrap_delimiters(font, &b, Some('('), Some(')'), gap, style.scale_of(font), 1.16)
        }
        Expr::Text(s) => {
            // 文本模式：按普通字符排，字号同正文
            layout_text_run(font, &s.chars().collect::<Vec<_>>(), scale)
        }
        Expr::Bigop { name, sub, sup } => layout_bigop(font, name, sub.as_deref(), sup.as_deref(), style),
        Expr::LeftRight { open, body, close } => layout_left_right(font, open, body.as_ref(), close, style),
        Expr::Matrix { env, rows } => layout_matrix(font, env, rows, style),
        Expr::Accent { name, body } => layout_accent(font, name, body.as_ref(), style),
        Expr::Line { name, body } => layout_line(font, name, body.as_ref(), style),
        Expr::Stack { base, top, bottom } => layout_stack(font, base.as_ref(), top.as_deref(), bottom.as_deref(), style),
        Expr::Lines(lines) => layout_lines(font, lines, style),
        Expr::Phantom { body, keep_w, keep_h } => {
            let b = layout_box(font, body, style);
            Box {
                width: if *keep_w { b.width } else { 0.0 },
                height: if *keep_h { b.height } else { 0.0 },
                depth: if *keep_h { b.depth } else { 0.0 },
                glyphs: vec![],
                rules: vec![],
            }
        }
        Expr::Styled { style: fs, body } => {
            // 先按原样布局，再把每个字形的码点换成对应数学字母区。
            // 数学字母区的字宽与普通字母一致，位置不必重算。
            let mut b = layout_box(font, body, style);
            for g in &mut b.glyphs {
                g.ch = map_font_style(*fs, g.ch);
            }
            b
        }
        Expr::Unknown(_) => Box::empty(),
    }
}

/// 上标重音（`\hat{x}`/`\bar{x}`/`\vec{v}`…）：body 保持，上方画符号（unscaled Unicode 重音 glyph）。
fn layout_accent(font: &MathFont, name: &str, body: &Expr, style: Style) -> Box {
    let scale = style.scale_of(font);
    let body_box = layout_box(font, body, style);
    let acc: Option<char> = match name {
        "hat" => Some('^'), "widehat" => Some('^'),
        "bar" => Some('¯'),
        "vec" => Some('→'), "tilde" => Some('˜'), "widetilde" => Some('˜'),
        "dot" => Some('˙'), "ddot" => Some('¨'),
        "check" => Some('ˇ'), "breve" => Some('˘'), "acute" => Some('´'), "grave" => Some('`'),
        _ => None,
    };
    let mut out = body_box.clone();
    if let Some(c) = acc {
        let as_ = scale * 0.7;
        let ab = layout_char(font, c, as_, 0.0);
        // 按**墨迹**居中而不是按 advance 居中：`^` `¯` 这类字符左右侧留白很不对称，
        // 用 advance 居中会让帽子整体偏向左边。
        let (amin, amax) = font.ink_x_extent(c);
        let ink_w = (amax - amin) * as_;
        let dx = (body_box.width - ink_w) / 2.0 - amin * as_;
        // 重音紧贴字顶，留一点缝即可；0.05em 会让帽子和字母之间空出一道。
        let dy = body_box.height + font.units_per_em() * 0.015 * scale;
        merge_at(&mut out, &ab, dx, dy);
        out.height = body_box.height.max(dy + ab.height * 0.6);
        out.width = body_box.width.max(ab.width);
    }
    out
}

/// 上/下划线（`\overline{ab}` / `\underline{ab}`）：body 顶部/底部画一条 rule（Kotlin 侧 drawRect）。
fn layout_line(font: &MathFont, name: &str, body: &Expr, style: Style) -> Box {
    let scale = style.scale_of(font);
    let ups = font.units_per_em();
    let body_box = layout_box(font, body, style);
    let thickness = ups * 0.05 * scale;
    let mut out = body_box.clone();
    if name == "overline" {
        let y = body_box.height + thickness * 0.5;
        out.rules.push(Rule { x: 0.0, y, width: body_box.width, thickness });
        out.height = y + thickness;
    } else {
        let y = -body_box.depth - thickness * 0.8;
        out.rules.push(Rule { x: 0.0, y, width: body_box.width, thickness });
        out.depth = -y + thickness;
    }
    out
}

/// 字符盒：glyph + advance。盒高深用**真实墨迹**（而非 hhea 全局上行/下行），
/// 否则所有 body 盒虚高 → 分数/根号/上下标/括号/矩阵的高度与覆盖几何全错乱。
fn layout_char(font: &MathFont, ch: char, scale: f32, dx: f32) -> Box {
    // 数学变量斜体：传给绘制的码点也用数学斜体（与 advance 同一映射，保证位置与字形一致）。
    // LM Math 含全部数学字母字形，Kotlin/drawText 直接画该码点即得 TeX 式斜体变量。
    let glyph_ch = crate::font::to_math_italic(ch);
    let advance = font.advance(ch) * scale;
    // 真实墨迹上/下行；无字形（CJK/回退）用全局度量兜底。
    let (asc, desc) = font.ink_extents(glyph_ch).unwrap_or((font.ascender, font.descender));
    Box {
        width: advance,
        height: asc * scale,
        depth: desc * scale,
        glyphs: vec![PlacedGlyph { ch: glyph_ch, x: dx, y: 0.0, scale }],
        rules: vec![],
    }
}

/// 正体字符盒（不套数学斜体映射）：`\lim`/`\max`/`\det` 等运算符名用正体。
fn layout_char_upright(font: &MathFont, ch: char, scale: f32, dx: f32) -> Box {
    let advance = font.advance_plain(ch) * scale;
    let (asc, desc) = font.ink_extents_plain(ch).unwrap_or((font.ascender, font.descender));
    Box {
        width: advance,
        height: asc * scale,
        depth: desc * scale,
        glyphs: vec![PlacedGlyph { ch, x: dx, y: 0.0, scale }],
        rules: vec![],
    }
}

/// 正体横向排（运算符名）：与 layout_text_run 相同，但不套数学斜体。
fn layout_text_run_upright(font: &MathFont, chars: &[char], scale: f32) -> Box {
    let mut out = Box::empty();
    for &ch in chars {
        let cb = layout_char_upright(font, ch, scale, 0.0);
        let cbm = translate(&cb, out.width, 0.0);
        out.width += cb.width;
        out.height = out.height.max(cb.height);
        out.depth = out.depth.max(cb.depth);
        out.glyphs.extend_from_slice(&cbm.glyphs);
        out.rules.extend_from_slice(&cbm.rules);
    }
    out
}

/// 符号命令 → glyph（查表：希腊字母/运算符映射到 Unicode 数学符号）。
fn layout_symbol(font: &MathFont, name: &str, scale: f32) -> Box {
    let ch = symbol_to_char(name);
    match ch {
        Some(c) => layout_char(font, c, scale, 0.0),
        None => Box::empty(),   // 未知符号：空（渲染层回退由 caller 处理）
    }
}

/// 数学符号命令 → Unicode 字符（覆盖常用全集；查不到的返回 None）。
fn symbol_to_char(name: &str) -> Option<char> {
    use std::collections::HashMap;
    // 静态表，避免每次构建
    // 惰性初始化
    fn table() -> &'static HashMap<&'static str, char> {
        use std::sync::OnceLock;
        static M: OnceLock<HashMap<&'static str, char>> = OnceLock::new();
        M.get_or_init(|| {
            let mut m = HashMap::new();
            // 希腊字母（小写）
            for (n, c) in [
                ("alpha", 'α'), ("beta", 'β'), ("gamma", 'γ'), ("delta", 'δ'),
                ("epsilon", 'ϵ'), ("varepsilon", 'ε'), ("zeta", 'ζ'), ("eta", 'η'),
                ("theta", 'θ'), ("vartheta", 'ϑ'), ("iota", 'ι'), ("kappa", 'κ'),
                ("lambda", 'λ'), ("mu", 'μ'), ("nu", 'ν'), ("xi", 'ξ'), ("pi", 'π'),
                ("varpi", 'ϖ'), ("rho", 'ρ'), ("varrho", 'ϱ'), ("sigma", 'σ'),
                ("varsigma", 'ς'), ("tau", 'τ'), ("upsilon", 'υ'), ("phi", 'ϕ'),
                ("varphi", 'φ'), ("chi", 'χ'), ("psi", 'ψ'), ("omega", 'ω'),
                // 大写
                ("Gamma", 'Γ'), ("Delta", 'Δ'), ("Theta", 'Θ'), ("Lambda", 'Λ'),
                ("Xi", 'Ξ'), ("Pi", 'Π'), ("Sigma", 'Σ'), ("Upsilon", 'Υ'),
                ("Phi", 'Φ'), ("Psi", 'Ψ'), ("Omega", 'Ω'),
                // 大写数学斜体特殊
                ("ell", 'ℓ'), ("hbar", 'ℏ'), ("imath", 'ı'), ("jmath", 'ȷ'),
                ("wp", '℘'), ("Re", 'ℜ'), ("Im", 'ℑ'), ("aleph", 'ℵ'),
                ("partial", '∂'), ("nabla", '∇'), ("infty", '∞'), ("emptyset", '∅'),
                ("varnothing", '∅'),
            ] { m.insert(n, c); }
            // 运算符/关系/箭头
            for (n, c) in [
                ("times", '×'), ("div", '÷'), ("pm", '±'), ("mp", '∓'), ("cdot", '⋅'),
                ("ast", '∗'), ("star", '⋆'), ("circ", '∘'), ("bullet", '∙'),
                ("leq", '≤'), ("geq", '≥'), ("neq", '≠'), ("equiv", '≡'), ("sim", '∼'),
                ("le", '≤'), ("ge", '≥'), ("ne", '≠'), ("leqq", '≦'), ("geqq", '≧'),
                ("simeq", '≃'), ("approx", '≈'), ("cong", '≅'), ("propto", '∝'),
                ("prec", '≺'), ("succ", '≻'), ("preceq", '⪯'), ("succeq", '⪰'),
                ("ll", '≪'), ("gg", '≫'), ("subset", '⊂'), ("supset", '⊃'),
                ("subseteq", '⊆'), ("supseteq", '⊇'), ("cup", '∪'), ("cap", '∩'),
                ("setminus", '∖'), ("in", '∈'), ("ni", '∋'), ("notin", '∉'),
                ("forall", '∀'), ("exists", '∃'), ("neg", '¬'), ("land", '∧'),
                ("lor", '∨'), ("oplus", '⊕'), ("otimes", '⊗'), ("ominus", '⊖'),
                ("oslash", '⊘'), ("odot", '⊙'), ("dagger", '†'), ("ddagger", '‡'),
                ("dots", '…'), ("ldots", '…'), ("cdots", '⋯'), ("vdots", '⋮'), ("ddots", '⋱'),
                ("prime", '′'), ("backslash", '∖'), ("angle", '∠'), ("triangle", '△'),
                ("surd", '√'), ("succeq", '⪰'), ("in", '∈'),
                ("to", '→'), ("rightarrow", '→'), ("leftarrow", '←'), ("leftrightarrow", '↔'),
                ("mapsto", '↦'), ("Rightarrow", '⇒'), ("Leftarrow", '⇐'), ("Leftrightarrow", '⇔'),
                ("uparrow", '↑'), ("downarrow", '↓'), ("updownarrow", '↕'),
                ("hookrightarrow", '↪'), ("hookleftarrow", '↩'),
                ("longrightarrow", '⟶'), ("longleftarrow", '⟵'), ("longleftrightarrow", '⟷'),
                ("iff", '⟺'), ("implies", '⟹'), ("impliedby", '⟸'), ("gets", '←'),
                ("lt", '<'), ("gt", '>'),
                ("land", '∧'), ("lor", '∨'),
                ("bigodot", '\u{2299}'),
                ("bigsqcup", '\u{2A06}'),
                ("bigstar", '\u{2605}'),
                ("biguplus", '\u{2A04}'),
                ("iiiint", '\u{2A0C}'),
                ("circledast", '\u{229B}'),
                ("circledcirc", '\u{229A}'),
                ("circleddash", '\u{229D}'),
                ("uplus", '\u{228E}'),
                ("sqcup", '\u{2294}'),
                ("sqcap", '\u{2293}'),
                ("smallint", '\u{222B}'),
                ("oiint", '\u{222F}'),
                ("oiiint", '\u{2230}'),
                ("intop", '\u{222B}'),
                ("ulcorner", '\u{231C}'),
                ("urcorner", '\u{231D}'),
                ("llcorner", '\u{231E}'),
                ("lrcorner", '\u{231F}'),
                ("llbracket", '\u{27E6}'),
                ("rrbracket", '\u{27E7}'),
                ("llparenthesis", '\u{2985}'),
                ("rrparenthesis", '\u{2986}'),
                ("nearrow", '\u{2197}'),
                ("searrow", '\u{2198}'),
                ("swarrow", '\u{2199}'),
                ("nwarrow", '\u{2196}'),
                ("leftharpoonup", '\u{21BC}'),
                ("leftharpoondown", '\u{21BD}'),
                ("rightharpoonup", '\u{21C0}'),
                ("rightharpoondown", '\u{21C1}'),
                ("rightleftharpoons", '\u{21CC}'),
                ("leftrightharpoons", '\u{21CB}'),
                ("rightrightarrows", '\u{21C9}'),
                ("leftleftarrows", '\u{21C7}'),
                ("rightleftarrows", '\u{21C4}'),
                ("leftrightarrows", '\u{21C6}'),
                ("upuparrows", '\u{21C8}'),
                ("downdownarrows", '\u{21CA}'),
                ("upharpoonleft", '\u{21BF}'),
                ("upharpoonright", '\u{21BE}'),
                ("downharpoonleft", '\u{21C3}'),
                ("downharpoonright", '\u{21C2}'),
                ("curvearrowleft", '\u{21B6}'),
                ("curvearrowright", '\u{21B7}'),
                ("circlearrowleft", '\u{21BA}'),
                ("circlearrowright", '\u{21BB}'),
                ("looparrowleft", '\u{21AB}'),
                ("looparrowright", '\u{21AC}'),
                ("multimap", '\u{22B8}'),
                ("rightsquigarrow", '\u{21DD}'),
                ("leadsto", '\u{21DD}'),
                ("dashrightarrow", '\u{21E2}'),
                ("dashleftarrow", '\u{21E0}'),
                ("twoheadrightarrow", '\u{21A0}'),
                ("twoheadleftarrow", '\u{219E}'),
                ("Longleftarrow", '\u{27F8}'),
                ("Longrightarrow", '\u{27F9}'),
                ("Longleftrightarrow", '\u{27FA}'),
                ("Uparrow", '\u{21D1}'),
                ("Downarrow", '\u{21D3}'),
                ("Updownarrow", '\u{21D5}'),
                ("nsubseteq", '\u{2288}'),
                ("nsupseteq", '\u{2289}'),
                ("subsetneq", '\u{228A}'),
                ("supsetneq", '\u{228B}'),
                ("varsubsetneq", '\u{228A}'),
                ("varsupsetneq", '\u{228B}'),
                ("sqsubset", '\u{228F}'),
                ("sqsupset", '\u{2290}'),
                ("sqsubseteq", '\u{2291}'),
                ("sqsupseteq", '\u{2292}'),
                ("Subset", '\u{22D0}'),
                ("Supset", '\u{22D1}'),
                ("owns", '\u{220B}'),
                ("complement", '\u{2201}'),
                ("smallsetminus", '\u{2216}'),
                ("nleq", '\u{2270}'),
                ("ngeq", '\u{2271}'),
                ("nless", '\u{226E}'),
                ("ngtr", '\u{226F}'),
                ("nsim", '\u{2241}'),
                ("ncong", '\u{2247}'),
                ("nparallel", '\u{2226}'),
                ("nmid", '\u{2224}'),
                ("nprec", '\u{2280}'),
                ("nsucc", '\u{2281}'),
                ("triangleq", '\u{225C}'),
                ("eqsim", '\u{2242}'),
                ("approxeq", '\u{224A}'),
                ("asymp", '\u{224D}'),
                ("smile", '\u{2323}'),
                ("frown", '\u{2322}'),
                ("models", '\u{22A8}'),
                ("vDash", '\u{22A8}'),
                ("Vdash", '\u{22A9}'),
                ("VDash", '\u{22AB}'),
                ("nvdash", '\u{22AC}'),
                ("nVdash", '\u{22AE}'),
                ("perp", '\u{22A5}'),
                ("parallel", '\u{2225}'),
                ("mid", '\u{2223}'),
                ("shortmid", '\u{2223}'),
                ("shortparallel", '\u{2225}'),
                ("nexists", '\u{2204}'),
                ("lnot", '\u{00AC}'),
                ("wedge", '\u{2227}'),
                ("vee", '\u{2228}'),
                ("impliedby", '\u{27F8}'),
                ("iff", '\u{27FA}'),
                ("blacklozenge", '\u{25C6}'),
                ("diamond", '\u{22C4}'),
                ("lozenge", '\u{25CA}'),
                ("boxplus", '\u{229E}'),
                ("boxminus", '\u{229F}'),
                ("boxtimes", '\u{22A0}'),
                ("boxdot", '\u{22A1}'),
                ("dotplus", '\u{2214}'),
                ("divideontimes", '\u{22C7}'),
                ("ltimes", '\u{22C9}'),
                ("rtimes", '\u{22CA}'),
                ("leftthreetimes", '\u{22CB}'),
                ("rightthreetimes", '\u{22CC}'),
                ("barwedge", '\u{22BC}'),
                ("veebar", '\u{22BB}'),
                ("doublebarwedge", '\u{2A5E}'),
                ("curlywedge", '\u{22CF}'),
                ("curlyvee", '\u{22CE}'),
                ("Cup", '\u{22D3}'),
                ("Cap", '\u{22D2}'),
                ("intercal", '\u{22BA}'),
                ("triangle", '\u{25B3}'),
                ("blacktriangle", '\u{25B2}'),
                ("blacktriangledown", '\u{25BC}'),
                ("blacktriangleleft", '\u{25C0}'),
                ("blacktriangleright", '\u{25B6}'),
                ("blacksquare", '\u{25A0}'),
                ("square", '\u{25A1}'),
                ("Box", '\u{25A1}'),
                ("Diamond", '\u{25C7}'),
            ] { m.insert(n, c); }
            m
        })
    }
    table().get(name).copied()
}

/// 多个子表达式横向排列：共享基线，累加宽度。
fn layout_row(font: &MathFont, items: &[Expr], style: Style) -> Box {
    let mut out = Box::empty();
    let mut prev_class = MClass::Ord;
    // merge 的宽度语义是 max（用于纵向堆叠/重叠），行的宽度必须**累加**——
    // 每项平移到当前累计 x，然后 out.width += 该项宽度。
    // TeX 规则：行首的二元运算符当一元用（`-x` 的减号不是 Bin 而是 Ord），
    // 行尾的 Bin 同理当作 Ord。不处理的话 `-x` 会多出二元运算符两侧的间距。
    let classes: Vec<MClass> = items.iter().map(math_class).collect();
    let n_items = classes.len();
    for (idx, item) in items.iter().enumerate() {
        let child = layout_box(font, item, style);
        let mut cls = classes[idx];
        if cls == MClass::Bin && (idx == 0 || idx + 1 == n_items) {
            cls = MClass::Ord;
        }
        let mut prev = prev_class;
        if prev == MClass::Bin && idx == 1 {
            prev = MClass::Ord;
        }
        // TeX 数学间距：相邻两项类别查表插空（mu 间距，1mu=1/18em）
        if out.glyphs.len() > 0 || out.width > 0.0 {
            let gap = math_spacing(prev, cls);
            if gap > 0.0 { out.width += gap; }
        }
        prev_class = cls;
        let shifted = translate(&child, out.width, 0.0);
        out.width += child.width;
        out.height = out.height.max(child.height);
        out.depth = out.depth.max(child.depth);
        out.glyphs.extend_from_slice(&shifted.glyphs);
        out.rules.extend_from_slice(&shifted.rules);
    }
    out
}

/// TeX 数学符号类别（用于间距规则）。
#[derive(Clone, Copy, PartialEq)]
enum MClass { Ord, Op, Bin, Rel, Open, Close, Punct, Inner }

/// Plain TeX math spacing 表（单位 mu，1mu = 1/18 em）：行[prev]", "#, "#", "列[cur]"。
/// 只取“需加空”的组合（0 = 无额外空）；thin≈3mu、med≈4mu、thick≈5mu。
fn math_class(e: &Expr) -> MClass {
    use MClass::*;
    match e {
        Expr::Atom { ch, .. } => match ch {
            '+' | '-' | '±' | '∓' | '×' | '⋅' | '*' | '/' | '∘' | '⊕' | '⊗' => Bin,
            '=' | '<' | '>' | '≤' | '≥' | '≠' | '≈' | '≅' | '≡' | '∼' | '∝'
            | '⊂' | '⊃' | '⊆' | '⊇' | '∈' | '∋' | '→' | '←' | '↔' | '⇒' | '⟹' => Rel,
            '(' | '[' | '{' | '⟨' => Open,
            ')' | ']' | '}' | '⟩' => Close,
            ',' | ';' | '!' => Punct,
            _ => Ord,
        },
        Expr::Symbol(s) => match s.as_str() {
            "leq" | "geq" | "neq" | "approx" | "equiv" | "sim" | "propto" | "subset" | "supset"
            | "subseteq" | "supseteq" | "in" | "ni" | "to" | "rightarrow" | "leftarrow"
            | "leftrightarrow" | "Rightarrow" | "Leftarrow"
            | "le" | "ge" | "ne" | "leqq" | "geqq" | "lt" | "gt" | "iff" | "implies" => Rel,
            "times" | "div" | "pm" | "cdot" | "ast" | "circ" | "oplus" | "otimes" => Bin,
            _ => Ord,
        },
        Expr::Bigop { .. } => Op,
        Expr::LeftRight { .. } => Inner,
        Expr::Frac { .. } | Expr::Sqrt { .. } | Expr::Binom { .. } | Expr::Row(_) | Expr::Text(_) => Ord,
        _ => Ord,
    }
}

fn math_spacing(prev: MClass, cur: MClass) -> f32 {
    use MClass::*;
    let mu = |n: f32| n / 18.0 * 1000.0;   // 1mu = 1/18 em（font units，upem=1000 时 1mu≈55.6）
    match (prev, cur) {
        (Ord, Op) | (Op, Ord) | (Ord, Bin) | (Bin, Ord) | (Op, Op) => mu(4.0),      // medspace
        (Ord, Rel) | (Rel, Ord) | (Op, Rel) | (Rel, Op) | (Bin, Rel) | (Rel, Bin)
        | (Rel, Rel) | (Open, Rel) | (Rel, Open) | (Rel, Close) | (Rel, Punct) => mu(5.0), // thickspace
        (Close, Bin) | (Close, Op) | (Punct, Bin) | (Punct, Op) | (Inner, Bin) | (Inner, Op)
        | (Open, Bin) | (Open, Op) => mu(4.0),
        (Close, Rel) | (Punct, Rel) | (Inner, Rel) => mu(5.0),
        (Bin, Close) => mu(0.0),
        _ => 0.0,
    }
}

/// 单字符横向排（text run／大运算符单词）：宽度累加、基线共享。
fn layout_text_run(font: &MathFont, chars: &[char], scale: f32) -> Box {
    let mut out = Box::empty();
    for &ch in chars {
        let cb = layout_char(font, ch, scale, 0.0);
        let cbm = translate(&cb, out.width, 0.0);
        out.width += cb.width;
        out.height = out.height.max(cb.height);
        out.depth = out.depth.max(cb.depth);
        out.glyphs.extend_from_slice(&cbm.glyphs);
        out.rules.extend_from_slice(&cbm.rules);
    }
    out
}

/// 上下标：base 保持，sub 下移、sup 上移，脚本用缩小字号。
/// TeX 规则：sub/sup 都锚在 base 右上（同 x），纵向堆叠；间距用 MATH 常量
/// （superscript_shift_up / subscript_shift_down / sub_superscript_gap_min），无常量时回退 0.7·0.5 缩放的经验值。
fn layout_scripts(
    font: &MathFont,
    base: &Expr,
    sub: Option<&Expr>,
    sup: Option<&Expr>,
    style: Style,
) -> Box {
    let base_box = layout_box(font, base, style);
    let mut out = base_box.clone();
    let ups = font.units_per_em();
    let scale = style.scale_of(font);

    let math = font.face_ref().and_then(|f| f.tables().math);
    let constants = math.and_then(|m| m.constants);
    let sup_shift = constants.map(|c| c.superscript_shift_up().value as f32).unwrap_or(ups * 0.5 * scale);
    let sub_shift = constants.map(|c| c.subscript_shift_down().value as f32).unwrap_or(ups * 0.22 * scale);
    let gap_min = constants.map(|c| c.sub_superscript_gap_min().value as f32).unwrap_or(ups * 0.1 * scale);

    // 先布局 sub/sup，算好各自尺寸
    let sub_box = sub.map(|s| layout_box(font, s, Style::Script));
    let sup_box = sup.map(|s| layout_box(font, s, Style::Script));

    // 垂直锚定（TeX）：
    //   sup 基线 = base 基线 + sup_shift（并 ≥ sup 底部低于 base 顶）
    //   sub 基线 = base 基线 - sub_shift（并 ≥ sub 顶部低于 base 底）
    let mut sup_dy = sup_shift;
    let mut sub_dy = -sub_shift;

    // sub/sup 同现：sup 底 与 sub 顶间距 ≥ gap_min（若 sub 靠下则再抬 sup）
    if let (Some(sb), Some(pb)) = (&sub_box, &sup_box) {
        let sup_bottom_rel = sup_dy - pb.depth;         // sup 底相对 base 基线
        let sub_top_rel = sub_dy + sb.height;            // sub 顶相对 base 基线
        let need = gap_min - (sup_bottom_rel - sub_top_rel);
        if need > 0.0 { sup_dy += need; }
    }

    // sub/sup 同 x：都在 base 右侧
    let script_x = base_box.width;

    if let Some(sb) = &sub_box {
        // 视觉上 sub 应低于 base 深度（若 shift 不够深则加深）
        sub_dy = sub_dy.min(-(base_box.depth * 0.5 + ups * 0.02 * scale));
        merge_at(&mut out, sb, script_x, sub_dy);
    }
    if let Some(pb) = &sup_box {
        merge_at(&mut out, pb, script_x, sup_dy);
    }

    // 宽度：base + max(sub, sup) 宽度（TeX：脚本并列，取最宽）
    let sw = sub_box.as_ref().map(|b| b.width).unwrap_or(0.0)
        .max(sup_box.as_ref().map(|b| b.width).unwrap_or(0.0));
    out.width = base_box.width + sw;
    out
}

/// 分数线堆叠（frac 与 binom 共用）：分子在上、分母在下，中间隔 [rule]（frac）或直接堆（binom）。
fn layout_stacked(
    font: &MathFont,
    num: &Expr,
    den: &Expr,
    style: Style,
    has_rule: bool,
) -> Box {
    let scale = style.scale_of(font);
    let ups = font.units_per_em();
    let num_box = layout_box(font, num, Style::Script);
    let den_box = layout_box(font, den, Style::Script);

    // 分数线 y = -axis（在轴线下方一点；TeX：rule 中心在轴线上）
    let rule_y = -font.axis_height * scale;
    // 无分数线（binom）时上下要拉开更多，否则 n 和 k 几乎贴在一起
    let gap = ups * (if has_rule { 0.12 } else { 0.26 }) * scale;
    // TeX 的 default_rule_thickness 是 0.4pt @10pt = 0.04em，原实现 0.06em 偏粗
    let thickness = ups * 0.04 * scale;

    // 分子底到分数线的距离、分母顶到分数线的距离都 ≥ gap（y 向上为正，rule_y 在基线下方）：
    //   分子底 = rule_y + gap → 分子基线 = 底 + depth
    //   分母顶 = rule_y - gap → 分母基线 = 顶 - height
    let num_dy = rule_y + gap + num_box.depth;    // 分子基线（在分数线上方，y 较大）
    let den_dy = rule_y - gap - den_box.height;   // 分母基线（在分数线下方，y 较小为负）

    let mut out = Box::empty();
    let w = num_box.width.max(den_box.width);
    let off = |b: &Box| (w - b.width) / 2.0;
    out.width = w;
    merge_at(&mut out, &num_box, off(&num_box), num_dy);
    merge_at(&mut out, &den_box, off(&den_box), den_dy);
    if has_rule {
        out.rules.push(Rule { x: 0.0, y: rule_y, width: w, thickness });
    }
    out
}

fn layout_fraction(font: &MathFont, num: &Expr, den: &Expr, style: Style) -> Box {
    layout_stacked(font, num, den, style, true)
}

/// 根号：√ + body（body 留在基线，√ 包住 body，横线在 body 顶上方并连 √ 肩）。
/// 几何用真实墨迹（layout_char 已改）+ TeX MATH radical 参数（radicalRuleThickness / radicalVerticalGap）。
/// 约定（y 向上为正，font units）：
///   - body 不动，墨迹顶 = body_box.height，墨迹底 = -body_box.depth
///   - 横线顶在 body 墨迹顶上方 cleargap = radicalVerticalGap + radicalRuleThickness
///   - √ 缩放使其墨迹从「横线顶」延伸到「body 墨迹底之下 overhang」
///   - body 水平起点 ≥ √ 墨迹右缘 + 水平 gap（防 √ 腿戳进 body）
fn layout_sqrt(font: &MathFont, degree: Option<&Expr>, body: &Expr, style: Style) -> Box {
    let scale = style.scale_of(font);
    let ups = font.units_per_em();
    let body_box = layout_box(font, body, style);

    let math = font.face_ref().and_then(|f| f.tables().math);
    let constants = math.and_then(|m| m.constants);
    let rule_t = (constants.map(|c| c.radical_rule_thickness().value as f32)
        .unwrap_or(ups * 0.04)).max(1.0) * scale;
    let vgap = constants.map(|c| c.radical_vertical_gap().value as f32)
        .unwrap_or(ups * 0.1) * scale;
    let cleargap = vgap + rule_t;                    // body 墨迹顶 → 横线底

    // √ 墨迹自然范围（scale 1）：顶 rad_asc（上）、底 -rad_desc（下），总高 rad_total。
    let (rad_asc, rad_desc) = font.ink_extents('√').unwrap_or((font.ascender, font.descender));
    let rad_total = (rad_asc + rad_desc).max(1.0);
    let (_, rad_xmax) = font.ink_x_extent('√');

    // 横线中心 y：横线底 = body 墨迹顶 + cleargap
    let body_h = body_box.height;
    let body_d = body_box.depth;
    let rule_center = body_h + cleargap + rule_t * 0.5;

    // √ 缩放：墨迹总高覆盖 [body 墨迹底 - overhang, 横线顶]
    let overhang = ups * 0.04 * scale;
    let rad_scale = ((body_h + body_d + cleargap + rule_t + overhang) / rad_total * scale)
        .clamp(scale, scale * 2.5);

    // √ 基线：顶墨迹对齐横线中心 → 底墨迹自然垂过 body 底
    let rad_y = rule_center - rad_asc * rad_scale;
    let rad_bot = rad_y - rad_desc * rad_scale;      // √ 墨迹底（≤ -body_d）

    // 横线：从 √ 肩（≈墨迹右缘 0.95 处）向右覆盖 body 宽 + 水平 gap
    let hgap = ups * 0.09 * scale;
    let shoulder_x = rad_xmax * rad_scale * 0.95;
    let body_x = rad_xmax * rad_scale + hgap;        // body 在 √ 墨迹右缘之后
    let rule_x = shoulder_x;
    let rule_w = body_x + body_box.width - rule_x;

    let mut out = Box::empty();
    out.width = body_x + body_box.width;
    out.height = (rule_center + rule_t * 0.5).max(rad_y + rad_asc * rad_scale);
    out.depth = (-rad_bot).max(body_d);
    out.glyphs.push(PlacedGlyph { ch: '√', x: 0.0, y: rad_y, scale: rad_scale });
    merge_at(&mut out, &body_box, body_x, 0.0);
    out.rules.push(Rule { x: rule_x, y: rule_center, width: rule_w, thickness: rule_t });
    if let Some(d) = degree {
        let db = layout_box(font, d, Style::Script);
        // 度数：抬高到 √ 顶墨迹上方（radicalDegreeBottomRaisePercent 为底抬升比例）
        let raise = constants.map(|c| c.radical_degree_bottom_raise_percent() as f32).unwrap_or(0.0);
        // 度数要贴在**横线**上方，而不是贴 √ 墨迹顶上：
        // √ 的墨迹顶比横线高得多（钩子会伸上去），按它算会让度数悬空一大截。
        let _ = raise;
        let degree_bot = rule_center + rule_t * 0.5 + vgap * 0.25;
        let deg_dy = degree_bot + db.depth;          // 度数基线
        // 水平上右端对齐横线起点，让度数坐在钩子与横线交界的上方。
        let deg_dx = (rule_x - db.width).max(0.0);
        merge_at(&mut out, &db, deg_dx, deg_dy);
        out.height = out.height.max(deg_dy + db.height);
    }
    out
}

/// 大运算符（∑/∫/∏）：glyph 更大 + 上下限放上下。
///
/// ⚠ 上下限的纵向锚点必须相对**运算符真实墨迹盒**（op_box.height/depth），不能用固定 0.25em：
/// ∑ 放大后墨迹高达 ~0.9em，固定 0.25em 会把上限塞进 ∑ 内部、下限抬到基线以上（重叠）。
/// 水平方向运算符与上下限三者**居中对齐**（TeX 规则），不是左对齐。
fn layout_bigop(font: &MathFont, name: &str, sub: Option<&Expr>, sup: Option<&Expr>, style: Style) -> Box {
    let scale = style.scale_of(font);
    // 单字符大运算符 → 对应 Unicode；多字符运算符名（lim/max 等）→ 单词排版
    let op: String = match name {
        "sum" => "∑".into(), "int" => "∫".into(), "prod" => "∏".into(), "oint" => "∮".into(),
        "bigcup" => "⋃".into(), "bigcap" => "⋂".into(), "bigoplus" => "⨁".into(), "bigotimes" => "⨂".into(),
        "bigvee" => "⋁".into(), "bigwedge" => "⋀".into(), "coprod" => "∐".into(),
        "iint" => "∬".into(), "iiint" => "∭".into(),
        "lim" => "lim".into(), "limsup" => "lim sup".into(), "liminf" => "lim inf".into(),
        "max" => "max".into(), "min" => "min".into(), "sup" => "sup".into(), "inf" => "inf".into(),
        "det" => "det".into(), "gcd" => "gcd".into(),
        _ => return layout_symbol(font, name, scale),
    };
    let single = op.chars().count() == 1;
    // 大运算符尺寸放大（∑∏ 略大；∫ 保持）
    let big_scale = if single && op != "∫" { scale * 1.2 } else { scale };
    // 单字符大运算符（∑∫∏…）是符号；多字符是运算符名（lim/max/det…）→ TeX 规定用**正体**，不能斜。
    let op_box = if single {
        layout_text_run(font, &op.chars().collect::<Vec<_>>(), big_scale)
    } else {
        layout_text_run_upright(font, &op.chars().collect::<Vec<_>>(), big_scale)
    };
    let ups = font.units_per_em();

    // 上限底 / 下限顶与运算符墨迹之间的最小间隙（MATH 常量优先，缺失回退 0.12em）。
    let constants = font.face_ref().and_then(|f| f.tables().math).and_then(|m| m.constants);
    let gap = constants
        .map(|c| c.upper_limit_gap_min().value.max(c.lower_limit_gap_min().value) as f32)
        .unwrap_or(ups * 0.12 * scale);

    let sub_box = sub.map(|s| layout_box(font, s, Style::Script));
    let sup_box = sup.map(|s| layout_box(font, s, Style::Script));

    // ∫ 系列：TeX 默认 nolimits —— 上下限放**右侧**（同普通上下标），不堆上下。
    // 否则积分会被撑成两三行高、行距爆炸；∑∏⋃… 才是 limits（上下）。
    let integral = matches!(name, "int" | "iint" | "iiint" | "oint");
    if integral {
        let sup_shift = constants.map(|c| c.superscript_shift_up().value as f32).unwrap_or(ups * 0.5 * scale);
        let sub_shift = constants.map(|c| c.subscript_shift_down().value as f32).unwrap_or(ups * 0.22 * scale);
        let script_x = op_box.width;
        let mut out = op_box.clone();
        if let Some(sb) = &sub_box { merge_at(&mut out, sb, script_x, -sub_shift); }
        if let Some(sb) = &sup_box { merge_at(&mut out, sb, script_x, sup_shift); }
        let sw = sub_box.as_ref().map(|b| b.width).unwrap_or(0.0)
            .max(sup_box.as_ref().map(|b| b.width).unwrap_or(0.0));
        out.width = op_box.width + sw;
        return out;
    }

    // 整体宽度 = 运算符与上下限中的最大者；三者水平居中对齐。
    let out_w = op_box.width
        .max(sub_box.as_ref().map(|b| b.width).unwrap_or(0.0))
        .max(sup_box.as_ref().map(|b| b.width).unwrap_or(0.0));

    let mut out = Box::empty();
    out.width = out_w;
    out.height = op_box.height;
    out.depth = op_box.depth;
    merge_at(&mut out, &op_box, (out_w - op_box.width) / 2.0, 0.0);
    if let Some(sb) = &sub_box {
        // 下限顶 = 运算符墨迹底 - gap → 下限基线 = -(op.depth + gap + sub.height)
        let dy = -(op_box.depth + gap + sb.height);
        merge_at(&mut out, sb, (out_w - sb.width) / 2.0, dy);
    }
    if let Some(sb) = &sup_box {
        // 上限底 = 运算符墨迹顶 + gap → 上限基线 = op.height + gap + sup.depth
        let dy = op_box.height + gap + sb.depth;
        merge_at(&mut out, sb, (out_w - sb.width) / 2.0, dy);
    }
    out
}

/// `\left( ... \right)`：body + 两侧分隔符。分隔符用真实墨迹缩放以覆盖 body，
/// 并**垂直居中于 body**（body 中心可能偏离基线，分隔符须随之中移，否则高深不对称时包不住）。
fn layout_left_right(font: &MathFont, open: &str, body: &Expr, close: &str, style: Style) -> Box {
    let scale = style.scale_of(font);
    let ups = font.units_per_em();
    let open_ch = delim_to_char(open);
    let close_ch = delim_to_char(close);
    let body_box = layout_box(font, body, style);
    // 分隔符用真实墨迹缩放并垂直居中，包住 body（公共逻辑见 wrap_delimiters）
    let gap = ups * 0.08 * scale;
    wrap_delimiters(font, &body_box, open_ch, close_ch, gap, scale, 1.06)
}

fn delim_to_char(d: &str) -> Option<char> {
    match d {
        "(" => Some('('), ")" => Some(')'),
        "[" => Some('['), "]" => Some(']'),
        "{" => Some('{'), "}" => Some('}'),
        "|" => Some('|'), "," => Some(','), "." => None,
        "langle" => Some('⟨'), "rangle" => Some('⟩'),
        "lfloor" => Some('⌊'), "rfloor" => Some('⌋'),
        "lceil" => Some('⌈'), "rceil" => Some('⌉'),
        "/" => Some('/'), "\\" => Some('\\'),
        "uparrow" => Some('↑'), "downarrow" => Some('↓'),
        "updownarrow" => Some('↕'),
        _ => None,
    }
}

/// 矩阵：行列对齐（每列最宽），垂直堆叠。env 决定外框（pmatrix→括号，matrix→无）。
fn layout_matrix(font: &MathFont, env: &str, rows: &[Vec<Expr>], style: Style) -> Box {
    let scale = style.scale_of(font);
    let ups = font.units_per_em();
    // 布局每个 cell
    let cells: Vec<Vec<Box>> = rows.iter()
        .map(|row| row.iter().map(|e| layout_box(font, e, Style::Text)).collect())
        .collect();
    let cols = cells.first().map(|r| r.len()).unwrap_or(0);
    // 每列宽度
    let col_w: Vec<f32> = (0..cols).map(|c| {
        cells.iter().map(|r| r.get(c).map(|b| b.width).unwrap_or(0.0)).fold(0.0, f32::max)
    }).collect();
    let col_gap = ups * 0.3 * scale;
    let row_gap = ups * 0.25 * scale;

    let mut out = Box::empty();
    let mut y = 0.0f32;   // 当前行基线的累计偏移
    for (ri, row) in cells.iter().enumerate() {
        let row_h = row.iter().map(|b| b.height).fold(0.0, f32::max);
        let row_d = row.iter().map(|b| b.depth).fold(0.0, f32::max);
        if ri > 0 { y += row_gap; }
        let mut x = 0.0f32;
        for (ci, cell) in row.iter().enumerate() {
            let cw = col_w.get(ci).copied().unwrap_or(cell.width);
            let cx = x + if cols > 1 { (cw - cell.width) / 2.0 } else { 0.0 };
            merge_at(&mut out, cell, cx, -y);
            x += cw + col_gap;
        }
        y += row_h + row_d;
    }
    out.width = col_w.iter().sum::<f32>() + col_gap * (cols.saturating_sub(1)) as f32;
    out.height = out.height.max(0.0);
    out.depth = out.depth.max(0.0);

    // 外框（pmatrix/bmatrix/vmatrix 的括号、cases 的左大括号）：内容整体右移括号宽，括号放两端。
    let paren: Option<(char, Option<char>)> = match env {
        "pmatrix" => Some(('(', Some(')'))),
        "bmatrix" => Some(('[', Some(']'))),
        "vmatrix" => Some(('|', Some('|'))),
        "cases" => Some(('{', None)),   // 分段函数：只有左大括号
        _ => None,
    };
    if let Some((o, c)) = paren {
        let gap = ups * 0.06 * scale;
        out = wrap_delimiters(font, &out, Some(o), c, gap, scale, 1.06);
    }
    out
}

// ---------- 工具 ----------

/// 平移：所有 glyph/rule 加 dx/dy。
fn translate(b: &Box, dx: f32, dy: f32) -> Box {
    Box {
        width: b.width,
        height: b.height,
        depth: b.depth,
        glyphs: b.glyphs.iter().map(|g| PlacedGlyph { x: g.x + dx, y: g.y + dy, ..*g }).collect(),
        rules: b.rules.iter().map(|r| Rule { x: r.x + dx, y: r.y + dy, ..*r }).collect(),
    }
}

/// 平移后合并：把 src（平移到 dx/dy）并入 dst——尺寸按偏移折算。
/// ⚠ 必须用这个而不是 `merge(&mut out, &translate(...))`：merge 的 height/depth 不通晓平移偏移，
/// translate 后的盒 height/depth 是"自己坐标系"的，直接 max 会尺寸错乱（分数/根号/括号全受影响）。
/// 分隔符（括号、方括号、大括号）包住内容时的拉伸参数。
struct DelimFit {
    scale: f32,
    dy: f32,
}

/// 算出「让分隔符墨迹恰好包住 height/depth 的内容」所需缩放与垂直偏移。
///
/// 三个调用点（`\binom`、`\left…\right`、矩阵环境）原本各自抄了一遍同样的公式，
/// 改一处忘另一处就会一个包得住、一个包不住。抽到这里，`factor` 只留各处的视觉余量差异。
fn fit_delimiter(
    font: &MathFont,
    ch: char,
    height: f32,
    depth: f32,
    min_scale: f32,
    factor: f32,
) -> DelimFit {
    let (pa, pd) = font.ink_extents(ch).unwrap_or((font.ascender, font.descender));
    let (pa, pd) = (pa.max(1.0), pd.max(1.0));
    // 按**总墨迹高**比，而不是 max(h/pa, d/pd)：非对称内容（分数整体挂在基线下方）
    // 用后者会把括号撑爆或错位。
    let scale = ((height + depth) / (pa + pd) * factor).clamp(min_scale, min_scale * 4.0);
    // 垂直居中：分隔符中心与内容中心对齐
    let dy = (height - depth) / 2.0 - (pa - pd) / 2.0 * scale;
    DelimFit { scale, dy }
}

/// 用一对分隔符把 body 包起来（`close` 为 `None` 时只有左侧，例如 `cases`）。
///
/// 宽度、高度、深度都按实际拼出来的结果重算，调用方不用自己维护。
fn wrap_delimiters(
    font: &MathFont,
    body: &Box,
    open: Option<char>,
    close: Option<char>,
    gap: f32,
    min_scale: f32,
    factor: f32,
) -> Box {
    let reference = open.or(close);
    let fit = reference.map(|ch| {
        fit_delimiter(font, ch, body.height, body.depth, min_scale, factor)
    });
    let (ds, dy) = fit.map(|f| (f.scale, f.dy)).unwrap_or((min_scale, 0.0));

    let open_b = open.map(|ch| layout_char(font, ch, ds, 0.0));
    let close_b = close.map(|ch| layout_char(font, ch, ds, 0.0));

    let mut out = Box::empty();
    let mut x = 0.0;
    if let Some(ob) = &open_b {
        merge_at(&mut out, ob, 0.0, dy);
        x += ob.width + gap;
    }
    merge_at(&mut out, body, x, 0.0);
    x += body.width;
    if let Some(cb) = &close_b {
        x += gap;
        merge_at(&mut out, cb, x, dy);
        x += cb.width;
    }
    out.width = x;
    out.height = out.height.max(body.height);
    out.depth = out.depth.max(body.depth);
    out
}

fn merge_at(dst: &mut Box, src: &Box, dx: f32, dy: f32) {
    // 平移后该盒相对 dst 基线的垂向范围：顶 = src.height + dy，底 = src.depth - dy
    dst.height = dst.height.max(src.height + dy);
    dst.depth = dst.depth.max(src.depth - dy);
    dst.width = dst.width.max(src.width + dx);
    for g in &src.glyphs {
        dst.glyphs.push(crate::layout::PlacedGlyph { x: g.x + dx, y: g.y + dy, ..*g });
    }
    for r in &src.rules {
        dst.rules.push(crate::layout::Rule { x: r.x + dx, y: r.y + dy, ..*r });
    }
}

// ---------- 测试（用真实 LM Math 字体做端到端布局断言）----------

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::ast::parse;

    const LM_PATH: &str = crate::LM_MATH_OTF;

    /// 按 ASCII 字符查 glyph（自动应用数学斜体映射，与布局输出一致）
    fn findGlyph(l: &crate::layout::Layout, ascii: char) -> Option<&crate::layout::PlacedGlyph> {
        let m = crate::font::to_math_italic(ascii);
        l.glyphs.iter().find(|g| g.ch == m)
    }

    fn font() -> MathFont<'static> {
        let data = crate::font::load_latin_modern_math(LM_PATH).expect("字体存在");
        let bytes: &'static [u8] = std::boxed::Box::leak(data.into_boxed_slice());
        MathFont::from_bytes(bytes).expect("解析")
    }

    #[test]
    fn 单个字符布局() {
        let f = font();
        let ast = parse("x").unwrap();
        let l = layout(&f, &ast);
        assert_eq!(l.glyphs.len(), 1);
        assert_eq!(l.glyphs[0].ch, crate::font::to_math_italic('x'));
        assert!(l.width > 0.0);
        assert!(l.height > 0.0);
    }

    #[test]
    fn 加法行累加宽度() {
        let f = font();
        let ast = parse("a+b").unwrap();
        let l = layout(&f, &ast);
        assert_eq!(l.glyphs.len(), 3);
        // 三个字符宽度之和 ≥ 总宽（间距可能让总宽略大）
        let wa = f.advance('a');
        let wp = f.advance('+');
        let wb = f.advance('b');
        assert!(l.width >= wa + wp + wb, "width={} vs {} ", l.width, wa + wp + wb);
        // 所有 glyph 在基线上（y≈0）
        assert!(l.glyphs.iter().all(|g| (g.y - 0.0).abs() < 1.0));
    }

    #[test]
    fn 上下标抬高下沉() {
        let f = font();
        let ast = parse("x^2").unwrap();
        let l = layout(&f, &ast);
        assert_eq!(l.glyphs.len(), 2);
        let x = l.glyphs.iter().find(|g| g.ch == crate::font::to_math_italic('x')).unwrap();
        let sup = l.glyphs.iter().find(|g| g.ch == '2').unwrap();
        assert!(sup.y > x.y + 10.0, "上标应高于基线: sup.y={} x.y={}", sup.y, x.y);
        assert!(sup.scale < 0.85, "上标应缩小: {}", sup.scale);
    }

    #[test]
    fn 分数有规则线() {
        let f = font();
        let ast = parse(r"\frac{a}{b}").unwrap();
        let l = layout(&f, &ast);
        assert_eq!(l.rules.len(), 1, "frac 应有分数线");
        assert!(l.rules[0].width > 0.0);
        // 分子在上、分母在下
        let a = l.glyphs.iter().find(|g| g.ch == crate::font::to_math_italic('a')).unwrap();
        let b = l.glyphs.iter().find(|g| g.ch == crate::font::to_math_italic('b')).unwrap();
        assert!(a.y > b.y, "分子 a 应在分母 b 上方: a.y={} b.y={}", a.y, b.y);
    }

    #[test]
    fn 根号包含radical与横线() {
        let f = font();
        let ast = parse(r"\sqrt{x}").unwrap();
        let l = layout(&f, &ast);
        assert!(l.glyphs.iter().any(|g| g.ch == '√'));
        assert!(l.rules.iter().any(|r| r.width > 0.0), "根号应有覆盖横线");
    }

    #[test]
    fn 求和带上下限() {
        let f = font();
        let ast = parse(r"\sum_{i=1}^{n}").unwrap();
        let l = layout(&f, &ast);
        assert!(l.glyphs.iter().any(|g| g.ch == '∑'));
        // 有上下限字符
        assert!(l.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('i')));
        assert!(l.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('n')));
    }

    #[test]
    fn 大运算符上下限不重叠且居中() {
        // 回归：∑ 上下限曾用固定 0.25em 锚点，上限塞进 ∑ 内部、下限抬到基线上 → 重叠。
        // 现在锚定在运算符真实墨迹盒外侧，必须与 ∑ 墨迹不相交，且水平居中。
        let f = font();
        let ast = parse(r"\sum_{i=1}^{n}").unwrap();
        let l = layout(&f, &ast);
        let ink = |g: &crate::layout::PlacedGlyph| -> (f32, f32, f32, f32) {
            let (asc, desc) = f.ink_extents(g.ch).unwrap_or((f.ascender, f.descender));
            let top = g.y + asc * g.scale;
            let bottom = g.y - desc * g.scale;
            (top, bottom, g.x, g.x + f.advance(g.ch) * g.scale)
        };
        let sum = l.glyphs.iter().find(|g| g.ch == '∑').unwrap();
        let (st, sb, sl, sr) = ink(sum);
        let sup = l.glyphs.iter().find(|g| g.ch == crate::font::to_math_italic('n')).unwrap();
        let sub_glyphs: Vec<_> = l.glyphs.iter()
            .filter(|g| g.y < -100.0)
            .collect();
        assert!(!sub_glyphs.is_empty(), "应有下标 glyph");
        let (upt, _upb, _upl, _upr) = ink(sup);
        // 下标整体（i=1）的墨迹底与水平范围
        let lob = sub_glyphs.iter().map(|g| ink(g).1).fold(f32::INFINITY, f32::min);
        let sub_l = sub_glyphs.iter().map(|g| ink(g).2).fold(f32::INFINITY, f32::min);
        let sub_r = sub_glyphs.iter().map(|g| ink(g).3).fold(f32::NEG_INFINITY, f32::max);
        assert!(upt > st, "上限应完全在 ∑ 墨迹之上: sup_top={} sum_top={}", upt, st);
        assert!(lob < sb, "下限应完全在 ∑ 墨迹之下: sub_bottom={} sum_bottom={}", lob, sb);
        // 水平居中：上限/下限的盒中心与 ∑ 盒中心接近
        let sum_c = (sl + sr) / 2.0;
        let (_, _, ul, ur) = ink(sup);
        assert!(((ul + ur) / 2.0 - sum_c).abs() < 60.0, "上限应水平居中");
        assert!(((sub_l + sub_r) / 2.0 - sum_c).abs() < 60.0, "下限应水平居中: {sub_l} {sub_r} {sum_c}");
    }

    #[test]
    fn 运算符名用正体() {
        // \lim/\max/\det 等多字符运算符名必须正体；∑∫∏ 是符号不受影响。
        let f = font();
        let l = layout(&f, &parse(r"\lim_{x \to 0}").unwrap());
        assert!(l.glyphs.iter().any(|g| g.ch == 'l'), "lim 的 l 应是正体 ASCII");
        assert!(l.glyphs.iter().any(|g| g.ch == 'm'), "lim 的 m 应是正体 ASCII");
        assert!(!l.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('l')), "不应出现数学斜体 l");
        // 变量 x 仍斜体
        assert!(l.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('x')));
    }

    #[test]
    fn 矩阵括号包住内容且垂直居中() {
        let f = font();
        let l = layout(&f, &parse(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}").unwrap());
        let open = l.glyphs.iter().find(|g| g.ch == '(').expect("应有左括号");
        let close = l.glyphs.iter().find(|g| g.ch == ')').expect("应有右括号");
        let ink = |g: &crate::layout::PlacedGlyph| -> (f32, f32) {
            let (asc, desc) = f.ink_extents(g.ch).unwrap_or((f.ascender, f.descender));
            (g.y + asc * g.scale, g.y - desc * g.scale)
        };
        let (ot, ob) = ink(open);
        let (ct, cb) = ink(close);
        // 内容（a/b/c/d）的垂直范围
        let body: Vec<_> = l.glyphs.iter().filter(|g| matches!(g.ch, 'a' | 'b' | 'c' | 'd')).collect();
        let btop = body.iter().map(|g| ink(g).0).fold(f32::NEG_INFINITY, f32::max);
        let bbot = body.iter().map(|g| ink(g).1).fold(f32::INFINITY, f32::min);
        assert!(ot >= btop - 20.0, "左括号顶应覆盖内容顶: {ot} vs {btop}");
        assert!(ob <= bbot + 20.0, "左括号底应覆盖内容底: {ob} vs {bbot}");
        assert!(ct >= btop - 20.0 && cb <= bbot + 20.0, "右括号应覆盖内容");
        // 左右括号垂直位置一致
        assert!((ot - ct).abs() < 30.0, "左右括号应同高: {ot} vs {ct}");
    }

    #[test]
    fn 积分上下限放右侧() {
        // TeX：\int 默认 nolimits，上下限在 ∫ 右侧（不堆上下），保证行高不爆炸。
        let f = font();
        let l = layout(&f, &parse(r"\int_0^1 x").unwrap());
        let intg = l.glyphs.iter().find(|g| g.ch == '∫').expect("应有 ∫");
        let zero = l.glyphs.iter().find(|g| g.ch == '0').expect("应有下限 0");
        let one = l.glyphs.iter().find(|g| g.ch == '1').expect("应有上限 1");
        assert!(zero.x >= intg.x + f.advance('∫') * 0.8, "下限应在 ∫ 右侧: {} vs {}", zero.x, intg.x);
        assert!(one.x >= intg.x + f.advance('∫') * 0.8, "上限应在 ∫ 右侧: {} vs {}", one.x, intg.x);
        assert!(one.y > zero.y, "上限 1 应高于下限 0");
    }

    #[test]
    fn 上下标并排同x不串行() {
        // TeX：sub/sup 都锚在 base 右上同 x（纵向堆叠），宽度取 max 而非两者相加
        let f = font();
        let ast = parse("a_i^2").unwrap();
        let l = layout(&f, &ast);
        assert_eq!(l.glyphs.len(), 3, "a + i + 2");
        let i = l.glyphs.iter().find(|g| g.ch == crate::font::to_math_italic('i')).unwrap();
        let two = l.glyphs.iter().find(|g| g.ch == '2').unwrap();
        // sub(i) 与 sup(2) 的 x 都应大致在 a 的右侧同一处（不串行）
        assert!((i.x - two.x).abs() < 50.0, "sub/sup 应并排: i.x={} two.x={}", i.x, two.x);
        // i 在基线下方、2 在基线上方
        assert!(i.y < -10.0, "下标 i 应低于基线: {}", i.y);
        assert!(two.y > 10.0, "上标 2 应高于基线: {}", two.y);
    }

    #[test]
    fn 括号高度随body传播() {
        // \left( \frac{x}{y} \right)：括号应比单体大（覆盖分数高度）
        let f = font();
        let plain = layout(&f, &parse(r"\left( x \right)").unwrap());
        let frac = layout(&f, &parse(r"\left( \frac{x}{y} \right)").unwrap());
        assert!(frac.height + frac.depth > plain.height + plain.depth,
                "分数 body 的括号总高应更大: {}+{} vs {}+{}", frac.height, frac.depth, plain.height, plain.depth);
        // 分母存在
        assert!(frac.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('y')));
    }


    #[test]
    fn 根号radical覆盖body() {
        // \sqrt{\frac{a}{b}} 的 radical 应比 \sqrt{x} 大（覆盖更高 body）
        let f = font();
        let small = layout(&f, &parse(r"\sqrt{x}").unwrap());
        let big = layout(&f, &parse(r"\sqrt{\frac{a}{b}}").unwrap());
        // √ 墨迹底必须包住 body 墨迹底：rad_bot ≤ -body_d（√ 腿垂过 body 底）。
        // √ 墨迹顶到 40、底到 -960（font units, scale 1），墨迹底 = y - 960*scale。
        let rad = big.glyphs.iter().find(|g| g.ch == '√').expect("√ 存在");
        let rad_bot = rad.y - 960.0 * rad.scale;      // √ 墨迹底（相对基线）
        let body_bot = -big.depth;                     // body 整体墨迹底
        assert!(rad_bot <= body_bot + 1.0,
                "√ 底应包住 body 底: √底={} body底={}", rad_bot, body_bot);
        // 对比小 body：大 body 的 √ 更大（更深的腿）
        let rad_s = small.glyphs.iter().find(|g| g.ch == '√').unwrap();
        assert!(rad.scale >= rad_s.scale, "大 body 的 √ 应更大: {} vs {}", rad.scale, rad_s.scale);
        // 横线应在 body 顶上方（注意 frac 内部有自己的分数线在基线下方；根号横线是 y 最大的那条）
        let rule = big.rules.iter().max_by(|a, b| a.y.partial_cmp(&b.y).unwrap()).expect("根号应有横线");
        assert!(rule.y > 0.0, "根号横线应在基线以上: {}", rule.y);
        // 横线在 body 墨迹顶上方（body 顶 = body 最高 glyph 的墨迹顶）
        let body_top = big.glyphs.iter()
            .filter(|g| g.ch != '√')
            .map(|g| g.y + f.ink_extents(g.ch).map(|(a, _)| a).unwrap_or(0.0) * g.scale)
            .fold(f32::MIN, f32::max);
        assert!(rule.y > body_top, "根号横线应在 body 墨迹顶上方: rule={} body_top={}", rule.y, body_top);
    }

    #[test]
    fn 上标重音_hat() {
        let f = font();
        let l = layout(&f, &parse(r"\hat{x}").unwrap());
        // x 保持 + 帽子在 body 上方
        assert!(l.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('x')));
        let hat = l.glyphs.iter().find(|g| g.ch == '^').expect("hat 应有 ^");
        let x = l.glyphs.iter().find(|g| g.ch == crate::font::to_math_italic('x')).unwrap();
        assert!(hat.y > x.y, "帽子应在 body 上方: hat.y={} x.y={}", hat.y, x.y);
    }

    #[test]
    fn 上划线_overline() {
        let f = font();
        let l = layout(&f, &parse(r"\overline{ab}").unwrap());
        assert!(l.rules.len() >= 1, "overline 应有横线");
        assert!(l.rules[0].y > 0.0, "上划线应在基线上方: {}", l.rules[0].y);
    }

    #[test]
    fn cases环境有左大括号() {
        let f = font();
        let l = layout(&f, &parse(r"\begin{cases} x & x>0 \\ 0 & x\\leq 0 \end{cases}").unwrap());
        assert!(l.glyphs.iter().any(|g| g.ch == '{'), "cases 应有左大括号");
        // 有分段内容
        assert!(l.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('x')));
        assert!(l.glyphs.iter().any(|g| g.ch == '0'));
    }

    #[test]
    fn 中文文本满宽advance() {
        let f = font();
        let l = layout(&f, &parse(r"\text{面积}").unwrap());
        // 两个中文字，各 1em 宽 → 总宽 2em（2000 font units，LM Math upem=1000）
        assert!((l.width - 2000.0).abs() < 50.0, "中文应按 1em/字: {}", l.width);
        assert_eq!(l.glyphs.len(), 2);
    }

    #[test]
    fn 拉丁字符不受影响() {
        let f = font();
        let l = layout(&f, &parse("ab").unwrap());
        let wa = f.advance('a');
        let wb = f.advance('b');
        assert!((l.width - (wa + wb)).abs() < 1.0, "拉丁仍按字形 advance: {} vs {}+{}", l.width, wa, wb);
    }

    #[test]
    fn 大小括号用MATH常量() {
        // x^2 的 sup 位移应来自 MATH 表（LM Math superscriptShiftUp ≈ 0.45em 量级），
        // 而不是固定常数——断言 sup 高于基线 0.3em 以上
        let f = font();
        let l = layout(&f, &parse("x^2").unwrap());
        let two = l.glyphs.iter().find(|g| g.ch == '2').unwrap();
        assert!(two.y > 0.3 * f.units_per_em(), "sup 应抬到 0.3em 以上: {}", two.y);
    }

    #[test]
    fn 完整公式布局() {
        let f = font();
        let ast = parse(r"E = \frac{1}{2} m v^2").unwrap();
        let l = layout(&f, &ast);
        assert!(!l.glyphs.is_empty());
        assert!(l.width > 100.0, "宽度应较大: {}", l.width);
        assert!(l.height > 50.0, "有分数应较高: {}", l.height);
    }

    #[test]
    fn 矩阵行列() {
        let f = font();
        let ast = parse(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}").unwrap();
        let l = layout(&f, &ast);
        // 4 个元素 + 可能的外框括号
        assert!(l.glyphs.len() >= 4, "矩阵应有 4 元素: {}", l.glyphs.len());
    }

    #[test]
    fn 左括号分隔符() {
        let f = font();
        let ast = parse(r"\left( x \right)").unwrap();
        let l = layout(&f, &ast);
        assert_eq!(l.glyphs.len(), 3);
        assert_eq!(l.glyphs[0].ch, '(');
        assert_eq!(l.glyphs[2].ch, ')');
    }
}

/// 回归测试：都是 2026-11-13 这轮迭代实际修掉的缺陷，留在这里防止再犯。
#[cfg(test)]
mod regression {
    use super::*;
    use crate::ast::parse;

    const LM_PATH: &str = crate::LM_MATH_OTF;

    fn font() -> MathFont<'static> {
        let data = crate::font::load_latin_modern_math(LM_PATH).expect("字体存在");
        let bytes: &'static [u8] = std::boxed::Box::leak(data.into_boxed_slice());
        MathFont::from_bytes(bytes).expect("字体可解析")
    }

    #[test]
    fn 二项式必须带括号() {
        // 旧实现把括号留给「绘制层」，结果 \binom 只画了上下两个数
        let f = font();
        let l = layout(&f, &parse(r"\binom{n}{k}").unwrap());
        let chars: Vec<char> = l.glyphs.iter().map(|g| g.ch).collect();
        assert!(chars.contains(&'('), "缺少左括号: {chars:?}");
        assert!(chars.contains(&')'), "缺少右括号: {chars:?}");
    }

    #[test]
    fn 二项式括号包住内容() {
        let f = font();
        let l = layout(&f, &parse(r"\binom{2n}{n}").unwrap());
        let inner_h = l
            .glyphs
            .iter()
            .filter(|g| g.ch == 'n' || g.ch == '2')
            .map(|g| g.y)
            .fold(f32::MIN, f32::max);
        assert!(l.height > inner_h, "括号顶应高于内容顶: {} <= {}", l.height, inner_h);
    }

    #[test]
    fn 根指数不越出左边界() {
        // 旧实现 deg_dx 恒为较大负数，3 会被推到公式左侧外面
        let f = font();
        let l = layout(&f, &parse(r"\sqrt[3]{x}").unwrap());
        for g in &l.glyphs {
            assert!(g.x >= -1.0, "字形 `{}` 跑到左边界外: x={}", g.ch, g.x);
        }
    }

    #[test]
    fn 根指数贴在横线上方() {
        let f = font();
        let l = layout(&f, &parse(r"\sqrt[3]{x}").unwrap());
        let deg = l.glyphs.iter().find(|g| g.ch == '3').expect("应有根指数 3");
        let rule = l.rules.first().expect("应有根号横线");
        assert!(
            deg.y >= rule.y,
            "根指数应在横线上方: deg.y={} rule.y={}",
            deg.y,
            rule.y
        );
    }

    #[test]
    fn 重音落在字母正上方() {
        let f = font();
        for src in [r"\hat{a}", r"\bar{b}", r"\vec{v}"] {
            let l = layout(&f, &parse(src).unwrap());
            let body = l.glyphs.first().expect("应有底字");
            let acc = l.glyphs.last().expect("应有重音");
            // 重音的水平中心与底字中心相差不应超过半个底字宽
            let diff = (acc.x - body.x).abs();
            assert!(diff < f.advance(body.ch) * 0.5, "{src}: 重音偏移过大 {diff}");
        }
    }
}

/// 字体族与堆叠的回归测试（2026-11-13 按真实文档对齐 TeX）。
#[cfg(test)]
mod tex_align {
    use super::*;
    use crate::ast::parse;

    fn font() -> MathFont<'static> {
        let data = crate::font::load_latin_modern_math(crate::LM_MATH_OTF).expect("字体存在");
        let bytes: &'static [u8] = std::boxed::Box::leak(data.into_boxed_slice());
        MathFont::from_bytes(bytes).expect("字体可解析")
    }

    #[test]
    fn 字体族映射到数学字母区() {
        let f = font();
        let l = layout(&f, &parse(r"\mathbb{R}").unwrap());
        assert_eq!(l.glyphs[0].ch, 'ℝ', "\\mathbb{{R}} 应映射到 U+211D");
        let l = layout(&f, &parse(r"\mathbf{A}").unwrap());
        assert_eq!(l.glyphs[0].ch as u32, 0x1D400);
        let l = layout(&f, &parse(r"\mathcal{L}").unwrap());
        assert_eq!(l.glyphs[0].ch, 'ℒ');
        let l = layout(&f, &parse(r"\mathfrak{g}").unwrap());
        assert_eq!(l.glyphs[0].ch as u32, 0x1D524);
    }

    #[test]
    fn 字体族不缺字形() {
        let f = font();
        for src in [r"\mathbb{RNCZQ}", r"\mathcal{BCEFHILMR}", r"\mathfrak{CHIRZ}"] {
            let l = layout(&f, &parse(src).unwrap());
            for g in &l.glyphs {
                assert!(f.glyph_id(g.ch).is_some(), "{src}: {:?} 缺字形", g.ch);
            }
        }
    }

    #[test]
    fn overset在基线上方() {
        let f = font();
        let l = layout(&f, &parse(r"\overset{a}{=}").unwrap());
        let base = l.glyphs.iter().find(|g| g.ch == '=').expect("等号");
        let top = l.glyphs.iter().find(|g| g.ch != '=').expect("上标项");
        assert!(top.y > base.y, "overset 的 a 应在等号上方");
    }

    #[test]
    fn underset在基线下方() {
        let f = font();
        let l = layout(&f, &parse(r"\underset{x}{\lim}").unwrap());
        assert!(
            l.glyphs.iter().any(|g| g.y < 0.0),
            "underset 应有字形落在基线下方"
        );
    }

    #[test]
    fn substack多行不重叠() {
        let f = font();
        let l = layout(&f, &parse(r"\substack{a\\b}").unwrap());
        let mut ys: Vec<f32> = l.glyphs.iter().map(|g| g.y).collect();
        ys.sort_by(|a, b| b.partial_cmp(a).unwrap());
        assert_eq!(ys.len(), 2, "应有两行各一个字符");
        assert!(ys[0] > ys[1], "第一行应在第二行上方: {ys:?}");
        assert!(l.height > 0.0, "整体高度应为正");
    }
}
