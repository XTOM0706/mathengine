//! `cargo run --release --example video_frames -- 输出目录`：生成演示视频的逐帧 SVG。
//!
//! 每一帧都是一张 1920×1080 的完整 SVG：公式部分不是截图，而是调用
//! `mathengine::layout` 算布局、再向字体索取字形轮廓画出来的路径——和
//! `examples/render_svg.rs` 走的是同一份数据。中文与说明文字用 SVG `<text>`
//! 交给 rsvg / Pango 排版，避免 ffmpeg drawtext 的字体路径问题。
//!
//! 观众是科创赛评委：全片用效果图和耗时说事，不出现实现内部的术语。
//! 离线逐帧预渲染，换场一律硬切，一帧只画一条公式、最多一套字幕。
//!
//! 宽度测量直接读字体 `hmtx`：中文用 Noto Sans CJK SC，拉丁用 JetBrains Maple
//! Mono，这样居中与分段文字的落点不靠猜。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::time::Instant;
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
const C_AMBER: &str = "#f7b955";

const SANS: &str = "Noto Sans CJK SC";
const MONO: &str = "JetBrains Maple Mono";

// 场景切分（帧，30 fps）。S_ 为场景起始，TOTAL 为总帧数。
const S_COVER: usize = 0; // 4.0 s
const S_SHOW: usize = 120; // 56.0 s：四条公式，各 14 s
const S_MATRIX: usize = 1800; // 46.0 s：2×2 / 5×5 / 20×20
const S_PERF: usize = 3180; // 40.0 s：表格 19 s + 柱状图 21 s
const S_ARCH: usize = 4380; // 34.0 s：三步流程
const S_END: usize = 5400; // 8.0 s
const TOTAL: usize = 5640; // 188.0 s

const SHOW_SLOT: usize = 420; // 每条公式 14 s
const MAT_SLOT: [usize; 3] = [360, 420, 600]; // 12 s / 14 s / 20 s
const TBL_LEN: usize = 570; // 19 s
const CHART_LEN: usize = 630; // 21 s
// 场景长度必须首尾接上，改一个数字忘了另一个会在编译时报错
const _: () = assert!(S_MATRIX - S_SHOW == 4 * SHOW_SLOT);
const _: () = assert!(S_PERF - S_MATRIX == MAT_SLOT[0] + MAT_SLOT[1] + MAT_SLOT[2]);
const _: () = assert!(S_ARCH - S_PERF == TBL_LEN + CHART_LEN);
const _: () = assert!(TOTAL == S_END + 240);

const SCENES: [(usize, &str); 6] = [
    (S_COVER, "封面"),
    (S_SHOW, "公式演示"),
    (S_MATRIX, "复杂矩阵"),
    (S_PERF, "性能对比"),
    (S_ARCH, "怎么做的"),
    (S_END, "仓库"),
];

/// 全片禁用的词：观众是评委，不该看到实现内部的行话。
const BANNED: [&str; 8] = ["实时", "token", "AST", "glyph", "Box", "指令", "语法树", "词元"];

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

