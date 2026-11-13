//! `cargo run --example render_svg -- "<latex>" out.svg [字号]`：把一条公式画成 SVG。
//!
//! 实际渲染在 `mathengine::svg` 里，这里只做命令行外壳——预览服务用的是同一个函数，
//! 不存在「文档里的图和网页显示的不是一套代码」这种情况。

use mathengine::font::{load_latin_modern_math, MathFont};

fn main() {
    let mut args = std::env::args().skip(1);
    let latex = args.next().unwrap_or_else(|| r"E = mc^2".to_string());
    let out = args.next().unwrap_or_else(|| "out.svg".to_string());
    let size: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(64.0);

    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体文件应存在");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("字体应能解析");

    match mathengine::svg::render(&font, &latex, size) {
        Ok(r) => {
            std::fs::write(&out, r.svg).expect("写出 SVG");
            println!("{latex}  →  {out}  ({:.0}×{:.0}, {} 字形)", r.width, r.height, r.glyphs);
        }
        Err(e) => {
            eprintln!("解析失败：{e}");
            std::process::exit(1);
        }
    }
}
