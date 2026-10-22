# gridity

Arrange PDF pages, video frames, GIF/APNG frames and images into labeled grid sheets,
sized so an LLM can understand many pages or moments from a single image.

```sh
gridity probe report.pdf                          # pages, sizes, sheet estimates
gridity render report.pdf                         # prints absolute paths of the sheets
gridity render report.pdf --pages 3-8 --json      # manifest with cell coordinates
gridity render talk.mp4 --start 1:00 --end 2:30 --every 10s
gridity render screenshots/ --cols 3
gridity guide                                     # documentation for agents
```

Sheets fit Claude's vision limits without downscaling (`--preset compat`: 1568 px /
1568 visual tokens, `--preset hires`: 2576 px / 4784 tokens), and items are split
evenly across sheets when they do not fit on one.

## Install

```sh
cargo install --path .
```

Video input needs `ffmpeg` and `ffprobe` on `PATH` (`brew install ffmpeg`), or
`--ffmpeg` / `GRIDITY_FFMPEG`. PDFs, images, GIF and APNG need nothing else.

## Documentation

`gridity guide [topic]` prints the bundled guide (`docs/guide/*.md`):
`recipes`, `pdf`, `video`, `images`, `layout`, `output`. The design notes are in
[`docs/PLAN.md`](docs/PLAN.md).

## License notes

Labels use the embedded BIZ UDGothic font, licensed under the SIL Open Font License 1.1
(`assets/fonts/OFL.txt`).
