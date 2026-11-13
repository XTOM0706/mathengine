//! `cargo run --release --example symbol_audit`：符号表自检。
//!
//! 遍历全部已登记的命令，检查三件事：字体里有没有对应字形、能否解析成 Symbol、
//! 能否布局出字形。任何一个缺失都会在用户那里变成一个豆腐块，靠肉眼翻表是翻不出来的。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;

fn main() {
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体");
    let bytes: &'static [u8] = std::boxed::Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("parse font");

    let table = mathengine::layout::symbol_table();
    let mut missing_glyph = Vec::new();
    let mut no_layout = Vec::new();

    for (name, expected) in &table {
        let src = format!(r"\{name}");
        match parse(&src) {
            Ok(ast) => {
                let l = layout(&font, &ast);
                if l.glyphs.is_empty() {
                    no_layout.push(*name);
                } else if font.glyph_id(l.glyphs[0].ch).is_none() {
                    missing_glyph.push((*name, l.glyphs[0].ch, *expected));
                }
            }
            Err(_) => no_layout.push(*name),
        }
    }

    println!("符号表共 {} 条", table.len());
    println!("字体缺字形 {} 条", missing_glyph.len());
    for (n, got, want) in &missing_glyph {
        println!("   \\{n:<18} 布局得到 U+{:04X} {:?}，字体无此字形", *got as u32, got);

    }
    println!("无法布局 {} 条：{:?}", no_layout.len(), no_layout);
}
