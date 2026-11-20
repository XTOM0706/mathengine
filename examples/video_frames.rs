//! `cargo run --release --example video_frames -- 输出目录`：生成演示视频的逐帧 SVG。
//!
//! 每一帧都是一张 1920×1080 的完整 SVG。公式部分不是截图：调用 `mathengine::layout`
//! 算出布局，再向字体索取字形轮廓画成 path。中文与说明文字用 SVG `<text>` 交给
//! rsvg / Pango 排版。
//!
//! 六段结构（旁白后配，画面只留底部 y 900–1000 的字幕安全区）：
//!   一 网页方案的问题 → 二 原理 → 三 核心优势 → 四 局限 → 五 应用 → 六 结尾
//!
//! 全片逐帧离线生成，不做即时渲染；一帧最多一条公式、零套字幕。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;
use std::fmt::Write as _;
use std::fs;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

const FW: f32 = 1920.0;
const FH: f32 = 1080.0;
const FPS: f32 = 30.0;

// 配色：深底 + 青绿强调 + 琥珀色警示。全片只用这几档。
const C_TXT: &str = "#eef2f9";
const C_DIM: &str = "#9aa8bd";
const C_FAINT: &str = "#5f6d83";
const C_SUB: &str = "#8290a7";
const C_LINE: &str = "#1d2939";
const C_PANEL2: &str = "#0b111b";
const C_ACC: &str = "#2ee6c5";
const C_AMBER: &str = "#f7b955";
const C_RED: &str = "#ff6b6b";
const C_BLUE: &str = "#6aa9ff";

const SANS: &str = "Noto Sans CJK SC";
const MONO: &str = "JetBrains Maple Mono";

// 时间轴（帧，30 fps）。
const S_WEB: usize = 0; //    0 –  24 s  网页方案的麻烦
const S_HOW: usize = 720; //  24 –  76 s  原理四步
const S_WIN: usize = 2280; // 76 – 128 s  三块优势
const S_LIM: usize = 3840; // 128 – 158 s  局限
const S_USE: usize = 4740; // 158 – 172 s  应用
const S_END: usize = 5160; // 172 – 180 s  结尾
const TOTAL: usize = 5400; // 180.0 s

const HOW_SLOT: usize = 390; // 4 × 13 s
const WIN_A: usize = 540; // 体积 18 s
const WIN_B: usize = 540; // 用例 18 s
const WIN_C: usize = 480; // 手机 16 s
const LIM_A: usize = 540; // 矩阵 18 s
const LIM_B: usize = 360; // 不支持 12 s

const _: () = assert!(S_HOW - S_WEB == 720);
const _: () = assert!(S_WIN - S_HOW == 4 * HOW_SLOT);
const _: () = assert!(S_LIM - S_WIN == WIN_A + WIN_B + WIN_C);
const _: () = assert!(S_USE - S_LIM == LIM_A + LIM_B);
const _: () = assert!(S_END - S_USE == 420);
const _: () = assert!(TOTAL - S_END == 240);

const SCENES: [(usize, &str); 6] = [
    (S_WEB, "网页方案"),
    (S_HOW, "排版原理"),
    (S_WIN, "核心优势"),
    (S_LIM, "当前局限"),
    (S_USE, "应用价值"),
    (S_END, "结尾"),
];

/// 禁用词：不出现「实时」，也不暗示即时渲染。
const BANNED: [&str; 6] = ["实时", "即时", "秒开", "秒出", "瞬间完成", "零等待"];

// 字幕安全区：底部 y 900–1000 留给后期烧字幕，画面不得占用。
const SAFE_TOP: f32 = 900.0;
const SAFE_BOT: f32 = 1000.0;

// ---------------------------------------------------------------- 字体与测量

/// 名字表里的族名：项目关掉了 ttf-parser 的 `std` feature，`Name::to_string` 不可用，
/// 这里自己按 UTF-16BE 解。族名不会有代理对，忽略。
fn unicode_name(n: &ttf_parser::name::Name) -> Option<String> {
    if !n.is_unicode() || n.name.len() % 2 != 0 {
        return None;
    }
    let mut out = String::new();
    for c in n.name.chunks_exact(2) {
        let v = u16::from_be_bytes([c[0], c[1]]);
        out.push(char::from_u32(v as u32)?);
    }
    Some(out)
}

struct Metrics {
    face: Face<'static>,
    upem: f32,
}

impl Metrics {
    fn load(data: &'static [u8], family: &str) -> Metrics {
        for idx in 0..64 {
            let Ok(face) = Face::parse(data, idx) else { break };
            let hit = face.names().into_iter().any(|n| {
                (n.name_id == 1 || n.name_id == 16)
                    && unicode_name(&n).map(|s| s.contains(family)).unwrap_or(false)
            });
            if hit {
                return Metrics { upem: face.units_per_em() as f32, face };
            }
        }
        panic!("字体里找不到族名：{family}");
    }

    fn advance(&self, s: &str, size: f32, warn: &mut Vec<String>) -> f32 {
        let k = size / self.upem;
        let mut w = 0.0;
        for ch in s.chars() {
            match self.face.glyph_index(ch).and_then(|g| self.face.glyph_hor_advance(g)) {
                Some(a) => w += a as f32 * k,
                None => {
                    let msg = format!("字体缺字 U+{:04X} {:?}", ch as u32, ch);
                    if !warn.contains(&msg) {
                        warn.push(msg);
                    }
                    w += size * 0.55; // 估算，只影响居中
                }
            }
        }
        w
    }
}

// ---------------------------------------------------------------- 公式美术

