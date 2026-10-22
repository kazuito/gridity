# AGENTS.md

Guidance for AI agents developing **gridity**, a Rust CLI that arranges PDF pages, video
frames, GIF/APNG frames and images into labeled grid sheets sized for LLM vision.

This file is about developing gridity. To learn how to *use* it, run `gridity guide`
(sources in `docs/guide/`).

## Commands

```sh
cargo build --release              # binary: target/release/gridity
cargo test                         # unit tests (src/**) + integration tests (tests/cli.rs)
cargo clippy --all-targets         # must stay warning-free
cargo fmt                          # run before finishing any change
```

The video tests need `ffmpeg`/`ffprobe` on `PATH`; they skip themselves when it is missing.
Before finishing, run all four commands above. If you change rendering, also render a real
input and look at the output image (see "Manual verification").

CI (`.github/workflows/ci.yml`) runs the same checks on Linux, macOS and Windows, plus
`cargo check` with the MSRV (`rust-version` in `Cargo.toml`), `cargo deny check`
(advisories, licenses, sources; config in `deny.toml`) and a check that `image`,
`imageproc` and `ab_glyph` have no duplicate versions. When adding a dependency, run
`cargo deny check` locally if `cargo-deny` is installed.

## Layout

```
src/
  main.rs        entry point; maps errors to exit codes; JSON errors with --json
  cli.rs         clap derive definitions (all flags, defaults, help text)
  error.rs       CliError { kind, reason, message, hint }; ErrorKind -> exit code;
                 Reason -> stable `reason` string in JSON errors
  detect.rs      input type detection (magic bytes, extensions, APNG acTL check)
  select.rs      pure parsing: --pages, times, --crop, sampling (Count/Every/At), time labels
  layout.rs      pure layout math: Limits, fits(), token_cost(), best_grid(), capacity(), plan()
  compose.rs     draws a sheet: header, cells, borders, labels; place() positions images
  text.rs        font loading (embedded BIZ UDGothic) and text drawing
  output.rs      output paths (tmp dir + hash), encoding, JSON structs (serde + schemars),
                 SCHEMA_VERSION
  render.rs      `render` command: wires everything together; shared helpers for `probe`
  probe.rs       `probe` command: source info, per-detail estimates and scale, `--per-page`
                 PDF analysis, suggested commands
  extract.rs     `text` command: visible PDF text per page with its content type
  pagetext.rs    pure page analysis: text assembly, text sizes, coverage, content type
  guide.rs       `guide` command: include_str! of docs/guide/*.md
  mcp.rs         `mcp` command: synchronous JSON-RPC over stdin/stdout; tool parameters
                 become CLI arguments parsed by clap, then render::execute / probe::report /
                 extract::report
  source/
    mod.rs       Item, ItemKey, LabelVars, enum Source, fit_size(), Warnings
    pdf.rs       hayro renderer (`render_page`) and page scanner (`scan_page`, `Collector`)
    video.rs     ffprobe/ffmpeg subprocesses
    anim.rs      GIF/APNG via image::AnimationDecoder
    images.rs    image files/directories, EXIF orientation, natural sort
docs/guide/      user/agent documentation embedded into the binary
tests/cli.rs     integration tests; fixtures are generated in temp dirs
tests/fixtures/  committed fixtures that cannot be generated in Rust (encrypted.pdf)
samples/         committed inputs for visual review; render.sh writes samples/out/ (gitignored)
assets/fonts/    embedded font + its OFL license (keep OFL.txt next to the font)
```

## Pipeline

`render::run`: `detect` → `check_flags` → `build_layout` → `Source::open` →
`select_items` (→ `Vec<Item>`, cropped by `--crop`) → `layout::plan` (→ `Vec<SheetPlan>`)
→ per sheet: `Source::render` → `compose` → `encode` → write → print paths or manifest.

`probe` and `text` scan PDF pages with `PdfSource::scan` (→ `pagetext::PageScan`) and
summarize them with `pagetext::summarize`.

Each source turns a selection into `Item`s (aspect ratio, optional native size, key, label
variables), then renders a batch of items into `Rendered` images that fit given boxes. An
item that cannot be decoded becomes a gray placeholder of the planned size with `error` set
(shown in the manifest cell), so one bad file never fails a whole render.

## Invariants (do not break)

- **Sheets never exceed `Limits`.** Every sheet satisfies `layout::fits()`, which mirrors
  Claude's resize rule: each edge rounded up to 28 px ≤ `max_edge`, and
  `ceil(w/28) * ceil(h/28)` ≤ `max_tokens`. Presets: `compat` 1568/1568, `hires`
  2576/4784. The test `grids_never_exceed_limits` guards this.
- **Dry-run rects equal rendered rects.** Both use `compose::place()`. Never position a
  cell image any other way.
- **Raster sources are never upscaled.** `fit_size` caps at `Item::native`, and
  `PlanOptions::max_cell` (from `render::native_bounds`) shrinks cells to the largest
  native size. With `--crop`, both describe the cropped region.
- **Probe numbers equal render numbers.** `probe` computes `scale` from `layout::plan`
  with real default `RenderArgs`, the same path `render` uses.
- **Output contract.** For `render`, stdout carries only sheet paths (one absolute path
  per line), the JSON manifest, or the dry-run summary; `probe` prints its report and
  `text` the page text (or JSON with `--json`). Everything else goes to stderr. Exit codes:
  0 ok, 1 runtime, 2 usage, 3 input, 4 dependency. With `--json`, errors are also
  printed as `{"error": {...}}` on stdout. For `mcp`, stdout carries only JSON-RPC
  messages, one per line; nothing reached from a tool call may print to stdout.
