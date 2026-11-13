//! Android JNI 桥：`MathEngine.nativeLayout(latex, fontBytes)`。
//!
//! 字体字节每次传入（公式布局是低频操作，memcpy 级开销可忽略），不持全局状态，无锁。
//! 任何失败都返回 `null`，由 Kotlin 侧回退显示原文，绝不跨 extern 边界 panic。

use jni::objects::{JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jobject, jsize};
use jni::JNIEnv;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::layout::Layout;

#[no_mangle]
pub extern "system" fn Java_com_arix_math_MathEngine_nativeLayout(
    mut env: JNIEnv,
    _class: JClass,
    latex: JString,
    font_bytes: JByteArray,
) -> jobject {
    let r = catch_unwind(AssertUnwindSafe(|| -> jobject {
        let result = (|| -> Result<Layout, String> {
            let latex: String = env.get_string(&latex).map_err(|e| e.to_string())?.into();
            let bytes = env.convert_byte_array(&font_bytes).map_err(|e| e.to_string())?;
            let font = crate::font::MathFont::from_bytes(&bytes)
                .ok_or_else(|| "font parse failed".to_string())?;
            let ast = crate::ast::parse(&latex).map_err(|e| format!("{e:?}"))?;
            Ok(crate::layout::layout(&font, &ast))
        })();

        match result {
            Ok(l) => match build_layout_object(&mut env, &l) {
                Some(obj) => obj.into_raw(),
                None => std::ptr::null_mut(),
            },
            Err(_) => std::ptr::null_mut(),
        }
    }));
    r.unwrap_or(std::ptr::null_mut())
}

/// 把 [Layout] 包装成 `com.arix.math.MathEngine$Layout` 对象。
fn build_layout_object<'local>(env: &mut JNIEnv<'local>, l: &Layout) -> Option<JObject<'local>> {
    let class = env.find_class("com/arix/math/MathEngine$Layout").ok()?;

    let glyphs: Vec<i32> = l.glyphs.iter().map(|g| g.ch as i32).collect();
    let j_glyphs = env.new_int_array(glyphs.len() as jsize).ok()?;
    env.set_int_array_region(&j_glyphs, 0, &glyphs).ok()?;

    let j_xs = {
        let a = env.new_float_array(l.glyphs.len() as jsize).ok()?;
        let vals: Vec<f32> = l.glyphs.iter().map(|g| g.x).collect();
        env.set_float_array_region(&a, 0, &vals).ok()?;
        a
    };
    let j_ys = {
        let a = env.new_float_array(l.glyphs.len() as jsize).ok()?;
        let vals: Vec<f32> = l.glyphs.iter().map(|g| g.y).collect();
        env.set_float_array_region(&a, 0, &vals).ok()?;
        a
    };
    let j_scales = {
        let a = env.new_float_array(l.glyphs.len() as jsize).ok()?;
        let vals: Vec<f32> = l.glyphs.iter().map(|g| g.scale).collect();
        env.set_float_array_region(&a, 0, &vals).ok()?;
        a
    };
    let rules: Vec<f32> = l.rules.iter().flat_map(|r| [r.x, r.y, r.width, r.thickness]).collect();
    let j_rules = {
        let a = env.new_float_array(rules.len() as jsize).ok()?;
        env.set_float_array_region(&a, 0, &rules).ok()?;
        a
    };

    env.new_object(
        &class,
        "([I[F[F[F[FFFF)V",
        &[
            JValue::Object(&j_glyphs),
            JValue::Object(&j_xs),
            JValue::Object(&j_ys),
            JValue::Object(&j_scales),
            JValue::Object(&j_rules),
            JValue::Float(l.width),
            JValue::Float(l.height),
            JValue::Float(l.depth),
        ],
    )
    .ok()
}
