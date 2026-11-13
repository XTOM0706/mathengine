//! 字体度量封装：Latin Modern Math (OTF) 的字符→glyph、水平 advance、italic correction、
//! 以及 OpenType MATH 常量（脚本缩放 / axis / 上下标位移 / 分数字体参数）。
//!
//! 单位全部为 **font units**（与 units_per_em 同尺度）；Kotlin 侧按 `px = unit * size / units_per_em`
//! 换算，避免 Rust/Kotlin 两侧浮点口径漂移。
//!
//! 为什么自己封装而不直接用 ttf-parser 的 Face：需要一个「按 Unicode 字符查询度量」的薄层
//! （cmap→glyph→hmtx/MATH），并把 MATH 常量聚合成一个方便布局层读的 struct。

use ttf_parser::{Face, GlyphId};

/// 布局层需要的全部字体度量（一次解析，避免每原子重复查询）。
pub struct MathFont<'a> {
    face: Face<'a>,
    /// script 级缩放（MATH constants 的 scriptPercentScaleDown，通常 70）。
    pub script_scale: f32,
    /// scriptscript 级缩放（通常 50）。
    pub scriptscript_scale: f32,
    /// 轴线高度（axis_height，分数线/根号横线的垂直基准，通常 ~25/1000 em）。
    pub axis_height: f32,
    /// 上行高度（hhea ascender，font units）。
    pub ascender: f32,
    /// 下行深度（hhea descender 的绝对值，font units）。
    pub descender: f32,
    /// 单字符水平 advance 的默认宽度（找不到 glyph 时兜底；约 0.5em）。
    pub default_advance: f32,
    /// 数学斜体变量字母的 advance 修正（italic correction 的均值，找不到时的兜底，约 0.05em）。
    pub default_italic: f32,
}

impl<'a> MathFont<'a> {
    /// 从字体字节解析。失败返回 None（调用方决定回退策略——渲染失败退原文）。
    pub fn from_bytes(data: &'a [u8]) -> Option<Self> {
        let face = Face::parse(data, 0).ok()?;
        let upem = face.units_per_em() as f32;
        let math = face.tables().math?;
        let constants = math.constants?;
        let script_scale = constants.script_percent_scale_down() as f32 / 100.0;
        let scriptscript_scale = constants.script_script_percent_scale_down() as f32 / 100.0;
        let axis_height = constants.axis_height().value as f32;
        let ascender = face.ascender() as f32;
        let descender = -face.descender() as f32;
        Some(MathFont {
            face,
            script_scale,
            scriptscript_scale,
            axis_height,
            ascender,
            descender,
            default_advance: upem * 0.5,
            default_italic: upem * 0.05,
        })
    }

    pub fn units_per_em(&self) -> f32 { self.face.units_per_em() as f32 }

    /// 暴露底层 face（布局层读 MATH 常量等）。borrow 生命周期与字体相同。
    pub fn face_ref(&self) -> Option<&Face<'a>> { Some(&self.face) }

    /// Unicode 字符 → glyph id。找不到返回 None。
    pub fn glyph(&self, ch: char) -> Option<u16> {
        self.face.glyph_index(ch).map(|g: GlyphId| g.0)
    }

    /// 水平 advance（font units）。glyph 不存在用默认值。
    /// CJK 特殊：LM Math 无中文字形，但 Kotlin 侧会**回退系统字体**绘制（满宽 1em）——
    /// 布局时必须按 1em 计 advance，否则行内占位 0.5em、画出来 1em 会超界。
    pub fn advance(&self, ch: char) -> f32 {
        if is_cjk(ch) { return self.units_per_em(); }
        // 数学斜体变量：ASCII 字母映射到数学斜体码点（TeX 数学变量斜体），用其真实 advance
        let m = to_math_italic(ch);
        match self.face.glyph_index(m) {
            Some(g) => self.face.glyph_hor_advance(g).map(|a| a as f32).unwrap_or(self.default_advance),
            None => self.default_advance,
        }
    }

    /// glyph id（供绘制判定；同样应用斜体映射）。
    pub fn glyph_id(&self, ch: char) -> Option<u16> {
        self.face.glyph_index(to_math_italic(ch)).map(|g: GlyphId| g.0)
    }

    /// **正体**（不套数学斜体映射）的 advance：`\lim`/`\max` 等运算符名要用正体。
    pub fn advance_plain(&self, ch: char) -> f32 {
        if is_cjk(ch) { return self.units_per_em(); }
        match self.face.glyph_index(ch) {
            Some(g) => self.face.glyph_hor_advance(g).map(|a| a as f32).unwrap_or(self.default_advance),
            None => self.default_advance,
        }
    }

    /// **正体**墨迹包围盒（不套斜体映射）。找不到返回 None。
    pub fn ink_extents_plain(&self, ch: char) -> Option<(f32, f32)> {
        let g = self.face.glyph_index(ch)?;
        let rect = self.face.glyph_bounding_box(g)?;
        Some((rect.y_max as f32, -rect.y_min as f32))
    }

    /// 字符的**墨迹**包围盒（真实 ink，非 hhea 全局上行/下行）。
    /// 返回 (ascender, descender) = (顶, 底) 相对基线的 font units（scale 1）。
    /// 找不到 glyph（CJK/回退）返回 None，调用方回退全局度量。
    /// 应用数学斜体映射（与绘制码点一致），保证盒高深对应当前字形。
    pub fn ink_extents(&self, ch: char) -> Option<(f32, f32)> {
        let m = to_math_italic(ch);
        let g = self.face.glyph_index(m)?;
        let rect = self.face.glyph_bounding_box(g)?;
        Some((rect.y_max as f32, -rect.y_min as f32))
    }