/// 一条公式在画面上的数据：名字、一句话说明、原始输入、算好的图形、实测耗时。
struct Shot {
    name: &'static str,
    note: &'static str,
    latex: String,
    art: Art,
    micros: f64,
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

/// 落位用：先越过目标一点再回弹，像字块被放上去。
fn ease_out_back(x: f32) -> f32 {
    let p = clamp01(x);
    let c1 = 1.35;
    let c3 = c1 + 1.0;
    1.0 + c3 * (p - 1.0).powi(3) + c1 * (p - 1.0).powi(2)
}

/// 一段时间内的「亮起」包络：起、亮、落。
fn ping(t: f32, start: f32, hold: f32) -> f32 {
    ramp(t, start, 0.45) * (1.0 - ramp(t, start + hold, 0.45))
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

/// 底部分段进度：`n` 段，当前段按 `frac` 推进。
fn slider(b: &mut String, n: usize, cur: usize, frac: f32, y: f32) {
    let pw = 156.0;
    let pg = 16.0;
    let totalw = n as f32 * pw + (n.saturating_sub(1)) as f32 * pg;
    let sx = (FW - totalw) / 2.0;
    for i in 0..n {
        let x = sx + i as f32 * (pw + pg);
        b.push_str(&rect(x, y, pw, 4.0, 2.0, "#1b2434", 0.95, None));
        if i < cur {
            b.push_str(&rect(x, y, pw, 4.0, 2.0, C_ACC, 0.40, None));
        } else if i == cur {
            b.push_str(&rect(x, y, pw * clamp01(frac), 4.0, 2.0, C_ACC, 0.95, None));
        }
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
    let sec = Spec { s: section, size: 20.0, fill: C_SUB, alpha: 0.95, mono: false, bold: false, ls: 3.0 };
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
    let over = Spec { s: overline, size: 22.0, fill: C_ACC, alpha: a, mono: false, bold: false, ls: 8.0 };
    b.push_str(&ctx.text_at(&over, 200.0, 214.0, false));
    let ttl = Spec { s: title, size: 52.0, fill: C_TXT, alpha: a, mono: false, bold: true, ls: 0.0 };
    b.push_str(&ctx.text_at(&ttl, 200.0, 282.0, false));
    b.push_str(&rect(170.0, 240.0, 5.0, 50.0, 2.5, C_ACC, a, None));
}

/// 页脚小字：第三档层次，固定贴在进度条上方。
fn footnote(ctx: &Ctx, s: &str, a: f32) -> String {
    if a <= 0.004 {
        return String::new();
    }
    let sp = Spec { s, size: 18.0, fill: C_FOOT, alpha: a, mono: false, bold: false, ls: 0.6 };
    ctx.text_at(&sp, 200.0, 978.0, false)
}

// ---------------------------------------------------------------- 场景一：封面

fn scene_cover(ctx: &Ctx, art: &Art, t: f32) -> String {
    let mut b = String::new();

    let over = Spec { s: "用 Rust 从零写的 · 不用 Java · 不用浏览器", size: 24.0, fill: C_ACC, alpha: ramp(t, 0.05, 0.55), mono: false, bold: false, ls: 8.0 };
    b.push_str(&ctx.text_at(&over, 960.0, 300.0, true));

    let title = Spec { s: "mathengine", size: 152.0, fill: C_TXT, alpha: ramp(t, 0.15, 0.65), mono: true, bold: true, ls: 0.0 };
    let dy = 26.0 * (1.0 - ramp(t, 0.15, 0.75));
    b.push_str(&ctx.text_at(&title, 960.0, 470.0 + dy, true));

    b.push_str(&txt(ctx, "给手机用的公式排版引擎", 48.0, C_DIM, ramp(t, 0.55, 0.6), 960.0, 560.0 + dy * 0.5, true));

    // 分隔线由中心向两侧展开
    let grow = ramp(t, 0.75, 0.7);
    if grow > 0.01 {
        let w = 300.0 * grow;
        b.push_str(&rect(960.0 - w / 2.0, 612.0, w, 2.0, 1.0, C_ACC, 0.85, None));
    }

    let contest = Spec { s: "第 38 届苏州市青少年科技创新大赛", size: 28.0, fill: C_DIM, alpha: ramp(t, 1.0, 0.6), mono: false, bold: false, ls: 3.0 };
    b.push_str(&ctx.text_at(&contest, 960.0, 682.0, true));

    let deco = ramp(t, 1.35, 0.85);
    b.push_str(&draw_art(ctx, art, 960.0, 858.0, 96.0, 1500.0, 200.0, C_TXT, 0.46 * deco));

    b.push_str(&footnote(ctx, "全片画面由引擎离线逐帧生成，不是录屏，也不是截图", ramp(t, 2.2, 0.8)));
    b
}

// ---------------------------------------------------------------- 场景二：公式演示

/// 开头的四条公式：普通公式一条一条出来，先让人看见东西。
const SHOW_SRC: [(&str, &str, &str); 4] = [
    (r"x=\frac{-b\pm\sqrt{b^{2}-4ac}}{2a}", "二次方程", "分式、根号、上下标，挤进一条里"),
    (r"\int_{-\infty}^{\infty} e^{-x^{2}}\,dx=\sqrt{\pi}", "积分", "上下限贴在积分号的上下，字号自动变小"),
    (r"\sum_{i=1}^{n} i^{2}=\frac{n(n+1)(2n+1)}{6}", "求和", "求和号的上下限，右边再跟一个分式"),
    (r"x_{1}^{2}+x_{2}^{2}+\cdots+x_{n}^{2}", "上下标", "上标抬高、下标压低，位置一个不差"),
];

fn scene_show(ctx: &Ctx, shows: &[Shot], local: usize) -> String {
    let mut b = String::new();
    let cur = (local / SHOW_SLOT).min(shows.len() - 1);
    let t = (local % SHOW_SLOT) as f32 / FPS;
    let sh = &shows[cur];

    scene_head(ctx, &mut b, "公式演示", "这是它画出来的", 1.0);

    // 右上角：第几条
    let idxs = format!("第 {} 条 / 共 {} 条", cur + 1, shows.len());
    let isp = Spec { s: &idxs, size: 20.0, fill: C_FAINT, alpha: 0.95, mono: false, bold: false, ls: 1.0 };
    b.push_str(&ctx.text_right(&isp, 1758.0, 214.0));

    let n = sh.art.glyphs.len();
    let stagger = (2.3 / n.max(1) as f32).min(0.17);
    let g = reveal_art(&sh.art, ctx, 960.0, 512.0, 150.0, 1380.0, 300.0, C_TXT, t, 0.22, stagger, 0.42);
    b.push_str(&g);

    // 一条字幕：名字 + 一句说明
    let ca = ramp(t, 2.95, 0.55);
    let name = txt(ctx, sh.name, 34.0, C_TXT, ca, 960.0, 726.0, true);
    let note = txt(ctx, sh.note, 24.0, C_DIM, ramp(t, 3.2, 0.55), 960.0, 766.0, true);
    b.push_str(&cap_group(format!("{name}{note}")));

    // 输入：一行文字
    let ia = ramp(t, 6.2, 0.6);
    if ia > 0.004 {
        let lab = Spec { s: "输入", size: 21.0, fill: C_DIM, alpha: ia, mono: false, bold: false, ls: 2.0 };
        let src = Spec { s: &sh.latex, size: 21.0, fill: C_TXT, alpha: 0.92 * ia, mono: true, bold: false, ls: 0.0 };
        let lw = ctx.tw(&lab);
        let w = lw + 16.0 + ctx.tw(&src);
        let x = 960.0 - w / 2.0;
        b.push_str(&rect(x - 24.0, 792.0, w + 48.0, 44.0, 10.0, "#0d1420", 0.92 * ia, Some((C_LINE, 1.2, ia))));
        b.push_str(&ctx.text_at(&lab, x, 820.0, false));
        b.push_str(&ctx.text_at(&src, x + lw + 16.0, 820.0, false));
    }

    // 实测耗时（当场测，不是抄来的）
    let stat = format!("画出来 {} 个符号 · 算一遍 {}", n, us_text(sh.micros));
    b.push_str(&mono(ctx, &stat, 20.0, C_SUB, ramp(t, 8.6, 0.6), 960.0, 886.0, true));

    let frac = (local % SHOW_SLOT) as f32 / SHOW_SLOT as f32;
    slider(&mut b, shows.len(), cur, frac, 930.0);

    b.push_str(&footnote(
        ctx,
        "先算位置，再一个个落下来；画面由引擎逐帧生成，不是截图",
        ramp(t, 1.6, 0.8),
    ));
    b
}

// ---------------------------------------------------------------- 场景三：复杂矩阵

fn matrix_digits(n: usize) -> String {
    // 大于 5 行时不用括号：引擎把括号的拉伸倍数卡在 4 倍以内（防止笔画被拉成粗黑条），
    // 20 行的括号只能包住中间几行，摆在画面里反而像画错了。大矩阵按数字表来展示。
    let env = if n > 5 { "matrix" } else { "pmatrix" };
    let mut s = format!(r"\begin{{{env}}}");
    for r in 0..n {
        for c in 0..n {
            if c > 0 {
                s.push('&');
            }
            if n == 2 {
                let _ = write!(s, "{}", r * 2 + c * 3 + 1);
            } else {
                let v = 10 + (r * 37 + c * 11) % 90;
                let _ = write!(s, "{v}");
            }
        }
        if r + 1 < n {
            s.push_str(r"\\");
        }
    }
    s.push_str(&format!(r"\end{{{env}}}"));
    s
}

const MAT_NAMES: [(&str, &str); 3] = [
    ("2 × 2", "四个格子，一对括号"),
    ("5 × 5", "数字有长有短，列还是对齐的"),
    ("20 × 20", "四百个数字摆在一起，一眼能数清"),
];

fn scene_matrix(ctx: &Ctx, mats: &[Shot], local: usize) -> String {
    let mut b = String::new();
    let (cur, l0) = if local < MAT_SLOT[0] {
        (0, local)
    } else if local < MAT_SLOT[0] + MAT_SLOT[1] {
        (1, local - MAT_SLOT[0])
    } else {
        (2, local - MAT_SLOT[0] - MAT_SLOT[1])
    };
    let t = l0 as f32 / FPS;
    let sh = &mats[cur];

    scene_head(ctx, &mut b, "复杂矩阵", "越排越密，也不会乱", 1.0);

    let tag = format!("第 {} 组 / 共 3 组", cur + 1);
    let tsp = Spec { s: &tag, size: 20.0, fill: C_FAINT, alpha: 0.95, mono: false, bold: false, ls: 1.0 };
    b.push_str(&ctx.text_right(&tsp, 1758.0, 214.0));

    let n = sh.art.glyphs.len();
    // 小矩阵两三秒内落完，大矩阵四秒扫完：落子过程要看得见，但不能让半张矩阵停在画面里
    let stagger = if n > 200 {
        4.2 / n as f32
    } else {
        (2.6 / n.max(1) as f32).min(0.16)
    };
    let g = reveal_art(&sh.art, ctx, 960.0, 540.0, 150.0, 1500.0, 500.0, C_TXT, t, 0.25, stagger, 0.34);
    b.push_str(&g);

    let ca = ramp(t, 1.4, 0.55);
    let name = txt(ctx, MAT_NAMES[cur].0, 34.0, C_TXT, ca, 960.0, 842.0, true);
    let note = txt(ctx, MAT_NAMES[cur].1, 24.0, C_DIM, ramp(t, 1.65, 0.55), 960.0, 880.0, true);
    b.push_str(&cap_group(format!("{name}{note}")));

    let stat = format!("画出来 {} 个符号 · 算一遍 {}", n, us_text(sh.micros));
    b.push_str(&mono(ctx, &stat, 19.0, C_SUB, ramp(t, 4.4, 0.6), 960.0, 922.0, true));

    if cur == 2 {
        let more = "40 × 40 的 7202 个符号：算一遍 3.9 毫秒";
        b.push_str(&mono(ctx, more, 19.0, C_ACC, ramp(t, 10.5, 0.7), 960.0, 946.0, true));
    }

    b.push_str(&footnote(
        ctx,
        "格子的宽窄、行距、每个字的大小，都由引擎自己量出来",
        ramp(t, 2.4, 0.8),
    ));
    b
}

// ---------------------------------------------------------------- 场景四：性能对比（表格）

/// 数字取自 docs/EXPERIMENTS.md（实验一、实验九），单位与位数照原表。
const ROWS: [(&str, &str, &str, &str, &str); 4] = [
    ("单个变量", "0.71 微秒", "3.43 微秒", "4.8 倍", "一个字母"),
    ("分式", "1.48 微秒", "3.74 微秒", "2.5 倍", "上下两行，中间一条横线"),
    ("矩阵 2×2", "3.91 微秒", "19.74 微秒", "5.0 倍", "四个格子加一对括号"),
    ("超大矩阵 40×40", "3.9 毫秒", "—", "单独测得", "7202 个符号 · 含出图 9.7 毫秒"),
];
const ROW_T: [f32; 4] = [0.8, 3.6, 6.4, 9.2];

fn scene_table(ctx: &Ctx, previews: &[Art], t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "性能对比", "同一批公式，两套实现各花多少时间", 1.0);

    // 表头
    let ha = ramp(t, 0.15, 0.5);
    let hd = |s: &'static str, x: f32, right: bool| -> String {
        let sp = Spec { s, size: 22.0, fill: C_DIM, alpha: ha, mono: false, bold: true, ls: 1.0 };
        if right {
            ctx.text_right(&sp, x, 396.0)
        } else {
            ctx.text_at(&sp, x, 396.0, false)
        }
    };
    b.push_str(&hd("场景", 232.0, false));
    b.push_str(&hd("本引擎", 790.0, true));
    b.push_str(&hd("现有方案", 1020.0, true));
    b.push_str(&hd("快多少倍", 1200.0, true));
    b.push_str(&line(200.0, 414.0, 1216.0, 414.0, "#3d5273", 1.8, 0.95 * ha));

    // 当前讲到哪一行
    let mut active = 0usize;
    for (i, st) in ROW_T.iter().enumerate() {
        if t >= *st {
            active = i;
        }
    }

    for (i, (label, me, theirs, ratio, note)) in ROWS.iter().enumerate() {
        let st = ROW_T[i];
        let a = ramp(t, st, 0.45);
        if a <= 0.004 {
            continue;
        }
        let yt = 424.0 + i as f32 * 80.0;
        let hot = ping(t, st, 2.1);
        b.push_str(&rect(200.0, yt, 1016.0, 72.0, 12.0, C_PANEL, (0.55 + 0.42 * hot) * a, Some((C_LINE, 1.4, a))));
        b.push_str(&rect(200.0, yt, 5.0, 72.0, 2.5, C_ACC, (0.45 + 0.5 * hot) * a, None));

        let lb = Spec { s: label, size: 26.0, fill: C_TXT, alpha: a, mono: false, bold: true, ls: 0.0 };
        b.push_str(&ctx.text_at(&lb, 232.0, yt + 34.0, false));
        let nb = Spec { s: note, size: 17.0, fill: C_DIM, alpha: 0.92 * a, mono: false, bold: false, ls: 0.0 };
        b.push_str(&ctx.text_at(&nb, 232.0, yt + 58.0, false));

        let mv = Spec { s: me, size: 26.0, fill: C_ACC, alpha: a, mono: true, bold: true, ls: 0.0 };
        b.push_str(&ctx.text_right(&mv, 790.0, yt + 46.0));
        let tv = Spec { s: theirs, size: 26.0, fill: C_DIM, alpha: a, mono: true, bold: false, ls: 0.0 };
        b.push_str(&ctx.text_right(&tv, 1020.0, yt + 46.0));
        let rv = Spec { s: ratio, size: 26.0, fill: if i == 3 { C_AMBER } else { C_TXT }, alpha: a, mono: false, bold: true, ls: 0.0 };
        b.push_str(&ctx.text_right(&rv, 1200.0, yt + 46.0));
    }

    // 右侧：这一行对应的公式
    // 换个行就换一张缩略图：整块硬切，不做交叉淡入，保证一帧只有一条公式
    let pv_idx = active;
    let pv_alpha = 0.02 + 0.98 * ramp(t, 0.8, 0.55);
    let pv_art = &previews[pv_idx];

    let pa = ramp(t, 0.3, 0.5);
    b.push_str(&rect(1252.0, 380.0, 476.0, 376.0, 14.0, C_PANEL, 0.95 * pa, Some((C_LINE, 1.4, pa))));
    b.push_str(&rect(1252.0, 380.0, 132.0, 3.0, 1.5, C_ACC, 0.9 * pa, None));
    let card_cap = if pv_idx == 3 { "示意：20 × 20，同一套流程" } else { "表中这一行对应的公式" };
    let cap = Spec { s: card_cap, size: 17.0, fill: C_SUB, alpha: 0.95 * pa, mono: false, bold: false, ls: 1.0 };
    b.push_str(&ctx.text_at(&cap, 1276.0, 410.0, false));
    b.push_str(&draw_art(ctx, pv_art, 1490.0, 586.0, 120.0, 400.0, 300.0, C_TXT, pv_alpha));

    // 结论
    let s1 = "同一个公式，快 2 到 5 倍；整套引擎只依赖一个字体库。";
    b.push_str(&txt(ctx, s1, 26.0, C_TXT, ramp(t, 13.0, 0.7), 200.0, 830.0, false));
    let s2 = "超大矩阵那一行只做了本引擎的单独测量，所以不写倍数。";
    b.push_str(&txt(ctx, s2, 24.0, C_DIM, ramp(t, 13.7, 0.7), 200.0, 876.0, false));

    b.push_str(&footnote(ctx, "数据来自同一台电脑，仓库里可一键复现", ramp(t, 16.4, 0.7)));
    b
}

// ---------------------------------------------------------------- 场景五：性能对比（柱状图）

const CHART: [(&str, f32, f32, &str); 3] = [
    ("单个变量", 0.71, 3.43, "4.8 倍"),
    ("分式", 1.48, 3.74, "2.5 倍"),
    ("矩阵 2×2", 3.91, 19.74, "5.0 倍"),
];

fn scene_chart(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "性能对比", "差多少倍，一眼看得出来", 1.0);

