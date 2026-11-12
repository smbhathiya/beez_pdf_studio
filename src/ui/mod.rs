//! Application shell: Home (tool launcher) → full-window tool pages with a back button,
//! a Windows 11 style InfoBar for results, and background task handling.

mod preview;
mod theme;
mod tools;
mod update;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::Result;
use eframe::egui::{
    self, Align2, Color32, FontId, Margin, RichText, Sense, Stroke, StrokeKind, Vec2,
};

use crate::pdf::progress::{Progress, is_cancelled};
use preview::Thumbs;
use theme::*;
use tools::*;
use update::UpdateUi;

struct Running {
    label: String,
    rx: mpsc::Receiver<Result<String>>,
    /// Shared with the task: progress reports flow in, the cancel request flows out.
    progress: Progress,
    started: std::time::Instant,
}

/// How the last task ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Done,
    Failed,
    Cancelled,
}

pub struct App {
    /// `None` = Home screen.
    tool: Option<Tool>,
    running: Option<Running>,
    /// Last result: (success, message).
    status: Option<(Outcome, String)>,
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
    update: UpdateUi,
    /// Page thumbnails and the full-size page viewer, shared by all tools.
    thumbs: Thumbs,
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
            update: UpdateUi::new(&cc.egui_ctx),
            thumbs: Thumbs::default(),
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
        let progress = Progress::default();
        let task_progress = progress.clone();
        std::thread::spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&task_progress)))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Unexpected internal error")));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.status = None;
        self.running = Some(Running {
            label,
            rx,
            progress,
            started: std::time::Instant::now(),
        });
    }

    fn poll(&mut self) {
        let Some(r) = &self.running else { return };
        let result = match r.rx.try_recv() {
            Ok(Ok(msg)) => (Outcome::Done, msg),
            Ok(Err(e)) if is_cancelled(&e) => (
                Outcome::Cancelled,
                "The operation was cancelled. Nothing was saved.".to_string(),
            ),
            Ok(Err(e)) => (Outcome::Failed, format!("{e:#}")),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                (Outcome::Failed, "The task stopped unexpectedly".into())
            }
        };
        self.status = Some(result);
        self.running = None;
    }

    /// Route opened/dropped files to a tool. On Home, the file type picks the tool.
    fn open_files(&mut self, tool: Option<Tool>, files: Vec<PathBuf>) {
        // Re-opened files may have changed on disk: drop their old previews.
        for f in &files {
            self.thumbs.invalidate(f);
        }
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
        let clicked = drop_zone(
            ui,
            150.0,
            "Drop files here or click to browse",
            "PDFs open in the Page Organizer · images open in Images to PDF",
        );
        if clicked {
            let files = rfd::FileDialog::new()
                .add_filter("PDFs and images", &[PDF_EXT, IMG_EXT].concat())
                .pick_files()
                .unwrap_or_default();
            if !files.is_empty() {
                self.open_files(None, files);
            }
        }

        // Tools grouped by category, as before.
        let card_w = ((ui.available_width() - 24.0) / 3.0).clamp(240.0, 360.0);
        for (group, tools) in Tool::GROUPS {
            ui.add_space(24.0);
            ui.label(strong(group).size(15.0));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(12.0);
                for &t in tools {
                    if tool_card(ui, t, card_w) {
                        self.tool = Some(t);
                    }
                }
            });
        }
        ui.add_space(16.0);
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
            Tool::Organize => app.organize.ui(ui, &mut app.thumbs),
            Tool::Merge => app.merge.ui(ui, &mut app.thumbs),
            Tool::Split => app.split.ui(ui, &mut app.thumbs),
            Tool::Compress => app.compress.ui(ui, &mut app.thumbs),
            Tool::ImagesToPdf => app.images.ui(ui, &mut app.thumbs),
            Tool::Links => app.links.ui(ui, &mut app.thumbs),
            Tool::Metadata => app.meta.ui(ui, &mut app.thumbs),
            Tool::ExtractText => app.text.ui(ui, &mut app.thumbs),
            Tool::Protect => app.protect.ui(ui, &mut app.thumbs),
            Tool::Unlock => app.unlock.ui(ui, &mut app.thumbs),
        };
        // Tools are disabled while a task runs or while an update is being installed.
        let busy = self.running.is_some() || self.update.busy();
        ui.add_enabled_ui(!busy, |ui| {
            if matches!(tool, Tool::Organize | Tool::Links) {
                // The organizer fills the window and scrolls its own page grid.
                body(self, ui)
            } else {
                // A separate scroll position for each tool.
                egui::ScrollArea::vertical()
                    .id_salt(tool.name())
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Use the full window width (leave room for the scroll bar).
                        ui.set_width(ui.available_width() - 12.0);
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
            let pr = r.progress.clone();
            let cancelling = pr.is_cancelled();
            let elapsed = r.started.elapsed().as_secs();
            let fraction = pr.fraction();
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let title = if cancelling {
                            "Cancelling…".to_string()
                        } else {
                            format!("{}…", r.label)
                        };
                        ui.label(strong(title));
                        let stage = pr.stage();
                        let mut detail = if stage.is_empty() {
                            "Starting".to_string()
                        } else {
                            stage
                        };
                        if let Some(f) = fraction {
                            detail = format!("{:.0}%  ·  {detail}", f * 100.0);
                        }
                        detail.push_str(&format!(
                            "  ·  {}:{:02} elapsed",
                            elapsed / 60,
                            elapsed % 60
                        ));
                        caption(ui, &detail);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn = egui::Button::new(labeled(I_CLOSE, "Cancel"))
                            .min_size(Vec2::new(100.0, 32.0));
                        if ui
                            .add_enabled(!cancelling, btn)
                            .on_hover_text("Stop now. Nothing is saved.")
                            .clicked()
                        {
                            pr.cancel();
                        }
                    });
                });
                ui.add_space(6.0);
                let bar = match fraction {
                    // Real progress when the task reports it…
                    Some(f) => egui::ProgressBar::new(f),
                    // …otherwise an animated "working" bar.
                    None => egui::ProgressBar::new(((ui.input(|i| i.time) * 0.6) % 1.0) as f32),
                };
                ui.add(bar.desired_height(6.0).fill(p.accent).corner_radius(3.0));
            });
            // Keep the elapsed time and the bar moving.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let Some((outcome, msg)) = self.status.clone() else {
            return;
        };
        let ok = outcome == Outcome::Done;
        let (bg, fg, icon, head) = match outcome {
            Outcome::Done => (p.success_bg, p.success, I_CHECK, "Done"),
            Outcome::Failed => (p.error_bg, p.error, I_ERROR, "Something went wrong"),
            Outcome::Cancelled => (p.card, p.text2, I_INFO, "Cancelled"),
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
                                && ui.button(labeled(I_FOLDER, "Show in folder")).clicked()
                            {
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
fn tool_card(ui: &mut egui::Ui, tool: Tool, width: f32) -> bool {
    let p = Palette::current(ui);
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
        self.update.poll();
        self.thumbs.poll(ui.ctx());
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

        let bg = Palette::current(ui).bg;
        // Version + "Check for updates", pinned to the bottom of the Home screen.
        if self.tool.is_none() {
            egui::Panel::bottom("home_footer")
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin {
                    left: 40,
                    right: 40,
                    top: 6,
                    bottom: 10,
                }))
                .show(ui, |ui| self.update.footer(ui));
        }
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
                        // No visible scroll bar on Home; the mouse wheel still scrolls.
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
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

        // Esc goes back to Home. Checked after the tools ran, so a tool can use Esc first
        // (e.g. to clear a selection); the page viewer and dialogs close on Esc themselves.
        if self.tool.is_some()
            && self.running.is_none()
            && !self.thumbs.viewer_open()
            && !self.update.dialog_open()
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.tool = None;
        }

        self.thumbs.show_viewer(&ctx);
        self.update.dialog(&ctx);

        if let Some(t) = action {
            self.start(&ctx, t);
        }
    }
}
