//! `cargo run --example sheet -- 公式.txt out.svg [字号]`：把多条公式竖向排进一张 SVG。
//!
//! 每行一条公式，空行与 `#` 开头的行忽略。用于一次性检查一批公式的排版是否正常。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;
use std::fmt::Write as _;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

struct Path {
    d: String,
    k: f32,
}

impl Path {
    fn new(k: f32) -> Self {
        Self { d: String::new(), k }
    }
}

impl OutlineBuilder for Path {
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
            x1 * self.k, -y1 * self.k, x * self.k, -y * self.k
        );
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(
            self.d,
            "C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} ",
            x1 * self.k, -y1 * self.k, x2 * self.k, -y2 * self.k, x * self.k, -y * self.k
        );
    }
    fn close(&mut self) {
        self.d.push_str("Z ");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let list = args.next().expect("用法: sheet 公式列表文件 输出.svg [字号]");
    let out = args.next().unwrap_or_else(|| "sheet.svg".to_string());
    let size: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(48.0);

    let src = std::fs::read_to_string(&list).expect("读公式列表");
    let items: Vec<&str> = src
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("parse font");
    let face: &Face = font.face_ref().expect("face");
    let upem = font.units_per_em();
    let k = size / upem;
    let pad = size * 0.5;
    let row_gap = size * 0.9;

    let mut body = String::new();
    let mut y_cursor = pad;
    let mut max_w = 0.0f32;

    for f in &items {
        let ast = match parse(f) {
            Ok(a) => a,
            Err(_) => continue,
        };
        let l = layout(&font, &ast);
        let base = y_cursor + l.height * k;
        for g in &l.glyphs {
            // 同 render_svg：布局层已经定好正/斜体，这里不能再套一次斜体映射
            let Some(gid) = face.glyph_index(g.ch).map(|i| i.0) else { continue };
            let mut p = Path::new(k);
            if face.outline_glyph(GlyphId(gid), &mut p).is_none() || p.d.is_empty() {
                continue;
            }
            let _ = write!(
                body,
                r#"<path transform="translate({:.2} {:.2}) scale({:.6})" d="{}"/>"#,
                g.x * k + pad,
                base - g.y * k,
                g.scale,
                p.d
            );
            body.push('\n');
        }
        for r in &l.rules {
            let _ = write!(
                body,
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}"/>"#,
                r.x * k + pad,
                base - r.y * k,
                r.width * k,
                (r.thickness * k).max(0.6)
            );
            body.push('\n');
        }
        max_w = max_w.max(l.width * k);
        y_cursor += (l.height + l.depth) * k + row_gap;
    }

    let w = max_w + pad * 2.0;
    let h = y_cursor + pad;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.2} {h:.2}">
<rect width="100%" height="100%" fill="white"/>
<g fill="black">{body}</g>
</svg>
"#
    );
    std::fs::write(&out, svg).expect("写 SVG");
    println!("{} 条公式 → {out} ({:.0}×{:.0})", items.len(), w, h);
}
