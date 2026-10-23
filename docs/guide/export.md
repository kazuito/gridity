# Export

`gridity export` writes every selected page, frame or image (or its `--crop` region) to its
own file: no header, labels, frames or vision size limits. Use it to keep a figure, a slide
region or a video frame found while reading the sheets. The selection flags are the same as
in `render`, so a command that rendered the right region exports it by changing the
subcommand.

```sh
gridity export slides.pdf --pages 6 --crop 0.53,0.76,0.42,0.1 -o figs/lts.png
gridity export slides.pdf --pages 3-5 --dpi 300 -o figs/
gridity export talk.mp4 --at 13:45,38:25 --crop 0.07,0,0.8,1 -o frames/ --name "{time}_{index}"
gridity export shots/ --crop 0,0,1,0.5 --max-edge 800 -o tops/
```

## Size

- PDF pages are rendered at `--dpi` (150, 36-1200): a full A4 page at 150 dpi is
  1240x1754 px, and a crop is rendered sharp at the same resolution.
- Video frames, animation frames and images keep their native pixels (of the crop).
- `--max-edge PX` shrinks each file so its long edge is at most `PX`; it never enlarges.
- A file may have at most 16384 px per edge and 100 megapixels (`does_not_fit`).

## Selection

- PDF: `--pages` (default all), `--password-file`.
- Video and animations: one of `--at`, `--every` or `--count` is required; `--start` and
  `--end` narrow the range. An animation frame is written once even if several times fall
  on it.
- Images: every input file.
- `--crop X,Y,W,H` as in `render` (fractions of the whole page or frame).
- `--max-files N` (100, up to 10000) fails with `too_many_files` instead of writing more.

## File names

| `-o` | Files written |
|---|---|
| (none) | `$TMPDIR/gridity-<uid>/<name>-export-<hash>/<file name>.<ext>` |
| `dir/` (no image extension) | `dir/<file name>.<ext>` |
| `fig.png` / `fig.jpg` | `fig.png` for one item; `fig-01.png`, `fig-02.png`, ... for several |

`--name TEMPLATE` sets the file name without extension (not allowed when `-o` names a
file). Defaults: `{stem}-p{page}` (PDF), `{stem}-{time}` (video), `{stem}-f{frame}`
(animation), `{name}` (images).

| Variable | Value |
|---|---|
| `{stem}` | input file name without extension (`images` for several image files) |
| `{index}` | position in the selection, 1-based |
| `{total}` | number of selected items |
| `{page}` | PDF page number |
| `{time}` | video or animation time: `13m45s`, `1h02m03s` past an hour, `13m45.5s` when any selected time has tenths |
| `{frame}` | animation frame number, 1-based |
| `{name}` | image file name without extension |

Numbers are zero-padded to the largest value (`p07` in a 12-page PDF), so files sort in
order. Characters other than letters, digits, `-`, `_` and `.` become `_`. Two items
with the same file name (ignoring case) are an error; add `{index}` to `--name`. An output
path that is one of the inputs is an error too: exporting `shots/` into `shots/` needs a
`--name` such as `"{name}-top"`.

Formats: `--format png|jpeg` and `--quality` as in `render`; the extension of `-o` sets
the format too. Defaults are PNG for PDFs and images and JPEG for video and animations.

## Output

stdout lists one absolute path per written file. `--dry-run` prints the files that would
be written with their sizes, without writing anything. `--json` prints:

```json
{
  "schema_version": 1,
  "source": { "kind": "pdf", "path": "/abs/slides.pdf", "pages": 24 },
  "files": [
    {
      "index": 1, "path": "/abs/figs/lts.png", "page": 6,
      "width": 875, "height": 117, "scale": 2.083,
      "crop": [0.53, 0.76, 0.42, 0.1], "bytes": 20512
    }
  ],
  "warnings": []
}
```

Each file carries `page`, `time` (seconds), `frame` or `file` like the cells of a render
manifest. `width` and `height` are the written pixels, and `scale` is pixels per PDF point
or written / native pixels. An item that cannot be decoded is reported with `error` and no
`path`, and nothing is written for it. The default directory follows the same rules as for
sheets: it is private to the user and the same command overwrites the same files.