    b.push_str(&txt(ctx, "算一遍要多久（微秒，数字越小越快）", 22.0, C_FAINT, ramp(t, 0.2, 0.5), 200.0, 348.0, false));

    // 图例
    let la = ramp(t, 0.35, 0.5);
    b.push_str(&rect(1230.0, 336.0, 12.0, 12.0, 3.0, C_ACC, 0.95 * la, None));
    b.push_str(&txt(ctx, "本引擎", 20.0, C_DIM, la, 1252.0, 348.0, false));
    b.push_str(&rect(1450.0, 336.0, 12.0, 12.0, 3.0, "#5a6b85", 0.95 * la, None));
    b.push_str(&txt(ctx, "现有方案", 20.0, C_DIM, la, 1472.0, 348.0, false));

    let x0 = 620.0;
    let max_v = 19.74f32;
    let bw = 960.0f32;
    let sc = bw / max_v;

    let ga = ramp(t, 0.3, 0.5);
    for v in [0.0f32, 5.0, 10.0, 15.0, 20.0] {
        let x = x0 + v * sc;
        b.push_str(&rect(x, 392.0, 1.0, 420.0, 0.5, "#243248", 0.9 * ga, None));
    }

    for (i, (name, me, jl, ratio)) in CHART.iter().enumerate() {
        let y = 424.0 + i as f32 * 126.0;
        let st = 0.7 + i as f32 * 1.5;
        let p = ease_out_cubic((t - st) / 1.1);
        let a = ramp(t, st, 0.3);
        if a <= 0.004 {
            continue;
        }
        b.push_str(&txt(ctx, name, 26.0, C_TXT, a, 210.0, y + 34.0, false));

        let wm = me * sc * p;
        let wj = jl * sc * p;
        b.push_str(&rect(x0, y + 4.0, wm.max(1.0), 26.0, 4.0, C_ACC, 0.92 * a, None));
        b.push_str(&rect(x0, y + 38.0, wj.max(1.0), 26.0, 4.0, "#5a6b85", 0.9 * a, None));

        let va = ramp(t, st + 0.6, 0.35);
        b.push_str(&mono(ctx, &format!("{me:.2}"), 19.0, C_ACC, va, x0 + wm + 12.0, y + 25.0, false));
        b.push_str(&mono(ctx, &format!("{jl:.2}"), 19.0, C_DIM, va, x0 + wj + 12.0, y + 59.0, false));

        let rsp = Spec { s: ratio, size: 30.0, fill: C_ACC, alpha: va, mono: false, bold: true, ls: 0.0 };
        b.push_str(&ctx.text_right(&rsp, 1750.0, y + 40.0));
    }

