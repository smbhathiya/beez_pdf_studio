//! Generates the app icon: `cargo run --example make_icon`
//!
//! Writes `assets/icon.ico` (16–256 px, embedded in the .exe and used by the installer)
//! and `assets/icon.png` (256 px, used as the window icon).
//!
//! The artwork is drawn with signed-distance functions on a 1024 px canvas, so every edge is
//! anti-aliased, and then downsampled for each icon size.

use std::path::Path;

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::FilterType;
use image::{ExtendedColorType, RgbaImage};

const N: u32 = 1024;
type Rgb = [f32; 3];

fn hex(c: u32) -> Rgb {
    [
        ((c >> 16) & 255) as f32 / 255.0,
        ((c >> 8) & 255) as f32 / 255.0,
        (c & 255) as f32 / 255.0,
    ]
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Coverage of a shape from its signed distance (negative inside), ~1 px anti-aliasing.
fn cover(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn sd_round_rect(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (x - cx).abs() - hw + r;
    let qy = (y - cy).abs() - hh + r;
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

/// Regular hexagon with flat left/right sides; `r` is the inradius.
fn sd_hexagon(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let (kx, ky, kz) = (-0.866_025_4_f32, 0.5_f32, 0.577_350_3_f32);
    // Rotate 90° so the hexagon has pointy top/bottom like a honeycomb cell.
    let (mut px, mut py) = ((y - cy).abs(), (x - cx).abs());
    let d = 2.0 * (kx * px + ky * py).min(0.0);
    px -= d * kx;
    py -= d * ky;
    px -= px.clamp(-kz * r, kz * r);
    py -= r;
    (px * px + py * py).sqrt() * py.signum()
}

struct Canvas {
    px: Vec<[f32; 4]>, // premultiplied RGBA
}

impl Canvas {
    fn new() -> Self {
        Self {
            px: vec![[0.0; 4]; (N * N) as usize],
        }
    }

    /// Paint `color` with per-pixel coverage `f(x, y) -> (alpha, color)`.
    fn paint(&mut self, f: impl Fn(f32, f32) -> Option<(f32, Rgb)>) {
        for y in 0..N {
            for x in 0..N {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                if let Some((a, c)) = f(fx, fy) {
                    if a <= 0.0 {
                        continue;
                    }
                    let p = &mut self.px[(y * N + x) as usize];
                    for i in 0..3 {
                        p[i] = c[i] * a + p[i] * (1.0 - a);
                    }
                    p[3] = a + p[3] * (1.0 - a);
                }
            }
        }
    }

    fn to_image(&self) -> RgbaImage {
        let mut img = RgbaImage::new(N, N);
        for (i, p) in self.px.iter().enumerate() {
            let a = p[3];
            let un = |v: f32| {
                if a > 0.0 {
                    (v / a * 255.0).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            img.as_mut()[i * 4..i * 4 + 4].copy_from_slice(&[
                un(p[0]),
                un(p[1]),
                un(p[2]),
                (a * 255.0).round() as u8,
            ]);
        }
        img
    }
}

fn draw() -> RgbaImage {
    let mut c = Canvas::new();

    // 1. Tile: rounded square with a diagonal Windows-blue gradient.
    let (top, bottom) = (hex(0x3AA0FF), hex(0x0052CC));
    c.paint(|x, y| {
        let d = sd_round_rect(x, y, 512.0, 512.0, 448.0, 448.0, 196.0);
        Some((
            cover(d),
            lerp(top, bottom, ((x + y) / 2048.0).clamp(0.0, 1.0)),
        ))
    });

    // Document geometry: x 262..702, y 200..800, folded top-right corner.
    let (dl, dr, dt, db, fold) = (262.0, 702.0, 200.0, 800.0, 150.0);
    let doc = move |x: f32, y: f32| {
        let rect = sd_round_rect(
            x,
            y,
            (dl + dr) / 2.0,
            (dt + db) / 2.0,
            (dr - dl) / 2.0,
            (db - dt) / 2.0,
            34.0,
        );
        let cut = ((x - (dr - fold)) - (y - dt)) / std::f32::consts::SQRT_2;
        rect.max(cut)
    };

    // 2. Soft shadow under the document.
    c.paint(|x, y| {
        let d = doc(x, y - 18.0);
        let a = 0.30 * (1.0 - (d / 40.0).clamp(0.0, 1.0));
        Some((a, hex(0x002A6B)))
    });

    // 3. Document page.
    c.paint(|x, y| Some((cover(doc(x, y)), hex(0xFFFFFF))));

    // 4. Folded corner.
    c.paint(|x, y| {
        let a = (dr - fold) - x; // left edge
        let b = y - (dt + fold); // bottom edge
        let diag = ((x - (dr - fold)) - (y - dt)) / std::f32::consts::SQRT_2;
        Some((cover(a.max(b).max(diag)), hex(0xBBD6F7)))
    });

    // 5. Text lines.
    for (i, right) in [600.0, 640.0, 520.0].iter().enumerate() {
        let cy = 420.0 + i as f32 * 72.0;
        let (l, r) = (322.0, *right);
        c.paint(move |x, y| {
            let d = sd_round_rect(x, y, (l + r) / 2.0, cy, (r - l) / 2.0, 15.0, 15.0);
            Some((cover(d), hex(0xA8C4E8)))
        });
    }

    // 6. Honeycomb badge: white ring, golden cell, inner cell outline.
    let (hx, hy) = (690.0, 690.0);
    c.paint(|x, y| Some((cover(sd_hexagon(x, y, hx, hy, 205.0)), hex(0xFFFFFF))));
    let (gold_top, gold_bottom) = (hex(0xFFCB45), hex(0xF5A300));
    c.paint(|x, y| {
        let t = ((y - (hy - 180.0)) / 360.0).clamp(0.0, 1.0);
        Some((
            cover(sd_hexagon(x, y, hx, hy, 175.0)),
            lerp(gold_top, gold_bottom, t),
        ))
    });
    c.paint(|x, y| {
        let ring = sd_hexagon(x, y, hx, hy, 92.0).abs() - 13.0;
        Some((cover(ring), hex(0xC97F00)))
    });

    c.to_image()
}

fn main() {
    let big = draw();
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    std::fs::create_dir_all(&out).unwrap();

    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let images: Vec<RgbaImage> = sizes
        .iter()
        .map(|&s| image::imageops::resize(&big, s, s, FilterType::Lanczos3))
        .collect();

    images.last().unwrap().save(out.join("icon.png")).unwrap();
    big.save(Path::new(env!("CARGO_MANIFEST_DIR")).join("target/icon-1024.png"))
        .ok();

    let frames: Vec<IcoFrame> = images
        .iter()
        .map(|img| {
            IcoFrame::as_png(
                img.as_raw(),
                img.width(),
                img.height(),
                ExtendedColorType::Rgba8,
            )
            .unwrap()
        })
        .collect();
    let file = std::fs::File::create(out.join("icon.ico")).unwrap();
    IcoEncoder::new(file).encode_images(&frames).unwrap();
    println!("Wrote {} and icon.png", out.join("icon.ico").display());
}
