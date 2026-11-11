//! The desktop user interface (egui). Each tool has a small state struct and a `ui` method
//! that returns a [`Task`] when the user presses the action button. Tasks run on a
//! background thread so the window never freezes.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::Result;
use eframe::egui;
use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, StrokeKind, Vec2};

use crate::pdf::{
    self,
    assemble::{MergeItem, PageRef, SplitMode},
    compress::CompressOptions,
    images::{ImageOptions, Orientation, PageSize},
    links::{ExistingLink, LinkRect, LinkTarget, NewLink},
    meta::{Allow, DocInfo, FIELDS},
    stamp::{Position, TextStamp},
};

const HONEY: Color32 = Color32::from_rgb(245, 183, 0);
const PDF_EXT: &[&str] = &["pdf"];
const IMG_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// Work to run in the background: a label for the status bar and the job itself.
type Task = (String, Box<dyn FnOnce() -> Result<String> + Send>);

fn task(
    label: impl Into<String>,
    f: impl FnOnce() -> Result<String> + Send + 'static,
) -> Option<Task> {
    Some((label.into(), Box::new(f)))
}

// ---------------------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    Organize,
    Merge,
    Split,
    Compress,
    ImagesToPdf,
    Watermark,
    PageNumbers,
    Links,
    Metadata,
    ExtractText,
    Protect,
    Unlock,
}

impl Tool {
    const GROUPS: [(&'static str, &'static [Tool]); 5] = [
        ("Organize", &[Tool::Organize, Tool::Merge, Tool::Split]),
        (
            "Optimize & Convert",
            &[Tool::Compress, Tool::ImagesToPdf, Tool::ExtractText],
        ),
        ("Edit", &[Tool::Watermark, Tool::PageNumbers, Tool::Links]),
        ("Document", &[Tool::Metadata]),
        ("Security", &[Tool::Protect, Tool::Unlock]),
    ];

    fn name(self) -> &'static str {
        match self {
            Tool::Organize => "Page Organizer",
            Tool::Merge => "Merge PDFs",
            Tool::Split => "Split / Extract",
            Tool::Compress => "Compress PDF",
            Tool::ImagesToPdf => "Images to PDF",
            Tool::Watermark => "Watermark",
            Tool::PageNumbers => "Page Numbers",
            Tool::Links => "Link Editor",
            Tool::Metadata => "Properties & Metadata",
            Tool::ExtractText => "Extract Text",
            Tool::Protect => "Protect (Password)",
            Tool::Unlock => "Unlock PDF",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tool::Organize => "▦",
            Tool::Merge => "⊕",
            Tool::Split => "✂",
            Tool::Compress => "⇩",
            Tool::ImagesToPdf => "🖼",
            Tool::Watermark => "💧",
            Tool::PageNumbers => "#",
            Tool::Links => "🔗",
            Tool::Metadata => "ℹ",
            Tool::ExtractText => "📝",
            Tool::Protect => "🔒",
            Tool::Unlock => "🔓",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Tool::Organize => {
                "Combine pages from several PDFs, then reorder, rotate, duplicate or delete them."
            }
            Tool::Merge => {
                "Join PDFs into one file. Optionally take only some pages from each file."
            }
            Tool::Split => {
                "Split into single pages, chunks or ranges, or pull selected pages into a new PDF."
            }
            Tool::Compress => {
                "Shrink files: remove unused data, pack objects and downsample images."
            }
            Tool::ImagesToPdf => {
                "Turn JPG, PNG, WEBP, GIF and BMP images into a PDF, one image per page."
            }
            Tool::Watermark => {
                "Stamp text such as CONFIDENTIAL or DRAFT across pages, with opacity and angle."
            }
            Tool::PageNumbers => "Number pages with a custom format, position and starting number.",
            Tool::Links => "Add clickable links to websites or pages, and remove existing links.",
            Tool::Metadata => "See document details and edit title, author, subject and keywords.",
            Tool::ExtractText => "Save the text of a PDF as a .txt file.",
            Tool::Protect => {
                "Encrypt with AES-128. Set an open password and limit printing or copying."
            }
            Tool::Unlock => "Remove the password from a PDF you have the password for.",
        }
    }
}

// ---------------------------------------------------------------------------------------
// File dialogs & small helpers
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

fn has_ext(p: &Path, ext: &[&str]) -> bool {
    p.extension()
        .map(|e| ext.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).weak().small());
}

fn primary(ui: &mut egui::Ui, enabled: bool, text: &str) -> bool {
    let btn = egui::Button::new(RichText::new(text).strong().color(Color32::BLACK))
        .fill(HONEY)
        .min_size(Vec2::new(160.0, 32.0));
    ui.add_enabled(enabled, btn).clicked()
}

fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong());
            ui.add_space(4.0);
            add(ui);
        });
    ui.add_space(8.0);
}

/// "Choose file" row for tools that work on one PDF. Returns true when the file changed.
fn single_pdf(ui: &mut egui::Ui, path: &mut Option<PathBuf>, pages: &mut u32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.button("📂 Choose PDF…").clicked()
            && let Some(p) = pick_file("PDF", PDF_EXT)
        {
            *path = Some(p);
            changed = true;
        }
        match path {
            Some(p) => {
                ui.label(RichText::new(file_name(p)).strong());
                if *pages > 0 {
                    ui.label(RichText::new(format!("{pages} pages")).weak());
                }
            }
            None => {
                ui.label(
                    RichText::new("No file selected. You can also drag a PDF onto the window.")
                        .weak(),
                );
            }
        }
    });
    if changed {
        *pages = path
            .as_deref()
            .and_then(|p| pdf::assemble::count_pages(p).ok())
            .unwrap_or(0);
    }
    changed
}

