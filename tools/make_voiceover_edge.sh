#!/bin/bash
# 用 edge-tts 的男声生成旁白，并把朗读文本作为字幕烧进画面，最后与配乐混音。
#
# 用法：bash tools/make_voiceover_edge.sh <视频> <配乐> <台词表> <输出>
#   台词表每行：开始秒|文本
#
# 为什么不用 piper：piper 的中文只有女声（huayan），要男声只能走 edge-tts。
# 这台机器的代理走 127.0.0.1:7890，脚本里显式传，不依赖全局代理变量。
set -e

VIDEO=${1:?视频}
MUSIC=${2:?配乐}
LINES=${3:?台词表}
OUT=${4:?输出}

EDGE=~/.local/venvs/piper/bin/edge-tts
VOICE=zh-CN-YunxiNeural
FONT="Noto Sans CJK SC"
PROXY=http://127.0.0.1:7890
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

[ -x "$EDGE" ] || { echo "缺少 edge-tts：$EDGE"; exit 1; }

DUR=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$VIDEO")
echo "视频时长 ${DUR}s，音色 $VOICE"

inputs=()
delays=()
ass="$WORK/sub.ass"
{
  echo "[Script Info]"
  echo "ScriptType: v4.00+"
  echo "PlayResX: 1920"
  echo "PlayResY: 1080"
  echo "WrapStyle: 2"
  echo
  echo "[V4+ Styles]"
  echo "Format: Name, Fontname, Fontsize, PrimaryColour, BackColour, Bold, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding"
  echo "Style: Sub,${FONT},44,&H00FFFFFF,&H80000000,0,3,2,0,2,80,80,150,1"
  echo
  echo "[Events]"
  echo "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text"
} > "$ass"

ass_time() {
  awk -v t="$1" 'BEGIN{
    h=int(t/3600); m=int((t-h*3600)/60); s=t-h*3600-m*60;
    printf "%d:%02d:%05.2f", h, m, s
  }'
}

i=0
while IFS='|' read -r start text; do
    [ -z "$text" ] && continue
    i=$((i+1))
    mp3="$WORK/v$i.mp3"
    env -u no_proxy -u NO_PROXY https_proxy=$PROXY http_proxy=$PROXY \
      "$EDGE" --voice "$VOICE" --text "$text" --write-media "$mp3" 2>/dev/null
    ffmpeg -v error -i "$mp3" -af "loudnorm=I=-16:TP=-1.5:LRA=11" \
      -ar 44100 -ac 1 -c:a pcm_s16le -y "$WORK/n$i.wav"
    d=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$WORK/n$i.wav")
    echo "  [$start s] ${d}s  $text"

    end=$(awk -v a="$start" -v b="$d" 'BEGIN{print a+b+0.4}')
    echo "Dialogue: 0,$(ass_time "$start"),$(ass_time "$end"),Sub,,0,0,0,,$text" >> "$ass"

    inputs+=(-i "$WORK/n$i.wav")
    delays+=("[$((${i}-1)):a]adelay=${start}s:all=1,volume=1.0[v$i]")
done < "$LINES"

[ "$i" -gt 0 ] || { echo "台词表为空"; exit 1; }

filter=""
labels=""
for n in $(seq 1 "$i"); do
  filter+="${delays[$((n-1))]};"
  labels+="[v$n]"
done
filter+="${labels}amix=inputs=$i:normalize=0:dropout_transition=0[voice]"
ffmpeg -v error "${inputs[@]}" -filter_complex "$filter" -map "[voice]" -ar 44100 -ac 1 -y "$WORK/voice.wav"

ffmpeg -v error -i "$WORK/voice.wav" -i "$MUSIC" -filter_complex \
  "[0:a]volume=0.95[v];[1:a]volume=0.22[m];[v][m]amix=inputs=2:normalize=0:duration=longest[a]" \
  -map "[a]" -ar 44100 -ac 2 -y "$WORK/mix.wav"

ffmpeg -v error -i "$VIDEO" -i "$WORK/mix.wav" \
  -vf "subtitles=${ass}:fontsdir=/usr/share/fonts/noto-cjk" \
  -map 0:v -map 1:a -c:v libx264 -crf 18 -preset medium -pix_fmt yuv420p \
  -c:a aac -b:a 160k -shortest -y "$OUT"

echo "完成：$OUT"
ffprobe -v error -show_entries format=duration,size -show_entries stream=codec_type -of default=nw=1 "$OUT"
