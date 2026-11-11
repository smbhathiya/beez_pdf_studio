//! Update banner: background check on start-up, "Update now", restart.

use std::sync::mpsc;

use anyhow::Result;
use eframe::egui::{self, Margin, RichText, Stroke};

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

    /// The InfoBar shown at the top of every page while there is something to say.
    pub fn banner(&mut self, ui: &mut egui::Ui) {
        let p = Palette::current(ui);
        let (bg, fg, icon) = match &self.state {
            State::Idle | State::Checking { manual: false, .. } => return,
            State::Failed(_) => (p.error_bg, p.error, I_ERROR),
            State::Installed(_) | State::UpToDate => (p.success_bg, p.success, I_CHECK),
            _ => (
                p.accent.gamma_multiply(if p.dark { 0.16 } else { 0.10 }),
                p.accent,
                I_UPDATE,
            ),
        };
        let ctx = ui.ctx().clone();
        let mut next = None;
        let mut install = None;
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
                    match &self.state {
                        State::Checking { .. } => {
                            ui.spinner();
                            ui.label("Checking for updates…");
                        }
                        State::Available(a) => {
                            ui.label(strong(format!("Version {} is available", a.version)));
                            ui.label(secondary(ui, format!("You have version {CURRENT_VERSION}")));
                        }
                        State::Installing { version, .. } => {
                            ui.spinner();
                            ui.label(strong(format!(
                                "Downloading and installing version {version}…"
                            )));
                        }
                        State::Installed(v) => {
                            ui.label(strong(format!("Updated to version {v}")));
                            ui.label(secondary(ui, "Restart the app to use it."));
                        }
                        State::UpToDate => {
                            ui.label(strong("You're up to date"));
                            ui.label(secondary(
                                ui,
                                format!("Version {CURRENT_VERSION} is the latest version."),
                            ));
                        }
                        State::Failed(e) => {
                            ui.label(strong("Update failed"));
                            ui.add(egui::Label::new(secondary(ui, e.clone())).wrap());
                        }
                        State::Idle => {}
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let closable = !matches!(
                            self.state,
                            State::Installing { .. } | State::Checking { .. }
                        );
                        if closable && icon_button(ui, true, I_CLOSE, "Dismiss") {
                            next = Some(State::Idle);
                        }
                        match &self.state {
                            State::Available(a) => {
                                if accent_button(ui, true, &labeled(I_DOWNLOAD, "Update now")) {
                                    install = Some(a.version.clone());
                                }
                                if ui.button("What's new").clicked() {
                                    ctx.open_url(egui::OpenUrl::new_tab(&a.notes_url));
                                }
                            }
                            State::Installed(_) => {
                                if accent_button(ui, true, &labeled(I_UPDATE, "Restart now")) {
                                    match updater::relaunch() {
                                        Ok(()) => {
                                            ctx.send_viewport_cmd(egui::ViewportCommand::Close)
                                        }
                                        Err(e) => next = Some(State::Failed(format!("{e:#}"))),
                                    }
                                }
                            }
                            State::Failed(_) if ui.button("Open download page").clicked() => {
                                ctx.open_url(egui::OpenUrl::new_tab(updater::releases_page()));
                            }
                            _ => {}
                        }
                    });
                });
            });
        if let Some(n) = next {
            self.state = n;
        }
        if let Some(v) = install {
            self.install(&ctx, v);
        }
        ui.add_space(12.0);
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