fn list_controls<T>(ui: &mut egui::Ui, list: &mut Vec<T>, i: usize) {
    let n = list.len();
    if ui
        .add_enabled(i > 0, egui::Button::new("⬆").small())
        .on_hover_text("Move up")
        .clicked()
    {
        list.swap(i, i - 1);
    }
    if ui
        .add_enabled(i + 1 < n, egui::Button::new("⬇").small())
        .on_hover_text("Move down")
        .clicked()
    {
        list.swap(i, i + 1);
    }
    if ui.small_button("✖").on_hover_text("Remove").clicked() {
        list.remove(i);
    }
}

fn stamp_style_ui(ui: &mut egui::Ui, s: &mut TextStamp, id: &str, show_angle: bool) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Font size");
            ui.add(egui::Slider::new(&mut s.size, 6.0..=200.0).suffix(" pt"));
            ui.end_row();
            ui.label("Color");
            ui.horizontal(|ui| {
                ui.color_edit_button_rgb(&mut s.color);
                ui.checkbox(&mut s.bold, "Bold");
            });
            ui.end_row();
            ui.label("Opacity");
            ui.add(
                egui::Slider::new(&mut s.opacity, 0.05..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
            ui.end_row();
            if show_angle {
                ui.label("Angle");
                ui.add(egui::Slider::new(&mut s.angle, -90.0..=90.0).suffix("°"));
                ui.end_row();
            }
            ui.label("Position");
            egui::ComboBox::from_id_salt(format!("{id}_pos"))
                .selected_text(s.position.label())
                .show_ui(ui, |ui| {
                    for p in Position::ALL {
                        ui.selectable_value(&mut s.position, p, p.label());
                    }
                });
            ui.end_row();
            ui.label("Margin");
            ui.add(egui::Slider::new(&mut s.margin, 0.0..=144.0).suffix(" pt"));
            ui.end_row();
            ui.label("Layer");
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.under, false, "Over content");
                ui.radio_value(&mut s.under, true, "Behind content");
            });
            ui.end_row();
        });
}

// ---------------------------------------------------------------------------------------
// Per-tool state
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct MergeState {
    items: Vec<(PathBuf, u32, String)>,
}

