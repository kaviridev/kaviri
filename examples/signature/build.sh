#!/bin/sh
# Rebuilds site/email/signature.gif, the animated email footer and signature.
# Top: the brand row (brand.html). Below, with room to breathe: a real kaviri take of app.html
# on the brand-green backdrop, as wide as the strip so it stays legible on a phone.
# The GIF is 720px wide for a 360px slot: sharp on a phone, light enough for an inbox.
# Run from the repo root: sh examples/signature/build.sh
set -eu
D=examples/signature
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
./target/release/kaviri record --script $D/take.jsonl --width 640 --height 360 --scale 2 \
  --out-width 1280 --out-height 720 --background brand/backgrounds/take-green.png --loop 400 \
  --out "$T/take.mp4"
${KAVIRI_CHROMIUM:-chromium} --headless=new --no-sandbox --hide-scrollbars --window-size=480,350 \
  --force-device-scale-factor=2 --virtual-time-budget=2000 \
  --screenshot="$PWD/$D/.brand.png" "file://$PWD/$D/brand.html" >/dev/null 2>&1
mv "$D/.brand.png" "$T/brand.png"
python3 -c "
from PIL import Image, ImageDraw
m=Image.new('L',(960,540),0); ImageDraw.Draw(m).rounded_rectangle((0,0,959,539),radius=30,fill=255); m.save('$T/mask.png')"
# 960x700 canvas: brand row in the top 120px, a 40px gap, then the 960x540 take.
ffmpeg -v error -y -loop 1 -i "$T/brand.png" -i "$T/take.mp4" -loop 1 -i "$T/mask.png" -filter_complex \
  "[1:v]scale=960:540:flags=lanczos,fps=12[t];[t][2:v]alphamerge[tr];[0:v][tr]overlay=0:160:shortest=1,format=rgb24" \
  -t 7 -an "$T/sig.mp4"
ffmpeg -v error -y -i "$T/sig.mp4" -vf \
  "fps=10,scale=720:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=96:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle" \
  -loop 0 site/email/signature.gif
echo "site/email/signature.gif: $(wc -c < site/email/signature.gif) bytes"
