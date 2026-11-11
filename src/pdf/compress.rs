//! PDF compression: drop unused objects, Flate-compress streams, pack objects into
//! object streams and (optionally) downsample / re-encode embedded images as JPEG.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage, RgbImage, codecs::jpeg::JpegEncoder};
use lopdf::{Document, Object, ObjectId, SaveOptions};
use rayon::prelude::*;

use super::{human_size, load};

#[derive(Clone, Debug)]
pub struct CompressOptions {
    /// Re-encode images as JPEG.
    pub images: bool,
    pub jpeg_quality: u8,
    /// Downscale images whose longest side exceeds this many pixels (0 = never).
    pub max_dimension: u32,
    /// Also convert lossless (Flate) images to JPEG. Best for scans/photos, may blur line art.
    pub lossless_to_jpeg: bool,
    /// Remove document info and XMP metadata.
    pub strip_metadata: bool,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            images: true,
            jpeg_quality: 70,
            max_dimension: 2000,
            lossless_to_jpeg: false,
            strip_metadata: false,
        }
    }
}

pub fn compress_file(input: &Path, output: &Path, opts: &CompressOptions) -> Result<String> {
    let before = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let mut doc = load(input)?;
    let images = if opts.images {
        recompress_images(&mut doc, opts)
    } else {
        0
    };
    let bytes = compress_doc(&mut doc, opts.strip_metadata)?;
    fs::write(output, &bytes).with_context(|| format!("Could not write {}", output.display()))?;
    let after = bytes.len() as u64;
    let pct = if before > 0 {
        100.0 - after as f64 * 100.0 / before as f64
    } else {
        0.0
    };
    Ok(format!(
        "{} → {} ({}{:.1}%), {images} image(s) re-encoded → {}",
        human_size(before),
        human_size(after),
        if pct >= 0.0 { "-" } else { "+" },
        pct.abs(),
        output.display()
    ))
}

/// Structural compression, returns the serialized file.
pub fn compress_doc(doc: &mut Document, strip_metadata: bool) -> Result<Vec<u8>> {
    if strip_metadata {
        if let Ok(cat) = doc.catalog_mut() {
            cat.remove(b"Metadata");
        }
        doc.trailer.remove(b"Info");
    }
    doc.prune_objects();
    doc.delete_zero_length_streams();
    doc.compress();
    doc.renumber_objects();
    let mut buf = Vec::new();
    let opts = SaveOptions::builder()
        .use_object_streams(true)
        .use_xref_streams(true)
        .compression_level(9)
        .build();
    doc.save_with_options(&mut buf, opts)?;
    Ok(buf)
}

struct Candidate {
    id: ObjectId,
    width: u32,
    height: u32,
    gray: bool,
    jpeg: bool,
    data: Vec<u8>,
    /// Size of the stream as currently stored in the file.
    stored_len: usize,
}

