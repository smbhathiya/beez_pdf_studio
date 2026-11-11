//! Stamping text onto pages: watermarks and page numbers.

use std::path::Path;

use anyhow::Result;
use lopdf::{Document, Object, ObjectId, dictionary};

use super::{
    VisualFrame, add_resource, expand_ranges, helvetica_width, load, overlay, parse_ranges,
    pdf_literal, save, win_ansi,
};

const FONT_NAME: &str = "FUtilBee";
const GS_NAME: &str = "GSUtilBee";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Position {
    TopLeft,
    TopCenter,
    TopRight,
    Center,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Position {
    pub const ALL: [Position; 7] = [
        Position::TopLeft,
        Position::TopCenter,
        Position::TopRight,
        Position::Center,
        Position::BottomLeft,
        Position::BottomCenter,
        Position::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Position::TopLeft => "Top left",
            Position::TopCenter => "Top center",
            Position::TopRight => "Top right",
            Position::Center => "Center",
            Position::BottomLeft => "Bottom left",
            Position::BottomCenter => "Bottom center",
            Position::BottomRight => "Bottom right",
        }
    }

    /// Anchor point (centre of the text) on a page of size w×h with a margin.
    fn anchor(self, w: f32, h: f32, margin: f32, text_w: f32, size: f32) -> (f32, f32) {
        let left = margin + text_w / 2.0;
        let right = w - margin - text_w / 2.0;
        let top = h - margin - size / 2.0;
        let bottom = margin + size / 2.0;
        match self {
            Position::TopLeft => (left, top),
            Position::TopCenter => (w / 2.0, top),
            Position::TopRight => (right, top),
            Position::Center => (w / 2.0, h / 2.0),
            Position::BottomLeft => (left, bottom),
            Position::BottomCenter => (w / 2.0, bottom),
            Position::BottomRight => (right, bottom),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextStamp {
    pub size: f32,
    /// RGB 0..1
    pub color: [f32; 3],
    /// 0..1
    pub opacity: f32,
    /// Degrees counter-clockwise.
    pub angle: f32,
    pub position: Position,
    pub margin: f32,
    pub bold: bool,
    /// Paint behind the page content instead of over it.
    pub under: bool,
}

/// Shared font + transparency objects, created once per document.
fn stamp_resources(doc: &mut Document, s: &TextStamp) -> (ObjectId, ObjectId) {
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1",
        "BaseFont" => if s.bold { "Helvetica-Bold" } else { "Helvetica" },
        "Encoding" => "WinAnsiEncoding",
    });
    let gs = doc.add_object(dictionary! {
        "Type" => "ExtGState", "ca" => Object::Real(s.opacity), "CA" => Object::Real(s.opacity),
    });
    (font, gs)
}

fn stamp_page(
    doc: &mut Document,
    res: (ObjectId, ObjectId),
    page: u32,
    text: &str,
    s: &TextStamp,
) -> Result<()> {
    let (font, gs) = res;
    let page_id = super::page_id(doc, page)?;
    add_resource(doc, page_id, "Font", FONT_NAME, Object::Reference(font))?;
    add_resource(doc, page_id, "ExtGState", GS_NAME, Object::Reference(gs))?;

    let frame = VisualFrame::of(doc, page_id);
    let bytes = win_ansi(text);
    let width_factor = if s.bold { 1.06 } else { 1.0 };
    let tw = helvetica_width(&bytes) / 1000.0 * s.size * width_factor;

    // Rotated text needs a bounding box for corner positioning; centre is exact.
    let (sin, cos) = s.angle.to_radians().sin_cos();
    let bw = (tw * cos).abs() + (s.size * sin).abs();
    let bh = (tw * sin).abs() + (s.size * cos).abs();
    let (cx, cy) = s
        .position
        .anchor(frame.width, frame.height, s.margin, bw, bh);

    let [r, g, b] = s.color;
    let ops = format!(
        "/{GS_NAME} gs\n{r} {g} {b} rg\n{cm}1 0 0 1 {cx} {cy} cm\n{cos} {sin} {nsin} {cos} 0 0 cm\nBT /{FONT_NAME} {size} Tf {tx} {ty} Td {lit} Tj ET",
        cm = frame.cm(),
        nsin = -sin,
        size = s.size,
        tx = -tw / 2.0,
        ty = -s.size * 0.35,
        lit = pdf_literal(&bytes),
    );
    overlay(doc, page_id, &ops, s.under)
}

pub fn watermark(
    input: &Path,
    output: &Path,
    text: &str,
    pages: &str,
    style: &TextStamp,
) -> Result<String> {
    let mut doc = load(input)?;
    let selected = expand_ranges(&parse_ranges(pages, super::page_count(&doc))?);
    let res = stamp_resources(&mut doc, style);
    for &p in &selected {
        stamp_page(&mut doc, res, p, text, style)?;
    }
    doc.compress();
    save(&mut doc, output)?;
    Ok(format!(
        "Watermarked {} page(s) → {}",
        selected.len(),
        output.display()
    ))
}

/// `format` may contain `{n}` (page number) and `{total}`.
pub fn page_numbers(
    input: &Path,
    output: &Path,
    format: &str,
    start_at: i64,
    skip_first: u32,
    style: &TextStamp,
) -> Result<String> {
    let mut doc = load(input)?;
    let count = super::page_count(&doc);
    let numbered = count.saturating_sub(skip_first);
    let total = start_at + numbered as i64 - 1;
    let res = stamp_resources(&mut doc, style);
    for (i, page) in ((skip_first + 1)..=count).enumerate() {
        let label = format
            .replace("{n}", &(start_at + i as i64).to_string())
            .replace("{total}", &total.to_string());
        stamp_page(&mut doc, res, page, &label, style)?;
    }
    doc.compress();
    save(&mut doc, output)?;
    Ok(format!(
        "Numbered {numbered} page(s) → {}",
        output.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::tests::{roundtrip, sample};

    #[test]
    fn stamp_adds_text() {
        let mut doc = sample(2);
        let style = TextStamp {
            size: 40.0,
            color: [1.0, 0.0, 0.0],
            opacity: 0.3,
            angle: 45.0,
            position: Position::Center,
            margin: 20.0,
            bold: true,
            under: false,
        };
        let res = stamp_resources(&mut doc, &style);
        stamp_page(&mut doc, res, 2, "CONFIDENTIAL", &style).unwrap();
        let back = roundtrip(&mut doc);
        let text = back.extract_text(&[2]).unwrap();
        assert!(
            text.contains("Page 2") && text.contains("CONFIDENTIAL"),
            "{text}"
        );
        assert!(!back.extract_text(&[1]).unwrap().contains("CONFIDENTIAL"));
    }
}
