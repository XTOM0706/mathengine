//! 稳定性测试：不管输入多离谱，引擎都不许 panic、不许产出 NaN、不许无限递归。
//!
//! 用固定种子的伪随机串生成输入，保证每次运行可复现（不引入 rand 依赖）。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;

fn font() -> MathFont<'static> {
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体存在");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    MathFont::from_bytes(bytes).expect("字体可解析")
}

/// 线性同余，和 `examples/fuzz_report.rs`、`Bench.java` 用同一套参数，
/// 三个实现吃的是同一串输入，才谈得上对比。
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() as usize) % xs.len()]
    }
}

const TOKENS: &[&str] = &[
    "x", "y", "a", "1", "2", "+", "-", "=", "^", "_", "{", "}", "\\frac", "\\sqrt", "\\sum",
    "\\int", "\\alpha", "\\left(", "\\right)", "\\binom", "\\begin{pmatrix}", "\\end{pmatrix}",
    "&", "\\\\", "\\hat", "\\text", "\\,", "\\unknowncmd", "中", " ", "(", ")", "[", "]",
    "\\lim", "\\infty", "\\partial", "\\to",
];

fn random_latex(rng: &mut Lcg) -> String {
    let n = (rng.next() % 24) as usize;
    let mut s = String::new();
    for _ in 0..n {
        let t = *rng.pick(TOKENS);
        // 随机在 token 之间插空格，制造更多边界情况
        if rng.next() % 4 == 0 {
            s.push(' ');
        }
        s.push_str(t);
    }
    s
}

/// 一条输入是否满足「可接受」：不 panic、尺寸有限且非负、字形缩放为正。
fn check(src: &str, f: &MathFont) -> Result<(), String> {
    let ast = parse(src).map_err(|e| format!("parse: {e:?}"))?;
    let l = layout(f, &ast);
    for v in [l.width, l.height, l.depth] {
        if !v.is_finite() {
            return Err(format!("尺寸非有限: {v}"));
        }
        if v < 0.0 {
            return Err(format!("尺寸为负: {v}"));
        }
    }
    for g in &l.glyphs {
        if !g.x.is_finite() || !g.y.is_finite() || !g.scale.is_finite() {
            return Err("字形坐标非有限".into());
        }
        if g.scale <= 0.0 {
            return Err(format!("缩放非正: {}", g.scale));
        }
    }
    Ok(())
}

#[test]
fn 两千条随机输入不崩不产出坏数据() {
    let f = font();
    let mut rng = Lcg::new(0x5EED_0706);
    let mut panics = 0;
    let mut bad = 0;
    for i in 0..2000 {
        let src = random_latex(&mut rng);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&src, &f)));
        match r {
            Err(_) => {
                panics += 1;
                eprintln!("panic 于第 {i} 条: {src:?}");
            }
            Ok(Err(e)) => {
                // 解析失败是可以接受的（返回 Err），但绝不能是坏数据
                if !e.starts_with("parse:") {
                    bad += 1;
                    eprintln!("坏数据于第 {i} 条 {src:?}: {e}");
                }
            }
            Ok(Ok(())) => {}
        }
    }
    assert_eq!(panics, 0, "有 {panics} 条输入触发 panic");
    assert_eq!(bad, 0, "有 {bad} 条输入产出非法布局");
}

#[test]
fn 极深嵌套不栈溢出() {
    let f = font();
    // 40 层分式嵌套：递归下降解析器与布局都是递归实现，这条用来卡住爆栈
    let mut src = String::from("x");
    for _ in 0..40 {
        src = format!(r"\frac{{{src}}}{{x}}");
    }
    let ast = parse(&src).expect("深层嵌套应能解析");
    let l = layout(&f, &ast);
    assert!(l.width.is_finite() && l.width > 0.0);
}

#[test]
fn 超长输入不失控() {
    let f = font();
    let src = "x+".repeat(20_000);
    let ast = parse(&src).expect("长串应能解析");
    let l = layout(&f, &ast);
    assert!(l.width.is_finite(), "宽度必须是有限值");
}

#[test]
fn 未闭合的括号与空组不崩() {
    let f = font();
    for src in [
        r"{{{{{{", r"}}}}}", r"\frac{", r"\frac{}{}", r"\sqrt{}", r"\left(", r"\right)",
        r"^", r"_", r"^^^", r"&&&", r"\\\\\\", r"\begin{pmatrix}", r"\end{pmatrix}",
        r"\binom{}{}", r"\text{}", r"\hat{}",
    ] {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(src, &f)));
        assert!(r.is_ok(), "{src:?} 触发 panic");
        if let Ok(Err(e)) = r {
            assert!(e.starts_with("parse:"), "{src:?} 产出非法布局: {e}");
        }
    }
}

#[test]
fn 空串与纯空白() {
    let f = font();
    for src in ["", " ", "\t", "\n", "   \n  "] {
        let ast = parse(src).expect("空白应能解析");
        let l = layout(&f, &ast);
        assert_eq!(l.width, 0.0);
        assert_eq!(l.glyphs.len(), 0);
    }
}
