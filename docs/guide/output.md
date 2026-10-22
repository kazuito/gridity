# Output

## Files

| `-o` | Files written |
|---|---|
| (none) | `$TMPDIR/gridity-<uid>/<name>-<hash>/<name>.grid.png` or `<name>.grid-01.png`, ... |
| `dir/` (no image extension) | `dir/<name>.grid.png` or `dir/<name>.grid-01.png`, ... |
| `out.png` / `out.jpg` | `out.png` or `out-01.png`, ...; the extension sets the format |

The default directory depends only on the input path and the options, so re-running a
command overwrites its previous sheets and removes ones no longer produced. It belongs to
the current user (`<uid>` is the numeric user id, mode 0700; `gridity` on Windows), and
gridity refuses to use it if another user owns it or can write to it. Sheets are written
to a temporary file and renamed, so a sheet path never holds a half-written image.

Formats: `--format png|jpeg` (`--quality 1-100`, default 85). Defaults are PNG for PDFs
and images, JPEG for video and animations. Image size limits are checked on the base64
encoding, as the APIs count it: Claude's API accepts 10 MB, Amazon Bedrock and Google Cloud
5 MB. A PNG sheet above 5 MB as base64 (about 3.75 MB on disk) is re-encoded as JPEG with
a warning when the format was not chosen; with `--format png` or `-o out.png` the PNG is
kept and only the warning is printed.

## stdout

Without `--json`: one absolute sheet path per line. With `--dry-run`: a layout summary
and no files.

## JSON manifest (`--json`)

```json
{
  "source": { "kind": "pdf", "path": "/abs/deck.pdf", "pages": 42 },
  "limits": { "preset": "compat", "max_edge": 1568, "max_tokens": 1568 },
  "sheets": [
    {
      "path": "/abs/tmp/gridity-501/deck-1a2b3c4d5e6f7a8b/deck.grid-01.png",
      "width": 896, "height": 1319, "est_tokens": 1536, "bytes": 68967,
      "grid": { "cols": 2, "rows": 2, "cell_width": 432, "cell_height": 611 },
      "cells": [
        { "index": 1, "label": "p.1", "page": 1, "rect": [12, 35, 432, 611], "scale": 0.726 }
      ]
    }
  ],
  "warnings": []
}
```

- `source` by kind: `pdf` (`pages`, `title`), `video` (`duration`, `width`, `height`,
  `rotation`, `fps`, `codec`), `anim` (`frames`, `duration`), `images` (`count`).
- Cells carry `page` (PDF), `time` in seconds (video, animation), `frame` (animation, 1-based)
  or `file` (images). `index` is 1-based across the whole render.
- `rect` is `[x, y, width, height]` of the placed image in sheet pixels.
- `scale` is sheet pixels per point for PDF pages, and displayed / native pixels for
  images and frames.
- `crop` (only with `--crop`) is the shown region as `[x, y, width, height]` fractions.
- `est_tokens` uses Claude's 28x28 px patch rule; other models count differently.
- `--dry-run --json` prints the same structure without `path` and `bytes`.

## Probe JSON (`gridity probe --json`)

- `source`: as above, plus `page_sizes` (PDF) or `files` (images).
- `estimates`: one entry per `--detail` with `per_sheet`, `sheets` (when the item count is
  known), `scale` (smallest among the inputs), `text_12pt_px` (PDF) and `body_text_px`
  (PDF with extractable text). With `--per-page`, `visual_scale` is the scale when only the
  `mixed` and `graphics` pages are rendered, as the suggested command does. The default
  detail has `"default": true`.
- `body_text_pt` (PDF): typical body text size of the document.
- `pages` (`--per-page`): per page `content`, `chars`, `unmapped`, `body_pt`, `small_pt`,
  `scripts`, `short_lines`, `images`, `image_cover`, `vector_cover` (see
  `gridity guide text`).
- `suggestions`: ready-to-run commands.

## Errors

Errors go to stderr as `error: ...` and an optional `hint: ...`. With `--json`, stdout also
gets:

```json
{ "error": { "kind": "usage", "code": 2, "message": "...", "hint": "..." } }
```

| Exit | Kind | Meaning |
|---|---|---|
| 0 | | success |
| 1 | runtime | unexpected failure (I/O, encoding) |
| 2 | usage | invalid option, selection out of range, too many sheets |
| 3 | input | missing, unsupported, encrypted or damaged input |
| 4 | dependency | ffmpeg / ffprobe not found |
