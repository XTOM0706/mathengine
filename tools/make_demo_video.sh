#!/usr/bin/env bash
# 生成 docs/demo.mp4：逐帧 SVG → PNG（rsvg-convert）→ H.264（ffmpeg）。
#
#   tools/make_demo_video.sh [输出路径]
#
# 公式图形全部来自本引擎：examples/video_frames.rs 调 mathengine::layout 算布局，
# 再向字体索取字形轮廓画成 path。中文说明文字排进 SVG <text>，不用 ffmpeg drawtext。
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$repo/docs/demo.mp4}
work=${WORK:-/tmp/mathengine-demo}
svgd="$work/svg"
pngd="$work/png"
mkdir -p "$svgd" "$pngd"
# 清掉上一轮留下的帧：帧数校验只应数这一轮生成的文件
find "$svgd" "$pngd" -maxdepth 1 -type f \( -name 'f*.svg' -o -name 'f*.png' \) -delete

echo "== 1/3 生成帧（SVG）"
cargo run --release --manifest-path "$repo/Cargo.toml" --example video_frames -- "$svgd"

echo "== 2/3 光栅化（rsvg-convert，$(nproc) 路并行）"
ls "$svgd"/f*.svg | xargs -P "$(nproc)" -I{} sh -c \
  'b=$(basename {} .svg); /usr/sbin/rsvg-convert -w 1920 -h 1080 "{}" -o "'"$pngd"'/$b.png"'

n=$(ls "$pngd"/f*.png | wc -l)
FRAMES=2175
if [ "$n" -ne "$FRAMES" ]; then
  echo "帧数不对：$n / $FRAMES（目录里可能有旧帧）" >&2
  exit 1
fi

echo "== 3/3 编码（libx264 / yuv420p / 30fps）"
/usr/sbin/ffmpeg -hide_banner -loglevel warning -y \
  -framerate 30 -i "$pngd/f%05d.png" \
  -c:v libx264 -profile:v high -level 4.1 -pix_fmt yuv420p \
  -tune animation -crf 16 -preset slow -r 30 -movflags +faststart \
  "$out"

/usr/sbin/ffprobe -v error -show_entries format=duration,size \
  -show_entries stream=width,height,r_frame_rate,pix_fmt,nb_frames \
  -of default=noprint_wrappers=1 "$out"
