//! Page assembly: the single engine behind Merge, Split, Extract, Organize and Rotate.
//!
//! A new document is built from a list of `(source, page, extra rotation)` entries. Only the
//! objects reachable from the chosen pages are copied, so splitting a big PDF into many small
//! ones stays fast and every output contains just what it needs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use lopdf::{Dictionary, Document, Object, ObjectId, dictionary};

use super::progress::Progress;
use super::{expand_ranges, inherited, load, page_count, parse_ranges, save};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRef {
    /// Index into the `sources` slice.
    pub source: usize,
    /// 1-based page number in that source.
    pub page: u32,
    /// Extra clockwise rotation in degrees (multiple of 90).
    pub rotate: i64,
}

/// Build a new document out of pages taken from one or more source documents.
pub fn assemble(sources: &[Document], pages: &[PageRef]) -> Result<Document> {
    if pages.is_empty() {
        bail!("No pages to write");
    }
    let mut out = Document::with_version("1.7");
    let pages_id = out.new_object_id();

    // Per-source map from old object id to new object id.
    let mut maps: Vec<HashMap<ObjectId, ObjectId>> = vec![HashMap::new(); sources.len()];
    let page_lists: Vec<_> = sources.iter().map(|d| d.get_pages()).collect();

    // Reserve ids for the first use of every selected page, so links between
    // selected pages (bookmarked destinations, /P back-pointers) stay intact.
    let mut first_use = vec![false; pages.len()];
    for (i, r) in pages.iter().enumerate() {
        let Some(src_id) = page_lists
            .get(r.source)
            .and_then(|p| p.get(&r.page))
            .copied()
        else {
            bail!("Page {} does not exist in source {}", r.page, r.source + 1);
        };
        if let std::collections::hash_map::Entry::Vacant(e) = maps[r.source].entry(src_id) {
            e.insert(out.new_object_id());
            first_use[i] = true;
        }
    }

    let mut kids = Vec::with_capacity(pages.len());
    for (i, r) in pages.iter().enumerate() {
        let src = &sources[r.source];
        let src_id = page_lists[r.source][&r.page];
        let mut page = src.get_dictionary(src_id)?.clone();

        // Materialise attributes that may be inherited from the old page tree.
        for key in [&b"MediaBox"[..], b"CropBox", b"Resources"] {
            if let Some(v) = inherited(src, src_id, key) {
                page.set(key, v);
            }
        }
        if !page.has(b"MediaBox") {
            page.set("MediaBox", vec![0.into(), 0.into(), 595.into(), 842.into()]);
        }
        let rot = super::page_rotation(src, src_id);
        page.remove(b"Parent");

        let mut copied = copy_dict(src, &page, &mut maps[r.source], &mut out);
        copied.set("Parent", pages_id);
        copied.set("Rotate", (rot + r.rotate).rem_euclid(360));

        let new_id = if first_use[i] {
            maps[r.source][&src_id]
        } else {
            out.new_object_id()
        };
        out.objects.insert(new_id, Object::Dictionary(copied));
        kids.push(Object::Reference(new_id));
    }

    let count = kids.len() as i64;
    out.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = out.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    out.trailer.set("Root", catalog);
    Ok(out)
}

fn copy_dict(
    src: &Document,
    dict: &Dictionary,
    map: &mut HashMap<ObjectId, ObjectId>,
    out: &mut Document,
) -> Dictionary {
    let mut d = Dictionary::new();
    for (k, v) in dict.iter() {
        d.set(k.clone(), copy_obj(src, v, map, out));
    }
    d
}

