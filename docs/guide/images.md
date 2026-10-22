# Image input

```sh
gridity render a.png b.jpg c.webp
gridity render screenshots/            # every image in the directory (not recursive)
```

- Formats: PNG, JPEG, GIF (first frame when several inputs are given), WebP, BMP, TIFF.
- Directories are sorted naturally: `shot2.png` comes before `shot10.png`. Files whose name
  starts with `.` (such as macOS `._shot.png` resource forks) are skipped.
- A single animated GIF or APNG is treated as an animation; pass `--type images` to use
  only its first frame.
- EXIF orientation is applied, so phone photos are upright.
- Images are never upscaled; cells shrink to the largest input instead of adding empty
  space.
- `--crop X,Y,W,H` keeps the same region of every image, in fractions of its upright size.
- An image (or GIF/APNG frame) that needs more than 512 MiB to decode is rejected with
  exit 3; resize it first.
- An unreadable file does not stop the render. If its header cannot be read, it is left
  out with a warning. If it fails to decode later, its cell is gray and the manifest cell
  carries `error`. Only when no input can be read does gridity fail (exit 3).

## Labels

Default `{name}` (file name) below each image. Variables: `{name}`, `{index}`, `{total}`.

```sh
gridity render shots/ --label "{index}/{total} {name}"
```

Selection options (`--pages`, `--start`, `--every`, ...) are rejected for images;
`--crop` works for every input type.
