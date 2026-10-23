# Recipes

## Read a document with as few tokens as possible

```sh
gridity probe report.pdf --per-page    # content type and text size of every page
gridity text report.pdf                # read the text pages as text
gridity render report.pdf --pages 4-7,12 --detail low   # only pages that need to be seen
gridity render report.pdf --pages 9 --crop 0,0.5,1,0.5  # close-up of a small part
```

`probe --per-page` suggests these commands for the file: the `--pages` list holds the
`mixed` and `graphics` pages, the detail is the lowest one that keeps the body text at 8 px
or more, and pages whose small text stays below 6 px get a close-up. Slides with large text
usually fit at `--detail low`; papers with 10–11pt text need `medium` or more. The
suggestions are a starting point: check the sheets and zoom in where needed.

## Skim a whole PDF

```sh
gridity probe deck.pdf                 # shows sheets needed per --detail level
gridity render deck.pdf                # 4 pages per sheet by default
gridity render deck.pdf --detail low   # 9 pages per sheet, for a quick overview
```

If the render fails with "more than --max-sheets", either narrow `--pages`, use
`--detail low`, or pass the `--max-sheets` value suggested in the hint.

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

PDF crops are rendered at the cell size, so a small region gets many pixels. Raster
inputs are never upscaled: a crop of a small image stays small.

## Save a figure or frame

```sh
gridity export paper.pdf --pages 7 --crop 0.1,0.55,0.8,0.3 -o figs/chart.png   # 150 dpi
gridity export paper.pdf --pages 7 --crop 0.1,0.55,0.8,0.3 --dpi 300 -o figs/
gridity export talk.mp4 --at 12:30,14:05 -o frames/                            # native size
```

`export` takes the same selection as `render` and writes each item to its own file with
no sheet layout (`gridity guide export`).

## Understand a video

```sh
gridity render talk.mp4                                   # one sheet, evenly spaced frames
gridity render talk.mp4 --every 30s --max-sheets 4        # regular sampling
gridity render talk.mp4 --start 12:00 --end 14:00 --every 5s
gridity render talk.mp4 --at 1:05,7:30,12:44 --detail high
```

Start broad (default sampling), then narrow with `--start/--end` around the moments that
matter.

## Compare screenshots or photos

```sh
gridity render before.png after.png --cols 2
gridity render screenshots/ --label "{index}: {name}"
```

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
