//! One state struct + `ui` method per tool. A `ui` method returns a [`Task`] when the
//! user presses the main action; the shell runs it on a background thread.

use std::path::{Path, PathBuf};

use anyhow::Result;
use eframe::egui::{
    self, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};

use super::preview::{
    self, LARGE, THUMB, Thumbs, TileStyle, ViewItem, page_grid, page_strip, pages_header,
    selected_pages,
};
use super::theme::*;
use crate::pdf::{
    self,
    assemble::{MergeItem, PageRef, SplitMode},
    compress::CompressOptions,
    images::{ImageOptions, Orientation, PageSize, Quality},
    links::{ExistingLink, LinkRect, LinkTarget, NewLink},
    meta::{Allow, DocInfo, FIELDS},
    progress::Progress,
};

pub const PDF_EXT: &[&str] = &["pdf"];
pub const IMG_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// Background work: a label for the progress bar and the job itself.
pub type Task = (String, Box<dyn FnOnce(&Progress) -> Result<String> + Send>);

fn task(
    label: impl Into<String>,
    f: impl FnOnce(&Progress) -> Result<String> + Send + 'static,
) -> Option<Task> {
    Some((label.into(), Box::new(f)))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Organize,
    Merge,
    Split,
    Compress,
    ImagesToPdf,
    ExtractText,
    Links,
    Metadata,
    Protect,
    Unlock,
}

impl Tool {
    pub const GROUPS: [(&'static str, &'static [Tool]); 4] = [
        (
            "Organize pages",
            &[Tool::Organize, Tool::Merge, Tool::Split],
        ),
        (
            "Optimize & convert",
            &[Tool::Compress, Tool::ImagesToPdf, Tool::ExtractText],
        ),
        ("Edit document", &[Tool::Links, Tool::Metadata]),
        ("Security", &[Tool::Protect, Tool::Unlock]),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Organize => "Page Organizer",
            Tool::Merge => "Merge PDFs",
            Tool::Split => "Split & Extract",
            Tool::Compress => "Compress PDF",
            Tool::ImagesToPdf => "Images to PDF",
            Tool::ExtractText => "Extract Text",
            Tool::Links => "Link Editor",
            Tool::Metadata => "Properties",
            Tool::Protect => "Protect PDF",
            Tool::Unlock => "Unlock PDF",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Organize => I_GRID,
            Tool::Merge => I_MERGE,
            Tool::Split => I_CUT,
            Tool::Compress => I_PACKAGE,
            Tool::ImagesToPdf => I_PHOTO,
            Tool::ExtractText => I_TEXT,
            Tool::Links => I_LINK,
            Tool::Metadata => I_INFO,
            Tool::Protect => I_LOCK,
            Tool::Unlock => I_UNLOCK,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Tool::Organize => {
                "Combine pages from several PDFs, then reorder, rotate, duplicate or delete them."
            }
            Tool::Merge => {
                "Join PDFs into one file. Optionally take only some pages from each file."
            }
            Tool::Split => {
                "Split into pages, chunks or ranges, or pull selected pages into a new PDF."
            }
            Tool::Compress => {
                "Shrink files: remove unused data, pack objects and downsample images."
            }
            Tool::ImagesToPdf => {
                "Turn images into a PDF with lossless optimisation, one image per page."
            }
            Tool::ExtractText => "Save the text of a PDF as a plain .txt file.",
            Tool::Links => "Add clickable links to websites or pages, and remove existing links.",
            Tool::Metadata => "See document details and edit title, author, subject and keywords.",
            Tool::Protect => {
                "Encrypt with AES-128. Set an open password and limit printing or copying."
            }
            Tool::Unlock => "Remove the password from a PDF you have the password for.",
        }
    }
}

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

fn pick_files(name: &str, ext: &[&str]) -> Vec<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(name, ext)
        .pick_files()
        .unwrap_or_default()
}

fn pick_file(name: &str, ext: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter(name, ext).pick_file()
}

/// Ask where to save; suggests `<input stem>_<suffix>.<ext>` next to the input.
fn save_as(input: Option<&Path>, suffix: &str, ext: &str) -> Option<PathBuf> {
    let stem = input
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or("output".into());
    let mut dlg = rfd::FileDialog::new()
        .add_filter(ext.to_uppercase(), &[ext])
        .set_file_name(format!("{stem}_{suffix}.{ext}"));
    if let Some(dir) = input.and_then(|p| p.parent()) {
        dlg = dlg.set_directory(dir);
    }
    dlg.save_file()
}

