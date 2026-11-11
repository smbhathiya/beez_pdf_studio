//! PDF engine: everything that reads or writes PDF files lives here.
//! The UI layer only calls the high-level functions exported by the sub-modules.

pub mod assemble;
pub mod compress;
pub mod images;
pub mod links;
pub mod meta;

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use lopdf::{Document, Object, ObjectId};

/// Load a PDF from disk. Files protected with only an owner password open transparently.
pub fn load(path: &Path) -> Result<Document> {
    let doc = Document::load(path).with_context(|| format!("Could not open {}", path.display()))?;
    if doc.is_encrypted() {
        bail!(
            "{} is password protected – unlock it first with the Unlock tool",
            path.display()
        );
    }
    Ok(doc)
}

/// Save a document using a classic cross-reference table (maximum compatibility).
pub fn save(doc: &mut Document, path: &Path) -> Result<()> {
    let file =
        File::create(path).with_context(|| format!("Could not create {}", path.display()))?;
    let mut w = BufWriter::new(file);
    doc.save_to(&mut w)
        .with_context(|| format!("Could not write {}", path.display()))?;
    Ok(())
}

/// 1-based page number -> page object id.
pub fn page_id(doc: &Document, page: u32) -> Result<ObjectId> {
    doc.get_pages()
        .get(&page)
        .copied()
        .ok_or_else(|| anyhow!("Page {page} does not exist"))
}

pub fn page_count(doc: &Document) -> u32 {
    doc.get_pages().len() as u32
}

/// Numeric value of an object (follows references).
pub fn num(doc: &Document, obj: &Object) -> Option<f32> {
    match doc.dereference(obj).ok()?.1 {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// Look up a page attribute, walking up the page tree for inheritable keys.
pub fn inherited(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page_id;
    for _ in 0..64 {
        let node = doc.get_dictionary(current).ok()?;
        if let Ok(v) = node.get(key)
            && !matches!(doc.dereference(v), Ok((_, Object::Null)) | Err(_))
        {
            return Some(v.clone());
        }
        current = node.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn read_box(doc: &Document, obj: &Object) -> Option<[f32; 4]> {
    let (_, obj) = doc.dereference(obj).ok()?;
    let arr = obj.as_array().ok()?;
    if arr.len() != 4 {
        return None;
    }
    let v: Vec<f32> = arr.iter().filter_map(|o| num(doc, o)).collect();
    if v.len() != 4 {
        return None;
    }
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

/// Visible page area (CropBox, falling back to MediaBox, then A4).
pub fn page_box(doc: &Document, page_id: ObjectId) -> [f32; 4] {
    inherited(doc, page_id, b"CropBox")
        .and_then(|o| read_box(doc, &o))
        .or_else(|| inherited(doc, page_id, b"MediaBox").and_then(|o| read_box(doc, &o)))
        .unwrap_or([0.0, 0.0, 595.0, 842.0])
}

/// Page rotation normalised to 0, 90, 180 or 270.
pub fn page_rotation(doc: &Document, page_id: ObjectId) -> i64 {
    let r = inherited(doc, page_id, b"Rotate")
        .and_then(|o| num(doc, &o))
        .unwrap_or(0.0) as i64;
    (r.rem_euclid(360) / 90) * 90
}

/// The page as the reader sees it on screen: its width, height and the matrix that maps
/// "visual" coordinates (origin bottom-left of the displayed page) to PDF user space.
pub struct VisualFrame {
    pub width: f32,
    pub height: f32,
    pub matrix: [f32; 6],
}

impl VisualFrame {
    pub fn of(doc: &Document, page_id: ObjectId) -> Self {
        let [x0, y0, x1, y1] = page_box(doc, page_id);
        let (w, h) = (x1 - x0, y1 - y0);
        match page_rotation(doc, page_id) {
            90 => Self {
                width: h,
                height: w,
                matrix: [0.0, 1.0, -1.0, 0.0, x1, y0],
            },
            180 => Self {
                width: w,
                height: h,
                matrix: [-1.0, 0.0, 0.0, -1.0, x1, y1],
            },
            270 => Self {
                width: h,
                height: w,
                matrix: [0.0, -1.0, 1.0, 0.0, x0, y1],
            },
            _ => Self {
                width: w,
                height: h,
                matrix: [1.0, 0.0, 0.0, 1.0, x0, y0],
            },
        }
    }

    pub fn to_user(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.matrix;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// Inverse of [`Self::to_user`] (the matrix is a pure rotation + translation).
    pub fn to_visual(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.matrix;
        let (dx, dy) = (x - e, y - f);
        (a * dx + b * dy, c * dx + d * dy)
    }
}

/// Parse a page selection such as `1-3, 5, 8-` into ranges (1-based, inclusive).
/// An empty string selects every page.
pub fn parse_ranges(spec: &str, total: u32) -> Result<Vec<(u32, u32)>> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("all") {
        return Ok(if total == 0 { vec![] } else { vec![(1, total)] });
    }
    let mut out = Vec::new();
    for part in spec.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let parse = |s: &str, default: u32| -> Result<u32> {
            let s = s.trim();
            if s.is_empty() {
                return Ok(default);
            }
            if s.eq_ignore_ascii_case("end") || s.eq_ignore_ascii_case("last") {
                return Ok(total);
            }
            s.parse::<u32>()
                .map_err(|_| anyhow!("'{s}' is not a page number"))
        };
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (parse(a, 1)?, parse(b, total)?),
            None => {
                let n = parse(part, 0)?;
                (n, n)
            }
        };
        if a == 0 || b == 0 || a > total || b > total {
            bail!("Page range '{part}' is outside 1-{total}");
        }
        out.push((a, b));
    }
    if out.is_empty() {
        bail!("No pages selected");
    }
    Ok(out)
}

/// Flatten ranges to an ordered list of page numbers (reverse ranges like `5-1` allowed).
pub fn expand_ranges(ranges: &[(u32, u32)]) -> Vec<u32> {
    let mut v = Vec::new();
    for &(a, b) in ranges {
        if a <= b {
            v.extend(a..=b);
        } else {
            v.extend((b..=a).rev());
        }
    }
    v
}

/// Decode a PDF text string (UTF-16BE with BOM, UTF-8 with BOM, or PDFDocEncoding≈Latin-1).
pub fn decode_text_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&bytes[3..]).into_owned()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Encode a PDF text string: plain ASCII stays readable, anything else uses UTF-16BE.
pub fn encode_text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(bytes, lopdf::StringFormat::Hexadecimal)
    }
}

pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.2} MB", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    /// Build a simple n-page document in memory for tests.
    pub fn sample(n: u32) -> Document {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let res = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let mut kids = vec![];
        for i in 1..=n {
            let content = format!("BT /F1 24 Tf 72 720 Td (Page {i}) Tj ET");
            let c = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
            let p = doc.add_object(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "Contents" => c,
            });
            kids.push(Object::Reference(p));
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => kids, "Count" => n as i64,
                "Resources" => res,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            }),
        );
        let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", cat);
        doc
    }

    /// Serialize + reparse, proving the output is a valid PDF.
    pub fn roundtrip(doc: &mut Document) -> Document {
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        Document::load_mem(&buf).unwrap()
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_ranges("", 5).unwrap(), vec![(1, 5)]);
        assert_eq!(
            parse_ranges("1-2, 4, 3-", 5).unwrap(),
            vec![(1, 2), (4, 4), (3, 5)]
        );
        assert_eq!(
            expand_ranges(&parse_ranges("3-1", 5).unwrap()),
            vec![3, 2, 1]
        );
        assert!(parse_ranges("7", 5).is_err());
        assert!(parse_ranges("x", 5).is_err());
    }

    #[test]
    fn rotated_frame_maps_corners() {
        let mut doc = sample(1);
        let id = page_id(&doc, 1).unwrap();
        doc.get_dictionary_mut(id).unwrap().set("Rotate", 90);
        let f = VisualFrame::of(&doc, id);
        assert_eq!((f.width, f.height), (792.0, 612.0));
        // Rotated 90° clockwise, the displayed bottom-left is the page's user-space bottom-right.
        assert_eq!(f.to_user(0.0, 0.0), (612.0, 0.0));
        assert_eq!(f.to_user(792.0, 612.0), (0.0, 792.0));
    }

    #[test]
    fn text_strings() {
        assert_eq!(decode_text_string(b"Hello"), "Hello");
        if let Object::String(b, _) = encode_text_string("Héllo ☃") {
            assert_eq!(decode_text_string(&b), "Héllo ☃");
        } else {
            panic!()
        }
    }
}

#[cfg(test)]
mod samples {
    /// `cargo test write_samples -- --ignored` writes demo PDFs to target/samples.
    #[test]
    #[ignore]
    fn write_samples() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/samples");
        std::fs::create_dir_all(&dir).unwrap();
        super::save(&mut super::tests::sample(5), &dir.join("five.pdf")).unwrap();
        let mut d = super::tests::sample(3);
        let id = super::page_id(&d, 2).unwrap();
        d.get_dictionary_mut(id).unwrap().set("Rotate", 90);
        super::save(&mut d, &dir.join("three.pdf")).unwrap();
    }
}
