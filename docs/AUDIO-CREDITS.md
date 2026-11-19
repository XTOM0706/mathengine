# 音频来源

演示视频 `docs/demo.mp4` 的音轨由旁白与背景音乐合成。

## 旁白

piper（本地神经语音合成）中文模型 `zh_CN-huayan-medium`，由 `tools/make_voiceover.sh` 生成，台词见 `tools/voiceover.txt`。每段合成后做响度归一化（-16 LUFS）——piper 的输出电平不稳定，实测有段落峰值只有 -40 dB，不归一化会出现「只剩背景音乐」。

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