pub fn has_ext(p: &Path, ext: &[&str]) -> bool {
    p.extension()
        .map(|e| ext.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn file_size(p: &Path) -> String {
    std::fs::metadata(p)
        .map(|m| pdf::human_size(m.len()))
        .unwrap_or_default()
}

/// A single input PDF, shown as a card with a Browse button.
#[derive(Default)]
pub struct Source {
    pub path: Option<PathBuf>,
    pub pages: u32,
}

impl Source {
    pub fn set(&mut self, p: PathBuf) {
        self.pages = pdf::assemble::count_pages(&p).unwrap_or(0);
        self.path = Some(p);
    }

    /// Returns true when the file changed.
    fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> bool {
        if self.path.is_none() {
            let picked = drop_zone(
                ui,
                170.0,
                "Drop a PDF here or click to browse",
                "Choose the PDF file to work on",
            )
            .then(|| pick_file("PDF", PDF_EXT))
            .flatten();
            ui.add_space(4.0);
            return match picked {
                Some(p) => {
                    thumbs.invalidate(&p);
                    self.set(p);
                    true
                }
                None => false,
            };
        }
        let (name, info) = match &self.path {
            Some(p) => {
                let pages = if self.pages > 0 {
                    format!("{} pages · ", self.pages)
                } else {
                    String::new()
                };
                (file_name(p), format!("{pages}{}", file_size(p)))
            }
            None => (
                "No file selected".into(),
                "Choose a PDF, or drag one onto the window".into(),
            ),
        };
        let mut picked = None;
        setting(ui, I_DOCUMENT, &name, &info, |ui| {
            let label = if self.path.is_some() {
                "Change…"
            } else {
                "Browse…"
            };
            if ui.button(labeled(I_OPEN, label)).clicked() {
                picked = pick_file("PDF", PDF_EXT);
            }
        });
        match picked {
            Some(p) => {
                thumbs.invalidate(&p);
                self.set(p);
                true
            }
            None => false,
        }
    }

    /// "Pages" section with a thumbnail of every page; `dim` fades pages out.
    fn preview(&self, ui: &mut egui::Ui, thumbs: &mut Thumbs, dim: impl Fn(u32) -> bool) {
        if let Some(path) = &self.path
            && self.pages > 0
        {
            pages_header(ui, self.pages);
            page_grid(
                ui,
                thumbs,
                path,
                self.pages,
                None,
                |p| TileStyle {
                    dim: dim(p),
                    ..Default::default()
                },
                |_, _, _| {},
            );
        }
    }
}

fn pages_field(ui: &mut egui::Ui, value: &mut String, width: f32) {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text("All pages")
            .desired_width(width),
    );
}

/// Up / down / remove buttons for list rows. Returns true if the row was removed.
fn row_controls<T>(ui: &mut egui::Ui, list: &mut Vec<T>, i: usize) -> bool {
    let n = list.len();
    if icon_button(ui, true, I_DELETE, "Remove") {
        list.remove(i);
        return true;
    }
    if icon_button(ui, i + 1 < n, I_DOWN, "Move down") {
        list.swap(i, i + 1);
    }
    if icon_button(ui, i > 0, I_UP, "Move up") {
        list.swap(i, i - 1);
    }
    false
}

fn footer(ui: &mut egui::Ui, enabled: bool, text: &str) -> bool {
    ui.add_space(16.0);
    accent_button(ui, enabled, text)
}

// ---------------------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------------------

#[derive(Default)]
pub struct MergeState {
    items: Vec<(PathBuf, u32, String)>,
    /// Index of the file being dragged by its handle.
    drag: Option<usize>,
}

impl MergeState {
    pub fn add(&mut self, paths: Vec<PathBuf>) {
        for p in paths.into_iter().filter(|p| has_ext(p, PDF_EXT)) {
            let n = pdf::assemble::count_pages(&p).unwrap_or(0);
            self.items.push((p, n, String::new()));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        ui.horizontal(|ui| {
            if ui.button(labeled(I_ADD, "Add PDFs…")).clicked() {
                self.add(pick_files("PDF", PDF_EXT));
            }
            if command(ui, self.items.len() > 1, I_SORT, "Sort by name") {
                self.items.sort_by_key(|i| file_name(&i.0).to_lowercase());
            }
            if command(ui, !self.items.is_empty(), I_CLEAR, "Clear") {
                self.items.clear();
            }
        });
        section(ui, "Files, merged from top to bottom");
        if self.items.is_empty()
            && drop_zone(
                ui,
                170.0,
                "Drop PDF files here or click to browse",
                "Add two or more PDFs to combine them into one",
            )
        {
            self.add(pick_files("PDF", PDF_EXT));
        }
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let clip = ui.clip_rect();
        let mut card_rects: Vec<Rect> = Vec::with_capacity(self.items.len());
        let mut started = None;
        let mut i = 0;
        while i < self.items.len() {
            let top = ui.cursor().top();
            let removed = card(ui, |ui| {
                let removed = ui
                    .horizontal(|ui| {
                        let grip = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(I_GRIP)
                                        .size(16.0)
                                        .color(Palette::current(ui).text2),
                                )
                                .sense(Sense::drag()),
                            )
                            .on_hover_cursor(egui::CursorIcon::Grab)
                            .on_hover_text(
                                "Drag to change the order, or onto the red zone to remove",
                            );
                        if grip.drag_started() {
                            started = Some(i);
                        }
                        ui.label(secondary(ui, format!("{:>2}", i + 1)));
                        ui.label(RichText::new(I_DOCUMENT).size(18.0));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(file_name(&self.items[i].0));
                            caption(
                                ui,
                                &format!(
                                    "{} pages · {}",
                                    self.items[i].1,
                                    file_size(&self.items[i].0)
                                ),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let removed = row_controls(ui, &mut self.items, i);
                            if !removed {
                                ui.add_space(8.0);
                                pages_field(ui, &mut self.items[i].2, 120.0);
                                ui.label(secondary(ui, "Pages"));
                            }
                            removed
                        })
                        .inner
                    })
                    .inner;
                if !removed && self.items[i].1 > 0 {
                    let (path, n, spec) = (&self.items[i].0, self.items[i].1, &self.items[i].2);
                    let sel = selected_pages(spec, n);
                    ui.add_space(8.0);
                    page_strip(ui, thumbs, path, n, |p| {
                        sel.as_ref().is_some_and(|s| !s.contains(&p))
                    });
                }
                removed
            });
            card_rects.push(Rect::from_x_y_ranges(
                ui.max_rect().x_range(),
                top..=ui.cursor().top() - 4.0,
            ));
            if !removed {
                i += 1;
            }
        }

        if !self.items.is_empty() {
            ui.add_space(4.0);
            if drop_zone(
                ui,
                56.0,
                "Add more PDFs",
                "Drop files here or click to browse",
            ) {
                self.add(pick_files("PDF", PDF_EXT));
            }
            ui.add_space(4.0);
        }

        // --- Drag files by their handle to reorder or remove them ---------------------
        if let Some(i) = started {
            self.drag = Some(i);
        }
        if let (Some(from), Some(pos)) = (self.drag, pointer) {
            let ctx = ui.ctx().clone();
            let p = Palette::current(ui);
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(r) = card_rects.get(from) {
                ui.painter().rect_filled(*r, 8.0, p.bg.gamma_multiply(0.7));
            }
            let over_trash = preview::trash_zone(&ctx, clip, pos, "file");
            let insert = (!over_trash).then(|| {
                card_rects
                    .iter()
                    .position(|r| pos.y < r.center().y)
                    .unwrap_or(card_rects.len())
            });
            if let (Some(at), Some(last)) = (insert, card_rects.last()) {
                let (y, r) = if at < card_rects.len() {
                    (card_rects[at].top() - 3.0, card_rects[at])
                } else {
                    (last.bottom() + 3.0, *last)
                };
                preview::drag_painter(&ctx).line_segment(
                    [
                        egui::pos2(r.left() + 8.0, y),
                        egui::pos2(r.right() - 8.0, y),
                    ],
                    Stroke::new(4.0, p.accent),
                );
            }
            if let Some((path, n, _)) = self.items.get(from) {
                let (name, pages) = (file_name(path), *n);
                egui::Area::new(egui::Id::unique("merge_ghost"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(pos + Vec2::new(14.0, 10.0))
                    .interactable(false)
                    .show(&ctx, |ui| {
                        egui::Frame::new()
                            .fill(p.card)
                            .stroke(Stroke::new(2.0, p.accent))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(I_DOCUMENT).size(16.0));
                                    ui.label(strong(name));
                                    ui.label(secondary(ui, format!("{pages} pages")));
                                });
                            });
                    });
            }
            preview::drag_autoscroll(ui, clip, pos, over_trash);
            if ui.input(|i| i.pointer.any_released()) {
                if over_trash {
                    if from < self.items.len() {
                        self.items.remove(from);
                    }
                } else if let Some(at) = insert {
                    preview::move_item(&mut self.items, from, at);
                }
                self.drag = None;
            }
            ctx.request_repaint();
        } else if self.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
            self.drag = None;
        }

        caption(
            ui,
            "Page selection examples: 1-3, 5, 8-end. Leave the field empty to include every page.",
        );

        let total: u32 = self.items.iter().map(|i| i.1).sum();
        if footer(
            ui,
            !self.items.is_empty(),
            &labeled(I_SAVE, &format!("Merge {} files", self.items.len())),
        ) {
            let out = save_as(self.items.first().map(|i| i.0.as_path()), "merged", "pdf")?;
            let items: Vec<_> = self
                .items
                .iter()
                .map(|(p, _, r)| MergeItem {
                    path: p.clone(),
                    ranges: r.clone(),
                })
                .collect();
            return task(format!("Merging {total} pages"), move |pr| {
                pdf::assemble::merge(&items, &out, pr)
            });
        }
        None
    }
}

// ---------------------------------------------------------------------------------------
// Split
// ---------------------------------------------------------------------------------------

pub struct SplitState {
    pub src: Source,
    mode: SplitMode,
    every: u32,
    spec: String,
    /// Pages picked with the mouse in the preview (Ranges / Extract modes).
    sel: preview::PageSel,
    /// The page-range text the selection was last synced with.
    last_spec: String,
}

impl Default for SplitState {
    fn default() -> Self {
        Self {
            src: Source::default(),
            mode: SplitMode::EveryPage,
            every: 2,
            spec: String::new(),
            sel: Default::default(),
            last_spec: String::new(),
        }
    }
}