impl MergeState {
    fn add(&mut self, paths: Vec<PathBuf>) {
        for p in paths.into_iter().filter(|p| has_ext(p, PDF_EXT)) {
            let n = pdf::assemble::count_pages(&p).unwrap_or(0);
            self.items.push((p, n, String::new()));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Files to merge (top to bottom)", |ui| {
            if ui.button("➕ Add PDFs…").clicked() {
                self.add(pick_files("PDF", PDF_EXT));
            }
            ui.add_space(6.0);
            if self.items.is_empty() {
                hint(ui, "Add two or more PDFs, or drag them onto the window.");
            }
            let mut i = 0;
            while i < self.items.len() {
                let len = self.items.len();
                ui.horizontal(|ui| {
                    ui.label(format!("{}.", i + 1));
                    ui.label(RichText::new(file_name(&self.items[i].0)).strong());
                    ui.label(RichText::new(format!("{} pages", self.items[i].1)).weak());
                    ui.label("Pages:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.items[i].2)
                            .hint_text("all")
                            .desired_width(110.0),
                    );
                    list_controls(ui, &mut self.items, i);
                });
                if self.items.len() == len {
                    i += 1;
                }
            }
            hint(
                ui,
                "Page selection examples: 1-3, 5, 8-end  (leave empty for all pages)",
            );
        });
        let total: u32 = self.items.iter().map(|i| i.1).sum();
        if primary(
            ui,
            !self.items.is_empty(),
            &format!("Merge {} files…", self.items.len()),
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

struct SplitState {
    path: Option<PathBuf>,
    pages: u32,
    mode: SplitMode,
    every: u32,
    spec: String,
}

impl Default for SplitState {
    fn default() -> Self {
        Self {
            path: None,
            pages: 0,
            mode: SplitMode::EveryPage,
            every: 2,
            spec: String::new(),
        }
    }
}

impl SplitState {
    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            single_pdf(ui, &mut self.path, &mut self.pages);
        });
        section(ui, "How to split", |ui| {
            ui.radio_value(&mut self.mode, SplitMode::EveryPage, "One file per page");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.mode, SplitMode::EveryN, "One file every");
                ui.add(egui::DragValue::new(&mut self.every).range(1..=10_000));
                ui.label("pages");
            });
            ui.radio_value(&mut self.mode, SplitMode::Ranges, "One file per range");
            ui.radio_value(
                &mut self.mode,
                SplitMode::Extract,
                "Extract the selected pages into one PDF",
            );
            if matches!(self.mode, SplitMode::Ranges | SplitMode::Extract) {
                ui.horizontal(|ui| {
                    ui.label("Pages:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.spec)
                            .hint_text("e.g. 1-3, 4-7, 10")
                            .desired_width(260.0),
                    );
                });
                hint(
                    ui,
                    "Separate ranges with commas. 'end' means the last page.",
                );
            }
        });
        let path = self.path.clone()?;
        let label = if self.mode == SplitMode::Extract {
            "Extract pages…"
        } else {
            "Split into folder…"
        };
        if primary(ui, true, label) {
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

#[derive(Clone)]
struct Tile {
    r: PageRef,
    selected: bool,
}

#[derive(Default)]
struct OrganizeState {
    sources: Vec<PathBuf>,
    /// Visual page sizes per source.
    sizes: Vec<Vec<(f32, f32)>>,
    tiles: Vec<Tile>,
    zoom: f32,
    error: Option<String>,
}

const SOURCE_COLORS: [Color32; 6] = [
    Color32::from_rgb(245, 183, 0),
    Color32::from_rgb(66, 165, 245),
    Color32::from_rgb(102, 187, 106),
    Color32::from_rgb(239, 83, 80),
    Color32::from_rgb(171, 71, 188),
    Color32::from_rgb(38, 198, 218),
];

impl OrganizeState {
    fn add(&mut self, paths: Vec<PathBuf>) {
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

    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        if self.zoom == 0.0 {
            self.zoom = 1.0;
        }
        let any = !self.tiles.is_empty();
        let nsel = self.tiles.iter().filter(|t| t.selected).count();

        ui.horizontal_wrapped(|ui| {
            if ui.button("➕ Add PDFs…").clicked() {
                self.add(pick_files("PDF", PDF_EXT));
            }
            ui.separator();
            if ui
                .add_enabled(any, egui::Button::new("Select all"))
                .clicked()
            {
                self.tiles.iter_mut().for_each(|t| t.selected = true);
            }
            if ui
                .add_enabled(nsel > 0, egui::Button::new("Select none"))
                .clicked()
            {
                self.tiles.iter_mut().for_each(|t| t.selected = false);
            }
            ui.separator();
            let has = nsel > 0;
            if ui
                .add_enabled(has, egui::Button::new("↺ Rotate left"))
                .clicked()
            {
                self.tiles
                    .iter_mut()
                    .filter(|t| t.selected)
                    .for_each(|t| t.r.rotate = (t.r.rotate - 90).rem_euclid(360));
            }
            if ui
                .add_enabled(has, egui::Button::new("↻ Rotate right"))
                .clicked()
            {
                self.tiles
                    .iter_mut()
                    .filter(|t| t.selected)
                    .for_each(|t| t.r.rotate = (t.r.rotate + 90).rem_euclid(360));
            }
            if ui.add_enabled(has, egui::Button::new("◀ Move")).clicked() {
                self.move_selected(-1);
            }
            if ui.add_enabled(has, egui::Button::new("Move ▶")).clicked() {
                self.move_selected(1);
            }
            if ui
                .add_enabled(has, egui::Button::new("⧉ Duplicate"))
                .clicked()
            {
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
            if ui.add_enabled(has, egui::Button::new("🗑 Delete")).clicked() {
                self.tiles.retain(|t| !t.selected);
            }
            ui.separator();
            if ui
                .add_enabled(any, egui::Button::new("⇅ Reverse"))
                .clicked()
            {
                self.tiles.reverse();
            }
            if ui.add_enabled(any, egui::Button::new("Clear")).clicked() {
                *self = Self::default();
            }
            ui.separator();
            ui.add(
                egui::Slider::new(&mut self.zoom, 0.6..=2.0)
                    .text("Size")
                    .show_value(false),
            );
        });
        if let Some(e) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, e);
        }
        ui.horizontal_wrapped(|ui| {
            for (i, s) in self.sources.iter().enumerate() {
                ui.label(RichText::new("■").color(SOURCE_COLORS[i % SOURCE_COLORS.len()]));
                ui.label(RichText::new(file_name(s)).small());
            }
        });
        ui.label(
            RichText::new(format!(
                "{} pages · {} selected  —  click to select, Ctrl+click or Shift+click to select several",
                self.tiles.len(),
                nsel
            ))
            .weak(),
        );
        ui.separator();

        let mut action = None;
        let bottom = 52.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height() - bottom)
            .show(ui, |ui| {
                if !any {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("▦").size(48.0).weak());
                        ui.label("Add one or more PDFs to start arranging their pages.");
                    });
                    return;
                }
                let tile = Vec2::new(110.0, 150.0) * self.zoom;
                let modifiers = ui.input(|i| i.modifiers);
                let mut clicked = None;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(12.0);
                    for (i, t) in self.tiles.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(tile, Sense::click());
                        let p = ui.painter();
                        let visuals = ui.visuals();
                        let color = SOURCE_COLORS[t.r.source % SOURCE_COLORS.len()];
                        if t.selected {
                            p.rect_filled(rect, 8.0, HONEY.gamma_multiply(0.25));
                            p.rect_stroke(rect, 8.0, Stroke::new(2.0, HONEY), StrokeKind::Inside);
                        } else if resp.hovered() {
                            p.rect_filled(rect, 8.0, visuals.widgets.hovered.bg_fill);
                        }
                        // Page shape with the right aspect ratio after rotation.
                        let (mut w, mut h) = self.sizes[t.r.source]
                            .get(t.r.page as usize - 1)
                            .copied()
                            .unwrap_or((595.0, 842.0));
                        if t.r.rotate % 180 != 0 {
                            std::mem::swap(&mut w, &mut h);
                        }
                        let area = Vec2::new(tile.x - 24.0, tile.y - 44.0);
                        let s = (area.x / w).min(area.y / h);
                        let page = egui::Rect::from_center_size(
                            rect.center_top() + Vec2::new(0.0, 10.0 + area.y / 2.0),
                            Vec2::new(w * s, h * s),
                        );
                        p.rect_filled(
                            page.translate(Vec2::splat(2.0)),
                            2.0,
                            Color32::from_black_alpha(60),
                        );
                        p.rect_filled(page, 2.0, Color32::from_gray(250));
                        p.rect_filled(
                            egui::Rect::from_min_size(page.min, Vec2::new(page.width(), 4.0)),
                            0.0,
                            color,
                        );
                        p.text(
                            page.center(),
                            Align2::CENTER_CENTER,
                            t.r.page.to_string(),
                            FontId::proportional(22.0 * self.zoom),
                            Color32::from_gray(90),
                        );
                        if t.r.rotate != 0 {
                            p.text(
                                page.right_bottom() - Vec2::new(4.0, 4.0),
                                Align2::RIGHT_BOTTOM,
                                format!("↻{}°", t.r.rotate),
                                FontId::proportional(11.0),
                                Color32::from_rgb(200, 120, 0),
                            );
                        }
                        p.text(
                            rect.center_bottom() - Vec2::new(0.0, 8.0),
                            Align2::CENTER_BOTTOM,
                            format!("{}", i + 1),
                            FontId::proportional(13.0),
                            visuals.text_color(),
                        );
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        resp.on_hover_text(format!(
                            "{} — page {}",
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
            });
        ui.add_space(8.0);
        if primary(ui, any, &format!("Save {} pages as PDF…", self.tiles.len()))
            && let Some(out) = save_as(
                self.sources.first().map(|p| p.as_path()),
                "organized",
                "pdf",
            )
        {
            let sources = self.sources.clone();
            let pages: Vec<PageRef> = self.tiles.iter().map(|t| t.r).collect();
            action = task("Saving organized PDF", move || {
                pdf::assemble::organize(&sources, &pages, &out)
            });
        }
        action
    }
}

