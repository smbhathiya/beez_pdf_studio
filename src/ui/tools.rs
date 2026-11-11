//! One state struct + `ui` method per tool. A `ui` method returns a [`Task`] when the
//! user presses the main action; the shell runs it on a background thread.

use std::path::{Path, PathBuf};

use anyhow::Result;
use eframe::egui::{self, Align2, Color32, FontId, RichText, Sense, Stroke, StrokeKind, Vec2};

use super::theme::*;
use crate::pdf::{
    self,
    assemble::{MergeItem, PageRef, SplitMode},
    compress::CompressOptions,
    images::{ImageOptions, Orientation, PageSize, Quality},
    links::{ExistingLink, LinkRect, LinkTarget, NewLink},
    meta::{Allow, DocInfo, FIELDS},
};

pub const PDF_EXT: &[&str] = &["pdf"];
pub const IMG_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// Background work: a label for the progress bar and the job itself.
pub type Task = (String, Box<dyn FnOnce() -> Result<String> + Send>);

fn task(
    label: impl Into<String>,
    f: impl FnOnce() -> Result<String> + Send + 'static,
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
    fn ui(&mut self, ui: &mut egui::Ui) -> bool {
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
                self.set(p);
                true
            }
            None => false,
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
}

impl MergeState {
    pub fn add(&mut self, paths: Vec<PathBuf>) {
        for p in paths.into_iter().filter(|p| has_ext(p, PDF_EXT)) {
            let n = pdf::assemble::count_pages(&p).unwrap_or(0);
            self.items.push((p, n, String::new()));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
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
        if self.items.is_empty() {
            card(ui, |ui| {
                ui.label(secondary(
                    ui,
                    "Add two or more PDFs, or drag them onto the window.",
                ));
            });
        }
        let mut i = 0;
        while i < self.items.len() {
            let removed = card(ui, |ui| {
                ui.horizontal(|ui| {
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
                .inner
            });
            if !removed {
                i += 1;
            }
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
            return task(format!("Merging {total} pages"), move || {
                pdf::assemble::merge(&items, &out)
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
}

impl Default for SplitState {
    fn default() -> Self {
        Self {
            src: Source::default(),
            mode: SplitMode::EveryPage,
            every: 2,
            spec: String::new(),
        }
    }
}

impl SplitState {
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source");
        self.src.ui(ui);
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
            return task("Splitting", move || {
                pdf::assemble::split(&path, mode, n, &spec, &dir)
            });
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
}

impl Default for OrganizeState {
    fn default() -> Self {
        Self {
            sources: vec![],
            sizes: vec![],
            tiles: vec![],
            zoom: 1.0,
            error: None,
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

    fn rotate_selected(&mut self, by: i64) {
        for t in self.tiles.iter_mut().filter(|t| t.selected) {
            t.r.rotate = (t.r.rotate + by).rem_euclid(360);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
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
                        let mut out = Vec::with_capacity(self.tiles.len() + nsel);
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
                    .show(ui, |ui| self.grid(ui));
            });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if accent_button(
                ui,
                any,
                &labeled(I_SAVE, &format!("Save {} pages as PDF", self.tiles.len())),
            )
                && let Some(out) = save_as(
                    self.sources.first().map(|p| p.as_path()),
                    "organized",
                    "pdf",
                ) {
                    let sources = self.sources.clone();
                    let pages: Vec<PageRef> = self.tiles.iter().map(|t| t.r).collect();
                    action = task("Saving organized PDF", move || {
                        pdf::assemble::organize(&sources, &pages, &out)
                    });
                }
            ui.add_space(8.0);
            caption(
                ui,
                "Click to select. Ctrl+click adds a page, Shift+click selects a range.",
            );
        });
        action
    }

    fn grid(&mut self, ui: &mut egui::Ui) {
        let p = Palette::current(ui);
        if self.tiles.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(I_GRID).size(44.0).color(p.text2));
                ui.add_space(8.0);
                ui.label(subtitle("No pages yet"));
                ui.label(secondary(
                    ui,
                    "Add one or more PDFs, or drag them here, to arrange their pages.",
                ));
                ui.add_space(12.0);
                if accent_button(ui, true, &labeled(I_ADD, "Add PDFs")) {
                    self.add(pick_files("PDF", PDF_EXT));
                }
            });
            return;
        }
        let nsel = self.tiles.iter().filter(|t| t.selected).count();
        let tile = Vec2::new(120.0, 164.0) * self.zoom;
        let modifiers = ui.input(|i| i.modifiers);
        let mut clicked = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(14.0);
            for (i, t) in self.tiles.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(tile, Sense::click());
                let painter = ui.painter();
                let color = SOURCE_COLORS[t.r.source % SOURCE_COLORS.len()];
                if t.selected {
                    painter.rect_filled(rect, 8.0, p.accent.gamma_multiply(0.18));
                    painter.rect_stroke(rect, 8.0, Stroke::new(2.0, p.accent), StrokeKind::Inside);
                } else if resp.hovered() {
                    painter.rect_filled(rect, 8.0, p.card_hover);
                }
                // Page outline with the right aspect ratio after rotation.
                let (mut w, mut h) = self.sizes[t.r.source]
                    .get(t.r.page as usize - 1)
                    .copied()
                    .unwrap_or((595.0, 842.0));
                if t.r.rotate % 180 != 0 {
                    std::mem::swap(&mut w, &mut h);
                }
                let area = Vec2::new(tile.x - 28.0, tile.y - 48.0);
                let s = (area.x / w).min(area.y / h);
                let page = egui::Rect::from_center_size(
                    rect.center_top() + Vec2::new(0.0, 12.0 + area.y / 2.0),
                    Vec2::new(w * s, h * s),
                );
                painter.rect_filled(
                    page.translate(Vec2::new(0.0, 2.0)),
                    3.0,
                    Color32::from_black_alpha(50),
                );
                painter.rect_filled(page, 3.0, Color32::WHITE);
                painter.rect_stroke(
                    page,
                    3.0,
                    Stroke::new(1.0, Color32::from_gray(210)),
                    StrokeKind::Inside,
                );
                painter.rect_filled(
                    egui::Rect::from_min_size(page.min, Vec2::new(page.width(), 3.0)),
                    egui::CornerRadius {
                        nw: 3,
                        ne: 3,
                        sw: 0,
                        se: 0,
                    },
                    color,
                );
                painter.text(
                    page.center(),
                    Align2::CENTER_CENTER,
                    t.r.page.to_string(),
                    FontId::new(22.0 * self.zoom, semibold()),
                    Color32::from_gray(110),
                );
                if t.r.rotate != 0 {
                    painter.text(
                        page.right_bottom() - Vec2::new(5.0, 4.0),
                        Align2::RIGHT_BOTTOM,
                        format!("{}°", t.r.rotate),
                        FontId::proportional(11.0),
                        Color32::from_rgb(0x00, 0x5F, 0xB8),
                    );
                }
                painter.text(
                    rect.center_bottom() - Vec2::new(0.0, 10.0),
                    Align2::CENTER_BOTTOM,
                    format!("{}", i + 1),
                    FontId::proportional(12.0),
                    if t.selected { p.accent } else { p.text2 },
                );
                if resp.clicked() {
                    clicked = Some(i);
                }
                resp.on_hover_text(format!(
                    "{} · page {}",
                    file_name(&self.sources[t.r.source]),
                    t.r.page
                ));
            }
        });
        if let Some(i) = clicked {
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
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source");
        self.src.ui(ui);

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
            return task("Compressing", move || {
                pdf::compress::compress_file(&path, &out, &opts)
            });
        }
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

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        ui.horizontal(|ui| {
            if ui.button(labeled(I_ADD, "Add images…")).clicked() {
                self.add(pick_files("Images", IMG_EXT));
            }
            if command(ui, self.images.len() > 1, I_SORT, "Sort by name") {
                self.images.sort_by_key(|p| file_name(p).to_lowercase());
            }
            if command(ui, !self.images.is_empty(), I_CLEAR, "Clear") {
                self.images.clear();
            }
        });

