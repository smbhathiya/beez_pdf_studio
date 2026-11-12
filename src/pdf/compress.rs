//! PDF compression.
//!
//! Every pass only keeps a change when it makes the file smaller:
//!   1. **Images** (in parallel):
//!      * JPEG photos are re-encoded with an efficient encoder (optimized Huffman tables,
//!        progressive, 4:2:0 chroma) and optionally downscaled. Colour photos that are really
//!        gray are stored as grayscale.
//!      * Lossless (Flate) images are re-optimised losslessly with the same engine as
//!        Images to PDF: PNG prediction, palette detection (≤256 colours → 1/2/4/8-bit),
//!        grayscale detection and maximum Deflate. Optionally they become JPEG instead.
//!   2. **Duplicates**: identical images, fonts and other streams are stored once.
//!   3. **Junk**: page thumbnails, editor-private data (PieceInfo) and, optionally, metadata.
//!   4. **Streams**: every other stream is re-compressed with maximum-level Deflate.
//!   5. **Structure**: unused objects removed, objects packed into compressed object streams.

use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use image::{DynamicImage, GrayImage, RgbImage};
use lopdf::{Document, Object, ObjectId, SaveOptions};
use rayon::prelude::*;

use super::images::{Quality, deflate, encode_pixels, jpeg_encode};
use super::progress::Progress;
use super::{human_size, load, write_file};

#[derive(Clone, Debug)]
pub struct CompressOptions {
    /// Re-encode images (JPEG photos re-compressed, lossless images optimised).
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

/// What the passes achieved, for the result message.
#[derive(Default)]
struct Stats {
    images: usize,
    duplicates: usize,
}

pub fn compress_file(
    input: &Path,
    output: &Path,
    opts: &CompressOptions,
    pr: &Progress,
) -> Result<String> {
    let before = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    pr.set(0.0, "Reading the PDF");
    let mut doc = load(input)?;
    pr.check()?;
    let mut stats = Stats::default();
    if opts.images {
        stats.images = recompress_images(&mut doc, opts, pr)?;
    }
    pr.check()?;
    pr.set(0.62, "Removing duplicate images and fonts");
    stats.duplicates = dedupe_streams(&mut doc);
    pr.check()?;
    let bytes = compress_doc(&mut doc, opts.strip_metadata, pr)?;
    pr.check()?;
    pr.set(0.97, "Saving");
    // Never make a file bigger: if nothing helped, keep the original bytes.
    let bytes = if (bytes.len() as u64) < before || before == 0 {
        bytes
    } else {
        fs::read(input)?
    };
    write_file(output, &bytes)?;
    pr.set(1.0, "Done");
    let after = bytes.len() as u64;
    let pct = if before > 0 {
        100.0 - after as f64 * 100.0 / before as f64
    } else {
        0.0
    };
    Ok(format!(
        "{} → {} (-{:.1}%) · {} image(s) optimised, {} duplicate(s) removed → {}",
        human_size(before),
        human_size(after),
        pct.max(0.0),
        stats.images,
        stats.duplicates,
        output.display()
    ))
}

/// Junk removal, stream re-compression and structural packing; returns the file bytes.
pub fn compress_doc(doc: &mut Document, strip_metadata: bool, pr: &Progress) -> Result<Vec<u8>> {
    pr.set(0.66, "Removing unused data");
    if strip_metadata {
        doc.trailer.remove(b"Info");
    }
    for obj in doc.objects.values_mut() {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &mut s.dict,
            _ => continue,
        };
        // Page thumbnails and private editor data (Illustrator / Photoshop "PieceInfo").
        dict.remove(b"Thumb");
        dict.remove(b"PieceInfo");
        if strip_metadata {
            dict.remove(b"Metadata");
        }
    }
    doc.prune_objects();
    doc.delete_zero_length_streams();
    pr.check()?;

    pr.set(0.72, "Re-compressing streams");
    recompress_streams(doc);
    pr.check()?;
    doc.compress();

    pr.set(0.9, "Packing objects");
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

// ---------------------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Jpeg,
    Lossless,
}

struct Candidate {
    id: ObjectId,
    width: u32,
    height: u32,
    channels: u32,
    kind: Kind,
    /// JPEG bytes (Kind::Jpeg) or raw 8-bit samples (Kind::Lossless).
    data: Vec<u8>,
    /// Size of the stream as currently stored in the file.
    stored_len: usize,
}

