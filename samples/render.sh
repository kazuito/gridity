#!/usr/bin/env bash
# Renders every case below into samples/out/<case>/ (sheets + manifest.json)
# and writes samples/out/index.html for side-by-side review.
# Usage: samples/render.sh [case-name-substring]
set -euo pipefail

cd "$(dirname "$0")"
cargo build --release --quiet
G=../target/release/gridity
out=out

filter=${1:-}
failed=0
[[ -z $filter ]] && rm -rf "$out"
mkdir -p "$out"

case_() {
  local name=$1
  shift
  [[ -n $filter && $name != *"$filter"* ]] && return
  rm -rf "${out:?}/$name"
  mkdir -p "$out/$name"
  local cmd="gridity render" arg
  for arg in "$@"; do
    [[ $arg =~ [^A-Za-z0-9_./:,=-] ]] && arg="'$arg'"
    cmd+=" $arg"
  done
  echo "$cmd" >"$out/$name/command.txt"
  if $G render "$@" -o "$out/$name/" --json >"$out/$name/manifest.json" 2>"$out/$name/stderr.txt"; then
    echo "ok    $name"
  else
    echo "FAIL  $name (see $out/$name/stderr.txt)"
    failed=1
  fi
}

case_ pdf-report inputs/report.pdf
case_ pdf-report-low inputs/report.pdf --detail low
case_ pdf-report-high inputs/report.pdf --pages 1-4 --detail high
case_ pdf-page-hires inputs/report.pdf --pages 3 --per-sheet 1 --preset hires
case_ pdf-mixed inputs/mixed.pdf
case_ pdf-dark inputs/mixed.pdf --bg "#1f2937" --title "Dark background"
case_ video-clip inputs/clip.mp4
case_ video-range inputs/clip.mp4 --start 0:05 --end 0:15 --every 1s
case_ video-label-below inputs/clip.mp4 --count 6 --label "{index}/{total} {time}" --label-pos below
case_ video-rotated inputs/rotated.mp4
case_ video-anamorphic inputs/anamorphic.mp4
case_ gif inputs/counter.gif
case_ apng inputs/zoom.png
case_ images inputs/photos
case_ images-cols3 inputs/photos --cols 3 --label "{index}/{total} {name}"
case_ images-split inputs/shots --per-sheet 8
case_ images-no-header inputs/shots --no-header --no-label --detail low
case_ crop-pdf-rotated inputs/mixed.pdf --pages 3-4 --crop 0,0,0.5,0.5
case_ crop-pdf-zoom inputs/report.pdf --pages 1 --crop 0.05,0.05,0.9,0.3
case_ crop-images inputs/photos --crop 0,0,1,0.5
case_ crop-video-rotated inputs/rotated.mp4 --count 4 --crop 0,0.25,1,0.5
case_ crop-video-anamorphic inputs/anamorphic.mp4 --count 4 --crop 0,0,0.5,1
case_ crop-gif inputs/counter.gif --count 4 --crop 0.5,0,0.5,1

{
  echo '<!doctype html><meta charset="utf-8"><title>gridity samples</title>'
  echo '<style>body{font:14px system-ui;margin:24px;background:#f3f4f6}section{margin-bottom:40px}'
  echo 'code{background:#e5e7eb;padding:2px 6px;border-radius:4px}img{max-width:100%;border:1px solid #9ca3af;margin:8px 8px 0 0;vertical-align:top}'
  echo 'pre{color:#b45309;white-space:pre-wrap}</style>'
  for dir in "$out"/*/; do
    name=$(basename "$dir")
    echo "<section id=\"$name\"><h2>$name</h2><code>$(<"$dir/command.txt")</code>"
    echo "<a href=\"$name/manifest.json\">manifest.json</a>"
    [[ -s $dir/stderr.txt ]] && echo "<pre>$(sed 's/&/\&amp;/g; s/</\&lt;/g' "$dir/stderr.txt")</pre>"
    for img in "$dir"*.png "$dir"*.jpg; do
      [[ -e $img ]] && echo "<div><a href=\"$name/$(basename "$img")\"><img src=\"$name/$(basename "$img")\"></a></div>"
    done
    echo '</section>'
  done
} >"$out/index.html"

echo "open $(pwd)/$out/index.html"
exit $failed
