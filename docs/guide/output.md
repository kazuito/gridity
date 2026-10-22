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
  "schema_version": 1,
  "source": { "kind": "pdf", "path": "/abs/deck.pdf", "pages": 42 },
  "limits": { "preset": "compat", "rule": "claude-28px-patch", "max_edge": 1568, "max_tokens": 1568 },
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
- `error` (only when an item could not be decoded) explains why the cell is a gray
  placeholder; the cell keeps its planned `rect`.
- `limits.rule` names how sizes and tokens are counted; `claude-28px-patch` is the only
  rule so far. `est_tokens` follows it; other models count differently.
- `--dry-run --json` prints the same structure without `path` and `bytes`.

## Versions and schemas

Every JSON document (`render`, `probe`, `text` and errors) starts with `"schema_version": 1`.
New fields may appear without a version change; removing, renaming or retyping a field
raises it. JSON Schemas for all four documents are in the repository under
`docs/schema/` (`render.schema.json`, `probe.schema.json`, `text.schema.json`,
`error.schema.json`).

## Text JSON (`gridity text --json`)

- `source`: as above; `pages`: per page `page`, the analysis fields of `gridity guide text`
  and `text`.
- `next_pages` (MCP `text` tool only, when pages were left out to fit `max_chars`): the
  remaining selected pages as a page list, e.g. `"15-50"`; pass it as `pages` to read on.

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
- `warnings`: inputs that were skipped (unreadable images, damaged animations).

## Errors

Errors go to stderr as `error: ...` and an optional `hint: ...`. With `--json`, stdout also
gets:

```json
{
  "schema_version": 1,
  "error": {
    "kind": "usage", "code": 2, "reason": "page_out_of_range",
    "message": "page 99 is out of range: the document has 12 pages",
    "hint": "use --pages within 1-12"
  }
}
```

`code` is the exit code. Branch on `reason`, not on `message` (whose wording may change);
`hint` is `null` when there is none.

| Exit | Kind | Meaning |
|---|---|---|
| 0 | | success |
| 1 | runtime | unexpected failure (I/O, encoding) |
| 2 | usage | invalid option, selection out of range, too many sheets |
| 3 | input | missing, unsupported, encrypted or damaged input |
| 4 | dependency | ffmpeg / ffprobe not found |

| Reason | Kind | Meaning |
|---|---|---|
| `invalid_argument` | usage | unknown flag, missing value, conflicting flags, mixed inputs |
| `invalid_value` | usage | a value that cannot be parsed or is out of range (`--pages`, times, `--crop`, font, password file, MCP `max_chars`) |
| `page_out_of_range` | usage | `--pages` beyond the last page |
| `flag_not_for_input` | usage | a flag for another input type (`--pages` on a video) |
| `label_variable_not_for_input` | usage | a `--label` variable the input type does not have |
| `too_many_sheets` | usage | the selection needs more than `--max-sheets` sheets |
| `too_many_samples` | usage | an animation selection with too many timestamps |
| `does_not_fit` | usage | `--per-sheet` / `--cols` / `--rows` cannot fit within the size limits |
| `outside_media` | usage | `--start`, `--end` or `--at` outside the media |
| `unknown_topic` | usage | `gridity guide` topic that does not exist |
| `outside_root` | usage | MCP server: a path outside the directories given with `--root` |
| `too_large` | usage | no longer returned (the MCP server now attaches the sheets that fit); the name stays reserved |
| `not_found` | input | the input path does not exist |
| `unknown_type` | input | the input type cannot be detected (pass `--type`) |
| `wrong_type` | input | the input is not of the required type (`text` needs a PDF; video needs a container, not a playlist) |
| `unreadable` | input | a file cannot be read (image header, password file) |
| `decode_failed` | input | the image or video cannot be decoded |
| `too_large_to_decode` | input | decoding would need more than 512 MiB |
| `password_required` | input | the PDF is encrypted and no password was given |
| `password_incorrect` | input | the PDF password is wrong |
| `decrypt_failed` | input | the PDF uses unsupported encryption |
| `invalid_pdf` | input | the PDF is damaged or not a PDF |
| `no_pages` | input | the PDF has no pages |
| `no_images` | input | no image files in the given directory |
| `no_frames` | input | the animation has no frames |
| `no_video_stream` | input | the file has no usable video stream |
| `unknown_duration` | input | the video duration is unknown (pass `--end` or `--at`) |
| `timeout` | input | ffmpeg / ffprobe did not finish in time |
| `tool_missing` | dependency | ffmpeg / ffprobe not found |
| `unsafe_output_dir` | runtime | the default output directory is not private to the user |
| `internal` | runtime | a bug in gridity (please report it) |
| `other` | runtime | other runtime failures (I/O, encoding) |