impl SplitState {
    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        section(ui, "Source");
        self.src.ui(ui, thumbs);
        section(ui, "Split method");
        card(ui, |ui| {
            ui.radio_value(
                &mut self.mode,
                SplitMode::EveryPage,
                "One file for every page",
            );
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.mode, SplitMode::EveryN, "One file every");
                ui.add(egui::DragValue::new(&mut self.every).range(1..=10_000));
                ui.label("pages");
            });
            ui.radio_value(
                &mut self.mode,
                SplitMode::Ranges,
                "One file for each page range",
            );
            ui.radio_value(
                &mut self.mode,
                SplitMode::Extract,
                "Extract the selected pages into one new PDF",
            );
            if matches!(self.mode, SplitMode::Ranges | SplitMode::Extract) {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Pages");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.spec)
                            .hint_text("e.g. 1-3, 4-7, 10")
                            .desired_width(260.0),
                    );
                });
                caption(
                    ui,
                    "Separate ranges with commas. 'end' means the last page.",
                );
            }
        });
        let path = self.src.path.clone()?;
        let label = if self.mode == SplitMode::Extract {
            "Extract pages"
        } else {
            "Split into folder"
        };
        if footer(ui, true, &labeled(I_FOLDER, label)) {
            let (mode, n, spec) = (self.mode, self.every, self.spec.clone());
            let mut dlg = rfd::FileDialog::new();
            if let Some(d) = path.parent() {
                dlg = dlg.set_directory(d);
            }
            let dir = dlg.pick_folder()?;
            return task("Splitting", move |pr| {
                pdf::assemble::split(&path, mode, n, &spec, &dir, pr)
            });
        }
        if let Some(path) = self.src.path.clone() {
            let total = self.src.pages;
            // Which output file each page goes to (None = not included).
            let groups: Option<Vec<Option<usize>>> = match self.mode {
                SplitMode::EveryPage => Some((0..total as usize).map(Some).collect()),
                SplitMode::EveryN => {
                    let n = self.every.max(1) as usize;
                    Some((0..total as usize).map(|i| Some(i / n)).collect())
                }
                SplitMode::Ranges => pdf::parse_ranges(&self.spec, total).ok().map(|ranges| {
                    (1..=total)
                        .map(|p| {
                            ranges
                                .iter()
                                .position(|&(a, b)| (a.min(b)..=a.max(b)).contains(&p))
                        })
                        .collect()
                }),
                SplitMode::Extract => selected_pages(&self.spec, total)
                    .map(|sel| (1..=total).map(|p| sel.contains(&p).then_some(0)).collect()),
            };
            let extract = self.mode == SplitMode::Extract;
            // In Ranges / Extract mode pages can be picked with the mouse; the selection and
            // the page-range text stay in sync both ways.
            let selecting = matches!(self.mode, SplitMode::Ranges | SplitMode::Extract);
            if selecting {
                self.sel.selected.resize(total as usize, false);
                if self.spec != self.last_spec {
                    if self.spec.trim().is_empty() {
                        self.sel.set_pages(&Default::default());
                    } else if let Some(set) = selected_pages(&self.spec, total) {
                        self.sel.set_pages(&set);
                    }
                    self.last_spec = self.spec.clone();
                }
            }
            let before = self.sel.pages();
            pages_header(ui, total);
            page_grid(
                ui,
                thumbs,
                &path,
                total,
                selecting.then_some(&mut self.sel),
                |p| match &groups {
                    None => TileStyle::default(),
                    Some(g) => match g[(p - 1) as usize] {
                        None => TileStyle {
                            dim: true,
                            ..Default::default()
                        },
                        Some(_) if extract => TileStyle {
                            highlight: true,
                            ..Default::default()
                        },
                        Some(k) => TileStyle {
                            badge: Some((
                                format!("File {}", k + 1),
                                SOURCE_COLORS[k % SOURCE_COLORS.len()],
                            )),
                            ..Default::default()
                        },
                    },
                },
                |_, _, _| {},
            );
            if selecting {
                let after = self.sel.pages();
                if after != before {
                    self.spec = preview::ranges_text(&after);
                    self.last_spec = self.spec.clone();
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------------------
// Organize
// ---------------------------------------------------------------------------------------

#[derive(Clone)]
struct Tile {
    r: PageRef,
    selected: bool,
}

pub struct OrganizeState {
    sources: Vec<PathBuf>,
    /// Visual page sizes per source.
    sizes: Vec<Vec<(f32, f32)>>,
    tiles: Vec<Tile>,
    zoom: f32,
    error: Option<String>,
    /// Tiles being dragged (indices into `tiles`), while a drag is in progress.
    drag: Option<Vec<usize>>,
    /// Rubber-band selection: start point and the selection when it started.
    marquee: Option<(egui::Pos2, Vec<bool>)>,
}

impl Default for OrganizeState {
    fn default() -> Self {
        Self {
            sources: vec![],
            sizes: vec![],
            tiles: vec![],
            zoom: 1.0,
            error: None,
            drag: None,
            marquee: None,
        }
    }
}

const SOURCE_COLORS: [Color32; 6] = [
    Color32::from_rgb(0x00, 0x78, 0xD4),
    Color32::from_rgb(0xE7, 0x48, 0x56),
    Color32::from_rgb(0x10, 0x89, 0x3E),
    Color32::from_rgb(0xCA, 0x50, 0x10),
    Color32::from_rgb(0x88, 0x17, 0x98),
    Color32::from_rgb(0x00, 0x99, 0xBC),
];

impl OrganizeState {
    pub fn add(&mut self, paths: Vec<PathBuf>) {
        for p in paths.into_iter().filter(|p| has_ext(p, PDF_EXT)) {
            match pdf::links::scan(&p) {
                Ok(scan) => {
                    let source = self.sources.len();
                    for page in 1..=scan.page_sizes.len() as u32 {
                        self.tiles.push(Tile {
                            r: PageRef {
                                source,
                                page,
                                rotate: 0,
                            },
                            selected: false,
                        });
                    }
                    self.sizes.push(scan.page_sizes);
                    self.sources.push(p);
                    self.error = None;
                }
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
    }

    fn selected(&self) -> Vec<usize> {
        self.tiles
            .iter()
            .enumerate()
            .filter(|(_, t)| t.selected)
            .map(|(i, _)| i)
            .collect()
    }

    fn move_selected(&mut self, delta: isize) {
        let sel = self.selected();
        if sel.is_empty() {
            return;
        }
        if delta < 0 && sel[0] > 0 {
            for &i in &sel {
                self.tiles.swap(i, i - 1);
            }
        } else if delta > 0 && *sel.last().unwrap() + 1 < self.tiles.len() {
            for &i in sel.iter().rev() {
                self.tiles.swap(i, i + 1);
            }
        }
    }

    fn duplicate_selected(&mut self) {
        let mut out = Vec::with_capacity(self.tiles.len() * 2);
        for t in self.tiles.drain(..) {
            let copy = t.selected.then(|| Tile {
                selected: false,
                ..t.clone()
            });
            out.push(t);
            out.extend(copy);
        }
        self.tiles = out;
    }

    fn rotate_selected(&mut self, by: i64) {
        for t in self.tiles.iter_mut().filter(|t| t.selected) {
            t.r.rotate = (t.r.rotate + by).rem_euclid(360);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        let p = Palette::current(ui);
        let any = !self.tiles.is_empty();
        let nsel = self.tiles.iter().filter(|t| t.selected).count();
        let has = nsel > 0;

        // Command bar
        egui::Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if command(ui, true, I_ADD, "Add PDFs") {
                        self.add(pick_files("PDF", PDF_EXT));
                    }
                    ui.separator();
                    if command(
                        ui,
                        any,
                        I_SELECT_ALL,
                        if any && nsel == self.tiles.len() {
                            "Select none"
                        } else {
                            "Select all"
                        },
                    ) {
                        let all = nsel < self.tiles.len();
                        self.tiles.iter_mut().for_each(|t| t.selected = all);
                    }
                    if command(ui, has, I_ROTATE_LEFT, "Rotate left") {
                        self.rotate_selected(-90);
                    }
                    if command(ui, has, I_ROTATE, "Rotate right") {
                        self.rotate_selected(90);
                    }
                    if command(ui, has, I_LEFT, "Move left") {
                        self.move_selected(-1);
                    }
                    if command(ui, has, I_RIGHT, "Move right") {
                        self.move_selected(1);
                    }
                    if command(ui, has, I_COPY, "Duplicate") {
                        self.duplicate_selected();
                    }
                    if command(ui, has, I_DELETE, "Delete") {
                        self.tiles.retain(|t| !t.selected);
                    }
                    ui.separator();
                    if command(ui, any, I_SORT, "Reverse") {
                        self.tiles.reverse();
                    }
                    if command(ui, any, I_CLEAR, "Clear") {
                        *self = Self::default();
                    }
                });
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(secondary(
                ui,
                format!("{} pages · {} selected", self.tiles.len(), nsel),
            ));
            for (i, s) in self.sources.iter().enumerate() {
                ui.add_space(8.0);
                ui.label(RichText::new("●").color(SOURCE_COLORS[i % SOURCE_COLORS.len()]));
                ui.label(secondary(ui, file_name(s)).size(12.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.zoom, 0.6..=2.0).show_value(false));
                ui.label(secondary(ui, "Thumbnail size"));
            });
        });
        if let Some(e) = &self.error {
            ui.colored_label(p.error, e);
        }
        ui.add_space(4.0);

        let mut action = None;
        let footer_h = 64.0;
        egui::Frame::new()
            .fill(if p.dark {
                Color32::from_rgb(0x1C, 0x1C, 0x1C)
            } else {
                Color32::from_rgb(0xEB, 0xEB, 0xEB)
            })
            .corner_radius(8.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height((ui.available_height() - footer_h).max(120.0))
                    .show(ui, |ui| self.grid(ui, thumbs));
            });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if accent_button(
                ui,
                any,
                &labeled(I_SAVE, &format!("Save {} pages as PDF", self.tiles.len())),
            ) && let Some(out) = save_as(
                self.sources.first().map(|p| p.as_path()),
                "organized",
                "pdf",
            ) {
                let sources = self.sources.clone();
                let pages: Vec<PageRef> = self.tiles.iter().map(|t| t.r).collect();
                action = task("Saving organized PDF", move |pr| {
                    pdf::assemble::organize(&sources, &pages, &out, pr)
                });
            }
            ui.add_space(8.0);
            caption(
                ui,
                "Drag pages to reorder · drag on empty space to select · Ctrl+A all · Del delete · Ctrl+D duplicate · Ctrl+R / Ctrl+L rotate · double-click to view",
            );
        });
        action
    }

    fn grid(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) {
        let p = Palette::current(ui);
        if self.tiles.is_empty() {
            let h = (ui.available_height() - 8.0).clamp(170.0, 420.0);
            if drop_zone(
                ui,
                h,
                "Drop PDF files here or click to browse",
                "Add one or more PDFs to arrange, rotate, duplicate or delete their pages",
            ) {
                self.add(pick_files("PDF", PDF_EXT));
            }
            return;
        }
        let nsel = self.tiles.iter().filter(|t| t.selected).count();
        let tile = Vec2::new(120.0, 164.0) * self.zoom;
        let modifiers = ui.input(|i| i.modifiers);
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let clip = ui.clip_rect();
        let mut clicked = None;
        let mut view = None;
        let mut drag_started = None;
        let mut rects = Vec::with_capacity(self.tiles.len());
        let dragging: Vec<usize> = self.drag.clone().unwrap_or_default();
        let background = ui.interact(
            clip,
            ui.make_persistent_id("organizer_background"),
            Sense::click_and_drag(),
        );

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(14.0);
            for (i, t) in self.tiles.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(tile, Sense::click_and_drag());
                rects.push(rect);
                let painter = ui.painter();
                let color = SOURCE_COLORS[t.r.source % SOURCE_COLORS.len()];
                let lifted = dragging.contains(&i);
                if t.selected {
                    painter.rect_filled(rect, 8.0, p.accent.gamma_multiply(0.18));
                    painter.rect_stroke(rect, 8.0, Stroke::new(2.0, p.accent), StrokeKind::Inside);
                } else if resp.hovered() && dragging.is_empty() {
                    painter.rect_filled(rect, 8.0, p.card_hover);
                }
                // Real page thumbnail, shown with the tile's rotation.
                let area = Rect::from_min_max(
                    rect.min + Vec2::new(12.0, 12.0),
                    rect.max - Vec2::new(12.0, 30.0),
                );
                let page = thumbs.paint_page(
                    ui,
                    &self.sources[t.r.source],
                    t.r.page,
                    THUMB,
                    t.r.rotate,
                    area,
                );
                let painter = ui.painter();
                painter.rect_filled(
                    Rect::from_min_size(page.min, Vec2::new(page.width(), 3.0)),
                    0.0,
                    color,
                );
                if t.r.rotate != 0 {
                    let label = format!("{}°", t.r.rotate);
                    let galley =
                        painter.layout_no_wrap(label, FontId::proportional(11.0), Color32::WHITE);
                    let r = Rect::from_min_size(
                        page.right_bottom() - galley.size() - Vec2::new(14.0, 8.0),
                        galley.size() + Vec2::new(10.0, 4.0),
                    );
                    painter.rect_filled(r, 4.0, p.accent.gamma_multiply(0.9));
                    painter.galley(r.min + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
                }
                if lifted {
                    // The page is "picked up": leave a faded placeholder behind.
                    painter.rect_filled(rect, 8.0, p.bg.gamma_multiply(0.75));
                }
                painter.text(
                    rect.center_bottom() - Vec2::new(0.0, 10.0),
                    Align2::CENTER_BOTTOM,
                    format!("{}", i + 1),
                    FontId::proportional(12.0),
                    if t.selected { p.accent } else { p.text2 },
                );
                if resp.drag_started() {
                    drag_started = Some(i);
                } else if resp.double_clicked() {
                    view = Some(i);
                } else if resp.clicked() {
                    clicked = Some(i);
                }
                if dragging.is_empty() {
                    resp.on_hover_text(format!(
                        "{} · page {}",
                        file_name(&self.sources[t.r.source]),
                        t.r.page
                    ))
                    .on_hover_cursor(egui::CursorIcon::Grab);
                }
            }
        });

        // --- Drag & drop ------------------------------------------------------------------
        if let Some(i) = drag_started {
            // Dragging a selected page moves the whole selection; otherwise just that page.
            let set: Vec<usize> = if self.tiles[i].selected {
                (0..self.tiles.len())
                    .filter(|&j| self.tiles[j].selected)
                    .collect()
            } else {
                self.tiles.iter_mut().for_each(|t| t.selected = false);
                self.tiles[i].selected = true;
                vec![i]
            };
            self.drag = Some(set);
        }

        if let (Some(set), Some(pos)) = (self.drag.clone(), pointer) {
            let ctx = ui.ctx().clone();
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let fg = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("organizer_drag"),
            ));

            // Delete zone along the bottom of the visible grid.
            let trash = Rect::from_center_size(
                egui::pos2(clip.center().x, clip.bottom() - 44.0),
                Vec2::new(clip.width().min(420.0), 60.0),
            );
            let over_trash = trash.contains(pos);
            let (fill, stroke) = if over_trash {
                (p.error, p.error)
            } else {
                (p.error_bg, p.error.gamma_multiply(0.7))
            };
            fg.rect(
                trash,
                10.0,
                fill,
                Stroke::new(2.0, stroke),
                StrokeKind::Inside,
            );
            fg.text(
                trash.center(),
                Align2::CENTER_CENTER,
                format!("{I_DELETE}   Drop here to delete {} page(s)", set.len()),
                FontId::new(15.0, semibold()),
                if over_trash { Color32::WHITE } else { p.error },
            );

            // Where the pages would be inserted: before/after the nearest tile.
            let insert = (!over_trash).then(|| {
                rects
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        a.1.center()
                            .distance(pos)
                            .total_cmp(&b.1.center().distance(pos))
                    })
                    .map(|(j, r)| if pos.x < r.center().x { j } else { j + 1 })
                    .unwrap_or(self.tiles.len())
            });
            if let Some(at) = insert {
                // Vertical marker in the gap where the pages will land.
                let (r, x) = if at < rects.len() {
                    (rects[at], rects[at].left() - 7.0)
                } else {
                    let last = *rects.last().unwrap();
                    (last, last.right() + 7.0)
                };
                fg.line_segment(
                    [
                        egui::pos2(x, r.top() + 6.0),
                        egui::pos2(x, r.bottom() - 6.0),
                    ],
                    Stroke::new(4.0, p.accent),
                );
            }

            // Floating preview of the dragged page under the cursor.
            let first = self.tiles[set[0]].clone();
            let ghost_size = tile * 0.8;
            egui::Area::new(egui::Id::unique("organizer_ghost"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + Vec2::new(12.0, 12.0))
                .interactable(false)
                .show(&ctx, |ui| {
                    let (r, _) = ui.allocate_exact_size(ghost_size, Sense::hover());
                    ui.painter()
                        .rect_filled(r, 8.0, p.card.gamma_multiply(0.95));
                    ui.painter().rect_stroke(
                        r,
                        8.0,
                        Stroke::new(2.0, p.accent),
                        StrokeKind::Inside,
                    );
                    thumbs.paint_page(
                        ui,
                        &self.sources[first.r.source],
                        first.r.page,
                        THUMB,
                        first.r.rotate,
                        r.shrink(10.0),
                    );
                    if set.len() > 1 {
                        let c = r.right_top() + Vec2::new(-6.0, 6.0);
                        ui.painter().circle_filled(c, 12.0, p.accent);
                        ui.painter().text(
                            c,
                            Align2::CENTER_CENTER,
                            set.len().to_string(),
                            FontId::new(12.0, semibold()),
                            p.on_accent,
                        );
                    }
                });

            // Scroll when dragging near the top or bottom edge.
            if pos.y < clip.top() + 40.0 {
                ui.scroll_with_delta(Vec2::new(0.0, 12.0));
            } else if pos.y > clip.bottom() - 90.0 && !over_trash {
                ui.scroll_with_delta(Vec2::new(0.0, -12.0));
            }

            if ui.input(|i| i.pointer.any_released()) {
                if over_trash {
                    let mut k = 0;
                    self.tiles.retain(|_| {
                        k += 1;
                        !set.contains(&(k - 1))
                    });
                } else if let Some(at) = insert {
                    self.move_tiles(&set, at);
                }
                self.drag = None;
            }
            ctx.request_repaint();
        } else if self.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
            self.drag = None;
        }

        // --- Rubber-band selection: drag on empty space -------------------------------
        if background.drag_started()
            && self.drag.is_none()
            && let Some(start) = background.interact_pointer_pos()
        {
            let base = if modifiers.command || modifiers.shift {
                self.tiles.iter().map(|t| t.selected).collect()
            } else {
                vec![false; self.tiles.len()]
            };
            self.marquee = Some((start, base));
        }
        if let Some((start, base)) = &self.marquee {
            if let Some(pos) = pointer {
                let band = Rect::from_two_pos(*start, pos);
                for (i, t) in self.tiles.iter_mut().enumerate() {
                    t.selected = base.get(i).copied().unwrap_or(false)
                        || rects.get(i).is_some_and(|r| r.intersects(band));
                }
                let fg = preview::drag_painter(ui.ctx());
                fg.rect(
                    band.intersect(clip),
                    2.0,
                    p.accent.gamma_multiply(0.12),
                    Stroke::new(1.0, p.accent),
                    StrokeKind::Inside,
                );
                ui.ctx().request_repaint();
            }
            if !ui.input(|i| i.pointer.any_down()) {
                self.marquee = None;
            }
        } else if background.clicked() && !modifiers.command && !modifiers.shift {
            // Click on empty space clears the selection.
            self.tiles.iter_mut().for_each(|t| t.selected = false);
        }

        // --- Keyboard shortcuts (not while typing in a text field) ---------------------
        if self.drag.is_none() && !ui.ctx().egui_wants_keyboard_input() {
            use egui::{Key, Modifiers};
            let (all, clear, delete, dup, rot_r, rot_l, left, right) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND, Key::A),
                    nsel > 0 && i.consume_key(Modifiers::NONE, Key::Escape),
                    i.key_pressed(Key::Delete),
                    i.consume_key(Modifiers::COMMAND, Key::D),
                    i.consume_key(Modifiers::COMMAND, Key::R),
                    i.consume_key(Modifiers::COMMAND, Key::L),
                    i.consume_key(Modifiers::COMMAND, Key::ArrowLeft),
                    i.consume_key(Modifiers::COMMAND, Key::ArrowRight),
                )
            });
            if all {
                self.tiles.iter_mut().for_each(|t| t.selected = true);
            }
            if clear {
                self.tiles.iter_mut().for_each(|t| t.selected = false);
            }
            if nsel > 0 {
                if delete {
                    self.tiles.retain(|t| !t.selected);
                }
                if dup {
                    self.duplicate_selected();
                }
                if rot_r {
                    self.rotate_selected(90);
                }
                if rot_l {
                    self.rotate_selected(-90);
                }
                if left {
                    self.move_selected(-1);
                }
                if right {
                    self.move_selected(1);
                }
            }
        }

        if let Some(i) = view {
            let items = self
                .tiles
                .iter()
                .map(|t| ViewItem {
                    path: self.sources[t.r.source].clone(),
                    page: t.r.page,
                    rotate: t.r.rotate,
                })
                .collect();
            thumbs.open_viewer(items, i);
        } else if let Some(i) = clicked {
            if modifiers.shift {
                let anchor = self.tiles.iter().position(|t| t.selected).unwrap_or(i);
                let (a, b) = (anchor.min(i), anchor.max(i));
                self.tiles
                    .iter_mut()
                    .enumerate()
                    .for_each(|(j, t)| t.selected = (a..=b).contains(&j));
            } else if modifiers.command {
                self.tiles[i].selected ^= true;
            } else {
                let only = self.tiles[i].selected && nsel == 1;
                self.tiles.iter_mut().for_each(|t| t.selected = false);
                self.tiles[i].selected = !only;
            }
        }
    }

    /// Move the tiles at `set` (sorted indices) so they land before position `at`.
    fn move_tiles(&mut self, set: &[usize], at: usize) {
        let moving: Vec<Tile> = set.iter().map(|&i| self.tiles[i].clone()).collect();
        let before = set.iter().filter(|&&i| i < at).count();
        let mut k = 0;
        self.tiles.retain(|_| {
            k += 1;
            !set.contains(&(k - 1))
        });
        let at = (at - before).min(self.tiles.len());
        self.tiles.splice(at..at, moving);
    }
}