fn copy_obj(
    src: &Document,
    obj: &Object,
    map: &mut HashMap<ObjectId, ObjectId>,
    out: &mut Document,
) -> Object {
    match obj {
        Object::Reference(id) => {
            if let Some(new) = map.get(id) {
                return Object::Reference(*new);
            }
            let Ok(target) = src.get_object(*id) else {
                return Object::Null;
            };
            // Never pull in pages we did not select, the old page tree or the catalog.
            if matches!(target.type_name(), Ok(b"Page" | b"Pages" | b"Catalog")) {
                return Object::Null;
            }
            let new_id = out.new_object_id();
            map.insert(*id, new_id);
            let copied = copy_obj(src, target, map, out);
            out.objects.insert(new_id, copied);
            Object::Reference(new_id)
        }
        Object::Array(a) => Object::Array(a.iter().map(|o| copy_obj(src, o, map, out)).collect()),
        Object::Dictionary(d) => Object::Dictionary(copy_dict(src, d, map, out)),
        Object::Stream(s) => {
            let mut s = s.clone();
            s.dict = copy_dict(src, &s.dict, map, out);
            Object::Stream(s)
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------------------
// High-level operations used by the UI.
// ---------------------------------------------------------------------------------------

pub struct MergeItem {
    pub path: PathBuf,
    /// Page selection like `1-3,5`; empty = all pages.
    pub ranges: String,
}

pub fn merge(items: &[MergeItem], output: &Path, pr: &Progress) -> Result<String> {
    if items.is_empty() {
        bail!("Add at least one PDF");
    }
    let mut docs = Vec::new();
    let mut refs = Vec::new();
    for (i, item) in items.iter().enumerate() {
        pr.check()?;
        pr.step(
            0.0,
            0.6,
            i,
            items.len(),
            format!("Reading file {} of {}", i + 1, items.len()),
        );
        let doc = load(&item.path)?;
        let ranges = parse_ranges(&item.ranges, page_count(&doc))?;
        refs.extend(expand_ranges(&ranges).into_iter().map(|page| PageRef {
            source: i,
            page,
            rotate: 0,
        }));
        docs.push(doc);
    }
    pr.check()?;
    pr.set(0.65, format!("Combining {} pages", refs.len()));
    let mut out = assemble(&docs, &refs)?;
    pr.check()?;
    pr.set(0.8, "Compressing");
    out.compress();
    pr.check()?;
    pr.set(0.9, "Saving");
    save(&mut out, output)?;
    pr.set(1.0, "Done");
    Ok(format!(
        "Merged {} files ({} pages) → {}",
        items.len(),
        refs.len(),
        output.display()
    ))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitMode {
    /// One file per page.
    EveryPage,
    /// One file every N pages.
    EveryN,
    /// One file per comma-separated range.
    Ranges,
    /// All selected pages into a single file.
    Extract,
}

pub fn split(
    input: &Path,
    mode: SplitMode,
    n: u32,
    spec: &str,
    out_dir: &Path,
    pr: &Progress,
) -> Result<String> {
    pr.set(0.0, "Reading the PDF");
    let doc = load(input)?;
    let total = page_count(&doc);
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or("document".into());

    let groups: Vec<Vec<u32>> = match mode {
        SplitMode::EveryPage => (1..=total).map(|p| vec![p]).collect(),
        SplitMode::EveryN => {
            let n = n.max(1);
            (1..=total)
                .collect::<Vec<_>>()
                .chunks(n as usize)
                .map(|c| c.to_vec())
                .collect()
        }
        SplitMode::Ranges => parse_ranges(spec, total)?
            .iter()
            .map(|r| expand_ranges(&[*r]))
            .collect(),
        SplitMode::Extract => vec![expand_ranges(&parse_ranges(spec, total)?)],
    };

    let sources = [doc];
    let mut written: Vec<PathBuf> = Vec::new();
    for (i, group) in groups.iter().enumerate() {
        if pr.is_cancelled() {
            // Don't leave a half-finished set of files behind.
            for f in &written {
                let _ = std::fs::remove_file(f);
            }
            pr.check()?;
        }
        pr.step(
            0.05,
            1.0,
            i,
            groups.len(),
            format!("Writing file {} of {}", i + 1, groups.len()),
        );
        let name = match (mode, group.as_slice()) {
            (SplitMode::Extract, _) => format!("{stem}_extracted.pdf"),
            (_, [only]) => format!("{stem}_page{only}.pdf"),
            (_, [first, .., last]) => format!("{stem}_pages{first}-{last}.pdf"),
            _ => format!("{stem}_part{}.pdf", i + 1),
        };
        let refs: Vec<_> = group
            .iter()
            .map(|&page| PageRef {
                source: 0,
                page,
                rotate: 0,
            })
            .collect();
        let mut out = assemble(&sources, &refs)?;
        out.compress();
        let path = out_dir.join(name);
        save(&mut out, &path)?;
        written.push(path);
    }
    pr.set(1.0, "Done");
    Ok(format!(
        "Created {} file(s) in {}",
        written.len(),
        out_dir.display()
    ))
}

/// Write a document built from an arbitrary page arrangement (Organize tool).
pub fn organize(
    paths: &[PathBuf],
    pages: &[PageRef],
    output: &Path,
    pr: &Progress,
) -> Result<String> {
    let mut docs = Vec::with_capacity(paths.len());
    for (i, p) in paths.iter().enumerate() {
        pr.check()?;
        pr.step(
            0.0,
            0.6,
            i,
            paths.len(),
            format!("Reading file {} of {}", i + 1, paths.len()),
        );
        docs.push(load(p)?);
    }
    pr.check()?;
    pr.set(0.65, format!("Arranging {} pages", pages.len()));
    let mut out = assemble(&docs, pages)?;
    pr.check()?;
    pr.set(0.8, "Compressing");
    out.compress();
    pr.check()?;
    pr.set(0.9, "Saving");
    save(&mut out, output)?;
    pr.set(1.0, "Done");
    Ok(format!(
        "Saved {} pages → {}",
        pages.len(),
        output.display()
    ))
}

/// Number of pages of a file, used by the UI when files are added.
pub fn count_pages(path: &Path) -> Result<u32> {
    Ok(page_count(&load(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::page_id;
    use crate::pdf::tests::{roundtrip, sample};

    fn text_of(doc: &Document, page: u32) -> String {
        doc.extract_text(&[page]).unwrap()
    }

    #[test]
    fn merge_reorder_rotate_duplicate() {
        let a = sample(3);
        let b = sample(2);
        let refs = [
            PageRef {
                source: 1,
                page: 2,
                rotate: 0,
            },
            PageRef {
                source: 0,
                page: 1,
                rotate: 90,
            },
            PageRef {
                source: 0,
                page: 1,
                rotate: 0,
            },
            PageRef {
                source: 0,
                page: 3,
                rotate: -90,
            },
        ];
        let mut out = assemble(&[a, b], &refs).unwrap();
        let out = roundtrip(&mut out);
        assert_eq!(page_count(&out), 4);
        assert!(text_of(&out, 1).contains("Page 2"));
        assert!(text_of(&out, 2).contains("Page 1"));
        assert!(text_of(&out, 4).contains("Page 3"));
        let rot = |p| crate::pdf::page_rotation(&out, page_id(&out, p).unwrap());
        assert_eq!((rot(1), rot(2), rot(3), rot(4)), (0, 90, 0, 270));
    }

    #[test]
    fn extract_copies_only_needed_objects() {
        let src = sample(50);
        let out = assemble(
            &[src],
            &[PageRef {
                source: 0,
                page: 7,
                rotate: 0,
            }],
        )
        .unwrap();
        // catalog + pages + page + content + resources + font
        assert!(
            out.objects.len() <= 6,
            "copied {} objects",
            out.objects.len()
        );
    }
}