#[derive(Default)]
struct CompressState {
    path: Option<PathBuf>,
    pages: u32,
    opts: CompressOptions,
}

impl CompressState {
    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            single_pdf(ui, &mut self.path, &mut self.pages);
            if let Some(size) = self
                .path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
            {
                hint(ui, &format!("Current size: {}", pdf::human_size(size)));
            }
        });
        section(ui, "Presets", |ui| {
            ui.horizontal(|ui| {
                if ui.button("Light (best quality)").clicked() {
                    self.opts = CompressOptions {
                        images: false,
                        ..Default::default()
                    };
                }
                if ui.button("Balanced").clicked() {
                    self.opts = CompressOptions::default();
                }
                if ui.button("Strong (smallest)").clicked() {
                    self.opts = CompressOptions {
                        images: true,
                        jpeg_quality: 50,
                        max_dimension: 1400,
                        lossless_to_jpeg: true,
                        strip_metadata: true,
                    };
                }
            });
        });
        section(ui, "Options", |ui| {
            let o = &mut self.opts;
            hint(
                ui,
                "Unused objects are always removed, and streams and the object table are always compressed.",
            );
            ui.checkbox(&mut o.images, "Re-compress images (JPEG)");
            ui.add_enabled_ui(o.images, |ui| {
                ui.add(egui::Slider::new(&mut o.jpeg_quality, 10..=95).text("JPEG quality"));
                ui.horizontal(|ui| {
                    ui.label("Downscale images larger than");
                    ui.add(
                        egui::DragValue::new(&mut o.max_dimension)
                            .range(0..=10_000)
                            .suffix(" px"),
                    );
                    hint(ui, "(0 = keep size)");
                });
                ui.checkbox(
                    &mut o.lossless_to_jpeg,
                    "Also convert lossless images to JPEG (good for scans)",
                );
            });
            ui.checkbox(
                &mut o.strip_metadata,
                "Remove metadata (title, author, XMP)",
            );
        });
        let path = self.path.clone()?;
        if primary(ui, true, "Compress & save…") {
            let out = save_as(Some(&path), "compressed", "pdf")?;
            let opts = self.opts.clone();
            return task("Compressing", move || {
                pdf::compress::compress_file(&path, &out, &opts)
            });
        }
        None
    }
}

struct ImagesState {
    images: Vec<PathBuf>,
    opts: ImageOptions,
}

impl Default for ImagesState {
    fn default() -> Self {
        Self {
            images: vec![],
            opts: ImageOptions {
                page_size: PageSize::A4,
                orientation: Orientation::Auto,
                margin: 18.0,
            },
        }
    }
}

impl ImagesState {
    fn add(&mut self, paths: Vec<PathBuf>) {
        self.images
            .extend(paths.into_iter().filter(|p| has_ext(p, IMG_EXT)));
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Images (one per page, in this order)", |ui| {
            ui.horizontal(|ui| {
                if ui.button("➕ Add images…").clicked() {
                    self.add(pick_files("Images", IMG_EXT));
                }
                if ui
                    .add_enabled(self.images.len() > 1, egui::Button::new("Sort by name"))
                    .clicked()
                {
                    self.images.sort_by_key(|p| file_name(p).to_lowercase());
                }
            });
            if self.images.is_empty() {
                hint(
                    ui,
                    "Supported formats: JPG, PNG, WEBP, GIF, BMP. JPEGs are embedded without any quality loss.",
                );
            }
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    let mut i = 0;
                    while i < self.images.len() {
                        let len = self.images.len();
                        ui.horizontal(|ui| {
                            ui.label(format!("{}.", i + 1));
                            ui.label(file_name(&self.images[i]));
                            list_controls(ui, &mut self.images, i);
                        });
                        if self.images.len() == len {
                            i += 1;
                        }
                    }
                });
        });
        section(ui, "Page setup", |ui| {
            let o = &mut self.opts;
            egui::Grid::new("img_grid")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Page size");
                    egui::ComboBox::from_id_salt("pagesize")
                        .selected_text(o.page_size.label())
                        .show_ui(ui, |ui| {
                            for s in PageSize::ALL {
                                ui.selectable_value(&mut o.page_size, s, s.label());
                            }
                        });
                    ui.end_row();
                    ui.label("Orientation");
                    ui.add_enabled_ui(o.page_size != PageSize::FitImage, |ui| {
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut o.orientation, Orientation::Auto, "Auto");
                            ui.radio_value(&mut o.orientation, Orientation::Portrait, "Portrait");
                            ui.radio_value(&mut o.orientation, Orientation::Landscape, "Landscape");
                        });
                    });
                    ui.end_row();
                    ui.label("Margin");
                    ui.add(egui::Slider::new(&mut o.margin, 0.0..=72.0).suffix(" pt"));
                    ui.end_row();
                });
        });
        if primary(
            ui,
            !self.images.is_empty(),
            &format!("Create PDF from {} image(s)…", self.images.len()),
        ) {
            let out = save_as(self.images.first().map(|p| p.as_path()), "images", "pdf")?;
            let (imgs, opts) = (self.images.clone(), self.opts.clone());
            return task("Converting images", move || {
                pdf::images::images_to_pdf(&imgs, &opts, &out)
            });
        }
        None
    }
}

