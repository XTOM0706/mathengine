//! `cargo run --release --example video_frames -- 输出目录`：生成演示视频的逐帧 SVG。
//!
//! 每一帧都是一张 1920×1080 的完整 SVG：公式部分不是截图，而是调用
//! `mathengine::layout` 现算布局、再向字体索取字形轮廓画出来的路径——和
//! `examples/render_svg.rs` 走的是同一份数据。中文与说明文字用 SVG `<text>`
//! 交给 rsvg / Pango 排版，避免 ffmpeg drawtext 的字体路径问题。
//!
//! 宽度测量直接读字体 `hmtx`：中文用 Noto Sans CJK SC，拉丁用 JetBrains Maple
//! Mono，这样居中与分段文字的落点不靠猜。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

const FW: f32 = 1920.0;
const FH: f32 = 1080.0;
const FPS: f32 = 30.0;

// 配色：深底 + 青绿强调，公式用近白。
const C_TXT: &str = "#eef2f9";
const C_DIM: &str = "#94a2b8";
const C_FAINT: &str = "#5f6d83";
/// 卡片内小字：比 C_FAINT 亮一档，投影也看得清。
const C_SUB: &str = "#7d8ba2";
const C_LINE: &str = "#1d2939";
const C_PANEL: &str = "#0f1622";
const C_ACC: &str = "#2ee6c5";
const C_WARN: &str = "#ff7a6b";
const C_AMBER: &str = "#f7b955";

const SANS: &str = "Noto Sans CJK SC";
const MONO: &str = "JetBrains Maple Mono";

// 场景切分（帧）
const F_PROBLEM: usize = 120;
const F_ARCH: usize = 300;
const F_DEMO: usize = 540;
const F_DATA: usize = 1020;
const F_END: usize = 1260;
const TOTAL: usize = 1380;

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

/// 一条公式的绘制数据，坐标是字体单位（1 em = upem），画的时候整体缩放。
struct Art {
    paths: Vec<String>,
    rules: Vec<(f32, f32, f32, f32)>,
    width: f32,
    height: f32,
    depth: f32,
    glyphs: usize,
}

fn build_art(
    font: &MathFont,
    cache: &mut HashMap<u16, String>,
    latex: &str,
) -> Result<Art, String> {
    let ast = parse(latex).map_err(|e| format!("{e:?}"))?;
    let l = layout(font, &ast);
    let face = font.face_ref().ok_or_else(|| "字体没有 face".to_string())?;
    if l.glyphs.is_empty() {
        return Err("布局结果为空".to_string());
    }

    let mut paths = Vec::with_capacity(l.glyphs.len());
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
        let d = &cache[&gid.0];
        paths.push(format!(
            r#"<path transform="translate({:.1} {:.1}) scale({:.3})" d="{}"/>"#,
            g.x, -g.y, g.scale, d
        ));
    }
    let rules = l
        .rules
        .iter()
        .map(|r| (r.x, -r.y, r.width, r.thickness.max(1.0)))
        .collect();

    Ok(Art {
        paths,
        rules,
        width: l.width,
        height: l.height,
        depth: l.depth,
        glyphs: l.glyphs.len(),
    })
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
    let brand = spec("mathengine", 22.0, C_DIM, 0.55);
    let brand = Spec { mono: true, ..brand };
    b.push_str(&ctx.text_at(&brand, 182.0, 74.0, false));
    let sec = spec(section, 20.0, C_FAINT, 0.85);
    let sec = Spec { ls: 3.0, ..sec };
    b.push_str(&ctx.text_at(&sec, 1758.0, 74.0, true));

    // 页脚进度条 + 分段刻度
    let bx = 160.0;
    let bw = 1600.0;
    b.push_str(&rect(bx, 1030.0, bw, 2.0, 1.0, "#1b2434", 1.0, None));
    b.push_str(&rect(bx, 1030.0, bw * clamp01(progress), 2.0, 1.0, C_ACC, 0.75, None));
    for f in [F_PROBLEM, F_ARCH, F_DEMO, F_DATA, F_END] {
        let tx = bx + bw * (f as f32 / (TOTAL - 1) as f32);
        b.push_str(&rect(tx - 0.5, 1026.0, 1.0, 10.0, 0.5, C_DIM, 0.28, None));
    }
    b
}

