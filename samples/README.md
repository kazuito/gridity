# Samples

Inputs for checking rendered sheets by eye. Integration tests do not use them; `tests/cli.rs`
generates its own fixtures.

```sh
samples/render.sh             # render every case into samples/out/, then open out/index.html
samples/render.sh video       # only the cases whose name contains "video"
samples/generate.sh           # regenerate samples/inputs (typst, qpdf, ffmpeg, magick, python3)
```

`samples/out/` is gitignored. Each render case directory holds the sheets, `manifest.json`
(`--json`), `command.txt` and `stderr.txt` (warnings). The `probe-*` and `text-*` cases
write the command's stdout to `output.txt` instead, shown inline in `index.html`.

## Inputs

| File | What to check |
|---|---|
| `report.pdf` | 12 A4 pages with text, Japanese, a chart and a table; page number in the footer |
| `mixed.pdf` | A4, US Letter, A4 landscape, a page with `/Rotate 90` (top edge ends up on the right), a 16:9 slide |
| `clip.mp4` | 20 s 16:9 video; the counter shows whole seconds to compare with the time labels |
| `rotated.mp4` | display rotation 90°; the counter digits are upright only when rotation is applied |
| `anamorphic.mp4` | 480x360 stored with SAR 4:3; the circle is round only when the SAR is applied |
| `counter.gif` | 24 frames at 100 ms, 200x150 (cells must not be upscaled) |
| `zoom.png` | APNG, 10 frames at 200 ms |
| `photos/` | mixed aspect ratios, natural sort (`img2` before `img10`), a 96x64 image that must not be upscaled, a JPEG with EXIF orientation 6 that must show "↑ TOP" upright |
| `shots/` | 10 screenshots; `--per-sheet 8` must split them 5 + 5 |

The `header-*` cases render narrow sheets with a long file name (a link to `report.pdf`)
or title: the header must keep `p.1–2 of 12 · sheet 1/2` and shorten only the name.

The `intl-*` cases label copies of a screenshot with Korean, Arabic, Thai, emoji and
Simplified Chinese file names. With the embedded font they show boxes and a warning listing
the characters; `intl-fallback-font` (macOS, Arial Unicode as `--font`) must draw all but
the emoji. Arabic and Thai are not shaped.

The PDFs are built from `sources/*.typ`. Add a case by adding a `case_` (render) or `text_case_` (other commands) line in `render.sh`
and, if it needs a new input, a step in `generate.sh`.
