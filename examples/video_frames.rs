//! `cargo run --release --example video_frames -- 输出目录`：生成演示视频的逐帧 SVG。
//!
//! 每一帧都是一张 1920×1080 的完整 SVG：公式部分不是截图，而是调用
//! `mathengine::layout` 算布局、再向字体索取字形轮廓画出来的路径——和
//! `examples/render_svg.rs` 走的是同一份数据。中文与说明文字用 SVG `<text>`
//! 交给 rsvg / Pango 排版，避免 ffmpeg drawtext 的字体路径问题。
//!
//! 全片离线逐帧预渲染：没有「即时出图」的承诺，换场一律硬切，一帧只画一条公式。
//!
//! 宽度测量直接读字体 `hmtx`：中文用 Noto Sans CJK SC，拉丁用 JetBrains Maple
//! Mono，这样居中与分段文字的落点不靠猜。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::{layout, Layout};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

const FW: f32 = 1920.0;
const FH: f32 = 1080.0;
const FPS: f32 = 30.0;

// 配色：深底 + 青绿强调，公式用近白。全片只用这几档，不加杂色。
const C_TXT: &str = "#eef2f9";
const C_DIM: &str = "#94a2b8";
const C_FAINT: &str = "#5f6d83";
/// 脚注：比 C_FAINT 亮一档，投影与压缩后仍读得出。
const C_FOOT: &str = "#7b8aa3";
/// 卡片内小字：比 C_FAINT 亮一档，投影也看得清。
const C_SUB: &str = "#7d8ba2";
const C_LINE: &str = "#1d2939";
const C_PANEL: &str = "#0f1622";
const C_ACC: &str = "#2ee6c5";
const C_WARN: &str = "#ff7a6b";
const C_AMBER: &str = "#f7b955";

const SANS: &str = "Noto Sans CJK SC";
const MONO: &str = "JetBrains Maple Mono";

// 场景切分（帧，30 fps）。S_ 为场景起始，TOTAL 为总帧数。
const S_COVER: usize = 0;
const S_PROBLEM: usize = 130;
const S_ARCH: usize = 375;
const S_ASSEMBLE: usize = 615;
const S_INSTRUCT: usize = 930;
const S_GALLERY: usize = 1185;
const S_CODE: usize = 1545;
const S_CHART: usize = 1770;
const S_END: usize = 2010;
const TOTAL: usize = 2175; // 72.5 s

const SCENES: [(usize, &str); 9] = [
    (S_COVER, "封面"),
    (S_PROBLEM, "问题"),
    (S_ARCH, "架构"),
    (S_ASSEMBLE, "字形组装"),
    (S_INSTRUCT, "指令结构"),
    (S_GALLERY, "公式演示"),
    (S_CODE, "实现"),
    (S_CHART, "实测"),
    (S_END, "仓库"),
];

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

struct PathBuilder {
    d: String,
}

impl OutlineBuilder for PathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "M{x:.1} {:.1} ", -y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "L{x:.1} {:.1} ", -y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.d, "Q{x1:.1} {:.1} {x:.1} {:.1} ", -y1, -y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(self.d, "C{x1:.1} {:.1} {x2:.1} {:.1} {x:.1} {:.1} ", -y1, -y2, -y);
    }
    fn close(&mut self) {
        self.d.push_str("Z ");
    }
}

/// 一个已放置字形的绘制数据：轮廓 path 用字体单位，落点用布局坐标（y 向上）。
struct GlyphArt {
    d: String,
    x: f32,
    y: f32,
    scale: f32,
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
}

fn build_art(font: &MathFont, cache: &mut HashMap<u16, String>, latex: &str) -> Result<Art, String> {
    let ast = parse(latex).map_err(|e| format!("{e:?}"))?;
    let l = layout(font, &ast);
    let face = font.face_ref().ok_or_else(|| "字体没有 face".to_string())?;
    if l.glyphs.is_empty() {
        return Err("布局结果为空".to_string());
    }

    let mut glyphs = Vec::with_capacity(l.glyphs.len());
    for g in &l.glyphs {
        let gid = face
            .glyph_index(g.ch)
            .ok_or_else(|| format!("字体缺字 {:?}", g.ch))?;
        if !cache.contains_key(&gid.0) {
            let mut pb = PathBuilder { d: String::new() };
            if face.outline_glyph(GlyphId(gid.0), &mut pb).is_none() || pb.d.is_empty() {
                return Err(format!("字形无轮廓 {:?}", g.ch));
            }
            cache.insert(gid.0, pb.d);
        }
        glyphs.push(GlyphArt {
            d: cache[&gid.0].clone(),
            x: g.x,
            y: g.y,
            scale: g.scale,
        });
    }
    let rules = l
        .rules
        .iter()
        .map(|r| RuleArt { x: r.x, y: r.y, w: r.width, th: r.thickness.max(1.0) })
        .collect();

    Ok(Art { glyphs, rules, width: l.width, height: l.height, depth: l.depth })
}

// ---------------------------------------------------------------- 上下文

struct Ctx {
    upem: f32,
    reg: Metrics,
    bold: Metrics,
    mono: Metrics,
    mono_bold: Metrics,
    warn: std::cell::RefCell<Vec<String>>,
}

struct Spec<'a> {
    s: &'a str,
    size: f32,
    fill: &'a str,
    alpha: f32,
    mono: bool,
    bold: bool,
    ls: f32,
    /// 代码/指令面板：保留前导空格（SVG 默认会折叠）。
    pre: bool,
}

