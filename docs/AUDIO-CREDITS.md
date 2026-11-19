# 音频来源

演示视频 `docs/demo.mp4` 的音轨由两部分合成。

## 旁白

piper（本地神经语音合成，MIT）中文模型 `zh_CN-huayan-medium`，由工具 `tools/make_voiceover.sh` 生成，台词见 `tools/voiceover.txt`。

## 背景音乐

| 项 | 值 |
|---|---|
| 曲名 | Upbeat Pop Background - Background Music For Videos (No Copyright) |
| 作者 | Background Music For Videos |
| 来源 | https://soundcloud.com/twisterium/upbeat-pop |
| 使用片段 | 原曲 30–76 秒，共 46 秒 |

**注意**：该曲目标题声明免版权，但 SoundCloud 平台的许可字段显示 `all-rights-reserved`。两者矛盾。正式提交前有两条路：

1. 联系作者取得书面许可（截图保存）；
2. 换成许可明确的作品（CC0 或 CC-BY，例如 Free Music Archive、Pixabay Music）。

自己合成的版本仍在 `tools/make_bgm.py`，零许可风险，可作为兜底。

## 处理方式

音乐音量压到旁白的四分之一，两端各留 2.5 秒淡入、4 秒淡出。旁白六段按场景时间点叠加，全部重采样到 44.1 kHz。
