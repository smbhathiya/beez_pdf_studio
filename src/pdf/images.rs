//! Images → PDF. JPEGs are embedded as-is (no quality loss); other formats are stored
//! losslessly with Flate, with transparency preserved through a soft mask.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};

use super::save;

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
            PageSize::A4 => "A4 (210×297 mm)",
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

#[derive(Clone, Debug)]
pub struct ImageOptions {
    pub page_size: PageSize,
    pub orientation: Orientation,
    /// Margin in points (1/72 inch).
    pub margin: f32,
}

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

/// Add one image as an XObject; returns (object id, pixel width, pixel height).
fn add_image(doc: &mut Document, path: &Path) -> Result<(lopdf::ObjectId, u32, u32)> {
    let data = fs::read(path).with_context(|| format!("Could not read {}", path.display()))?;
    let is_jpeg = matches!(image::guess_format(&data), Ok(image::ImageFormat::Jpeg));

    if is_jpeg && let Some((w, h, comps)) = jpeg_info(&data) {
        let mut dict = dictionary! {
            "Type" => "XObject", "Subtype" => "Image",
            "Width" => w as i64, "Height" => h as i64,
            "BitsPerComponent" => 8, "Filter" => "DCTDecode",
        };
        match comps {
            1 => dict.set("ColorSpace", "DeviceGray"),
            3 => dict.set("ColorSpace", "DeviceRGB"),
            4 => {
                dict.set("ColorSpace", "DeviceCMYK");
                // Adobe CMYK JPEGs are stored inverted.
                dict.set(
                    "Decode",
                    (0..4)
                        .flat_map(|_| [Object::Integer(1), Object::Integer(0)])
                        .collect::<Vec<_>>(),
                );
            }
            _ => bail!("Unsupported JPEG in {}", path.display()),
        }
        let id = doc.add_object(Stream::new(dict, data).with_compression(false));
        return Ok((id, w, h));
    }

    let img = image::load_from_memory(&data)
        .with_context(|| format!("Unsupported image {}", path.display()))?;
    let (w, h) = (img.width(), img.height());
    let mut dict = dictionary! {
        "Type" => "XObject", "Subtype" => "Image",
        "Width" => w as i64, "Height" => h as i64, "BitsPerComponent" => 8,
    };
    let pixels = if img.color().has_color() {
        dict.set("ColorSpace", "DeviceRGB");
        img.to_rgb8().into_raw()
    } else {
        dict.set("ColorSpace", "DeviceGray");
        img.to_luma8().into_raw()
    };
    if img.color().has_alpha() {
        let alpha: Vec<u8> = img.to_rgba8().pixels().map(|p| p.0[3]).collect();
        if alpha.iter().any(|&a| a != 255) {
            let mut mask = Stream::new(
                dictionary! {
                    "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
                    "BitsPerComponent" => 8, "ColorSpace" => "DeviceGray",
                },
                alpha,
            );
            let _ = mask.compress();
            dict.set("SMask", doc.add_object(mask));
        }
    }
    let mut stream = Stream::new(dict, pixels);
    let _ = stream.compress();
    Ok((doc.add_object(stream), w, h))
}

pub fn images_to_pdf(paths: &[PathBuf], opts: &ImageOptions, output: &Path) -> Result<String> {
    if paths.is_empty() {
        bail!("Add at least one image");
    }
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();

    for path in paths {
        let (img_id, px_w, px_h) = add_image(&mut doc, path)?;
        // 96 DPI: 1 px = 0.75 pt
        let (iw, ih) = (px_w as f32 * 0.75, px_h as f32 * 0.75);
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
        let (aw, ah) = ((pw - 2.0 * m).max(1.0), (ph - 2.0 * m).max(1.0));
        let scale = (aw / iw).min(ah / ih);
        let scale = if opts.page_size == PageSize::FitImage {
            1.0
        } else {
            scale
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
    save(&mut doc, output)?;
    Ok(format!(
        "Converted {} image(s) → {}",
        paths.len(),
        output.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_and_jpeg_to_pdf() {
        let dir = std::env::temp_dir().join("utilbee_img_test");
        fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        let jpg = dir.join("b.jpg");
        let img =
            image::RgbaImage::from_fn(40, 30, |x, _| image::Rgba([x as u8 * 6, 100, 50, 128]));
        img.save(&png).unwrap();
        image::DynamicImage::ImageRgba8(img)
            .to_rgb8()
            .save(&jpg)
            .unwrap();
        let out = dir.join("out.pdf");
        let opts = ImageOptions {
            page_size: PageSize::A4,
            orientation: Orientation::Auto,
            margin: 20.0,
        };
        images_to_pdf(&[png, jpg.clone()], &opts, &out).unwrap();
        let doc = Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
        assert_eq!(jpeg_info(&fs::read(&jpg).unwrap()), Some((40, 30, 3)));
    }
}