fn spec<'a>(s: &'a str, size: f32, fill: &'a str, alpha: f32) -> Spec<'a> {
    Spec { s, size, fill, alpha, mono: false, bold: false, ls: 0.0, pre: false }
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

    /// 文字宽度（含字距）。
    fn tw(&self, sp: &Spec) -> f32 {
        let mut w = self
            .metrics(sp.mono, sp.bold)
            .advance(sp.s, sp.size, &mut self.warn.borrow_mut());
        if sp.ls != 0.0 {
            w += sp.ls * sp.s.chars().count() as f32;
        }
        w
    }

    /// 右对齐，返回 (svg, 实际左边界)。
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
        if sp.pre {
            attrs.push_str(r#" xml:space="preserve""#);
        }
        if sp.ls != 0.0 {
            let _ = write!(attrs, r#" letter-spacing="{:.1}""#, sp.ls);
        }
        // 居中时把字距的尾随量折掉，避免整行右偏
        let x = if center && sp.ls != 0.0 { x - sp.ls * 0.5 } else { x };
        let w = self.tw(sp);
        let (l, r) = if center { (x - w / 2.0, x + w / 2.0) } else { (x, x + w) };
        if l < 16.0 || r > 1904.0 {
            let msg = format!("超出安全区 x[{l:.0},{r:.0}]  {}", sp.s);
            let mut wr = self.warn.borrow_mut();
            if !wr.contains(&msg) {
                wr.push(msg);
            }
        }
        format!(
            r#"<text x="{x:.1}" y="{y:.1}" font-family="{fam}" font-size="{:.1}" fill="{}" fill-opacity="{:.3}"{attrs}>{}</text>"#,
            sp.size,
            sp.fill,
            sp.alpha,
            esc(sp.s)
        )
    }
}

/// 普通文字（不保留空格）。
fn txt(ctx: &Ctx, s: &str, size: f32, fill: &str, a: f32, x: f32, y: f32, center: bool) -> String {
    ctx.text_at(&spec(s, size, fill, a), x, y, center)
}

/// 等宽文字。
fn mono(ctx: &Ctx, s: &str, size: f32, fill: &str, a: f32, x: f32, y: f32, center: bool) -> String {
    let sp = Spec { s, size, fill, alpha: a, mono: true, bold: false, ls: 0.0, pre: false };
    ctx.text_at(&sp, x, y, center)
}

/// 等宽且保留前导空格（代码与指令树）。
fn mono_pre(ctx: &Ctx, s: &str, size: f32, fill: &str, a: f32, x: f32, y: f32) -> String {
    let sp = Spec { s, size, fill, alpha: a, mono: true, bold: false, ls: 0.0, pre: true };
    ctx.text_at(&sp, x, y, false)
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

/// 落位用：先越过目标一点再回弹，像字块被放上去。
fn ease_out_back(x: f32) -> f32 {
    let p = clamp01(x);
    let c1 = 1.35;
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
        Some((c, sw, sa)) => format!(
            r#" stroke="{c}" stroke-width="{sw:.1}" stroke-opacity="{sa:.3}""#
        ),
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

fn dot(cx: f32, cy: f32, r: f32, c: &str, a: f32) -> String {
    if a <= 0.004 {
        return String::new();
    }
    format!(
        r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{c}" fill-opacity="{a:.3}"/>"#
    )
}

/// 把字幕包成一组，便于断言「一帧最多一套字幕」。
fn cap_group(inner: String) -> String {
    if inner.is_empty() {
        String::new()
    } else {
        format!(r#"<g data-cap="1">{inner}</g>"#)
    }
}

// ---------------------------------------------------------------- 背景与外框

fn background(progress: f32, section: &str, ctx: &Ctx) -> String {
    let mut b = String::new();
    b.push_str(&format!(
        r##"<defs>
<linearGradient id="bg" x1="0" y1="0" x2="0.35" y2="1">
<stop offset="0" stop-color="#0c1420"/>
<stop offset="0.55" stop-color="#090d15"/>
<stop offset="1" stop-color="#06080d"/>
</linearGradient>
<linearGradient id="glow" x1="0" y1="0" x2="1" y2="1">
<stop offset="0" stop-color="{C_ACC}" stop-opacity="0.10"/>
<stop offset="0.6" stop-color="{C_ACC}" stop-opacity="0.00"/>
</linearGradient>
</defs>
<rect width="{FW}" height="{FH}" fill="url(#bg)"/>
<rect x="0" y="0" width="{FW}" height="620" fill="url(#glow)"/>
"##
    ));

    // 静态网格：一排细线，给画面一点工程感，同时几乎不增加码率
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
    let sec = Spec { s: section, size: 20.0, fill: C_SUB, alpha: 0.95, mono: false, bold: false, ls: 3.0, pre: false };
    b.push_str(&ctx.text_at(&sec, 1758.0, 74.0, true));

    // 页脚进度条 + 分段刻度
    let bx = 160.0;
    let bw = 1600.0;
    b.push_str(&rect(bx, 1030.0, bw, 2.0, 1.0, "#1b2434", 1.0, None));
    b.push_str(&rect(bx, 1030.0, bw * clamp01(progress), 2.0, 1.0, C_ACC, 0.75, None));
    for (f, _) in SCENES.iter().skip(1) {
        let tx = bx + bw * (*f as f32 / (TOTAL - 1) as f32);
        b.push_str(&rect(tx - 0.5, 1026.0, 1.0, 10.0, 0.5, C_DIM, 0.28, None));
    }
    b
}

// ---------------------------------------------------------------- 场景公共件

fn scene_head(ctx: &Ctx, b: &mut String, overline: &str, title: &str, a: f32) {
    let over = Spec { s: overline, size: 22.0, fill: C_ACC, alpha: a, mono: false, bold: false, ls: 8.0, pre: false };
    b.push_str(&ctx.text_at(&over, 200.0, 214.0, false));
    let ttl = Spec { s: title, size: 52.0, fill: C_TXT, alpha: a, mono: false, bold: true, ls: 0.0, pre: false };
    b.push_str(&ctx.text_at(&ttl, 200.0, 282.0, false));
    b.push_str(&rect(170.0, 240.0, 5.0, 50.0, 2.5, C_ACC, a, None));
}

/// 页脚小字：第三档层次，固定贴在进度条上方。
fn footnote(ctx: &Ctx, s: &str, a: f32) -> String {
    if a <= 0.004 {
        return String::new();
    }
    let sp = Spec { s, size: 18.0, fill: C_FOOT, alpha: a, mono: false, bold: false, ls: 0.6, pre: false };
    ctx.text_at(&sp, 200.0, 978.0, false)
}

// ---------------------------------------------------------------- 场景一：封面

fn scene_cover(ctx: &Ctx, art: &Art, t: f32) -> String {
    let mut b = String::new();

    let over = Spec { s: "RUST 从零实现 · 数学模式排版", size: 24.0, fill: C_ACC, alpha: ramp(t, 0.10, 0.60), mono: false, bold: false, ls: 8.0, pre: false };
    b.push_str(&ctx.text_at(&over, 960.0, 300.0, true));

    let title = Spec { s: "mathengine", size: 152.0, fill: C_TXT, alpha: ramp(t, 0.28, 0.75), mono: true, bold: true, ls: 0.0, pre: false };
    let dy = 26.0 * (1.0 - ramp(t, 0.28, 0.85));
    b.push_str(&ctx.text_at(&title, 960.0, 470.0 + dy, true));

    b.push_str(&txt(ctx, "面向移动设备的轻量 LaTeX 排版引擎", 48.0, C_DIM, ramp(t, 0.85, 0.7), 960.0, 560.0 + dy * 0.5, true));

    // 分隔线由中心向两侧展开
    let grow = ramp(t, 1.15, 0.8);
    if grow > 0.01 {
        let w = 300.0 * grow;
        b.push_str(&rect(960.0 - w / 2.0, 612.0, w, 2.0, 1.0, C_ACC, 0.85, None));
    }

    let contest = Spec { s: "第 38 届苏州市青少年科技创新大赛", size: 28.0, fill: C_DIM, alpha: ramp(t, 1.55, 0.7), mono: false, bold: false, ls: 3.0, pre: false };
    b.push_str(&ctx.text_at(&contest, 960.0, 682.0, true));

    let deco = ramp(t, 1.95, 0.9);
    b.push_str(&draw_art(ctx, art, 960.0, 858.0, 96.0, 1500.0, 200.0, C_TXT, 0.46 * deco));

    b.push_str(&footnote(ctx, "全片离线逐帧预渲染：每一帧都是一张独立绘制的 SVG，再由 ffmpeg 编码", ramp(t, 2.8, 0.8)));
    b
}

// ---------------------------------------------------------------- 场景二：问题

enum Mark {
    Cross,
    Check,
}

fn mark_icon(cx: f32, cy: f32, m: &Mark, a: f32) -> String {
    let mut b = String::new();
    let (c, r) = match m {
        Mark::Cross => (C_WARN, 21.0),
        Mark::Check => (C_ACC, 21.0),
    };
    b.push_str(&format!(
        r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="{c}" stroke-width="2.5" stroke-opacity="{:.3}"/>"#,
        0.85 * a
    ));
    match m {
        Mark::Cross => {
            b.push_str(&line(cx - 8.0, cy - 8.0, cx + 8.0, cy + 8.0, c, 2.5, 0.85 * a));
            b.push_str(&line(cx - 8.0, cy + 8.0, cx + 8.0, cy - 8.0, c, 2.5, 0.85 * a));
        }
        Mark::Check => {
            b.push_str(&format!(
                r#"<path d="M{:.1} {:.1} L{:.1} {:.1} L{:.1} {:.1}" fill="none" stroke="{c}" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round" stroke-opacity="{:.3}"/>"#,
                cx - 8.0,
                cy + 1.0,
                cx - 2.5,
                cy + 7.0,
                cx + 8.5,
                cy - 7.0,
                0.95 * a
            ));
        }
    }
    b
}

fn problem_row(
    ctx: &Ctx,
    y: f32,
    segs: &[(&str, bool, bool, &str)],
    mark: Mark,
    t: f32,
    delay: f32,
) -> String {
    let a = ramp(t, delay, 0.45);
    if a <= 0.004 {
        return String::new();
    }
    let mut b = String::new();
    let dx = 26.0 * (1.0 - ramp(t, delay, 0.55));
    b.push_str(&rect(200.0, y, 1520.0, 104.0, 14.0, C_PANEL, 0.92 * a, Some((C_LINE, 1.5, a))));
    b.push_str(&mark_icon(268.0, y + 52.0, &mark, a));

    let mut px = 336.0 + dx;
    for (s, m, bold, fill) in segs {
        let sp = Spec { s, size: 36.0, fill, alpha: a, mono: *m, bold: *bold, ls: 0.0, pre: false };
        b.push_str(&ctx.text_at(&sp, px, y + 66.0, false));
        px += ctx.tw(&sp) + 18.0;
    }
    b
}

fn scene_problem(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "问题", "为什么还要再造一个排版引擎", 1.0);

    b.push_str(&problem_row(
        ctx,
        360.0,
        &[("jlatexmath", false, true, C_TXT), ("GPL-2.0，许可传染 + 依赖 JVM", false, false, C_DIM)],
        Mark::Cross,
        t,
        0.45,
    ));
    b.push_str(&problem_row(
        ctx,
        488.0,
        &[("KaTeX / MathJax", false, true, C_TXT), ("需要 WebView 或 JS 运行时", false, false, C_DIM)],
        Mark::Cross,
        t,
        1.25,
    ));
    b.push_str(&problem_row(
        ctx,
        616.0,
        &[("mathengine", true, true, C_ACC), ("无 JVM、无 WebView、Apache-2.0", false, false, C_DIM)],
        Mark::Check,
        t,
        2.05,
    ));

    b.push_str(&txt(
        ctx,
        "两条现成的路在手表与低端 Android 上都走不通：要么整个应用被 GPL 传染，要么必须背一个 WebView。",
        27.0,
        C_DIM,
        ramp(t, 3.05, 0.6),
        200.0,
        786.0,
        false,
    ));
    b.push_str(&txt(
        ctx,
        "我们要的是：不依赖任何运行时、能把布局数据直接交给宿主的数学排版器。",
        27.0,
        C_DIM,
        ramp(t, 3.75, 0.6),
        200.0,
        838.0,
        false,
    ));
    b.push_str(&txt(
        ctx,
        "代价也写清楚：命令覆盖约 40 个，远窄于 jlatexmath 的数百个——窄而深，不做「全面支持」。",
        24.0,
        C_SUB,
        ramp(t, 4.55, 0.6),
        200.0,
        890.0,
        false,
    ));

    b.push_str(&footnote(ctx, "对照实现：jlatexmath 1.0.7（Maven Central）· KaTeX 0.19.1 · 许可与依赖均以各自仓库为准", ramp(t, 5.2, 0.7)));
    b
}

// ---------------------------------------------------------------- 场景三：架构

fn scene_arch(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "架构", "从 LaTeX 串到绘制指令", 1.0);

    let steps: [(&str, bool); 5] =
        [("LaTeX", true), ("token", true), ("AST", true), ("TeX Box 布局", false), ("扁平绘制指令", false)];
    let fs = 34.0;
    let pad = 34.0;
    let gap = 62.0;
    let boxh = 86.0;
    let y0 = 452.0;

    let widths: Vec<f32> = steps
        .iter()
        .map(|(s, m)| {
            let sp = Spec { s, size: fs, fill: C_TXT, alpha: 1.0, mono: *m, bold: false, ls: 0.0, pre: false };
            ctx.tw(&sp) + pad * 2.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + gap * (steps.len() - 1) as f32;
    let mut x = (FW - total) / 2.0;

    for (i, (s, m)) in steps.iter().enumerate() {
        let a = ramp(t, 0.45 + 0.52 * i as f32, 0.42);
        let w = widths[i];
        if i > 0 {
            let aa = ramp(t, 0.45 + 0.52 * i as f32 - 0.24, 0.30);
            let ax0 = x - gap + 12.0;
            let ax1 = x - 16.0;
            let cy = y0 + boxh / 2.0;
            b.push_str(&line(ax0, cy, ax1 - 8.0, cy, "#46587a", 2.0, 0.9 * aa));
            b.push_str(&format!(
                r##"<path d="M{:.1} {:.1} L{:.1} {:.1} L{:.1} {:.1} Z" fill="#46587a" fill-opacity="{:.3}"/>"##,
                ax1,
                cy,
                ax1 - 11.0,
                cy - 6.0,
                ax1 - 11.0,
                cy + 6.0,
                aa
            ));
        }
        if a > 0.004 {
            let dy = 18.0 * (1.0 - ramp(t, 0.45 + 0.52 * i as f32, 0.5));
            b.push_str(&rect(x, y0 + dy, w, boxh, 12.0, C_PANEL, 0.95 * a, Some(("#28374f", 1.5, a))));
            let numtxt = format!("{:02}", i + 1);
            b.push_str(&mono(ctx, &numtxt, 18.0, C_FAINT, 0.85 * a, x + w / 2.0, y0 + dy - 16.0, true));
            let label = Spec { s, size: fs, fill: C_TXT, alpha: a, mono: *m, bold: false, ls: 0.0, pre: false };
            b.push_str(&ctx.text_at(&label, x + w / 2.0, y0 + dy + 56.0, true));
        }
        x += w + gap;
    }

    b.push_str(&txt(ctx, "2900 行 Rust · 84 项测试 · 只依赖 ttf-parser", 30.0, C_DIM, ramp(t, 3.30, 0.6), 200.0, 676.0, false));
    b.push_str(&txt(
        ctx,
        "输出：字形码位 / 坐标 / 缩放 + 分数线与根号横线矩形 —— 宿主不必理解 TeX。",
        27.0,
        C_DIM,
        ramp(t, 4.05, 0.6),
        200.0,
        734.0,
        false,
    ));
    b.push_str(&txt(
        ctx,
        "同一份布局数据，可编成 .so 嵌进 Android，也可编成 JNI 库供 Java 调用。",
        27.0,
        C_DIM,
        ramp(t, 4.80, 0.6),
        200.0,
        792.0,
        false,
    ));
    b.push_str(&footnote(ctx, "布局不做任何光栅化：位图或矢量由宿主自己决定，引擎只交数字", ramp(t, 5.6, 0.7)));
    b
}

// ---------------------------------------------------------------- 场景四：字形组装

/// 组装用的公式：一条里同时有分式、根号与上下标。
const ASSEMBLE_LATEX: &str = r"\frac{-b+\sqrt{b^2-4ac}}{2a}";

fn art_fit(art: &Art, ctx: &Ctx, base: f32, max_w: f32, max_h: f32) -> f32 {
    let mut size = base;
    if art.width > 0.0 {
        size = size.min(max_w * ctx.upem / art.width);
    }
    let hd = art.height + art.depth;
    if hd > 0.0 {
        size = size.min(max_h * ctx.upem / hd);
    }
    size / ctx.upem
}

/// 组装段：字形按布局给出的顺序逐个落位，最后补分数线与根号横线。
fn scene_assemble(ctx: &Ctx, art: &Art, t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "逐帧渲染", "一条公式的组装过程", 1.0);

    let tag = Spec {
        s: "layout(&font, &parse(latex))",
        size: 21.0,
        fill: C_SUB,
        alpha: 0.85,
        mono: true,
        bold: false,
        ls: 0.0,
        pre: false,
    };
    b.push_str(&ctx.text_right(&tag, 1758.0, 214.0));

    let cx = 960.0;
    let cy = 520.0;
    let max_w = 1420.0;
    let max_h = 320.0;
    let k = art_fit(art, ctx, 150.0, max_w, max_h);
    let left = cx - art.width * k / 2.0;
    let hd = (art.height + art.depth) * k;
    let baseline = cy - hd / 2.0 + art.height * k;

    let n = art.glyphs.len();
    let stagger = 0.26;
    let drop = 0.42;
    let t0 = 0.75;

    let mut g = String::new();
    g.push_str(r#"<g data-formula="1">"#);
    for (i, gl) in art.glyphs.iter().enumerate() {
        let st = t0 + i as f32 * stagger;
        let p = clamp01((t - st) / drop);
        if p <= 0.0 {
            continue;
        }
        let e = ease_out_back(p);
        let a = clamp01(p * 2.2);
        let px = left + gl.x * k;
        let py = baseline - gl.y * k - 90.0 * (1.0 - e);
        let sc = k * gl.scale * (1.0 + 0.06 * (1.0 - e));
        let _ = write!(
            g,
            r#"<path transform="translate({px:.2} {py:.2}) scale({sc:.5})" fill="{C_TXT}" fill-opacity="{a:.3}" d="{}"/>"#,
            gl.d
        );
        // 落位瞬间点一颗小亮点，给出逐个落下的节奏
        let land = st + drop;
        if t > land {
            let da = (1.0 - (t - land) / 0.55).clamp(0.0, 1.0) * 0.85;
            g.push_str(&dot(px + 4.0, baseline - gl.y * k, 2.2, C_ACC, da));
        }
    }

    let r0 = t0 + n as f32 * stagger + 0.10;
    for (j, r) in art.rules.iter().enumerate() {
        let st = r0 + j as f32 * 0.30;
        let p = ease_out_cubic((t - st) / 0.55);
        if p <= 0.0 {
            continue;
        }
        let _ = write!(
            g,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{C_TXT}" fill-opacity="{:.3}"/>"#,
            left + r.x * k,
            baseline - r.y * k,
            r.w * k * p,
            (r.th * k).max(1.4),
            0.98
        );
    }
    g.push_str("</g>");
    b.push_str(&g);

    // 落位计数：真的按时间数出来的，不是写死的
    let landed = art
        .glyphs
        .iter()
        .enumerate()
        .filter(|(i, _)| t >= t0 + *i as f32 * stagger + drop)
        .count();
    let cnt = format!("字形 {landed:02} / {n:02}");
    b.push_str(&mono(ctx, &cnt, 20.0, C_FAINT, 0.9, 200.0, 700.0, false));
    let drawn_rules = art
        .rules
        .iter()
        .enumerate()
        // 起笔就算「已画出」，与画面同步；不要等整条横线铺满才加一
        .filter(|(j, _)| t >= r0 + *j as f32 * 0.30)
        .count();
    let rc = format!("横线 {drawn_rules:02} / {:02}", art.rules.len());
    let rsp = Spec { s: &rc, size: 20.0, fill: C_FAINT, alpha: 0.9, mono: true, bold: false, ls: 0.0, pre: false };
    b.push_str(&ctx.text_right(&rsp, 1758.0, 700.0));

    // 四个指标块：字形的数、横线的数、宽、高+深（全部来自 layout 输出）
    let a = ramp(t, r0 + 0.9, 0.6);
    let cw = 280.0;
    let cgap = 20.0;
    let x0 = (FW - (4.0 * cw + 3.0 * cgap)) / 2.0;
    let cy0 = 742.0;
    let vals = [
        (format!("{n}"), "字形 glyphs"),
        (format!("{}", art.rules.len()), "横线 rules"),
        (format!("{:.0}", art.width), "宽 width"),
        (format!("{:.0}+{:.0}", art.height, art.depth), "高 height + 深 depth"),
    ];
    for (i, (v, lb)) in vals.iter().enumerate() {
        let x = x0 + i as f32 * (cw + cgap);
        b.push_str(&rect(x, cy0, cw, 84.0, 12.0, C_PANEL, 0.95 * a, Some((C_LINE, 1.4, a))));
        b.push_str(&mono(ctx, v, 34.0, C_ACC, a, x + cw / 2.0, cy0 + 44.0, true));
        b.push_str(&txt(ctx, lb, 17.0, C_SUB, 0.95 * a, x + cw / 2.0, cy0 + 70.0, true));
    }

    // 组装完成后一条细线从左扫到右：顺序就是 layout 给出的顺序
    let sp = ease_out_cubic((t - 6.55) / 1.15);
    let sfade = 1.0 - ramp(t, 8.05, 0.7);
    if sp > 0.0 && sfade > 0.004 {
        let y = baseline + art.depth * k + 34.0;
        b.push_str(&rect(left, y, art.width * k * sp, 2.0, 1.0, C_ACC, 0.55 * sfade, None));
    }

    let cap = txt(ctx, "分式、根式、上标在同一条公式里：每个字形的位置都是布局算出来的", 25.0, C_DIM, ramp(t, 6.4, 0.6), 960.0, 888.0, true);
    b.push_str(&cap_group(cap));
    b.push_str(&footnote(ctx, "逐帧离线渲染 · 非即时播放 · 落点与缩放取 layout() 的原样输出（字体单位）", ramp(t, 2.2, 0.8)));
    b
}

// ---------------------------------------------------------------- 场景五：指令结构

struct InstrLine {
    text: String,
    /// 0 普通、1 标题、2 强调
    hi: u8,
}

fn instr_lines(l: &Layout, latex: &str) -> Vec<InstrLine> {
    let mut v = Vec::new();
    v.push(InstrLine { text: format!("layout(r\"{latex}\")"), hi: 1 });
    v.push(InstrLine { text: format!("  glyphs: {}", l.glyphs.len()), hi: 1 });
    for (i, g) in l.glyphs.iter().enumerate() {
        v.push(InstrLine {
            text: format!(
                "    [{:>2}] U+{:04X}   x={:>7.1}  y={:>7.1}  scale={:.3}",
                i, g.ch as u32, g.x, g.y, g.scale
            ),
            hi: if g.ch == '√' { 2 } else { 0 },
        });
    }
    v.push(InstrLine { text: format!("  rules: {}", l.rules.len()), hi: 1 });
    for (i, r) in l.rules.iter().enumerate() {
        v.push(InstrLine {
            text: format!(
                "    [{:>2}] x={:>7.1}  y={:>7.1}  width={:>7.1}  thickness={:.1}",
                i, r.x, r.y, r.width, r.thickness
            ),
            hi: 2,
        });
    }
    v.push(InstrLine {
        text: format!("  width={:.1}  height={:.1}  depth={:.1}", l.width, l.height, l.depth),
        hi: 0,
    });
    v
}

fn scene_instruct(ctx: &Ctx, lines: &[InstrLine], t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "指令结构", "布局输出就是这几行数字", 1.0);

    let panel = ramp(t, 0.25, 0.45);
    b.push_str(&rect(200.0, 296.0, 1520.0, 574.0, 14.0, C_PANEL, 0.95 * panel, Some((C_LINE, 1.5, panel))));
    b.push_str(&rect(200.0, 296.0, 150.0, 3.0, 1.5, C_ACC, 0.9 * panel, None));
    b.push_str(&mono(ctx, "mathengine::layout::Layout", 17.0, C_SUB, 0.9 * panel, 228.0, 330.0, false));
    let tag = Spec {
        s: "字体单位 · 1 em = 1000",
        size: 17.0,
        fill: C_FAINT,
        alpha: 0.9 * panel,
        mono: true,
        bold: false,
        ls: 0.0,
        pre: false,
    };
    b.push_str(&ctx.text_right(&tag, 1692.0, 330.0));

    // 右侧字段说明：把左边这棵树读成「宿主能直接用的三组数据」
    b.push_str(&rect(1110.0, 360.0, 1.0, 430.0, 0.5, "#233246", 0.9 * panel, None));
    let fa = ramp(t, 2.30, 0.55);
    b.push_str(&mono(ctx, "fields", 16.0, C_FAINT, 0.9 * fa, 1190.0, 386.0, false));
    let groups: [(&str, &str, &str); 3] = [
        ("glyphs", "码位 · x · y · scale", "按 layout 的输出顺序"),
        ("rules", "x · y · width · thickness", "只有水平线，没有位图"),
        ("size", "width · height · depth", "基线为原点，向上为正"),
    ];
    for (i, (name, fields, note)) in groups.iter().enumerate() {
        let y = 424.0 + i as f32 * 112.0;
        let a = ramp(t, 2.45 + i as f32 * 0.22, 0.5);
        b.push_str(&dot(1132.0, y - 7.0, 3.2, C_ACC, 0.9 * a));
        b.push_str(&mono(ctx, name, 21.0, C_ACC, a, 1150.0, y, false));
        b.push_str(&txt(ctx, fields, 18.0, C_DIM, a * 0.95, 1150.0, y + 28.0, false));
        b.push_str(&txt(ctx, note, 16.0, C_FAINT, a * 0.9, 1150.0, y + 52.0, false));
    }

    b.push_str(&txt(ctx, "输出里没有字体文件、位图或 TeX 内部状态，", 17.0, C_DIM, ramp(t, 3.6, 0.6), 1150.0, 790.0, false));
    b.push_str(&txt(ctx, "宿主自带字体就能照着这些数字画。", 17.0, C_DIM, ramp(t, 3.8, 0.6), 1150.0, 816.0, false));

    let lh = 26.0;
    let y0 = 362.0;
    for (i, ln) in lines.iter().enumerate() {
        let a = ramp(t, 0.55 + i as f32 * 0.145, 0.28);
        if a <= 0.004 {
            continue;
        }
        let (c, al) = match ln.hi {
            2 => (C_ACC, a),
            1 => (C_DIM, a * 0.95),
            _ => (C_TXT, a * 0.9),
        };
        let dx = 10.0 * (1.0 - a);
        b.push_str(&mono_pre(ctx, &ln.text, 19.0, c, al, 248.0 + dx, y0 + i as f32 * lh));
    }

    b.push_str(&txt(
        ctx,
        "宿主拿到这三组数字就能直接绘制，不需要理解 TeX；引擎自己不碰像素。",
        22.0,
        C_DIM,
        ramp(t, 4.55, 0.6),
        200.0,
        914.0,
        false,
    ));
    b.push_str(&footnote(ctx, "坐标以基线为原点、向上为正；分数线与根号横线是唯一的非字形指令", ramp(t, 4.9, 0.6)));
    b
}

// ---------------------------------------------------------------- 场景六：公式演示

const GALLERY: [(&str, &str); 4] = [
    (r"\int_{-\infty}^{\infty} e^{-x^2}\,dx=\sqrt{\pi}", "积分上下限 + 指数里的上标"),
    (r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}^{-1}", "pmatrix 环境 + 自适应括号"),
    (r"\sum_{i=1}^{n} i^2=\frac{n(n+1)(2n+1)}{6}", "求和带上下限 + 分式"),
    (r"\oint_{\partial\Omega}\vec{B}\cdot d\vec{S}=\mu_0 I", "环路积分 + 矢量重音"),
];

fn scene_gallery(ctx: &Ctx, arts: &[Art], local: usize) -> String {
    let mut b = String::new();
    let slot = 90usize; // 3.0 s 一条，硬切
    let cur = (local / slot).min(arts.len() - 1);
    let t = (local % slot) as f32 / FPS;
    let art = &arts[cur];

    scene_head(ctx, &mut b, "逐帧渲染", "不同类型的公式，同一套流程", 1.0);

    let tag = Spec {
        s: "mathengine::svg::render(&font, latex, size)",
        size: 20.0,
        fill: C_SUB,
        alpha: 0.85,
        mono: true,
        bold: false,
        ls: 0.0,
        pre: false,
    };
    b.push_str(&ctx.text_right(&tag, 1758.0, 214.0));

    // 淡入的同一帧里也保留公式组：断言要求每个演示帧恰好一条公式
    let p = ramp(t, 0.0, 0.22);
    let a = 0.02 + 0.98 * p;
    let rise = 12.0 * (1.0 - p);
    b.push_str(&draw_art(ctx, art, 960.0, 512.0 + rise, 132.0, 1400.0, 340.0, C_TXT, a));

    let cap = txt(ctx, GALLERY[cur].1, 26.0, C_DIM, a, 960.0, 792.0, true);
    b.push_str(&cap_group(cap));

    let idxs = format!("{:02} / {:02}", cur + 1, arts.len());
    b.push_str(&mono(ctx, &idxs, 21.0, C_FAINT, a, 200.0, 792.0, false));
    let stat = format!("{} 字形 · {} 段横线", art.glyphs.len(), art.rules.len());
    let st = Spec { s: &stat, size: 21.0, fill: C_FAINT, alpha: a, mono: true, bold: false, ls: 0.0, pre: false };
    b.push_str(&ctx.text_right(&st, 1758.0, 792.0));

    // 滑槽进度：四段，当前段按帧推进
    let pw = 148.0;
    let pg = 16.0;
    let totalw = 4.0 * pw + 3.0 * pg;
    let sx = (FW - totalw) / 2.0;
    for i in 0..4 {
        let x = sx + i as f32 * (pw + pg);
        b.push_str(&rect(x, 862.0, pw, 4.0, 2.0, "#1b2434", 0.95, None));
        if i < cur {
            b.push_str(&rect(x, 862.0, pw, 4.0, 2.0, C_ACC, 0.42, None));
        } else if i == cur {
            let p = (local % slot) as f32 / slot as f32;
            b.push_str(&rect(x, 862.0, pw * p, 4.0, 2.0, C_ACC, 0.95, None));
        }
    }

    let src = Spec { s: GALLERY[cur].0, size: 24.0, fill: C_SUB, alpha: a, mono: true, bold: false, ls: 0.0, pre: false };
    let sw = ctx.tw(&src);
    b.push_str(&rect(960.0 - sw / 2.0 - 26.0, 900.0, sw + 52.0, 48.0, 10.0, "#0d1420", 0.95 * a, Some((C_LINE, 1.2, a))));
    b.push_str(&ctx.text_at(&src, 960.0, 931.0, true));

    b.push_str(&footnote(ctx, "逐帧离线渲染 · 非即时播放 · 每条公式独立计算，不复用上一条的布局结果", ramp(t, 0.3, 0.6)));
    b
}

// ---------------------------------------------------------------- 场景七：真实代码

/// 逐字取自 `src/layout.rs` 的 `layout_row` 内层循环（构建时会校验）。
const CODE_FILE: &str = "src/layout.rs";
const CODE_LINES: [&str; 15] = [
    "        prev_class = cls;",
    "        // 直接按偏移 push，不再先 translate 出一份中间 Box：",
    "        // 长行（几百项）下每次 append 都多一次全量复制，整体会退化成 O(n²)。",
    "        let base_x = out.width;",
    "        out.glyphs.reserve(child.glyphs.len());",
    "        out.rules.reserve(child.rules.len());",
    "        for g in &child.glyphs {",
    "            out.glyphs.push(PlacedGlyph { x: g.x + base_x, y: g.y, ..*g });",
    "        }",
    "        for r in &child.rules {",
    "            out.rules.push(Rule { x: r.x + base_x, ..*r });",
    "        }",
    "        out.width = base_x + child.width;",
    "        out.height = out.height.max(child.height);",
    "        out.depth = out.depth.max(child.depth);",
];

/// 起始行号：在真实文件里定位这段，顺带证明片段没有被改写。
fn code_start_line() -> usize {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/layout.rs");
    let src = fs::read_to_string(path).expect("读 src/layout.rs");
    let needle = CODE_LINES.join("\n");
    let pos = src.find(&needle).expect("代码片段与 src/layout.rs 不一致");
    src[..pos].matches('\n').count() + 1
}

fn scene_code(ctx: &Ctx, first: usize, t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "实现", "布局循环里的一处性能修复", 1.0);

    let panel = ramp(t, 0.20, 0.40);
    b.push_str(&rect(200.0, 300.0, 1520.0, 604.0, 14.0, C_PANEL, 0.95 * panel, Some((C_LINE, 1.5, panel))));
    b.push_str(&rect(200.0, 300.0, 520.0, 3.0, 1.5, C_ACC, 0.9 * panel, None));

    let loc = format!("{CODE_FILE} · {}–{}", first, first + CODE_LINES.len() - 1);
    b.push_str(&mono(ctx, &loc, 18.0, C_SUB, 0.95 * panel, 230.0, 340.0, false));
    let hint = Spec { s: "高亮 = 本轮的关键改动", size: 17.0, fill: C_FAINT, alpha: 0.9 * panel, mono: false, bold: false, ls: 0.0, pre: false };
    b.push_str(&ctx.text_right(&hint, 1690.0, 340.0));

    // 高亮底色（先画，文字压在上面）
    for k in [4usize, 5, 6, 7] {
        let a = ramp(t, 1.45 + k as f32 * 0.08, 0.35);
        let y = 376.0 + k as f32 * 31.0;
        b.push_str(&rect(250.0, y, 1440.0, 30.0, 4.0, C_ACC, 0.11 * a, None));
        b.push_str(&rect(250.0, y, 4.0, 30.0, 2.0, C_ACC, 0.95 * a, None));
    }

    let lh = 31.0;
    let y0 = 400.0;
    for (i, ln) in CODE_LINES.iter().enumerate() {
        let a = ramp(t, 0.35 + i as f32 * 0.085, 0.30);
        if a <= 0.004 {
            continue;
        }
        let num = format!("{:>4}", first + i);
        let nsp = Spec { s: &num, size: 18.0, fill: C_FAINT, alpha: 0.75 * a, mono: true, bold: false, ls: 0.0, pre: true };
        b.push_str(&ctx.text_right(&nsp, 300.0, y0 + i as f32 * lh));
        b.push_str(&mono_pre(ctx, ln, 20.0, C_TXT, 0.92 * a, 320.0, y0 + i as f32 * lh));
    }

    let stat = "这一处改动的效果：500 项长行 33.9 ms → 3.54 ms（9.6×）";
    let sa = ramp(t, 2.8, 0.6);
    b.push_str(&mono(ctx, stat, 21.0, C_ACC, sa, 232.0, 872.0, false));

    b.push_str(&footnote(ctx, "片段逐字取自 src/layout.rs，构建时用字符串匹配校验与文件一致", ramp(t, 3.4, 0.7)));
    b
}

// ---------------------------------------------------------------- 场景八：对比条形图

/// 数据来自 docs/EXPERIMENTS.md 实验一（中位耗时，µs，各 2000 次）。
/// 最后一项 slower 为真：求和上本引擎略慢，照实画。
const BARS: [(&str, f32, f32, &str, bool); 6] = [
    ("简单变量 x", 0.71, 3.43, "4.8×", false),
    ("分式", 1.48, 3.74, "2.5×", false),
    ("根号", 1.50, 3.33, "2.2×", false),
    ("矩阵", 3.91, 19.74, "5.0×", false),
    ("求和", 4.18, 3.97, "0.95×", true),
    ("长公式", 7.20, 9.45, "1.3×", false),
];

fn scene_chart(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "实测", "同一批公式，两条实现", 1.0);

    b.push_str(&txt(ctx, "中位耗时（µs）· 每条各 2000 次采样 · 预热后取值", 21.0, C_FAINT, ramp(t, 0.2, 0.5), 200.0, 340.0, false));

    // 图例
    let la = ramp(t, 0.35, 0.5);
    b.push_str(&rect(1210.0, 328.0, 12.0, 12.0, 3.0, C_ACC, 0.95 * la, None));
    b.push_str(&txt(ctx, "mathengine", 20.0, C_DIM, la, 1232.0, 340.0, false));
    b.push_str(&rect(1450.0, 328.0, 12.0, 12.0, 3.0, "#5a6b85", 0.95 * la, None));
    b.push_str(&txt(ctx, "jlatexmath 1.0.7", 20.0, C_DIM, la, 1472.0, 340.0, false));

    let x0 = 560.0;
    let max_v = 19.74f32;
    let bw = 1000.0f32;
    let sc = bw / max_v;

    // 网格与刻度
    let ga = ramp(t, 0.3, 0.5);
    for v in [0.0f32, 5.0, 10.0, 15.0, 20.0] {
        let x = x0 + v * sc;
        b.push_str(&rect(x, 388.0, 1.0, 486.0, 0.5, "#243248", 0.9 * ga, None));
        let lb = if v == 0.0 { "0".to_string() } else { format!("{v:.0}") };
        b.push_str(&mono(ctx, &lb, 16.0, C_FAINT, 0.85 * ga, x, 898.0, true));
    }
    b.push_str(&mono(ctx, "µs", 16.0, C_FAINT, 0.85 * ga, x0 + 20.0 * sc + 22.0, 898.0, false));

    for (i, (name, me, jl, ratio, slower)) in BARS.iter().enumerate() {
        let y = 404.0 + i as f32 * 76.0;
        let st = 0.55 + i as f32 * 0.30;
        let p = ease_out_cubic((t - st) / 1.15);
        let a = ramp(t, st, 0.25);
        if a <= 0.004 {
            continue;
        }

        b.push_str(&txt(ctx, name, 22.0, C_DIM, a, 220.0, y + 30.0, false));

        let wm = me * sc * p;
        let wj = jl * sc * p;
        b.push_str(&rect(x0, y + 4.0, wm.max(1.0), 20.0, 3.0, C_ACC, 0.92 * a, None));
        b.push_str(&rect(x0, y + 32.0, wj.max(1.0), 20.0, 3.0, "#5a6b85", 0.9 * a, None));

        let va = ramp(t, st + 0.85, 0.3);
        b.push_str(&mono(ctx, &format!("{me:.2}"), 16.0, C_ACC, va, x0 + wm + 9.0, y + 20.0, false));
        b.push_str(&mono(ctx, &format!("{jl:.2}"), 16.0, C_DIM, va, x0 + wj + 9.0, y + 48.0, false));

        let rc = if *slower { C_AMBER } else { C_ACC };
        let rsp = Spec { s: ratio, size: 19.0, fill: rc, alpha: va, mono: true, bold: true, ls: 0.0, pre: false };
        b.push_str(&ctx.text_right(&rsp, 1740.0, y + 30.0));
    }

    b.push_str(&txt(
        ctx,
        "矩阵 5.0×，求和 0.95×：慢的那条照实画，没有为了数据好看去针对它调优。",
        23.0,
        C_DIM,
        ramp(t, 4.4, 0.7),
        200.0,
        944.0,
        false,
    ));
    b.push_str(&footnote(ctx, "Ryzen 5 5600 · Arch Linux · rustc 1.98 · opt-level=3 + lto · 计时含解析与布局", ramp(t, 4.9, 0.7)));
    b
}

// ---------------------------------------------------------------- 场景九：结尾

fn scene_end(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let over = Spec { s: "仓库", size: 24.0, fill: C_ACC, alpha: ramp(t, 0.1, 0.5), mono: false, bold: false, ls: 10.0, pre: false };
    b.push_str(&ctx.text_at(&over, 960.0, 372.0, true));

    let repo = Spec { s: "mathengine", size: 104.0, fill: C_TXT, alpha: ramp(t, 0.25, 0.6), mono: true, bold: true, ls: 0.0, pre: false };
    b.push_str(&ctx.text_at(&repo, 960.0, 492.0, true));

    b.push_str(&txt(ctx, "Rust 从零实现 · 2900 行 · 只依赖 ttf-parser", 28.0, C_DIM, ramp(t, 0.65, 0.6), 960.0, 562.0, true));

    let la = ramp(t, 0.95, 0.6);
    let lic = Spec { s: "Apache-2.0", size: 28.0, fill: C_ACC, alpha: la, mono: true, bold: true, ls: 2.0, pre: false };
    let lw = ctx.tw(&lic);
    b.push_str(&rect(960.0 - lw / 2.0 - 32.0, 604.0, lw + 64.0, 54.0, 27.0, "#0d2a26", 0.9 * la, Some((C_ACC, 1.6, 0.65 * la))));
    b.push_str(&ctx.text_at(&lic, 960.0, 641.0, true));

    b.push_str(&txt(ctx, "源码与实验数据均可复现", 32.0, C_TXT, ramp(t, 1.35, 0.6), 960.0, 742.0, true));

    b.push_str(&mono(
        ctx,
        "docs/EXPERIMENTS.md · docs/screenshots/ · tools/make_demo_video.sh",
        19.0,
        C_FAINT,
        ramp(t, 1.75, 0.6),
        960.0,
        800.0,
        true,
    ));
    b.push_str(&footnote(ctx, "本片无声版：公式图形逐帧预渲染，配乐与旁白后续统一混音", ramp(t, 2.1, 0.6)));
    b
}

// ---------------------------------------------------------------- 公式绘制

#[allow(clippy::too_many_arguments)]
fn draw_art(
    ctx: &Ctx,
    art: &Art,
    cx: f32,
    cy: f32,
    base_size: f32,
    max_w: f32,
    max_h: f32,
    fill: &str,
    alpha: f32,
) -> String {
    if alpha <= 0.004 || art.glyphs.is_empty() {
        return String::new();
    }
    let k = art_fit(art, ctx, base_size, max_w, max_h);
    let hd = (art.height + art.depth) * k;
    let left = cx - art.width * k / 2.0;
    let baseline = cy - hd / 2.0 + art.height * k;

    let mut b = format!(
        r#"<g data-formula="1" transform="translate({left:.2} {baseline:.2}) scale({k:.5})" fill="{fill}" fill-opacity="{alpha:.3}">"#
    );
    for g in &art.glyphs {
        let _ = write!(
            b,
            r#"<path transform="translate({:.1} {:.1}) scale({:.4})" d="{}"/>"#,
            g.x, -g.y, g.scale, g.d
        );
    }
    for r in &art.rules {
        let _ = write!(
            b,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}"/>"#,
            r.x, -r.y, r.w, r.th
        );
    }
    b.push_str("</g>");
    b
}

