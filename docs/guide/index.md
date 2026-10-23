# gridity guide

gridity turns a PDF, a video, an animated GIF/APNG or a set of images into one or more
**grid sheets**: labeled thumbnails arranged on a single image that fits an LLM's vision
limits. Read one sheet instead of taking many screenshots.

## Commands

| Command | Purpose |
|---|---|
| `gridity probe <INPUT>` | Page count / duration / resolution and how many sheets a render needs |
| `gridity text <PDF>` | Visible text of each page and whether the page needs to be seen |
| `gridity render <INPUT>... [OPTIONS]` | Write sheets; prints their absolute paths |
| `gridity export <INPUT>... [OPTIONS]` | Write each page, frame or image (or a region) to its own file |
| `gridity guide [TOPIC]` | This documentation |
| `gridity mcp` | Serve these commands as MCP tools (`gridity guide mcp`) |
| `gridity version` | Print the version (also `-v`, `--version`) |

## Typical workflow

1. `gridity probe report.pdf` — learn the size of the input, the sheet estimates and how
   large the text will be. For PDFs add `--per-page` to see which pages need images.
2. PDFs: `gridity text report.pdf` — read text pages as text; it costs far fewer tokens.
3. `gridity render report.pdf --pages ...` — render the pages or frames that need to be
   seen, and read the printed paths with your image viewer tool.
4. Zoom in where needed: `--pages 7 --detail high`, or `--crop X,Y,W,H` (fractions of
   the page) for part of it, e.g. `--pages 7 --crop 0,0.5,1,0.5` for the bottom half.
   Add `--ruler` to read the region's coordinates off the sheet instead of estimating them.
5. To keep a figure or frame as a file, run the same selection with `gridity export`.

`probe`'s content types, text sizes and suggested commands are heuristics. Use them as a
starting point, look at the sheets, and adjust `--detail`, `--pages` or `--crop` when text
is too small or a page was misjudged. Text extracted from formulas can be wrong in ways
that change their meaning; read formulas from the page images.

## Output contract

- stdout: `render` prints one absolute path per sheet and `export` one per file (or a JSON
  manifest with `--json`);
  `probe` and `text` print their report (JSON with `--json`).
- stderr: warnings and errors (`error: ...` / `hint: ...`).
- Sheets go to `$TMPDIR/gridity-<uid>/<name>-<hash>/` unless `-o` is given; re-running the
  same command overwrites the same files.
- Exit codes: 0 ok, 1 runtime error, 2 bad arguments or selection, 3 unsupported or
  broken input, 4 missing dependency (ffmpeg).

## Topics

- `gridity guide recipes` — task-oriented examples
- `gridity guide pdf` — page selection, passwords (`--password-file`, `GRIDITY_PDF_PASSWORD`)
- `gridity guide text` — text extraction and page analysis for PDFs
- `gridity guide video` — time ranges, sampling, GIF/APNG
- `gridity guide images` — files and directories
- `gridity guide export` — one file per page, frame or region, without sheet layout
- `gridity guide layout` — presets, detail, grid and style options
- `gridity guide output` — file locations, JSON manifest schema, exit codes
- `gridity guide mcp` — `gridity mcp`, the MCP server for clients without a shell
- `gridity guide all` — everything at once
