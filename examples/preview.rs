//! `cargo run --example preview -- "<latex>"`：宿主 ASCII 预览。
fn main() {
    let latex = std::env::args().nth(1).unwrap_or_else(|| "x^2 + \\frac{a}{b}".into());
    let data = mathengine::font::load_latin_modern_math(
        mathengine::LM_MATH_OTF,
    ).expect("字体");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = mathengine::font::MathFont::from_bytes(bytes).expect("parse font");
    println!("{}", mathengine::preview::dump_ascii(&font, &latex));
}
