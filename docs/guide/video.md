# Video and animation input

```sh
gridity render clip.mp4 [--start T] [--end T] [--every T | --count N | --at T,T,...]
gridity render anim.gif  (same options; also APNG)
```

Videos need `ffmpeg` and `ffprobe` (`brew install ffmpeg`). Point gridity at another
build with `--ffmpeg /path/to/ffmpeg` or `GRIDITY_FFMPEG`; ffprobe is looked up next to
it, or via `GRIDITY_FFPROBE`. GIF and APNG are decoded without ffmpeg.

## Time syntax

`90`, `90.5`, `90s`, `500ms`, `1m30s`, `1h2m`, `1:30`, `01:02:03.5`

## Sampling

| Option | Frames taken |
|---|---|
| (none) | As many as fit on one sheet, evenly spaced |
| `--count N` | N evenly spaced frames (middle of each slice, avoiding fade-ins) |
| `--every T` | One frame every T, starting at `--start` |
| `--at T1,T2` | Exactly these times |

`--start` (default 0) and `--end` (default: end of media) limit the range for `--count`
and `--every`. Times past the end are an error (exit 2).

For animations, each time maps to the frame visible at that moment; duplicates are merged.
Frame timing follows browsers: a delay of 10 ms or less is shown for 100 ms.

## Labels

Default `{time}` as a badge in the top-left corner, truncated like a media player
(`1:15`, or `1:15.2` when frames are less than a second apart; `h:mm:ss` for long media).
Variables: `{time}`, `{index}`, `{total}`, and `{frame}` for animations.

## Geometry

Frames are laid out at their display size: non-square pixels (SAR) and rotation metadata
(phone videos) are applied, and frames are never upscaled beyond their native resolution.
`--crop X,Y,W,H` keeps a region of every frame, in fractions of the displayed frame (see
`gridity guide layout`).

## Output

JPEG by default (`--format png` to change). Undecodable timestamps become gray cells and a
warning, never a silent gap.

`--pages` and `--password` are rejected for video and animation input.
