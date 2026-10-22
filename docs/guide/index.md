# gridity guide

gridity turns a PDF, a video, an animated GIF/APNG or a set of images into one or more
**grid sheets**: labeled thumbnails arranged on a single image that fits an LLM's vision
limits. Read one sheet instead of taking many screenshots.

## Commands

| Command | Purpose |
|---|---|
| `gridity probe <INPUT>` | Page count / duration / resolution and how many sheets a render needs |
| `gridity render <INPUT>... [OPTIONS]` | Write sheets; prints their absolute paths |
| `gridity guide [TOPIC]` | This documentation |

## Typical workflow

1. `gridity probe report.pdf` — learn the size of the input and the sheet estimates.
2. `gridity render report.pdf --dry-run` — optional: check the layout and token cost.
3. `gridity render report.pdf` — read the printed paths with your image viewer tool.
4. Zoom in where needed: `gridity render report.pdf --pages 7 --detail high`.

## Output contract

- stdout: one absolute path per sheet (or a JSON manifest with `--json`).
- stderr: warnings and errors (`error: ...` / `hint: ...`).
- Sheets go to `$TMPDIR/gridity/<name>-<hash>/` unless `-o` is given; re-running the
  same command overwrites the same files.
- Exit codes: 0 ok, 1 runtime error, 2 bad arguments or selection, 3 unsupported or
  broken input, 4 missing dependency (ffmpeg).

## Topics

- `gridity guide recipes` — task-oriented examples
- `gridity guide pdf` — page selection, passwords
- `gridity guide video` — time ranges, sampling, GIF/APNG
- `gridity guide images` — files and directories
- `gridity guide layout` — presets, detail, grid and style options
- `gridity guide output` — file locations, JSON manifest schema, exit codes
- `gridity guide all` — everything at once