// ---------------------------------------------------------------- 场景一：封面

fn scene_cover(ctx: &Ctx, art: &Art, t: f32) -> String {
    let mut b = String::new();
    let out = 1.0; // 场景边界的淡出由 frame() 统一做交叉淡化

    let over = Spec {
        s: "RUST 从零实现 · 数学模式排版",
        size: 24.0,
        fill: C_ACC,
        alpha: ramp(t, 0.10, 0.60) * out,
        mono: false,
        bold: false,
        ls: 8.0,
    };
    b.push_str(&ctx.text_at(&over, 960.0, 300.0, true));

    let title = Spec {
        s: "mathengine",
        size: 152.0,
        fill: C_TXT,
        alpha: ramp(t, 0.28, 0.75) * out,
        mono: true,
        bold: true,
        ls: 0.0,
    };
    let dy = 26.0 * (1.0 - ramp(t, 0.28, 0.85));
    b.push_str(&ctx.text_at(&title, 960.0, 470.0 + dy, true));

    let sub = spec("面向移动设备的轻量 LaTeX 排版引擎", 48.0, C_DIM, ramp(t, 0.85, 0.7) * out);
    b.push_str(&ctx.text_at(&sub, 960.0, 560.0 + dy * 0.5, true));

    // 分隔线由中心向两侧展开
    let grow = ramp(t, 1.15, 0.8);
    if grow > 0.01 {
        let w = 300.0 * grow;
        b.push_str(&rect(960.0 - w / 2.0, 612.0, w, 2.0, 1.0, C_ACC, 0.85 * out, None));
    }

    let contest = Spec {
        s: "第 38 届苏州市青少年科技创新大赛",
        size: 28.0,
        fill: C_DIM,
        alpha: ramp(t, 1.55, 0.7) * out,
        mono: false,
        bold: false,
        ls: 3.0,
    };
    b.push_str(&ctx.text_at(&contest, 960.0, 682.0, true));

    let deco = ramp(t, 2.10, 0.9) * out;
    b.push_str(&draw_art(ctx, art, 960.0, 862.0, 64.0, 1500.0, 190.0, C_TXT, 0.42 * deco));
    b
}

// ---------------------------------------------------------------- 场景二：问题

fn scene_head(ctx: &Ctx, b: &mut String, overline: &str, title: &str, _t: f32, a: f32) {
    let over = Spec {
        s: overline,
        size: 22.0,
        fill: C_ACC,
        alpha: a,
        mono: false,
        bold: false,
        ls: 8.0,
    };
    b.push_str(&ctx.text_at(&over, 200.0, 214.0, false));
    let ttl = Spec {
        s: title,
        size: 52.0,
        fill: C_TXT,
        alpha: a,
        mono: false,
        bold: true,
        ls: 0.0,
    };
    b.push_str(&ctx.text_at(&ttl, 200.0, 282.0, false));
    let bar = a;
    b.push_str(&rect(170.0, 240.0, 5.0, 50.0, 2.5, C_ACC, bar, None));
}

fn problem_row(ctx: &Ctx, y: f32, segs: &[(&str, bool, bool, &str)], t: f32, delay: f32, out: f32) -> String {
    let a = ramp(t, delay, 0.45) * out;
    if a <= 0.004 {
        return String::new();
    }
    let mut b = String::new();
    let dx = 26.0 * (1.0 - ramp(t, delay, 0.55));
    b.push_str(&rect(200.0, y, 1520.0, 108.0, 14.0, C_PANEL, 0.92 * a, Some((C_LINE, 1.5, a))));
    // 手画的叉，不依赖字体里有没有 ✕
    let cx = 268.0;
    let cy = y + 54.0;
    b.push_str(&format!(
        r#"<circle cx="{cx}" cy="{cy}" r="21" fill="none" stroke="{C_WARN}" stroke-width="2.5" stroke-opacity="{:.3}"/>"#,
        0.85 * a
    ));
    b.push_str(&line(cx - 8.0, cy - 8.0, cx + 8.0, cy + 8.0, C_WARN, 2.5, 0.85 * a));
    b.push_str(&line(cx - 8.0, cy + 8.0, cx + 8.0, cy - 8.0, C_WARN, 2.5, 0.85 * a));

    let mut px = 336.0 + dx;
    for (s, mono, bold, fill) in segs {
        let sp = Spec { s, size: 36.0, fill, alpha: a, mono: *mono, bold: *bold, ls: 0.0 };
        b.push_str(&ctx.text_at(&sp, px, y + 68.0, false));
        px += ctx.tw(&sp) + 18.0;
    }
    b
}