#[cfg(test)]
mod organize_tests {
    use super::*;

    fn tiles(n: u32) -> OrganizeState {
        OrganizeState {
            tiles: (1..=n)
                .map(|page| Tile {
                    r: PageRef {
                        source: 0,
                        page,
                        rotate: 0,
                    },
                    selected: false,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn order(o: &OrganizeState) -> Vec<u32> {
        o.tiles.iter().map(|t| t.r.page).collect()
    }

    #[test]
    fn move_tiles_forward_backward_and_groups() {
        let mut o = tiles(5);
        o.move_tiles(&[0], 3); // page 1 dropped before page 4
        assert_eq!(order(&o), [2, 3, 1, 4, 5]);
        let mut o = tiles(5);
        o.move_tiles(&[4], 0); // last page to the front
        assert_eq!(order(&o), [5, 1, 2, 3, 4]);
        let mut o = tiles(5);
        o.move_tiles(&[1, 3], 5); // pages 2 and 4 to the end
        assert_eq!(order(&o), [1, 3, 5, 2, 4]);
        let mut o = tiles(5);
        o.move_tiles(&[2], 2); // dropped where it already is
        assert_eq!(order(&o), [1, 2, 3, 4, 5]);
    }
}

// ---------------------------------------------------------------------------------------
// Compress
// ---------------------------------------------------------------------------------------

#[derive(Default)]
pub struct CompressState {
    pub src: Source,
    opts: CompressOptions,
}

impl CompressState {
    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        section(ui, "Source");
        self.src.ui(ui, thumbs);

        section(ui, "Compression level");
        card(ui, |ui| {
            let o = &self.opts;
            let current = if !o.images {
                0
            } else if o.lossless_to_jpeg {
                2
            } else {
                1
            };
            ui.horizontal(|ui| {
                for (i, (name, desc)) in [
                    ("Light", "Best quality, keeps images as they are"),
                    ("Balanced", "Good quality, smaller images"),
                    ("Strong", "Smallest file, lower image quality"),
                ]
                .iter()
                .enumerate()
                {
                    let btn = egui::Button::selectable(current == i, format!("{name}\n{desc}"))
                        .min_size(Vec2::new(200.0, 52.0));
                    if ui.add(btn).clicked() {
                        self.opts = match i {
                            0 => CompressOptions {
                                images: false,
                                ..Default::default()
                            },
                            1 => CompressOptions::default(),
                            _ => CompressOptions {
                                images: true,
                                jpeg_quality: 50,
                                max_dimension: 1400,
                                lossless_to_jpeg: true,
                                strip_metadata: true,
                            },
                        };
                    }
                }
            });
        });

        section(ui, "Advanced");
        let o = &mut self.opts;
        setting(
            ui,
            I_PHOTO,
            "Re-compress images",
            "Re-encode images as JPEG",
            |ui| {
                ui.checkbox(&mut o.images, "");
            },
        );
        if o.images {
            setting(
                ui,
                "",
                "JPEG quality",
                "Lower values give smaller files",
                |ui| {
                    ui.add(egui::Slider::new(&mut o.jpeg_quality, 10..=95));
                },
            );
            setting(
                ui,
                "",
                "Maximum image size",
                "Downscale larger images (0 = keep size)",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut o.max_dimension)
                            .range(0..=10_000)
                            .suffix(" px"),
                    );
                },
            );
            setting(
                ui,
                "",
                "Convert lossless images",
                "Also turn PNG-style images into JPEG (good for scans)",
                |ui| {
                    ui.checkbox(&mut o.lossless_to_jpeg, "");
                },
            );
        }
        setting(
            ui,
            I_INFO,
            "Remove metadata",
            "Remove title, author and XMP data",
            |ui| {
                ui.checkbox(&mut o.strip_metadata, "");
            },
        );
        caption(
            ui,
            "Unused objects are always removed, and streams and the object table are always compressed.",
        );

        let path = self.src.path.clone()?;
        if footer(ui, true, &labeled(I_SAVE, "Compress & save")) {
            let out = save_as(Some(&path), "compressed", "pdf")?;
            let opts = self.opts.clone();
            return task("Compressing", move |pr| {
                pdf::compress::compress_file(&path, &out, &opts, pr)
            });
        }
        self.src.preview(ui, thumbs, |_| false);
        None
    }
}