/// The new encoding of one image.
struct Recoded {
    id: ObjectId,
    width: u32,
    height: u32,
    gray: bool,
    jpeg: bool,
    /// Dictionary entries describing the encoding (Filter, DecodeParms, ColorSpace, BPC).
    dict: lopdf::Dictionary,
    data: Vec<u8>,
}

/// Re-encode suitable image XObjects; returns how many were replaced.
fn recompress_images(doc: &mut Document, opts: &CompressOptions, pr: &Progress) -> Result<usize> {
    pr.set(0.03, "Finding images");
    let mut candidates = Vec::new();
    for (&id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        let d = &s.dict;
        if !matches!(d.get(b"Subtype").and_then(Object::as_name), Ok(b"Image")) {
            continue;
        }
        // Skip what we cannot safely re-encode: custom decode arrays, stencil masks,
        // colour-key masks.
        if d.has(b"Decode")
            || d.has(b"Mask")
            || matches!(d.get(b"ImageMask"), Ok(Object::Boolean(true)))
        {
            continue;
        }
        let num = |k: &[u8]| d.get(k).ok().and_then(|o| super::num(doc, o));
        let (w, h) = (
            num(b"Width").unwrap_or(0.0) as u32,
            num(b"Height").unwrap_or(0.0) as u32,
        );
        if w < 16 || h < 16 || w > 30_000 || h > 30_000 {
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
        let kind = match filters.as_slice() {
            [b"DCTDecode"] => Kind::Jpeg,
            [b"FlateDecode"] | [] => Kind::Lossless,
            _ => continue,
        };
        if kind == Kind::Lossless && num(b"BitsPerComponent").unwrap_or(8.0) != 8.0 {
            continue;
        }
        let data = match kind {
            Kind::Jpeg => s.content.clone(),
            // Handles PNG predictors as well.
            Kind::Lossless if filters.is_empty() => s.content.clone(),
            Kind::Lossless => match s.decompressed_content() {
                Ok(c) => c,
                Err(_) => continue,
            },
        };
        if kind == Kind::Lossless && data.len() != (w * h * channels) as usize {
            continue;
        }
        candidates.push(Candidate {
            id,
            width: w,
            height: h,
            channels,
            kind,
            data,
            stored_len: s.content.len(),
        });
    }

    let total = candidates.len();
    let finished = AtomicUsize::new(0);
    pr.set(0.05, format!("Optimising {total} image(s)"));
    let results: Vec<Recoded> = candidates
        .into_par_iter()
        .filter_map(|c| {
            if pr.is_cancelled() {
                return None;
            }
            let r = recode(c, opts);
            let k = finished.fetch_add(1, Ordering::Relaxed) + 1;
            pr.step(
                0.05,
                0.6,
                k,
                total,
                format!("Optimising image {k} of {total}"),
            );
            r
        })
        .collect();
    pr.check()?;

    let n = results.len();
    for r in results {
        let original_cs = doc
            .get_object(r.id)
            .ok()
            .and_then(|o| o.as_stream().ok())
            .and_then(|s| s.dict.get(b"ColorSpace").ok().cloned());
        if let Ok(Object::Stream(s)) = doc.get_object_mut(r.id) {
            for key in [
                &b"Filter"[..],
                b"DecodeParms",
                b"BitsPerComponent",
                b"ColorSpace",
            ] {
                s.dict.remove(key);
            }
            for (k, v) in r.dict.iter() {
                if !matches!(k.as_slice(), b"Type" | b"Subtype" | b"Width" | b"Height") {
                    s.dict.set(k.clone(), v.clone());
                }
            }
            // A colour image that stayed colour keeps its original (e.g. ICC) colour space.
            let same_model = !r.gray
                && matches!(
                    s.dict.get(b"ColorSpace").and_then(Object::as_name),
                    Ok(b"DeviceRGB")
                );
            if (same_model || r.jpeg && !r.gray)
                && let Some(cs) = &original_cs
            {
                s.dict.set("ColorSpace", cs.clone());
            }
            s.dict.set("Width", r.width as i64);
            s.dict.set("Height", r.height as i64);
            s.set_content(r.data);
            s.allows_compression = false;
        }
    }
    Ok(n)
}

/// Whether RGB pixels are (almost) gray. JPEG artefacts add a little colour noise.
fn looks_gray(rgb: &[u8], tolerance: u8) -> bool {
    rgb.as_chunks::<3>().0.iter().step_by(3).all(|p| {
        let (r, g, b) = (p[0], p[1], p[2]);
        r.abs_diff(g) <= tolerance && g.abs_diff(b) <= tolerance && r.abs_diff(b) <= tolerance
    })
}

/// Decode, optionally downscale and re-encode one image. `None` = keep the original.
fn recode(c: Candidate, opts: &CompressOptions) -> Option<Recoded> {
    let img = match c.kind {
        Kind::Jpeg => {
            let img =
                image::load_from_memory_with_format(&c.data, image::ImageFormat::Jpeg).ok()?;
            // The decoder must agree with the PDF about the colour model (no CMYK etc.).
            if img.color().channel_count() as u32 != c.channels {
                return None;
            }
            img
        }
        Kind::Lossless if c.channels == 1 => {
            DynamicImage::ImageLuma8(GrayImage::from_raw(c.width, c.height, c.data)?)
        }
        Kind::Lossless => DynamicImage::ImageRgb8(RgbImage::from_raw(c.width, c.height, c.data)?),
    };
    let max = opts.max_dimension;
    let img = if max > 0 && img.width().max(img.height()) > max {
        img.resize(max, max, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let (w, h) = (img.width(), img.height());

    let lossy = c.kind == Kind::Jpeg || opts.lossless_to_jpeg;
    let (gray, dict, data) = if lossy {
        let rgb = img.to_rgb8();
        let gray = c.channels == 1 || looks_gray(rgb.as_raw(), 6);
        let data = if gray {
            jpeg_encode(img.to_luma8().as_raw(), w, h, true, opts.jpeg_quality).ok()?
        } else {
            jpeg_encode(rgb.as_raw(), w, h, false, opts.jpeg_quality).ok()?
        };
        let mut dict = lopdf::Dictionary::new();
        dict.set("Filter", "DCTDecode");
        dict.set("BitsPerComponent", 8);
        dict.set("ColorSpace", if gray { "DeviceGray" } else { "DeviceRGB" });
        (gray, dict, data)
    } else {
        // Lossless: palette / grayscale / prediction / maximum Deflate.
        let (dict, data) = encode_pixels(img.to_rgb8().as_raw(), w, h, Quality::Lossless).ok()?;
        let gray = !matches!(
            dict.get(b"ColorSpace").and_then(Object::as_name),
            Ok(b"DeviceRGB")
        ) && !matches!(dict.get(b"ColorSpace"), Ok(Object::Array(_)));
        (gray && c.channels == 1, dict, data)
    };

    // Keep the new version only when it is clearly smaller.
    (data.len() < c.stored_len * 95 / 100).then_some(Recoded {
        id: c.id,
        width: w,
        height: h,
        gray,
        jpeg: lossy,
        dict,
        data,
    })
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

// ---------------------------------------------------------------------------------------
// Duplicates & stream re-compression
// ---------------------------------------------------------------------------------------

/// Store identical streams (images, fonts, colour profiles…) once and point every
/// reference at that copy. Returns how many duplicates were removed.
fn dedupe_streams(doc: &mut Document) -> usize {
    let mut first: HashMap<u64, Vec<ObjectId>> = HashMap::new();
    let mut replace: HashMap<ObjectId, ObjectId> = HashMap::new();
    for (&id, obj) in &doc.objects {
        let Object::Stream(s) = obj else { continue };
        let mut h = DefaultHasher::new();
        format!("{:?}", s.dict).hash(&mut h);
        s.content.hash(&mut h);
        let bucket = first.entry(h.finish()).or_default();
        // Compare for real (hash collisions are possible) before treating as a duplicate.
        let same = bucket.iter().copied().find(|other| {
            matches!(doc.objects.get(other), Some(Object::Stream(o))
                if o.content == s.content && o.dict == s.dict)
        });
        match same {
            Some(keep) => {
                replace.insert(id, keep);
            }
            None => bucket.push(id),
        }
    }
    if replace.is_empty() {
        return 0;
    }
    doc.traverse_objects(|obj| {
        if let Object::Reference(id) = obj
            && let Some(keep) = replace.get(id)
        {
            *id = *keep;
        }
    });
    for id in replace.keys() {
        doc.objects.remove(id);
    }
    replace.len()
}

/// Re-compress plain and Flate streams with maximum-level Deflate when that is smaller.
fn recompress_streams(doc: &mut Document) {
    let work: Vec<(ObjectId, Vec<u8>, usize)> = doc
        .objects
        .iter()
        .filter_map(|(&id, obj)| {
            let Object::Stream(s) = obj else { return None };
            if !s.allows_compression && s.dict.has(b"Filter") {
                return None;
            }
            let raw = match s.dict.get(b"Filter") {
                Err(_) => s.content.clone(),
                Ok(Object::Name(n)) if n == b"FlateDecode" && !s.dict.has(b"DecodeParms") => {
                    s.decompressed_content().ok()?
                }
                _ => return None,
            };
            Some((id, raw, s.content.len()))
        })
        .collect();
    let better: Vec<(ObjectId, Vec<u8>)> = work
        .into_par_iter()
        .filter_map(|(id, raw, stored)| {
            let packed = deflate(&raw);
            (packed.len() < stored).then_some((id, packed))
        })
        .collect();
    for (id, packed) in better {
        if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
            s.dict.set("Filter", "FlateDecode");
            s.dict.remove(b"DecodeParms");
            s.set_content(packed);
            s.allows_compression = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::sample;
    use lopdf::{Stream, dictionary};

    #[test]
    fn compress_produces_loadable_pdf() {
        let mut doc = sample(5);
        let bytes = compress_doc(&mut doc, true, &Progress::default()).unwrap();
        let back = Document::load_mem(&bytes).unwrap();
        assert_eq!(back.get_pages().len(), 5);
    }

    /// A raw (uncompressed) RGB image XObject attached to page 1 of `doc`.
    fn add_image(doc: &mut Document, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> ObjectId {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&f(x, y));
            }
        }
        doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64,
                "Height" => h as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
            },
            px,
        ))
    }

    #[test]
    fn lossless_images_shrink_and_duplicates_merge() {
        let mut doc = sample(2);
        // A flat-colour "screenshot" (few colours → palette) and an identical copy.
        let a = add_image(&mut doc, 200, 120, |x, _| {
            if x < 100 { [255, 0, 0] } else { [0, 0, 255] }
        });
        let b = add_image(&mut doc, 200, 120, |x, _| {
            if x < 100 { [255, 0, 0] } else { [0, 0, 255] }
        });
        // Keep both referenced from the catalog so pruning does not drop them.
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        doc.get_dictionary_mut(root).unwrap().set(
            "TestImages",
            vec![Object::Reference(a), Object::Reference(b)],
        );

        let opts = CompressOptions {
            max_dimension: 0,
            ..Default::default()
        };
        let n = recompress_images(&mut doc, &opts, &Progress::default()).unwrap();
        assert_eq!(n, 2, "both raw images re-encoded losslessly");
        let s = doc.get_object(a).unwrap().as_stream().unwrap();
        assert!(
            s.content.len() < 200 * 120 * 3 / 20,
            "palette + deflate is tiny: {}",
            s.content.len()
        );
        assert!(
            matches!(s.dict.get(b"ColorSpace"), Ok(Object::Array(_))),
            "indexed colour"
        );

        assert_eq!(dedupe_streams(&mut doc), 1, "the identical copy is merged");
        let bytes = compress_doc(&mut doc, false, &Progress::default()).unwrap();
        assert!(Document::load_mem(&bytes).is_ok());
    }

    #[test]
    fn gray_detection() {
        assert!(looks_gray(&[10, 12, 11, 200, 198, 201, 0, 0, 0], 6));
        assert!(!looks_gray(&[255, 0, 0], 6));
    }

    #[test]
    fn cancel_stops_without_writing_anything() {
        let dir = std::env::temp_dir().join("utilbee_cancel_test");
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.pdf");
        crate::pdf::save(&mut sample(3), &input).unwrap();

        // Compress: cancelled before it starts → error, no output, no leftover .part file.
        let out = dir.join("out.pdf");
        let _ = std::fs::remove_file(&out);
        let pr = Progress::default();
        pr.cancel();
        let err = compress_file(&input, &out, &CompressOptions::default(), &pr).unwrap_err();
        assert!(crate::pdf::progress::is_cancelled(&err));
        assert!(!out.exists() && !dir.join("out.pdf.part").exists());

        // Split: cancelled → no page files are left behind.
        let split_dir = dir.join("split");
        let _ = std::fs::remove_dir_all(&split_dir);
        std::fs::create_dir_all(&split_dir).unwrap();
        let err = crate::pdf::assemble::split(
            &input,
            crate::pdf::assemble::SplitMode::EveryPage,
            1,
            "",
            &split_dir,
            &pr,
        )
        .unwrap_err();
        assert!(crate::pdf::progress::is_cancelled(&err));
        assert_eq!(std::fs::read_dir(&split_dir).unwrap().count(), 0);

        // Not cancelled: progress reaches 100 %.
        let pr = Progress::default();
        compress_file(&input, &out, &CompressOptions::default(), &pr).unwrap();
        assert_eq!(pr.fraction(), Some(1.0));
        assert!(out.exists());
    }
}