fn scene_problem(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let out = 1.0; // 场景边界的淡出由 frame() 统一做交叉淡化
    scene_head(ctx, &mut b, "问题", "为什么还要再造一个", t, out);

    b.push_str(&problem_row(
        ctx,
        372.0,
        &[
            ("jlatexmath", false, true, C_TXT),
            ("GPL-2.0，许可传染 + 依赖 JVM", false, false, C_DIM),
        ],
        t,
        0.55,
        out,
    ));
    b.push_str(&problem_row(
        ctx,
        512.0,
        &[
            ("KaTeX / MathJax", false, true, C_TXT),
            ("需要 WebView 或 JS 运行时", false, false, C_DIM),
        ],
        t,
        1.55,
        out,
    ));

    let note = spec(
        "两条路在手表与低端 Android 上都走不通：要么整个应用被 GPL 传染，要么必须背一个 WebView。",
        28.0,
        C_FAINT,
        ramp(t, 3.0, 0.6) * out,
    );
    b.push_str(&ctx.text_at(&note, 200.0, 700.0, false));

    let note2 = spec(
        "我们要的是：一个不依赖任何运行时、能把布局数据直接交给宿主的数学排版器。",
        28.0,
        C_DIM,
        ramp(t, 3.8, 0.6) * out,
    );
    b.push_str(&ctx.text_at(&note2, 200.0, 756.0, false));
    b
}

// ---------------------------------------------------------------- 场景三：架构

