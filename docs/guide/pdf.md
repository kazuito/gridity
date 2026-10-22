# PDF input

```sh
gridity render file.pdf [--pages LIST] [--password PW] [layout and style options]
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
- `--crop X,Y,W,H` renders only a region of each page, sharp at the cell size (see
  `gridity guide layout`).

## Encrypted PDFs

Password-protected files fail with exit 3 and the hint `pass --password <PASSWORD>`.

## Limitations

Rendering uses hayro, a pure-Rust renderer. Most PDFs render faithfully. Rare features
(non-embedded CJK fonts with custom encodings, knockout groups) may render imperfectly;
gridity prints a warning such as `page 3: unsupported font, some text may be missing`
when the renderer reports a problem.

`--start`, `--end`, `--every`, `--count` and `--at` are rejected for PDFs.
