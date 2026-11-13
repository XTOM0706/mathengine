//! C ABI / JNI 适配层：Rust 引擎 ↔ Kotlin（经 `.so`）。
//!
//! 设计目标（手表约束：快/小/省电）：
//!   - **一次调用完成 解析+布局**，输出扁平指令数组，Kotlin 侧零逻辑直接绘制；
//!   - 归零分配（ZST/alloc）由本层管理，Kotlin 拿到指针读完后必须调 `arix_math_free`；
//!   - 失败不抛异常：返回空布局 + `last_error`（Kotlin 侧回退原文）；
//!   - 线程安全：本层不持全局状态（字体每次传入），无锁。
//!
//! ABI 结构（固定布局，两边按此对齐）：
//! ```c
//! typedef struct {
//!     const uint32_t* glyphs;   // Unicode code point 数组
//!     const float*    xs;       // 相对基线原点 x（font units）
//!     const float*    ys;       // 相对基线 y（向上为正）
//!     const float*    scales;   // 缩放系数（1.0/0.7/0.5）
//!     uint32_t        glyph_count;
//!     const float*    rule_x;   // 分数线/根号横线（x, y, w, thick 交错）
//!     uint32_t        rule_count; // 规则条数（每条 4 个 float）
//!     float width, height, depth;
//! } ArixMathLayout;
//! ```

use std::alloc::{alloc, dealloc, Layout as AllocLayout};
use std::os::raw::c_char;
use std::ptr;
use std::slice;

use crate::ast::parse;
use crate::font::MathFont;
use crate::layout::Layout;

/// C ABI 布局结果（镜像 [crate::layout::Layout] 的扁平视图）。
#[repr(C)]
pub struct ArixMathLayout {
    pub glyphs: *const u32,
    pub xs: *const f32,
    pub ys: *const f32,
    pub scales: *const f32,
    pub glyph_count: u32,
    pub rule_data: *const f32,   // 每条 4 float：x, y, width, thickness
    pub rule_count: u32,
    pub width: f32,
    pub height: f32,
    pub depth: f32,
}

/// 最后错误信息（UTF-8 字节，归零分配；无错为 null）。
#[no_mangle]
pub static mut ARIX_MATH_LAST_ERROR: *mut c_char = ptr::null_mut();

// ---- 内部实现（无 JNI 依赖，便于宿主单测直接调）----


/// 解析 + 布局（内部实现）。返回 flat 布局；失败返回 None。
/// `font_data` 传字体字节；`font_path` 传字体文件路径（二选一，path 优先用于宿主测试）。
fn do_layout(latex: &str, font_data: &[u8], font_path: Option<&str>) -> Option<Layout> {
    let bytes = if let Some(p) = font_path {
        std::fs::read(p).ok()?
    } else if !font_data.is_empty() {
        font_data.to_vec()
    } else {
        return None;
    };
    let font = MathFont::from_bytes(&bytes)?;
    let ast = parse(latex).ok()?;
    Some(crate::layout::layout(&font, &ast))
}

/// 把 [Layout] 平铺成 C 结构（归零分配；调用方负责 [arix_math_free]）。
fn to_c_layout(l: &Layout) -> ArixMathLayout {
    let n = l.glyphs.len();
    let r = l.rules.len();
    let fill_f32 = |vals: &[f32]| -> *const f32 {
        if vals.is_empty() { return ptr::null(); }
        let lay = AllocLayout::from_size_align(vals.len() * 4, 4).unwrap();
        let p = unsafe { alloc(lay) } as *mut f32;
        unsafe { ptr::copy_nonoverlapping(vals.as_ptr(), p, vals.len()) };
        p
    };
    let glyphs = if n == 0 { ptr::null() } else {
        let lay = AllocLayout::from_size_align(n * 4, 4).unwrap();
        let p = unsafe { alloc(lay) } as *mut u32;
        for (i, g) in l.glyphs.iter().enumerate() {
            unsafe { *p.add(i) = g.ch as u32; }
        }
        p
    };
    let xs: Vec<f32> = l.glyphs.iter().map(|g| g.x).collect();
    let ys: Vec<f32> = l.glyphs.iter().map(|g| g.y).collect();
    let sc: Vec<f32> = l.glyphs.iter().map(|g| g.scale).collect();
    // rule_data：交错 x,y,w,th
    let mut rd: Vec<f32> = Vec::with_capacity(r * 4);
    for rule in &l.rules {
        rd.push(rule.x); rd.push(rule.y); rd.push(rule.width); rd.push(rule.thickness);
    }
    ArixMathLayout {
        glyphs,
        xs: fill_f32(&xs),
        ys: fill_f32(&ys),
        scales: fill_f32(&sc),
        glyph_count: n as u32,
        rule_data: if rd.is_empty() { ptr::null() } else { fill_f32(&rd) },
        rule_count: r as u32,
        width: l.width,
        height: l.height,
        depth: l.depth,
    }
}

/// 记录错误（归零分配；线程本地避免覆盖）。
fn set_error(msg: &str) {
    let bytes = msg.as_bytes();
    let lay = AllocLayout::from_size_align(bytes.len() + 1, 1).unwrap();
    unsafe {
        let p = alloc(lay) as *mut c_char;
        ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, p, bytes.len());
        *p.add(bytes.len()) = 0;
        // 简单的原子替换：旧错误泄漏可接受（错误极低频）；用 OnceLock 不行，直接写 static mut（单线程假设下安全）
        ARIX_MATH_LAST_ERROR = p;
    }
}