struct WatermarkState {
    path: Option<PathBuf>,
    pages: u32,
    text: String,
    range: String,
    style: TextStamp,
}

impl Default for WatermarkState {
    fn default() -> Self {
        Self {
            path: None,
            pages: 0,
            text: "CONFIDENTIAL".into(),
            range: String::new(),
            style: TextStamp {
                size: 64.0,
                color: [0.85, 0.1, 0.1],
                opacity: 0.25,
                angle: 45.0,
                position: Position::Center,
                margin: 36.0,
                bold: true,
                under: false,
            },
        }
    }
}

impl WatermarkState {
    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            single_pdf(ui, &mut self.path, &mut self.pages);
        });
        section(ui, "Watermark", |ui| {
            ui.horizontal(|ui| {
                ui.label("Text");
                ui.add(egui::TextEdit::singleline(&mut self.text).desired_width(320.0));
            });
            ui.horizontal(|ui| {
                for preset in [
                    "CONFIDENTIAL",
                    "DRAFT",
                    "COPY",
                    "APPROVED",
                    "SAMPLE",
                    "DO NOT COPY",
                ] {
                    if ui.small_button(preset).clicked() {
                        self.text = preset.into();
                    }
                }
            });
            ui.add_space(6.0);
            stamp_style_ui(ui, &mut self.style, "wm", true);
            ui.horizontal(|ui| {
                ui.label("Pages");
                ui.add(
                    egui::TextEdit::singleline(&mut self.range)
                        .hint_text("all")
                        .desired_width(160.0),
                );
            });
            hint(
                ui,
                "Uses a built-in Helvetica font. Characters outside Latin-1 appear as '?'.",
            );
        });
        let path = self.path.clone()?;
        if primary(ui, !self.text.trim().is_empty(), "Apply watermark & save…") {
            let out = save_as(Some(&path), "watermarked", "pdf")?;
            let (text, range, style) = (self.text.clone(), self.range.clone(), self.style.clone());
            return task("Adding watermark", move || {
                pdf::stamp::watermark(&path, &out, &text, &range, &style)
            });
        }
        None
    }
}

struct NumbersState {
    path: Option<PathBuf>,
    pages: u32,
    format: String,
    start: i64,
    skip: u32,
    style: TextStamp,
}

impl Default for NumbersState {
    fn default() -> Self {
        Self {
            path: None,
            pages: 0,
            format: "Page {n} of {total}".into(),
            start: 1,
            skip: 0,
            style: TextStamp {
                size: 10.0,
                color: [0.2, 0.2, 0.2],
                opacity: 1.0,
                angle: 0.0,
                position: Position::BottomCenter,
                margin: 24.0,
                bold: false,
                under: false,
            },
        }
    }
}

impl NumbersState {
    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            single_pdf(ui, &mut self.path, &mut self.pages);
        });
        section(ui, "Numbering", |ui| {
            ui.horizontal(|ui| {
                ui.label("Format");
                ui.add(egui::TextEdit::singleline(&mut self.format).desired_width(220.0));
                for f in [
                    "{n}",
                    "Page {n}",
                    "Page {n} of {total}",
                    "{n} / {total}",
                    "- {n} -",
                ] {
                    if ui.small_button(f).clicked() {
                        self.format = f.into();
                    }
                }
            });
            hint(ui, "{n} is the page number and {total} is the last number.");
            ui.horizontal(|ui| {
                ui.label("Start at");
                ui.add(egui::DragValue::new(&mut self.start).range(-9999..=99_999));
                ui.add_space(16.0);
                ui.label("Skip first");
                ui.add(egui::DragValue::new(&mut self.skip).range(0..=9999));
                ui.label("page(s), e.g. a cover");
            });
            ui.add_space(6.0);
            stamp_style_ui(ui, &mut self.style, "pn", false);
            if self.pages > 0 {
                let first = self.format.replace("{n}", &self.start.to_string()).replace(
                    "{total}",
                    &(self.start + self.pages.saturating_sub(self.skip) as i64 - 1).to_string(),
                );
                hint(
                    ui,
                    &format!("Preview on page {}: \"{first}\"", self.skip + 1),
                );
            }
        });
        let path = self.path.clone()?;
        if primary(ui, true, "Add page numbers & save…") {
            let out = save_as(Some(&path), "numbered", "pdf")?;
            let (fmt, start, skip, style) = (
                self.format.clone(),
                self.start,
                self.skip,
                self.style.clone(),
            );
            return task("Numbering pages", move || {
                pdf::stamp::page_numbers(&path, &out, &fmt, start, skip, &style)
            });
        }
        None
    }
}

