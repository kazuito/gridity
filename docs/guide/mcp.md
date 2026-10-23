# MCP server

`gridity mcp` serves gridity as Model Context Protocol tools over stdin/stdout, so clients
without a shell or an image viewer tool can use it. Sheets come back as images in the tool
result; nothing has to be read from disk.

## Setup

```sh
claude mcp add gridity -- gridity mcp                 # Claude Code
claude mcp add gridity -- gridity mcp --root ~/docs   # only read files under ~/docs
```

Claude Desktop (`claude_desktop_config.json`):

```json
{ "mcpServers": { "gridity": { "command": "gridity", "args": ["mcp"] } } }
```

Options: `--root DIR` (repeatable) limits inputs, the files of an input directory (links
included) and password files to those directories (video playlists, which could name other
files, are never read);
`--jobs N` sets the number of workers. Video needs `ffmpeg` on the server's `PATH` (or
`GRIDITY_FFMPEG` in its environment).

## Tools

| Tool | Returns |
|---|---|
| `probe` | sheet estimates and suggested calls (`gridity guide output`) |
| `render` | the sheets as images, plus the manifest (cell rects, labels, pages or times) |
| `guide` | this documentation; `topic` is a guide topic or `all` |

Parameters are the CLI options with underscores: `per_sheet` for `--per-sheet`, `type` for
`--type`, booleans for switches (`no_header: true`, `ruler: true`). `export` is CLI only. Inputs are `inputs` (a list). `render` returns `images: false` results
without image blocks when only the manifest is needed. Error messages and hints name the
tool parameters (`max_sheets 6`, not `--max-sheets 6`); a flag marked `(CLI only)` has no
tool parameter.

`probe` returns its suggestions twice: `suggestions` as CLI commands, and
`suggested_calls` as `{ "tool": ..., "arguments": {...} }` for this server. Make a
suggested call with its `arguments` as they are (add `images: false` to a `render` if you
only need the manifest); they already carry `max_sheets` when a render needs more than 4
sheets.

The JSON document (manifest or report) is both the first text block of the result and its
`structuredContent`. Failures are results with `isError: true` and a text block holding
`error`, `reason` (see `gridity guide output`) and `hint`.

## Limits

- `render` writes at most 4 sheets per call unless `max_sheets` is given. One result
  carries at most 20 MB of images (base64): when the sheets are larger together, the first
  sheets that fit are attached and the summary line names the ones left out. Their paths are
  in the manifest (`sheets[i].path`); read those files, or render the rest in another call
  with `pages` or the time range narrowed.
- `probe` declares `anthropic/maxResultSizeChars` (500000), so Claude Code keeps its
  results inline (`per_page` on long PDFs) instead of saving results over 50000 characters
  to a file.
- Sheets are also written to the default private output directory, as with the CLI.
- Passwords are not tool parameters, so they stay out of the conversation. Start the
  server with `GRIDITY_PDF_PASSWORD` set, or pass `password_file` (a path, not `-`).
- A PDF that takes very long to render blocks the server until it finishes; gridity has no
  time limit for PDF rendering yet.
