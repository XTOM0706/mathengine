//! `cargo run --release --example fuzz_report`：随机输入下的稳定性统计。
//!
//! 与 `tests/robustness.rs`、`Bench.java`、`Fuzz.java` 用同一套 LCG 与同一张 token 表，
//! 保证三个实现吃的是完全相同的 2000 条输入，对比才有意义。
//!
//! 口径说明：
//! - 本引擎「解析失败」返回 `Err`，是正常错误处理；
//! - jlatexmath 对同类输入抛 `ParseException`，也是正常错误处理。
//!   真正要对比的是**崩溃率**：本引擎要求 0 panic、0 非法布局。

use mathengine::ast::parse;
use mathengine::font::{load_latin_modern_math, MathFont};
use mathengine::layout::layout;

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
        s.push_str(*rng.pick(TOKENS));
        if rng.next() % 4 == 0 {
            s.push(' ');
        }
    }
    s
}

fn main() {
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("parse font");

    let mut rng = Lcg::new(0x5EED_0706);
    let (mut ok, mut rejected, mut panics, mut bad) = (0, 0, 0, 0);
    let mut widths: Vec<f32> = Vec::new();

    for _ in 0..2000 {
        let src = random_latex(&mut rng);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let ast = match parse(&src) {
                Ok(a) => a,
                Err(_) => return Err("rejected"),
            };
            let l = layout(&font, &ast);
            if ![l.width, l.height, l.depth].iter().all(|v| v.is_finite() && *v >= 0.0) {
                return Err("bad");
            }
            Ok(l.width)
        }));
        match r {
            Err(_) => panics += 1,
            Ok(Err("bad")) => bad += 1,
            Ok(Err(_)) => rejected += 1,
            Ok(Ok(w)) => {
                ok += 1;
                widths.push(w);
            }
        }
    }

    println!("输入 2000 条（种子 0x5EED0706，token 池 {} 个）", TOKENS.len());
    println!("  布局成功   {ok}");
    println!("  解析拒绝   {rejected}");
    println!("  崩溃 panic {panics}");
    println!("  非法布局   {bad}");
    if !widths.is_empty() {
        widths.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = widths[widths.len() / 2];
        println!("  成功项宽度中位 {med:.0} font units，最大 {:.0}", widths.last().unwrap());
    }
}
