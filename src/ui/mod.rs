//! Application shell: Home (tool launcher) → full-window tool pages with a back button,
//! a Windows 11 style InfoBar for results, and background task handling.

mod theme;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::Result;
use eframe::egui::{
    self, Align2, Color32, FontId, Margin, RichText, Sense, Stroke, StrokeKind, Vec2,
};

use theme::*;
use tools::*;

struct Running {
    label: String,
    rx: mpsc::Receiver<Result<String>>,
}

pub struct App {
    /// `None` = Home screen.
    tool: Option<Tool>,
    running: Option<Running>,
    /// Last result: (success, message).
    status: Option<(bool, String)>,
    merge: MergeState,
    split: SplitState,
    organize: OrganizeState,
    compress: CompressState,
    images: ImagesState,
    links: LinksState,
    meta: MetaState,
    text: TextState,
    protect: ProtectState,
    unlock: UnlockState,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        install_theme(&cc.egui_ctx);
        let mut app = Self {
            tool: None,
            running: None,
            status: None,
            merge: Default::default(),
            split: Default::default(),
            organize: Default::default(),
            compress: Default::default(),
            images: Default::default(),
            links: Default::default(),
            meta: Default::default(),
            text: Default::default(),
            protect: Default::default(),
            unlock: Default::default(),
        };
        // Files passed on the command line (e.g. "Open with…").
        let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !args.is_empty() {
            app.open_files(None, args);
        }
        app
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
        self.status = None;
        self.running = Some(Running { label, rx });
    }

    fn poll(&mut self) {
        let Some(r) = &self.running else { return };
        let result = match r.rx.try_recv() {
            Ok(Ok(msg)) => (true, msg),
            Ok(Err(e)) => (false, format!("{e:#}")),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                (false, "The task stopped unexpectedly".into())
            }
        };
        self.status = Some(result);
        self.running = None;
    }

    /// Route opened/dropped files to a tool. On Home, the file type picks the tool.
    fn open_files(&mut self, tool: Option<Tool>, files: Vec<PathBuf>) {
        let pdfs: Vec<PathBuf> = files
            .iter()
            .filter(|p| has_ext(p, PDF_EXT))
            .cloned()
            .collect();
        let images: Vec<PathBuf> = files
            .iter()
            .filter(|p| has_ext(p, IMG_EXT))
            .cloned()
            .collect();
        let tool = tool.unwrap_or(if pdfs.is_empty() && !images.is_empty() {
            Tool::ImagesToPdf
        } else {
            Tool::Organize
        });
        self.tool = Some(tool);
        let first = pdfs.first().cloned();
        match tool {
            Tool::Merge => self.merge.add(pdfs),
            Tool::Organize => self.organize.add(pdfs),
            Tool::ImagesToPdf => self.images.add(images),
            Tool::Unlock => {
                if first.is_some() {
                    self.unlock.path = first;
                }
            }
            _ => {
                let Some(p) = first else { return };
                match tool {
                    Tool::Split => self.split.src.set(p),
                    Tool::Compress => self.compress.src.set(p),
                    Tool::ExtractText => self.text.src.set(p),
                    Tool::Protect => self.protect.src.set(p),
                    Tool::Links => {
                        self.links.src.set(p);
                        self.links.load();
                    }
                    Tool::Metadata => {
                        self.meta.src.set(p);
                        self.meta.load();
                    }
                    _ => {}
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------
    // Home
    // ---------------------------------------------------------------------------------

    fn home(&mut self, ui: &mut egui::Ui) {
        let p = Palette::current(ui);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(title("PDF Studio"));
                ui.label(secondary(ui, "Choose a tool to get started. Your files are processed on this PC and never uploaded."));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme_toggle(ui);
            });
        });

        // Drop zone
        ui.add_space(20.0);
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 96.0), Sense::click());
        let hovered = resp.hovered();
        ui.painter()
            .rect_filled(rect, 8.0, if hovered { p.card_hover } else { p.card });
        ui.painter().rect_stroke(
            rect,
            8.0,
            Stroke::new(
                1.0,
                if hovered {
                    p.accent
                } else {
                    p.text2.gamma_multiply(0.5)
                },
            ),
            StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center() - Vec2::new(0.0, 12.0),
            Align2::CENTER_CENTER,
            format!("{I_OPEN}   Open or drop files here"),
            FontId::new(16.0, semibold()),
            p.text,
        );
        ui.painter().text(
            rect.center() + Vec2::new(0.0, 14.0),
            Align2::CENTER_CENTER,
            "PDFs open in the Page Organizer and images open in Images to PDF",
            FontId::proportional(12.0),
            p.text2,
        );
        if resp.clicked() {
            let files = rfd::FileDialog::new()
                .add_filter("PDFs and images", &[PDF_EXT, IMG_EXT].concat())
                .pick_files()
                .unwrap_or_default();
            if !files.is_empty() {
                self.open_files(None, files);
            }
        }

        for (group, tools) in Tool::GROUPS {
            ui.add_space(24.0);
            ui.label(strong(group).size(15.0));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(12.0);
                for &t in tools {
                    if tool_card(ui, t) {
                        self.tool = Some(t);
                    }
                }
            });
        }
        ui.add_space(24.0);
    }

    // ---------------------------------------------------------------------------------
    // Tool page
    // ---------------------------------------------------------------------------------

    fn tool_page(&mut self, ui: &mut egui::Ui, tool: Tool) -> Option<Task> {
        // Header: back button + breadcrumb title
        ui.horizontal(|ui| {
            let back = egui::Button::new(RichText::new(I_BACK).size(14.0))
                .frame_when_inactive(false)
                .min_size(Vec2::splat(36.0));
            if ui.add(back).on_hover_text("Back to Home (Esc)").clicked() {
                self.tool = None;
            }
            ui.add_space(4.0);
            let home = ui.add(
                egui::Label::new(title("Home").color(Palette::current(ui).text2))
                    .sense(Sense::click()),
            );
            if home
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.tool = None;
            }
            ui.label(
                RichText::new(format!("  {I_RIGHT}  "))
                    .size(14.0)
                    .color(Palette::current(ui).text2),
            );
            ui.label(title(tool.name()));
        });
        ui.label(secondary(ui, tool.description()));
        ui.add_space(12.0);

        let body = |app: &mut Self, ui: &mut egui::Ui| match tool {
            Tool::Organize => app.organize.ui(ui),
            Tool::Merge => app.merge.ui(ui),
            Tool::Split => app.split.ui(ui),
            Tool::Compress => app.compress.ui(ui),
            Tool::ImagesToPdf => app.images.ui(ui),
            Tool::Links => app.links.ui(ui),
            Tool::Metadata => app.meta.ui(ui),
            Tool::ExtractText => app.text.ui(ui),
            Tool::Protect => app.protect.ui(ui),
            Tool::Unlock => app.unlock.ui(ui),
        };
        let busy = self.running.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            if tool == Tool::Organize {
                // The organizer fills the window and scrolls its own page grid.
                body(self, ui)
            } else {
                // A separate scroll position for each tool.
                egui::ScrollArea::vertical()
                    .id_salt(tool.name())
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(920.0);
                        let r = body(self, ui);
                        ui.add_space(24.0);
                        r
                    })
                    .inner
            }
        })
        .inner
    }

    /// Windows 11 InfoBar with progress / success / error.
    fn info_bar(&mut self, ui: &mut egui::Ui) {
        let p = Palette::current(ui);
        if let Some(r) = &self.running {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(strong(format!("{}…", r.label)));
                });
                ui.add(
                    egui::ProgressBar::new(0.0)
                        .animate(true)
                        .desired_height(4.0),
                );
            });
            return;
        }
        let Some((ok, msg)) = self.status.clone() else {
            return;
        };
        let (bg, fg, icon, head) = if ok {
            (p.success_bg, p.success, I_CHECK, "Done")
        } else {
            (p.error_bg, p.error, I_ERROR, "Something went wrong")
        };
        let mut close = false;
        egui::Frame::new()
            .fill(bg)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(8.0)
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).color(fg).size(16.0));
                    ui.add_space(6.0);
                    ui.label(strong(head));
                    // Show the produced file name rather than the whole path.
                    let (text, out) = match msg.rsplit_once(" → ") {
                        Some((t, path)) => (t.to_string(), Some(PathBuf::from(path.trim()))),
                        None => (msg.clone(), None),
                    };
                    ui.add(egui::Label::new(text).wrap());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, true, I_CLOSE, "Dismiss") {
                            close = true;
                        }
                        if let Some(out) = out.filter(|_| ok) {
                            if out.is_file() && ui.button(labeled(I_OPEN, "Open file")).clicked() {
                                let _ = open_path(&out);
                            }
                            let dir = if out.is_dir() {
                                Some(out.clone())
                            } else {
                                out.parent().map(Path::to_path_buf)
                            };
                            if let Some(dir) = dir
                                && ui.button(labeled(I_FOLDER, "Show in folder")).clicked() {
                                    let _ = open_path(&dir);
                                }
                        }
                    });
                });
            });
        if close {
            self.status = None;
        }
    }
}

