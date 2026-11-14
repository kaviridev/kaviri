#!/bin/sh
# Rebuilds site/email/signature.gif, the animated email footer and signature.
# The moving half is a real kaviri take of app.html; the still half is brand.html.
# Run from the repo root: sh examples/signature/build.sh
set -eu
D=examples/signature
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
./target/release/kaviri record --script $D/take.jsonl --width 640 --height 400 --scale 2 \
  --out-width 1280 --out-height 800 --loop 400 --out "$T/take.mp4"
${KAVIRI_CHROMIUM:-chromium} --headless=new --no-sandbox --hide-scrollbars --window-size=600,160 \
  --force-device-scale-factor=2 --virtual-time-budget=2000 \
  --screenshot="$PWD/$D/.brand.png" "file://$PWD/$D/brand.html" >/dev/null 2>&1
mv "$D/.brand.png" "$T/brand.png"
python3 -c "
from PIL import Image, ImageDraw
m=Image.new('L',(448,280),0); ImageDraw.Draw(m).rounded_rectangle((0,0,447,279),radius=22,fill=255); m.save('$T/mask.png')"
ffmpeg -v error -y -loop 1 -i "$T/brand.png" -i "$T/take.mp4" -loop 1 -i "$T/mask.png" -filter_complex \
  "[1:v]scale=448:280:flags=lanczos,fps=12[t];[t][2:v]alphamerge[tr];[0:v][tr]overlay=736:20:shortest=1,format=rgb24" \
  -t 7 -an "$T/sig.mp4"
ffmpeg -v error -y -i "$T/sig.mp4" -vf \
  "fps=12,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle" \
  -loop 0 site/email/signature.gif
echo "site/email/signature.gif: $(wc -c < site/email/signature.gif) bytes"
