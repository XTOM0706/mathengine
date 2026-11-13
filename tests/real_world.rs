//! 真实场景回归：教材/试卷/工程里真会用到的公式，必须全部能渲染且干净。
//!
//! "干净"指：没有未识别的命令被静默丢掉、没有缺字形。这两类问题都不会报错，
//! 只会让公式少一截或多出豆腐块，所以必须由测试守住。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;

const CASES: &str = include_str!("../docs/cases-real.txt");

fn font() -> MathFont<'static> {
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体存在");
    let bytes: &'static [u8] = std::boxed::Box::leak(data.into_boxed_slice());
    MathFont::from_bytes(bytes).expect("字体可解析")
}

#[test]
fn 真实公式全部能解析() {
    let f = font();
    let mut failed = Vec::new();
    for line in CASES.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if parse(line).is_err() {
            failed.push(line);
        }
    }
    assert!(failed.is_empty(), "解析失败的公式：\n{}", failed.join("\n"));
}

#[test]
fn 真实公式没有命令被丢弃() {
    let _f = font();
    let mut bad = Vec::new();
    for line in CASES.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let ast = match parse(line) { Ok(a) => a, Err(_) => continue };
        let unknown = mathengine::ast::unknown_commands(&ast);
        if !unknown.is_empty() {
            bad.push(format!("{line}  →  {}", unknown.join(" ")));
        }
    }
    assert!(bad.is_empty(), "以下公式有命令被丢弃：\n{}", bad.join("\n"));
}

#[test]
fn 真实公式没有缺字形() {
    let f = font();
    let mut bad = Vec::new();
    for line in CASES.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let ast = match parse(line) { Ok(a) => a, Err(_) => continue };
        let l = layout(&f, &ast);
        for g in &l.glyphs {
            if f.glyph_id(g.ch).is_none() {
                bad.push(format!("{line}  →  {:?} U+{:04X}", g.ch, g.ch as u32));
                break;
            }
        }
    }
    assert!(bad.is_empty(), "以下公式有缺字形：\n{}", bad.join("\n"));
}
