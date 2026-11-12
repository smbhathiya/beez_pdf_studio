//! Page previews: thumbnail cache, page-grid widget and a full-size page viewer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2,
};

use super::theme::*;
use crate::pdf::render::{self, Rendered, Request};

/// Longest side of a thumbnail bitmap (rendered for crisp display on high-DPI screens).
pub const THUMB: u32 = 360;
/// Longest side of the bitmap shown in the page viewer.
pub const LARGE: u32 = 1800;

type Key = (PathBuf, Request);

enum Slot {
    Loading,
    Failed,
    Ready(TextureHandle),
}

/// One page in the viewer's sequence (a file, a page and an extra rotation in degrees).
#[derive(Clone)]
pub struct ViewItem {
    pub path: PathBuf,
    pub page: u32,
    pub rotate: i64,
}

struct Viewer {
    items: Vec<ViewItem>,
    index: usize,
    zoom: f32,
}

pub struct Thumbs {
    workers: HashMap<PathBuf, Sender<Request>>,
    tx: Sender<Rendered>,
    rx: Receiver<Rendered>,
    slots: HashMap<Key, Slot>,
    viewer: Option<Viewer>,
    /// Shared decoder for image files (Images to PDF).
    image_worker: Option<Sender<(PathBuf, Request)>>,
    /// Thumbnail size multiplier shared by all page grids.
    pub zoom: f32,
}

impl Default for Thumbs {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            workers: HashMap::new(),
            tx,
            rx,
            slots: HashMap::new(),
            viewer: None,
            image_worker: None,
            zoom: 1.0,
        }
    }
}

