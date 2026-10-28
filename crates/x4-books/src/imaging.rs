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
    /// Baked-in bars and edge colors, for the cover editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<crate::cover::Analysis>,
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
        analysis: Some(crate::cover::analyze(&small)),
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

/// 8-bit grayscale BMP (uncompressed, 256-entry gray palette, bottom-up rows) — the format
/// CrossInk recommends for sleep and boot screens. Luma follows ITU-R BT.601.
pub fn encode_gray_bmp(img: &RgbImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let row = (w as usize + 3) & !3;
    let pixels_at = 14 + 40 + 256 * 4;
    let total = pixels_at + row * h as usize;
    let mut out = Vec::with_capacity(total);
    // BITMAPFILEHEADER
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixels_at as u32).to_le_bytes());
    // BITMAPINFOHEADER
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes()); // positive: bottom-up
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((row * h as usize) as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes()); // 72 dpi
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&256u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for i in 0..=255u8 {
        out.extend_from_slice(&[i, i, i, 0]);
    }
    for y in (0..h).rev() {
        for x in 0..w {
            let Rgb([r, g, b]) = *img.get_pixel(x, y);
            let l = (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
            out.push(l as u8);
        }
        out.resize(out.len() + row - w as usize, 0);
    }
    out
}

/// A picture made into a sleep screen: filled to `w`×`h` (cropping the overflow), grayscale BMP.
pub fn sleep_screen(bytes: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    Ok(encode_gray_bmp(&bake(
        bytes,
        w,
        h,
        Fit::Cover,
        [255, 255, 255],
    )?))
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

    #[test]
    fn gray_bmp_round_trips() {
        let bmp = sleep_screen(&sample(300, 200), 481, 800).unwrap();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u16::from_le_bytes([bmp[28], bmp[29]]), 8);
        assert_eq!(bmp.len(), 14 + 40 + 1024 + 484 * 800);
        let back = decode(&bmp).unwrap();
        assert_eq!(back.dimensions(), (481, 800));
        // Left half was red (luma 60), right half blue (luma 23): both dark, red lighter.
        let px = |x| back.to_luma8().get_pixel(x, 400).0[0];
        assert!(px(100) > px(380), "{} vs {}", px(100), px(380));
    }
}
