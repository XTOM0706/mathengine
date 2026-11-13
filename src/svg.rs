//! 把布局结果画成 SVG。
//!
//! 不经过任何光栅化库：直接向字体索取字形轮廓，换算成 path。这样输出与引擎用的是
//! 同一套布局数据，不存在「图是别处画的」。`examples/render_svg.rs` 与本地预览服务
//! 都走这里，不各写一份。

use crate::ast::parse;
use crate::font::MathFont;
use crate::layout::layout;
use std::fmt::Write as _;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

pub struct Rendered {
    pub svg: String,
    pub width: f32,
    pub height: f32,
    pub glyphs: usize,
    /// 字体里没有字形、被跳过的字符数
    pub missing: usize,
}

struct PathBuilder {
    d: String,
    k: f32,
}

impl PathBuilder {
    fn new(k: f32) -> Self {
        Self { d: String::new(), k }
    }
}

impl OutlineBuilder for PathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "M{:.2} {:.2} ", x * self.k, -y * self.k);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.d, "L{:.2} {:.2} ", x * self.k, -y * self.k);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(
            self.d,
            "Q{:.2} {:.2} {:.2} {:.2} ",
            x1 * self.k,
            -y1 * self.k,
            x * self.k,
            -y * self.k
        );
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(
            self.d,
            "C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} ",
            x1 * self.k,
            -y1 * self.k,
            x2 * self.k,
            -y2 * self.k,
            x * self.k,
            -y * self.k
        );
    }
    fn close(&mut self) {
        self.d.push_str("Z ");
    }
}

fn glyph_path(face: &Face, gid: u16, scale: f32) -> Option<String> {
    let mut b = PathBuilder::new(scale);
    face.outline_glyph(GlyphId(gid), &mut b)?;
    if b.d.is_empty() {
        None
    } else {
        Some(b.d)
    }
}

/// 渲染一条公式。`size` 是字号（px），失败返回解析错误的可读描述。
pub fn render(font: &MathFont, latex: &str, size: f32) -> Result<Rendered, String> {
    let ast = parse(latex).map_err(|e| format!("{e:?}"))?;
    let l = layout(font, &ast);

    let upem = font.units_per_em();
    let k = size / upem;
    let pad = size * 0.4;
    let w = l.width * k + pad * 2.0;
    let h = (l.height + l.depth) * k + pad * 2.0;
    let base_y = pad + l.height * k;

    let face = font.face_ref().ok_or_else(|| "字体没有 face".to_string())?;
    let mut body = String::new();
    let mut missing = 0usize;

    for g in &l.glyphs {
        // 不能走 MathFont::glyph_id —— 它会再做一次数学斜体映射，
        // 而布局层已经决定过这个字符该正体还是斜体。
        let Some(gid) = face.glyph_index(g.ch) else {
            missing += 1;
            continue;
        };
        let Some(d) = glyph_path(face, gid.0, g.scale) else { continue };
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
</svg>"#
    );

    Ok(Rendered {
        svg,
        width: w,
        height: h,
        glyphs: l.glyphs.len(),
        missing,
    })
}
