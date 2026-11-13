//! `cargo run --example dump_json -- "<latex>"`：输出 Layout 为 JSON（渲染调试用）。
use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;

fn main() {
    let latex = std::env::args().nth(1).unwrap_or_else(|| "x".into());
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("parse font");
    let ast = match parse(&latex) {
        Ok(a) => a,
        Err(e) => { eprintln!("parse err: {e:?}"); std::process::exit(1); }
    };
    let l = layout(&font, &ast);
    println!("{{\"width\":{},\"height\":{},\"depth\":{}", l.width, l.height, l.depth);
    let glyphs: Vec<String> = l.glyphs.iter()
        .map(|g| format!("{{\"ch\":\"{}\",\"cp\":{},\"x\":{},\"y\":{},\"scale\":{}}}",
             g.ch, g.ch as u32, g.x, g.y, g.scale))
        .collect();
    println!(",\"glyphs\":[{}]", glyphs.join(","));
    let rules: Vec<String> = l.rules.iter()
        .map(|r| format!("{{\"x\":{},\"y\":{},\"width\":{},\"thickness\":{}}}", r.x, r.y, r.width, r.thickness))
        .collect();
    println!(",\"rules\":[{}]}}", rules.join(","));
}