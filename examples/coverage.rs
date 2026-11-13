//! `cargo run --release --example coverage -- 用例文件`：逐条报告能否解析并布局。
//! 与 `Coverage.java`、`coverage.mjs` 读同一个文件，用于横向比较功能覆盖。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;

fn main() {
    let path = std::env::args().nth(1).expect("用法: coverage 用例文件");
    let src = std::fs::read_to_string(path).expect("读用例");
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("parse font");

    let (mut ok, mut no) = (0, 0);
    for line in src.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match parse(line) {
            Ok(ast) => {
                let l = layout(&font, &ast);
                let missing = l.glyphs.iter().filter(|g| font.glyph_id(g.ch).is_none()).count();
                if missing > 0 {
                    println!("OK(缺字{missing}) {line}");
                } else {
                    println!("OK           {line}");
                }
                ok += 1;
            }
            Err(e) => {
                println!("FAIL         {line}   [{e:?}]");
                no += 1;
            }
        }
    }
    println!("--- 成功 {ok} / 失败 {no}");
}