    b.push_str(&txt(
        ctx,
        "超大矩阵 40 × 40：7202 个符号，算一遍 3.9 毫秒。",
        27.0,
        C_TXT,
        ramp(t, 6.2, 0.7),
        200.0,
        858.0,
        false,
    ));
    b.push_str(&txt(
        ctx,
        "求和这一项我们略慢（0.95 倍），照实说，没有删。",
        24.0,
        C_SUB,
        ramp(t, 8.2, 0.7),
        200.0,
        904.0,
        false,
    ));
    b.push_str(&footnote(ctx, "AMD Ryzen 5 5600 · Arch Linux · 每项 2000 次取中位数 · 计时含拆开与算位置", ramp(t, 4.0, 0.7)));
    b
}

// ---------------------------------------------------------------- 场景六：三步流程

const STEPS: [(&str, &str, &str); 3] = [
    ("拆开", "把一串文字拆成一个个符号", "分子、分母、上下限，各就各位"),
    ("算位置", "每个符号放哪、多大，一次算完", "上下标自动缩小，长公式自动折行"),
    ("画出来", "把位置交给手机，一个一个落下去", "先算位置，再照着位置画"),
];

fn scene_arch(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    scene_head(ctx, &mut b, "怎么做的", "三步：拆开、算位置、画出来", 1.0);

    let bw = 440.0;
    let bh = 210.0;
    let gap = 80.0;
    let x0 = (FW - (3.0 * bw + 2.0 * gap)) / 2.0;
    let y0 = 404.0;

    for (i, (title, l1, l2)) in STEPS.iter().enumerate() {
        let st = 0.7 + i as f32 * 0.55;
        let a = ramp(t, st, 0.5);
        let x = x0 + i as f32 * (bw + gap);
        let hot = ping(t, 5.5 + i as f32 * 4.6, 3.2);
        let dy = 14.0 * (1.0 - ramp(t, st, 0.55));

        b.push_str(&rect(x, y0 + dy, bw, bh, 16.0, C_PANEL, (0.9 + 0.08 * hot) * a, Some(("#28374f", 1.5, a))));
        if hot > 0.004 {
            b.push_str(&rect(x, y0, bw, bh, 16.0, C_ACC, 0.07 * hot * a, Some((C_ACC, 2.0, 0.85 * hot * a))));
        }
        b.push_str(&mono(ctx, &format!("0{}", i + 1), 20.0, C_ACC, 0.9 * a, x + 28.0, y0 + dy + 40.0, false));
        let ts = Spec { s: title, size: 36.0, fill: C_TXT, alpha: a, mono: false, bold: true, ls: 1.0 };
        b.push_str(&ctx.text_at(&ts, x + 28.0, y0 + dy + 96.0, false));
        b.push_str(&txt(ctx, l1, 20.0, C_DIM, 0.95 * a, x + 28.0, y0 + dy + 140.0, false));
        b.push_str(&txt(ctx, l2, 19.0, C_SUB, 0.9 * a, x + 28.0, y0 + dy + 174.0, false));

        // 箭头
        if i < 2 {
            let aa = ramp(t, st + 0.35, 0.35);
            let ax0 = x + bw + 12.0;
            let ax1 = x + bw + gap - 14.0;
            let cy = y0 + bh / 2.0;
            b.push_str(&line(ax0, cy, ax1 - 6.0, cy, "#6d84a8", 3.0, 0.95 * aa));
            let _ = write!(
                b,
                r##"<path d="M{:.1} {:.1} L{:.1} {:.1} L{:.1} {:.1} Z" fill="#46587a" fill-opacity="{:.3}"/>"##,
                ax1,
                cy,
                ax1 - 12.0,
                cy - 7.0,
                ax1 - 12.0,
                cy + 7.0,
                aa
            );
        }
    }

    // 一句话把流程再讲一遍
    b.push_str(&txt(
        ctx,
        "你写的一行公式  →  每个字的位置和大小  →  屏幕上的公式",
        30.0,
        C_TXT,
        ramp(t, 19.0, 0.7),
        960.0,
        700.0,
        true,
    ));
    b.push_str(&txt(ctx, "2900 行 Rust · 84 项测试 · 只依赖一个字体解析库", 26.0, C_DIM, ramp(t, 21.0, 0.7), 960.0, 760.0, true));
    b.push_str(&txt(ctx, "不用 Java，不用浏览器内核；装进 App 只有几百 KB", 26.0, C_DIM, ramp(t, 23.0, 0.7), 960.0, 808.0, true));
    b.push_str(&txt(ctx, "三步都是纯计算：同样的输入，永远得到同样的结果", 24.0, C_SUB, ramp(t, 25.5, 0.7), 960.0, 858.0, true));

    b.push_str(&footnote(ctx, "引擎只算位置，不画像素；怎么画由手机上的程序照着做", ramp(t, 12.0, 0.8)));
    b
}

