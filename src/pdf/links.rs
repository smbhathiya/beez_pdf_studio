//! Clickable link hotspots: list existing link annotations, remove them, add new ones that
//! open a web address or jump to another page.
//!
//! Rectangles are given in "visual" points measured from the top-left corner of the page
//! as it is displayed (rotation already applied), which is what a user expects.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};
use lopdf::{Document, Object, dictionary};

use super::{VisualFrame, decode_text_string, load, num, page_count, page_id, save};

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Url(String),
    Page(u32),
}

#[derive(Clone, Debug)]
pub struct LinkRect {
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug)]
pub struct ExistingLink {
    pub rect: LinkRect,
    /// Position inside the page's /Annots array (used to delete it).
    pub index: usize,
    pub target: String,
}

#[derive(Clone, Debug)]
pub struct NewLink {
    pub rect: LinkRect,
    pub target: LinkTarget,
}

pub struct LinkScan {
    pub links: Vec<ExistingLink>,
    /// Visual (width, height) of every page, in points.
    pub page_sizes: Vec<(f32, f32)>,
}

fn annots(doc: &Document, page_id: lopdf::ObjectId) -> Vec<Object> {
    doc.get_dictionary(page_id)
        .ok()
        .and_then(|p| p.get(b"Annots").ok())
        .and_then(|a| doc.dereference(a).ok())
        .and_then(|(_, a)| a.as_array().ok().cloned())
        .unwrap_or_default()
}

fn describe_target(doc: &Document, annot: &lopdf::Dictionary) -> String {
    let pages = doc.get_pages();
    let page_of = |o: &Object| -> Option<u32> {
        let arr = doc.dereference(o).ok()?.1.as_array().ok()?;
        let id = arr.first()?.as_reference().ok()?;
        pages.iter().find(|(_, v)| **v == id).map(|(k, _)| *k)
    };
    if let Ok(action) = annot
        .get(b"A")
        .and_then(|a| doc.dereference(a))
        .and_then(|(_, a)| a.as_dict())
    {
        match action.get(b"S").and_then(Object::as_name) {
            Ok(b"URI") => {
                if let Ok(Object::String(s, _)) = action
                    .get(b"URI")
                    .and_then(|u| doc.dereference(u))
                    .map(|x| x.1)
                {
                    return decode_text_string(s);
                }
            }
            Ok(b"GoTo") => {
                if let Some(p) = action.get(b"D").ok().and_then(page_of) {
                    return format!("→ page {p}");
                }
                return "→ named destination".into();
            }
            Ok(other) => return format!("{} action", String::from_utf8_lossy(other)),
            Err(_) => {}
        }
    }
    if let Some(p) = annot.get(b"Dest").ok().and_then(page_of) {
        return format!("→ page {p}");
    }
    "(unknown target)".into()
}

pub fn scan(path: &Path) -> Result<LinkScan> {
    let doc = load(path)?;
    let mut links = Vec::new();
    let mut page_sizes = Vec::new();
    for (page, pid) in doc.get_pages() {
        let frame = VisualFrame::of(&doc, pid);
        page_sizes.push((frame.width, frame.height));
        for (index, a) in annots(&doc, pid).iter().enumerate() {
            let Ok((_, Object::Dictionary(d))) = doc.dereference(a) else {
                continue;
            };
            if !matches!(d.get(b"Subtype").and_then(Object::as_name), Ok(b"Link")) {
                continue;
            }
            let r: Vec<f32> = d
                .get(b"Rect")
                .and_then(|r| doc.dereference(r))
                .ok()
                .and_then(|(_, r)| r.as_array().ok())
                .map(|a| a.iter().filter_map(|o| num(&doc, o)).collect())
                .unwrap_or_default();
            if r.len() != 4 {
                continue;
            }
            let (ax, ay) = frame.to_visual(r[0], r[1]);
            let (bx, by) = frame.to_visual(r[2], r[3]);
            let (x0, x1) = (ax.min(bx), ax.max(bx));
            let (y0, y1) = (ay.min(by), ay.max(by));
            links.push(ExistingLink {
                rect: LinkRect {
                    page,
                    x: x0,
                    y: frame.height - y1,
                    w: x1 - x0,
                    h: y1 - y0,
                },
                index,
                target: describe_target(&doc, d),
            });
        }
    }
    Ok(LinkScan { links, page_sizes })
}

