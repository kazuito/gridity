# Layout and style

## Size limits (`--preset`)

Every sheet is sized so Claude does not downscale it, which keeps cell coordinates exact.

| Preset | Max edge | Max visual tokens | Use for |
|---|---|---|---|
| `compat` (default) | 1568 px | 1568 | Every Claude model; safe for other LLMs |
| `hires` | 2576 px | 4784 | Claude 4.7 and later models |

A request with more than 20 images (counting earlier turns and tool results) rejects
every image larger than 2000 px, so use `compat` for such requests; `hires` sheets above
2000 px get a warning. Limits are from
<https://platform.claude.com/docs/en/build-with-claude/vision> (checked 2026-10-22).

Tokens are counted as `ceil(w/28) * ceil(h/28)` (`limits.rule` in the manifest:
`claude-28px-patch`). Override with `--max-edge` (28-8000) and `--max-tokens` (1-81225)
to approximate other models' limits; their own token counts differ from `est_tokens`.
Claude rejects images larger than 8000 px.

## Items per sheet

`--detail` sets the smallest allowed cell, which decides how many items share a sheet:

| Input | low | medium (default) | high |
|---|---|---|---|
| PDF (A4/Letter) | 9 | 4 | 2 |
| Video 16:9 | 16 | 12 | 6 |
| Images 4:3 | 12 | 6 | 3 |

`hires` fits more items at the same detail. Small sources (e.g. a 320x240 GIF) fit more,
because cells never grow beyond the source resolution.

Explicit control:

- `--per-sheet N` — items per sheet (overrides `--detail`)
- `--cols N`, `--rows N` — fix the grid shape; a sheet holds at least one full row (or
  column), even when that makes cells smaller than `--detail` would allow
- `--max-sheets N` (default 10) — fail instead of writing more sheets

Items are spread evenly: 10 items at 8 per sheet become 5 + 5, not 8 + 2.

## Scale

`gridity probe` reports how much each `--detail` shrinks the input, and the manifest
gives a `scale` per cell:

- PDF: sheet pixels per point. `probe` measures the document's body text and prints its
  size on the sheet, e.g. `0.49 px/pt (body 22pt text = 11px)`, or the size of 12pt text
  when the PDF has no extractable text. Text of 8 px or more is readable, about 6 px can
  be skimmed, and smaller text needs a higher `--detail`, fewer `--pages` or `--crop`.
  `probe` suggests the lowest detail that keeps the body text at 8 px or more.
- Images, video, animations: displayed size / native size (`1.0` = full resolution).
  `probe` reports the smallest scale among the inputs.

## Crop

`--crop X,Y,W,H` shows only a region of every selected page, frame or image, given as
fractions of its displayed size (after page rotation, EXIF orientation and video
rotation): `0,0,1,0.5` is the top half, `0.5,0.5,0.5,0.5` the bottom-right quarter. Cells
take the region's shape, the header shows `crop X,Y,W,H`, and each cell in the manifest
has `"crop": [X, Y, W, H]`. PDF regions are rendered at the cell size; raster regions keep
their pixels and are not upscaled.

## Spacing and colors

- `--gap PX` (8), `--padding PX` (12), each 0-1000
- `--bg COLOR` (`#ffffff`); colors accept `#rgb`, `#rrggbb`, `#rrggbbaa`, `white`,
  `black`, `gray`, `red`, `transparent`

## Header

A header line names the source, the range on the sheet and the sheet number, e.g.
`deck.pdf · p.1–4 of 42 · sheet 1/11`. On narrow sheets a long file name (or title) is
shortened in the middle, so the range and sheet number stay visible. Replace it with
`--title TEXT` or drop it with `--no-header`.

## Labels

- `--label TEMPLATE`: e.g. `"p.{page}"`, `"{time}"`, `"{index}: {name}"`. Unknown
  variables are an error that lists the valid ones.
- `--no-label`
- `--label-pos below|overlay` — below the cell, or a badge in its top-left corner
- `--label-size PX` (15, 4-200), `--label-color COLOR`, `--label-bg COLOR`
- `--font PATH` — TTF/OTF/TTC for header and labels; repeat it to add fallbacks. Each
  character uses the first font that has it, then the embedded BIZ UDGothic (Japanese and
  Latin). Characters no font has are drawn as boxes and listed in one warning; pass a font
  that covers them, e.g. Noto Sans CJK SC/KR for Chinese or Korean, Noto Sans Arabic or
  Thai, or `/System/Library/Fonts/Supplemental/Arial Unicode.ttf` on macOS. Text is not
  shaped: Arabic letters are not joined and right-to-left text is drawn left to right.
  Color emoji cannot be drawn.