// ---------------------------------------------------------------- 场景七：结尾

fn scene_end(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let over = Spec { s: "仓库", size: 24.0, fill: C_ACC, alpha: ramp(t, 0.1, 0.5), mono: false, bold: false, ls: 10.0 };
    b.push_str(&ctx.text_at(&over, 960.0, 372.0, true));

    let repo = Spec { s: "mathengine", size: 104.0, fill: C_TXT, alpha: ramp(t, 0.25, 0.6), mono: true, bold: true, ls: 0.0 };
    b.push_str(&ctx.text_at(&repo, 960.0, 492.0, true));

    b.push_str(&txt(ctx, "Rust 从零实现 · 2900 行 · 只依赖一个字体解析库", 28.0, C_DIM, ramp(t, 0.65, 0.6), 960.0, 562.0, true));

    let la = ramp(t, 0.95, 0.6);
    let lic = Spec { s: "Apache-2.0", size: 28.0, fill: C_ACC, alpha: la, mono: true, bold: true, ls: 2.0 };
    let lw = ctx.tw(&lic);
    b.push_str(&rect(960.0 - lw / 2.0 - 32.0, 604.0, lw + 64.0, 54.0, 27.0, "#0d2a26", 0.9 * la, Some((C_ACC, 1.6, 0.65 * la))));
    b.push_str(&ctx.text_at(&lic, 960.0, 641.0, true));

    b.push_str(&txt(ctx, "源码与数据可复现", 32.0, C_TXT, ramp(t, 1.35, 0.6), 960.0, 742.0, true));

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
    b.push_str(&footnote(ctx, "本片为无声版：画面逐帧预渲染，配乐与旁白另行混音", ramp(t, 2.4, 0.6)));
    b
}

