//! `cargo run --example render_svg -- "<latex>" out.svg [字号]`：把布局结果画成 SVG。
//!
//! 不用任何光栅化库：直接向字体要字形轮廓，换算成 SVG path。
//! 这样 README 里的效果图与引擎本身用的是同一套数据，不存在「图是别处画的」。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;
use std::fmt::Write as _;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

struct SvgPath {
    d: String,
    scale: f32,
}

impl SvgPath {
    fn new(scale: f32) -> Self {
        Self { d: String::new(), scale }
    }
    fn p(&mut self, cmd: char, x: f32, y: f32) {
        let _ = write!(self.d, "{cmd}{:.2} {:.2} ", x * self.scale, -y * self.scale);
    }
}

impl OutlineBuilder for SvgPath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.p('M', x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.p('L', x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(
            self.d,
            "Q{:.2} {:.2} {:.2} {:.2} ",
            x1 * self.scale,
            -y1 * self.scale,
            x * self.scale,
            -y * self.scale
        );
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(
            self.d,
            "C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} ",
            x1 * self.scale,
            -y1 * self.scale,
            x2 * self.scale,
            -y2 * self.scale,
            x * self.scale,
            -y * self.scale
        );
    }
    fn close(&mut self) {
        self.d.push('Z');
        self.d.push(' ');
    }
}

fn glyph_path(face: &Face, gid: u16, scale: f32) -> Option<String> {
    let mut b = SvgPath::new(scale);
    face.outline_glyph(GlyphId(gid), &mut b)?;
    if b.d.is_empty() {
        None
    } else {
        Some(b.d)
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let latex = args.next().unwrap_or_else(|| r"E = mc^2".to_string());
    let out = args.next().unwrap_or_else(|| "out.svg".to_string());
    let size: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(64.0);

    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体文件应存在");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("字体应能解析");
    let ast = match parse(&latex) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("解析失败：{e:?}");
            std::process::exit(1);
        }
    };
    let l = layout(&font, &ast);

    let upem = font.units_per_em();
    let k = size / upem;
    let pad = size * 0.35;
    let w = l.width * k + pad * 2.0;
    let h = (l.height + l.depth) * k + pad * 2.0;
    let base_y = pad + l.height * k;

    let face = font.face_ref().expect("字体 face");
    let mut body = String::new();

    for g in &l.glyphs {
        let Some(gid) = font.glyph_id(g.ch) else { continue };
        // 轮廓本身是 font units，乘 g.scale 后再交给 SvgPath 统一换算
        let Some(d) = glyph_path(face, gid, g.scale) else { continue };
        let _ = write!(
            body,
            r#"<path transform="translate({:.2} {:.2}) scale({:.6})" d="{}"/>"#,
            g.x * k,
            base_y - g.y * k,
            k,
            d
        );
        body.push('\n');
    }

    for r in &l.rules {
        let _ = write!(
            body,
            r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}"/>"#,
            r.x * k,
            base_y - r.y * k,
            r.width * k,
            (r.thickness * k).max(0.6)
        );
        body.push('\n');
    }

    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.2} {h:.2}">
<rect width="100%" height="100%" fill="white"/>
<g fill="black">{body}</g>
</svg>
"#
    );
    std::fs::write(&out, svg).expect("写出 SVG");
    println!("{latex}  →  {out}  ({:.0}×{:.0})", w, h);
}
