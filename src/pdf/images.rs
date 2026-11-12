//! Images → PDF, with every image optimised before it is embedded.
//!
//! PDF cannot hold WebP data, so the same lossless tricks WebP/PNG use are applied
//! with the filters PDF *does* support:
//!   * per-row PNG prediction (None/Sub/Up/Average/Paeth, best chosen per row),
//!   * maximum-level Deflate,
//!   * automatic palette conversion (≤256 colours → 1/2/4/8-bit indexed),
//!   * grayscale detection (RGB images that are really gray → 1 channel),
//!   * transparency kept as a separate soft mask, dropped when fully opaque.
//!
//! JPEG files are embedded untouched in lossless mode (they are already compressed and
//! re-encoding would lose quality). An optional lossy mode re-encodes as JPEG.
//! Images are processed in parallel.

use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use flate2::{Compression as Level, write::ZlibEncoder};
use image::{DynamicImage, ImageDecoder, ImageReader, metadata::Orientation as Exif};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};
use rayon::prelude::*;

use super::progress::Progress;
use super::{human_size, save};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageSize {
    /// Page is exactly the image size (at 96 DPI).
    FitImage,
    A4,
    Letter,
    Legal,
    A3,
    A5,
}

impl PageSize {
    pub const ALL: [PageSize; 6] = [
        PageSize::FitImage,
        PageSize::A4,
        PageSize::Letter,
        PageSize::Legal,
        PageSize::A3,
        PageSize::A5,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PageSize::FitImage => "Same as image",
            PageSize::A4 => "A4 (210 × 297 mm)",
            PageSize::Letter => "US Letter",
            PageSize::Legal => "US Legal",
            PageSize::A3 => "A3",
            PageSize::A5 => "A5",
        }
    }

    fn points(self) -> Option<(f32, f32)> {
        match self {
            PageSize::FitImage => None,
            PageSize::A4 => Some((595.28, 841.89)),
            PageSize::Letter => Some((612.0, 792.0)),
            PageSize::Legal => Some((612.0, 1008.0)),
            PageSize::A3 => Some((841.89, 1190.55)),
            PageSize::A5 => Some((419.53, 595.28)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Orientation {
    Auto,
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quality {
    /// Pixel-perfect: optimised lossless compression.
    Lossless,
    /// Re-encode as JPEG with this quality (10-100). Much smaller for photos.
    Lossy(u8),
}

#[derive(Clone, Debug)]
pub struct ImageOptions {
    pub page_size: PageSize,
    pub orientation: Orientation,
    /// Margin in points (1/72 inch).
    pub margin: f32,
    pub quality: Quality,
    /// Downscale images whose longest side is larger than this (0 = keep resolution).
    pub max_dimension: u32,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            page_size: PageSize::A4,
            orientation: Orientation::Auto,
            margin: 18.0,
            quality: Quality::Lossless,
            max_dimension: 0,
        }
    }
}

/// An image ready to be written into the PDF.
struct Encoded {
    width: u32,
    height: u32,
    dict: Dictionary,
    data: Vec<u8>,
    smask: Option<(Dictionary, Vec<u8>)>,
    source_size: usize,
}

impl Encoded {
    fn pdf_size(&self) -> usize {
        self.data.len() + self.smask.as_ref().map_or(0, |s| s.1.len())
    }
}

// ---------------------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------------------

/// Read a JPEG's dimensions and number of components from its SOF marker.
fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xFF || (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += if marker == 0xFF { 1 } else { 2 };
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
            return Some((w, h, data[i + 9]));
        }
        i += 2 + len;
    }
    None
}

fn image_dict(w: u32, h: u32) -> Dictionary {
    dictionary! {
        "Type" => "XObject", "Subtype" => "Image",
        "Width" => w as i64, "Height" => h as i64,
    }
}

/// Embed an existing JPEG file without touching its data.
fn jpeg_passthrough(data: Vec<u8>, w: u32, h: u32, comps: u8) -> Option<Encoded> {
    let mut dict = image_dict(w, h);
    dict.set("BitsPerComponent", 8);
    dict.set("Filter", "DCTDecode");
    match comps {
        1 => dict.set("ColorSpace", "DeviceGray"),
        3 => dict.set("ColorSpace", "DeviceRGB"),
        4 => {
            dict.set("ColorSpace", "DeviceCMYK");
            // Adobe CMYK JPEGs are stored inverted.
            let decode: Vec<Object> = (0..4).flat_map(|_| [1.into(), 0.into()]).collect();
            dict.set("Decode", decode);
        }
        _ => return None,
    }
    let source_size = data.len();
    Some(Encoded {
        width: w,
        height: h,
        dict,
        data,
        smask: None,
        source_size,
    })
}

