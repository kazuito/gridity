# Recipes

The basic commands per input type are in `gridity guide`. These recipes explain the
choices behind them.

## Read a document with as few tokens as possible

```sh
gridity probe report.pdf --per-page    # content type and text size of every page
# read the listed text pages with a PDF text extractor (e.g. docling or MarkItDown)
gridity render report.pdf --pages 4-7,12 --detail low   # only pages that need to be seen
gridity render report.pdf --pages 9 --crop 0,0.5,1,0.5  # close-up of a small part
```

`probe --per-page` suggests these commands for the file: the `--pages` list holds the
`mixed` and `graphics` pages, the detail is the lowest one that keeps the body text at 8 px
or more, and pages whose small text stays below 6 px get a close-up. Slides with large text
usually fit at `--detail low`; papers with 10–11pt text need `medium` or more.

If a render fails with "more than --max-sheets", narrow `--pages`, use `--detail low`, or
pass the `--max-sheets` value suggested in the hint.

## Read one page or figure closely

```sh
gridity render paper.pdf --pages 7 --detail high
gridity render paper.pdf --pages 7 --per-sheet 1 --preset hires   # largest possible
```

## Zoom into part of a cell

Render with `--ruler`, read the values at the edges of the region, and pass them as
`--crop X,Y,W,H` (`W` = right − left, `H` = bottom − top):

```sh
gridity render paper.pdf --pages 7 --ruler                          # region is .1–.9 across, .55–.85 down
gridity render paper.pdf --pages 7 --crop 0.1,0.55,0.8,0.3 --ruler  # PDF: re-rendered sharp
gridity render talk.mp4 --at 12:30 --crop 0.5,0,0.5,0.5 --ruler     # top-right quarter
```

The ruler of a cropped cell still shows whole-page values, so each further zoom reuses the
numbers as they are. Reading positions in sheet pixels is less reliable: if you do, convert
them with the cell's `rect` from the manifest (`X = crop.x + (region_x - rect.x) /
rect.width * crop.width`, and likewise for Y, W, H).

## Read small text in a video or screenshot

Raster sources are never enlarged, so zooming helps only while the cells are smaller than
the source. Check before cropping further:

- `gridity probe talk.mp4` prints the native size; `% of native size` per `--detail` shows
  how much each level shrinks the frames.
- In `--json`, a cell's `scale` is shown / native pixels; `1.0` means every source pixel
  is already on the sheet.
- gridity warns "shown at native resolution" when a crop or a larger detail reached that
  point. Then neither a narrower `--crop` nor `export` can make the text larger. Read it from
  subtitles, a transcript or the original document instead.

A crop still saves tokens at native size: `--at 6:20,6:35 --crop 0,0.7,1,0.3` puts the
same strip from several moments on one small sheet.

## Find slide or scene changes in a long video

gridity samples at fixed times and never merges similar frames. To take one frame per
scene, let ffmpeg find the changes (it decodes the whole video) and pass the times to
`--at`:

```sh
times=$(ffmpeg -i talk.mp4 -vf "select='gt(scene,0.3)',showinfo" -f null - 2>&1 \
  | grep -o 'pts_time:[0-9.]*' | cut -d: -f2 | paste -sd, -)
gridity render talk.mp4 --at "0,$times" --max-sheets 4
```

Lower the threshold (`0.3`) to catch smaller changes. A slide that changes only a little
(a line of text, a number in a terminal) may not count as a scene change, so check the
sections in between with `--start/--end --every` when they matter.

## Locate a cell precisely

```sh
gridity render deck.pdf --json
```

Each cell in the manifest has `rect: [x, y, width, height]` in sheet pixels plus its page,
time, frame or file, so "the chart in the third cell" maps back to an exact source.

## Save sheets next to your work

```sh
gridity render deck.pdf -o review/          # review/deck.grid-01.png, ...
gridity render deck.pdf -o review/deck.png  # review/deck-01.png, ...
```
