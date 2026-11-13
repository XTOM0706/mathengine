//! 对外 API 的集成测试：只用 `pub` 接口，模拟宿主（Kotlin / C / 自己的 CLI）的调用方式。
//!
//! 与 `src/*.rs` 里的单元测试分工不同：那边测内部实现细节，这里只测「从外部看，行为对不对」。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;

fn font() -> MathFont<'static> {
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体文件应存在");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    MathFont::from_bytes(bytes).expect("字体应能解析")
}

fn lay(latex: &str) -> mathengine::layout::Layout {
    let f = font();
    let ast = parse(latex).expect("这些公式都应能解析");
    layout(&f, &ast)
}

#[test]
fn 输出尺寸有限且非负() {
    for src in [r"x", r"\frac{a}{b}", r"\sqrt{x+1}", r"a_{i}^{2}"] {
        let l = lay(src);
        for v in [l.width, l.height, l.depth] {
            assert!(v.is_finite(), "{src}: 出现 NaN 或无穷");
            assert!(v >= 0.0, "{src}: 尺寸为负");
        }
    }
}

#[test]
fn 字形位置全部有限() {
    let l = lay(r"\frac{-b\pm\sqrt{b^2-4ac}}{2a}");
    for g in &l.glyphs {
        assert!(g.x.is_finite() && g.y.is_finite() && g.scale.is_finite());
        assert!(g.scale > 0.0, "缩放必须为正");
    }
    for r in &l.rules {
        assert!(r.width >= 0.0 && r.thickness > 0.0);
    }
}

#[test]
fn 无字形落在宽度之外() {
    // 允许微小越界（斜体的右侧探出），但不允许整块跑到外面
    for src in [r"x", r"a+b", r"\frac{a}{b}", r"\sqrt{x}"] {
        let l = lay(src);
        for g in &l.glyphs {
            assert!(g.x >= -1.0, "{src}: 字形 x 为负 {}", g.x);
            assert!(g.x <= l.width + 1.0, "{src}: 字形越界 {} > {}", g.x, l.width);
        }
    }
}

#[test]
fn 未知命令不失败() {
    // 宽容策略：宁可显示难看，也不要整段公式崩掉
    let l = lay(r"\thiscommanddoesnotexist{x}");
    assert!(!l.glyphs.is_empty());
}

#[test]
fn 空输入得到零尺寸() {
    let l = lay("");
    assert_eq!(l.width, 0.0);
    assert_eq!(l.height, 0.0);
    assert_eq!(l.depth, 0.0);
}

#[test]
fn 中文混排按满宽占位() {
    // LM Math 没有中文字形，宿主会回退系统字体；布局必须按 1em 留位，否则画出来超界
    let f = font();
    let width_cn = f.advance('中');
    assert!((width_cn - f.units_per_em()).abs() < 1.0);
}

#[test]
fn 同样的输入两次布局结果一致() {
    let a = lay(r"\int_0^1 x^2 \, dx");
    let b = lay(r"\int_0^1 x^2 \, dx");
    assert_eq!(a, b, "布局必须是纯函数");
}