/// High-efficiency JPEG encoding (shared with the Compress tool):
/// optimized Huffman tables, progressive scans and, below quality 90, 4:2:0 colour
/// subsampling. Typically 15–30 % smaller than a baseline encoder at the same quality.
pub(crate) fn jpeg_encode(
    pixels: &[u8],
    w: u32,
    h: u32,
    gray: bool,
    quality: u8,
) -> Result<Vec<u8>> {
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
    let (w16, h16) = (u16::try_from(w)?, u16::try_from(h)?);
    let quality = quality.clamp(10, 100);
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, quality);
    enc.set_optimized_huffman_tables(true);
    enc.set_progressive(true);
    if !gray && quality < 90 {
        enc.set_sampling_factor(SamplingFactor::R_4_2_0);
    }
    let color = if gray {
        ColorType::Luma
    } else {
        ColorType::Rgb
    };
    enc.encode(pixels, w16, h16, color)?;
    Ok(out)
}

/// PNG-style prediction: each row is prefixed by the filter that makes it most compressible.
/// `fixed` forces one filter type for every row; `None` picks the best filter per row.
fn png_predict(raw: &[u8], row_bytes: usize, bpp: usize, fixed: Option<u8>) -> Vec<u8> {
    let rows = raw.len() / row_bytes;
    let mut out = Vec::with_capacity(raw.len() + rows);
    let zero = vec![0u8; row_bytes];
    let mut candidate = vec![0u8; row_bytes];
    let mut best = vec![0u8; row_bytes];
    for r in 0..rows {
        let row = &raw[r * row_bytes..(r + 1) * row_bytes];
        let prev = if r == 0 {
            &zero[..]
        } else {
            &raw[(r - 1) * row_bytes..r * row_bytes]
        };
        let mut best_filter = 0u8;
        let mut best_score = u64::MAX;
        let filters = match fixed {
            Some(f) => f..=f,
            None => 0..=4u8,
        };
        for filter in filters {
            for i in 0..row_bytes {
                let a = if i >= bpp { row[i - bpp] } else { 0 };
                let b = prev[i];
                let c = if i >= bpp { prev[i - bpp] } else { 0 };
                let pred = match filter {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((a as u16 + b as u16) / 2) as u8,
                    _ => {
                        let p = a as i16 + b as i16 - c as i16;
                        let (pa, pb, pc) = (
                            (p - a as i16).abs(),
                            (p - b as i16).abs(),
                            (p - c as i16).abs(),
                        );
                        if pa <= pb && pa <= pc {
                            a
                        } else if pb <= pc {
                            b
                        } else {
                            c
                        }
                    }
                };
                candidate[i] = row[i].wrapping_sub(pred);
            }
            // Minimum sum of absolute differences: the standard PNG heuristic.
            let score: u64 = candidate
                .iter()
                .map(|&v| (v as i8).unsigned_abs() as u64)
                .sum();
            if score < best_score {
                best_score = score;
                best_filter = filter;
                best.copy_from_slice(&candidate);
            }
        }
        out.push(best_filter);
        out.extend_from_slice(&best);
    }
    out
}

pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(data.len() / 2), Level::best());
    enc.write_all(data).expect("writing to memory cannot fail");
    enc.finish().expect("writing to memory cannot fail")
}

/// Compress raw samples with prediction + Deflate and fill in the stream dictionary.
fn lossless_stream(dict: &mut Dictionary, raw: &[u8], w: u32, colors: u32, bpc: u32) -> Vec<u8> {
    let row_bytes = (w * colors * bpc).div_ceil(8) as usize;
    let bpp = ((colors * bpc).div_ceil(8)).max(1) as usize;
    dict.set("BitsPerComponent", bpc as i64);
    dict.set("Filter", "FlateDecode");

    // Like PNG optimisers: try several strategies and keep the smallest result.
    // None = plain Deflate, Some(None) = adaptive per-row filters, Some(Some(f)) = one filter for all rows.
    let strategies: [Option<Option<u8>>; 4] = [None, Some(None), Some(Some(2)), Some(Some(4))];
    let (best, data) = strategies
        .par_iter()
        .map(|s| {
            let data = match s {
                None => deflate(raw),
                Some(f) => deflate(&png_predict(raw, row_bytes, bpp, *f)),
            };
            (*s, data)
        })
        .min_by_key(|(_, d)| d.len())
        .expect("strategies is not empty");
    if best.is_some() {
        dict.set(
            "DecodeParms",
            dictionary! {
                "Predictor" => 15, "Colors" => colors as i64,
                "BitsPerComponent" => bpc as i64, "Columns" => w as i64,
            },
        );
    } else {
        dict.remove(b"DecodeParms");
    }
    data
}