impl Thumbs {
    /// Move finished renders into GPU textures. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(r) = self.rx.try_recv() {
            let key = (r.path.clone(), r.request);
            let slot = match r.bitmap {
                Some(b) => {
                    let image = ColorImage::from_rgba_premultiplied(
                        [b.width as usize, b.height as usize],
                        &b.rgba,
                    );
                    let name =
                        format!("{}#{}@{}", r.path.display(), r.request.page, r.request.size);
                    Slot::Ready(ctx.load_texture(name, image, TextureOptions::LINEAR))
                }
                None => Slot::Failed,
            };
            self.slots.insert(key, slot);
        }
    }

    /// Forget everything about a file (call when it is re-selected, as it may have changed).
    pub fn invalidate(&mut self, path: &Path) {
        self.workers.remove(path);
        self.slots.retain(|(p, _), _| p != path);
    }

    fn request(&mut self, ctx: &egui::Context, path: &Path, page: u32, size: u32) -> &Slot {
        let key = (path.to_path_buf(), Request { page, size });
        if !self.slots.contains_key(&key) {
            if is_pdf(path) {
                let worker = self.workers.entry(path.to_path_buf()).or_insert_with(|| {
                    let ctx = ctx.clone();
                    render::spawn_worker(path.to_path_buf(), self.tx.clone(), move || {
                        ctx.request_repaint()
                    })
                });
                let _ = worker.send(key.1);
            } else {
                let worker = self.image_worker.get_or_insert_with(|| {
                    let ctx = ctx.clone();
                    render::spawn_image_worker(self.tx.clone(), move || ctx.request_repaint())
                });
                let _ = worker.send(key.clone());
            }
            self.slots.insert(key.clone(), Slot::Loading);
        }
        &self.slots[&key]
    }

    /// Paint one page into `rect` (aspect-fit), rotated by `rotate` degrees clockwise.
    /// Only requests rendering when the rect is on screen. Returns the painted page rect.
    pub fn paint_page(
        &mut self,
        ui: &egui::Ui,
        path: &Path,
        page: u32,
        size: u32,
        rotate: i64,
        rect: Rect,
    ) -> Rect {
        let p = Palette::current(ui);
        if !ui.is_rect_visible(rect) {
            return rect;
        }
        let ctx = ui.ctx().clone();
        let slot = self.request(&ctx, path, page, size);
        match slot {
            Slot::Ready(tex) => {
                let [tw, th] = tex.size();
                let (mut w, mut h) = (tw as f32, th as f32);
                let quarter = rotate.rem_euclid(180) == 90;
                if quarter {
                    std::mem::swap(&mut w, &mut h);
                }
                let s = (rect.width() / w).min(rect.height() / h);
                let shown = Rect::from_center_size(rect.center(), Vec2::new(w * s, h * s));
                // The texture is drawn unrotated into `base` and rotated around its centre.
                let base = if quarter {
                    Rect::from_center_size(shown.center(), Vec2::new(shown.height(), shown.width()))
                } else {
                    shown
                };
                let painter = ui.painter();
                painter.rect_filled(
                    shown.translate(Vec2::new(0.0, 2.0)),
                    2.0,
                    Color32::from_black_alpha(45),
                );
                egui::Image::from_texture((tex.id(), base.size()))
                    .rotate((rotate as f32).to_radians(), Vec2::splat(0.5))
                    .paint_at(ui, base);
                painter.rect_stroke(
                    shown,
                    0.0,
                    Stroke::new(1.0, Color32::from_black_alpha(40)),
                    StrokeKind::Outside,
                );
                shown
            }
            Slot::Loading => {
                let page_rect = Rect::from_center_size(
                    rect.center(),
                    Vec2::new(rect.width().min(rect.height() * 0.75), rect.height()),
                );
                ui.painter().rect_filled(
                    page_rect,
                    2.0,
                    if p.dark {
                        Color32::from_gray(58)
                    } else {
                        Color32::from_gray(230)
                    },
                );
                ui.painter().text(
                    page_rect.center(),
                    Align2::CENTER_CENTER,
                    "…",
                    FontId::proportional(18.0),
                    p.text2,
                );
                page_rect
            }
            Slot::Failed => {
                let page_rect = Rect::from_center_size(
                    rect.center(),
                    Vec2::new(rect.width().min(rect.height() * 0.75), rect.height()),
                );
                ui.painter().rect_filled(
                    page_rect,
                    2.0,
                    if p.dark {
                        Color32::from_gray(58)
                    } else {
                        Color32::from_gray(230)
                    },
                );
                ui.painter().text(
                    page_rect.center(),
                    Align2::CENTER_CENTER,
                    "No preview",
                    FontId::proportional(11.0),
                    p.text2,
                );
                page_rect
            }
        }
    }

    // -------------------------------------------------------------------------------------
    // Viewer
    // -------------------------------------------------------------------------------------

    pub fn open_viewer(&mut self, items: Vec<ViewItem>, index: usize) {
        if !items.is_empty() {
            let index = index.min(items.len() - 1);
            self.viewer = Some(Viewer {
                items,
                index,
                zoom: 1.0,
            });
        }
    }

    pub fn viewer_open(&self) -> bool {
        self.viewer.is_some()
    }

    /// Draw the page viewer when open. Call once per frame, after the main UI.
    pub fn show_viewer(&mut self, ctx: &egui::Context) {
        let Some(v) = &self.viewer else { return };
        let (count, index, zoom) = (v.items.len(), v.index, v.zoom);
        let item = v.items[index].clone();

        let p = Palette::of(ctx.global_style().visuals.dark_mode);
        let screen = ctx.content_rect();
        let mut close = false;
        let mut go: isize = 0;
        let mut new_zoom = zoom;

        // Keyboard: arrows / PageUp / PageDown to move, +/- to zoom.
        ctx.input(|i| {
            if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::PageDown) {
                go = 1;
            }
            if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp) {
                go = -1;
            }
            if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                new_zoom = (zoom * 1.25).min(4.0);
            }
            if i.key_pressed(egui::Key::Minus) {
                new_zoom = (zoom / 1.25).max(1.0);
            }
        });

        let modal = egui::Modal::new(egui::Id::unique("page_viewer"))
            .backdrop_color(Color32::from_black_alpha(190))
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(8.0)
                    .inner_margin(16.0),
            )
            .show(ctx, |ui| {
                let size = Vec2::new(screen.width() * 0.86, screen.height() * 0.88);
                ui.set_width(size.x);
                ui.horizontal(|ui| {
                    ui.label(strong(file_label(&item.path)).size(15.0));
                    if item.page > 0 {
                        ui.label(secondary(ui, format!("Page {}", item.page)));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, true, I_CLOSE, "Close (Esc)") {
                            close = true;
                        }
                        ui.add_space(8.0);
                        if icon_button(ui, zoom < 4.0, I_ADD, "Zoom in (+)") {
                            new_zoom = (zoom * 1.25).min(4.0);
                        }
                        ui.label(secondary(ui, format!("{:.0}%", zoom * 100.0)));
                        if icon_button(ui, zoom > 1.0, I_REMOVE, "Zoom out (−)") {
                            new_zoom = (zoom / 1.25).max(1.0);
                        }
                        ui.add_space(8.0);
                        if icon_button(ui, index + 1 < count, I_RIGHT, "Next page (→)") {
                            go = 1;
                        }
                        ui.label(format!("{} / {}", index + 1, count));
                        if icon_button(ui, index > 0, I_LEFT, "Previous page (←)") {
                            go = -1;
                        }
                    });
                });
                ui.add_space(8.0);
                let view_h = size.y - 56.0;
                // Reserve the full viewer area: inside a pop-up the scroll area would
                // otherwise shrink and cut the page off.
                egui::ScrollArea::both()
                    .min_scrolled_height(view_h)
                    .max_height(view_h)
                    .min_scrolled_width(size.x)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let avail = Vec2::new(size.x - 12.0, view_h - 12.0);
                        let canvas = Vec2::new(avail.x * zoom.max(1.0), avail.y * zoom);
                        let (rect, _) = ui.allocate_exact_size(canvas, Sense::hover());
                        self.paint_page(ui, &item.path, item.page, LARGE, item.rotate, rect);
                    });
            });
        if modal.should_close() {
            close = true;
        }

        if let Some(v) = &mut self.viewer {
            v.zoom = new_zoom;
            v.index = (v.index as isize + go).clamp(0, v.items.len() as isize - 1) as usize;
        }
        if close {
            self.viewer = None;
        }
    }
}