/// Remove the listed `(page, annots index)` links and add new ones, writing `output`.
pub fn apply(
    input: &Path,
    output: &Path,
    remove: &[(u32, usize)],
    add: &[NewLink],
) -> Result<String> {
    let mut doc = load(input)?;
    let total = page_count(&doc);
    let remove: BTreeSet<_> = remove.iter().copied().collect();

    // Removals first, so indices from the scan stay valid.
    for page in remove.iter().map(|(p, _)| *p).collect::<BTreeSet<_>>() {
        let pid = page_id(&doc, page)?;
        let kept: Vec<Object> = annots(&doc, pid)
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !remove.contains(&(page, *i)))
            .map(|(_, a)| a)
            .collect();
        doc.get_dictionary_mut(pid)?.set("Annots", kept);
    }

    for link in add {
        let r = &link.rect;
        if r.page == 0 || r.page > total {
            bail!("Link page {} is outside 1-{total}", r.page);
        }
        if r.w <= 0.0 || r.h <= 0.0 {
            bail!("Link on page {} has an empty area", r.page);
        }
        let pid = page_id(&doc, r.page)?;
        let frame = VisualFrame::of(&doc, pid);
        let (ax, ay) = frame.to_user(r.x, frame.height - r.y - r.h);
        let (bx, by) = frame.to_user(r.x + r.w, frame.height - r.y);
        let rect = vec![
            Object::Real(ax.min(bx)),
            Object::Real(ay.min(by)),
            Object::Real(ax.max(bx)),
            Object::Real(ay.max(by)),
        ];
        let mut annot = dictionary! {
            "Type" => "Annot", "Subtype" => "Link", "Rect" => rect,
            "Border" => vec![0.into(), 0.into(), 0.into()], "P" => pid,
        };
        match &link.target {
            LinkTarget::Url(url) => {
                let url = url.trim();
                if url.is_empty() {
                    bail!("A link on page {} has no address", r.page);
                }
                annot.set("A", dictionary! { "S" => "URI", "URI" => Object::string_literal(url.as_bytes().to_vec()) });
            }
            LinkTarget::Page(p) => {
                let target = page_id(&doc, *p)?;
                annot.set("Dest", vec![Object::Reference(target), "Fit".into()]);
            }
        }
        let id = doc.add_object(annot);
        let mut list = annots(&doc, pid);
        list.push(Object::Reference(id));
        doc.get_dictionary_mut(pid)?.set("Annots", list);
    }

    save(&mut doc, output)?;
    Ok(format!(
        "Removed {} and added {} link(s) → {}",
        remove.len(),
        add.len(),
        output.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_remove_links() {
        let dir = std::env::temp_dir().join("utilbee_links_test");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pdf");
        let b = dir.join("b.pdf");
        let c = dir.join("c.pdf");
        crate::pdf::save(&mut crate::pdf::tests::sample(3), &a).unwrap();

        let rect = LinkRect {
            page: 1,
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 30.0,
        };
        let add = [
            NewLink {
                rect: rect.clone(),
                target: LinkTarget::Url("https://example.com".into()),
            },
            NewLink {
                rect: LinkRect {
                    page: 2,
                    ..rect.clone()
                },
                target: LinkTarget::Page(3),
            },
        ];
        apply(&a, &b, &[], &add).unwrap();
        let s = scan(&b).unwrap();
        assert_eq!(s.links.len(), 2);
        assert_eq!(s.links[0].target, "https://example.com");
        assert_eq!(s.links[1].target, "→ page 3");
        let r = &s.links[0].rect;
        assert!(
            (r.x - 10.0).abs() < 0.01 && (r.y - 20.0).abs() < 0.01 && (r.w - 100.0).abs() < 0.01
        );

        apply(&b, &c, &[(1, 0)], &[]).unwrap();
        assert_eq!(scan(&c).unwrap().links.len(), 1);
    }
}