// ---------------------------------------------------------------------------------------
// Images → PDF
// ---------------------------------------------------------------------------------------

#[derive(Default)]
pub struct ImagesState {
    images: Vec<PathBuf>,
    opts: ImageOptions,
    /// Selection / drag state of the preview grid.
    sel: preview::GridSel,
}

const MAX_SIZES: [(u32, &str); 6] = [
    (0, "Original resolution"),
    (4000, "4000 px (≈ 4K)"),
    (3000, "3000 px"),
    (2000, "2000 px (print)"),
    (1600, "1600 px"),
    (1200, "1200 px (screen)"),
];

impl ImagesState {
    pub fn add(&mut self, paths: Vec<PathBuf>) {
        self.images
            .extend(paths.into_iter().filter(|p| has_ext(p, IMG_EXT)));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        ui.horizontal(|ui| {
            if ui.button(labeled(I_ADD, "Add images…")).clicked() {
                self.add(pick_files("Images", IMG_EXT));
            }
            if command(ui, self.images.len() > 1, I_SORT, "Sort by name") {
                self.images.sort_by_key(|p| file_name(p).to_lowercase());
                self.sel = Default::default();
            }
            if command(ui, !self.images.is_empty(), I_CLEAR, "Clear") {
                self.images.clear();
            }
        });

        if self.images.is_empty() {
            section(ui, "Images");
            if drop_zone(
                ui,
                170.0,
                "Drop images here or click to browse",
                "JPG, PNG, WEBP, GIF or BMP · one image per page",
            ) {
                self.add(pick_files("Images", IMG_EXT));
            }
        } else {
            let total: u64 = self
                .images
                .iter()
                .filter_map(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .sum();
            section(ui, "Images");
            setting(
                ui,
                I_PHOTO,
                &format!("{} image(s), one per page", self.images.len()),
                &format!(
                    "{} in total · arrange them in the preview below",
                    pdf::human_size(total)
                ),
                |_| {},
            );
            if drop_zone(
                ui,
                56.0,
                "Add more images",
                "Drop files here or click to browse",
            ) {
                self.add(pick_files("Images", IMG_EXT));
            }
        }

        section(ui, "Image quality");
        let o = &mut self.opts;
        card(ui, |ui| {
            let lossless = matches!(o.quality, Quality::Lossless);
            if ui
                .radio(lossless, "Lossless: exact pixels, optimised compression")
                .clicked()
            {
                o.quality = Quality::Lossless;
            }
            caption(
                ui,
                "Uses the techniques of lossless WebP and PNG: per-row prediction, maximum Deflate, \
                 automatic palettes and grayscale detection. JPEG photos are kept unchanged.",
            );
            ui.add_space(6.0);
            if ui
                .radio(!lossless, "Smaller file: re-encode as JPEG")
                .clicked()
                && lossless
            {
                o.quality = Quality::Lossy(80);
            }
            if let Quality::Lossy(q) = &mut o.quality {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.label("Quality");
                    ui.add(egui::Slider::new(q, 30..=95));
                });
            }
        });
        setting(
            ui,
            I_PHOTO,
            "Limit resolution",
            "Downscale very large images before converting",
            |ui| {
                let label = MAX_SIZES
                    .iter()
                    .find(|m| m.0 == o.max_dimension)
                    .map(|m| m.1)
                    .unwrap_or("Custom");
                egui::ComboBox::from_id_salt("maxdim")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for (v, l) in MAX_SIZES {
                            ui.selectable_value(&mut o.max_dimension, v, l);
                        }
                    });
            },
        );

        section(ui, "Page layout");
        setting(ui, I_DOCUMENT, "Page size", "", |ui| {
            egui::ComboBox::from_id_salt("pagesize")
                .selected_text(o.page_size.label())
                .show_ui(ui, |ui| {
                    for s in PageSize::ALL {
                        ui.selectable_value(&mut o.page_size, s, s.label());
                    }
                });
        });
        if o.page_size != PageSize::FitImage {
            setting(
                ui,
                I_ROTATE,
                "Orientation",
                "Auto picks portrait or landscape to match each image",
                |ui| {
                    ui.radio_value(&mut o.orientation, Orientation::Landscape, "Landscape");
                    ui.radio_value(&mut o.orientation, Orientation::Portrait, "Portrait");
                    ui.radio_value(&mut o.orientation, Orientation::Auto, "Auto");
                },
            );
        }
        setting(ui, "", "Margin", "Space around each image", |ui| {
            ui.add(egui::Slider::new(&mut o.margin, 0.0..=72.0).suffix(" pt"));
        });

        let n = self.images.len();
        if footer(
            ui,
            n > 0,
            &labeled(I_SAVE, &format!("Create PDF from {n} image(s)")),
        ) {
            let out = save_as(self.images.first().map(|p| p.as_path()), "images", "pdf")?;
            let (imgs, opts) = (self.images.clone(), self.opts.clone());
            return task(
                format!("Optimising and converting {n} image(s)"),
                move |pr| pdf::images::images_to_pdf(&imgs, &opts, &out, pr),
            );
        }
        if !self.images.is_empty() {
            section(ui, &format!("Order of pages ({})", self.images.len()));
            preview::sortable_grid(ui, thumbs, &mut self.sel, &mut self.images, "image", |p| {
                (
                    ViewItem {
                        path: p.clone(),
                        page: 0,
                        rotate: 0,
                    },
                    file_name(p),
                    file_size(p),
                )
            });
        }
        None
    }
}