fn file_label(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// Page grid widget
// ---------------------------------------------------------------------------------------

/// How one page tile should look.
#[derive(Default)]
pub struct TileStyle {
    /// Fade the page out (e.g. not part of the current selection).
    pub dim: bool,
    /// Small badge in the top-left corner (text, colour).
    pub badge: Option<(String, Color32)>,
    /// Accent outline.
    pub highlight: bool,
}

/// Page selection for [`page_grid`]: one flag per page plus rubber-band state.
#[derive(Default)]
pub struct PageSel {
    pub selected: Vec<bool>,
    marquee: Option<(egui::Pos2, Vec<bool>)>,
}

impl PageSel {
    /// Selected page numbers (1-based), in order.
    pub fn pages(&self) -> Vec<u32> {
        self.selected
            .iter()
            .enumerate()
            .filter(|(_, s)| **s)
            .map(|(i, _)| i as u32 + 1)
            .collect()
    }

    /// Select exactly these pages (1-based).
    pub fn set_pages(&mut self, pages: &std::collections::HashSet<u32>) {
        for (i, s) in self.selected.iter_mut().enumerate() {
            *s = pages.contains(&(i as u32 + 1));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        ui: &egui::Ui,
        clicked: Option<usize>,
        background: &egui::Response,
        rects: &[Rect],
        modifiers: egui::Modifiers,
        pointer: Option<egui::Pos2>,
        clip: Rect,
    ) {
        let p = Palette::current(ui);
        let count = self.selected.iter().filter(|s| **s).count();
        if let Some(i) = clicked {
            if modifiers.shift {
                let anchor = self.selected.iter().position(|s| *s).unwrap_or(i);
                let (a, b) = (anchor.min(i), anchor.max(i));
                self.selected
                    .iter_mut()
                    .enumerate()
                    .for_each(|(j, s)| *s = (a..=b).contains(&j));
            } else if modifiers.command {
                self.selected[i] ^= true;
            } else {
                let only = self.selected[i] && count == 1;
                self.selected.iter_mut().for_each(|s| *s = false);
                self.selected[i] = !only;
            }
        }
        if background.drag_started()
            && let Some(start) = background.interact_pointer_pos()
        {
            let base = if modifiers.command || modifiers.shift {
                self.selected.clone()
            } else {
                vec![false; self.selected.len()]
            };
            self.marquee = Some((start, base));
        }
        if let Some((start, base)) = &self.marquee {
            if let Some(pos) = pointer {
                let band = Rect::from_two_pos(*start, pos);
                for (i, s) in self.selected.iter_mut().enumerate() {
                    *s = base.get(i).copied().unwrap_or(false)
                        || rects.get(i).is_some_and(|r| r.intersects(band));
                }
                drag_painter(ui.ctx()).rect(
                    band.intersect(clip),
                    2.0,
                    p.accent.gamma_multiply(0.12),
                    Stroke::new(1.0, p.accent),
                    StrokeKind::Inside,
                );
                drag_autoscroll(ui, clip, pos, false);
                ui.ctx().request_repaint();
            }
            if !ui.input(|i| i.pointer.any_down()) {
                self.marquee = None;
            }
        } else if background.clicked() && !modifiers.command && !modifiers.shift {
            self.selected.iter_mut().for_each(|s| *s = false);
        }
        if !ui.ctx().egui_wants_keyboard_input() {
            let (all, clear) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                    count > 0 && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            if all {
                self.selected.iter_mut().for_each(|s| *s = true);
            }
            if clear {
                self.selected.iter_mut().for_each(|s| *s = false);
            }
        }
    }
}

/// Compact page-range text for a sorted page list, e.g. `[1,2,3,5,8,9]` → `"1-3, 5, 8-9"`.
pub fn ranges_text(pages: &[u32]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let start = pages[i];
        let mut end = start;
        while i + 1 < pages.len() && pages[i + 1] == end + 1 {
            i += 1;
            end = pages[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    parts.join(", ")
}

/// A wrapping grid of every page of `path`. `style` customises each tile, `overlay` can draw
/// on top of a page (it receives the page number and the on-screen rect of the page).
/// Clicking a page opens the viewer.
///
/// With `selection` (one flag per page), clicking selects pages instead: Ctrl+click,
/// Shift+click, dragging on empty space (rubber band), Ctrl+A and Esc work like in
/// Windows Explorer, and double-click opens the viewer.
pub fn page_grid(
    ui: &mut egui::Ui,
    thumbs: &mut Thumbs,
    path: &Path,
    pages: u32,
    mut selection: Option<&mut PageSel>,
    style: impl Fn(u32) -> TileStyle,
    overlay: impl Fn(&egui::Painter, u32, Rect),
) {
    let p = Palette::current(ui);
    if let Some(sel) = selection.as_deref_mut() {
        sel.selected.resize(pages as usize, false);
    }
    let selecting = selection.is_some();
    let modifiers = ui.input(|i| i.modifiers);
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let clip = ui.clip_rect();
    let mut rects = Vec::with_capacity(pages as usize);
    let mut clicked = None;
    ui.horizontal(|ui| {
        caption(
            ui,
            if selecting {
                "Click to select · Ctrl / Shift for several · drag on empty space to select · Ctrl+A all · Esc clear · double-click to view"
            } else {
                "Click a page to view it full size."
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Slider::new(&mut thumbs.zoom, 0.6..=2.2).show_value(false));
            caption(ui, "Thumbnail size");
        });
    });
    ui.add_space(4.0);

    let tile = Vec2::new(132.0, 176.0) * thumbs.zoom;
    let mut open = None;
    let background = egui::Frame::new()
        .fill(if p.dark {
            Color32::from_rgb(0x1C, 0x1C, 0x1C)
        } else {
            Color32::from_rgb(0xEB, 0xEB, 0xEB)
        })
        .corner_radius(8.0)
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Background for rubber-band selection; created before the tiles so tiles win.
            let background = selecting.then(|| {
                ui.interact(
                    ui.max_rect(),
                    ui.make_persistent_id(("page_grid_bg", path)),
                    Sense::click_and_drag(),
                )
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(14.0, 14.0);
                for page in 1..=pages {
                    let s = style(page);
                    let (rect, resp) = ui.allocate_exact_size(tile, Sense::click());
                    rects.push(rect);
                    let is_selected = selection
                        .as_deref()
                        .is_some_and(|sel| sel.selected[(page - 1) as usize]);
                    if is_selected {
                        ui.painter()
                            .rect_filled(rect, 6.0, p.accent.gamma_multiply(0.18));
                        ui.painter().rect_stroke(
                            rect,
                            6.0,
                            Stroke::new(2.0, p.accent),
                            StrokeKind::Inside,
                        );
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 6.0, p.card_hover);
                    }
                    let area = Rect::from_min_max(
                        rect.min + Vec2::splat(8.0),
                        rect.max - Vec2::new(8.0, 26.0),
                    );
                    let shown = thumbs.paint_page(ui, path, page, THUMB, 0, area);
                    overlay(ui.painter(), page, shown);
                    if s.dim {
                        ui.painter()
                            .rect_filled(shown, 0.0, p.bg.gamma_multiply(0.65));
                    }
                    if s.highlight {
                        ui.painter().rect_stroke(
                            shown.expand(3.0),
                            4.0,
                            Stroke::new(2.0, p.accent),
                            StrokeKind::Outside,
                        );
                    }
                    if let Some((text, color)) = &s.badge {
                        let galley = ui.painter().layout_no_wrap(
                            text.clone(),
                            FontId::proportional(11.0),
                            Color32::WHITE,
                        );
                        let r = Rect::from_min_size(
                            shown.min + Vec2::splat(4.0),
                            galley.size() + Vec2::new(10.0, 4.0),
                        );
                        ui.painter().rect_filled(r, 4.0, *color);
                        ui.painter()
                            .galley(r.min + Vec2::new(5.0, 2.0), galley, Color32::WHITE);
                    }
                    ui.painter().text(
                        rect.center_bottom() - Vec2::new(0.0, 6.0),
                        Align2::CENTER_BOTTOM,
                        page.to_string(),
                        FontId::proportional(12.0),
                        if s.dim {
                            p.text2.gamma_multiply(0.6)
                        } else {
                            p.text2
                        },
                    );
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if !selecting && resp.clicked() || selecting && resp.double_clicked() {
                        open = Some(page);
                    } else if selecting && resp.clicked() {
                        clicked = Some((page - 1) as usize);
                    }
                }
            });
            background
        })
        .inner;

    if let (Some(sel), Some(background)) = (selection, background) {
        sel.update(ui, clicked, &background, &rects, modifiers, pointer, clip);
    }

    if let Some(page) = open {
        let items = (1..=pages)
            .map(|n| ViewItem {
                path: path.to_path_buf(),
                page: n,
                rotate: 0,
            })
            .collect();
        thumbs.open_viewer(items, page as usize - 1);
    }
}

