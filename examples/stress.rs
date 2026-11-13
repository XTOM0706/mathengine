//! `cargo run --release --example stress`：压力测试。
//!
//! 生成几类刻意刁钻的输入（深嵌套、大矩阵、超长行、脚本链、混合华丽符号），
//! 逐条记录耗时、字形数与进程峰值内存。
//!
//! 峰值内存直接读 /proc/self/status 的 VmHWM，不引第三方库。

use mathengine::font::{load_latin_modern_math, MathFont};
use std::time::Instant;

fn peak_rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1).map(|v| v.to_string()))
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn nested_frac(n: usize) -> String {
    let mut s = String::from("x");
    for _ in 0..n {
        s = format!(r"\frac{{1}}{{{s}}}");
    }
    s
}

fn nested_sqrt(n: usize) -> String {
    let mut s = String::from("x");
    for _ in 0..n {
        s = format!(r"\sqrt{{{s}+1}}");
    }
    s
}

fn matrix(rows: usize, cols: usize) -> String {
    let mut s = String::from(r"\begin{pmatrix}");
    for r in 0..rows {
        for c in 0..cols {
            if c > 0 {
                s.push('&');
            }
            s.push_str(&format!("a_{{{r}{c}}}"));
        }
        if r + 1 < rows {
            s.push_str(r"\\");
        }
    }
    s.push_str(r"\end{pmatrix}");
    s
}

fn long_row(n: usize) -> String {
    (0..n).map(|i| format!("x_{{{i}}}^2")).collect::<Vec<_>>().join("+")
}

fn big_sum(n: usize) -> String {
    (0..n).map(|i| format!(r"\frac{{1}}{{k_{i}^2}}")).collect::<Vec<_>>().join("+")
}

fn mixed() -> String {
    r"\displaystyle\sum_{n=1}^{\infty}\frac{1}{n^2}=\frac{\pi^2}{6},\quad
\int_{-\infty}^{\infty}e^{-x^2}\,dx=\sqrt{\pi},\quad
\mathbb{R}^{n}\xrightarrow{\ \mathcal{L}\ }\mathbb{C},\quad
\mathfrak{gl}(n,\mathbb{C})\cong\mathfrak{sl}(n,\mathbb{C})\oplus\mathbb{C},\quad
\oint_{\partial\Omega}\vec{B}\cdot d\vec{S}=\mu_0 I,\quad
\bigoplus_{i=1}^{\infty}\mathcal{H}_i,\quad
\underbrace{a+b+\cdots+z}_{26},\quad
\begin{pmatrix}1&0&0\\0&1&0\\0&0&1\end{pmatrix}^{-1},\quad
\lim_{x\to0}\frac{\sin x}{x}=1"
        .replace('\n', " ")
}

fn main() {
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体");
    let bytes: &'static [u8] = std::boxed::Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("parse font");

    let cases: Vec<(String, String)> = vec![
        ("深嵌套分式 10".into(), nested_frac(10)),
        ("深嵌套分式 30".into(), nested_frac(30)),
        ("深嵌套分式 60".into(), nested_frac(60)),
        ("深嵌套分式 100".into(), nested_frac(100)),
        ("嵌套根号 20".into(), nested_sqrt(20)),
        ("矩阵 5×5".into(), matrix(5, 5)),
        ("矩阵 10×10".into(), matrix(10, 10)),
        ("矩阵 20×20".into(), matrix(20, 20)),
        ("矩阵 40×40".into(), matrix(40, 40)),
        ("长行 100 项".into(), long_row(100)),
        ("长行 500 项".into(), long_row(500)),
        ("长行 2000 项".into(), long_row(2000)),
        ("求和 100 项".into(), big_sum(100)),
        ("华丽混合".into(), mixed()),
    ];

    println!("{:<18} {:>10} {:>8} {:>10} {:>8}", "用例", "输入字节", "字形", "耗时(µs)", "峰值MB");
    println!("{}", "-".repeat(60));

    let mut worst = 0.0f64;
    for (name, src) in &cases {
        let t = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mathengine::svg::render(&font, src, 40.0)
        }));
        let us = t.elapsed().as_secs_f64() * 1e6;
        worst = worst.max(us);
        match r {
            Ok(Ok(out)) => println!(
                "{name:<18} {:>10} {:>8} {:>10.0} {:>8.1}",
                src.len(), out.glyphs, us,
                peak_rss_kb() as f64 / 1024.0
            ),
            Ok(Err(e)) => println!("{name:<18} {:>10}   解析失败: {e}", src.len()),
            Err(_) => println!("{name:<18} {:>10}   ** panic **", src.len()),
        }
    }
    println!("{}", "-".repeat(60));
    println!("最慢一条 {:.0} µs，进程峰值内存 {:.1} MB", worst, peak_rss_kb() as f64 / 1024.0);
}
