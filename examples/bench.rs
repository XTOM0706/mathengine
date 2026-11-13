//! `cargo run --release --example bench`：布局性能自测。
//!
//! 输出每条公式的解析 + 布局耗时（微秒），以及整体中位数 / p95。
//! 只测布局，不含光栅化与绘制——那部分在宿主里。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;
use std::time::Instant;

const CASES: &[(&str, &str)] = &[
    ("简单变量", r"x"),
    ("上标", r"x^2"),
    ("上下标", r"a_{i}^{2}"),
    ("希腊字母", r"\alpha + \beta = \gamma"),
    ("分式", r"\frac{a}{b}"),
    ("嵌套分式", r"\frac{1}{1+\frac{1}{1+\frac{1}{x}}}"),
    ("根号", r"\sqrt{x}"),
    ("n 次根", r"\sqrt[3]{x+1}"),
    ("二次方程", r"x = \frac{-b\pm\sqrt{b^2-4ac}}{2a}"),
    ("求和", r"\sum_{i=1}^{n} i^2"),
    ("积分", r"\int_0^1 x^2 \, dx"),
    ("矩阵", r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}"),
    ("括号自适应", r"\left( \frac{a}{b} \right)"),
    ("重音", r"\hat{x} + \bar{y}"),
    ("长公式", r"e^{i\pi} + 1 = 0 \quad \forall x \in \mathbb{R}"),
];

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn main() {
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("parse font");

    let iters: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);

    // 预热，让分支预测与字体缓存进入稳态
    for _ in 0..200 {
        for (_, src) in CASES {
            let ast = parse(src).unwrap();
            std::hint::black_box(layout(&font, &ast));
        }
    }

    println!("{:<14} {:>10} {:>10} {:>8}", "用例", "中位(µs)", "p95(µs)", "字形数");
    println!("{}", "-".repeat(46));

    let mut all: Vec<f64> = Vec::new();
    for (name, src) in CASES {
        let mut samples = Vec::with_capacity(iters);
        let mut glyphs = 0;
        for _ in 0..iters {
            let t = Instant::now();
            let ast = parse(src).unwrap();
            let l = layout(&font, &ast);
            let dt = t.elapsed().as_secs_f64() * 1e6;
            samples.push(dt);
            glyphs = l.glyphs.len();
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = percentile(&samples, 0.5);
        let p95 = percentile(&samples, 0.95);
        all.extend_from_slice(&samples);
        println!("{name:<14} {med:>10.2} {p95:>10.2} {glyphs:>8}");
    }

    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{}", "-".repeat(46));
    println!(
        "全部 {} 次：中位 {:.2} µs，p95 {:.2} µs，均值 {:.2} µs",
        all.len(),
        percentile(&all, 0.5),
        percentile(&all, 0.95),
        all.iter().sum::<f64>() / all.len() as f64
    );
}
