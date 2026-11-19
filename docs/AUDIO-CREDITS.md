# 音频来源

演示视频 `docs/demo.mp4` 时长 188 秒，音轨由旁白与背景音乐合成。

> 现状：当前 `docs/demo.mp4` 是 188 秒（3 分 08 秒）的**无声版**，画面按下表的场景表重做：
> 封面 0–4、公式演示 4–60、复杂矩阵 60–106、性能对比 106–146、怎么做的 146–180、仓库 180–188。
> 下面的音频素材与署名适用于后续混音；`tools/voiceover.txt` 的时间点仍按旧的
> 46 秒版编排，混音前需要按这张新场景表重排。

## 旁白

edge-tts 的 `zh-CN-YunxiNeural`（男声），由 `tools/make_voiceover_edge.sh` 生成，台词见 `tools/voiceover.txt`。

- 为什么不用 piper：piper 的中文只有女声（huayan），要男声只能走 edge-tts。代价是必须联网，且接口非官方。
- 每段合成后做响度归一化（-16 LUFS）。piper 的输出电平不稳定，实测有段落峰值只有 -40 dB，不归一化会出现「只剩背景音乐」。
- **朗读文本同时以字幕烧进画面**（ASS + Noto Sans CJK SC），字幕在底部，页脚之上，不与页脚叠字。

## 背景音乐

| 项 | 值 |
|---|---|
| 曲名 | Feel me |
| 作者 | LiQWYD |
| 中间来源 | B 站 `BV1Lg411X7Ce`（UP：无版权音乐口袋） |
| 原作者频道 | https://www.youtube.com/watch?v=4fDbwGS5oa8 |
| 使用片段 | 原曲中段，长度与成片一致 |

**署名要求**：LiQWYD 的作品通常以 CC-BY 发布，使用时须标注「Music by LiQWYD」。正式提交时请把这一行放进视频简介或附件说明；若组委会要求书面授权，联系原作者或改用 CC0 曲目。

## 兜底方案

`tools/make_bgm.py` 可以用纯 Python 合成一段 A 小调四和弦的背景音乐，零许可风险。命令：

```bash
python3 tools/make_bgm.py 70 /tmp/bgm.wav
```

## 处理方式

音乐音量压到旁白的四分之一，两端淡入淡出；旁白按场景时间点叠加，全部重采样到 44.1 kHz。