/// Pack 8-bit palette indices into rows of 1/2/4-bit samples (MSB first).
fn pack_indices(idx: &[u8], w: usize, bpc: u32) -> Vec<u8> {
    if bpc == 8 {
        return idx.to_vec();
    }
    let per_byte = (8 / bpc) as usize;
    let row_bytes = w.div_ceil(per_byte);
    let mut out = Vec::with_capacity(row_bytes * idx.len() / w.max(1));
    for row in idx.chunks(w) {
        for chunk in row.chunks(per_byte) {
            let mut byte = 0u8;
            for (j, &v) in chunk.iter().enumerate() {
                byte |= v << (8 - bpc as usize * (j + 1));
            }
            out.push(byte);
        }
    }
    out
}

/// Encode decoded pixels (`rgb` is 3 bytes/pixel) for the PDF.
pub(crate) fn encode_pixels(
    rgb: &[u8],
    w: u32,
    h: u32,
    quality: Quality,
) -> Result<(Dictionary, Vec<u8>)> {
    let dict = image_dict(w, h);
    let gray = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .all(|p| p[0] == p[1] && p[1] == p[2]);
    let samples: Vec<u8> = if gray {
        rgb.as_chunks::<3>().0.iter().map(|p| p[0]).collect()
    } else {
        rgb.to_vec()
    };
    let color_space = if gray { "DeviceGray" } else { "DeviceRGB" };

    if let Quality::Lossy(q) = quality {
        let mut d = dict.clone();
        d.set("ColorSpace", color_space);
        d.set("BitsPerComponent", 8);
        d.set("Filter", "DCTDecode");
        let jpeg = jpeg_encode(&samples, w, h, gray, q)?;
        // Screenshots and line art often compress better losslessly than as JPEG:
        // use the lossless result when it is smaller.
        let lossless = encode_pixels(rgb, w, h, Quality::Lossless)?;
        return Ok(if lossless.1.len() <= jpeg.len() {
            lossless
        } else {
            (d, jpeg)
        });
    }

    // Palette: screenshots, logos, diagrams and scans usually have few colours.
    let mut palette: HashMap<[u8; 3], u8> = HashMap::new();
    let mut indices = Vec::with_capacity((w * h) as usize);
    let mut fits = true;
    for p in rgb.as_chunks::<3>().0 {
        let key = [p[0], p[1], p[2]];
        let next = palette.len();
        let i = match palette.get(&key) {
            Some(&i) => i,
            None if next < 256 => {
                palette.insert(key, next as u8);
                next as u8
            }
            None => {
                fits = false;
                break;
            }
        };
        indices.push(i);
    }

    let direct = {
        let mut d = dict.clone();
        d.set("ColorSpace", color_space);
        let data = lossless_stream(&mut d, &samples, w, if gray { 1 } else { 3 }, 8);
        (d, data)
    };
    if !fits {
        return Ok(direct);
    }

    let bpc = match palette.len() {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        _ => 8,
    };
    let mut table = vec![0u8; palette.len() * 3];
    for (color, &i) in &palette {
        table[i as usize * 3..i as usize * 3 + 3].copy_from_slice(color);
    }
    let mut d = dict;
    d.set(
        "ColorSpace",
        vec![
            "Indexed".into(),
            "DeviceRGB".into(),
            (palette.len() as i64 - 1).into(),
            Object::String(table, lopdf::StringFormat::Hexadecimal),
        ],
    );
    let packed = pack_indices(&indices, w as usize, bpc);
    let data = lossless_stream(&mut d, &packed, w, 1, bpc);
    // Keep whichever is smaller (gray photos can beat a 256-colour palette).
    Ok(if data.len() < direct.1.len() {
        (d, data)
    } else {
        direct
    })
}