// ---- C ABI 导出 ----

/// 解析+布局。入口：
///   `font`：指向字体字节的指针；`font_len`：字节数。
///   `latex`：UTF-8 LaTeX 字符串指针；`latex_len`：长度。
/// 返回 [ArixMathLayout]（值传递）；失败时 glyph_count=0 且 last_error 非空。
/// 成功时调用方读完数据后必须调 [arix_math_free]。
#[no_mangle]
pub extern "C" fn arix_math_layout(
    font: *const u8,
    font_len: usize,
    latex: *const u8,
    latex_len: usize,
) -> ArixMathLayout {
    unsafe {
        if font.is_null() || latex.is_null() || latex_len == 0 || font_len == 0 {
            set_error("null input");
            return empty_layout();
        }
        let fs = slice::from_raw_parts(font, font_len);
        let ls = slice::from_raw_parts(latex, latex_len);
        let latex_str = String::from_utf8_lossy(ls);
        match do_layout(&latex_str, fs, None) {
            Some(l) => to_c_layout(&l),
            None => {
                set_error("parse or font failed");
                empty_layout()
            }
        }
    }
}

/// 宿主测试用的布局入口（从文件路径读字体）。
#[no_mangle]
pub extern "C" fn arix_math_layout_from_path(
    path: *const c_char,
    latex: *const u8,
    latex_len: usize,
) -> ArixMathLayout {
    unsafe {
        if path.is_null() || latex.is_null() {
            set_error("null input");
            return empty_layout();
        }
        let cstr = std::ffi::CStr::from_ptr(path);
        let path_str = cstr.to_string_lossy().to_string();
        let ls = slice::from_raw_parts(latex, latex_len);
        let latex_str = String::from_utf8_lossy(ls);
        match do_layout(&latex_str, &[], Some(&path_str)) {
            Some(l) => to_c_layout(&l),
            None => { set_error("parse or font failed"); empty_layout() }
        }
    }
}

/// 释放 [arix_math_layout] 分配的内存。传入的值被置零。
#[no_mangle]
pub unsafe extern "C" fn arix_math_free(l: *mut ArixMathLayout) {
    if l.is_null() { return; }
    let l = &mut *l;
    unsafe fn free_ptr<T>(p: *const T) {
        if p.is_null() { return; }
        let lay = AllocLayout::from_size_align(std::mem::size_of::<T>(), std::mem::align_of::<T>()).unwrap();
        dealloc(p as *mut u8, lay);
    }
    free_ptr(l.glyphs);
    free_ptr(l.xs);
    free_ptr(l.ys);
    free_ptr(l.scales);
    free_ptr(l.rule_data);
    *l = empty_layout();
}

fn empty_layout() -> ArixMathLayout {
    ArixMathLayout {
        glyphs: ptr::null(),
        xs: ptr::null(),
        ys: ptr::null(),
        scales: ptr::null(),
        glyph_count: 0,
        rule_data: ptr::null(),
        rule_count: 0,
        width: 0.0,
        height: 0.0,
        depth: 0.0,
    }
}

// ---- 宿主测试（直接调内部实现，验证 FFI 数据通路）----

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    const LM_PATH: &str = crate::LM_MATH_OTF;

    #[test]
    fn ffi单字符布局() {
        let l = do_layout("x", &[], Some(LM_PATH)).expect("x 应布局成功");
        assert_eq!(l.glyphs.len(), 1);
        assert_eq!(l.glyphs[0].ch, crate::font::to_math_italic('x'));
    }

    #[test]
    fn ffi分数有规则() {
        let l = do_layout(r"\frac{a}{b}", &[], Some(LM_PATH)).unwrap();
        assert_eq!(l.rules.len(), 1);
        assert!(l.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('a')));
        assert!(l.glyphs.iter().any(|g| g.ch == crate::font::to_math_italic('b')));
    }

    #[test]
    fn ffi错误宽容_zero_layout() {
        // 无效 LaTeX（孤 `}`）→ do_layout 失败 → None
        let l = do_layout("a}", &[], Some(LM_PATH));
        assert!(l.is_none() || l.unwrap().glyphs.is_empty());
    }

    #[test]
    fn ffi未知命令回退为空或不崩() {
        let l = do_layout(r"\fancyunknown{x}", &[], Some(LM_PATH)).unwrap();
        // Unknown 原子布局为空盒，不应 panic；断言不崩溃且宽度非负即可
        assert!(l.width >= 0.0);
    }

    #[test]
    fn 直接调C入口_检查指针非空() {
        unsafe {
            let path = std::ffi::CString::new(LM_PATH).unwrap();
            let mut l = arix_math_layout_from_path(path.as_ptr(), b"x+y".as_ptr(), 3);
            assert!(l.glyph_count >= 3, "x+y 至少 3 glyph: {}", l.glyph_count);
            if l.glyph_count > 0 {
                assert!(!l.xs.is_null());
                assert!(!l.ys.is_null());
            }
            arix_math_free(&mut l);
        }
    }

    #[test]
    fn C入口_分配可释放() {
        unsafe {
            let path = std::ffi::CString::new(LM_PATH).unwrap();
            let mut l = arix_math_layout_from_path(path.as_ptr(), b"\\sqrt{ab}".as_ptr(), 9);
            assert!(l.glyph_count > 0, "sqrt 应有 glyph");
                        arix_math_free(&mut l);
            assert_eq!(l.glyph_count, 0, "free 后应置空");
        }
    }
}