// ---------------------------------------------------------------- 公式绘制

struct Place {
    k: f32,
    left: f32,
    baseline: f32,
}

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

fn place(art: &Art, ctx: &Ctx, cx: f32, cy: f32, base: f32, max_w: f32, max_h: f32) -> Place {
    let k = art_fit(art, ctx, base, max_w, max_h);
    let hd = (art.height + art.depth) * k;
    Place { k, left: cx - art.width * k / 2.0, baseline: cy - hd / 2.0 + art.height * k }
}

/// 静止的一条公式：一个组，组内每个字按布局坐标放好。
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
    let p = place(art, ctx, cx, cy, base_size, max_w, max_h);

    let mut b = format!(
        r#"<g data-formula="1" transform="translate({:.2} {:.2}) scale({:.5})" fill="{fill}" fill-opacity="{alpha:.3}">"#,
        p.left, p.baseline, p.k
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

/// 演示用：字按布局顺序一个个落下来，横线随后补上。
///
/// 始终输出一个带 `data-formula="1"` 的组（哪怕这一帧还没落下任何字），
/// 这样「一帧一条公式」的断言才数得准。
#[allow(clippy::too_many_arguments)]
fn reveal_art(
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
    let dots = n <= 40;

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
        let py = p.baseline - gl.y * p.k - 90.0 * (1.0 - e);
        let sc = p.k * gl.scale * (1.0 + 0.06 * (1.0 - e));
        let _ = write!(
            g,
            r#"<path transform="translate({px:.2} {py:.2}) scale({sc:.5})" fill="{fill}" fill-opacity="{a:.3}" d="{}"/>"#,
            gl.d
        );
        // 落位瞬间点一颗小亮点，给出逐个落下的节奏
        let land = st + drop;
        if dots && t > land {
            let da = (1.0 - (t - land) / 0.55).clamp(0.0, 1.0) * 0.85;
            g.push_str(&dot(px + 4.0, p.baseline - gl.y * p.k, 2.2, C_ACC, da));
        }
    }

    let r0 = t0 + n as f32 * stagger + 0.12;
    for (j, r) in art.rules.iter().enumerate() {
        let st = r0 + j as f32 * 0.30;
        let pr = ease_out_cubic((t - st) / 0.55);
        if pr <= 0.0 {
            continue;
        }
        let _ = write!(
            g,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{fill}" fill-opacity="{:.3}"/>"#,
            p.left + r.x * p.k,
            p.baseline - r.y * p.k,
            r.w * p.k * pr,
            (r.th * p.k).max(1.4),
            0.98
        );
    }
    g.push_str("</g>");
    g
}