/// A single horizontal row of page thumbnails (used for compact file lists like Merge).
pub fn page_strip(
    ui: &mut egui::Ui,
    thumbs: &mut Thumbs,
    path: &Path,
    pages: u32,
    dim: impl Fn(u32) -> bool,
) {
    let p = Palette::current(ui);
    let tile = Vec2::new(78.0, 104.0);
    let mut open = None;
    egui::ScrollArea::horizontal()
        .id_salt(("strip", path))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for page in 1..=pages {
                    let (rect, resp) = ui.allocate_exact_size(tile, Sense::click());
                    let area = Rect::from_min_max(rect.min, rect.max - Vec2::new(0.0, 16.0));
                    let shown = thumbs.paint_page(ui, path, page, THUMB / 2, 0, area);
                    let dimmed = dim(page);
                    if dimmed {
                        ui.painter()
                            .rect_filled(shown, 0.0, p.card.gamma_multiply(0.7));
                    }
                    ui.painter().text(
                        rect.center_bottom(),
                        Align2::CENTER_BOTTOM,
                        page.to_string(),
                        FontId::proportional(11.0),
                        if dimmed {
                            p.text2.gamma_multiply(0.5)
                        } else {
                            p.text2
                        },
                    );
                    if resp.on_hover_text(format!("Page {page}")).clicked() {
                        open = Some(page);
                    }
                }
            });
        });
    if let Some(page) = open {
        let items = (1..=pages)
            .map(|n| ViewItem {
                path: path.to_path_buf(),
                page: n,
                rotate: 0,
            })
            .collect();
        thumbs.open_viewer(items, page as usize - 1);
    }
}

