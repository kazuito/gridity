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

Find the region in the sheet, then convert it to fractions of the cell's `rect` from the
manifest:

```
X = (region_x - rect.x) / rect.width     W = region_width / rect.width
Y = (region_y - rect.y) / rect.height    H = region_height / rect.height
```

```sh
gridity render paper.pdf --pages 7 --crop 0.1,0.55,0.8,0.3        # PDF: re-rendered sharp
gridity render talk.mp4 --at 12:30 --crop 0.5,0,0.5,0.5            # top-right quarter
```

PDF crops are rendered at the cell size, so a small region gets many pixels. Raster
inputs are never upscaled: a crop of a small image stays small.

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
