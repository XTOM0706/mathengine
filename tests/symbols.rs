//! 符号表回归：每个登记的命令都必须能在字体里找到字形。
//!
//! 缺字形的命令在用户那里就是一个豆腐块，而且不会报错——只能靠这条测试兜住。
//! 换字体或改子集范围时最容易踩。

use mathengine::font::{load_latin_modern_math, MathFont};

#[test]
fn 符号表里每条命令都有字形() {
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体文件应存在");
    let bytes: &'static [u8] = std::boxed::Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("字体应能解析");

    let mut bad = Vec::new();
    for (name, ch) in mathengine::layout::symbol_table() {
        if font.glyph_id(ch).is_none() {
            bad.push(format!("\\{name} → U+{:04X} {:?}", ch as u32, ch));
        }
    }
    assert!(bad.is_empty(), "以下命令在字体里没有字形：\n{}", bad.join("\n"));
}
