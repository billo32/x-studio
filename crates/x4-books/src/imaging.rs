//! Image helpers: thumbnails for the UI and covers baked to the reader's screen.

use std::io::Cursor;

use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat, Rgb, RgbImage};
use serde::{Deserialize, Serialize};

use crate::Result;

/// How a cover is placed on a screen of a different aspect ratio.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Fill the screen, cropping the overflow (centered).
    Cover,
    /// Show the whole image, with bars.
    Contain,
    /// Fill the screen, distorting the proportions.
    Stretch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// `data:image/jpeg;base64,…`
    pub data_url: String,
    /// Size of the original image.
    pub width: u32,
    pub height: u32,
}

pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    Ok(image::load_from_memory(bytes)?)
}

fn jpeg_data_url(img: &DynamicImage, quality: u8) -> Result<String> {
    let mut out = Vec::new();
    let rgb = img.to_rgb8();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&rgb)?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(out)
    ))
}

/// A JPEG data URL scaled to fit inside `max_w`×`max_h`, plus the original size.
pub fn preview(bytes: &[u8], max_w: u32, max_h: u32) -> Result<Preview> {
    let img = decode(bytes)?;
    let (width, height) = img.dimensions();
    let small = if width > max_w || height > max_h {
        img.resize(max_w, max_h, FilterType::Triangle)
    } else {
        img
    };
    Ok(Preview {
        data_url: jpeg_data_url(&small, 82)?,
        width,
        height,
    })
}

/// Renders `bytes` onto a `w`×`h` canvas with `fit`. `background` fills the bars of `Contain`.
pub fn bake(bytes: &[u8], w: u32, h: u32, fit: Fit, background: [u8; 3]) -> Result<RgbImage> {
    let img = decode(bytes)?;
    let out = match fit {
        Fit::Stretch => img.resize_exact(w, h, FilterType::Lanczos3).to_rgb8(),
        Fit::Cover => img.resize_to_fill(w, h, FilterType::Lanczos3).to_rgb8(),
        Fit::Contain => {
            let scaled = img.resize(w, h, FilterType::Lanczos3).to_rgb8();
            let mut canvas = RgbImage::from_pixel(w, h, Rgb(background));
            let x = (w - scaled.width()) / 2;
            let y = (h - scaled.height()) / 2;
            image::imageops::replace(&mut canvas, &scaled, x as i64, y as i64);
            canvas
        }
    };
    Ok(out)
}

pub fn encode(img: &RgbImage, ext: &str) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    match ext {
        "png" => img.write_to(&mut out, ImageFormat::Png)?,
        "bmp" => img.write_to(&mut out, ImageFormat::Bmp)?,
        _ => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(img)?,
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_fn(w, h, |x, _| {
            if x < w / 2 {
                Rgb([200, 0, 0])
            } else {
                Rgb([0, 0, 200])
            }
        });
        encode(&img, "png").unwrap()
    }

    #[test]
    fn bake_sizes_and_bars() {
        let src = sample(200, 300); // 2:3 cover
        for fit in [Fit::Cover, Fit::Contain, Fit::Stretch] {
            let out = bake(&src, 480, 800, fit, [255, 255, 255]).unwrap();
            assert_eq!(out.dimensions(), (480, 800));
        }
        // 2:3 into 3:5 contained: bars of 40 px top and bottom, white.
        let contained = bake(&src, 480, 800, Fit::Contain, [255, 255, 255]).unwrap();
        assert_eq!(contained.get_pixel(240, 10), &Rgb([255, 255, 255]));
        assert_ne!(contained.get_pixel(100, 400), &Rgb([255, 255, 255]));
        let p = preview(&src, 100, 100).unwrap();
        assert_eq!((p.width, p.height), (200, 300));
        assert!(p.data_url.starts_with("data:image/jpeg;base64,"));
    }
}