/// Section header for a page grid.
pub fn pages_header(ui: &mut egui::Ui, pages: u32) {
    section(ui, &format!("Pages ({pages})"));
}

/// Pages included by a page-range spec (empty spec = all). `None` if the spec is invalid.
pub fn selected_pages(spec: &str, total: u32) -> Option<HashSet<u32>> {
    crate::pdf::parse_ranges(spec, total)
        .ok()
        .map(|r| crate::pdf::expand_ranges(&r).into_iter().collect())
}

fn is_pdf(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

// ---------------------------------------------------------------------------------------
// Drag & drop helpers
// ---------------------------------------------------------------------------------------

/// Foreground painter for drag feedback (drawn above everything else).
pub fn drag_painter(ctx: &egui::Context) -> egui::Painter {
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("drag_feedback"),
    ))
}

/// Red "drop here to delete" zone along the bottom of `clip`. Returns true while `pos` is over it.
pub fn trash_zone(ctx: &egui::Context, clip: Rect, pos: egui::Pos2, what: &str) -> bool {
    let p = Palette::of(ctx.global_style().visuals.dark_mode);
    let zone = Rect::from_center_size(
        egui::pos2(clip.center().x, clip.bottom() - 44.0),
        Vec2::new(clip.width().min(420.0), 60.0),
    );
    let over = zone.contains(pos);
    let (fill, stroke) = if over {
        (p.error, p.error)
    } else {
        (p.error_bg, p.error.gamma_multiply(0.7))
    };
    let painter = drag_painter(ctx);
    painter.rect(
        zone,
        10.0,
        fill,
        Stroke::new(2.0, stroke),
        StrokeKind::Inside,
    );
    painter.text(
        zone.center(),
        Align2::CENTER_CENTER,
        format!("{I_DELETE}   Drop here to remove {what}"),
        FontId::new(15.0, semibold()),
        if over { Color32::WHITE } else { p.error },
    );
    over
}

/// Auto-scroll the surrounding scroll area while dragging near its top or bottom edge.
pub fn drag_autoscroll(ui: &egui::Ui, clip: Rect, pos: egui::Pos2, over_trash: bool) {
    if pos.y < clip.top() + 40.0 {
        ui.scroll_with_delta(Vec2::new(0.0, 12.0));
    } else if pos.y > clip.bottom() - 90.0 && !over_trash {
        ui.scroll_with_delta(Vec2::new(0.0, -12.0));
    }
}

