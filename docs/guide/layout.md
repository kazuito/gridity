# Layout and style

## Size limits (`--preset`)

Every sheet is sized so Claude does not downscale it, which keeps cell coordinates exact.

| Preset | Max edge | Max visual tokens | Use for |
|---|---|---|---|
| `compat` (default) | 1568 px | 1568 | Every Claude model; safe for other LLMs |
| `hires` | 2576 px | 4784 | Claude 4.7 and later models |

Tokens are counted as `ceil(w/28) * ceil(h/28)`. Override with `--max-edge` and
`--max-tokens` for other models.

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
- `--cols N`, `--rows N` — fix the grid shape
- `--max-sheets N` (default 10) — fail instead of writing more sheets

Items are spread evenly: 10 items at 8 per sheet become 5 + 5, not 8 + 2.

## Scale

Each cell in the manifest has a `scale`: sheet pixels per point for PDF pages, and
displayed / native size for images, video and animations (`1.0` = full resolution).

## Crop

`--crop X,Y,W,H` shows only a region of every selected page, frame or image, given as
fractions of its displayed size (after page rotation, EXIF orientation and video
rotation): `0,0,1,0.5` is the top half, `0.5,0.5,0.5,0.5` the bottom-right quarter. Cells
take the region's shape, the header shows `crop X,Y,W,H`, and each cell in the manifest
has `"crop": [X, Y, W, H]`. PDF regions are rendered at the cell size; raster regions keep
their pixels and are not upscaled.

## Spacing and colors

- `--gap PX` (8), `--padding PX` (12)
- `--bg COLOR` (`#ffffff`); colors accept `#rgb`, `#rrggbb`, `#rrggbbaa`, `white`,
  `black`, `gray`, `red`, `transparent`

## Header

A header line names the source, the range on the sheet and the sheet number, e.g.
`deck.pdf · p.1–4 of 42 · sheet 1/11`. Replace it with `--title TEXT` or drop it with
`--no-header`.

## Labels

- `--label TEMPLATE`: e.g. `"p.{page}"`, `"{time}"`, `"{index}: {name}"`. Unknown
  variables are an error that lists the valid ones.
- `--no-label`
- `--label-pos below|overlay` — below the cell, or a badge in its top-left corner
- `--label-size PX` (15), `--label-color COLOR`, `--label-bg COLOR`
- `--font PATH` — TTF/OTF for header and labels (default: embedded BIZ UDGothic, which
  covers Japanese)