fn encode_file(path: &Path, opts: &ImageOptions) -> Result<Encoded> {
    let data = fs::read(path).with_context(|| format!("Could not read {}", path.display()))?;
    let source_size = data.len();

    let mut decoder = ImageReader::new(Cursor::new(&data))
        .with_guessed_format()?
        .into_decoder()
        .with_context(|| format!("Unsupported image {}", path.display()))?;
    let exif = decoder.orientation().unwrap_or(Exif::NoTransforms);
    let (dw, dh) = decoder.dimensions();
    let max = opts.max_dimension;
    let needs_resize = max > 0 && dw.max(dh) > max;

    // JPEGs: embed the original bytes unless we must rotate, resize or re-encode them.
    let is_jpeg = matches!(image::guess_format(&data), Ok(image::ImageFormat::Jpeg));
    let passthrough = if is_jpeg && exif == Exif::NoTransforms && !needs_resize {
        jpeg_info(&data).and_then(|(w, h, c)| jpeg_passthrough(data.clone(), w, h, c))
    } else {
        None
    };
    let passthrough = match (passthrough, opts.quality) {
        (Some(p), Quality::Lossless) => return Ok(p),
        (p, _) => p,
    };

    let mut img = DynamicImage::from_decoder(decoder)
        .with_context(|| format!("Could not decode {}", path.display()))?;
    img.apply_orientation(exif);
    if needs_resize {
        img = img.resize(max, max, image::imageops::FilterType::Lanczos3);
    }
    let (w, h) = (img.width(), img.height());

    // Transparency → soft mask, but only when something is actually transparent.
    let smask = if img.color().has_alpha() {
        let alpha: Vec<u8> = img.to_rgba8().pixels().map(|p| p.0[3]).collect();
        if alpha.iter().any(|&a| a != 255) {
            let mut d = image_dict(w, h);
            d.set("ColorSpace", "DeviceGray");
            let data = lossless_stream(&mut d, &alpha, w, 1, 8);
            Some((d, data))
        } else {
            None
        }
    } else {
        None
    };

    let rgb = img.to_rgb8().into_raw();
    let (dict, encoded) = encode_pixels(&rgb, w, h, opts.quality)?;
    let result = Encoded {
        width: w,
        height: h,
        dict,
        data: encoded,
        smask,
        source_size,
    };

    // A lossy re-encode that came out bigger than the original JPEG is pointless.
    match passthrough {
        Some(p) if p.data.len() <= result.data.len() => Ok(p),
        _ => Ok(result),
    }
}

// ---------------------------------------------------------------------------------------
// PDF assembly
// ---------------------------------------------------------------------------------------