fn theme_toggle(ui: &mut egui::Ui) {
    let dark = ui.visuals().dark_mode;
    let (icon, tip) = if dark {
        (I_SUN, "Switch to light mode")
    } else {
        (I_MOON, "Switch to dark mode")
    };
    if icon_button(ui, true, icon, tip) {
        ui.ctx().set_theme(if dark {
            egui::Theme::Light
        } else {
            egui::Theme::Dark
        });
    }
}

/// A launcher card on the Home screen. Returns true when clicked.
fn tool_card(ui: &mut egui::Ui, tool: Tool) -> bool {
    let p = Palette::current(ui);
    let width = ((ui.available_width() - 24.0) / 3.0).clamp(240.0, 360.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 92.0), Sense::click());
    let painter = ui.painter();
    let hovered = resp.hovered();
    painter.rect_filled(rect, 8.0, if hovered { p.card_hover } else { p.card });
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

    let icon_box = egui::Rect::from_min_size(rect.min + Vec2::new(16.0, 16.0), Vec2::splat(40.0));
    painter.rect_filled(
        icon_box,
        6.0,
        p.accent.gamma_multiply(if p.dark { 0.18 } else { 0.12 }),
    );
    painter.text(
        icon_box.center(),
        Align2::CENTER_CENTER,
        tool.icon(),
        FontId::proportional(20.0),
        p.accent,
    );

    let x = icon_box.right() + 14.0;
    painter.text(
        egui::pos2(x, rect.top() + 16.0),
        Align2::LEFT_TOP,
        tool.name(),
        FontId::new(15.0, semibold()),
        p.text,
    );
    let desc = painter.layout(
        tool.description().to_string(),
        FontId::proportional(12.0),
        p.text2,
        rect.right() - x - 16.0,
    );
    painter.galley(egui::pos2(x, rect.top() + 40.0), desc, p.text2);
    if hovered {
        painter.text(
            rect.right_top() + Vec2::new(-14.0, 18.0),
            Align2::RIGHT_TOP,
            I_RIGHT,
            FontId::proportional(11.0),
            p.text2,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn open_path(p: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(p)
        .spawn()
        .map(|_| ())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.open_files(self.tool, dropped);
        }
        if self.tool.is_some()
            && self.running.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.tool = None;
        }

        let bg = Palette::current(ui).bg;
        if self.running.is_some() || self.status.is_some() {
            egui::Panel::bottom("infobar")
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin {
                    left: 40,
                    right: 40,
                    top: 8,
                    bottom: 16,
                }))
                .show(ui, |ui| self.info_bar(ui));
        }

        let mut action = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg).inner_margin(Margin {
                left: 40,
                right: 40,
                top: 28,
                bottom: 16,
            }))
            .show(ui, |ui| match self.tool {
                None => {
                    egui::ScrollArea::vertical()
                        .id_salt("home")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.home(ui));
                }
                Some(tool) => action = self.tool_page(ui, tool),
            });

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let p = Palette::current(ui);
            let rect = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop"),
            ));
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(150));
            let target = self.tool.map(Tool::name).unwrap_or("PDF Studio");
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("{I_OPEN}   Drop to open in {target}"),
                FontId::new(26.0, semibold()),
                p.accent,
            );
        }

        if let Some(t) = action {
            self.start(&ctx, t);
        }
    }
}
