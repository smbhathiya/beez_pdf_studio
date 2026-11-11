//! Windows 11 (Fluent) look: Segoe UI fonts, Segoe Fluent Icons, Mica-like neutral
//! surfaces, 4px control / 8px card corners, accent-coloured primary buttons.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText, Stroke, TextStyle,
    Vec2,
};
use std::sync::Arc;

// --- Segoe Fluent Icons code points (same values in Segoe MDL2 Assets on Windows 10) ---
pub const I_HOME: &str = "\u{E80F}";
pub const I_BACK: &str = "\u{E72B}";
pub const I_ADD: &str = "\u{E710}";
pub const I_CLOSE: &str = "\u{E711}";
pub const I_DELETE: &str = "\u{E74D}";
pub const I_UP: &str = "\u{E74A}";
pub const I_DOWN: &str = "\u{E74B}";
pub const I_LEFT: &str = "\u{E76B}";
pub const I_RIGHT: &str = "\u{E76C}";
pub const I_ROTATE: &str = "\u{E7AD}";
pub const I_COPY: &str = "\u{E8C8}";
pub const I_SELECT_ALL: &str = "\u{E8B3}";
pub const I_SORT: &str = "\u{E8CB}";
pub const I_OPEN: &str = "\u{E8E5}";
pub const I_FOLDER: &str = "\u{E838}";
pub const I_SAVE: &str = "\u{E74E}";
pub const I_LOCK: &str = "\u{E72E}";
pub const I_UNLOCK: &str = "\u{E785}";
pub const I_LINK: &str = "\u{E71B}";
pub const I_INFO: &str = "\u{E946}";
pub const I_PHOTO: &str = "\u{EB9F}";
pub const I_CUT: &str = "\u{E8C6}";
pub const I_GRID: &str = "\u{E8A9}";
pub const I_PACKAGE: &str = "\u{E7B8}";
pub const I_TEXT: &str = "\u{E8D2}";
pub const I_MERGE: &str = "\u{E8F1}";
pub const I_SUN: &str = "\u{E706}";
pub const I_MOON: &str = "\u{E708}";
pub const I_CHECK: &str = "\u{E73E}";
pub const I_ERROR: &str = "\u{E783}";
pub const I_DOCUMENT: &str = "\u{E8A5}";
pub const I_CLEAR: &str = "\u{E894}";

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Load Windows fonts when present; otherwise keep egui's built-in fonts.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var("WINDIR").map(|w| format!("{w}\\Fonts\\")).unwrap_or_else(|_| "C:\\Windows\\Fonts\\".into());
    let mut load = |name: &str, files: &[&str]| -> bool {
        for f in files {
            if let Ok(bytes) = std::fs::read(format!("{dir}{f}")) {
                fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                return true;
            }
        }
        false
    };
    let ui = load("segoe", &["segoeui.ttf"]);
    let sb = load("segoe-sb", &["seguisb.ttf", "segoeuib.ttf"]);
    let icons = load("fluent", &["SegoeIcons.ttf", "segmdl2.ttf"]);

    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut body = Vec::new();
    if ui {
        body.push("segoe".to_string());
    }
    if icons {
        body.push("fluent".to_string());
    }
    body.extend(defaults.iter().cloned());

    let mut heading = Vec::new();
    if sb {
        heading.push("segoe-sb".to_string());
    }
    heading.extend(body.iter().cloned());

    fonts.families.insert(FontFamily::Proportional, body);
    fonts.families.insert(semibold(), heading);
    ctx.set_fonts(fonts);
}

/// Colour palette for the current mode.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub success: Color32,
    pub success_bg: Color32,
    pub error: Color32,
    pub error_bg: Color32,
}

impl Palette {
    pub fn of(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                bg: Color32::from_rgb(0x20, 0x20, 0x20),
                card: Color32::from_rgb(0x2B, 0x2B, 0x2B),
                card_hover: Color32::from_rgb(0x32, 0x32, 0x32),
                border: Color32::from_rgb(0x1C, 0x1C, 0x1C),
                text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                text2: Color32::from_rgb(0xC5, 0xC5, 0xC5),
                accent: Color32::from_rgb(0x60, 0xCD, 0xFF),
                on_accent: Color32::BLACK,
                success: Color32::from_rgb(0x6C, 0xCB, 0x5F),
                success_bg: Color32::from_rgb(0x39, 0x3D, 0x1B),
                error: Color32::from_rgb(0xFF, 0x99, 0xA4),
                error_bg: Color32::from_rgb(0x44, 0x27, 0x26),
            }
        } else {
            Self {
                dark,
                bg: Color32::from_rgb(0xF3, 0xF3, 0xF3),
                card: Color32::from_rgb(0xFB, 0xFB, 0xFB),
                card_hover: Color32::from_rgb(0xF6, 0xF6, 0xF6),
                border: Color32::from_rgb(0xE5, 0xE5, 0xE5),
                text: Color32::from_rgb(0x1A, 0x1A, 0x1A),
                text2: Color32::from_rgb(0x5D, 0x5D, 0x5D),
                accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
                on_accent: Color32::WHITE,
                success: Color32::from_rgb(0x0F, 0x7B, 0x0F),
                success_bg: Color32::from_rgb(0xDF, 0xF6, 0xDD),
                error: Color32::from_rgb(0xC4, 0x2B, 0x1C),
                error_bg: Color32::from_rgb(0xFD, 0xE7, 0xE9),
            }
        }
    }

    pub fn current(ui: &egui::Ui) -> Self {
        Self::of(ui.visuals().dark_mode)
    }
}

