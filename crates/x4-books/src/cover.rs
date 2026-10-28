//! Cover layout: find bars already baked into a cover, read the background along each edge,
//! and compose the cover onto a reader screen — cropped, zoomed, moved, with the uncovered
//! area filled by the cover's own edge colors, a blurred copy, or a plain color.
//!
//! Rectangles and offsets are fractions (0–1) of the image or screen, so the UI can work on a
//! small preview and the bake on the full image with the same numbers. `src/lib/geometry.js`
//! mirrors `place`.

use image::imageops::{self, FilterType};
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use serde::{Deserialize, Serialize};

use crate::imaging::{decode, Fit};
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const FULL: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
}

/// Background along one edge of the (trimmed) cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    pub color: [u8; 3],
    /// Nearly one color: extending it past the edge looks seamless.
    pub uniform: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    /// The cover without bars that were already baked into it (full image if none).
    pub trim: Rect,
    pub top: Side,
    pub bottom: Side,
    pub left: Side,
    pub right: Side,
}

/// How the uncovered parts of the screen are filled.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Fill {
    /// Each side extended with its own edge color.
    Sides {
        top: [u8; 3],
        bottom: [u8; 3],
        left: [u8; 3],
        right: [u8; 3],
    },
    /// A blurred, screen-filling copy of the cover.
    Blur,
    Color {
        color: [u8; 3],
    },
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// Part of the image to use (fractions); `None` = all of it.
    #[serde(default)]
    pub crop: Option<Rect>,
    pub fit: Fit,
    /// 1 = as `fit` places it.
    #[serde(default = "one")]
    pub zoom: f32,
    /// Shift as a fraction of the screen width / height.
    #[serde(default)]
    pub dx: f32,
    #[serde(default)]
    pub dy: f32,
    pub fill: Fill,
}

impl Layout {
    pub fn simple(fit: Fit, background: [u8; 3]) -> Self {
        Layout {
            crop: None,
            fit,
            zoom: 1.0,
            dx: 0.0,
            dy: 0.0,
            fill: Fill::Color { color: background },
        }
    }
}

/// Where an image of aspect `cw`:`ch` lands on a `sw`×`sh` screen: (left, top, width, height).
pub fn place(cw: f64, ch: f64, sw: f64, sh: f64, l: &Layout) -> (f64, f64, f64, f64) {
    let ia = cw / ch;
    let (mut w, mut h) = match l.fit {
        Fit::Stretch => (sw, sh),
        Fit::Cover if sw / sh < ia => (sh * ia, sh),
        Fit::Cover => (sw, sw / ia),
        Fit::Contain if sw / sh < ia => (sw, sw / ia),
        Fit::Contain => (sh * ia, sh),
    };
    let z = l.zoom.clamp(0.1, 10.0) as f64;
    w *= z;
    h *= z;
    let left = (sw - w) / 2.0 + l.dx as f64 * sw;
    let top = (sh - h) / 2.0 + l.dy as f64 * sh;
    (left, top, w, h)
}

/// Largest channel difference.
fn dist(a: [u8; 3], b: [u8; 3]) -> u8 {
    (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
}

fn mean(px: &[[u8; 3]]) -> [u8; 3] {
    let n = px.len().max(1) as u64;
    let mut s = [0u64; 3];
    for p in px {
        for i in 0..3 {
            s[i] += p[i] as u64;
        }
    }
    [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]
}

/// Share of `px` within `tol` of `color`.
fn share_near(px: &[[u8; 3]], color: [u8; 3], tol: u8) -> f32 {
    if px.is_empty() {
        return 0.0;
    }
    px.iter().filter(|p| dist(**p, color) <= tol).count() as f32 / px.len() as f32
}

struct Grid<'a> {
    img: &'a RgbImage,
}

impl Grid<'_> {
    fn row(&self, y: u32, x0: u32, x1: u32) -> Vec<[u8; 3]> {
        (x0..x1).map(|x| self.img.get_pixel(x, y).0).collect()
    }
    fn col(&self, x: u32, y0: u32, y1: u32) -> Vec<[u8; 3]> {
        (y0..y1).map(|y| self.img.get_pixel(x, y).0).collect()
    }
}

const BAR_TOL: u8 = 18;
const BAR_SHARE: f32 = 0.985;