fn scene_arch(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let out = 1.0; // 场景边界的淡出由 frame() 统一做交叉淡化
    scene_head(ctx, &mut b, "架构", "从 LaTeX 串到绘制指令", t, out);

    let steps: [(&str, bool); 5] =
        [("LaTeX", true), ("token", true), ("AST", true), ("TeX Box 布局", false), ("扁平绘制指令", false)];
    let fs = 34.0;
    let pad = 34.0;
    let gap = 62.0;
    let boxh = 86.0;
    let y0 = 462.0;

    let widths: Vec<f32> = steps
        .iter()
        .map(|(s, mono)| {
            let sp = Spec { s, size: fs, fill: C_TXT, alpha: 1.0, mono: *mono, bold: false, ls: 0.0 };
            ctx.tw(&sp) + pad * 2.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + gap * (steps.len() - 1) as f32;
    let mut x = (FW - total) / 2.0;

    for (i, (s, mono)) in steps.iter().enumerate() {
        let a = ramp(t, 0.45 + 0.52 * i as f32, 0.42) * out;
        let w = widths[i];
        if i > 0 {
            let aa = ramp(t, 0.45 + 0.52 * i as f32 - 0.24, 0.30) * out;
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
            b.push_str(&rect(
                x,
                y0 + dy,
                w,
                boxh,
                12.0,
                C_PANEL,
                0.95 * a,
                Some(("#28374f", 1.5, a)),
            ));
            let numtxt = format!("{:02}", i + 1);
            let num = Spec { s: &numtxt, size: 18.0, fill: C_FAINT, alpha: 0.85 * a, mono: true, bold: false, ls: 1.0 };
            b.push_str(&ctx.text_at(&num, x + w / 2.0, y0 + dy - 16.0, true));
            let label = Spec { s, size: fs, fill: C_TXT, alpha: a, mono: *mono, bold: false, ls: 0.0 };
            b.push_str(&ctx.text_at(&label, x + w / 2.0, y0 + dy + 56.0, true));
        }
        x += w + gap;
    }

    let l1 = spec(
        "2900 行 Rust · 68 个单元测试 · 只依赖 ttf-parser",
        30.0,
        C_DIM,
        ramp(t, 3.50, 0.6) * out,
    );
    b.push_str(&ctx.text_at(&l1, 200.0, 690.0, false));
    let l2 = spec(
        "输出：字形码位 / 坐标 / 缩放 + 分数线与根号横线矩形 —— 宿主不必理解 TeX。",
        28.0,
        C_FAINT,
        ramp(t, 4.30, 0.6) * out,
    );
    b.push_str(&ctx.text_at(&l2, 200.0, 748.0, false));
    let l3 = spec(
        "同一份布局数据，可编成 .so 嵌进 Android，也可编成 JNI 库。",
        28.0,
        C_FAINT,
        ramp(t, 5.20, 0.6) * out,
    );
    b.push_str(&ctx.text_at(&l3, 200.0, 806.0, false));
    b
}

// ---------------------------------------------------------------- 场景四：实时渲染

const DEMOS: [(&str, &str); 6] = [
    (
        r"\frac{-b\pm\sqrt{b^2-4ac}}{2a}",
        "分式 + 根号 + 上标",
    ),
    (
        r"\int_{-\infty}^{\infty} e^{-x^2}\,dx=\sqrt{\pi}",
        "积分上下限 + 指数里的上标",
    ),
    (
        r"\sum_{i=1}^{n} i^2=\frac{n(n+1)(2n+1)}{6}",
        "求和带上下限 + 分式",
    ),
    (
        r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}^{-1}",
        "pmatrix 环境 + 上标",
    ),
    (
        r"\mathbb{R}^n\to\mathbb{C},\quad \mathfrak{g}",
        "字符映射：\\mathbb 与 \\mathfrak",
    ),
    (
        r"\oint_{\partial\Omega}\vec{B}\cdot d\vec{S}=\mu_0 I",
        "环路积分 + 矢量重音",
    ),
];

fn scene_demo(ctx: &Ctx, arts: &[Art], t: f32, local_frame: isize) -> String {
    let mut b = String::new();
    let out = 1.0; // 场景边界的淡出由 frame() 统一做交叉淡化
    scene_head(ctx, &mut b, "实时渲染", "引擎现算，不是截图", t, out);

    let tag = Spec {
        s: "mathengine::svg::render(&font, latex, size)",
        size: 21.0,
        fill: C_FAINT,
        alpha: 0.85 * out,
        mono: true,
        bold: false,
        ls: 0.0,
    };
    b.push_str(&ctx.text_right(&tag, 1758.0, 214.0));

    // 公式之间硬切：槽位按"场景内帧号"整除得到，整数运算不会在边界上飘一帧
    let slot_frames = 80usize;
    if (0..6 * slot_frames as isize).contains(&local_frame) {
        let cur = (local_frame as usize / slot_frames).min(5);
        let art = &arts[cur];
        b.push_str(&draw_art(ctx, art, 960.0, 556.0, 132.0, 1440.0, 360.0, C_TXT, out));

        let cap = spec(DEMOS[cur].1, 26.0, C_DIM, 0.95 * out);
        b.push_str(&ctx.text_at(&cap, 960.0, 792.0, true));

        let idxs = format!("{:02} / 06", cur + 1);
        let idx = Spec { s: &idxs, size: 21.0, fill: C_FAINT, alpha: out, mono: true, bold: false, ls: 1.0 };
        b.push_str(&ctx.text_at(&idx, 200.0, 792.0, false));

        let stat = format!("{} 字形 · {} 段横线", art.glyphs, art.rules.len());
        let st = Spec { s: &stat, size: 21.0, fill: C_FAINT, alpha: out, mono: true, bold: false, ls: 0.0 };
        b.push_str(&ctx.text_right(&st, 1758.0, 792.0));

        // 源码条：把输入原样摆出来，说明图是真的按这段 LaTeX 算的
        let src = spec(DEMOS[cur].0, 25.0, "#7d8ba2", out);
        let src = Spec { mono: true, ..src };
        let sw = ctx.tw(&src);
        b.push_str(&rect(960.0 - sw / 2.0 - 26.0, 836.0, sw + 52.0, 50.0, 10.0, "#0d1420", 0.95 * out, Some((C_LINE, 1.2, out))));
        b.push_str(&ctx.text_at(&src, 960.0, 868.0, true));
    }

    // 六格进度
    let n = 6.0;
    let pw = 148.0;
    let pg = 16.0;
    let totalw = n * pw + (n - 1.0) * pg;
    let x0 = (FW - totalw) / 2.0;
    let cur = ((local_frame.max(0) as usize) / slot_frames).min(5);
    for i in 0..6 {
        let x = x0 + i as f32 * (pw + pg);
        let base = rect(x, 962.0, pw, 4.0, 2.0, "#1b2434", 0.95 * out, None);
        b.push_str(&base);
        if i < cur {
            b.push_str(&rect(x, 962.0, pw, 4.0, 2.0, C_ACC, 0.42 * out, None));
        } else if i == cur {
            let p = clamp01(((local_frame.max(0) as usize) % slot_frames) as f32 / slot_frames as f32);
            b.push_str(&rect(x, 962.0, pw * p, 4.0, 2.0, C_ACC, 0.95 * out, None));
        }
    }
    b
}

// ---------------------------------------------------------------- 场景五：数据

fn stat_card(
    ctx: &Ctx,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    value: &str,
    color: &str,
    label: &str,
    label2: &str,
    sub: &str,
    t: f32,
    delay: f32,
    out: f32,
) -> String {
    let a = ramp(t, delay, 0.5) * out;
    if a <= 0.004 {
        return String::new();
    }
    let dy = 26.0 * (1.0 - ramp(t, delay, 0.6));
    let y = y + dy;
    let mut b = String::new();
    b.push_str(&rect(x, y, w, h, 16.0, C_PANEL, 0.95 * a, Some(("#233246", 1.5, a))));
    b.push_str(&rect(x, y, w, 4.0, 2.0, color, 0.9 * a, None));
    let v = Spec { s: value, size: 88.0, fill: color, alpha: a, mono: true, bold: true, ls: 0.0 };
    b.push_str(&ctx.text_at(&v, x + w / 2.0, y + 150.0, true));
    let l = spec(label, 27.0, C_TXT, a);
    b.push_str(&ctx.text_at(&l, x + w / 2.0, y + 202.0, true));
    if !label2.is_empty() {
        let l2 = spec(label2, 22.0, C_DIM, a);
        b.push_str(&ctx.text_at(&l2, x + w / 2.0, y + 234.0, true));
    }
    let s = spec(sub, 21.0, C_SUB, a * 0.95);
    b.push_str(&ctx.text_at(&s, x + w / 2.0, y + 268.0, true));
    b
}

fn scene_data(ctx: &Ctx, t: f32) -> String {
    let mut b = String::new();
    let out = 1.0; // 场景边界的淡出由 frame() 统一做交叉淡化
    scene_head(ctx, &mut b, "数据", "实测，不是估算", t, out);

    let w = 440.0;
    let h = 286.0;
    let gap = 58.0;
    let x0 = (FW - (3.0 * w + 2.0 * gap)) / 2.0;
    let y = 392.0;
    b.push_str(&stat_card(
        ctx, x0, y, w, h, "3.2 µs", C_ACC, "布局中位耗时", "", "30 000 次采样 · p95 8.6 µs", t, 0.35, out,
    ));
    b.push_str(&stat_card(
        ctx,
        x0 + w + gap,
        y,
        w,
        h,
        "5.0×",
        C_AMBER,
        "矩阵布局速度约为",
        "jlatexmath 的 5 倍",
        "同尺寸 pmatrix · 中位",
        t,
        1.85,
        out,
    ));
    b.push_str(&stat_card(
        ctx,
        x0 + 2.0 * (w + gap),
        y,
        w,
        h,
        "59/59",
        C_ACC,
        "真实文档公式全部渲染",
        "",
        "docs/cases-document.txt",
        t,
        3.35,
        out,
    ));

    let n1 = spec(
        "口径：jlatexmath 走公开入口 createTeXIcon，含图标对象开销；求和与积分基本持平，未做针对性调优。",
        24.0,
        C_DIM,
        ramp(t, 4.90, 0.7) * out,
    );
    b.push_str(&ctx.text_at(&n1, 200.0, 764.0, false));
    let n2 = spec(
        "Ryzen 5 5600 · Arch Linux · rustc 1.98 · opt-level=3 + lto · 计时含解析与布局",
        24.0,
        C_DIM,
        ramp(t, 5.60, 0.7) * out,
    );
    b.push_str(&ctx.text_at(&n2, 200.0, 812.0, false));
    b
}

// ---------------------------------------------------------------- 场景六：结尾

fn scene_end(ctx: &Ctx, _t: f32) -> String {
    let mut b = String::new();
    let over = Spec {
        s: "开源地址",
        size: 24.0,
        fill: C_ACC,
        alpha: 1.0,
        mono: false,
        bold: false,
        ls: 10.0,
    };
    b.push_str(&ctx.text_at(&over, 960.0, 392.0, true));

    let repo = Spec {
        s: "mathengine",
        size: 104.0,
        fill: C_TXT,
        alpha: 1.0,
        mono: true,
        bold: true,
        ls: 0.0,
    };
    b.push_str(&ctx.text_at(&repo, 960.0, 512.0, true));

    let line1 = spec("Rust 从零实现 · 2900 行 · 只依赖 ttf-parser", 30.0, C_DIM, 1.0);
    b.push_str(&ctx.text_at(&line1, 960.0, 586.0, true));

    // 许可徽章
    let lic = Spec {
        s: "AGPL-3.0",
        size: 30.0,
        fill: C_ACC,
        alpha: 1.0,
        mono: true,
        bold: true,
        ls: 2.0,
    };
    let lw = ctx.tw(&lic);
    let a = lic.alpha;
    b.push_str(&rect(960.0 - lw / 2.0 - 34.0, 638.0, lw + 68.0, 58.0, 29.0, "#0d2a26", 0.9 * a, Some((C_ACC, 1.6, 0.65 * a))));
    b.push_str(&ctx.text_at(&lic, 960.0, 678.0, true));

    let thx = spec("谢谢", 26.0, C_FAINT, 1.0);
    b.push_str(&ctx.text_at(&thx, 960.0, 800.0, true));
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
    if alpha <= 0.004 {
        return String::new();
    }
    let mut size = base_size;
    if art.width > 0.0 {
        size = size.min(max_w * ctx.upem / art.width);
    }
    let hd = art.height + art.depth;
    if hd > 0.0 {
        size = size.min(max_h * ctx.upem / hd);
    }
    let k = size / ctx.upem;
    let left = cx - art.width * k / 2.0;
    let top = cy - hd * k / 2.0;
    let baseline = top + art.height * k;

    let mut b = format!(
        r#"<g transform="translate({left:.2} {baseline:.2}) scale({k:.5})" fill="{fill}" fill-opacity="{alpha:.3}">"#
    );
    for p in &art.paths {
        b.push_str(p);
    }
    for (x, y, w, th) in &art.rules {
        let _ = write!(
            b,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{th:.1}"/>"#
        );
    }
    b.push_str("</g>");
    b
}

// ---------------------------------------------------------------- 合成一帧

/// 六个场景的起始帧与页眉标题。
const SCENES: [(usize, &str); 6] = [
    (0, "封面"),
    (F_PROBLEM, "问题"),
    (F_ARCH, "架构"),
    (F_DEMO, "实时渲染"),
    (F_DATA, "数据"),
    (F_END, "开源"),
];

fn scene_body(ctx: &Ctx, arts: &[Art], deco: &Art, idx: usize, i: usize) -> String {
    let t = (i as f32 - SCENES[idx].0 as f32) / FPS;
    match idx {
        0 => scene_cover(ctx, deco, t),
        1 => scene_problem(ctx, t),
        2 => scene_arch(ctx, t),
        3 => scene_demo(ctx, arts, t, i as isize - SCENES[3].0 as isize),
        4 => scene_data(ctx, t),
        _ => scene_end(ctx, t),
    }
}

fn frame(ctx: &Ctx, arts: &[Art], deco: &Art, i: usize) -> String {
    let mut idx = 0;
    for (k, (start, _)) in SCENES.iter().enumerate() {
        if i >= *start {
            idx = k;
        }
    }

    // 交界处叠化 0.13 秒（4 帧），旧场景上移、新场景下移 22px：
    // 两套标题最多同框 1 帧，看得出是在换页
    const XF: f32 = 0.13;
    let half = (XF * FPS / 2.0).round() as usize;
    let mut section = SCENES[idx].1;
    let mut drawn = vec![idx];
    let mut body = String::new();
    let blend = |a: f32, dy: f32, b: String| -> String {
        if a <= 0.004 {
            String::new()
        } else {
            format!(r#"<g opacity="{a:.3}" transform="translate(0 {dy:.1})">{b}</g>"#)
        }
    };

    // 渲染演示段两头直接硬切：不让公式与字幕在换场时被淡化或与邻场内容同框
    let near_demo = idx == 3
        || (idx == 2 && i >= SCENES[3].0.saturating_sub(half))
        || (idx == 4 && i < SCENES[4].0 + half);
    if near_demo {
        body = scene_body(ctx, arts, deco, idx, i);
    } else if idx > 0 && i < SCENES[idx].0 + half {
        let b = SCENES[idx].0;
        let p = smooth((i as f32 - (b as f32 - half as f32)) / (2.0 * half as f32));
        body.push_str(&blend(1.0 - p, -22.0 * p, scene_body(ctx, arts, deco, idx - 1, i)));
        body.push_str(&blend(p, 22.0 * (1.0 - p), scene_body(ctx, arts, deco, idx, i)));
        drawn.push(idx - 1);
        if p < 0.5 {
            section = SCENES[idx - 1].1;
        }
    } else if idx + 1 < SCENES.len() && i >= SCENES[idx + 1].0.saturating_sub(half) {
        let b = SCENES[idx + 1].0;
        let p = smooth((i as f32 - (b as f32 - half as f32)) / (2.0 * half as f32));
        body.push_str(&blend(1.0 - p, -22.0 * p, scene_body(ctx, arts, deco, idx, i)));
        body.push_str(&blend(p, 22.0 * (1.0 - p), scene_body(ctx, arts, deco, idx + 1, i)));
        drawn.push(idx + 1);
        if p >= 0.5 {
            section = SCENES[idx + 1].1;
        }
    } else {
        body = scene_body(ctx, arts, deco, idx, i);
    }

    // 硬保证：一帧里最多一条公式（封面装饰公式 / 渲染演示各 1 条，其余场景 0 条）
    let n_formula = body.matches(r#"<g transform="translate("#).count();
    let allow = usize::from(drawn.iter().any(|&k| k == 0 || k == 3));
    assert!(n_formula <= allow, "第 {i} 帧出现 {n_formula} 条公式，最多允许 {allow}");
    if idx == 3 && drawn.len() == 1 {
        assert_eq!(n_formula, 1, "渲染演示第 {i} 帧应有且只有一条公式");
    }

    let prog = i as f32 / (TOTAL - 1) as f32;
    let mut svg = String::with_capacity(24 * 1024);
    let _ = write!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{FW:.0}" height="{FH:.0}" viewBox="0 0 {FW:.0} {FH:.0}">"#
    );
    svg.push_str(&background(prog, section, ctx));
    svg.push_str(&body);
    // 收尾 0.6 秒压到黑，避免片子硬切结束
    let fade = 1.0 - smooth((i as f32 - (TOTAL as f32 - 18.0)) / 17.0);
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
    let mut arts = Vec::new();
    for (latex, _) in DEMOS {
        match build_art(&font, &mut cache, latex) {
            Ok(a) => {
                println!("OK   {:>2} 字形 {:>2} 横线  {}", a.glyphs, a.rules.len(), latex);
                arts.push(a);
            }
            Err(e) => {
                eprintln!("FAIL {latex}: {e}");
                std::process::exit(1);
            }
        }
    }
    let deco = build_art(&font, &mut cache, r"e^{i\pi}+1=0").expect("封面装饰公式");

    for i in 0..TOTAL {
        let svg = frame(&ctx, &arts, &deco, i);
        fs::write(format!("{dir}/f{i:05}.svg"), svg).expect("写 SVG");
        if i % 300 == 0 {
            println!("  帧 {i}/{TOTAL}");
        }
    }
    println!("共 {TOTAL} 帧 → {dir}");

    let warn = ctx.warn.borrow();
    if !warn.is_empty() {
        println!("检查提示：");
        for w in warn.iter() {
            println!("  {w}");
        }
    }
}
