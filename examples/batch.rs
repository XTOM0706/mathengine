//! `cargo run --release --example batch -- 公式块文件`：批量渲染并按条报告。
//!
//! 文件里用单独一行 `---` 分隔每条公式。输出每条的成败、字形数、缺字数、
//! 被丢弃的命令，最后给一份汇总——用来量「真实文档里的公式能过多少」。

use mathengine::font::{load_latin_modern_math, MathFont};
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("用法: batch 公式块文件");
    let src = std::fs::read_to_string(path).expect("读文件");
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("parse font");

    let blocks: Vec<String> = src
        .split("\n---\n")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let (mut ok, mut bad, mut with_unsupported, mut missing_chars) = (0, 0, 0, 0);
    let mut all_us: Vec<f64> = Vec::new();

    for (i, b) in blocks.iter().enumerate() {
        let t = Instant::now();
        match mathengine::svg::render(&font, b, 40.0) {
            Ok(r) => {
                let us = t.elapsed().as_secs_f64() * 1e6;
                all_us.push(us);
                ok += 1;
                if !r.unsupported.is_empty() {
                    with_unsupported += 1;
                }
                if r.missing > 0 {
                    missing_chars += 1;
                }
                let joined: String = b.split_whitespace().collect::<Vec<_>>().join(" ");
                let short: String = joined.chars().take(46).collect();
                let us_txt = if r.unsupported.is_empty() {
                    String::new()
                } else {
                    format!("  丢弃命令: {}", r.unsupported.join(" "))
                };
                println!(
                    "#{:02} OK   {:>3}字形 缺{}  {:.0}µs  {}{}",
                    i + 1, r.glyphs, r.missing, us, short, us_txt
                );
            }
            Err(e) => {
                bad += 1;
                let joined: String = b.split_whitespace().collect::<Vec<_>>().join(" ");
                let short: String = joined.chars().take(46).collect();
                println!("#{:02} FAIL {}  [{}]", i + 1, short, e);
            }
        }
    }

    all_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("----------------------------------------------");
    println!(
        "共 {} 条：成功 {}，失败 {}；含丢弃命令 {} 条，含缺字 {} 条",
        blocks.len(), ok, bad, with_unsupported, missing_chars
    );
    if !all_us.is_empty() {
        println!(
            "渲染耗时中位 {:.0} µs，p95 {:.0} µs",
            all_us[all_us.len() / 2],
            all_us[(all_us.len() as f64 * 0.95) as usize]
        );
    }
}