/// Finds baked-in bars and the background along each edge of what is left.
pub fn analyze(img: &DynamicImage) -> Analysis {
    // A small copy is plenty and keeps this fast on big covers.
    let small = if img.width() > 480 || img.height() > 800 {
        img.resize(480, 800, FilterType::Triangle).to_rgb8()
    } else {
        img.to_rgb8()
    };
    let (w, h) = small.dimensions();
    let g = Grid { img: &small };
    let (mut x0, mut y0, mut x1, mut y1) = (0u32, 0u32, w, h);
    let max_y = (h as f32 * 0.4) as u32;
    let max_x = (w as f32 * 0.4) as u32;

    // A line is a bar when it is one color, the same color as the first line of that edge.
    let is_bar = |px: &[[u8; 3]], reference: [u8; 3]| {
        share_near(px, reference, BAR_TOL) >= BAR_SHARE && dist(mean(px), reference) <= BAR_TOL
    };
    let mut refs = [[0u8; 3]; 4]; // top, bottom, left, right bar colors
    if h > 4 && w > 4 {
        refs[0] = mean(&g.row(0, 0, w));
        while y0 < max_y && is_bar(&g.row(y0, 0, w), refs[0]) {
            y0 += 1;
        }
        refs[1] = mean(&g.row(h - 1, 0, w));
        while h - y1 < max_y && y1 > y0 + 1 && is_bar(&g.row(y1 - 1, 0, w), refs[1]) {
            y1 -= 1;
        }
        refs[2] = mean(&g.col(0, y0, y1));
        while x0 < max_x && is_bar(&g.col(x0, y0, y1), refs[2]) {
            x0 += 1;
        }
        refs[3] = mean(&g.col(w - 1, y0, y1));
        while w - x1 < max_x && x1 > x0 + 1 && is_bar(&g.col(x1 - 1, y0, y1), refs[3]) {
            x1 -= 1;
        }
        // A bar frames the cover from one side only. If the cover's other edges are the same
        // color, it is the cover's own background (a white margin of a white cover): keep it.
        let (ex0, ey0, ex1, ey1) = (x0, y0, x1, y1);
        let sides_of = |vertical: bool| -> Vec<[u8; 3]> {
            if vertical {
                let mut v = g.col(ex0, ey0, ey1);
                v.extend(g.col(ex1 - 1, ey0, ey1));
                v
            } else {
                let mut v = g.row(ey0, ex0, ex1);
                v.extend(g.row(ey1 - 1, ex0, ex1));
                v
            }
        };
        let framed = |strip: Vec<[u8; 3]>, bar: [u8; 3]| share_near(&strip, bar, 28) > 0.5;
        if y0 > 0 && framed(sides_of(true), refs[0]) {
            y0 = 0;
        }
        if y1 < h && framed(sides_of(true), refs[1]) {
            y1 = h;
        }
        if x0 > 0 && framed(sides_of(false), refs[2]) {
            x0 = 0;
        }
        if x1 < w && framed(sides_of(false), refs[3]) {
            x1 = w;
        }
    }
    // Ignore hairlines, and give up on images that are (almost) all one color.
    let min_bar = 2;
    if y0 < min_bar {
        y0 = 0;
    }
    if h - y1 < min_bar {
        y1 = h;
    }
    if x0 < min_bar {
        x0 = 0;
    }
    if w - x1 < min_bar {
        x1 = w;
    }
    if (x1 - x0) < w * 3 / 10 || (y1 - y0) < h * 3 / 10 {
        (x0, y0, x1, y1) = (0, 0, w, h);
    }

    let side = |px: Vec<[u8; 3]>| {
        let color = mean(&px);
        Side {
            color,
            uniform: share_near(&px, color, 28) >= 0.92,
        }
    };
    let tv = ((y1 - y0) / 30).max(2).min(y1 - y0);
    let th = ((x1 - x0) / 30).max(2).min(x1 - x0);
    let rows = |a: u32, b: u32| (a..b).flat_map(|y| g.row(y, x0, x1)).collect::<Vec<_>>();
    let cols = |a: u32, b: u32| (a..b).flat_map(|x| g.col(x, y0, y1)).collect::<Vec<_>>();
    Analysis {
        trim: Rect {
            x: x0 as f32 / w as f32,
            y: y0 as f32 / h as f32,
            w: (x1 - x0) as f32 / w as f32,
            h: (y1 - y0) as f32 / h as f32,
        },
        top: side(rows(y0, y0 + tv)),
        bottom: side(rows(y1 - tv, y1)),
        left: side(cols(x0, x0 + th)),
        right: side(cols(x1 - th, x1)),
    }
}

fn crop_px(img: &DynamicImage, r: Rect) -> DynamicImage {
    let (w, h) = img.dimensions();
    let x = ((r.x.clamp(0.0, 1.0) * w as f32).round() as u32).min(w - 1);
    let y = ((r.y.clamp(0.0, 1.0) * h as f32).round() as u32).min(h - 1);
    let cw = ((r.w * w as f32).round() as u32).clamp(1, w - x);
    let ch = ((r.h * h as f32).round() as u32).clamp(1, h - y);
    img.crop_imm(x, y, cw, ch)
}