struct LinkDraft {
    rect: LinkRect,
    to_page: bool,
    url: String,
    page: u32,
}

#[derive(Default)]
struct LinksState {
    path: Option<PathBuf>,
    pages: u32,
    sizes: Vec<(f32, f32)>,
    existing: Vec<(ExistingLink, bool)>,
    drafts: Vec<LinkDraft>,
    error: Option<String>,
}

impl LinksState {
    fn load(&mut self) {
        self.existing.clear();
        self.drafts.clear();
        self.sizes.clear();
        self.error = None;
        if let Some(p) = &self.path {
            match pdf::links::scan(p) {
                Ok(s) => {
                    self.existing = s.links.into_iter().map(|l| (l, false)).collect();
                    self.sizes = s.page_sizes;
                }
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            if single_pdf(ui, &mut self.path, &mut self.pages) {
                self.load();
            }
            if let Some(e) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
        });
        self.path.as_ref()?;
        section(
            ui,
            &format!("Existing links ({})", self.existing.len()),
            |ui| {
                if self.existing.is_empty() {
                    hint(ui, "This PDF has no links.");
                }
                egui::ScrollArea::vertical()
                    .id_salt("existing")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        egui::Grid::new("ex_links")
                            .striped(true)
                            .num_columns(4)
                            .show(ui, |ui| {
                                for (l, remove) in &mut self.existing {
                                    ui.checkbox(remove, "Remove");
                                    ui.label(format!("Page {}", l.rect.page));
                                    ui.label(
                                        RichText::new(format!(
                                            "x {:.0}, y {:.0}, {:.0}×{:.0}",
                                            l.rect.x, l.rect.y, l.rect.w, l.rect.h
                                        ))
                                        .weak(),
                                    );
                                    ui.label(&l.target);
                                    ui.end_row();
                                }
                            });
                    });
            },
        );
        let pages = self.pages.max(1);
        section(ui, "New links", |ui| {
            hint(
                ui,
                "Positions are in points (1/72 inch), measured from the top-left corner of the page as displayed.",
            );
            if ui.button("➕ Add link").clicked() {
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
                egui::Frame::new()
                    .inner_margin(6.0)
                    .corner_radius(6.0)
                    .fill(ui.visuals().faint_bg_color)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Page");
                            ui.add(egui::DragValue::new(&mut d.rect.page).range(1..=pages));
                            let (pw, ph) = self
                                .sizes
                                .get(d.rect.page as usize - 1)
                                .copied()
                                .unwrap_or((595.0, 842.0));
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
                            ui.label(
                                RichText::new(format!("(page is {pw:.0}×{ph:.0})"))
                                    .weak()
                                    .small(),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.to_page, false, "Web address");
                            ui.radio_value(&mut d.to_page, true, "Go to page");
                            if d.to_page {
                                ui.add(egui::DragValue::new(&mut d.page).range(1..=pages));
                            } else {
                                ui.add(egui::TextEdit::singleline(&mut d.url).desired_width(320.0));
                            }
                            if ui.small_button("✖").clicked() {
                                del = Some(i);
                            }
                        });
                    });
            }
            if let Some(i) = del {
                self.drafts.remove(i);
            }
        });
        let removing: Vec<(u32, usize)> = self
            .existing
            .iter()
            .filter(|(_, r)| *r)
            .map(|(l, _)| (l.rect.page, l.index))
            .collect();
        let changes = removing.len() + self.drafts.len();
        let path = self.path.clone()?;
        if primary(ui, changes > 0, "Save links…") {
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

#[derive(Default)]
struct MetaState {
    path: Option<PathBuf>,
    pages: u32,
    info: Option<DocInfo>,
    fields: Vec<String>,
    error: Option<String>,
}

impl MetaState {
    fn load(&mut self) {
        self.error = None;
        self.info = None;
        if let Some(p) = &self.path {
            match pdf::meta::read_info(p) {
                Ok(info) => {
                    self.fields = info.fields.clone();
                    self.info = Some(info);
                }
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            if single_pdf(ui, &mut self.path, &mut self.pages) {
                self.load();
            }
            if let Some(e) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, e);
            }
        });
        let info = self.info.clone()?;
        section(ui, "Document", |ui| {
            egui::Grid::new("docinfo")
                .num_columns(2)
                .spacing([24.0, 6.0])
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
                        ui.label(RichText::new(k).weak());
                        ui.label(v);
                        ui.end_row();
                    }
                });
        });
        section(ui, "Metadata", |ui| {
            egui::Grid::new("meta")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    for (k, v) in FIELDS.iter().zip(self.fields.iter_mut()) {
                        ui.label(*k);
                        ui.add(egui::TextEdit::singleline(v).desired_width(420.0));
                        ui.end_row();
                    }
                });
            hint(ui, "Leave a field empty to remove it.");
        });
        let path = self.path.clone()?;
        if primary(ui, self.fields != info.fields, "Save metadata…") {
            let out = save_as(Some(&path), "edited", "pdf")?;
            let fields = self.fields.clone();
            return task("Saving metadata", move || {
                pdf::meta::write_info(&path, &out, &fields)
            });
        }
        None
    }
}

#[derive(Default)]
struct TextState {
    path: Option<PathBuf>,
    pages: u32,
    range: String,
}