// ---------------------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------------------

struct LinkDraft {
    rect: LinkRect,
    to_page: bool,
    url: String,
    page: u32,
}

/// The link selected in the editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinkSel {
    Existing(usize),
    Draft(usize),
}

/// What the mouse is doing on the page canvas.
#[derive(Clone, Copy, Debug)]
enum CanvasDrag {
    /// Drawing a new link area from this corner (page points).
    Draw(egui::Pos2),
    /// Moving new link `i`; the vector is the pointer's offset from the box corner (points).
    Move(usize, Vec2),
}

#[derive(Default)]
pub struct LinksState {
    pub src: Source,
    sizes: Vec<(f32, f32)>,
    existing: Vec<(ExistingLink, bool)>,
    drafts: Vec<LinkDraft>,
    error: Option<String>,
    /// Page shown in the editor (1-based).
    page: u32,
    selected: Option<LinkSel>,
    drag: Option<CanvasDrag>,
    /// Put the cursor in the address field of a link that was just drawn.
    focus_url: bool,
    /// Scroll the page list to the current page (after Previous / Next).
    scroll_list: bool,
}

const LINK_RED: Color32 = Color32::from_rgb(0xE8, 0x11, 0x23);

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

impl LinksState {
    pub fn load(&mut self) {
        self.existing.clear();
        self.drafts.clear();
        self.sizes.clear();
        self.error = None;
        self.selected = None;
        self.drag = None;
        self.page = 1;
        if let Some(p) = &self.src.path {
            match pdf::links::scan(p) {
                Ok(s) => {
                    self.existing = s.links.into_iter().map(|l| (l, false)).collect();
                    self.sizes = s.page_sizes;
                }
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
    }

    fn page_size(&self, page: u32) -> (f32, f32) {
        self.sizes
            .get(page.saturating_sub(1) as usize)
            .copied()
            .unwrap_or((595.0, 842.0))
    }

    fn go_to(&mut self, page: u32) {
        if page != self.page {
            self.page = page;
            self.selected = None;
            self.drag = None;
        }
    }

    /// Topmost link under a point (page points) on the current page.
    fn hit(&self, x: f32, y: f32) -> Option<LinkSel> {
        let inside = |r: &LinkRect| {
            r.page == self.page && x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
        };
        if let Some(i) = self.drafts.iter().rposition(|d| inside(&d.rect)) {
            return Some(LinkSel::Draft(i));
        }
        self.existing
            .iter()
            .rposition(|(l, _)| inside(&l.rect))
            .map(LinkSel::Existing)
    }

    fn target_text(d: &LinkDraft) -> String {
        if d.to_page {
            format!("→ page {}", d.page)
        } else {
            d.url.trim().to_string()
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        if self.src.ui(ui, thumbs) {
            self.load();
        }
        if let Some(e) = &self.error {
            ui.colored_label(Palette::current(ui).error, e);
        }
        let path = self.src.path.clone()?;
        let pages = self.sizes.len() as u32;
        if pages == 0 {
            return None;
        }
        self.page = self.page.clamp(1, pages);

        ui.add_space(8.0);
        let height = (ui.available_height() - 4.0).max(320.0);
        let mut action = None;
        let column = egui::Layout::top_down(egui::Align::Min);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(Vec2::new(132.0, height), column, |ui| {
                self.page_list(ui, thumbs, &path, pages)
            });
            ui.add_space(6.0);
            let side_w = 330.0;
            let canvas_w = (ui.available_width() - side_w - 12.0).max(240.0);
            ui.allocate_ui_with_layout(Vec2::new(canvas_w, height), column, |ui| {
                self.canvas(ui, thumbs, &path, pages)
            });
            ui.add_space(6.0);
            ui.allocate_ui_with_layout(Vec2::new(side_w, height), column, |ui| {
                action = self.side_panel(ui, &path, pages)
            });
        });
        action
    }

    /// Left column: a thumbnail of every page with its link boxes.
    fn page_list(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs, path: &Path, pages: u32) {
        let p = Palette::current(ui);
        let height = ui.available_height();
        egui::Frame::new()
            .fill(if p.dark {
                Color32::from_rgb(0x1C, 0x1C, 0x1C)
            } else {
                Color32::from_rgb(0xEB, 0xEB, 0xEB)
            })
            .corner_radius(8.0)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.set_height(height - 12.0);
                egui::ScrollArea::vertical()
                    .id_salt("link_pages")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for n in 1..=pages {
                            let (rect, resp) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 150.0),
                                Sense::click(),
                            );
                            let current = n == self.page;
                            if current {
                                ui.painter()
                                    .rect_filled(rect, 6.0, p.accent.gamma_multiply(0.18));
                                ui.painter().rect_stroke(
                                    rect,
                                    6.0,
                                    Stroke::new(2.0, p.accent),
                                    StrokeKind::Inside,
                                );
                                if self.scroll_list {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                    self.scroll_list = false;
                                }
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, 6.0, p.card_hover);
                            }
                            let area = Rect::from_min_max(
                                rect.min + Vec2::new(8.0, 8.0),
                                rect.max - Vec2::new(8.0, 24.0),
                            );
                            let shown = thumbs.paint_page(ui, path, n, THUMB, 0, area);
                            let (pw, ph) = self.page_size(n);
                            let map = |r: &LinkRect| {
                                Rect::from_min_size(
                                    shown.min
                                        + Vec2::new(
                                            r.x / pw * shown.width(),
                                            r.y / ph * shown.height(),
                                        ),
                                    Vec2::new(r.w / pw * shown.width(), r.h / ph * shown.height()),
                                )
                            };
                            let mut count = 0;
                            for (l, removed) in
                                self.existing.iter().filter(|(l, _)| l.rect.page == n)
                            {
                                let c = if *removed {
                                    LINK_RED.gamma_multiply(0.3)
                                } else {
                                    LINK_RED
                                };
                                ui.painter().rect_stroke(
                                    map(&l.rect),
                                    0.0,
                                    Stroke::new(1.0, c),
                                    StrokeKind::Inside,
                                );
                                count += usize::from(!removed);
                            }
                            for d in self.drafts.iter().filter(|d| d.rect.page == n) {
                                ui.painter().rect(
                                    map(&d.rect),
                                    0.0,
                                    p.accent.gamma_multiply(0.3),
                                    Stroke::new(1.0, p.accent),
                                    StrokeKind::Inside,
                                );
                                count += 1;
                            }
                            ui.painter().text(
                                rect.center_bottom() - Vec2::new(0.0, 5.0),
                                Align2::CENTER_BOTTOM,
                                n.to_string(),
                                FontId::proportional(12.0),
                                if current { p.accent } else { p.text2 },
                            );
                            if count > 0 {
                                let c = rect.right_top() + Vec2::new(-14.0, 14.0);
                                ui.painter().circle_filled(c, 10.0, p.accent);
                                ui.painter().text(
                                    c,
                                    Align2::CENTER_CENTER,
                                    count.to_string(),
                                    FontId::new(11.0, semibold()),
                                    p.on_accent,
                                );
                            }
                            if resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                self.go_to(n);
                            }
                        }
                    });
            });
    }

    /// Centre: the page, large, where link areas are drawn and selected.
    fn canvas(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs, path: &Path, pages: u32) {
        let p = Palette::current(ui);
        ui.horizontal(|ui| {
            if icon_button(ui, self.page > 1, I_LEFT, "Previous page") {
                let n = self.page - 1;
                self.go_to(n);
                self.scroll_list = true;
            }
            ui.label(strong(format!("Page {} of {}", self.page, pages)));
            if icon_button(ui, self.page < pages, I_RIGHT, "Next page") {
                let n = self.page + 1;
                self.go_to(n);
                self.scroll_list = true;
            }
            ui.add_space(12.0);
            caption(ui, "Drag on the page to draw a link area · click a box to select it · drag a new box to move it");
        });
        ui.add_space(4.0);

        let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        ui.painter().rect_filled(
            area,
            8.0,
            if p.dark {
                Color32::from_rgb(0x1C, 0x1C, 0x1C)
            } else {
                Color32::from_rgb(0xEB, 0xEB, 0xEB)
            },
        );
        let shown = thumbs.paint_page(ui, path, self.page, LARGE, 0, area.shrink(18.0));
        let (pw, ph) = self.page_size(self.page);
        let to_screen = |r: &LinkRect| {
            Rect::from_min_size(
                shown.min + Vec2::new(r.x / pw * shown.width(), r.y / ph * shown.height()),
                Vec2::new(r.w / pw * shown.width(), r.h / ph * shown.height()),
            )
        };
        let to_pts = |pos: egui::Pos2| {
            egui::pos2(
                ((pos.x - shown.left()) / shown.width() * pw).clamp(0.0, pw),
                ((pos.y - shown.top()) / shown.height() * ph).clamp(0.0, ph),
            )
        };

        let resp = ui.interact(
            shown,
            ui.make_persistent_id("link_canvas"),
            Sense::click_and_drag(),
        );
        let pointer = resp.interact_pointer_pos();

        if resp.drag_started()
            && let Some(pos) = pointer
        {
            let pt = to_pts(pos);
            match self.hit(pt.x, pt.y) {
                Some(LinkSel::Draft(i)) => {
                    self.selected = Some(LinkSel::Draft(i));
                    let r = &self.drafts[i].rect;
                    self.drag = Some(CanvasDrag::Move(i, Vec2::new(pt.x - r.x, pt.y - r.y)));
                }
                _ => {
                    self.selected = None;
                    self.drag = Some(CanvasDrag::Draw(pt));
                }
            }
        }

        let mut rubber = None;
        if let (Some(d), Some(pos)) = (self.drag, pointer) {
            let pt = to_pts(pos);
            match d {
                CanvasDrag::Draw(start) => rubber = Some(Rect::from_two_pos(start, pt)),
                CanvasDrag::Move(i, grab) => {
                    if let Some(dr) = self.drafts.get_mut(i) {
                        dr.rect.x = (pt.x - grab.x).clamp(0.0, (pw - dr.rect.w).max(0.0));
                        dr.rect.y = (pt.y - grab.y).clamp(0.0, (ph - dr.rect.h).max(0.0));
                    }
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(r) = rubber
                && r.width() >= 4.0
                && r.height() >= 4.0
            {
                self.drafts.push(LinkDraft {
                    rect: LinkRect {
                        page: self.page,
                        x: r.min.x,
                        y: r.min.y,
                        w: r.width(),
                        h: r.height(),
                    },
                    to_page: false,
                    url: "https://".into(),
                    page: 1,
                });
                self.selected = Some(LinkSel::Draft(self.drafts.len() - 1));
                self.focus_url = true;
            }
            self.drag = None;
        }
        if resp.clicked() {
            self.selected = pointer.and_then(|pos| {
                let pt = to_pts(pos);
                self.hit(pt.x, pt.y)
            });
        }
        if let Some(pos) = resp.hover_pos() {
            let pt = to_pts(pos);
            let icon = match self.hit(pt.x, pt.y) {
                Some(LinkSel::Draft(_)) => egui::CursorIcon::Move,
                Some(LinkSel::Existing(_)) => egui::CursorIcon::PointingHand,
                None => egui::CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        // Link boxes.
        let painter = ui.painter().with_clip_rect(area);
        for (i, (l, removed)) in self
            .existing
            .iter()
            .enumerate()
            .filter(|(_, (l, _))| l.rect.page == self.page)
        {
            let r = to_screen(&l.rect);
            let selected = self.selected == Some(LinkSel::Existing(i));
            let c = if *removed {
                LINK_RED.gamma_multiply(0.35)
            } else {
                LINK_RED
            };
            painter.rect(
                r,
                2.0,
                c.gamma_multiply(0.12),
                Stroke::new(if selected { 3.0 } else { 1.5 }, c),
                StrokeKind::Outside,
            );
            if *removed {
                painter.line_segment([r.left_top(), r.right_bottom()], Stroke::new(1.5, c));
                painter.line_segment([r.right_top(), r.left_bottom()], Stroke::new(1.5, c));
            }
        }
        for (i, d) in self
            .drafts
            .iter()
            .enumerate()
            .filter(|(_, d)| d.rect.page == self.page)
        {
            let r = to_screen(&d.rect);
            let selected = self.selected == Some(LinkSel::Draft(i));
            painter.rect(
                r,
                2.0,
                p.accent.gamma_multiply(0.22),
                Stroke::new(if selected { 3.0 } else { 1.5 }, p.accent),
                StrokeKind::Outside,
            );
            if selected {
                let label = shorten(&Self::target_text(d), 40);
                let galley = painter.layout_no_wrap(
                    format!("{I_LINK}  {label}"),
                    FontId::proportional(12.0),
                    p.on_accent,
                );
                let tag = Rect::from_min_size(
                    r.left_top() - Vec2::new(0.0, galley.size().y + 8.0),
                    galley.size() + Vec2::new(12.0, 6.0),
                );
                painter.rect_filled(tag, 4.0, p.accent);
                painter.galley(tag.min + Vec2::new(6.0, 3.0), galley, p.on_accent);
            }
        }
        if let Some(r) = rubber {
            let r = to_screen(&LinkRect {
                page: self.page,
                x: r.min.x,
                y: r.min.y,
                w: r.width(),
                h: r.height(),
            });
            painter.rect(
                r,
                2.0,
                p.accent.gamma_multiply(0.15),
                Stroke::new(2.0, p.accent),
                StrokeKind::Outside,
            );
        }
    }

    /// Right column: the selected link, the links on this page and Save.
    fn side_panel(&mut self, ui: &mut egui::Ui, path: &Path, pages: u32) -> Option<Task> {
        let p = Palette::current(ui);
        let height = ui.available_height();
        let mut action = None;
        egui::Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(8.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(height - 34.0);
                egui::ScrollArea::vertical().id_salt("link_side").auto_shrink([false, false]).show(ui, |ui| {
                    // --- Selected link ---
                    match self.selected {
                        Some(LinkSel::Draft(i)) if i < self.drafts.len() => {
                            ui.label(strong(format!("{I_LINK}  New link")).size(15.0));
                            ui.add_space(6.0);
                            let (pw, ph) = self.page_size(self.drafts[i].rect.page);
                            let d = &mut self.drafts[i];
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut d.to_page, false, "Web address");
                                ui.radio_value(&mut d.to_page, true, "Go to page");
                            });
                            if d.to_page {
                                ui.horizontal(|ui| {
                                    ui.label("Page");
                                    ui.add(egui::DragValue::new(&mut d.page).range(1..=pages));
                                });
                            } else {
                                let edit = ui.add(
                                    egui::TextEdit::singleline(&mut d.url)
                                        .hint_text("https://example.com")
                                        .desired_width(f32::INFINITY),
                                );
                                if self.focus_url {
                                    edit.request_focus();
                                    self.focus_url = false;
                                }
                            }
                            ui.add_space(8.0);
                            caption(ui, "Position on the page (points, from the top-left):");
                            egui::Grid::new("link_pos").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
                                ui.label("x");
                                ui.add(egui::DragValue::new(&mut d.rect.x).range(0.0..=pw).speed(1.0));
                                ui.label("y");
                                ui.add(egui::DragValue::new(&mut d.rect.y).range(0.0..=ph).speed(1.0));
                                ui.end_row();
                                ui.label("width");
                                ui.add(egui::DragValue::new(&mut d.rect.w).range(1.0..=pw).speed(1.0));
                                ui.label("height");
                                ui.add(egui::DragValue::new(&mut d.rect.h).range(1.0..=ph).speed(1.0));
                                ui.end_row();
                            });
                            ui.add_space(8.0);
                            let mut delete = false;
                            ui.horizontal(|ui| {
                                if ui.button("Whole page").clicked() {
                                    d.rect = LinkRect { page: d.rect.page, x: 0.0, y: 0.0, w: pw, h: ph };
                                }
                                delete = ui.button(labeled(I_DELETE, "Delete")).clicked();
                            });
                            if delete {
                                self.drafts.remove(i);
                                self.selected = None;
                            }
                        }
                        Some(LinkSel::Existing(i)) if i < self.existing.len() => {
                            ui.label(strong(format!("{I_LINK}  Existing link")).size(15.0));
                            ui.add_space(6.0);
                            let (l, removed) = &mut self.existing[i];
                            ui.add(egui::Label::new(l.target.clone()).wrap());
                            caption(ui, &format!("x {:.0}, y {:.0} · {:.0} × {:.0} pt", l.rect.x, l.rect.y, l.rect.w, l.rect.h));
                            ui.add_space(8.0);
                            let text = if *removed { labeled(I_ADD, "Keep this link") } else { labeled(I_DELETE, "Remove this link") };
                            if ui.button(text).clicked() {
                                *removed = !*removed;
                            }
                            if *removed {
                                ui.colored_label(p.error, "Will be removed when you save.");
                            }
                        }
                        _ => {
                            ui.label(strong("Add a link").size(15.0));
                            ui.add_space(4.0);
                            ui.add(
                                egui::Label::new(secondary(
                                    ui,
                                    "Drag on the page to draw the clickable area, then type the web address \
                                     or choose a page to jump to. Click any box to select it.",
                                ))
                                .wrap(),
                            );
                        }
                    }

                    // --- Links on this page ---
                    ui.add_space(12.0);
                    ui.separator();
                    ui.label(strong(format!("Links on page {}", self.page)));
                    let mut clicked = None;
                    let mut any = false;
                    for (i, (l, removed)) in self.existing.iter().enumerate().filter(|(_, (l, _))| l.rect.page == self.page) {
                        any = true;
                        let text = RichText::new(format!("●  {}", shorten(&l.target, 34)))
                            .color(if *removed { p.text2 } else { p.text });
                        let text = if *removed { text.strikethrough() } else { text };
                        if ui.selectable_label(self.selected == Some(LinkSel::Existing(i)), text).clicked() {
                            clicked = Some(LinkSel::Existing(i));
                        }
                    }
                    for (i, d) in self.drafts.iter().enumerate().filter(|(_, d)| d.rect.page == self.page) {
                        any = true;
                        let text = RichText::new(format!("+  {}", shorten(&Self::target_text(d), 34))).color(p.accent);
                        if ui.selectable_label(self.selected == Some(LinkSel::Draft(i)), text).clicked() {
                            clicked = Some(LinkSel::Draft(i));
                        }
                    }
                    if !any {
                        caption(ui, "No links on this page yet.");
                    }
                    if clicked.is_some() {
                        self.selected = clicked;
                    }
                });
            });

        // --- Save ---
        let removing: Vec<(u32, usize)> = self
            .existing
            .iter()
            .filter(|(_, r)| *r)
            .map(|(l, _)| (l.rect.page, l.index))
            .collect();
        let changes = removing.len() + self.drafts.len();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let label = labeled(
                I_SAVE,
                &format!(
                    "Save ({} new, {} removed)",
                    self.drafts.len(),
                    removing.len()
                ),
            );
            if accent_button(ui, changes > 0, &label)
                && let Some(out) = save_as(Some(path), "links", "pdf")
            {
                let add: Vec<NewLink> = self
                    .drafts
                    .iter()
                    .map(|d| NewLink {
                        rect: d.rect.clone(),
                        target: if d.to_page {
                            LinkTarget::Page(d.page)
                        } else {
                            LinkTarget::Url(d.url.clone())
                        },
                    })
                    .collect();
                let path = path.to_path_buf();
                action = task("Saving links", move |pr| {
                    pdf::links::apply(&path, &out, &removing, &add, pr)
                });
            }
        });
        action
    }
}