/// Insertion index for a grid: before/after the tile nearest to `pos`.
pub fn grid_insert_index(rects: &[Rect], pos: egui::Pos2) -> usize {
    rects
        .iter()
        .enumerate()
        .min_by(|a, b| {
            a.1.center()
                .distance(pos)
                .total_cmp(&b.1.center().distance(pos))
        })
        .map(|(j, r)| if pos.x < r.center().x { j } else { j + 1 })
        .unwrap_or(0)
}

/// Draw the vertical accent marker where grid items will be inserted.
pub fn grid_insert_marker(ctx: &egui::Context, rects: &[Rect], at: usize) {
    let Some(last) = rects.last() else { return };
    let p = Palette::of(ctx.global_style().visuals.dark_mode);
    let (r, x) = if at < rects.len() {
        (rects[at], rects[at].left() - 7.0)
    } else {
        (*last, last.right() + 7.0)
    };
    drag_painter(ctx).line_segment(
        [
            egui::pos2(x, r.top() + 6.0),
            egui::pos2(x, r.bottom() - 6.0),
        ],
        Stroke::new(4.0, p.accent),
    );
}

/// Move element `from` so that it lands before index `at` (index in the original order).
pub fn move_item<T>(items: &mut Vec<T>, from: usize, at: usize) {
    if from >= items.len() {
        return;
    }
    let item = items.remove(from);
    let at = if at > from { at - 1 } else { at }.min(items.len());
    items.insert(at, item);
}

/// Move the elements at `set` (sorted indices) so they land before index `at`
/// (index in the original order), keeping their relative order.
pub fn move_items<T>(items: &mut Vec<T>, set: &[usize], at: usize) {
    let mut moving = Vec::with_capacity(set.len());
    let mut rest = Vec::with_capacity(items.len());
    for (i, it) in items.drain(..).enumerate() {
        if set.contains(&i) {
            moving.push(it)
        } else {
            rest.push(it)
        }
    }
    let before = set.iter().filter(|&&i| i < at).count();
    let at = (at - before).min(rest.len());
    rest.splice(at..at, moving);
    *items = rest;
}

/// Selection, drag and rubber-band state of a [`sortable_grid`].
#[derive(Default)]
pub struct GridSel {
    pub selected: Vec<bool>,
    drag: Option<Vec<usize>>,
    marquee: Option<(egui::Pos2, Vec<bool>)>,
}

impl GridSel {
    pub fn count(&self) -> usize {
        self.selected.iter().filter(|s| **s).count()
    }

    pub fn clear(&mut self) {
        self.selected.iter_mut().for_each(|s| *s = false);
    }
}

