# Output

## Files

| `-o` | Files written |
|---|---|
| (none) | `$TMPDIR/gridity/<name>-<hash>/<name>.grid.png` or `<name>.grid-01.png`, ... |
| `dir/` (no image extension) | `dir/<name>.grid.png` or `dir/<name>.grid-01.png`, ... |
| `out.png` / `out.jpg` | `out.png` or `out-01.png`, ...; the extension sets the format |

The default directory depends only on the input path and the options, so re-running a
command overwrites its previous sheets and removes ones no longer produced.

Formats: `--format png|jpeg` (`--quality 1-100`, default 85). Defaults are PNG for PDFs
and images, JPEG for video and animations. A PNG sheet above 5 MB is re-encoded as JPEG
with a warning, because common APIs reject larger images.

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
      "path": "/abs/tmp/gridity/deck-1a2b3c4d/deck.grid-01.png",
      "width": 896, "height": 1319, "est_tokens": 1536, "bytes": 68967,
      "grid": { "cols": 2, "rows": 2, "cell_width": 432, "cell_height": 611 },
      "cells": [
        { "index": 1, "label": "p.1", "page": 1, "rect": [12, 35, 432, 611] }
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
- `est_tokens` uses Claude's 28x28 px patch rule; other models count differently.
- `--dry-run --json` prints the same structure without `path` and `bytes`.

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