/// Re-encode suitable image XObjects; returns how many were replaced.
fn recompress_images(doc: &mut Document, opts: &CompressOptions) -> usize {
    let mut candidates = Vec::new();
    for (&id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        let d = &s.dict;
        if !matches!(d.get(b"Subtype").and_then(Object::as_name), Ok(b"Image")) {
            continue;
        }
        if d.has(b"Decode") || matches!(d.get(b"ImageMask"), Ok(Object::Boolean(true))) {
            continue;
        }
        let w = d
            .get(b"Width")
            .ok()
            .and_then(|o| super::num(doc, o))
            .unwrap_or(0.0) as u32;
        let h = d
            .get(b"Height")
            .ok()
            .and_then(|o| super::num(doc, o))
            .unwrap_or(0.0) as u32;
        if w < 16 || h < 16 {
            continue;
        }
        let Some(channels) = color_channels(doc, d.get(b"ColorSpace").ok()) else {
            continue;
        };
        let filters: Vec<&[u8]> = match d.get(b"Filter") {
            Ok(Object::Name(n)) => vec![n.as_slice()],
            Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().ok()).collect(),
            _ => vec![],
        };
        let jpeg = match filters.as_slice() {
            [b"DCTDecode"] => true,
            [b"FlateDecode"] if opts.lossless_to_jpeg && !d.has(b"DecodeParms") => false,
            [] if opts.lossless_to_jpeg => false,
            _ => continue,
        };
        let bpc = d
            .get(b"BitsPerComponent")
            .ok()
            .and_then(|o| super::num(doc, o))
            .unwrap_or(8.0);
        if !jpeg && bpc != 8.0 {
            continue;
        }
        let data = if jpeg {
            s.content.clone()
        } else {
            match s.decompressed_content() {
                Ok(c) => c,
                Err(_) if filters.is_empty() => s.content.clone(),
                Err(_) => continue,
            }
        };
        if !jpeg && data.len() != (w * h * channels) as usize {
            continue;
        }
        candidates.push(Candidate {
            id,
            width: w,
            height: h,
            gray: channels == 1,
            jpeg,
            data,
            stored_len: s.content.len(),
        });
    }

    let q = opts.jpeg_quality.clamp(10, 100);
    let max = opts.max_dimension;
    let results: Vec<_> = candidates
        .into_par_iter()
        .filter_map(|c| {
            let img = if c.jpeg {
                let img =
                    image::load_from_memory_with_format(&c.data, image::ImageFormat::Jpeg).ok()?;
                // Make sure the decoder agrees with the PDF about the colour model.
                if (img.color().channel_count() == 1) != c.gray {
                    return None;
                }
                img
            } else if c.gray {
                DynamicImage::ImageLuma8(GrayImage::from_raw(c.width, c.height, c.data.clone())?)
            } else {
                DynamicImage::ImageRgb8(RgbImage::from_raw(c.width, c.height, c.data.clone())?)
            };
            let img = if max > 0 && img.width().max(img.height()) > max {
                img.resize(max, max, image::imageops::FilterType::Triangle)
            } else {
                img
            };
            let mut out = Vec::new();
            let mut enc = JpegEncoder::new_with_quality(Cursor::new(&mut out), q);
            if c.gray {
                enc.encode_image(&img.to_luma8()).ok()?;
            } else {
                enc.encode_image(&img.to_rgb8()).ok()?;
            }
            (out.len() < c.stored_len * 9 / 10).then_some((c.id, img.width(), img.height(), out))
        })
        .collect();

    let n = results.len();
    for (id, w, h, data) in results {
        if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
            s.dict.set("Filter", "DCTDecode");
            s.dict.remove(b"DecodeParms");
            s.dict.set("Width", w as i64);
            s.dict.set("Height", h as i64);
            s.dict.set("BitsPerComponent", 8);
            s.set_content(data);
            s.allows_compression = false;
        }
    }
    n
}

/// Number of colour channels for DeviceGray / DeviceRGB / ICCBased(N=1|3); None otherwise.
fn color_channels(doc: &Document, cs: Option<&Object>) -> Option<u32> {
    let (_, cs) = doc.dereference(cs?).ok()?;
    match cs {
        Object::Name(n) if n == b"DeviceRGB" => Some(3),
        Object::Name(n) if n == b"DeviceGray" => Some(1),
        Object::Array(a) if a.len() == 2 && a[0].as_name().ok() == Some(b"ICCBased") => {
            let (_, s) = doc.dereference(&a[1]).ok()?;
            let n = s
                .as_stream()
                .ok()?
                .dict
                .get(b"N")
                .ok()
                .and_then(|o| super::num(doc, o))? as u32;
            matches!(n, 1 | 3).then_some(n)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::sample;

    #[test]
    fn compress_produces_loadable_pdf() {
        let mut doc = sample(5);
        let bytes = compress_doc(&mut doc, true).unwrap();
        let back = Document::load_mem(&bytes).unwrap();
        assert_eq!(back.get_pages().len(), 5);
    }
}