- **JSON is versioned and typed.** Every JSON document is a serde struct (no `json!`) with
  `schema_version` first. Adding a field keeps `SCHEMA_VERSION`; removing, renaming or
  retyping one raises it. `docs/schema/*.schema.json` are generated from the structs; after
  changing them run `GRIDITY_UPDATE_SCHEMA=1 cargo test committed_schemas` and commit the
  result. `reason` names (`error::Reason`) are never renamed, and each is listed in
  `docs/guide/output.md` (a unit test checks this).
- **Computing is separate from printing.** `render::execute`, `probe::report` and
  `extract::report` return values and never write to stdout; their `run` functions print.
- **Flags for another input type are errors**, never silently ignored (`check_flags`).
  Label templates may only use the variables of the input kind (`label_vars`).
- **Deterministic output location.** The default directory hashes the input paths and
  options (`render::fingerprint`), not the time or mtime, so a re-run overwrites the same
  files. `Target::remove_stale` deletes old sheets only in that gridity-managed directory.
- **Private, atomic output.** The default root `$TMPDIR/gridity-<uid>/` is created 0700 and
  refused if another user owns it or can write to it (`output::create_private_dir`).
  Sheets are written with `output::write_atomic` (temp file + rename), never `fs::write`.
- **Items are split evenly** across sheets (10 at 8 per sheet → 5 + 5).
- **Video geometry uses display size**: SAR applied, width/height swapped for 90°/270°
  rotation (ffmpeg autorotates frames).

## Code style

- Write code that describes itself. Do not add comments that restate the code; keep doc
  comments short and only where intent or a non-obvious constraint needs explaining.
  Match the density and naming of the surrounding code.
- Keep the Rust simple; the maintainer is not a Rust expert:
  - pass owned types (`Vec<u8>`, `RgbaImage`, `String`, `PathBuf`) between modules; no
    lifetime parameters in our own types;
  - dispatch input kinds with `enum Source` and `match`, not traits or generics;
  - no async; parallelism only through `rayon` `par_iter` (thread count set once in
    `main::configure_threads`);
  - keep hayro's borrowed types inside `source/pdf.rs` (`render_page`, `scan_page` and
    the lifetime-free `Collector`, which implements `Device<'a>` for every `'a`).
- Errors: return `anyhow::Result`. For anything the user or an agent can fix, return a
  `CliError` built with `error::usage` / `input` / `dependency` and a `Reason` (add a new
  variant, plus a row in `docs/guide/output.md`, when none fits) and add `.with_hint(...)`
  with a concrete next step (a flag to pass, or `see: gridity guide <topic>`).
- Keep `layout.rs` and `select.rs` pure (no I/O) and unit-tested.
- `hayro` is pinned (`=0.8.0`). Upgrade deliberately and re-check the PDF fixtures.
- `image`, `imageproc` and `ab_glyph` must stay a compatible set (`cargo tree -d` should
  not show duplicates of them).

## Making changes

**Adding or changing an option**
1. Add it to the right `Args` struct in `cli.rs` with a `help_heading`, and show the
   default in the help text.
2. Apply it in `render.rs` (and in `check_flags` if it only applies to some input kinds).
3. If it changes the layout, make sure `--dry-run` and `probe` estimates reflect it.
   `probe` builds its layout from real default `RenderArgs` via `default_render_args`.
4. Update `docs/guide/*.md` (the shipped documentation).
5. Add a unit test for the logic and an integration test for the CLI behavior.

**Adding an input kind**
Add an `InputKind` variant, detection in `detect.rs`, a module in `source/` with
`open` / `items` / `render`, a `Source` variant, and a case in every `match` in
`render.rs` and `probe.rs` (the compiler lists them). Also add defaults: label, label
position, format, `min_cell`, label variables, and a guide topic.

**Tuning page analysis**
Thresholds for content types and `READABLE_PX` are constants in `pagetext.rs`. Check
changes with `gridity text` / `gridity probe --per-page` on `samples/inputs/*.pdf` and a
real slide deck or paper, and keep `docs/guide/text.md` in sync.

**Tuning `--detail`**
`render::min_cell` holds the minimum cell long edge per kind and detail. It is calibrated
so the compat preset gives PDF 9/4/2, video 16/12/6 and images 12/6/3 items per sheet
(A4 and Letter). Check with `gridity probe <file>` and
`gridity render <file> --per-sheet N --dry-run`, and keep `docs/guide/layout.md` in sync.

## Testing

- Unit tests live next to the code (`#[cfg(test)] mod tests`).
- Integration tests in `tests/cli.rs` generate their fixtures: `write_pdf` (a minimal
  PDF writer with page sizes and rotation), `write_png`, `write_exif_jpeg`, `write_gif`,
  and `write_video` (ffmpeg `testsrc`). Prefer extending these over committing binaries.
  Assert on manifest fields, image dimensions and exit codes, not on pixels.
- Put scratch fixtures and manual outputs under `target/manual/` (gitignored), never in
  the repo root.
- `samples/` is for human review only; tests keep generating their own fixtures. Add a
  `case_` to `samples/render.sh` when a change needs a new visual check.

## Manual verification

```sh
G=target/release/gridity
$G probe <input>
$G render <input> --dry-run
$G render <input> -v            # then open the printed path and inspect the image
$G render <input> --json        # check rects, labels, warnings
```

When changing visuals, run `samples/render.sh` and look at `samples/out/index.html`. It
covers PDFs (including rotated and mixed page sizes), a 16:9 video, a rotated and a
non-square-pixel video, a small GIF, an APNG and directories of images
(`samples/README.md` lists what each input checks).

## Git

- Do not commit or push unless the user asks.
- Commit `Cargo.lock` (binary crate) and keep `assets/fonts/OFL.txt` with the font.
- The project is dual-licensed `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`).