        section(
            ui,
            &format!("Images · one per page ({})", self.images.len()),
        );
        if self.images.is_empty() {
            card(ui, |ui| {
                ui.label(secondary(
                    ui,
                    "Add JPG, PNG, WEBP, GIF or BMP images, or drag them onto the window.",
                ));
            });
        } else {
            let total: u64 = self
                .images
                .iter()
                .filter_map(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .sum();
            card(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        let mut i = 0;
                        while i < self.images.len() {
                            let removed = ui
                                .horizontal(|ui| {
                                    ui.label(secondary(ui, format!("{:>3}", i + 1)));
                                    ui.label(RichText::new(I_PHOTO).size(15.0));
                                    ui.label(file_name(&self.images[i]));
                                    ui.label(secondary(ui, file_size(&self.images[i])).size(12.0));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| row_controls(ui, &mut self.images, i),
                                    )
                                    .inner
                                })
                                .inner;
                            if !removed {
                                i += 1;
                            }
                        }
                    });
                caption(ui, &format!("Total {}", pdf::human_size(total)));
            });
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
                move || pdf::images::images_to_pdf(&imgs, &opts, &out),
            );
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

#[derive(Default)]
pub struct LinksState {
    pub src: Source,
    sizes: Vec<(f32, f32)>,
    existing: Vec<(ExistingLink, bool)>,
    drafts: Vec<LinkDraft>,
    error: Option<String>,
}