/// 收集字形轮廓，同时记住轮廓包围盒（用来把符号摆进方块正中）。
struct PathBuilder {
    d: String,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl PathBuilder {
    fn new() -> PathBuilder {
        PathBuilder { d: String::new(), x0: f32::MAX, y0: f32::MAX, x1: f32::MIN, y1: f32::MIN }
    }
    fn pt(&mut self, x: f32, y: f32) {
        self.x0 = self.x0.min(x);
        self.x1 = self.x1.max(x);
        self.y0 = self.y0.min(y);
        self.y1 = self.y1.max(y);
    }
}

impl OutlineBuilder for PathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pt(x, y);
        let _ = write!(self.d, "M{x:.1} {:.1} ", -y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pt(x, y);
        let _ = write!(self.d, "L{x:.1} {:.1} ", -y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pt(x1, y1);
        self.pt(x, y);
        let _ = write!(self.d, "Q{x1:.1} {:.1} {x:.1} {:.1} ", -y1, -y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pt(x1, y1);
        self.pt(x2, y2);
        self.pt(x, y);
        let _ = write!(self.d, "C{x1:.1} {:.1} {x2:.1} {:.1} {x:.1} {:.1} ", -y1, -y2, -y);
    }
    fn close(&mut self) {
        self.d.push_str("Z ");
    }
}

/// 一个已放置字形的绘制数据：轮廓 path 用字体单位，落点用布局坐标（y 向上）。
struct GlyphArt {
    d: String,
    ch: char,
    x: f32,
    y: f32,
    scale: f32,
    /// 轮廓包围盒：(xmin, ymin, xmax, ymax)，字体单位、y 向上。
    bb: (f32, f32, f32, f32),
}

struct RuleArt {
    x: f32,
    y: f32,
    w: f32,
    th: f32,
}

/// 一条公式的绘制数据，坐标是字体单位（1 em = upem），画的时候整体缩放。
struct Art {
    glyphs: Vec<GlyphArt>,
    rules: Vec<RuleArt>,
    width: f32,
    height: f32,
    depth: f32,
    latex: String,
}

fn build_art(font: &MathFont, latex: &str) -> Result<Art, String> {
    let ast = parse(latex).map_err(|e| format!("{e:?}"))?;
    let l = layout(font, &ast);
    let face = font.face_ref().ok_or_else(|| "字体没有 face".to_string())?;
    if l.glyphs.is_empty() {
        return Err("布局结果为空".to_string());
    }

    let mut glyphs = Vec::with_capacity(l.glyphs.len());
    for g in &l.glyphs {
        let gid = face.glyph_index(g.ch).ok_or_else(|| format!("字体缺字 {:?}", g.ch))?;
        let mut pb = PathBuilder::new();
        if face.outline_glyph(GlyphId(gid.0), &mut pb).is_none() || pb.d.is_empty() {
            return Err(format!("字形无轮廓 {:?}", g.ch));
        }
        let bb = (pb.x0, pb.y0, pb.x1, pb.y1);
        glyphs.push(GlyphArt { d: pb.d, ch: g.ch, x: g.x, y: g.y, scale: g.scale, bb });
    }
    let rules = l
        .rules
        .iter()
        .map(|r| RuleArt { x: r.x, y: r.y, w: r.width, th: r.thickness.max(1.0) })
        .collect();

    Ok(Art { glyphs, rules, width: l.width, height: l.height, depth: l.depth, latex: latex.to_string() })
}

// ---------------------------------------------------------------- 上下文

struct Ctx {
    upem: f32,
    reg: Metrics,
    bold: Metrics,
    mono: Metrics,
    mono_bold: Metrics,
    warn: std::cell::RefCell<Vec<String>>,
    bad: std::cell::RefCell<Vec<String>>,
}

struct Spec<'a> {
    s: &'a str,
    size: f32,
    fill: &'a str,
    alpha: f32,
    mono: bool,
    bold: bool,
    ls: f32,
}

fn spec<'a>(s: &'a str, size: f32, fill: &'a str, alpha: f32) -> Spec<'a> {
    Spec { s, size, fill, alpha, mono: false, bold: false, ls: 0.0 }
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

impl Ctx {
    fn metrics(&self, mono: bool, bold: bool) -> &Metrics {
        match (mono, bold) {
            (false, false) => &self.reg,
            (false, true) => &self.bold,
            (true, false) => &self.mono,
            (true, true) => &self.mono_bold,
        }
    }

    fn tw(&self, sp: &Spec) -> f32 {
        let mut w = self.metrics(sp.mono, sp.bold).advance(sp.s, sp.size, &mut self.warn.borrow_mut());
        if sp.ls != 0.0 {
            w += sp.ls * sp.s.chars().count() as f32;
        }
        w
    }

    fn flag(&self, msg: String) {
        let mut b = self.bad.borrow_mut();
        if !b.contains(&msg) {
            b.push(msg);
        }
    }

    /// 字幕安全区检查：占住 y 900–1000 就记一笔，生成结束后统一报错。
    fn check_safe(&self, top: f32, bot: f32, what: &str) {
        if bot > SAFE_TOP && top < SAFE_BOT {
            self.flag(format!("占用字幕区 y[{top:.0},{bot:.0}]：{what}"));
        }
    }

    fn text_right(&self, sp: &Spec, right: f32, y: f32) -> String {
        let w = self.tw(sp);
        self.text_at(sp, right - w, y, false)
    }

    fn text_at(&self, sp: &Spec, x: f32, y: f32, center: bool) -> String {
        if sp.alpha <= 0.004 || sp.s.is_empty() {
            return String::new();
        }
        let fam = if sp.mono { MONO } else { SANS };
        let mut attrs = String::new();
        if sp.bold {
            attrs.push_str(r#" font-weight="bold""#);
        }
        if center {
            attrs.push_str(r#" text-anchor="middle""#);
        }
        if sp.ls != 0.0 {
            let _ = write!(attrs, r#" letter-spacing="{:.1}""#, sp.ls);
        }
        let x = if center && sp.ls != 0.0 { x - sp.ls * 0.5 } else { x };
        let w = self.tw(sp);
        let (l, r) = if center { (x - w / 2.0, x + w / 2.0) } else { (x, x + w) };
        if l < 16.0 || r > 1904.0 {
            let msg = format!("超出左右安全区 x[{l:.0},{r:.0}]  {}", sp.s);
            let mut wr = self.warn.borrow_mut();
            if !wr.contains(&msg) {
                wr.push(msg);
            }
        }
        self.check_safe(y - sp.size * 0.95, y + sp.size * 0.28, sp.s);
        format!(
            r#"<text x="{x:.1}" y="{y:.1}" font-family="{fam}" font-size="{:.1}" fill="{}" fill-opacity="{:.3}"{attrs}>{}</text>"#,
            sp.size,
            sp.fill,
            sp.alpha,
            esc(sp.s)
        )
    }
}

fn txt(ctx: &Ctx, s: &str, size: f32, fill: &str, a: f32, x: f32, y: f32, center: bool) -> String {
    ctx.text_at(&spec(s, size, fill, a), x, y, center)
}

fn mono(ctx: &Ctx, s: &str, size: f32, fill: &str, a: f32, x: f32, y: f32, center: bool) -> String {
    let sp = Spec { s, size, fill, alpha: a, mono: true, bold: false, ls: 0.0 };
    ctx.text_at(&sp, x, y, center)
}

// ---------------------------------------------------------------- 基础绘图

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn smooth(x: f32) -> f32 {
    let x = clamp01(x);
    x * x * (3.0 - 2.0 * x)
}

/// `t` 是场景内秒数：`start` 秒起，用 `dur` 秒完成淡入。
fn ramp(t: f32, start: f32, dur: f32) -> f32 {
    smooth((t - start) / dur.max(1e-4))
}

fn ease_out_cubic(x: f32) -> f32 {
    let y = 1.0 - clamp01(x);
    1.0 - y * y * y
}

/// 落位用：先越过目标一点再回弹。
fn ease_out_back(x: f32) -> f32 {
    let p = clamp01(x);
    let c1 = 1.25;
    let c3 = c1 + 1.0;
    1.0 + c3 * (p - 1.0).powi(3) + c1 * (p - 1.0).powi(2)
}

fn rect(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rx: f32,
    fill: &str,
    alpha: f32,
    stroke: Option<(&str, f32, f32)>,
) -> String {
    if alpha <= 0.004 || w <= 0.0 {
        return String::new();
    }
    let st = match stroke {
        Some((c, sw, sa)) => format!(r#" stroke="{c}" stroke-width="{sw:.1}" stroke-opacity="{sa:.3}""#),
        None => String::new(),
    };
    format!(
        r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="{rx:.1}" fill="{fill}" fill-opacity="{alpha:.3}"{st}/>"#
    )
}

fn line(x1: f32, y1: f32, x2: f32, y2: f32, c: &str, w: f32, a: f32) -> String {
    if a <= 0.004 {
        return String::new();
    }
    format!(
        r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{c}" stroke-width="{w:.1}" stroke-opacity="{a:.3}" stroke-linecap="round"/>"#
    )
}

fn dashed(x1: f32, y1: f32, x2: f32, y2: f32, c: &str, w: f32, a: f32, dash: &str) -> String {
    if a <= 0.004 {
        return String::new();
    }
    format!(
        r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{c}" stroke-width="{w:.1}" stroke-opacity="{a:.3}" stroke-dasharray="{dash}" stroke-linecap="round"/>"#
    )
}

fn dot(cx: f32, cy: f32, r: f32, c: &str, a: f32) -> String {
    if a <= 0.004 {
        return String::new();
    }
    format!(r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{c}" fill-opacity="{a:.3}"/>"#)
}

fn ring(cx: f32, cy: f32, r: f32, c: &str, w: f32, a: f32) -> String {
    if a <= 0.004 {
        return String::new();
    }
    format!(
        r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="{c}" stroke-width="{w:.1}" stroke-opacity="{a:.3}"/>"#
    )
}

/// 圆弧（度，0 在正右、顺时针增长，因为 SVG 的 y 朝下）。
fn arc(cx: f32, cy: f32, r: f32, from: f32, sweep: f32, c: &str, w: f32, a: f32) -> String {
    if a <= 0.004 {
        return String::new();
    }
    let p = |deg: f32| {
        let t = deg.to_radians();
        (cx + r * t.cos(), cy + r * t.sin())
    };
    let (x0, y0) = p(from);
    let (x1, y1) = p(from + sweep);
    let large = if sweep.abs() > 180.0 { 1 } else { 0 };
    let dir = if sweep >= 0.0 { 1 } else { 0 };
    format!(
        r#"<path d="M{x0:.1} {y0:.1} A{r:.1} {r:.1} 0 {large} {dir} {x1:.1} {y1:.1}" fill="none" stroke="{c}" stroke-width="{w:.1}" stroke-opacity="{a:.3}" stroke-linecap="round"/>"#
    )
}

/// 胶囊标签：返回 (svg, 宽度)。y 为垂直中心。
fn pill(
    ctx: &Ctx,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    fg: &str,
    bg: &str,
    a: f32,
    padx: f32,
) -> (String, f32) {
    if a <= 0.004 {
        return (String::new(), 0.0);
    }
    let sp = spec(s, size, fg, a);
    let w = ctx.tw(&sp) + padx * 2.0;
    let h = size * 1.9;
    let mut out = rect(x, y - h / 2.0, w, h, h / 2.0, bg, a * 0.9, None);
    out.push_str(&ctx.text_at(&sp, x + padx, y + size * 0.34, false));
    (out, w)
}

fn pill_w(ctx: &Ctx, s: &str, size: f32, padx: f32) -> f32 {
    ctx.tw(&spec(s, size, C_TXT, 1.0)) + padx * 2.0
}

// ---------------------------------------------------------------- 背景与外框

fn background(progress: f32, section: &str, ctx: &Ctx) -> String {
    let mut b = String::new();
    b.push_str(&format!(
        r##"<defs>
<linearGradient id="bg" x1="0" y1="0" x2="0.35" y2="1">
<stop offset="0" stop-color="#0c1420"/>
<stop offset="0.55" stop-color="#090d15"/>
<stop offset="1" stop-color="#05070c"/>
</linearGradient>
<linearGradient id="glow" x1="0" y1="0" x2="1" y2="1">
<stop offset="0" stop-color="{C_ACC}" stop-opacity="0.10"/>
<stop offset="0.6" stop-color="{C_ACC}" stop-opacity="0.00"/>
</linearGradient>
<linearGradient id="fadeup" x1="0" y1="0" x2="0" y2="1">
<stop offset="0" stop-color="#090d15" stop-opacity="1"/>
<stop offset="1" stop-color="#090d15" stop-opacity="0"/>
</linearGradient>
<linearGradient id="fadedown" x1="0" y1="0" x2="0" y2="1">
<stop offset="0" stop-color="#090d15" stop-opacity="0"/>
<stop offset="1" stop-color="#090d15" stop-opacity="1"/>
</linearGradient>
</defs>
<rect width="{FW}" height="{FH}" fill="url(#bg)"/>
<rect x="0" y="0" width="{FW}" height="620" fill="url(#glow)"/>
"##
    ));

    let mut d = String::new();
    let mut x = 96.0;
    while x < FW {
        let _ = write!(d, "M{x:.0} 0V{FH:.0}");
        x += 96.0;
    }
    let mut y = 96.0;
    while y < FH {
        let _ = write!(d, "M0 {y:.0}H{FW:.0}");
        y += 96.0;
    }
    let _ = write!(
        b,
        r##"<path d="{d}" stroke="#8fa3bf" stroke-opacity="0.045" stroke-width="1" fill="none"/>"##
    );

    // 页眉
    b.push_str(&rect(160.0, 62.0, 9.0, 9.0, 1.5, C_ACC, 0.9, None));
    b.push_str(&mono(ctx, "mathengine", 22.0, C_DIM, 0.75, 182.0, 74.0, false));
    let sec = Spec { s: section, size: 19.0, fill: C_SUB, alpha: 0.95, mono: false, bold: false, ls: 3.0 };
    b.push_str(&ctx.text_at(&sec, 1758.0, 72.0, true));

    // 页脚进度条（y=1030，落在字幕区之下）
    let bx = 160.0;
    let bw = 1600.0;
    b.push_str(&rect(bx, 1030.0, bw, 2.0, 1.0, "#29384c", 1.0, None));
    b.push_str(&rect(bx, 1030.0, bw * clamp01(progress), 2.0, 1.0, C_ACC, 0.70, None));
    for (f, _) in SCENES.iter().skip(1) {
        let tx = bx + bw * (*f as f32 / (TOTAL - 1) as f32);
        b.push_str(&rect(tx - 0.5, 1026.0, 1.0, 10.0, 0.5, C_DIM, 0.25, None));
    }
    b
}

fn section_head(ctx: &Ctx, overline: &str, title: &str, a: f32) -> String {
    let mut b = String::new();
    let over = Spec { s: overline, size: 20.0, fill: C_ACC, alpha: a, mono: false, bold: false, ls: 7.0 };
    b.push_str(&ctx.text_at(&over, 176.0, 180.0, false));
    let ttl = Spec { s: title, size: 42.0, fill: C_TXT, alpha: a, mono: false, bold: true, ls: 0.0 };
    b.push_str(&ctx.text_at(&ttl, 176.0, 236.0, false));
    b.push_str(&rect(146.0, 196.0, 4.0, 42.0, 2.0, C_ACC, a, None));
    b
}

// ---------------------------------------------------------------- 一、网页方案

fn scene_web(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let ea = ramp(t, 0.0, 0.7);

    b.push_str(&section_head(ctx, "第一个问题", "网页里的公式，得先有一个浏览器", ea));

    // ---- 浏览器窗口
    let (wx, wy, ww, wh) = (120.0, 268.0, 1020.0, 470.0);
    let wa = ramp(t, 0.25, 0.7);
    b.push_str(&rect(wx, wy, ww, wh, 14.0, "#0c121c", 0.98 * wa, Some(("#243248", 1.5, wa))));
    b.push_str(&rect(wx, wy, ww, 46.0, 14.0, "#131c2a", wa, None));
    b.push_str(&rect(wx, wy + 34.0, ww, 12.0, 0.0, "#131c2a", wa, None));
    for (i, c) in ["#ff6159", "#ffbd2e", "#28c840"].iter().enumerate() {
        b.push_str(&dot(wx + 26.0 + i as f32 * 20.0, wy + 23.0, 5.5, c, 0.85 * wa));
    }
    b.push_str(&rect(wx + 116.0, wy + 10.0, 610.0, 26.0, 13.0, "#0a0f18", wa, None));
    b.push_str(&mono(ctx, "https://example.edu/formula-demo", 14.0, C_DIM, 0.85 * wa, wx + 136.0, wy + 28.0, false));

    // 页内标题与段落骨架
    b.push_str(&rect(wx + 60.0, wy + 96.0, 360.0, 16.0, 8.0, "#2b3a52", 0.9 * ramp(t, 1.0, 0.5), None));
    let lines = [
        (60.0, 1.4, 720.0),
        (60.0, 1.55, 660.0),
        (60.0, 1.7, 700.0),
        (60.0, 1.85, 420.0),
    ];
    for (x, st, w) in lines {
        b.push_str(&rect(wx + x, wy + 130.0 + (st - 1.4) * 95.0, w, 10.0, 5.0, "#243147", 0.85 * ramp(t, st, 0.5), None));
    }

    // 公式占位区：三个灰块 + 加载圈
    let fa = ramp(t, 2.2, 0.6);
    b.push_str(&rect(wx + 60.0, wy + 200.0, 720.0, 150.0, 10.0, "#0a0f18", fa, Some(("#26344a", 1.2, fa))));
    b.push_str(&dashed(wx + 60.0, wy + 200.0, wx + 780.0, wy + 200.0, C_FAINT, 1.0, 0.0, "2 6"));
    let skel = [(190.0, 64.0, 44.0, 52.0), (262.0, 46.0, 38.0, 58.0), (322.0, 96.0, 40.0, 44.0), (430.0, 54.0, 30.0, 62.0)];
    for (x, w, h, y) in skel {
        b.push_str(&rect(wx + x, wy + 200.0 + y, w, h, 4.0, "#2a3850", 0.55 * fa, None));
    }
    // 转圈
    let spin = t * 250.0;
    b.push_str(&arc(1002.0, 332.0, 11.0, spin, 265.0, C_ACC, 3.0, 0.9 * fa));
    b.push_str(&mono(ctx, "加载中", 14.0, C_DIM, 0.9 * fa, 1020.0, 338.0, false));

    // 加载进度：爬到 62% 卡住，后面小幅抖动
    let p0 = ease_out_cubic(clamp01((t - 2.4) / 6.0)) * 0.62;
    let jitter = if t > 9.0 { ((t * 7.0).sin() * 0.006).max(0.0) } else { 0.0 };
    let prog = (p0 + jitter).min(0.665);
    b.push_str(&rect(wx + 60.0, wy + 388.0, 720.0, 6.0, 3.0, "#26344a", fa, None));
    b.push_str(&rect(wx + 60.0, wy + 388.0, 720.0 * prog, 6.0, 3.0, C_AMBER, 0.85 * fa, None));
    let pct = format!("{:.0}%", prog * 100.0);
    b.push_str(&mono(ctx, &pct, 15.0, C_AMBER, 0.9 * fa, wx + 60.0 + 720.0 * prog + 12.0, wy + 381.0, false));
    if t > 9.5 {
        let blink = 0.55 + 0.45 * (t * 3.0).sin();
        b.push_str(&txt(ctx, "排版工作停在浏览器里，进度条不动了", 16.0, C_DIM, 0.9 * blink, wx + 60.0, wy + 424.0, false));
    }

    // ---- 包体积（量级示意）
    let sa = ramp(t, 7.6, 0.6);
    b.push_str(&mono(ctx, "包体积 · 量级示意", 16.0, C_SUB, 0.95 * sa, wx, 776.0, false));
    let bars = [("MathJax 3", 1.00, "约 1.1 MB", C_RED), ("KaTeX + 字体", 0.34, "约 0.35 MB", C_AMBER)];
    for (i, (name, v, val, col)) in bars.iter().enumerate() {
        let y = 808.0 + i as f32 * 34.0;
        let ga = ramp(t, 8.0 + i as f32 * 0.5, 0.9);
        b.push_str(&mono(ctx, name, 16.0, C_DIM, 0.95 * ga, wx, y + 5.0, false));
        b.push_str(&rect(wx + 210.0, y - 6.0, 520.0, 10.0, 5.0, "#1b2536", 0.9 * ga, None));
        let grow = ease_out_cubic(ga);
        b.push_str(&rect(wx + 210.0, y - 6.0, 520.0 * v * grow, 10.0, 5.0, col, 0.85 * ga, None));
        b.push_str(&mono(ctx, val, 15.0, C_DIM, 0.95 * ga, wx + 750.0, y + 5.0, false));
    }

    // ---- 右侧手机：空白页
    let pa = ramp(t, 11.4, 0.8);
    let (px, py, pw, ph) = (1332.0, 250.0, 330.0, 552.0);
    b.push_str(&mono(ctx, "离线 App 里", 16.0, C_SUB, 0.9 * pa, px + pw / 2.0, py - 22.0, true));
    let lift = (1.0 - ease_out_cubic(pa)) * 26.0;
    b.push_str(&rect(px, py + lift, pw, ph, 40.0, "#0a0f17", pa, Some(("#2b3a52", 2.0, pa))));
    b.push_str(&rect(px + 10.0, py + 10.0 + lift, pw - 20.0, ph - 20.0, 32.0, "#070b12", pa, None));
    b.push_str(&rect(px + pw / 2.0 - 34.0, py + 18.0 + lift, 68.0, 6.0, 3.0, "#1d2838", pa, None));

    // 页面内容缺失：只剩占位框
    let inner_a = pa * ramp(t, 12.0, 0.6);
    b.push_str(&rect(px + 34.0, py + 66.0 + lift, 150.0, 12.0, 6.0, "#202c40", 0.8 * inner_a, None));
    b.push_str(&rect(px + 34.0, py + 96.0 + lift, 230.0, 9.0, 4.5, "#1a2536", 0.8 * inner_a, None));
    b.push_str(&rect(px + 34.0, py + 116.0 + lift, 196.0, 9.0, 4.5, "#1a2536", 0.8 * inner_a, None));
    b.push_str(&rect(px + 34.0, py + 160.0 + lift, 262.0, 150.0, 8.0, "#0b111b", inner_a, Some(("#2a3a52", 1.0, inner_a))));
    b.push_str(&rect(px + 34.0, py + 404.0 + lift, 262.0, 10.0, 5.0, "#1a2536", 0.9 * inner_a, None));
    b.push_str(&rect(px + 34.0, py + 426.0 + lift, 200.0, 10.0, 5.0, "#1a2536", 0.9 * inner_a, None));
    b.push_str(&rect(px + 34.0, py + 448.0 + lift, 232.0, 10.0, 5.0, "#1a2536", 0.9 * inner_a, None));

    let fail = ramp(t, 13.2, 0.6);
    b.push_str(&arc(px + 165.0, py + 228.0 + lift, 16.0, t * 300.0, 250.0, C_RED, 3.0, 0.75 * fail * (1.0 - ramp(t, 15.0, 0.6))));
    if t > 15.0 {
        let s = ease_out_back(ramp(t, 15.0, 0.5));
        let cx = px + 165.0;
        let cy = py + 228.0 + lift;
        b.push_str(&ring(cx, cy, 26.0 * s, C_RED, 2.0, 0.9 * fail));
        b.push_str(&line(cx - 9.0 * s, cy - 9.0 * s, cx + 9.0 * s, cy + 9.0 * s, C_RED, 3.4, 0.95 * fail));
        b.push_str(&line(cx + 9.0 * s, cy - 9.0 * s, cx - 9.0 * s, cy + 9.0 * s, C_RED, 3.4, 0.95 * fail));
    }
    let ta = ramp(t, 15.6, 0.6);
    b.push_str(&txt(ctx, "无法显示公式", 20.0, C_TXT, 0.95 * ta, px + 165.0, py + 352.0 + lift, true));
    b.push_str(&txt(ctx, "缺少浏览器内核", 15.0, C_RED, 0.85 * ta, px + 165.0, py + 380.0 + lift, true));
    b.push_str(&txt(ctx, "页面打不开", 15.0, C_FAINT, 0.9 * ramp(t, 17.0, 0.6), px + 165.0, 846.0, true));

    // 右上角三条结论
    let items = ["体积大", "依赖浏览器", "首屏等待"];
    let mut x;
    let mut widths = Vec::new();
    for s in items {
        widths.push(pill_w(ctx, s, 17.0, 16.0));
    }
    let total: f32 = widths.iter().sum::<f32>() + 12.0 * 2.0;
    x = 1796.0 - total;
    for (i, s) in items.iter().enumerate() {
        let ga = ramp(t, 18.6 + i as f32 * 0.35, 0.5);
        let (svg, w) = pill(ctx, s, x, 196.0, 17.0, C_RED, "#1b1218", ga * 0.95, 16.0);
        b.push_str(&svg);
        x += w + 12.0;
    }

    b
}

// ---------------------------------------------------------------- 二、原理

struct CodePanel {
    lines: Vec<String>,
    start_no: usize,
    size: f32,
    lh: f32,
    x: f32,
    y0: f32,
}

fn draw_code(ctx: &Ctx, c: &CodePanel, revealed: usize, hi: (usize, usize), ha: f32) -> String {
    let mut b = String::new();
    let (px, py, pw, ph) = (100.0, 292.0, 860.0, 566.0);
    b.push_str(&rect(px, py, pw, ph, 12.0, C_PANEL2, 0.97, Some(("#22304a", 1.2, 0.9))));
    // 标题条
    b.push_str(&rect(px, py, pw, 42.0, 12.0, "#121a27", 0.98, None));
    b.push_str(&rect(px, py + 30.0, pw, 12.0, 0.0, "#121a27", 0.98, None));
    b.push_str(&dot(px + 22.0, py + 21.0, 4.5, C_ACC, 0.8));
    b.push_str(&mono(ctx, "src/layout.rs", 14.0, C_DIM, 0.95, px + 38.0, py + 26.0, false));
    let range = format!("{}–{} 行", c.start_no, c.start_no + c.lines.len() - 1);
    b.push_str(&ctx.text_right(&spec(&range, 13.0, C_FAINT, 0.9), px + pw - 18.0, py + 26.0));
    b.push_str(&line(px, py + 42.0, px + pw, py + 42.0, "#22304a", 1.0, 0.9));

    // 高亮条
    if ha > 0.01 && hi.1 >= hi.0 {
        let y1 = c.y0 + hi.0 as f32 * c.lh - c.lh * 0.78;
        let y2 = c.y0 + hi.1 as f32 * c.lh + c.lh * 0.24;
        b.push_str(&rect(px + 2.0, y1, pw - 4.0, y2 - y1, 4.0, C_ACC, 0.09 * ha, None));
        b.push_str(&rect(px + 2.0, y1, 3.0, y2 - y1, 1.5, C_ACC, 0.9 * ha, None));
    }

    for (i, s) in c.lines.iter().enumerate() {
        if i >= revealed {
            break;
        }
        let y = c.y0 + i as f32 * c.lh;
        let no = format!("{:>4}", c.start_no + i);
        b.push_str(&mono(ctx, &no, c.size * 0.92, C_FAINT, 0.55, px + 16.0, y, false));
        let a = if i + 1 >= revealed { 0.55 } else { 1.0 };
        b.push_str(&mono(ctx, s, c.size, "#d3dde9", 0.92 * a, c.x, y, false));
    }
    b
}

fn stage_of(t: f32) -> (usize, f32) {
    let k = ((t / 13.0).floor() as usize).min(3);
    (k, t - k as f32 * 13.0)
}

fn scene_how(ctx: &Ctx, art: &Art, code: &CodePanel, t: f32) -> String {
    let mut b = String::new();
    let ea = ramp(t, 0.0, 0.6);
    b.push_str(&section_head(ctx, "怎么做的", "一行公式，拆成符号，算出坐标，再画出来", ea));

    let (stage, st) = stage_of(t);
    // 代码：0–4.5 s 逐行出现
    let revealed = ((t / 0.14) as usize).min(code.lines.len());
    let hi = match stage {
        0 => (2usize, 3usize),
        1 => (9usize, 13usize),
        2 => (18usize, 19usize),
        _ => (0usize, code.lines.len() - 1),
    };
    b.push_str(&draw_code(ctx, code, revealed, hi, ramp(t, 0.2, 0.6)));

    // 右侧舞台
    let (sx, sy, sw, sh) = (1002.0, 292.0, 818.0, 566.0);
    b.push_str(&rect(sx, sy, sw, sh, 12.0, "#080d15", 0.55, Some(("#1c2a3e", 1.0, 0.8))));

    // 步骤指示
    let titles = ["读入公式文本", "拆成符号单元", "算出每个坐标", "按坐标绘制"];
    for i in 0..4 {
        let x = sx + 30.0 + i as f32 * 196.0;
        let on = i == stage;
        let done = i < stage;
        b.push_str(&dot(x + 5.0, sy + 28.0, 5.0, if on { C_ACC } else { C_FAINT }, if on { 1.0 } else { 0.55 }));
        let c = if on { C_TXT } else if done { C_DIM } else { C_FAINT };
        b.push_str(&txt(ctx, titles[i], 15.0, c, if on { 0.98 } else if done { 0.85 } else { 0.62 }, x + 18.0, sy + 33.0, false));
    }

    // 阶段说明
    let cap = match stage {
        0 => format!("一条 LaTeX 字符串，{} 个字符", art.latex.chars().count()),
        1 => format!("按布局顺序切成 {} 个符号单元", art.glyphs.len()),
        2 => "每个单元都有 x、y 与缩放".to_string(),
        _ => "同一份布局数据，逐帧离线生成画面".to_string(),
    };
    b.push_str(&txt(ctx, &cap, 21.0, C_DIM, 0.95 * ramp(t, 0.4, 0.5), sx + 30.0, sy + 78.0, false));

    // 原始文本（阶段一）
    let ta = ramp(t, 0.5, 0.5) * (1.0 - ramp(t, 12.3, 0.6));
    if ta > 0.01 {
        let n = ((st - 0.6) / 0.09).max(0.0) as usize;
        let chars: Vec<char> = art.latex.chars().collect();
        let shown: String = chars.iter().take(n.min(chars.len())).collect();
        let size = 26.0;
        let tw = ctx.tw(&spec(art.latex.as_str(), size, C_TXT, 1.0));
        let tx = 1411.0 - tw / 2.0;
        let ty = 512.0;
        b.push_str(&rect(tx - 26.0, ty - 44.0, tw + 52.0, 70.0, 8.0, "#0c131f", 0.95 * ta, Some(("#233248", 1.0, ta))));
        b.push_str(&mono(ctx, &shown, size, C_TXT, 0.96 * ta, tx, ty, false));
        // 光标
        let cw = ctx.tw(&spec(&shown, size, C_TXT, 1.0));
        let blink = if (t * 2.2).sin() > -0.2 { 1.0 } else { 0.0 };
        b.push_str(&rect(tx + cw + 3.0, ty - size * 0.78, 2.2, size * 0.95, 1.0, C_ACC, ta * blink, None));
        b.push_str(&mono(ctx, "latex 输入", 13.0, C_FAINT, 0.9 * ta, tx - 26.0, ty + 52.0, false));
    }

    // 阶段一：字符数 + 解析出的结构
    let sa = ramp(t, 3.6, 0.6) * (1.0 - ramp(t, 12.2, 0.5));
    if sa > 0.01 {
        let chars = art.latex.chars().count();
        let info = format!("{chars} 个字符 → {} 个符号单元", art.glyphs.len());
        b.push_str(&mono(ctx, &info, 15.0, C_DIM, 0.9 * sa, 1411.0, 588.0, true));
        let chips = ["分式", "根号", "上标", "二元运算"];
        let widths: Vec<f32> = chips.iter().map(|c| pill_w(ctx, c, 15.0, 14.0)).collect();
        let total: f32 = widths.iter().sum::<f32>() + 10.0 * 3.0;
        let mut x = 1411.0 - total / 2.0;
        for (i, c) in chips.iter().enumerate() {
            let ga = ramp(t, 4.2 + i as f32 * 0.35, 0.55) * sa;
            let (svg, w) = pill(ctx, c, x, 660.0, 15.0, C_SUB, "#101a26", ga, 14.0);
            b.push_str(&svg);
            x += w + 10.0;
        }
    }

    // 符号单元 → 坐标 → 绘制
    let n = art.glyphs.len();
    let k_fit = {
        let avail_w = 760.0;
        let avail_h = 400.0;
        (avail_w / art.width).min(avail_h / (art.height + art.depth)) * 0.92
    };
    let cx0 = 1411.0;
    let cy0 = 596.0;
    let left = cx0 - art.width * k_fit / 2.0;
    let baseline0 = cy0 + (art.height - art.depth) * k_fit / 2.0;

    let grid_cols = 4usize;
    let tile = 136.0;
    let gap = 22.0;
    let grid_w = grid_cols as f32 * tile + (grid_cols - 1) as f32 * gap;
    let rows = n.div_ceil(grid_cols);
    let grid_h = rows as f32 * tile + (rows - 1) as f32 * gap;
    let gx0 = cx0 - grid_w / 2.0 + tile / 2.0;
    let gy0 = 634.0 - grid_h / 2.0 + tile / 2.0;

    let mut g = String::from(r#"<g data-formula="1">"#);
    for (i, gl) in art.glyphs.iter().enumerate() {
        let (bx0, by0, bx1, by1) = gl.bb;
        let bcx = (bx0 + bx1) / 2.0;
        let bcy = (by0 + by1) / 2.0;
        let bwh = ((bx1 - bx0).max(1.0)).max(by1 - by0);
        let k_tile = 70.0 / bwh.max(1.0);
        let k_final = k_fit * gl.scale;

        let (gcx, gcy) = (gx0 + (i % grid_cols) as f32 * (tile + gap), gy0 + (i / grid_cols) as f32 * (tile + gap));
        let (fcx, fcy) = (left + gl.x * k_fit + bcx * k_final, baseline0 - gl.y * k_fit - bcy * k_final);

        // 阶段一：藏在文本中心；二：飞到网格；三：飞到坐标；四：只留字形
        let p_b = ease_out_back(clamp01((t - 12.9 - i as f32 * 0.04) / 0.95));
        let p_c = ease_out_cubic(clamp01((t - 25.9 - i as f32 * 0.03) / 1.15));
        let grow = 1.0 + 0.10 * ease_out_cubic(clamp01((t - 39.0) / 1.0));
        let cx = cx0 + (gcx - cx0) * p_b + (fcx - gcx) * p_c;
        let cy = 512.0 + (gcy - 512.0) * p_b + (fcy - gcy) * p_c;
        let k = k_tile + (k_final - k_tile) * p_c;
        let k = k * (1.0 + (grow - 1.0) * p_c);
        let alpha = ramp(t, 12.9 + i as f32 * 0.04, 0.35);
        let frame_a = p_b * (1.0 - p_c) * (1.0 - ramp(t, 39.0, 0.6));
        let idx_a = (1.0 - p_c) * (1.0 - ramp(t, 39.0, 0.5)) * p_b;
        if alpha > 0.01 {
            let (px, py) = (cx - bcx * k, cy + bcy * k);
            if frame_a > 0.01 {
                let half = tile / 2.0 * (1.0 - 0.38 * p_c) * (0.85 + 0.15 * p_b);
                g.push_str(&rect(cx - half, cy - half, half * 2.0, half * 2.0, 10.0, "#0d1521", 0.9 * frame_a, Some(("#24344c", 1.2, frame_a))));
            }
            let _ = write!(
                g,
                r#"<path transform="translate({px:.2} {py:.2}) scale({k:.5})" fill="{}" fill-opacity="{alpha:.3}" d="{}"/>"#,
                C_TXT, gl.d
            );
            if idx_a > 0.01 {
                g.push_str(&mono(ctx, &format!("{:02}", i + 1), 12.0, C_ACC, 0.75 * idx_a * alpha, cx - tile / 2.0 + 14.0, cy - tile / 2.0 + 22.0, false));
            }
        }
    }
    // 分数线（阶段三之后）
    let ra = ramp(t, 27.0, 0.8);
    if ra > 0.01 {
        for r in &art.rules {
            let x = left + r.x * k_fit;
            let y = baseline0 - r.y * k_fit;
            g.push_str(&rect(x, y, r.w * k_fit * ra, (r.th * k_fit).max(1.6), 0.5, C_TXT, 0.95, None));
        }
    }
    g.push_str("</g>");
    b.push_str(&g);

    // 阶段三：轮播一个符号的坐标读数
    let ra2 = ramp(t, 27.0, 0.6) * (1.0 - ramp(t, 39.2, 0.5));
    if ra2 > 0.01 && t > 26.0 {
        let pick = (((t - 26.0) / 2.3) as usize) % n;
        let g0 = &art.glyphs[pick];
        let line1 = format!("第 {:02} 个符号单元   ·   x = {:.0}   y = {:.0}   缩放 {:.2}", pick + 1, g0.x, g0.y, g0.scale);
        let w = ctx.tw(&spec(&line1, 15.0, C_TXT, 1.0));
        b.push_str(&rect(1411.0 - w / 2.0 - 16.0, 404.0, w + 32.0, 30.0, 8.0, "#0c1420", 0.9 * ra2, Some(("#26364e", 1.0, 0.9 * ra2))));
        b.push_str(&mono(ctx, &line1, 15.0, C_BLUE, 0.95 * ra2, 1411.0 - w / 2.0, 425.0, false));
        // 圈出当前读数对应的那个符号
        let (bx0, by0, bx1, by1) = g0.bb;
        let gl_cx = left + g0.x * k_fit + (bx0 + bx1) / 2.0 * k_fit * g0.scale;
        let gl_cy = baseline0 - g0.y * k_fit - (by0 + by1) / 2.0 * k_fit * g0.scale;
        let half_w = (bx1 - bx0) * k_fit * g0.scale / 2.0 + 8.0;
        let half_h = (by1 - by0) * k_fit * g0.scale / 2.0 + 8.0;
        let pulse = 0.6 + 0.4 * ((t * 3.0).sin() * 0.5 + 0.5);
        b.push_str(&rect(gl_cx - half_w, gl_cy - half_h, half_w * 2.0, half_h * 2.0, 6.0, C_BLUE, 0.07 * ra2, Some((C_BLUE, 1.8, 0.65 * ra2 * pulse))));
    }

    // 坐标轴：阶段三出现
    let axa = ramp(t, 26.6, 0.7) * (1.0 - ramp(t, 39.0, 0.6));
    if axa > 0.01 {
        // 基线只在公式两侧画，避免虚线压在字形上
        b.push_str(&dashed(1018.0, baseline0, left - 18.0, baseline0, C_BLUE, 1.2, 0.45 * axa, "5 6"));
        b.push_str(&dashed(left + art.width * k_fit + 18.0, baseline0, 1804.0, baseline0, C_BLUE, 1.2, 0.45 * axa, "5 6"));
        let x0 = left - 30.0;
        b.push_str(&dashed(x0, 400.0, x0, 790.0, C_BLUE, 1.2, 0.22 * axa, "4 8"));
        // 尺寸标注
        let dy = baseline0 + art.depth * k_fit + 26.0;
        b.push_str(&line(left, dy, left + art.width * k_fit, dy, C_DIM, 1.0, 0.5 * axa));
        b.push_str(&line(left, dy - 5.0, left, dy + 5.0, C_DIM, 1.0, 0.5 * axa));
        b.push_str(&line(left + art.width * k_fit, dy - 5.0, left + art.width * k_fit, dy + 5.0, C_DIM, 1.0, 0.5 * axa));
        let dim = format!("宽 {:.0} · 高 {:.0} · 深 {:.0}（字体单位）", art.width, art.height, art.depth);
        b.push_str(&mono(ctx, &dim, 14.0, C_DIM, 0.8 * axa, left + art.width * k_fit / 2.0, dy + 34.0, true));
    }

    // 第四阶段收尾说明
    let done = ramp(t, 40.0, 0.7);
    if done > 0.01 {
        b.push_str(&txt(ctx, "布局结果 → 扁平字形列表 → 一次绘制", 15.0, C_SUB, 0.9 * done, 1411.0, 826.0, true));
    }

    b
}

// ---------------------------------------------------------------- 三、优势

fn scene_size(ctx: &Ctx, kb: u32, t: f32) -> String {
    let mut b = String::new();
    let ea = ramp(t, 0.0, 0.6);
    b.push_str(&section_head(ctx, "01 / 03 · 体积", "编译出来只有一个动态库", ea));

    // 文件图标
    let ia = ease_out_back(ramp(t, 0.3, 0.7));
    let (fx, fy) = (250.0, 400.0);
    let fw = 150.0;
    let fh = 190.0;
    let s = 0.92 + 0.08 * ia;
    let cx = fx + fw / 2.0;
    let cy = fy + fh / 2.0;
    let (ix, iy) = (cx - fw * s / 2.0, cy - fh * s / 2.0);
    b.push_str(&format!(
        r##"<path d="M{ix:.1} {iy:.1} h{:.1} l44 44 v{:.1} h{:.1} z" fill="#101a28" fill-opacity="{:.3}" stroke="#2c3d56" stroke-width="1.5" stroke-opacity="{:.3}"/>"##,
        fw * s - 44.0,
        fh * s - 44.0,
        -(fw * s),
        0.97 * ia,
        0.9 * ia
    ));
    b.push_str(&format!(
        r##"<path d="M{:.1} {iy:.1} v44 h44" fill="none" stroke="#2c3d56" stroke-width="1.5" stroke-opacity="{:.3}"/>"##,
        ix + fw * s - 44.0,
        0.9 * ia
    ));
    b.push_str(&mono(ctx, ".so", 30.0, C_ACC, 0.95 * ramp(t, 0.7, 0.5), cx, iy + fh * s * 0.66, true));
    b.push_str(&mono(ctx, "libmathengine", 12.0, C_FAINT, 0.8 * ramp(t, 0.9, 0.5), cx, iy + fh * s * 0.82, true));

    // 大数字
    let cnt = kb as f32 * ease_out_cubic(ramp(t, 0.5, 1.6));
    let num = format!("{:.0}", cnt);
    let na = ramp(t, 0.4, 0.5);
    b.push_str(&ctx.text_at(&Spec { s: &num, size: 158.0, fill: C_TXT, alpha: na, mono: false, bold: true, ls: 0.0 }, 690.0, 570.0, true));
    b.push_str(&txt(ctx, "KB", 54.0, C_ACC, 0.95 * na, 840.0, 552.0, false));
    b.push_str(&txt(ctx, "编译产物 · 单文件", 22.0, C_DIM, 0.95 * ramp(t, 1.4, 0.6), 690.0, 640.0, true));
    b.push_str(&mono(ctx, "x86_64 · opt-level=s · LTO · strip", 16.0, C_SUB, 0.9 * ramp(t, 1.8, 0.6), 690.0, 674.0, true));
    let sta = ramp(t, 2.0, 0.6);
    b.push_str(&txt(ctx, "源码 2900 余行 Rust · 84 组用例 · 只依赖 ttf-parser", 17.0, C_DIM, 0.95 * sta, 690.0, 720.0, true));

    // 对比条
    let ba = ramp(t, 2.4, 0.6);
    b.push_str(&mono(ctx, "体积对比（十进制 KB，量级示意）", 15.0, C_SUB, 0.9 * ba, 1000.0, 400.0, false));
    let rows = [
        ("本引擎 .so", kb as f32, format!("{kb} KB"), C_ACC),
        ("jlatexmath jar", 669.0, "669 KB".to_string(), C_DIM),
        ("浏览器方案", 1100.0, "约 1100 KB".to_string(), C_RED),
    ];
    // 数值列固定右对齐，别跟着条形跑

    let max = 1100.0;
    for (i, (name, v, val, col)) in rows.iter().enumerate() {
        let y = 460.0 + i as f32 * 92.0;
        let ga = ramp(t, 2.6 + i as f32 * 0.45, 0.8);
        b.push_str(&txt(ctx, name, 19.0, C_DIM, 0.95 * ga, 1000.0, y + 7.0, false));
        b.push_str(&rect(1000.0, y + 26.0, 700.0, 12.0, 6.0, "#182233", 0.9 * ga, None));
        let grow = ease_out_cubic(ga);
        let w = 700.0 * (v / max) * grow;
        b.push_str(&rect(1000.0, y + 26.0, w, 12.0, 6.0, col, 0.88 * ga, None));
        let vs = spec(val, 17.0, C_DIM, 0.95 * ga);
        b.push_str(&ctx.text_right(&vs, 1800.0, y + 37.0));
    }
    b.push_str(&txt(ctx, "宿主应用已有字体时，只需再加这一个动态库", 16.0, C_SUB, 0.9 * ramp(t, 4.2, 0.6), 1000.0, 700.0, false));

    // 结论标签
    let items = ["单文件", "无 JVM", "无浏览器"];
    let padx = 15.0;
    let widths: Vec<f32> = items.iter().map(|s| pill_w(ctx, s, 17.0, padx)).collect();
    let total: f32 = widths.iter().sum::<f32>() + 12.0 * (items.len() - 1) as f32;
    let mut x = 960.0 - total / 2.0;
    for (i, s) in items.iter().enumerate() {
        let ga = ramp(t, 5.0 + i as f32 * 0.25, 0.5);
        let (svg, w) = pill(ctx, s, x, 792.0, 17.0, C_TXT, "#12202c", 0.95 * ga, padx);
        b.push_str(&svg);
        x += w + 12.0;
    }
    b
}

fn scene_tests(ctx: &Ctx, names: &[String], t: f32) -> String {
    let mut b = String::new();
    let ea = ramp(t, 0.0, 0.6);
    b.push_str(&section_head(ctx, "02 / 03 · 用例", "84 组用例，全部通过", ea));

    let na = ramp(t, 0.4, 0.7);
    let cnt = (names.len() as f32 * ease_out_cubic(ramp(t, 0.4, 1.2))).round();
    b.push_str(&ctx.text_at(&Spec { s: &format!("{cnt:.0}"), size: 168.0, fill: C_TXT, alpha: na, mono: false, bold: true, ls: 0.0 }, 330.0, 520.0, true));
    b.push_str(&txt(ctx, "组测试用例", 24.0, C_DIM, 0.95 * na, 330.0, 578.0, true));
    b.push_str(&line(160.0, 616.0, 500.0, 616.0, C_LINE, 1.0, 0.9 * na));
    b.push_str(&txt(ctx, "68 组单元测试", 18.0, C_SUB, 0.9 * ramp(t, 1.4, 0.5), 200.0, 660.0, false));
    b.push_str(&txt(ctx, "16 组集成测试", 18.0, C_SUB, 0.9 * ramp(t, 1.6, 0.5), 200.0, 694.0, false));
    b.push_str(&mono(ctx, "cargo test", 16.0, C_ACC, 0.85 * ramp(t, 2.2, 0.6), 200.0, 748.0, false));
    b.push_str(&txt(ctx, "解析 · 布局 · 稳定性 · 公开接口", 16.0, C_SUB, 0.9 * ramp(t, 2.8, 0.6), 200.0, 778.0, false));

    // 列表：两列，匀速上滚，边缘渐隐
    let (lx, ly, lw, lh) = (620.0, 312.0, 1190.0, 546.0);
    b.push_str(&format!(r#"<defs><clipPath id="listclip"><rect x="{lx}" y="{ly}" width="{lw}" height="{lh}"/></clipPath></defs>"#));
    b.push_str(&rect(lx, ly, lw, lh, 10.0, "#080d15", 0.5, Some(("#1b283c", 1.0, 0.8))));

    let per_col = names.len().div_ceil(2);
    let row_h = 30.0;
    let offset = t * 40.0;
    let mut g = String::from(r#"<g clip-path="url(#listclip)">"#);
    for (i, name) in names.iter().enumerate() {
        let col = i / per_col;
        let row = i % per_col;
        let x = lx + 26.0 + col as f32 * 592.0;
        let y = ly + 30.0 + row as f32 * row_h - offset;
        if y < ly - 30.0 || y > ly + lh + 30.0 {
            continue;
        }
        let d = ((y - (ly + lh / 2.0)).abs() / (lh / 2.0)).min(1.0);
        let a = 0.70 + 0.28 * (1.0 - d * d);
        let appear = ramp(t, 0.3 + i as f32 * 0.012, 0.5);
        g.push_str(&mono(ctx, &format!("{:02}", i + 1), 13.0, C_FAINT, a * 0.85 * appear, x, y, false));
        g.push_str(&txt(ctx, name, 18.0, "#c3cfdf", a * appear, x + 38.0, y, false));
        g.push_str(&txt(ctx, "✓", 15.0, C_ACC, a * 0.95 * appear, x + 470.0, y, false));
    }
    g.push_str("</g>");
    b.push_str(&g);
    b.push_str(&rect(lx, ly, lw, 46.0, 0.0, "#090d15", 0.85, None));
    b.push_str(&format!(r##"<rect x="{lx}" y="{ly}" width="{lw}" height="40" fill="url(#fadeup)" fill-opacity="0.72"/>"##));
    b.push_str(&format!(r##"<rect x="{lx}" y="{}" width="{lw}" height="60" fill="url(#fadedown)" fill-opacity="0.95"/>"##, ly + lh - 60.0));
    b
}

fn scene_phone(ctx: &Ctx, art: &Art, t: f32) -> String {
    let mut b = String::new();
    let ea = ramp(t, 0.0, 0.6);

    // 左侧文案
    b.push_str(&section_head(ctx, "03 / 03 · 手机", "手机里直接画，不用浏览器", ea));
    b.push_str(&txt(ctx, "布局由引擎自己算完，", 24.0, C_DIM, 0.95 * ramp(t, 0.8, 0.6), 176.0, 360.0, false));
    b.push_str(&txt(ctx, "界面拿到坐标直接绘制。", 24.0, C_DIM, 0.95 * ramp(t, 1.0, 0.6), 176.0, 398.0, false));

    let feats = [
        ("包内不带浏览器内核", "安装包不因公式变大"),
        ("不依赖 JVM", "纯 Rust，可编到 arm64"),
        ("只依赖 ttf-parser", "一个字体解析库"),
    ];
    for (i, (h, s)) in feats.iter().enumerate() {
        let ga = ramp(t, 2.2 + i as f32 * 0.7, 0.55);
        let y = 480.0 + i as f32 * 106.0;
        b.push_str(&dot(184.0, y - 6.0, 4.5, C_ACC, 0.85 * ga));
        b.push_str(&txt(ctx, h, 21.0, C_TXT, 0.96 * ga, 204.0, y, false));
        b.push_str(&txt(ctx, s, 16.0, C_SUB, 0.85 * ga, 204.0, y + 28.0, false));
    }

    // 手机
    let (px, py, pw, ph) = (1150.0, 190.0, 380.0, 678.0);
    let pa = ramp(t, 0.2, 0.7);
    let lift = (1.0 - ease_out_cubic(pa)) * 40.0;
    b.push_str(&rect(px, py + lift, pw, ph, 46.0, "#0a0f17", pa, Some(("#2d3d57", 2.2, pa))));
    b.push_str(&rect(px + 9.0, py + 9.0 + lift, pw - 18.0, ph - 18.0, 38.0, "#070b12", pa, None));
    b.push_str(&rect(px + pw / 2.0 - 42.0, py + 20.0 + lift, 84.0, 7.0, 3.5, "#1e2a3c", pa, None));

    let ua = pa * ramp(t, 0.8, 0.6);
    b.push_str(&mono(ctx, "9:41", 13.0, C_DIM, 0.8 * ua, px + 34.0, py + 46.0 + lift, false));
    b.push_str(&text_right_safe(ctx, "离线", 13.0, C_ACC, 0.8 * ua, px + pw - 34.0, py + 46.0 + lift));

    // 应用界面
    b.push_str(&txt(ctx, "错题本", 22.0, C_TXT, 0.95 * ua, px + 34.0, py + 100.0 + lift, false));
    b.push_str(&line(px + 34.0, py + 120.0 + lift, px + pw - 34.0, py + 120.0 + lift, C_LINE, 1.0, 0.9 * ua));
    b.push_str(&txt(ctx, "第 3 题 · 二次方程", 14.0, C_SUB, 0.9 * ua, px + 34.0, py + 150.0 + lift, false));
    b.push_str(&rect(px + 26.0, py + 172.0 + lift, pw - 52.0, 190.0, 14.0, "#0d1521", 0.95 * ua, Some(("#22304a", 1.0, ua))));

    let fy = py + 172.0 + lift;
    b.push_str(&reveal_art_x(art, ctx, px + pw / 2.0, fy + 104.0, 30.0, 288.0, 128.0, C_TXT, t, 1.4, 0.05, 0.55));
    b.push_str(&txt(ctx, "求根公式已排好版", 13.0, C_FAINT, 0.8 * ramp(t, 3.0, 0.6), px + pw / 2.0, fy + 168.0, true));

    let ba = ramp(t, 3.6, 0.6);
    b.push_str(&rect(px + 34.0, py + 390.0 + lift, pw - 68.0, 46.0, 10.0, "#12202c", ba, Some((C_ACC, 1.2, 0.7 * ba))));
    b.push_str(&txt(ctx, "下一题", 17.0, C_ACC, 0.95 * ba, px + pw / 2.0, py + 421.0 + lift, true));
    for i in 0..3 {
        let da = ramp(t, 3.9 + i as f32 * 0.2, 0.5);
        b.push_str(&rect(px + 34.0, py + 462.0 + i as f32 * 58.0 + lift, pw - 68.0, 46.0, 9.0, "#0b111b", 0.9 * da, Some(("#1c283a", 1.0, da))));
        b.push_str(&rect(px + 46.0, py + 478.0 + i as f32 * 58.0 + lift, 120.0, 8.0, 4.0, "#243147", 0.9 * da, None));
    }

    // 标注
    let notes = [("无浏览器", 470.0, C_ACC), ("无 Java 运行时", 540.0, C_ACC), ("字体文件随包带", 610.0, C_SUB)];
    for (i, (s, y, c)) in notes.iter().enumerate() {
        let ga = ramp(t, 4.4 + i as f32 * 0.5, 0.55);
        let (svg, w) = pill(ctx, s, 980.0, *y, 17.0, c, "#0f1a24", 0.95 * ga, 14.0);
        b.push_str(&svg);
        b.push_str(&line(980.0 + w + 6.0, *y, px - 8.0, *y, c, 1.2, 0.35 * ga));
        b.push_str(&dot(px - 8.0, *y, 3.0, c, 0.8 * ga));
    }
    b
}

fn text_right_safe(ctx: &Ctx, s: &str, size: f32, fill: &str, a: f32, right: f32, y: f32) -> String {
    ctx.text_right(&spec(s, size, fill, a), right, y)
}

// ---------------------------------------------------------------- 四、局限

fn scene_matrix(ctx: &Ctx, art: &Art, t: f32) -> String {
    let mut b = String::new();
    let ea = ramp(t, 0.0, 0.6);
    b.push_str(&section_head(ctx, "01 / 02 · 矩阵", "复杂矩阵：括号拉伸有上限", ea));

    // 竖版矩阵：20 行 3 列
    let (cx, cy) = (520.0, 590.0);
    let max_w = 470.0;
    let max_h = 430.0;
    let k = (max_w / art.width).min(max_h / (art.height + art.depth));
    let left = cx - art.width * k / 2.0;
    let baseline = cy + (art.height - art.depth) * k / 2.0;

    // 逐行出现
    let mut g = String::from(r#"<g data-formula="1">"#);
    for (i, gl) in art.glyphs.iter().enumerate() {
        let st = 0.5 + i as f32 * 0.028;
        let pr = clamp01((t - st) / 0.35);
        if pr <= 0.0 {
            continue;
        }
        let a = clamp01(pr * 1.8);
        let px = left + gl.x * k;
        let py = baseline - gl.y * k;
        let _ = write!(
            g,
            r#"<path transform="translate({px:.2} {py:.2}) scale({:.4})" fill="{}" fill-opacity="{a:.3}" d="{}"/>"#,
            k * gl.scale,
            C_TXT,
            gl.d
        );
    }
    for r in &art.rules {
        g.push_str(&rect(left + r.x * k, baseline - r.y * k, r.w * k, (r.th * k).max(1.2), 0.4, C_TXT, 0.9, None));
    }
    g.push_str("</g>");
    b.push_str(&g);

    // 括号的实际跨度（取缩放最大的括号字形）
    let mut br = None;
    for gl in &art.glyphs {
        if (gl.ch == '(' || gl.ch == '[') && br.map(|b: (f32, f32, f32)| gl.scale > b.2).unwrap_or(true) {
            br = Some((gl.y + gl.bb.3 * gl.scale, gl.y + gl.bb.1 * gl.scale, gl.scale));
        }
    }
    let (btop, bbot) = br.map(|b| (b.0, b.1)).unwrap_or((art.height, -art.depth));
    let ytop = baseline - btop * k;
    let ybot = baseline - bbot * k;

    let ma = ramp(t, 2.0, 0.7);
    // 红色跨度线：矩阵整体
    if ma > 0.01 {
        let full_top = baseline - art.height * k;
        let full_bot = baseline + art.depth * k;
        b.push_str(&dashed(760.0, full_top, 760.0, full_bot, C_RED, 1.6, 0.55 * ma, "6 6"));
        b.push_str(&line(754.0, full_top, 766.0, full_top, C_RED, 1.6, 0.7 * ma));
        b.push_str(&line(754.0, full_bot, 766.0, full_bot, C_RED, 1.6, 0.7 * ma));
        b.push_str(&ctx.text_right(&spec("20 行", 15.0, C_RED, 0.9 * ma), 742.0, (full_top + full_bot) / 2.0 + 5.0));
    }

    // 右侧说明
    let ta = ramp(t, 0.8, 0.6);
    let items = [
        ("20 × 3 的矩阵", "212 个字形单元"),
        ("每列按最宽对齐", "行距 0.25 em，列距 0.3 em"),
    ];
    for (i, (h, s)) in items.iter().enumerate() {
        let ga = ramp(t, 0.9 + i as f32 * 0.5, 0.6);
        let y = 360.0 + i as f32 * 96.0;
        b.push_str(&txt(ctx, h, 24.0, C_TXT, 0.96 * ga, 900.0, y, false));
        b.push_str(&txt(ctx, s, 17.0, C_SUB, 0.9 * ga, 900.0, y + 30.0, false));
    }
    let _ = ta;

    // 括号上限
    let ca = ramp(t, 5.2, 0.7);
    if ca > 0.01 {
        b.push_str(&rect(930.0, 560.0, 800.0, 190.0, 12.0, "#140f14", 0.85 * ca, Some((C_RED, 1.4, 0.55 * ca))));
        b.push_str(&txt(ctx, "括号最多只能拉伸 4 倍", 26.0, C_RED, 0.98 * ca, 960.0, 612.0, false));
        b.push_str(&txt(ctx, "20 行的高度已经超出上限，", 18.0, C_DIM, 0.92 * ca, 960.0, 652.0, false));
        b.push_str(&txt(ctx, "上下两端露在括号外面。", 18.0, C_DIM, 0.92 * ca, 960.0, 682.0, false));
        b.push_str(&mono(ctx, "clamp(min_scale, 4 × min_scale)", 14.0, C_DIM, 0.85 * ca, 960.0, 718.0, false));

        // 括号实际覆盖范围高亮
        let pulse = 0.65 + 0.35 * ((t * 2.4).sin() * 0.5 + 0.5);
        b.push_str(&line(800.0, ytop, 800.0, ybot, C_ACC, 3.0, 0.9 * ca * pulse));
        b.push_str(&line(794.0, ytop, 806.0, ytop, C_ACC, 2.0, 0.8 * ca));
        b.push_str(&line(794.0, ybot, 806.0, ybot, C_ACC, 2.0, 0.8 * ca));
        b.push_str(&mono(ctx, "括号高度", 14.0, C_ACC, 0.9 * ca, 818.0, ytop - 8.0, false));
    }

    // 结论
    let fa = ramp(t, 8.0, 0.7);
    b.push_str(&txt(ctx, "常用的 2×2 到 5×5 没有问题；再大，括号就跟不上了。", 18.0, C_SUB, 0.9 * fa, 960.0, 800.0, false));

    b
}

fn scene_symbols(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let ea = ramp(t, 0.0, 0.6);
    b.push_str(&section_head(ctx, "02 / 02 · 符号", "这些命令，暂时画不出来", ea));

    let cards = [
        (r"\cancel{x}", "划掉一项", "取消线需要额外绘制元素"),
        (r"\ce{H2O}", "化学式", "化学排版是另一套规范"),
        (r"\textcolor{red}{x}", "彩色文字", "颜色需要绘制层配合"),
    ];
    let (cw, ch, gap) = (470.0, 300.0, 50.0);
    let total = 3.0 * cw + 2.0 * gap;
    let x0 = (FW - total) / 2.0;
    for (i, (cmd, name, note)) in cards.iter().enumerate() {
        let ga = ramp(t, 0.5 + i as f32 * 0.55, 0.7);
        let x = x0 + i as f32 * (cw + gap);
        let y = 340.0 - (1.0 - ease_out_cubic(ga)) * 26.0;
        let a = ga;
        b.push_str(&rect(x, y, cw, ch, 14.0, "#0d141f", 0.95 * a, Some(("#26344a", 1.2, a))));
        b.push_str(&mono(ctx, cmd, 21.0, C_TXT, 0.95 * a, x + 28.0, y + 62.0, false));
        b.push_str(&line(x + 28.0, y + 86.0, x + cw - 28.0, y + 86.0, C_LINE, 1.0, 0.9 * a));
        b.push_str(&txt(ctx, name, 22.0, C_DIM, 0.95 * a, x + 28.0, y + 132.0, false));
        b.push_str(&txt(ctx, note, 17.0, C_SUB, 0.95 * a, x + 28.0, y + 166.0, false));

        // 红色叉
        let sa = ramp(t, 1.4 + i as f32 * 0.55, 0.45);
        if sa > 0.01 {
            let s = 0.8 + 0.2 * ease_out_back(sa);
            let (cx, cy) = (x + cw - 62.0, y + 130.0);
            b.push_str(&ring(cx, cy, 26.0 * s, C_RED, 2.0, 0.75 * sa));
            b.push_str(&line(cx - 10.0 * s, cy - 10.0 * s, cx + 10.0 * s, cy + 10.0 * s, C_RED, 3.0, 0.9 * sa));
            b.push_str(&line(cx + 10.0 * s, cy - 10.0 * s, cx - 10.0 * s, cy + 10.0 * s, C_RED, 3.0, 0.9 * sa));
        }
        let pa = ramp(t, 1.9 + i as f32 * 0.55, 0.5);
        let (svg, _) = pill(ctx, "暂不支持", x + 28.0, y + 220.0, 16.0, "#ff8f8f", "#2a1219", 0.98 * pa, 13.0);
        b.push_str(&svg);
    }

    let na = ramp(t, 3.4, 0.7);
    b.push_str(&txt(ctx, "命令覆盖约 40 个：常用公式够用，冷门命令还要继续补。", 21.0, C_DIM, 0.95 * na, 960.0, 764.0, true));
    b.push_str(&txt(ctx, "这是「窄而深」的取舍，不是通用排版器。", 17.0, C_SUB, 0.9 * ramp(t, 4.2, 0.7), 960.0, 802.0, true));
    b
}

// ---------------------------------------------------------------- 五、应用

fn scene_use(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let ea = ramp(t, 0.0, 0.6);
    b.push_str(&section_head(ctx, "能拿来做什么", "轻量引擎，适合嵌进理科学习类软件", ea));

    let cards = [
        ("错题 App", "拍照搜题后，把公式重新排干净", 0u8),
        ("理科学习小程序", "课件、练习里内嵌公式，不额外加载内核", 1u8),
        ("自研教学软件", "离线题库与讲义，装机即用", 2u8),
    ];
    let (cw, ch, gap) = (500.0, 330.0, 55.0);
    let total = 3.0 * cw + 2.0 * gap;
    let x0 = (FW - total) / 2.0;
    for (i, (title, desc, kind)) in cards.iter().enumerate() {
        let ga = ramp(t, 0.5 + i as f32 * 0.6, 0.7);
        let x = x0 + i as f32 * (cw + gap);
        let y = 350.0 + (1.0 - ease_out_cubic(ga)) * 40.0;
        let a = ga;
        b.push_str(&rect(x, y, cw, ch, 14.0, "#0e1623", 0.96 * a, Some(("#33465f", 1.3, a))));

        let (ix, iy) = (x + cw / 2.0, y + 96.0);
        match kind {
            0 => {
                b.push_str(&rect(ix - 34.0, iy - 56.0, 68.0, 112.0, 12.0, "#0a111a", a, Some((C_ACC, 1.6, 0.8 * a))));
                b.push_str(&rect(ix - 22.0, iy - 34.0, 44.0, 8.0, 4.0, "#2a3a52", 0.9 * a, None));
                b.push_str(&rect(ix - 22.0, iy - 16.0, 34.0, 6.0, 3.0, "#22304a", 0.9 * a, None));
                b.push_str(&line(ix - 20.0, iy + 16.0, ix - 6.0, iy + 30.0, C_ACC, 3.0, 0.9 * a));
                b.push_str(&line(ix - 6.0, iy + 30.0, ix + 20.0, iy + 2.0, C_ACC, 3.0, 0.9 * a));
            }
            1 => {
                for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    let gx = ix + dx * 34.0 - 26.0;
                    let gy = iy + dy * 34.0 - 26.0;
                    b.push_str(&rect(gx, gy, 52.0, 52.0, 12.0, "#0a111a", a, Some((C_ACC, 1.4, 0.75 * a))));
                }
                b.push_str(&rect(ix - 12.0, iy - 12.0, 24.0, 24.0, 6.0, C_ACC, 0.5 * a, None));
            }
            _ => {
                b.push_str(&rect(ix - 58.0, iy - 44.0, 116.0, 76.0, 8.0, "#0a111a", a, Some((C_ACC, 1.6, 0.8 * a))));
                b.push_str(&line(ix - 4.0, iy + 32.0, ix - 4.0, iy + 44.0, C_ACC, 1.6, 0.7 * a));
                b.push_str(&line(ix - 22.0, iy + 44.0, ix + 14.0, iy + 44.0, C_ACC, 1.6, 0.7 * a));
                let curve = format!(
                    r#"<path d="M{:.1} {:.1} Q{:.1} {:.1} {:.1} {:.1}" fill="none" stroke="{}" stroke-width="2.4" stroke-opacity="{:.3}"/>"#,
                    ix - 40.0,
                    iy + 18.0,
                    ix,
                    iy - 50.0,
                    ix + 40.0,
                    iy + 18.0,
                    C_ACC,
                    0.85 * a
                );
                b.push_str(&curve);
            }
        }

        b.push_str(&txt(ctx, title, 24.0, C_TXT, 0.96 * a, x + cw / 2.0, y + 196.0, true));
        b.push_str(&txt(ctx, desc, 17.0, C_SUB, 0.92 * a, x + cw / 2.0, y + 238.0, true));
        b.push_str(&rect(x + cw / 2.0 - 24.0, y + 268.0, 48.0, 3.0, 1.5, C_ACC, 0.55 * a, None));
    }

    let ta = ramp(t, 2.6, 0.7);
    let items = ["可离线", "体积可控", "易嵌入"];
    let widths: Vec<f32> = items.iter().map(|s| pill_w(ctx, s, 17.0, 16.0)).collect();
    let totalw: f32 = widths.iter().sum::<f32>() + 14.0 * (items.len() - 1) as f32;
    let mut x = 960.0 - totalw / 2.0;
    for (i, s) in items.iter().enumerate() {
        let ga = ramp(t, 2.8 + i as f32 * 0.25, 0.5);
        let (svg, w) = pill(ctx, s, x, 772.0, 17.0, C_ACC, "#16303a", 0.95 * ga * ta, 16.0);
        b.push_str(&svg);
        x += w + 14.0;
    }
    b
}

// ---------------------------------------------------------------- 六、结尾

fn scene_end(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let a = ramp(t, 0.2, 0.8);
    b.push_str(&txt(ctx, "Mathengine", 92.0, C_TXT, 0.98 * a, 960.0, 380.0, true));
    b.push_str(&txt(ctx, "轻量 LaTeX 数学排版引擎", 26.0, C_DIM, 0.95 * ramp(t, 0.8, 0.7), 960.0, 436.0, true));

    let lw = 120.0 * ease_out_cubic(ramp(t, 1.2, 0.7));
    b.push_str(&rect(960.0 - lw, 468.0, lw * 2.0, 2.0, 1.0, C_ACC, 0.75 * a, None));

    let na = ramp(t, 1.6, 0.7);
    b.push_str(&ctx.text_right(&spec("姓名：", 26.0, C_DIM, 0.95 * na), 890.0, 570.0));
    let uw = 200.0 * ease_out_cubic(ramp(t, 1.9, 0.8));
    b.push_str(&line(900.0, 598.0, 900.0 + uw, 598.0, "#c2cede", 2.0, 0.9 * na));
    let blink = if (t * 1.8).sin() > -0.3 { 0.9 } else { 0.15 };
    b.push_str(&rect(900.0 + uw + 5.0, 574.0, 2.0, 26.0, 1.0, C_ACC, blink * na, None));

    b.push_str(&txt(ctx, "谢谢各位老师", 38.0, C_TXT, 0.96 * ramp(t, 2.6, 0.8), 960.0, 690.0, true));
    b.push_str(&mono(ctx, "逐帧离线生成 · 无声版", 14.0, C_FAINT, 0.8 * ramp(t, 3.4, 0.8), 960.0, 830.0, true));
    b
}

// ---------------------------------------------------------------- 公式落位

struct Place {
    left: f32,
    baseline: f32,
    k: f32,
}

fn art_fit(art: &Art, ctx: &Ctx, base: f32, max_w: f32, max_h: f32) -> f32 {
    let k = base / ctx.upem;
    k.min(max_w / art.width.max(1.0))
        .min(max_h / (art.height + art.depth).max(1.0))
}

fn place(art: &Art, ctx: &Ctx, cx: f32, cy: f32, base: f32, max_w: f32, max_h: f32) -> Place {
    let k = art_fit(art, ctx, base, max_w, max_h);
    let left = cx - art.width * k / 2.0;
    let baseline = cy + (art.height - art.depth) * k / 2.0;
    ctx.check_safe(cy - max_h / 2.0, cy + max_h / 2.0, "公式");
    Place { left, baseline, k }
}

/// 字按布局顺序一个个落下，横线随后补上。始终输出一个公式组。
#[allow(clippy::too_many_arguments)]
fn reveal_art_x(
    art: &Art,
    ctx: &Ctx,
    cx: f32,
    cy: f32,
    base_size: f32,
    max_w: f32,
    max_h: f32,
    fill: &str,
    t: f32,
    t0: f32,
    stagger: f32,
    drop: f32,
) -> String {
    let p = place(art, ctx, cx, cy, base_size, max_w, max_h);
    let n = art.glyphs.len();

    let mut g = String::from(r#"<g data-formula="1">"#);
    for (i, gl) in art.glyphs.iter().enumerate() {
        let st = t0 + i as f32 * stagger;
        let pr = clamp01((t - st) / drop);
        if pr <= 0.0 {
            continue;
        }
        let e = ease_out_back(pr);
        let a = clamp01(pr * 2.2);
        let px = p.left + gl.x * p.k;
        let py = p.baseline - gl.y * p.k - 16.0 * (1.0 - e);
        let sc = p.k * gl.scale * (1.0 + 0.05 * (1.0 - e));
        let _ = write!(
            g,
            r#"<path transform="translate({px:.2} {py:.2}) scale({sc:.5})" fill="{fill}" fill-opacity="{a:.3}" d="{}"/>"#,
            gl.d
        );
    }

    let r0 = t0 + n as f32 * stagger + 0.10;
    for (j, r) in art.rules.iter().enumerate() {
        let st = r0 + j as f32 * 0.22;
        let pr = ease_out_cubic((t - st) / 0.45);
        if pr <= 0.0 {
            continue;
        }
        let _ = write!(
            g,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{fill}" fill-opacity="{:.3}"/>"#,
            p.left + r.x * p.k,
            p.baseline - r.y * p.k,
            r.w * p.k * pr,
            (r.th * p.k).max(1.2),
            0.98
        );
    }
    g.push_str("</g>");
    g
}

// ---------------------------------------------------------------- 合成一帧

struct Reel {
    quad: Art,
    mat: Art,
    code: CodePanel,
    tests: Vec<String>,
    kb: u32,
}

fn scene_body(ctx: &Ctx, r: &Reel, idx: usize, i: usize) -> String {
    match idx {
        0 => scene_web(ctx, (i - S_WEB) as f32 / FPS),
        1 => scene_how(ctx, &r.quad, &r.code, (i - S_HOW) as f32 / FPS),
        2 => {
            let local = i - S_WIN;
            if local < WIN_A {
                scene_size(ctx, r.kb, local as f32 / FPS)
            } else if local < WIN_A + WIN_B {
                scene_tests(ctx, &r.tests, (local - WIN_A) as f32 / FPS)
            } else {
                scene_phone(ctx, &r.quad, (local - WIN_A - WIN_B) as f32 / FPS)
            }
        }
        3 => {
            let local = i - S_LIM;
            if local < LIM_A {
                scene_matrix(ctx, &r.mat, local as f32 / FPS)
            } else {
                scene_symbols(ctx, (local - LIM_A) as f32 / FPS)
            }
        }
        4 => scene_use(ctx, (i - S_USE) as f32 / FPS),
        _ => scene_end(ctx, (i - S_END) as f32 / FPS),
    }
}

fn frame(ctx: &Ctx, reel: &Reel, i: usize) -> String {
    let mut idx = 0;
    for (k, (start, _)) in SCENES.iter().enumerate() {
        if i >= *start {
            idx = k;
        }
    }

    let body = scene_body(ctx, reel, idx, i);

    // 硬保证：一帧最多一条公式
    let n_formula = body.matches(r#"data-formula="1""#).count();
    assert!(n_formula <= 1, "第 {i} 帧出现 {n_formula} 条公式，最多允许 1");
    let local = i.saturating_sub(SCENES[idx].0);
    let want_one = (idx == 1)
        || (idx == 2 && local >= WIN_A + WIN_B)
        || (idx == 3 && local < LIM_A);
    if want_one {
        assert_eq!(n_formula, 1, "第 {i} 帧（场景 {idx}）应有且只有一条公式");
    } else {
        assert_eq!(n_formula, 0, "第 {i} 帧（场景 {idx}）不该出现公式");
    }
    // 硬保证：一帧最多一套字幕（本片不烧字幕，留空给后期）
    let n_cap = body.matches(r#"data-cap="1""#).count();
    assert!(n_cap <= 1, "第 {i} 帧出现 {n_cap} 套字幕，最多允许 1");

    // 硬保证：不出现「实时」类措辞
    for w in BANNED {
        assert!(!body.contains(w), "第 {i} 帧出现禁用词「{w}」");
    }

    let prog = i as f32 / (TOTAL - 1) as f32;
    let mut svg = String::with_capacity(24 * 1024);
    let _ = write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{FW:.0}" height="{FH:.0}" viewBox="0 0 {FW:.0} {FH:.0}">"#
    );
    svg.push_str(&background(prog, SCENES[idx].1, ctx));
    svg.push_str(&body);
    // 收尾 0.8 秒压黑
    let fade = 1.0 - smooth((i as f32 - (TOTAL as f32 - 24.0)) / 23.0);
    if fade < 0.999 {
        let _ = write!(svg, r##"<rect width="{FW:.0}" height="{FH:.0}" fill="#000000" fill-opacity="{:.3}"/>"##, 1.0 - fade);
    }
    svg.push_str("</svg>");
    svg
}

// ---------------------------------------------------------------- 入口

fn load_fonts() -> (MathFont<'static>, Ctx) {
    let math_data: &'static [u8] =
        Box::leak(load_latin_modern_math(mathengine::LM_MATH_OTF).expect("读 assets/latinmodern-math.otf").into_boxed_slice());
    let font = MathFont::from_bytes(math_data).expect("解析字体");

    let noto: &'static [u8] = Box::leak(
        fs::read("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc")
            .expect("Noto Sans CJK Regular")
            .into_boxed_slice(),
    );
    let noto_bold: &'static [u8] = Box::leak(
        fs::read("/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc")
            .expect("Noto Sans CJK Bold")
            .into_boxed_slice(),
    );
    let mono_dir = "/usr/share/fonts/TTF/JetBrains-Maple-Mono-NF-XX-XX";
    let jbm: &'static [u8] = Box::leak(
        fs::read(format!("{mono_dir}/JetBrainsMapleMono-Regular.ttf"))
            .expect("JetBrains Maple Mono")
            .into_boxed_slice(),
    );
    let jbm_bold: &'static [u8] = Box::leak(
        fs::read(format!("{mono_dir}/JetBrainsMapleMono-Bold.ttf"))
            .expect("JetBrains Maple Mono Bold")
            .into_boxed_slice(),
    );

    let ctx = Ctx {
        upem: font.units_per_em(),
        reg: Metrics::load(noto, "Noto Sans CJK SC"),
        bold: Metrics::load(noto_bold, "Noto Sans CJK SC"),
        mono: Metrics::load(jbm, "JetBrains Maple Mono"),
        mono_bold: Metrics::load(jbm_bold, "JetBrains Maple Mono"),
        warn: std::cell::RefCell::new(Vec::new()),
        bad: std::cell::RefCell::new(Vec::new()),
    };
    (font, ctx)
}

/// 从源码里取真实代码行：片段必须逐字出现在 src/layout.rs 中。
fn read_code_panel(ctx: &Ctx, path: &str, marker: &str, skip: usize, count: usize) -> CodePanel {
    let src = fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {path}: {e}"));
    let lines: Vec<&str> = src.lines().collect();
    let head = lines
        .iter()
        .position(|l| l.contains(marker))
        .unwrap_or_else(|| panic!("在 {path} 里找不到代码起点：{marker}"));
    let start = head + skip;
    assert!(start + count <= lines.len(), "代码片段越界");
    let code: Vec<String> = lines[start..start + count].iter().map(|s| s.to_string()).collect();
    let snippet = code.join("\n");
    assert!(src.contains(&snippet), "代码片段与 src/layout.rs 不一致（字符串校验失败）");
    assert!(!snippet.contains("实时"), "代码片段出现禁用词");

    // 按最长一行反推字号，保证不撑破面板
    let mut w1 = 0.0f32;
    for l in &code {
        w1 = w1.max(ctx.tw(&spec(l, 1.0, C_TXT, 1.0)));
    }
    let avail = 780.0;
    let size = (avail / w1.max(0.001)).min(14.0).max(9.0);
    let y0 = 355.0;
    let lh = (292.0 + 566.0 - 14.0 - y0) / count as f32;
    CodePanel { lines: code, start_no: start + 1, size, lh, x: 160.0, y0 }
}

/// 从源码里收集 `#[test]` 后面的函数名，按文件顺序。
fn read_test_names(repo: &str) -> Vec<String> {
    let files = ["src/ast.rs", "src/ffi.rs", "src/font.rs", "src/layout.rs", "src/token.rs", "tests/public_api.rs", "tests/real_world.rs", "tests/robustness.rs", "tests/symbols.rs"];
    let mut out = Vec::new();
    for f in files {
        let p = format!("{repo}/{f}");
        let src = fs::read_to_string(&p).unwrap_or_else(|e| panic!("读 {p}: {e}"));
        let lines: Vec<&str> = src.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if !l.contains("#[test]") {
                continue;
            }
            for l2 in lines.iter().skip(i).take(4) {
                if let Some(rest) = l2.trim().strip_prefix("fn ") {
                    let name: String = rest.chars().take_while(|c| *c != '(').collect();
                    if !name.is_empty() {
                        out.push(name);
                        break;
                    }
                }
            }
        }
    }
    out
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/mgframes".to_string());
    fs::create_dir_all(&dir).expect("建输出目录");
    let repo = env!("CARGO_MANIFEST_DIR");

    let (font, ctx) = load_fonts();

    let quad = build_art(&font, r"\frac{-b\pm\sqrt{b^{2}-4ac}}{2a}").expect("求根公式");
    println!("原理公式：{} 个符号单元，盒 {:.0}×{:.0}，深 {:.0}", quad.glyphs.len(), quad.width, quad.height, quad.depth);

    let rows: Vec<String> = (0..20)
        .map(|r| (0..3).map(|c| format!("a_{{{r}{c}}}")).collect::<Vec<_>>().join(" & "))
        .collect();
    let latex = format!(r"\begin{{pmatrix}}{}\end{{pmatrix}}", rows.join(r" \\ "));
    let mat = build_art(&font, &latex).expect("20×3 矩阵");
    println!("矩阵：{} 个字形单元", mat.glyphs.len());

    let code = read_code_panel(&ctx, &format!("{repo}/src/layout.rs"), "fn layout_stacked(", 7, 27);
    println!("代码片段：{}–{} 行，{} 行", code.start_no, code.start_no + code.lines.len() - 1, code.lines.len());

    let tests = read_test_names(repo);
    assert_eq!(tests.len(), 84, "测试用例数应为 84，实际 {}", tests.len());
    println!("测试用例：{} 条", tests.len());

    // 源码行数：与 README / 论文里的「2900 余行」对齐（含测试模块，按物理行计）
    let mut src_lines = 0usize;
    let mut src_files: Vec<_> = fs::read_dir(format!("{repo}/src")).expect("src 目录").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map(|x| x == "rs").unwrap_or(false)).collect();
    src_files.sort();
    for f in &src_files {
        src_lines += fs::read_to_string(f).unwrap_or_default().lines().count();
    }
    println!("src/ 共 {src_lines} 行（{} 个文件）", src_files.len());
    assert!(src_lines >= 2900, "源码行数不到 2900 行，画面上不能写「2900 余行」");

    let so = format!("{repo}/target/release/libmathengine.so");
    let bytes = fs::metadata(&so).unwrap_or_else(|e| panic!("{so}: {e}")).len();
    let kb = (bytes as f64 / 1000.0).round() as u32;
    println!("动态库：{bytes} 字节 = {kb} KB");
    assert!((400..=700).contains(&kb), "动态库体积异常：{kb} KB");

    let reel = Reel { quad, mat, code, tests, kb };

    for i in 0..TOTAL {
        let svg = frame(&ctx, &reel, i);
        fs::write(format!("{dir}/f{i:05}.svg"), svg).expect("写 SVG");
        if i % 600 == 0 {
            println!("  帧 {i}/{TOTAL}");
        }
    }
    println!("共 {TOTAL} 帧（{:.1} s）→ {dir}", TOTAL as f32 / FPS);

    println!("分段时间：");
    for (k, (start, name)) in SCENES.iter().enumerate() {
        let end = if k + 1 < SCENES.len() { SCENES[k + 1].0 } else { TOTAL };
        println!("  {name:<6} {:>6.2} – {:>6.2} s  (帧 {start}–{})", *start as f32 / FPS, end as f32 / FPS, end - 1);
    }

    let warn = ctx.warn.borrow();
    if !warn.is_empty() {
        println!("检查提示：");
        for w in warn.iter() {
            println!("  {w}");
        }
    }
    let bad = ctx.bad.borrow();
    if !bad.is_empty() {
        panic!("字幕安全区被占用（{} 处）：\n{}", bad.len(), bad.join("\n"));
    }
}
