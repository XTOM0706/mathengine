#!/usr/bin/env python3
"""生成演示视频的背景音乐（纯 Python 合成，不依赖任何音频库，也没有版权问题）。

用法：
    python3 tools/make_bgm.py 46 out.wav

设计：
  - A 小调四和弦循环 Am → F → C → G，每 4 秒一个
  - 三声部：和弦垫（长音）、琶音（每 0.5 秒一个音）、低音（根音）
  - 用一次性延迟做假混响，整体淡入 3 秒、淡出 4 秒
  - 峰值控制在 -12 dB 左右，留足人声空间

为什么要合成而不是找现成曲子：比赛视频用别人的音乐要授权，免费曲库的授权条款
又要写进材料里。自己合成，来源清楚、可复现、零许可风险。
"""

import math
import struct
import sys
import wave

RATE = 44100

# 和弦：(根音, 三音, 五音)，单位 Hz
CHORDS = [
    (110.00, 130.81, 164.81),   # Am: A2 C3 E3
    (87.31, 110.00, 130.81),    # F : F2 A2 C3
    (130.81, 164.81, 196.00),   # C : C3 E3 G3
    (98.00, 123.47, 146.83),    # G : G2 B2 D3
]


def env(t: float, dur: float, attack: float, release: float) -> float:
    if t < attack:
        return t / attack
    if t > dur - release:
        return max(0.0, (dur - t) / release)
    return 1.0


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 1
    dur = float(sys.argv[1])
    out_path = sys.argv[2]
    n = int(RATE * dur)
    buf = [0.0] * n

    bar = 4.0
    for i in range(n):
        t = i / RATE
        bar_idx = int(t / bar) % len(CHORDS)
        root, third, fifth = CHORDS[bar_idx]
        tb = t % bar                      # 本小节内的时间
        bar_env = env(tb, bar, 0.35, 0.6)  # 小节之间的呼吸

        s = 0.0
        # 和弦垫：三个音，轻微失谐让声音厚一点
        s += 0.030 * math.sin(2 * math.pi * root * 2 * t)
        s += 0.024 * math.sin(2 * math.pi * third * 2 * t + 0.4)
        s += 0.020 * math.sin(2 * math.pi * fifth * 2 * t + 0.9)
        s += 0.012 * math.sin(2 * math.pi * root * 4.01 * t)   # 轻微失谐的高八度
        s *= bar_env

        # 琶音：每 0.5 秒一个音，取和弦音的高八度，指数衰减
        step = 0.5
        note_t = t % step
        note_idx = int(t / step) % 3
        freq = (root, third, fifth)[note_idx] * 4
        decay = math.exp(-note_t * 6.0)
        s += 0.028 * math.sin(2 * math.pi * freq * note_t) * decay

        # 低音：每小节第一拍
        if tb < 0.9:
            bass_env = math.exp(-tb * 2.5)
            s += 0.045 * math.sin(2 * math.pi * root * 0.5 * t) * bass_env

        buf[i] = s

    # 假混响：0.13 秒延迟，衰减 0.28
    delay = int(0.13 * RATE)
    for i in range(delay, n):
        buf[i] += buf[i - delay] * 0.28

    # 总淡入淡出
    fade_in = 3.0
    fade_out = 4.0
    for i in range(n):
        t = i / RATE
        g = 1.0
        if t < fade_in:
            g = t / fade_in
        if t > dur - fade_out:
            g = min(g, max(0.0, (dur - t) / fade_out))
        buf[i] *= g

    peak = max(abs(v) for v in buf) or 1.0
    scale = 0.25 / peak          # 峰值约 -12 dB
    with wave.open(out_path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(
            struct.pack("<h", int(max(-1.0, min(1.0, v * scale)) * 32767)) for v in buf
        ))
    print(f"{out_path}: {dur:.0f}s, 峰值归一化到 -12 dB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
