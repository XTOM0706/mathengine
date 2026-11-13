//! 布局预览（host 工具）：把 flat 布局转成 ASCII 网格，宿主目测"显示相似"。
//! 用法：`cargo run --example preview -- "x^2 + \frac{a}{b}"`（需要 examples/ 入口）

use crate::ast::parse;
use crate::font::{load_latin_modern_math, MathFont};
use crate::layout::{layout, PlacedGlyph};

/// 把布局渲染成字符网格（每字符约 0.06em × 0.06em），基线为中间行。
pub fn dump_ascii(font: &MathFont, latex: &str) -> String {
    let ast = match parse(latex) { Ok(a) => a, Err(_) => return "PARSE FAILED".into() };
    let l = layout(font, &ast);
    // 网格：宽 = ceil(width/60)，高 = ceil((height+depth)/60)，基线位置 = ceil(height/60)
    let cell = 35.0f32;   // 更细粒度，能看出上下标/分数位置
    let cols = (l.width / cell).ceil().max(1.0) as usize;
    let rows = ((l.height + l.depth) / cell).ceil().max(3.0) as usize;
    let base_row = (l.height / cell).ceil() as usize;
    let mut grid = vec![vec![' '; cols]; rows];
    for g in &l.glyphs {
        let cx = (g.x / cell) as usize;
        let cy = (base_row as f32 - g.y / cell) as isize;
        if cy >= 0 && (cy as usize) < rows && cx < cols {
            grid[cy as usize][cx] = g.ch;
        }
        
    }
    // 渲染分数线的 rule 为 '-'
    for rule in &l.rules {
        let cx = (rule.x / cell) as usize;
        let cy = (base_row as f32 - rule.y / cell) as isize;
        let cw = (rule.width / cell) as usize;
        if cy >= 0 && (cy as usize) < rows {
            for i in 0..cw {
                if cx + i < cols { grid[cy as usize][cx + i] = '-'; }
            }
        }
    }
    grid.iter().map(|row| row.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
}

pub fn _unused(_: &PlacedGlyph) {}
#[allow(dead_code)]
fn load_font() -> Option<MathFont<'static>> {
    let data = load_latin_modern_math(crate::LM_MATH_OTF)?;
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    MathFont::from_bytes(bytes)
}