/// 耗时写法：一千微秒以上换毫秒。
fn us_text(us: f64) -> String {
    if us >= 1000.0 {
        format!("{:.1} 毫秒", us / 1000.0)
    } else {
        format!("{:.1} 微秒", us)
    }
}

/// 当场测一遍：解析 + 算位置，取中位数。画面上写的数字就是这么来的。
fn bench_layout(font: &MathFont, latex: &str, iters: usize) -> f64 {
    let ast = parse(latex).expect("解析待测公式");
    let mut v: Vec<f64> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        let l = layout(font, &ast);
        std::hint::black_box(&l);
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

// ---------------------------------------------------------------- 合成一帧

struct Reel {
    deco: Art,
    shows: Vec<Shot>,
    mats: Vec<Shot>,
    previews: Vec<Art>,
}

fn secs(i: usize, start: usize) -> f32 {
    (i - start) as f32 / FPS
}

fn scene_body(ctx: &Ctx, r: &Reel, idx: usize, i: usize) -> String {
    match idx {
        0 => scene_cover(ctx, &r.deco, secs(i, S_COVER)),
        1 => scene_show(ctx, &r.shows, i - S_SHOW),
        2 => scene_matrix(ctx, &r.mats, i - S_MATRIX),
        3 => {
            let local = i - S_PERF;
            if local < TBL_LEN {
                scene_table(ctx, &r.previews, local as f32 / FPS)
            } else {
                scene_chart(ctx, (local - TBL_LEN) as f32 / FPS)
            }
        }
        4 => scene_arch(ctx, secs(i, S_ARCH)),
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

    // 硬保证：一帧里最多一条公式（封面装饰 / 演示 / 矩阵 / 表格缩略图各 1 条）
    let n_formula = body.matches(r#"data-formula="1""#).count();
    let allow = usize::from(matches!(idx, 0 | 1 | 2 | 3));
    assert!(n_formula <= allow, "第 {i} 帧出现 {n_formula} 条公式，最多允许 {allow}");
    // 演示、矩阵、表格缩略图这三处必须恰好一条；柱状图不该有任何公式
    let exact = matches!(idx, 1 | 2) || (idx == 3 && i - S_PERF < TBL_LEN);
    if exact {
        assert_eq!(n_formula, 1, "第 {i} 帧（场景 {idx}）应有且只有一条公式");
    }
    // 硬保证：一帧里最多一套字幕
    let n_cap = body.matches(r#"data-cap="1""#).count();
    assert!(n_cap <= 1, "第 {i} 帧出现 {n_cap} 套字幕，最多允许 1");

    // 硬保证：画面上不出现实现内部的行话（抽帧检查，扫全片成本太高）
    if i % 11 == 0 {
        for w in BANNED {
            assert!(!body.contains(w), "第 {i} 帧出现禁用词「{w}」");
        }
    }

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

    let deco = build_art(&font, &mut cache, r"e^{i\pi}+1=0").expect("封面装饰公式");

    let mut shows = Vec::new();
    for (latex, name, note) in SHOW_SRC {
        let art = build_art(&font, &mut cache, latex).unwrap_or_else(|e| {
            eprintln!("FAIL {latex}: {e}");
            std::process::exit(1);
        });
        let n = art.glyphs.len();
        let us = bench_layout(&font, latex, if n > 1500 { 40 } else { 400 });
        println!("OK   {n:>5} 个字  {:>12}  {latex}", us_text(us));
        shows.push(Shot { name, note, latex: latex.to_string(), art, micros: us });
    }

    let mut mats = Vec::new();
    for (i, n) in [2usize, 5, 20].into_iter().enumerate() {
        let latex = matrix_digits(n);
        let art = build_art(&font, &mut cache, &latex).unwrap_or_else(|e| {
            eprintln!("FAIL {latex}: {e}");
            std::process::exit(1);
        });
        let glyphs = art.glyphs.len();
        let us = bench_layout(&font, &latex, if glyphs > 1500 { 40 } else { 400 });
        println!("OK   {glyphs:>5} 个字  {:>12}  {latex}", us_text(us));
        mats.push(Shot { name: MAT_NAMES[i].0, note: MAT_NAMES[i].1, latex, art, micros: us });
    }
    // 表格每一行配一张缩略图；超大矩阵那一行用 20×20 示意（40×40 缩到 476px 宽只剩噪点）
    let dense = build_art(&font, &mut cache, &matrix_digits(20)).expect("20×20 缩略图");
    println!("OK   {:>5} 个字  表格用 20×20 缩略图", dense.glyphs.len());
    let previews = vec![
        build_art(&font, &mut cache, "x").expect("缩略图 x"),
        build_art(&font, &mut cache, r"\frac{a+b}{c}").expect("缩略图 分式"),
        build_art(&font, &mut cache, &matrix_digits(2)).expect("缩略图 2×2"),
        dense,
    ];

    let reel = Reel { deco, shows, mats, previews };

    for i in 0..TOTAL {
        let svg = frame(&ctx, &reel, i);
        fs::write(format!("{dir}/f{i:05}.svg"), svg).expect("写 SVG");
        if i % 600 == 0 {
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