/// A grid of thumbnails (one per item) with Windows-style selection:
/// click, Ctrl+click, Shift+click, drag on empty space (rubber band), Ctrl+A, Esc,
/// Delete. Selected items can be dragged together to reorder them or onto the delete zone.
/// Double-click opens the viewer; the × button removes one item.
/// `describe` returns the item's preview source, a title and a subtitle.
pub fn sortable_grid<T>(
    ui: &mut egui::Ui,
    thumbs: &mut Thumbs,
    sel: &mut GridSel,
    items: &mut Vec<T>,
    what: &str,
    describe: impl Fn(&T) -> (ViewItem, String, String),
) {
    let p = Palette::current(ui);
    sel.selected.resize(items.len(), false);
    let nsel = sel.count();
    ui.horizontal(|ui| {
        caption(
            ui,
            &format!(
                "{} selected · drag to reorder · drag on empty space to select · Ctrl+A all · Del remove · double-click to view",
                nsel
            ),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Slider::new(&mut thumbs.zoom, 0.6..=2.2).show_value(false));
            caption(ui, "Thumbnail size");
        });
    });
    ui.add_space(4.0);

    let tile = Vec2::new(150.0, 196.0) * thumbs.zoom;
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let modifiers = ui.input(|i| i.modifiers);
    let clip = ui.clip_rect();
    let mut rects = Vec::with_capacity(items.len());
    let (mut started, mut clicked, mut open, mut remove) = (None, None, None, None);
    let dragging: Vec<usize> = sel.drag.clone().unwrap_or_default();

    let frame = egui::Frame::new()
        .fill(if p.dark {
            Color32::from_rgb(0x1C, 0x1C, 0x1C)
        } else {
            Color32::from_rgb(0xEB, 0xEB, 0xEB)
        })
        .corner_radius(8.0)
        .inner_margin(14.0);
    let frame_resp = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Background for rubber-band selection; created before the tiles so tiles win.
        let background = ui.interact(
            ui.max_rect(),
            ui.make_persistent_id(("grid_bg", what)),
            Sense::click_and_drag(),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(14.0, 14.0);
            for (i, it) in items.iter().enumerate() {
                let (view, title, sub) = describe(it);
                let (rect, resp) = ui.allocate_exact_size(tile, Sense::click_and_drag());
                rects.push(rect);
                let selected = sel.selected[i];
                let hovered = resp.hovered() && dragging.is_empty();
                if selected {
                    ui.painter()
                        .rect_filled(rect, 8.0, p.accent.gamma_multiply(0.18));
                    ui.painter().rect_stroke(
                        rect,
                        8.0,
                        Stroke::new(2.0, p.accent),
                        StrokeKind::Inside,
                    );
                } else if hovered {
                    ui.painter().rect_filled(rect, 8.0, p.card_hover);
                }
                let area = Rect::from_min_max(
                    rect.min + Vec2::splat(10.0),
                    rect.max - Vec2::new(10.0, 46.0),
                );
                thumbs.paint_page(ui, &view.path, view.page, THUMB, view.rotate, area);
                let painter = ui.painter().with_clip_rect(rect);
                let badge = Rect::from_min_size(rect.min + Vec2::splat(6.0), Vec2::new(26.0, 20.0));
                painter.rect_filled(badge, 4.0, p.accent);
                painter.text(
                    badge.center(),
                    Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    FontId::new(11.0, semibold()),
                    p.on_accent,
                );
                painter.text(
                    egui::pos2(rect.left() + 10.0, rect.bottom() - 38.0),
                    Align2::LEFT_TOP,
                    title,
                    FontId::proportional(12.0),
                    p.text,
                );
                painter.text(
                    egui::pos2(rect.left() + 10.0, rect.bottom() - 20.0),
                    Align2::LEFT_TOP,
                    sub,
                    FontId::proportional(11.0),
                    p.text2,
                );
                if dragging.contains(&i) {
                    ui.painter()
                        .rect_filled(rect, 8.0, p.bg.gamma_multiply(0.75));
                }
                // × remove button (on hover).
                let x_rect = Rect::from_min_size(
                    rect.right_top() + Vec2::new(-30.0, 6.0),
                    Vec2::splat(24.0),
                );
                let x_resp = ui.interact(
                    x_rect,
                    ui.make_persistent_id(("remove", what, i)),
                    Sense::click(),
                );
                if hovered || x_resp.hovered() {
                    let fill = if x_resp.hovered() { p.error } else { p.card };
                    ui.painter()
                        .circle(x_rect.center(), 12.0, fill, Stroke::new(1.0, p.border));
                    ui.painter().text(
                        x_rect.center(),
                        Align2::CENTER_CENTER,
                        I_CLOSE,
                        FontId::proportional(10.0),
                        if x_resp.hovered() {
                            Color32::WHITE
                        } else {
                            p.text
                        },
                    );
                }
                if x_resp.on_hover_text("Remove").clicked() {
                    remove = Some(i);
                } else if resp.drag_started() {
                    started = Some(i);
                } else if resp.double_clicked() {
                    open = Some(i);
                } else if resp.clicked() {
                    clicked = Some(i);
                }
            }
        });
        background
    });
    let background = frame_resp.inner;

    // --- Click selection ---------------------------------------------------------------
    if let Some(i) = clicked {
        if modifiers.shift {
            let anchor = sel.selected.iter().position(|s| *s).unwrap_or(i);
            let (a, b) = (anchor.min(i), anchor.max(i));
            sel.selected
                .iter_mut()
                .enumerate()
                .for_each(|(j, s)| *s = (a..=b).contains(&j));
        } else if modifiers.command {
            sel.selected[i] ^= true;
        } else {
            let only = sel.selected[i] && nsel == 1;
            sel.clear();
            sel.selected[i] = !only;
        }
    }

    // --- Rubber-band selection -----------------------------------------------------------
    if background.drag_started()
        && sel.drag.is_none()
        && let Some(start) = background.interact_pointer_pos()
    {
        let base = if modifiers.command || modifiers.shift {
            sel.selected.clone()
        } else {
            vec![false; items.len()]
        };
        sel.marquee = Some((start, base));
    }
    if let Some((start, base)) = &sel.marquee {
        if let Some(pos) = pointer {
            let band = Rect::from_two_pos(*start, pos);
            for (i, s) in sel.selected.iter_mut().enumerate() {
                *s = base.get(i).copied().unwrap_or(false)
                    || rects.get(i).is_some_and(|r| r.intersects(band));
            }
            drag_painter(ui.ctx()).rect(
                band.intersect(clip),
                2.0,
                p.accent.gamma_multiply(0.12),
                Stroke::new(1.0, p.accent),
                StrokeKind::Inside,
            );
            ui.ctx().request_repaint();
        }
        if !ui.input(|i| i.pointer.any_down()) {
            sel.marquee = None;
        }
    } else if background.clicked() && !modifiers.command && !modifiers.shift {
        sel.clear();
    }

    // --- Drag to reorder / delete (the whole selection moves together) -----------------
    if let Some(i) = started {
        let set: Vec<usize> = if sel.selected[i] {
            (0..items.len()).filter(|&j| sel.selected[j]).collect()
        } else {
            sel.clear();
            sel.selected[i] = true;
            vec![i]
        };
        sel.drag = Some(set);
    }
    if let (Some(set), Some(pos)) = (sel.drag.clone(), pointer) {
        let ctx = ui.ctx().clone();
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let label = if set.len() == 1 {
            what.to_string()
        } else {
            format!("{} {what}s", set.len())
        };
        let over_trash = trash_zone(&ctx, clip, pos, &label);
        let insert = (!over_trash).then(|| grid_insert_index(&rects, pos));
        if let Some(at) = insert {
            grid_insert_marker(&ctx, &rects, at);
        }
        if let Some(it) = items.get(set[0]) {
            let (view, _, _) = describe(it);
            egui::Area::new(egui::Id::unique("sortable_ghost"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + Vec2::new(12.0, 12.0))
                .interactable(false)
                .show(&ctx, |ui| {
                    let (r, _) = ui.allocate_exact_size(tile * 0.7, Sense::hover());
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
                        &view.path,
                        view.page,
                        THUMB,
                        view.rotate,
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
        }
        drag_autoscroll(ui, clip, pos, over_trash);
        if ui.input(|i| i.pointer.any_released()) {
            if over_trash {
                remove_items(items, &mut sel.selected, &set);
            } else if let Some(at) = insert {
                move_items(items, &set, at);
                move_items(&mut sel.selected, &set, at);
            }
            sel.drag = None;
        }
        ctx.request_repaint();
    } else if sel.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
        sel.drag = None;
    }

    // --- Keyboard shortcuts ----------------------------------------------------------------
    if sel.drag.is_none() && !ui.ctx().egui_wants_keyboard_input() {
        let (all, clear, delete) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                nsel > 0 && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                nsel > 0 && i.key_pressed(egui::Key::Delete),
            )
        });
        if all {
            sel.selected.iter_mut().for_each(|s| *s = true);
        }
        if clear {
            sel.clear();
        }
        if delete {
            let set: Vec<usize> = (0..items.len()).filter(|&j| sel.selected[j]).collect();
            remove_items(items, &mut sel.selected, &set);
        }
    }

    if let Some(i) = remove {
        remove_items(items, &mut sel.selected, &[i]);
    } else if let Some(i) = open {
        let views = items.iter().map(|it| describe(it).0).collect();
        thumbs.open_viewer(views, i);
    }
}