// ---------------------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------------------

#[derive(Default)]
pub struct MetaState {
    pub src: Source,
    info: Option<DocInfo>,
    fields: Vec<String>,
    error: Option<String>,
}

impl MetaState {
    pub fn load(&mut self) {
        self.error = None;
        self.info = None;
        if let Some(p) = &self.src.path {
            match pdf::meta::read_info(p) {
                Ok(info) => {
                    self.fields = info.fields.clone();
                    self.info = Some(info);
                }
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        section(ui, "Source");
        if self.src.ui(ui, thumbs) {
            self.load();
        }
        if let Some(e) = &self.error {
            ui.colored_label(Palette::current(ui).error, e);
        }
        let info = self.info.clone()?;

        section(ui, "Document");
        card(ui, |ui| {
            egui::Grid::new("docinfo")
                .num_columns(2)
                .spacing([40.0, 8.0])
                .show(ui, |ui| {
                    for (k, v) in [
                        ("Pages", info.pages.to_string()),
                        ("PDF version", info.version.clone()),
                        ("File size", info.file_size.clone()),
                        ("First page", info.first_page_size.clone()),
                        ("Objects", info.objects.to_string()),
                        (
                            "Encrypted",
                            if info.was_encrypted {
                                "Yes (owner password only)".into()
                            } else {
                                "No".into()
                            },
                        ),
                    ] {
                        ui.label(secondary(ui, k));
                        ui.label(v);
                        ui.end_row();
                    }
                });
        });

        section(ui, "Description");
        card(ui, |ui| {
            egui::Grid::new("meta")
                .num_columns(2)
                .spacing([40.0, 10.0])
                .show(ui, |ui| {
                    for (k, v) in FIELDS.iter().zip(self.fields.iter_mut()) {
                        ui.label(*k);
                        ui.add(egui::TextEdit::singleline(v).desired_width(460.0));
                        ui.end_row();
                    }
                });
        });
        caption(ui, "Leave a field empty to remove it.");

        let path = self.src.path.clone()?;
        if footer(
            ui,
            self.fields != info.fields,
            &labeled(I_SAVE, "Save properties"),
        ) {
            let out = save_as(Some(&path), "edited", "pdf")?;
            let fields = self.fields.clone();
            return task("Saving properties", move |pr| {
                pdf::meta::write_info(&path, &out, &fields, pr)
            });
        }
        self.src.preview(ui, thumbs, |_| false);
        None
    }
}

// ---------------------------------------------------------------------------------------
// Extract text
// ---------------------------------------------------------------------------------------

#[derive(Default)]
pub struct TextState {
    pub src: Source,
    range: String,
}

impl TextState {
    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        section(ui, "Source");
        self.src.ui(ui, thumbs);
        section(ui, "Options");
        setting(ui, I_DOCUMENT, "Pages", "Examples: 1-3, 5, 8-end", |ui| {
            pages_field(ui, &mut self.range, 180.0);
        });
        caption(
            ui,
            "Scanned PDFs contain images rather than text, so no text can be extracted from them.",
        );
        let path = self.src.path.clone()?;
        if footer(ui, true, &labeled(I_SAVE, "Extract to .txt")) {
            let out = save_as(Some(&path), "text", "txt")?;
            let range = self.range.clone();
            return task("Extracting text", move |pr| {
                pdf::meta::extract_text(&path, &out, &range, pr)
            });
        }
        let sel = selected_pages(&self.range, self.src.pages);
        self.src.preview(ui, thumbs, |p| {
            sel.as_ref().is_some_and(|s| !s.contains(&p))
        });
        None
    }
}

// ---------------------------------------------------------------------------------------
// Protect / Unlock
// ---------------------------------------------------------------------------------------

#[derive(Default)]
pub struct ProtectState {
    pub src: Source,
    user: String,
    confirm: String,
    owner: String,
    allow: Allow,
}

impl ProtectState {
    pub fn ui(&mut self, ui: &mut egui::Ui, thumbs: &mut Thumbs) -> Option<Task> {
        section(ui, "Source");
        self.src.ui(ui, thumbs);

        section(ui, "Passwords");
        card(ui, |ui| {
            egui::Grid::new("pw")
                .num_columns(2)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Open password");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.user)
                            .password(true)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                    ui.label("Confirm password");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.confirm)
                            .password(true)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                    ui.label("Owner password");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.owner)
                            .password(true)
                            .hint_text("Optional")
                            .desired_width(280.0),
                    );
                    ui.end_row();
                });
            if self.user != self.confirm {
                ui.colored_label(Palette::current(ui).error, "The passwords do not match.");
            }
            caption(
                ui,
                "The open password is needed to view the file. The owner password removes the restrictions below.",
            );
        });

        section(ui, "Permissions");
        let a = &mut self.allow;
        for (flag, name, desc) in [
            (&mut a.print, "Printing", "Allow the document to be printed"),
            (&mut a.copy, "Copying", "Allow text and images to be copied"),
            (&mut a.modify, "Editing", "Allow changes to the content"),
            (
                &mut a.annotate,
                "Comments",
                "Allow adding or changing annotations",
            ),
            (
                &mut a.fill_forms,
                "Form filling",
                "Allow filling in form fields",
            ),
            (
                &mut a.assemble,
                "Page assembly",
                "Allow inserting, rotating or deleting pages",
            ),
        ] {
            setting(ui, "", name, desc, |ui| {
                ui.checkbox(flag, "");
            });
        }

        let path = self.src.path.clone()?;
        let ok = self.user == self.confirm && !(self.user.is_empty() && self.owner.is_empty());
        if footer(ui, ok, &labeled(I_LOCK, "Protect & save")) {
            let out = save_as(Some(&path), "protected", "pdf")?;
            let (u, o, a) = (self.user.clone(), self.owner.clone(), self.allow);
            return task("Encrypting", move |pr| {
                pdf::meta::protect(&path, &out, &u, &o, a, pr)
            });
        }
        self.src.preview(ui, thumbs, |_| false);
        None
    }
}

