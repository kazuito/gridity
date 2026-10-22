use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use rayon::prelude::*;

use super::{Item, ItemKey, LabelVars, decode_error, decode_limits, file_name, fit_size};
use crate::error::input;

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];

pub struct ImageFile {
    pub path: PathBuf,
    /// Size after applying EXIF orientation.
    pub size: (u32, u32),
}

pub struct ImagesSource {
    pub files: Vec<ImageFile>,
    pub name: String,
}

impl ImagesSource {
    pub fn open(inputs: &[PathBuf]) -> Result<ImagesSource> {
        let mut paths = Vec::new();
        for input_path in inputs {
            if input_path.is_dir() {
                paths.extend(list_images(input_path)?);
            } else {
                paths.push(input_path.clone());
            }
        }
        if paths.is_empty() {
            return Err(input("no image files found in the given directory")
                .with_hint(format!(
                    "supported extensions: {}",
                    IMAGE_EXTENSIONS.join(", ")
                ))
                .into());
        }

        let files = paths
            .par_iter()
            .map(|path| {
                Ok(ImageFile {
                    path: path.clone(),
                    size: oriented_size(path)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let name = match inputs {
            [single] => file_name(single),
            _ => format!("{} images", files.len()),
        };
        Ok(ImagesSource { files, name })
    }

    pub fn items(&self) -> Vec<Item> {
        self.files
            .iter()
            .enumerate()
            .map(|(i, file)| Item {
                aspect: file.size.0 as f64 / file.size.1 as f64,
                native: Some(file.size),
                key: ItemKey::File(file.path.clone()),
                vars: LabelVars {
                    index: i + 1,
                    total: self.files.len(),
                    name: Some(file_name(&file.path)),
                    ..LabelVars::default()
                },
                crop: None,
            })
            .collect()
    }

    pub fn render(&self, items: &[Item], boxes: &[(u32, u32)]) -> Result<Vec<RgbaImage>> {
        items
            .par_iter()
            .zip(boxes)
            .map(|(item, &bounds)| {
                let ItemKey::File(path) = &item.key else {
                    unreachable!("image items are files")
                };
                let (w, h) = fit_size(item.aspect, bounds, item.native);
                let mut image = decode_oriented(path)?;
                if let Some(crop) = item.crop {
                    let (x, y, cw, ch) = crop.pixels((image.width(), image.height()));
                    image = image.crop_imm(x, y, cw, ch);
                }
                Ok(image.resize_exact(w, h, FilterType::CatmullRom).to_rgba8())
            })
            .collect()
    }
}

fn list_images(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && has_image_extension(p))
        .collect();
    paths.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    Ok(paths)
}

pub fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn open_decoder(path: &Path) -> Result<impl ImageDecoder> {
    let unreadable =
        |e: &dyn std::fmt::Display| input(format!("cannot read image {}: {e}", path.display()));
    let mut reader = ImageReader::open(path)
        .map_err(|e| unreadable(&e))?
        .with_guessed_format()
        .map_err(|e| unreadable(&e))?;
    reader.limits(decode_limits());
    reader
        .into_decoder()
        .map_err(|e| decode_error(path, e).into())
}

fn oriented_size(path: &Path) -> Result<(u32, u32)> {
    let mut decoder = open_decoder(path)?;
    let (w, h) = decoder.dimensions();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    Ok(match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (h, w),
        _ => (w, h),
    })
}

fn decode_oriented(path: &Path) -> Result<DynamicImage> {
    let mut decoder = open_decoder(path)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| decode_error(path, e))?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// Orders "img2" before "img10" by comparing digit runs as numbers.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take_number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (na, nb) = (take_number(&mut a), take_number(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let order = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| na.len().cmp(&nb.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut names = vec!["img10.png", "img2.png", "Img1.png", "img02.png", "a.png"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec!["a.png", "Img1.png", "img2.png", "img02.png", "img10.png"]
        );
    }
}
