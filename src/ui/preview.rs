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
            let worker = self.workers.entry(path.to_path_buf()).or_insert_with(|| {
                let ctx = ctx.clone();
                render::spawn_worker(path.to_path_buf(), self.tx.clone(), move || {
                    ctx.request_repaint()
                })
            });
            let _ = worker.send(key.1);
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
                    ui.label(secondary(ui, format!("Page {}", item.page)));
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
                egui::ScrollArea::both()
                    .max_height(view_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let avail = Vec2::new(ui.available_width(), view_h - 8.0);
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

/// A wrapping grid of every page of `path`. `style` customises each tile, `overlay` can draw
/// on top of a page (it receives the page number and the on-screen rect of the page).
/// Clicking a page opens the viewer.
pub fn page_grid(
    ui: &mut egui::Ui,
    thumbs: &mut Thumbs,
    path: &Path,
    pages: u32,
    style: impl Fn(u32) -> TileStyle,
    overlay: impl Fn(&egui::Painter, u32, Rect),
) {
    let p = Palette::current(ui);
    ui.horizontal(|ui| {
        caption(ui, "Click a page to view it full size.");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Slider::new(&mut thumbs.zoom, 0.6..=2.2).show_value(false));
            caption(ui, "Thumbnail size");
        });
    });
    ui.add_space(4.0);

    let tile = Vec2::new(132.0, 176.0) * thumbs.zoom;
    let mut open = None;
    egui::Frame::new()
        .fill(if p.dark {
            Color32::from_rgb(0x1C, 0x1C, 0x1C)
        } else {
            Color32::from_rgb(0xEB, 0xEB, 0xEB)
        })
        .corner_radius(8.0)
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(14.0, 14.0);
                for page in 1..=pages {
                    let s = style(page);
                    let (rect, resp) = ui.allocate_exact_size(tile, Sense::click());
                    if resp.hovered() {
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
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
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