impl TextState {
    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            single_pdf(ui, &mut self.path, &mut self.pages);
            ui.horizontal(|ui| {
                ui.label("Pages");
                ui.add(
                    egui::TextEdit::singleline(&mut self.range)
                        .hint_text("all")
                        .desired_width(160.0),
                );
            });
            hint(
                ui,
                "Scanned PDFs contain images rather than text, so no text can be extracted from them.",
            );
        });
        let path = self.path.clone()?;
        if primary(ui, true, "Extract to .txt…") {
            let out = save_as(Some(&path), "text", "txt")?;
            let range = self.range.clone();
            return task("Extracting text", move || {
                pdf::meta::extract_text(&path, &out, &range)
            });
        }
        None
    }
}

#[derive(Default)]
struct ProtectState {
    path: Option<PathBuf>,
    pages: u32,
    user: String,
    confirm: String,
    owner: String,
    allow: Allow,
}

impl ProtectState {
    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            single_pdf(ui, &mut self.path, &mut self.pages);
        });
        section(ui, "Passwords", |ui| {
            egui::Grid::new("pw")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Open password");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.user)
                            .password(true)
                            .desired_width(260.0),
                    );
                    ui.end_row();
                    ui.label("Confirm");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.confirm)
                            .password(true)
                            .desired_width(260.0),
                    );
                    ui.end_row();
                    ui.label("Owner password");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.owner)
                            .password(true)
                            .hint_text("optional")
                            .desired_width(260.0),
                    );
                    ui.end_row();
                });
            if self.user != self.confirm {
                ui.colored_label(Color32::LIGHT_RED, "The passwords do not match.");
            }
            hint(
                ui,
                "The open password is needed to view the file. The owner password removes the restrictions below.",
            );
        });
        section(ui, "Allow", |ui| {
            let a = &mut self.allow;
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut a.print, "Printing");
                ui.checkbox(&mut a.copy, "Copying text");
                ui.checkbox(&mut a.modify, "Editing");
                ui.checkbox(&mut a.annotate, "Comments");
                ui.checkbox(&mut a.fill_forms, "Filling forms");
                ui.checkbox(&mut a.assemble, "Page assembly");
            });
        });
        let path = self.path.clone()?;
        let ok = self.user == self.confirm && !(self.user.is_empty() && self.owner.is_empty());
        if primary(ui, ok, "Protect & save…") {
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
struct UnlockState {
    path: Option<PathBuf>,
    password: String,
}

impl UnlockState {
    fn ui(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        section(ui, "Source", |ui| {
            ui.horizontal(|ui| {
                if ui.button("📂 Choose PDF…").clicked()
                    && let Some(p) = pick_file("PDF", PDF_EXT)
                {
                    self.path = Some(p);
                }
                match &self.path {
                    Some(p) => ui.label(RichText::new(file_name(p)).strong()),
                    None => ui.label(RichText::new("No file selected").weak()),
                };
            });
            ui.horizontal(|ui| {
                ui.label("Password");
                ui.add(
                    egui::TextEdit::singleline(&mut self.password)
                        .password(true)
                        .desired_width(260.0),
                );
            });
            hint(
                ui,
                "Files with only an owner password (printing or copying restrictions) can be unlocked with an empty password.",
            );
        });
        let path = self.path.clone()?;
        if primary(ui, true, "Unlock & save…") {
            let out = save_as(Some(&path), "unlocked", "pdf")?;
            let pw = self.password.clone();
            return task("Unlocking", move || pdf::meta::unlock(&path, &out, &pw));
        }
        None
    }
}

// ---------------------------------------------------------------------------------------
// App shell
// ---------------------------------------------------------------------------------------

struct Running {
    label: String,
    rx: mpsc::Receiver<Result<String>>,
}

pub struct App {
    tool: Tool,
    running: Option<Running>,
    /// (success, message) history, newest last.
    log: Vec<(bool, String)>,
    dark: bool,
    merge: MergeState,
    split: SplitState,
    organize: OrganizeState,
    compress: CompressState,
    images: ImagesState,
    watermark: WatermarkState,
    numbers: NumbersState,
    links: LinksState,
    meta: MetaState,
    text: TextState,
    protect: ProtectState,
    unlock: UnlockState,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            tool: Tool::Organize,
            running: None,
            log: vec![],
            dark: true,
            merge: Default::default(),
            split: Default::default(),
            organize: Default::default(),
            compress: Default::default(),
            images: Default::default(),
            watermark: Default::default(),
            numbers: Default::default(),
            links: Default::default(),
            meta: Default::default(),
            text: Default::default(),
            protect: Default::default(),
            unlock: Default::default(),
        };
        app.apply_theme(&cc.egui_ctx);
        // PDFs passed on the command line (e.g. "Open with…") open in the organizer.
        let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        app.organize.add(args);
        app
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let mut v = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.selection.bg_fill = HONEY.gamma_multiply(0.6);
        v.hyperlink_color = HONEY;
        ctx.set_visuals(v);
    }

    fn start(&mut self, ctx: &egui::Context, (label, job): Task) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Unexpected internal error")));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.running = Some(Running { label, rx });
    }

    fn poll(&mut self) {
        let Some(r) = &self.running else { return };
        match r.rx.try_recv() {
            Ok(Ok(msg)) => {
                self.log.push((true, msg));
                self.running = None;
            }
            Ok(Err(e)) => {
                self.log.push((false, format!("{e:#}")));
                self.running = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.log
                    .push((false, "The task stopped unexpectedly".into()));
                self.running = None;
            }
        }
    }

    /// Files dropped onto the window go to the current tool.
    fn accept_drop(&mut self, files: Vec<PathBuf>) {
        let first_pdf = files.iter().find(|p| has_ext(p, PDF_EXT)).cloned();
        let set = |path: &mut Option<PathBuf>, pages: &mut u32| {
            if let Some(p) = first_pdf.clone() {
                *pages = pdf::assemble::count_pages(&p).unwrap_or(0);
                *path = Some(p);
            }
        };
        match self.tool {
            Tool::Merge => self.merge.add(files),
            Tool::Organize => self.organize.add(files),
            Tool::ImagesToPdf => self.images.add(files),
            Tool::Split => set(&mut self.split.path, &mut self.split.pages),
            Tool::Compress => set(&mut self.compress.path, &mut self.compress.pages),
            Tool::Watermark => set(&mut self.watermark.path, &mut self.watermark.pages),
            Tool::PageNumbers => set(&mut self.numbers.path, &mut self.numbers.pages),
            Tool::ExtractText => set(&mut self.text.path, &mut self.text.pages),
            Tool::Protect => set(&mut self.protect.path, &mut self.protect.pages),
            Tool::Links => {
                set(&mut self.links.path, &mut self.links.pages);
                self.links.load();
            }
            Tool::Metadata => {
                set(&mut self.meta.path, &mut self.meta.pages);
                self.meta.load();
            }
            Tool::Unlock => {
                if first_pdf.is_some() {
                    self.unlock.path = first_pdf;
                }
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("🐝").size(26.0));
            ui.vertical(|ui| {
                ui.label(RichText::new("UtilBee").size(20.0).strong().color(HONEY));
                ui.label(RichText::new("PDF Studio").weak());
            });
        });
        ui.add_space(10.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height() - 40.0)
            .show(ui, |ui| {
                for (group, tools) in Tool::GROUPS {
                    ui.add_space(6.0);
                    ui.label(RichText::new(group.to_uppercase()).small().weak());
                    for &t in tools {
                        let selected = self.tool == t;
                        let text = RichText::new(format!("{}  {}", t.icon(), t.name()));
                        let text = if selected {
                            text.strong().color(Color32::WHITE)
                        } else {
                            text
                        };
                        let resp = ui.add(
                            egui::Button::selectable(selected, text)
                                .min_size(Vec2::new(ui.available_width(), 28.0)),
                        );
                        if resp.on_hover_text(t.description()).clicked() {
                            self.tool = t;
                        }
                    }
                }
            });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            let label = if self.dark {
                "☀ Light mode"
            } else {
                "🌙 Dark mode"
            };
            if ui.button(label).clicked() {
                self.dark = !self.dark;
                self.apply_theme(ui.ctx());
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(r) = &self.running {
                ui.spinner();
                ui.label(format!("{}…", r.label));
            } else if let Some((ok, msg)) = self.log.last() {
                let (icon, color) = if *ok {
                    ("✔", Color32::from_rgb(102, 187, 106))
                } else {
                    ("✖", Color32::from_rgb(239, 83, 80))
                };
                ui.label(RichText::new(icon).color(color));
                ui.label(RichText::new(msg).color(color));
                if *ok {
                    // Offer to open the folder of the produced file.
                    if let Some(path) = msg.rsplit(" → ").next().filter(|_| msg.contains(" → "))
                    {
                        let p = PathBuf::from(path.trim());
                        if ui.small_button("Open folder").clicked() {
                            let dir = if p.is_dir() {
                                p.clone()
                            } else {
                                p.parent().map(Path::to_path_buf).unwrap_or_default()
                            };
                            let _ = open_path(&dir);
                        }
                        if p.is_file() && ui.small_button("Open file").clicked() {
                            let _ = open_path(&p);
                        }
                    }
                }
            } else {
                ui.label(
                    RichText::new(
                        "Ready. Choose a tool on the left. Your files never leave this computer.",
                    )
                    .weak(),
                );
            }
        });
    }
}