// ---------------------------------------------------------------- 合成一帧

struct Reel {
    deco: Art,
    assemble: Art,
    gallery: Vec<Art>,
    instr: Vec<InstrLine>,
    code_line: usize,
}

fn secs(i: usize, start: usize) -> f32 {
    (i - start) as f32 / FPS
}

fn scene_body(ctx: &Ctx, r: &Reel, idx: usize, i: usize) -> String {
    match idx {
        0 => scene_cover(ctx, &r.deco, secs(i, S_COVER)),
        1 => scene_problem(ctx, secs(i, S_PROBLEM)),
        2 => scene_arch(ctx, secs(i, S_ARCH)),
        3 => scene_assemble(ctx, &r.assemble, secs(i, S_ASSEMBLE)),
        4 => scene_instruct(ctx, &r.instr, secs(i, S_INSTRUCT)),
        5 => scene_gallery(ctx, &r.gallery, i - S_GALLERY),
        6 => scene_code(ctx, r.code_line, secs(i, S_CODE)),
        7 => scene_chart(ctx, secs(i, S_CHART)),
        _ => scene_end(ctx, secs(i, S_END)),
    }
}

fn frame(ctx: &Ctx, reel: &Reel, i: usize) -> String {
    let mut idx = 0;
    for (k, (start, _)) in SCENES.iter().enumerate() {
        if i >= *start {
            idx = k;
        }
    }

    // 换场一律硬切：一帧只画一个场景，不做任何交叉淡化
    let body = scene_body(ctx, reel, idx, i);

    // 硬保证：一帧里最多一条公式（封面装饰 / 字形组装 / 公式演示各 1 条，其余 0 条）
    let n_formula = body.matches(r#"data-formula="1""#).count();
    let allow = usize::from(matches!(idx, 0 | 3 | 5));
    assert!(n_formula <= allow, "第 {i} 帧出现 {n_formula} 条公式，最多允许 {allow}");
    if idx == 3 || idx == 5 {
        assert_eq!(n_formula, 1, "第 {i} 帧（场景 {idx}）应有且只有一条公式");
    }
    // 硬保证：一帧里最多一套字幕
    let n_cap = body.matches(r#"data-cap="1""#).count();
    assert!(n_cap <= 1, "第 {i} 帧出现 {n_cap} 套字幕，最多允许 1");

    let prog = i as f32 / (TOTAL - 1) as f32;
    let mut svg = String::with_capacity(24 * 1024);
    let _ = write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{FW:.0}" height="{FH:.0}" viewBox="0 0 {FW:.0} {FH:.0}">"#
    );
    svg.push_str(&background(prog, SCENES[idx].1, ctx));
    svg.push_str(&body);
    // 收尾 0.8 秒压到黑，避免片子硬切结束
    let fade = 1.0 - smooth((i as f32 - (TOTAL as f32 - 24.0)) / 23.0);
    if fade < 0.999 {
        let _ = write!(
            svg,
            r##"<rect width="{FW:.0}" height="{FH:.0}" fill="#000000" fill-opacity="{:.3}"/>"##,
            1.0 - fade
        );
    }
    svg.push_str("</svg>");
    svg
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "/tmp/mgframes".to_string());
    fs::create_dir_all(&dir).expect("建输出目录");

    let math_data: &'static [u8] = Box::leak(
        load_latin_modern_math(mathengine::LM_MATH_OTF)
            .expect("读 assets/latinmodern-math.otf")
            .into_boxed_slice(),
    );
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
    let mono_path = "/usr/share/fonts/TTF/JetBrains-Maple-Mono-NF-XX-XX";
    let jbm: &'static [u8] = Box::leak(
        fs::read(format!("{mono_path}/JetBrainsMapleMono-Regular.ttf"))
            .expect("JetBrains Maple Mono")
            .into_boxed_slice(),
    );
    let jbm_bold: &'static [u8] = Box::leak(
        fs::read(format!("{mono_path}/JetBrainsMapleMono-Bold.ttf"))
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
    };

    let mut cache: HashMap<u16, String> = HashMap::new();

    let assemble = match build_art(&font, &mut cache, ASSEMBLE_LATEX) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("FAIL 组装公式 {ASSEMBLE_LATEX}: {e}");
            std::process::exit(1);
        }
    };
    println!("组装公式：{} 字形 · {} 横线 · 宽 {:.0} 高 {:.0} 深 {:.0}", assemble.glyphs.len(), assemble.rules.len(), assemble.width, assemble.height, assemble.depth);

    let mut gallery = Vec::new();
    for (latex, _) in GALLERY {
        match build_art(&font, &mut cache, latex) {
            Ok(a) => {
                println!("OK   {:>2} 字形 {:>2} 横线  {}", a.glyphs.len(), a.rules.len(), latex);
                gallery.push(a);
            }
            Err(e) => {
                eprintln!("FAIL {latex}: {e}");
                std::process::exit(1);
            }
        }
    }
    let deco = build_art(&font, &mut cache, r"e^{i\pi}+1=0").expect("封面装饰公式");

    let ast = parse(ASSEMBLE_LATEX).expect("解析组装公式");
    let l = layout(&font, &ast);
    let instr = instr_lines(&l, ASSEMBLE_LATEX);
    println!("指令面板 {} 行", instr.len());

    let code_line = code_start_line();
    println!("代码片段起始行：src/layout.rs:{code_line}");

    let reel = Reel { deco, assemble, gallery, instr, code_line };

    for i in 0..TOTAL {
        let svg = frame(&ctx, &reel, i);
        fs::write(format!("{dir}/f{i:05}.svg"), svg).expect("写 SVG");
        if i % 300 == 0 {
            println!("  帧 {i}/{TOTAL}");
        }
    }
    println!("共 {TOTAL} 帧（{:.1} s）→ {dir}", TOTAL as f32 / FPS);

    println!("场景表：");
    for (k, (start, name)) in SCENES.iter().enumerate() {
        let end = if k + 1 < SCENES.len() { SCENES[k + 1].0 } else { TOTAL };
        println!(
            "  {name:<6} {:>6.2} – {:>6.2} s  (帧 {start}–{})",
            *start as f32 / FPS,
            end as f32 / FPS,
            end - 1
        );
    }

    let warn = ctx.warn.borrow();
    if !warn.is_empty() {
        println!("检查提示：");
        for w in warn.iter() {
            println!("  {w}");
        }
    }
}
