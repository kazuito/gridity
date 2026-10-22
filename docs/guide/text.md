# Text and page analysis (PDF)

Reading extracted text costs far fewer tokens than reading page images. Use it for pages
that are plain text, and render only the pages that need to be seen.

## `gridity text`

```sh
gridity text file.pdf [--pages LIST] [--password PW] [--json]
```

Prints the text that is visible on each page, one block per page:

```
--- p.4 · mixed · 120 chars · body 22pt · small 20pt · formulas · drawings 9% ---
逆行列（春学期の復習）
...
```

Only visible text is printed: text outside the page or hidden by a clipping path (e.g. the
cropped-away part of an embedded page) is skipped. Lines follow the drawing order of the
PDF, so multi-column layouts and formulas may come out in a different order than they
appear.

## Content types

| Type | Meaning | What to do |
|---|---|---|
| `text` | Running text, no notable figures | Read the text |
| `mixed` | Text plus images, drawings (charts, tables, diagrams) or formulas | Read the text, render the page for the visual parts |
| `graphics` | Fewer than 30 characters, or text that cannot be extracted (unmapped glyphs) | Render the page |
| `empty` | Nothing visible | Skip |

The description line also lists the signals behind the type: `body` (median text size)
and `small` (size that 90% of the text reaches), `N image(s) X%` and `drawings X%` (share
of the page covered), `formulas` (sub/superscripts or many one- or two-character lines),
and `unmapped X%` (glyphs without a Unicode mapping).

## `gridity probe --per-page`

Adds the same analysis for every page to `probe`, and turns the suggested commands into a
plan: `gridity text` for the whole file (when it has extractable text),
`gridity render --pages ...` for the `mixed` and `graphics` pages at the lowest detail that
keeps the body text at 8 px or more on those sheets, and a close-up for pages whose small
text (captions, subscripts) would fall below 6 px.

Without `--per-page`, `probe` measures up to 8 pages spread over the document to report
the body text size in pixels for each `--detail` (see `gridity guide layout`, "Scale").

## JSON

`gridity text --json`:

```json
{
  "source": { "kind": "pdf", "path": "/abs/file.pdf", "pages": 13 },
  "pages": [
    { "page": 4, "content": "mixed", "chars": 120, "unmapped": 0.05,
      "body_pt": 22.0, "small_pt": 20.0, "scripts": 2, "short_lines": 0.44,
      "images": 0, "image_cover": 0.0, "vector_cover": 0.09, "text": "..." }
  ]
}
```

`probe --per-page --json` has the same fields (without `text`) in `pages`, plus
`body_text_pt` and, per estimate, `body_text_px`.
