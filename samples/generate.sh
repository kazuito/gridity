#!/usr/bin/env bash
# Regenerates samples/inputs. Requires typst, qpdf, ffmpeg, ImageMagick (magick) and python3.
set -euo pipefail

cd "$(dirname "$0")"
root=$(cd .. && pwd)
font="$root/assets/fonts/BIZUDGothic-Regular.ttf"
in=inputs
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

rm -rf "$in"
mkdir -p "$in/photos"

ff() { ffmpeg -v error -y "$@"; }
im() { magick "$@"; }
png_out() { echo "PNG:$1"; }
im_text() {
  im -size "$1" "$2" -font "$font" -gravity center -pointsize "$3" \
    -fill white -stroke black -strokewidth 2 -annotate 0 "$4" \
    -strip -define png:exclude-chunks=date,time,tIME "$5"
}

# PDF: 12-page A4 report
typst compile --font-path "$root/assets/fonts" --creation-timestamp 0 \
  sources/report.typ "$in/report.pdf"

# PDF: A4 portrait, Letter, A4 landscape, a page with /Rotate 90 and a wide slide
typst compile --font-path "$root/assets/fonts" --creation-timestamp 0 \
  sources/mixed.typ "$work/mixed.pdf"
qpdf --deterministic-id --rotate=+90:4 "$work/mixed.pdf" "$in/mixed.pdf"

# Video: 16:9 with a running clock (testsrc draws the seconds counter)
video_args=(-c:v mpeg4 -q:v 8 -map_metadata -1 -fflags +bitexact -flags +bitexact)
ff -f lavfi -i "testsrc=duration=20:size=640x360:rate=10" "${video_args[@]}" "$in/clip.mp4"

# Video: portrait content stored as 640x360 turned clockwise, with display rotation 90
# (counterclockwise); the counter digits are upright only when rotation is applied
ff -f lavfi -i "testsrc=duration=12:size=360x640:rate=10" -vf "transpose=clock" \
  "${video_args[@]}" "$work/rotated.mp4"
ff -display_rotation 90 -i "$work/rotated.mp4" -c copy -map_metadata -1 -fflags +bitexact "$in/rotated.mp4"

# Video: 640x360 content squeezed to 480x360 with SAR 4:3; the circle is round only when
# the SAR is applied
ff -f lavfi -i "testsrc=duration=8:size=640x360:rate=10" \
  -vf "scale=480:360,setsar=4/3" "${video_args[@]}" "$in/anamorphic.mp4"

# GIF: 24 small frames at 100 ms
ff -f lavfi -i "testsrc=duration=2.4:size=200x150:rate=10" \
  -vf "split[a][b];[a]palettegen=max_colors=64[p];[b][p]paletteuse" \
  -map_metadata -1 -fflags +bitexact "$in/counter.gif"

# APNG: 10 frames at 200 ms, looping
ff -f lavfi -i "mandelbrot=size=160x120:rate=5:end_scale=0.05" -t 2 -plays 0 \
  -map_metadata -1 -fflags +bitexact -f apng "$in/zoom.png"

# Images: mixed aspect ratios; img2 before img10 checks natural sort
im_text 1600x900 "gradient:#1e3a8a-#60a5fa" 120 "img1  16:9" "$(png_out "$in/photos/img1.png")"
im_text 900x1600 "gradient:#14532d-#86efac" 110 "img2\n9:16" "$(png_out "$in/photos/img2.png")"
im_text 1200x1200 "gradient:#7c2d12-#fdba74" 120 "img10\n1:1" "$(png_out "$in/photos/img10.png")"
im_text 2400x600 "gradient:#581c87-#d8b4fe" 120 "img11  4:1 panorama" "$(png_out "$in/photos/img11.png")"
im_text 96x64 "xc:#334155" 14 "tiny\n96x64" "$(png_out "$in/photos/tiny.png")"

# Images: stored rotated with EXIF Orientation 6; must display upright with the arrow on top
im_text 800x600 "gradient:#831843-#f9a8d4" 90 "↑ TOP\nEXIF 6" "$work/upright.png"
im "$work/upright.png" -rotate -90 -strip -quality 85 "$work/stored.jpg"
python3 - "$work/stored.jpg" "$in/photos/exif-rotated.jpg" <<'PY'
import struct, sys
jpeg = open(sys.argv[1], "rb").read()
tiff = b"MM\0*" + struct.pack(">IH", 8, 1) + struct.pack(">HHIHH", 0x0112, 3, 1, 6, 0) + b"\0" * 4
payload = b"Exif\0\0" + tiff
app1 = b"\xff\xe1" + struct.pack(">H", len(payload) + 2) + payload
open(sys.argv[2], "wb").write(jpeg[:2] + app1 + jpeg[2:])
PY

# Screenshots for the even-split case (10 items at --per-sheet 8 -> 5 + 5)
mkdir -p "$in/shots"
for i in $(seq -w 1 10); do
  hue=$(( (10#$i * 36) % 360 ))
  im_text 960x600 "xc:hsl($hue,60%,40%)" 120 "shot $i" "$(png_out "$in/shots/shot-$i.png")"
done

echo "wrote $(find "$in" -type f | wc -l | tr -d ' ') files, $(du -sh "$in" | cut -f1)"