#[derive(Default)]
pub struct UnlockState {
    pub path: Option<PathBuf>,
    password: String,
}

impl UnlockState {
    pub fn ui(&mut self, ui: &mut egui::Ui, _thumbs: &mut Thumbs) -> Option<Task> {
        section(ui, "Source");
        if self.path.is_none() {
            if drop_zone(
                ui,
                170.0,
                "Drop a protected PDF here or click to browse",
                "The file you want to remove the password from",
            ) && let Some(p) = pick_file("PDF", PDF_EXT)
            {
                self.path = Some(p);
            }
            return None;
        }
        let (name, info) = match &self.path {
            Some(p) => (file_name(p), file_size(p)),
            None => ("No file selected".into(), "Choose a protected PDF".into()),
        };
        setting(ui, I_LOCK, &name, &info, |ui| {
            if ui.button(labeled(I_OPEN, "Browse…")).clicked()
                && let Some(p) = pick_file("PDF", PDF_EXT)
            {
                self.path = Some(p);
            }
        });
        section(ui, "Password");
        setting(
            ui,
            "",
            "Password",
            "Leave empty if the file only restricts printing or copying",
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.password)
                        .password(true)
                        .desired_width(260.0),
                );
            },
        );
        let path = self.path.clone()?;
        if footer(ui, true, &labeled(I_UNLOCK, "Unlock & save")) {
            let out = save_as(Some(&path), "unlocked", "pdf")?;
            let pw = self.password.clone();
            return task("Unlocking", move |pr| {
                pdf::meta::unlock(&path, &out, &pw, pr)
            });
        }
        None
    }
}