/// Remove the items at `set` (sorted indices) together with their selection flags.
fn remove_items<T>(items: &mut Vec<T>, selected: &mut Vec<bool>, set: &[usize]) {
    let mut k = 0;
    items.retain(|_| {
        k += 1;
        !set.contains(&(k - 1))
    });
    let mut k = 0;
    selected.retain(|_| {
        k += 1;
        !set.contains(&(k - 1))
    });
}

#[cfg(test)]
mod tests {
    use super::move_item;

    #[test]
    fn ranges_text_compacts() {
        assert_eq!(super::ranges_text(&[1, 2, 3, 5, 8, 9]), "1-3, 5, 8-9");
        assert_eq!(super::ranges_text(&[]), "");
        assert_eq!(super::ranges_text(&[4]), "4");
    }

    #[test]
    fn move_items_groups() {
        let mut v = vec![1, 2, 3, 4, 5];
        super::move_items(&mut v, &[1, 3], 5);
        assert_eq!(v, [1, 3, 5, 2, 4]);
        let mut v = vec![1, 2, 3, 4, 5];
        super::move_items(&mut v, &[3, 4], 0);
        assert_eq!(v, [4, 5, 1, 2, 3]);
    }

    #[test]
    fn move_item_positions() {
        let mut v = vec![1, 2, 3, 4];
        move_item(&mut v, 0, 3);
        assert_eq!(v, [2, 3, 1, 4]);
        let mut v = vec![1, 2, 3, 4];
        move_item(&mut v, 3, 0);
        assert_eq!(v, [4, 1, 2, 3]);
        let mut v = vec![1, 2, 3, 4];
        move_item(&mut v, 1, 4);
        assert_eq!(v, [1, 3, 4, 2]);
        let mut v = vec![1, 2, 3, 4];
        move_item(&mut v, 2, 2);
        assert_eq!(v, [1, 2, 3, 4]);
    }
}
