#!/bin/bash
# 用 piper 生成旁白音轨，并与背景音乐混音。
#
# 用法：bash tools/make_voiceover.sh <视频> <配乐> <台词表> <输出>
#   台词表每行：开始秒|文本
#   例：0|这是一个用 Rust 从零实现的数学公式排版引擎。
#
# 依赖：~/.local/venvs/piper/bin/piper、~/.local/share/piper/zh_CN-huayan-medium.onnx
set -e

VIDEO=${1:?视频}
MUSIC=${2:?配乐}
LINES=${3:?台词表}
OUT=${4:?输出}

PIPER=~/.local/venvs/piper/bin/piper
MODEL=~/.local/share/piper/zh_CN-huayan-medium.onnx
CFG=~/.local/share/piper/zh_CN-huayan-medium.onnx.json
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

[ -x "$PIPER" ] || { echo "缺少 piper：$PIPER"; exit 1; }
[ -f "$MODEL" ] || { echo "缺少模型：$MODEL"; exit 1; }

DUR=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$VIDEO")
echo "视频时长 ${DUR}s"

# 逐段合成，记录每段的起始时间
inputs=()
delays=()
i=0
while IFS='|' read -r start text; do
    [ -z "$text" ] && continue
    i=$((i+1))
    wav="$WORK/v$i.wav"
    printf '%s' "$text" | "$PIPER" -m "$MODEL" -c "$CFG" -f "$wav" --sentence-silence 0.15 2>/dev/null
    # 统一重采样到 44.1k 单声道，与配乐同格式
    ffmpeg -v error -i "$wav" -ar 44100 -ac 1 -c:a pcm_s16le -y "$WORK/n$i.wav"
    d=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$WORK/n$i.wav")
    echo "  [$start s] ${d}s  $text"
    inputs+=(-i "$WORK/n$i.wav")
    delays+=("[$((${i}-1)):a]adelay=${start}s:all=1,volume=1.0[v$i]")
done < "$LINES"

[ "$i" -gt 0 ] || { echo "台词表为空"; exit 1; }

# 旁白按时间叠加成一条
filter=""
for n in $(seq 1 "$i"); do filter+="${delays[$((n-1))]};"; done
labels=""
for n in $(seq 1 "$i"); do labels+="[v$n]"; done
filter+="${labels}amix=inputs=$i:normalize=0:dropout_transition=0[voice]"
ffmpeg -v error "${inputs[@]}" -filter_complex "$filter" -map "[voice]" -ar 44100 -ac 1 -y "$WORK/voice.wav"

# 旁白 + 配乐（配乐压低，避免盖住人声）
ffmpeg -v error -i "$WORK/voice.wav" -i "$MUSIC" -filter_complex \
  "[0:a]volume=0.85[v];[1:a]volume=0.25[m];[v][m]amix=inputs=2:normalize=0:duration=longest[a]" \
  -map "[a]" -ar 44100 -ac 2 -y "$WORK/mix.wav"

# 合成进视频
ffmpeg -v error -i "$VIDEO" -i "$WORK/mix.wav" -map 0:v -map 1:a \
  -c:v copy -c:a aac -b:a 160k -shortest -y "$OUT"

echo "完成：$OUT"
ffprobe -v error -show_entries format=duration,size -show_entries stream=codec_type,channels -of default=nw=1 "$OUT"