pub fn images_to_pdf(
    paths: &[PathBuf],
    opts: &ImageOptions,
    output: &Path,
    pr: &Progress,
) -> Result<String> {
    if paths.is_empty() {
        bail!("Add at least one image");
    }
    let total = paths.len();
    let finished = AtomicUsize::new(0);
    pr.set(0.0, format!("Optimising {total} image(s)"));
    let encoded: Vec<Encoded> = paths
        .par_iter()
        .map(|p| {
            pr.check()?;
            let e = encode_file(p, opts)?;
            let k = finished.fetch_add(1, Ordering::Relaxed) + 1;
            pr.step(
                0.0,
                0.9,
                k,
                total,
                format!("Optimising image {k} of {total}"),
            );
            Ok(e)
        })
        .collect::<Result<_>>()?;
    pr.check()?;
    pr.set(0.92, "Building the PDF");

    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();

    for img in &encoded {
        let mut dict = img.dict.clone();
        if let Some((mask_dict, mask_data)) = &img.smask {
            let mask = Stream::new(mask_dict.clone(), mask_data.clone()).with_compression(false);
            dict.set("SMask", doc.add_object(mask));
        }
        let img_id = doc.add_object(Stream::new(dict, img.data.clone()).with_compression(false));

        // 96 DPI: 1 px = 0.75 pt
        let (iw, ih) = (img.width as f32 * 0.75, img.height as f32 * 0.75);
        let m = opts.margin.max(0.0);
        let (pw, ph) = match opts.page_size.points() {
            None => (iw + 2.0 * m, ih + 2.0 * m),
            Some((a, b)) => {
                let landscape = match opts.orientation {
                    Orientation::Auto => iw > ih,
                    Orientation::Portrait => false,
                    Orientation::Landscape => true,
                };
                if landscape { (b, a) } else { (a, b) }
            }
        };
        let scale = if opts.page_size == PageSize::FitImage {
            1.0
        } else {
            ((pw - 2.0 * m).max(1.0) / iw).min((ph - 2.0 * m).max(1.0) / ih)
        };
        let (dw, dh) = (iw * scale, ih * scale);
        let (x, y) = ((pw - dw) / 2.0, (ph - dh) / 2.0);

        let content = format!("q {dw:.3} 0 0 {dh:.3} {x:.3} {y:.3} cm /Im0 Do Q");
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)],
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => img_id } },
        });
        kids.push(Object::Reference(page));
    }

    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    doc.compress();
    pr.check()?;
    pr.set(0.96, "Saving");
    save(&mut doc, output)?;
    pr.set(1.0, "Done");

    let source: usize = encoded.iter().map(|e| e.source_size).sum();
    let images: usize = encoded.iter().map(|e| e.pdf_size()).sum();
    let out_size = fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    Ok(format!(
        "Converted {} image(s): {} of images → {} embedded, PDF {} → {}",
        paths.len(),
        human_size(source as u64),
        human_size(images as u64),
        human_size(out_size),
        output.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn png_and_jpeg_to_pdf() {
        let dir = test_dir("utilbee_img_test");
        let png = dir.join("a.png");
        let jpg = dir.join("b.jpg");
        let img =
            image::RgbaImage::from_fn(40, 30, |x, _| image::Rgba([x as u8 * 6, 100, 50, 128]));
        img.save(&png).unwrap();
        DynamicImage::ImageRgba8(img).to_rgb8().save(&jpg).unwrap();
        let out = dir.join("out.pdf");
        images_to_pdf(
            &[png, jpg.clone()],
            &ImageOptions::default(),
            &out,
            &Progress::default(),
        )
        .unwrap();
        let doc = Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
        assert_eq!(jpeg_info(&fs::read(&jpg).unwrap()), Some((40, 30, 3)));

        let lossy = ImageOptions {
            quality: Quality::Lossy(60),
            max_dimension: 20,
            ..Default::default()
        };
        images_to_pdf(
            &[dir.join("a.png")],
            &lossy,
            &dir.join("lossy.pdf"),
            &Progress::default(),
        )
        .unwrap();
        assert!(Document::load(dir.join("lossy.pdf")).is_ok());
    }

    /// Decode a lossless-encoded image back to RGB pixels and compare with the source.
    fn roundtrip(rgb: &[u8], w: u32, h: u32) -> usize {
        let (dict, data) = encode_pixels(rgb, w, h, Quality::Lossless).unwrap();
        let mut stream = Stream::new(dict.clone(), data.clone());
        let raw = stream.decompressed_content().unwrap();
        stream.set_content(raw.clone());
        let bpc = dict.get(b"BitsPerComponent").unwrap().as_i64().unwrap() as usize;
        let decoded: Vec<u8> = match dict.get(b"ColorSpace").unwrap() {
            Object::Name(n) if n == b"DeviceRGB" => raw,
            Object::Name(n) if n == b"DeviceGray" => raw.iter().flat_map(|&g| [g, g, g]).collect(),
            Object::Array(a) => {
                let Object::String(table, _) = &a[3] else {
                    panic!()
                };
                let row_bytes = (w as usize * bpc).div_ceil(8);
                let mut px = Vec::new();
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        let bit = x * bpc;
                        let byte = raw[y * row_bytes + bit / 8];
                        let i =
                            ((byte >> (8 - bpc - bit % 8)) & ((1u16 << bpc) - 1) as u8) as usize;
                        px.extend_from_slice(&table[i * 3..i * 3 + 3]);
                    }
                }
                px
            }
            _ => panic!(),
        };
        assert_eq!(decoded, rgb, "lossless roundtrip must be exact");
        data.len()
    }

    #[test]
    fn lossless_is_exact_and_small() {
        let (w, h) = (101u32, 37u32);
        // Two colours → 1-bit palette.
        let bw: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                if (i / 7) % 2 == 0 {
                    [0, 0, 0]
                } else {
                    [255, 255, 255]
                }
            })
            .collect();
        // Few colours → 4-bit palette.
        let few: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % 9) as u8 * 20, 50, 200])
            .collect();
        // Gray gradient.
        let gray: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let g = (i % w) as u8;
                [g, g, g]
            })
            .collect();
        // Many colours → direct RGB.
        let rich: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % 251) as u8, (i % 241) as u8, (i * 7 % 256) as u8])
            .collect();
        for px in [&bw, &few, &gray, &rich] {
            let size = roundtrip(px, w, h);
            assert!(size < px.len(), "compressed {size} vs raw {}", px.len());
        }
        assert!(roundtrip(&bw, w, h) < 200);
    }
}
