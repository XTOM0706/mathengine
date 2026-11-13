#!/usr/bin/env python3
"""把 Latin Modern Math 裁成只含引擎会用到的字符。

用法：
    python3 tools/subset_font.py 源字体.otf 输出.otf
    python3 tools/subset_font.py 源字体.otf assets/latinmodern-math.otf

为什么需要：完整字体 717 KB，其中绝大部分是公式里永远用不到的字形。
裁到下面这个范围后约 257 KB，测试全部照常通过。

保留范围与理由：
    U+0020-007E  ASCII：数字、拉丁字母、基础标点与运算符
    U+00A0-00FF  Latin-1：带重音字母、× ÷ ±
    U+0370-03FF  希腊字母：\\alpha 到 \\omega、\\Gamma 到 \\Omega
    U+2000-22FF  通用标点 + 数学运算符：∑ ∫ √ ≤ ≥ ≠ ∞ ⟶ 等全部在这里
    U+2100-214F  字母式符号：ℝ ℓ ℏ（\\mathbb 用）
    U+1D434-1D467 数学斜体 A-Z / a-z：变量斜体的码点映射目标

不保留：CJK（宿主回退系统字体）、装饰符号、箭头区以外的象形符号。
`to_math_italic` 只映射到上述斜体区间，所以不需要整个 U+1D400-1D7FF。
"""

import subprocess
import sys

RANGES = ",".join([
    "U+0020-007E",
    "U+00A0-00FF",
    "U+0370-03FF",
    "U+2000-22FF",
    "U+2100-214F",
    "U+1D434-1D467",
])


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 1
    src, dst = sys.argv[1], sys.argv[2]
    cmd = [
        "pyftsubset", src,
        f"--unicodes={RANGES}",
        "--layout-features=*",
        f"--output-file={dst}",
    ]
    print(" ".join(cmd))
    r = subprocess.run(cmd)
    if r.returncode != 0:
        return r.returncode

    from fontTools.ttLib import TTFont
    f = TTFont(dst)
    print(f"字形数 {len(f.getGlyphOrder())}，MATH 表 {'保留' if 'MATH' in f else '缺失'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