/// Install Fluent visuals for both light and dark mode, plus spacing and text sizes.
pub fn install_theme(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
    ctx.all_styles_mut(|s| {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r4 = CornerRadius::same(4);

    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.faint_bg_color = if dark { Color32::from_rgb(0x27, 0x27, 0x27) } else { Color32::from_rgb(0xF7, 0xF7, 0xF7) };
    v.extreme_bg_color = if dark { Color32::from_rgb(0x1F, 0x1F, 0x1F) } else { Color32::WHITE };
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.45 } else { 0.25 });
    v.selection.stroke = Stroke::new(1.0, p.accent);

    let control = if dark { Color32::from_rgb(0x2D, 0x2D, 0x2D) } else { Color32::from_rgb(0xFB, 0xFB, 0xFB) };
    let control_hover = if dark { Color32::from_rgb(0x33, 0x33, 0x33) } else { Color32::from_rgb(0xF5, 0xF5, 0xF5) };
    let control_press = if dark { Color32::from_rgb(0x28, 0x28, 0x28) } else { Color32::from_rgb(0xF0, 0xF0, 0xF0) };
    let stroke = if dark { Color32::from_rgb(0x3A, 0x3A, 0x3A) } else { Color32::from_rgb(0xD9, 0xD9, 0xD9) };

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, if dark { Color32::from_rgb(0x33, 0x33, 0x33) } else { p.border });
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r4;
    for (wv, bg) in [(&mut w.inactive, control), (&mut w.hovered, control_hover), (&mut w.active, control_press)] {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::new(1.0, stroke);
        wv.fg_stroke = Stroke::new(1.0, p.text);
        wv.corner_radius = r4;
        wv.expansion = 0.0;
    }
    w.open = w.active;
    v
}

fn __spacing_marker() {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 5.0);
        s.spacing.interact_size = Vec2::new(40.0, 30.0);
        s.spacing.slider_width = 220.0;
        s.spacing.combo_width = 220.0;
        s.spacing.text_edit_width = 280.0;
        s.spacing.window_margin = Margin::same(16);
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
            (TextStyle::Heading, FontId::new(28.0, semibold())),
        ]
        .into();
    });
}

// ---------------------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------------------

pub fn title(text: &str) -> RichText {
    RichText::new(text).family(semibold()).size(28.0)
}

pub fn subtitle(text: &str) -> RichText {
    RichText::new(text).family(semibold()).size(20.0)
}

pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).family(semibold())
}

pub fn secondary(ui: &egui::Ui, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(Palette::current(ui).text2)
}

pub fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(secondary(ui, text).size(12.0));
}

/// Icon + label, e.g. `labeled(I_ADD, "Add files")`.
pub fn labeled(icon: &str, text: &str) -> String {
    format!("{icon}   {text}")
}

/// Filled accent button (the "primary action").
pub fn accent_button(ui: &mut egui::Ui, enabled: bool, text: &str) -> bool {
    let p = Palette::current(ui);
    let fill = if enabled { p.accent } else { p.accent.gamma_multiply(0.4) };
    let btn = egui::Button::new(strong(text).color(p.on_accent))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(4.0)
        .min_size(Vec2::new(140.0, 34.0));
    ui.add_enabled(enabled, btn).clicked()
}

/// Flat command-bar button (no frame until hovered).
pub fn command(ui: &mut egui::Ui, enabled: bool, icon: &str, text: &str) -> bool {
    let btn = egui::Button::new(labeled(icon, text)).frame_when_inactive(false).min_size(Vec2::new(0.0, 32.0));
    ui.add_enabled(enabled, btn).clicked()
}

/// Small square icon-only button with tooltip.
pub fn icon_button(ui: &mut egui::Ui, enabled: bool, icon: &str, tip: &str) -> bool {
    let btn = egui::Button::new(RichText::new(icon).size(13.0)).frame_when_inactive(false).min_size(Vec2::splat(30.0));
    ui.add_enabled(enabled, btn).on_hover_text(tip).clicked()
}

/// A Fluent card: subtle filled surface with a 1px border and 8px corners.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = Palette::current(ui);
    let r = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(20, 16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(4.0);
    r
}

/// Section heading above a group of cards.
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(strong(text).size(15.0));
    ui.add_space(2.0);
}

/// Windows Settings style row: title + description on the left, control on the right.
pub fn setting<R>(ui: &mut egui::Ui, icon: &str, name: &str, desc: &str, control: impl FnOnce(&mut egui::Ui) -> R) -> R {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            if !icon.is_empty() {
                ui.label(RichText::new(icon).size(18.0));
                ui.add_space(10.0);
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(name);
                if !desc.is_empty() {
                    caption(ui, desc);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control).inner
        })
        .inner
    })
}
