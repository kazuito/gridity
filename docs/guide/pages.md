# Page analysis (PDF)

Images of text pages cost far more tokens than the text itself. `gridity probe --per-page`
tells which PDF pages need to be seen and which read just as well as text, so you can
render only the pages that need it.

gridity does not extract text. Read the `text` pages with a PDF text extractor (e.g.
docling or MarkItDown) and render the rest.

## `gridity probe --per-page`

```sh
gridity probe file.pdf --per-page [--password-file PATH] [--json]
```

Adds one line per page to the probe report, plus a list of the `text` pages:

```
pages:
  p.1    text · 1840 chars · body 11pt
  p.4    mixed · 120 chars · body 22pt · small 20pt · formulas · drawings 9%
text pages (a text extractor reads them for fewer tokens): 1-3,5
```

Only visible text counts: text outside the page or hidden by a clipping path (e.g. the
cropped-away part of an embedded page) is skipped.

The suggested commands turn the analysis into a plan: `gridity render --pages ...` for the
`mixed` and `graphics` pages at the lowest detail that keeps the body text at 8 px or more
on those sheets, and a close-up for pages whose small text (captions, subscripts) would
fall below 6 px. Commands that need more than 10 sheets include `--max-sheets`
(`suggested_calls`, for the MCP server: more than 4).

The content types, sizes and suggestions are heuristics. Treat them as a starting point:
look at the sheets and zoom in (`--detail high`, `--per-sheet 1`, `--crop`) where text is
still too small.

Without `--per-page`, `probe` measures up to 8 pages spread over the document to report
the body text size in pixels for each `--detail` (see `gridity guide layout`, "Scale").

## Content types

| Type | Meaning | What to do |
|---|---|---|
| `text` | Running text, no notable figures | Read it with a text extractor |
| `mixed` | Text plus images, drawings (charts, tables, diagrams) or formulas | Render the page for the visual parts |
| `graphics` | Fewer than 30 characters, or text that cannot be extracted (unmapped glyphs) | Render the page |
| `empty` | Nothing visible | Skip |

The description line also lists the signals behind the type: `body` (median text size)
and `small` (size that 90% of the text reaches), `N image(s) X%` and `drawings X%` (share
of the page covered), `formulas` (sub/superscripts or many one- or two-character lines),
and `unmapped X%` (glyphs without a Unicode mapping).

Text extracted from formulas can be wrong in ways that change their meaning: on pages
marked `formulas`, read the formulas from the rendered page.

## JSON

`gridity probe --per-page --json` adds:

```json
{
  "pages": [
    { "page": 4, "content": "mixed", "chars": 120, "unmapped": 0.05,
      "body_pt": 22.0, "small_pt": 20.0, "scripts": 2, "short_lines": 0.44,
      "images": 0, "image_cover": 0.0, "vector_cover": 0.09 }
  ],
  "text_pages": "1-3,5"
}
```

`text_pages` is absent when no page is of type `text`. `body_text_pt` and, per estimate,
`body_text_px` and `visual_scale` are described in `gridity guide output`.