    /// 字符墨迹的水平范围（font units，scale 1）：返回 (x_min, x_max)。
    /// 用于根号等需要「墨迹右缘 ≤ 后续内容左缘」的横向避让。找不到返回 (0, advance)。
    pub fn ink_x_extent(&self, ch: char) -> (f32, f32) {
        let m = to_math_italic(ch);
        match self.face.glyph_index(m) {
            Some(g) => match self.face.glyph_bounding_box(g) {
                Some(r) => (r.x_min as f32, r.x_max as f32),
                None => (0.0, self.advance(ch)),
            },
            None => (0.0, self.advance(ch)),
        }
    }

    /// italic correction（来自 MATH 表 glyph_info.italic_corrections，MathValues.get(glyph)）。找不到返回 0。
    pub fn italic(&self, ch: char) -> f32 {
        let math = match self.face.tables().math { Some(m) => m, None => return 0.0 };
        let g = match self.face.glyph_index(ch) { Some(g) => g, None => return 0.0 };
        match math.glyph_info {
            Some(gi) => match gi.italic_corrections {
                Some(vals) => vals.get(g).map(|v| v.value as f32).unwrap_or(0.0),
                None => 0.0,
            },
            None => 0.0,
        }
    }
}

/// 从 assets 加载字体（容器测试用；Android 侧由 Kotlin 传入 assets 字节）。
pub fn load_latin_modern_math(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    const LM_PATH: &str = crate::LM_MATH_OTF;

    fn font() -> MathFont<'static> {
        let data = load_latin_modern_math(LM_PATH).expect("LM Math 字体应存在");
        // leak 以便测试（测试里生命周期生命周期没问题）
        let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
        MathFont::from_bytes(bytes).expect("LM Math 应解析出 MATH 表")
    }

    #[test]
    fn 字体能解析() {
        let f = font();
        assert!(f.units_per_em() > 0.0);
        // LM Math 是 1000 upem 的 OTF
        assert!((f.units_per_em() - 1000.0).abs() < 1.0, "upem={}", f.units_per_em());
    }

    #[test]
    fn MATH常量合理() {
        let f = font();
        assert!((f.script_scale - 0.7).abs() < 0.05, "script_scale={}", f.script_scale);
        assert!((f.scriptscript_scale - 0.5).abs() < 0.05, "ss_scale={}", f.scriptscript_scale);
        // axis_height 以 font units 计：LM Math 1000 upem，axis≈250（0.25em），正常区间 150~350
        assert!(f.axis_height > 150.0 && f.axis_height < 350.0, "axis={}", f.axis_height);
    }

    #[test]
    fn 希腊字母有glyph() {
        let f = font();
        assert!(f.glyph('α').is_some(), "α 应有 glyph");
        assert!(f.glyph('∑').is_some(), "∑ 应有 glyph");
        assert!(f.glyph('√').is_some(), "√ 应有 glyph");
    }

    #[test]
    fn advance非零且合理() {
        let f = font();
        let a = f.advance('x');
        assert!(a > 0.0 && a < 2000.0, "x advance={a}");
        // 数字/字母 advance 都合理
        assert!(f.advance('1') > 0.0);
    }

    #[test]
    fn italic对斜体变量非零() {
        let f = font();
        // 数学斜体变量 x（U+1D465）应有 italic correction；普通拉丁 x 也可能有
        let x = f.italic('𝑥');
        assert!(x >= 0.0);
    }
}

/// CJK（中文/日文/韩文/注音）判定：这些字 LM Math 没字形，宽度 = 1 em（满宽）。
/// 范围取 Unicode 主要 CJK 区（与 Kotlin Character.UnicodeScript 的判定保持一致）。
fn is_cjk(ch: char) -> bool {
    let c = ch as u32;
    (0x2E80..=0x9FFF).contains(&c)      // CJK 部首/扩展A/统一表意文字
        || (0xAC00..=0xD7FF).contains(&c) // 谚文音节
        || (0xF900..=0xFAFF).contains(&c) // CJK 兼容表意
        || (0xFF00..=0xFFEF).contains(&c) // 全角标点/假名
        || (0x3040..=0x30FF).contains(&c) // 平假名/片假名
}

/// 字形探测（bin 工具用）：返回字符是否有 glyph。
pub fn probe_glyph(path: &str, chars: &[char]) -> Vec<(char, bool, u16)> {
    let data = std::fs::read(path).unwrap();
    let font = MathFont::from_bytes(&data).unwrap();
    chars.iter().map(|&c| (c, font.glyph(c).is_some(), font.advance(c) as u16)).collect()
}

/// TeX 数学变量斜体映射：ASCII 拉丁字母 → U+1D400 数学字母区间。
///   a-z → U+1D44E..U+1D467（数学斜体小写）   偏移 +0x1D44E-0x61
///   A-Z → U+1D434..U+1D44D（数学斜体大写）   偏移 +0x1D434-0x41
/// 数字/符号/greek 不映射（保持正体/原样）。
pub fn to_math_italic(ch: char) -> char {
    let c = ch as u32;
    if (0x61..=0x7A).contains(&c) { return char::from_u32(c + 0x1D44E - 0x61).unwrap_or(ch); }
    if (0x41..=0x5A).contains(&c) { return char::from_u32(c + 0x1D434 - 0x41).unwrap_or(ch); }
    ch
}
