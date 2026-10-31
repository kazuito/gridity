# gridity guide

gridity turns a PDF, a video, an animated GIF/APNG or a set of images into **grid sheets**:
labeled thumbnails on one image sized for an LLM's vision limits, so the model sees every
pixel without downscaling. Read one sheet instead of taking many screenshots. This page is
enough to start; the topics at the end go deeper.

## Commands

| Command | Purpose |
|---|---|
| `gridity probe <INPUT>` | Size of the input, items per sheet at each `--detail`, suggested commands |
| `gridity render <INPUT>... [OPTIONS]` | Write sheets; prints their absolute paths |
| `gridity export <INPUT>... [OPTIONS]` | Write each page, frame or image (or a region) to its own file |
| `gridity guide [TOPIC]` | This documentation |
| `gridity mcp` | Serve these commands as MCP tools (`gridity guide mcp`) |
| `gridity version` | Print the version (also `-v`, `--version`) |

## Workflow

Start with `probe`; its `suggested commands` are a good first `render`. Read the printed
sheet paths with your image viewer tool, then narrow the selection or zoom in.

PDF:

```sh
gridity probe report.pdf --per-page                # content type and text size of each page
# read the listed "text pages" with a text extractor (docling, MarkItDown): far fewer tokens
gridity render report.pdf --pages 4-7,12           # 4 pages per sheet; --detail low 9, high 2
gridity render report.pdf --pages 7 --detail high  # one page, larger
```

Video, GIF, APNG (video needs `ffmpeg`):

```sh
gridity render talk.mp4                                       # one sheet, evenly spaced frames
gridity render talk.mp4 --every 30s --max-sheets 4            # overview of a long video
gridity render talk.mp4 --start 12:00 --end 14:00 --every 5s  # one section in detail
gridity render talk.mp4 --at 1:05,7:30 --detail high          # exact moments, larger
```

Each frame is labeled with its time (`7:30`), so cells line up with subtitles or a transcript.

Images:

```sh
gridity render shots/                          # every image in a directory, natural order
gridity render before.png after.png --cols 2   # side by side
```

Zoom into a region (`--crop X,Y,W,H` in fractions of the page or frame):

```sh
gridity render report.pdf --pages 7 --ruler                   # read coordinates off the ruler
gridity render report.pdf --pages 7 --crop 0.1,0.55,0.8,0.3   # x .1-.9, y .55-.85
gridity render talk.mp4 --at 6:20,7:50 --crop 0.5,0,0.5,0.6   # same region at several times
```

Keep files (no sheet layout; same selection flags as `render`):

```sh
gridity export report.pdf --pages 7 --crop 0.1,0.55,0.8,0.3 --dpi 300 -o figs/chart.png
gridity export talk.mp4 --at 1:15,7:30 -o frames/ --name "{time}"   # frames/01m15s.jpg, ...
```

## Know before you start

- **Video frames, GIFs and images are never enlarged.** A cell shows at most the native
  pixels, and `--crop` keeps only the region's own pixels: the top-right quarter of a
  852x480 video is 426x240 px, whatever `--detail`, `--preset` or `export` you use. A
  `scale` of 1.0 in `--json` (100% in `probe`) means every source pixel is shown, and
  gridity warns "shown at native resolution" when `--crop`, `--detail high`, `--preset
  hires` or `--per-sheet` hit that limit. If text is still unreadable, the source is the
  limit: read it elsewhere (subtitles, the original slides). PDFs are vector: a PDF crop is
  rendered sharp at the cell size.
- **`--crop` is relative to the whole page or frame**, also when zooming further into a
  cropped cell. `--ruler` labels lines in these coordinates, so read values pass on as is.
- **Sampling sees only the sampled times.** A change between two `--every` samples is
  missed; narrow `--start`/`--end` and shorten `--every` around it. Similar frames are
  never merged.
- **Sheets have a fixed maximum size** (`--preset compat`, 1568 px, the default; `hires`,
  2576 px, for Claude 4.7 and later). More items per sheet means smaller cells; `--detail`
  trades one for the other.
- **`--max-sheets` (default 10)** caps a render; a larger selection fails before anything
  is written, with a hint. `--dry-run` prints the plan without writing.
- **Output goes to a private temp directory** (`$TMPDIR/gridity-<uid>/<name>-<hash>/`) that
  the same command overwrites. Pass `-o dir/` to keep files; `export --name` sets their
  names from `{stem}`, `{index}`, `{page}`, `{time}`, `{frame}` or `{name}`.
- Flags for another input type (`--pages` on a video, `--at` on a PDF) are errors.
- `probe`'s content types, text sizes and suggestions are heuristics: look at the sheets
  and adjust. Read formulas from page images; extracted formula text can be wrong.

## Output contract

- stdout: `render` prints one absolute path per sheet, `export` one per file, or a JSON
  manifest with `--json` (cell rects, times, pages, `scale`, warnings). `probe` prints its
  report (JSON with `--json`).
- stderr: `warning: ...`, `error: ...`, `hint: ...`.
- Exit codes: 0 ok, 1 runtime error, 2 bad arguments or selection, 3 unsupported or
  broken input, 4 missing dependency (ffmpeg).

## Topics

- `gridity guide recipes` — task-oriented examples with explanations
- `gridity guide pdf` — page selection, passwords (`--password-file`, `GRIDITY_PDF_PASSWORD`)
- `gridity guide pages` — which PDF pages to render and which to read as text
- `gridity guide video` — time syntax, sampling, GIF/APNG
- `gridity guide images` — files and directories
- `gridity guide export` — file names, sizes and formats of exported files
- `gridity guide layout` — presets, detail, crop, ruler, grid and style options
- `gridity guide output` — file locations, JSON manifest schema, exit codes
- `gridity guide mcp` — `gridity mcp`, the MCP server for clients without a shell
- `gridity guide all` — everything at once
