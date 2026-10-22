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

Options: `--root DIR` (repeatable) limits inputs and password files to those directories;
`--jobs N` sets the number of workers. Video needs `ffmpeg` on the server's `PATH` (or
`GRIDITY_FFMPEG` in its environment).

## Tools

| Tool | Returns |
|---|---|
| `probe` | sheet estimates and suggested render commands (`gridity guide output`) |
| `text` | visible text of PDF pages and their content type (`gridity guide text`) |
| `render` | the sheets as images, plus the manifest (cell rects, labels, pages or times) |
| `guide` | this documentation; `topic` is a guide topic or `all` |

Parameters are the CLI options with underscores: `per_sheet` for `--per-sheet`, `type` for
`--type`, booleans for switches (`no_header: true`). Inputs are `inputs` (a list) for
`render` and `probe`, and `input` for `text`. `render` returns `images: false` results
without image blocks when only the manifest is needed. Error messages and hints name the
CLI flags; translate them the same way.

The JSON document (manifest or report) is both the first text block of the result and its
`structuredContent`. Failures are results with `isError: true` and a text block holding
`error`, `reason` (see `gridity guide output`) and `hint`.

## Limits

- `render` writes at most 4 sheets per call unless `max_sheets` is given, and refuses to
  return more than 20 MB of images (base64) in one result: narrow `pages` or the time range.
- Sheets are also written to the default private output directory, as with the CLI.
- Passwords are not tool parameters, so they stay out of the conversation. Start the
  server with `GRIDITY_PDF_PASSWORD` set, or pass `password_file` (a path, not `-`).
- A PDF that takes very long to render blocks the server until it finishes; gridity has no
  time limit for PDF rendering yet.