/// The cover composed onto a `sw`×`sh` screen.
pub fn compose_image(img: &DynamicImage, sw: u32, sh: u32, l: &Layout) -> RgbImage {
    let src = match l.crop {
        Some(r) if r != Rect::FULL => crop_px(img, r),
        _ => img.clone(),
    };
    let (cw, ch) = src.dimensions();
    let (left, top, w, h) = place(cw as f64, ch as f64, sw as f64, sh as f64, l);

    let mut canvas = match l.fill {
        Fill::Color { color } => RgbImage::from_pixel(sw, sh, Rgb(color)),
        Fill::Blur => {
            let filled = src.resize_to_fill(sw.div_ceil(4), sh.div_ceil(4), FilterType::Triangle);
            let blurred = imageops::blur(&filled.to_rgb8(), (sw as f32 / 120.0).max(1.0));
            imageops::resize(&blurred, sw, sh, FilterType::Triangle)
        }
        Fill::Sides {
            top: t,
            bottom: b,
            left: lc,
            right: rc,
        } => RgbImage::from_fn(sw, sh, |x, y| {
            let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
            Rgb(if y < top {
                t
            } else if y >= top + h {
                b
            } else if x < left {
                lc
            } else if x >= left + w {
                rc
            } else {
                t // under the image
            })
        }),
    };

    let (rw, rh) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
    let scaled = src.resize_exact(rw, rh, FilterType::Lanczos3).to_rgb8();
    imageops::overlay(
        &mut canvas,
        &scaled,
        left.round() as i64,
        top.round() as i64,
    );
    canvas
}

pub fn compose(bytes: &[u8], sw: u32, sh: u32, l: &Layout) -> Result<RgbImage> {
    Ok(compose_image(&decode(bytes)?, sw, sh, l))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 3] = [250, 250, 248];
    const BLACK: [u8; 3] = [5, 5, 5];

    /// A 300×400 white cover with a dark title block, letterboxed with black bars of `bar` px.
    fn barred(bar: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(300, 400 + 2 * bar, |x, y| {
            if y < bar || y >= 400 + bar {
                Rgb(BLACK)
            } else if (100..200).contains(&x) && (bar + 150..bar + 250).contains(&y) {
                Rgb([40, 60, 120])
            } else {
                Rgb(WHITE)
            }
        }))
    }

    #[test]
    fn finds_baked_in_bars_and_edge_colors() {
        let a = analyze(&barred(50));
        assert!((a.trim.y - 50.0 / 500.0).abs() < 0.01, "{:?}", a.trim);
        assert!((a.trim.h - 400.0 / 500.0).abs() < 0.01, "{:?}", a.trim);
        assert_eq!((a.trim.x, a.trim.w), (0.0, 1.0));
        for s in [a.top, a.bottom, a.left, a.right] {
            assert!(s.uniform && dist(s.color, WHITE) <= 2, "{s:?}");
        }
        // A "photo" (busy edges) letterboxed in black: the bars go.
        let photo = DynamicImage::ImageRgb8(RgbImage::from_fn(300, 500, |x, y| {
            if !(60..440).contains(&y) {
                Rgb(BLACK)
            } else {
                let v = ((x * 7 + y * 13) % 200 + 40) as u8;
                Rgb([v, v / 2 + 60, 255 - v])
            }
        }));
        let p = analyze(&photo);
        assert!(
            (p.trim.y - 0.12).abs() < 0.01 && (p.trim.h - 0.76).abs() < 0.01,
            "{:?}",
            p.trim
        );
        assert!(!p.top.uniform);
        // No bars: nothing trimmed.
        let plain = analyze(&barred(0));
        assert_eq!(plain.trim, Rect::FULL);
        // All one color: not "all bars".
        let blank = DynamicImage::ImageRgb8(RgbImage::from_pixel(100, 160, Rgb(WHITE)));
        assert_eq!(analyze(&blank).trim, Rect::FULL);
    }

    #[test]
    fn extends_the_cover_to_the_screen_edges() {
        let img = barred(50);
        let a = analyze(&img);
        let l = Layout {
            crop: Some(a.trim),
            fit: Fit::Contain,
            zoom: 1.0,
            dx: 0.0,
            dy: 0.0,
            fill: Fill::Sides {
                top: a.top.color,
                bottom: a.bottom.color,
                left: a.left.color,
                right: a.right.color,
            },
        };
        let out = compose_image(&img, 480, 800, &l);
        assert_eq!(out.dimensions(), (480, 800));
        // The black bars are gone: corners and edges carry the cover's white.
        for (x, y) in [(2, 2), (477, 797), (240, 5), (240, 795)] {
            assert!(
                dist(out.get_pixel(x, y).0, WHITE) <= 6,
                "{x},{y}: {:?}",
                out.get_pixel(x, y)
            );
        }
        // The title block is still there, centered.
        assert!(dist(out.get_pixel(240, 400).0, [40, 60, 120]) <= 12);
    }

    #[test]
    fn zoom_and_shift_follow_place() {
        let l = Layout {
            zoom: 0.5,
            dx: 0.25,
            ..Layout::simple(Fit::Stretch, BLACK)
        };
        assert_eq!(
            place(1.0, 1.0, 400.0, 800.0, &l),
            (200.0, 200.0, 200.0, 400.0)
        );
        let src = DynamicImage::ImageRgb8(RgbImage::from_pixel(10, 10, Rgb(WHITE)));
        let out = compose_image(&src, 400, 800, &l);
        assert_eq!(out.get_pixel(100, 400).0, BLACK);
        assert!(dist(out.get_pixel(300, 400).0, WHITE) <= 2);
        let blur = Layout {
            fill: Fill::Blur,
            ..Layout::simple(Fit::Contain, BLACK)
        };
        assert_eq!(
            compose_image(&src, 480, 800, &blur).dimensions(),
            (480, 800)
        );
    }
}
