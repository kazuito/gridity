# PDF input

```sh
gridity render file.pdf [--pages LIST] [--password-file PATH] [layout and style options]
```

## Page selection

`--pages` takes 1-based numbers and ranges separated by commas. Order is kept and
duplicates are dropped.

| Spec | Pages |
|---|---|
| `5` | 5 |
| `1-3,8` | 1, 2, 3, 8 |
| `10-` | 10 to the last page |
| `-4` | 1 to 4 |

A page beyond the end is an error (exit 2) that states the page count.

## Defaults

- Label: `p.{page}` below each page. Variables: `{page}`, `{index}`, `{total}`.
- 4 pages per sheet (`--detail medium`); `low` = 9, `high` = 2 for A4/Letter pages.
- PNG output (sharp text).
- Pages of different sizes or orientations share a grid; each page is fitted inside its cell.
- Page rotation (`/Rotate`) is applied.
- `--crop X,Y,W,H` renders only a region of each page, sharp at the cell size. The values
  are fractions of the page: `--pages 9 --crop 0,0.5,1,0.5` is the bottom half of page 9
  (see `gridity guide layout`).

## Text first

`gridity text file.pdf` prints the visible text of each page and marks the pages that have
figures, tables or formulas; `gridity probe file.pdf --per-page` turns that into suggested
commands. See `gridity guide text`.

## Encrypted PDFs

Password-protected files fail with exit 3 and a hint. Give the password one of these ways
(`--password` wins over `--password-file`, which wins over the environment):

| Option | Notes |
|---|---|
| `--password-file PATH` | the file content without its final line break; `-` reads stdin |
| `GRIDITY_PDF_PASSWORD` | environment variable, used when no option is given |
| `--password PW` | visible to other processes (`ps`) and kept in shell history |

Agents: do not write the password into the conversation or a command line. Ask the user
to put it in a file or to set `GRIDITY_PDF_PASSWORD` for you, then pass `--password-file`
or nothing. The same options work with `probe` and `text`.

## Limitations

Rendering uses hayro, a pure-Rust renderer. Most PDFs render faithfully. Rare features
(non-embedded CJK fonts with custom encodings, knockout groups) may render imperfectly;
gridity prints a warning such as `page 3: unsupported font, some text may be missing`
when the renderer reports a problem.

`--start`, `--end`, `--every`, `--count` and `--at` are rejected for PDFs.
