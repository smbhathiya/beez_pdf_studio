//! Update dialog: background check on start-up, "Update now", restart.

use std::sync::mpsc;

use anyhow::Result;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

use super::theme::*;
use crate::updater::{self, Available, CURRENT_VERSION};

enum State {
    Idle,
    /// `manual` = the user pressed "Check for updates" (so report "up to date" and errors).
    Checking {
        manual: bool,
        rx: mpsc::Receiver<Result<Option<Available>>>,
    },
    Available(Available),
    Installing {
        version: String,
        rx: mpsc::Receiver<Result<String>>,
    },
    Installed(String),
    UpToDate,
    Failed(String),
}

pub struct UpdateUi {
    state: State,
}

const I_UPDATE: &str = "\u{E895}"; // Sync
const I_DOWNLOAD: &str = "\u{E896}";

impl UpdateUi {
    /// Starts a silent background check unless disabled with `UTILBEE_NO_UPDATE_CHECK`.
    pub fn new(ctx: &egui::Context) -> Self {
        let mut me = Self { state: State::Idle };
        if std::env::var_os(updater::DISABLE_ENV).is_none() {
            me.check(ctx, false);
        }
        me
    }

    fn check(&mut self, ctx: &egui::Context, manual: bool) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(updater::check());
            ctx.request_repaint();
        });
        self.state = State::Checking { manual, rx };
    }

    fn install(&mut self, ctx: &egui::Context, version: String) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(updater::install());
            ctx.request_repaint();
        });
        self.state = State::Installing { version, rx };
    }

    pub fn poll(&mut self) {
        let next = match &self.state {
            State::Checking { manual, rx } => match rx.try_recv() {
                Ok(Ok(Some(a))) => State::Available(a),
                Ok(Ok(None)) if *manual => State::UpToDate,
                Ok(Err(e)) if *manual => State::Failed(format!("{e:#}")),
                // Silent start-up check: no news, or offline, shows nothing.
                Ok(_) | Err(mpsc::TryRecvError::Disconnected) => State::Idle,
                Err(mpsc::TryRecvError::Empty) => return,
            },
            State::Installing { rx, .. } => match rx.try_recv() {
                Ok(Ok(v)) => State::Installed(v),
                Ok(Err(e)) => State::Failed(format!("{e:#}")),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    State::Failed("The update stopped unexpectedly".into())
                }
            },
            _ => return,
        };
        self.state = next;
    }

    pub fn busy(&self) -> bool {
        matches!(self.state, State::Installing { .. })
    }

    /// Whether the update dialog is on screen (so Esc closes it instead of leaving a tool).
    pub fn dialog_open(&self) -> bool {
        !matches!(
            self.state,
            State::Idle | State::Checking { manual: false, .. }
        )
    }

    /// The pop-up dialog, shown whenever there is something to say. Call once per frame.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        if !self.dialog_open() {
            return;
        }
        let p = Palette::of(ctx.global_style().visuals.dark_mode);
        let (icon, color, title) = match &self.state {
            State::Checking { .. } => (I_UPDATE, p.accent, "Checking for updates"),
            State::Available(_) => (I_UPDATE, p.accent, "Update available"),
            State::Installing { .. } => (I_DOWNLOAD, p.accent, "Installing update"),
            State::Installed(_) => (I_CHECK, p.success, "Update installed"),
            State::UpToDate => (I_CHECK, p.success, "You're up to date"),
            State::Failed(_) => (I_ERROR, p.error, "Update failed"),
            State::Idle => return,
        };
        // Busy states can't be dismissed.
        let closable = !matches!(
            self.state,
            State::Installing { .. } | State::Checking { .. }
        );

        let mut next = None;
        let mut install = None;
        let response = egui::Modal::new(egui::Id::unique("update_dialog"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(8.0)
                    .inner_margin(24.0),
            )
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.horizontal(|ui| {
                    egui::Frame::new()
                        .fill(color.gamma_multiply(0.15))
                        .corner_radius(8.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(icon).size(22.0).color(color));
                        });
                    ui.add_space(8.0);
                    ui.label(subtitle(title));
                });
                ui.add_space(14.0);

                match &self.state {
                    State::Checking { .. } => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Contacting GitHub…");
                        });
                    }
                    State::Available(a) => {
                        ui.label(format!("UtilBee PDF Studio {} is ready to install.", a.version));
                        ui.label(secondary(ui, format!("You have version {CURRENT_VERSION}.")));
                        ui.add_space(4.0);
                        caption(ui, "The update downloads in the background and takes effect after a restart.");
                    }
                    State::Installing { version, .. } => {
                        ui.label(format!("Downloading and installing version {version}…"));
                        ui.add_space(6.0);
                        ui.add(egui::ProgressBar::new(0.0).animate(true).desired_height(4.0));
                    }
                    State::Installed(v) => {
                        ui.label(format!("Version {v} has been installed."));
                        ui.label(secondary(ui, "Restart the app to start using it."));
                    }
                    State::UpToDate => {
                        ui.label(format!("Version {CURRENT_VERSION} is the latest version."));
                    }
                    State::Failed(e) => {
                        ui.add(egui::Label::new(secondary(ui, e.clone())).wrap());
                        ui.add_space(4.0);
                        caption(ui, "You can download the latest version from the release page instead.");
                    }
                    State::Idle => {}
                }

                ui.add_space(20.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let wide = Vec2::new(110.0, 32.0);
                    match &self.state {
                        State::Available(a) => {
                            if accent_button(ui, true, &labeled(I_DOWNLOAD, "Update now")) {
                                install = Some(a.version.clone());
                            }
                            if ui.add(egui::Button::new("What's new").min_size(wide)).clicked() {
                                ctx.open_url(egui::OpenUrl::new_tab(&a.notes_url));
                            }
                            if ui.add(egui::Button::new("Later").min_size(wide)).clicked() {
                                next = Some(State::Idle);
                            }
                        }
                        State::Installed(_) => {
                            if accent_button(ui, true, &labeled(I_UPDATE, "Restart now")) {
                                match updater::relaunch() {
                                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                                    Err(e) => next = Some(State::Failed(format!("{e:#}"))),
                                }
                            }
                            if ui.add(egui::Button::new("Later").min_size(wide)).clicked() {
                                next = Some(State::Idle);
                            }
                        }
                        State::UpToDate => {
                            if accent_button(ui, true, "OK") {
                                next = Some(State::Idle);
                            }
                        }
                        State::Failed(_) => {
                            if ui.add(egui::Button::new("Close").min_size(wide)).clicked() {
                                next = Some(State::Idle);
                            }
                            if accent_button(ui, true, "Open download page") {
                                ctx.open_url(egui::OpenUrl::new_tab(updater::releases_page()));
                            }
                        }
                        _ => {}
                    }
                });
            });

        // Esc or a click outside closes the dialog (except while busy).
        if closable && response.should_close() {
            next = Some(State::Idle);
        }
        if let Some(n) = next {
            self.state = n;
        }
        if let Some(v) = install {
            self.install(ctx, v);
        }
    }

    /// "Version x.y.z · Check for updates" line for the bottom of Home.
    pub fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            caption(
                ui,
                &format!("UtilBee PDF Studio · Version {CURRENT_VERSION}"),
            );
            let checking = matches!(
                self.state,
                State::Checking { .. } | State::Installing { .. }
            );
            if ui
                .add_enabled(
                    !checking,
                    egui::Button::new(RichText::new("Check for updates").size(12.0))
                        .frame_when_inactive(false),
                )
                .clicked()
            {
                let ctx = ui.ctx().clone();
                self.check(&ctx, true);
            }
        });
    }
}