fn open_path(p: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(p);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(p);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(p);
        c
    };
    cmd.spawn().map(|_| ())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();

        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.accept_drop(dropped);
        }
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(4.0);
            self.status_bar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::left("tools")
            .resizable(false)
            .exact_size(220.0)
            .show(ui, |ui| self.sidebar(ui));

        let mut action = None;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{}  {}", self.tool.icon(), self.tool.name()))
                    .size(24.0)
                    .strong(),
            );
            ui.label(RichText::new(self.tool.description()).weak());
            ui.add_space(10.0);
            let busy = self.running.is_some();
            ui.add_enabled_ui(!busy, |ui| {
                let organize = self.tool == Tool::Organize;
                let body = |app: &mut Self, ui: &mut egui::Ui| match app.tool {
                    Tool::Organize => app.organize.ui(ui),
                    Tool::Merge => app.merge.ui(ui),
                    Tool::Split => app.split.ui(ui),
                    Tool::Compress => app.compress.ui(ui),
                    Tool::ImagesToPdf => app.images.ui(ui),
                    Tool::Watermark => app.watermark.ui(ui),
                    Tool::PageNumbers => app.numbers.ui(ui),
                    Tool::Links => app.links.ui(ui),
                    Tool::Metadata => app.meta.ui(ui),
                    Tool::ExtractText => app.text.ui(ui),
                    Tool::Protect => app.protect.ui(ui),
                    Tool::Unlock => app.unlock.ui(ui),
                };
                // The organizer manages its own scrolling; other tools scroll as a whole.
                action = if organize {
                    body(self, ui)
                } else {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| body(self, ui))
                        .inner
                };
            });
        });

        if hovering {
            let rect = ui.ctx().content_rect();
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("utilbee_drop_overlay"),
            ));
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(160));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("Drop files into {}", self.tool.name()),
                FontId::proportional(28.0),
                HONEY,
            );
        }

        if let Some(t) = action {
            let ctx = ui.ctx().clone();
            self.start(&ctx, t);
        }
    }
}