impl LinksState {
    pub fn load(&mut self) {
        self.existing.clear();
        self.drafts.clear();
        self.sizes.clear();
        self.error = None;
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

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source");
        if self.src.ui(ui) {
            self.load();
        }
        if let Some(e) = &self.error {
            ui.colored_label(Palette::current(ui).error, e);
        }
        self.src.path.as_ref()?;

        section(ui, &format!("Existing links ({})", self.existing.len()));
        card(ui, |ui| {
            if self.existing.is_empty() {
                ui.label(secondary(ui, "This PDF has no links."));
            }
            egui::ScrollArea::vertical()
                .id_salt("existing")
                .max_height(180.0)
                .show(ui, |ui| {
                    egui::Grid::new("ex_links")
                        .striped(true)
                        .num_columns(4)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            for (l, remove) in &mut self.existing {
                                ui.checkbox(remove, "Remove");
                                ui.label(format!("Page {}", l.rect.page));
                                ui.label(secondary(
                                    ui,
                                    format!(
                                        "x {:.0}, y {:.0} · {:.0} × {:.0}",
                                        l.rect.x, l.rect.y, l.rect.w, l.rect.h
                                    ),
                                ));
                                ui.label(&l.target);
                                ui.end_row();
                            }
                        });
                });
        });

        let pages = self.src.pages.max(1);
        section(ui, "New links");
        if ui.button(labeled(I_ADD, "Add link")).clicked() {
            let (w, _) = self.sizes.first().copied().unwrap_or((595.0, 842.0));
            self.drafts.push(LinkDraft {
                rect: LinkRect {
                    page: 1,
                    x: 36.0,
                    y: 36.0,
                    w: (w - 72.0).max(50.0),
                    h: 40.0,
                },
                to_page: false,
                url: "https://".into(),
                page: 1,
            });
        }
        let mut del = None;
        for (i, d) in self.drafts.iter_mut().enumerate() {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(strong(format!("{}  Link {}", I_LINK, i + 1)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, true, I_DELETE, "Remove") {
                            del = Some(i);
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.to_page, false, "Web address");
                    ui.radio_value(&mut d.to_page, true, "Go to page");
                    if d.to_page {
                        ui.add(egui::DragValue::new(&mut d.page).range(1..=pages));
                    } else {
                        ui.add(egui::TextEdit::singleline(&mut d.url).desired_width(360.0));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("On page");
                    ui.add(egui::DragValue::new(&mut d.rect.page).range(1..=pages));
                    let (pw, ph) = self
                        .sizes
                        .get(d.rect.page as usize - 1)
                        .copied()
                        .unwrap_or((595.0, 842.0));
                    ui.add_space(8.0);
                    ui.label("x");
                    ui.add(
                        egui::DragValue::new(&mut d.rect.x)
                            .range(0.0..=pw)
                            .speed(1.0),
                    );
                    ui.label("y");
                    ui.add(
                        egui::DragValue::new(&mut d.rect.y)
                            .range(0.0..=ph)
                            .speed(1.0),
                    );
                    ui.label("width");
                    ui.add(
                        egui::DragValue::new(&mut d.rect.w)
                            .range(1.0..=pw)
                            .speed(1.0),
                    );
                    ui.label("height");
                    ui.add(
                        egui::DragValue::new(&mut d.rect.h)
                            .range(1.0..=ph)
                            .speed(1.0),
                    );
                    if ui.small_button("Whole page").clicked() {
                        d.rect = LinkRect {
                            page: d.rect.page,
                            x: 0.0,
                            y: 0.0,
                            w: pw,
                            h: ph,
                        };
                    }
                    caption(ui, &format!("Page is {pw:.0} × {ph:.0} pt"));
                });
            });
        }
        if let Some(i) = del {
            self.drafts.remove(i);
        }
        caption(
            ui,
            "Positions are in points (1/72 inch), measured from the top-left corner of the page as displayed.",
        );

        let removing: Vec<(u32, usize)> = self
            .existing
            .iter()
            .filter(|(_, r)| *r)
            .map(|(l, _)| (l.rect.page, l.index))
            .collect();
        let changes = removing.len() + self.drafts.len();
        let path = self.src.path.clone()?;
        if footer(ui, changes > 0, &labeled(I_SAVE, "Save links")) {
            let out = save_as(Some(&path), "links", "pdf")?;
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
            return task("Saving links", move || {
                pdf::links::apply(&path, &out, &removing, &add)
            });
        }
        None
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

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source");
        if self.src.ui(ui) {
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
            return task("Saving properties", move || {
                pdf::meta::write_info(&path, &out, &fields)
            });
        }
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
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source");
        self.src.ui(ui);
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
            return task("Extracting text", move || {
                pdf::meta::extract_text(&path, &out, &range)
            });
        }
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
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source");
        self.src.ui(ui);

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
            return task("Encrypting", move || {
                pdf::meta::protect(&path, &out, &u, &o, a)
            });
        }
        None
    }
}

#[derive(Default)]
pub struct UnlockState {
    pub path: Option<PathBuf>,
    password: String,
}

impl UnlockState {
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source");
        let (name, info) = match &self.path {
            Some(p) => (file_name(p), file_size(p)),
            None => ("No file selected".into(), "Choose a protected PDF".into()),
        };
        setting(ui, I_LOCK, &name, &info, |ui| {
            if ui.button(labeled(I_OPEN, "Browse…")).clicked()
                && let Some(p) = pick_file("PDF", PDF_EXT) {
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
            return task("Unlocking", move || pdf::meta::unlock(&path, &out, &pw));
        }
        None
    }
}
