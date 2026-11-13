//! 轻量 LaTeX 数学排版引擎（Rust）。
//!
//! 只做数学模式（math mode），不做整页排版。输入一段 LaTeX 公式，输出**扁平的绘制指令**：
//! 每个字形的码位、位置、缩放，加上分数线与根号横线的矩形。渲染层不需要理解 TeX，
//! 照着指令画即可。
//!
//! 设计动机：替换 GPL-2.0 的 jlatexmath。它在 Android 上有三个问题——许可会传染整个
//! 应用、依赖 JVM 带来启动与体积开销、小屏设备上布局结果不稳定。本引擎只依赖
//! `ttf-parser` 一个 crate，可以编成静态库或 `.so` 嵌入任何宿主。
//!
//! 模块：
//! - [`token`]：math-mode LaTeX → token 流
//! - [`ast`]：token → 数学 AST（原子序列 + Fraction / Sqrt / Scripts / Matrix …）
//! - [`font`]：OpenType 度量封装（cmap / hmtx / MATH 表）
//! - [`layout`]：AST → TeX Box 布局 → 扁平字形指令
//! - [`ffi`]：C ABI 导出（`mathengine_layout` / `mathengine_free`）
//! - [`preview`]：把布局结果画成 ASCII 网格，用于肉眼比对
//! - `jni`：Android JNI 导出（feature `jni`，默认关闭）

pub mod ast;
pub mod ffi;
pub mod font;
pub mod layout;
pub mod preview;
pub mod svg;
pub mod token;

#[cfg(feature = "jni")]
pub mod jni;

/// 随仓库附带的 Latin Modern Math 字体路径。
pub const LM_MATH_OTF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/latinmodern-math.otf");
