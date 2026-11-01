//! Settings window: audio driver, output device and input device.
//!
//! The choice is written to the config file at once. The audio streams are opened at start-up, so a
//! change takes effect after a restart; the window says so and offers "Quit & reopen".
use crate::audio_devices as ad;
use crate::state::State;
use crate::theme::*;
use eframe::egui::{self, vec2, Align2, Color32, Id, RichText, Rounding, Ui};
use project_io::Config;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by "Quit & reopen": `on_exit` starts a fresh copy of the app.
pub(crate) static RESTART_ON_EXIT: AtomicBool = AtomicBool::new(false);

/// Start the app again (called after the window has closed and the audio devices are released).
pub(crate) fn restart_if_requested() {
    if RESTART_ON_EXIT.load(Ordering::SeqCst) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).args(std::env::args().skip(1)).spawn();
        }
    }
}

#[derive(Clone, PartialEq)]
struct Choice {
    host: Option<String>,
    output: Option<String>,
    input: Option<String>,
}

impl Choice {
    fn of(cfg: &Config) -> Self {
        Self { host: cfg.audio_host.clone(), output: cfg.audio_output.clone(), input: cfg.audio_input.clone() }
    }
}

pub(crate) struct SettingsUi {
    /// What the running audio streams were opened with.
    running: Choice,
    scanned: bool,
    default_host: String,
    outputs: Vec<String>,
    inputs: Vec<String>,
    default_output: Option<String>,
    default_input: Option<String>,
    hosts: Vec<String>,
}

impl SettingsUi {
    pub fn new(cfg: &Config) -> Self {
        Self { running: Choice::of(cfg), scanned: false, default_host: String::new(), outputs: Vec::new(), inputs: Vec::new(), default_output: None, default_input: None, hosts: Vec::new() }
    }

    /// Ask the driver for its devices (done when the window opens, after a driver change and on "Rescan").
    fn scan(&mut self, host: Option<&str>) {
        let h = ad::open_host(host);
        self.hosts = ad::host_names();
        self.default_host = ad::default_host_name();
        self.outputs = ad::output_names(&h);
        self.inputs = ad::input_names(&h);
        self.default_output = ad::default_output_name(&h);
        self.default_input = ad::default_input_name(&h);
        self.scanned = true;
    }
}

/// A full-width drop-down: "System default (…)" first, then every entry. Returns true when changed.
fn choice_combo(ui: &mut Ui, id: &str, current: &mut Option<String>, options: &[String], default_text: String) -> bool {
    let mut changed = false;
    let missing = current.as_ref().map_or(false, |c| !options.contains(c));
    let shown = match current {
        None => default_text.clone(),
        Some(c) if missing => format!("{c}  (not found)"),
        Some(c) => c.clone(),
    };
    egui::ComboBox::from_id_source(id).width(ui.available_width() - 14.0).selected_text(shown).show_ui(ui, |ui| {
        if ui.selectable_label(current.is_none(), &default_text).clicked() {
            *current = None;
            changed = true;
        }
        for o in options {
            if ui.selectable_label(current.as_deref() == Some(o.as_str()), o).clicked() {
                *current = Some(o.clone());
                changed = true;
            }
        }
    });
    changed
}

fn field_label(ui: &mut Ui, text: &str, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).size(10.5).strong().color(MUTED));
        if !hint.is_empty() {
            ui.label(RichText::new(hint).size(10.5).color(MUTED.gamma_multiply(0.7)));
        }
    });
}

pub(crate) fn settings_window(s: &mut State, ctx: &egui::Context) {
    if !s.settings_open {
        return;
    }
    if !s.settings.scanned {
        let h = s.cfg.audio_host.clone();
        s.settings.scan(h.as_deref());
    }
    let mut open = true;
    let frame = egui::Frame::window(&ctx.style()).fill(PANEL).rounding(Rounding::same(16.0)).inner_margin(egui::Margin::same(16.0));
    egui::Window::new("Settings").id(Id::new("settings_window")).open(&mut open).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).default_width(460.0).frame(frame).show(ctx, |ui| {
        ui.set_width(430.0);
        ui.label(RichText::new("Audio").size(20.0).strong());
        ui.label(RichText::new(format!("Running at {:.0} Hz", s.sample_rate)).small().color(MUTED));
        ui.add_space(10.0);

        let before = Choice::of(&s.cfg);
        let mut cfg_host = s.cfg.audio_host.clone();
        let mut cfg_out = s.cfg.audio_output.clone();
        let mut cfg_in = s.cfg.audio_input.clone();
        let mut rescan = false;

        egui::Frame::none().fill(CARD).rounding(Rounding::same(12.0)).inner_margin(egui::Margin::same(14.0)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            field_label(ui, "DRIVER", "optional");
            let hosts = s.settings.hosts.clone();
            if choice_combo(ui, "set_host", &mut cfg_host, &hosts, format!("System default ({})", s.settings.default_host)) {
                // devices belong to a driver: start again from its defaults
                cfg_out = None;
                cfg_in = None;
                rescan = true;
            }
            ui.add_space(10.0);
            field_label(ui, "OUTPUT DEVICE", "");
            let outs = s.settings.outputs.clone();
            let def_out = s.settings.default_output.clone().unwrap_or_else(|| "none".into());
            choice_combo(ui, "set_out", &mut cfg_out, &outs, format!("System default ({def_out})"));
            ui.add_space(10.0);
            field_label(ui, "INPUT DEVICE", "used for recording");
            let ins = s.settings.inputs.clone();
            let def_in = s.settings.default_input.clone().unwrap_or_else(|| "none".into());
            choice_combo(ui, "set_in", &mut cfg_in, &ins, format!("System default ({def_in})"));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new("Rescan devices").size(12.5)).fill(CARD_HI).rounding(Rounding::same(8.0)).min_size(vec2(0.0, 26.0));
                if ui.add(b).on_hover_text("Look for newly plugged-in devices").clicked() {
                    rescan = true;
                }
            });
        });

        let now = Choice { host: cfg_host, output: cfg_out, input: cfg_in };
        if now != before {
            s.cfg.audio_host = now.host.clone();
            s.cfg.audio_output = now.output.clone();
            s.cfg.audio_input = now.input.clone();
            s.cfg.save();
        }
        if rescan {
            let h = s.cfg.audio_host.clone();
            s.settings.scan(h.as_deref());
        }

        // the streams are opened at start-up: a different choice needs a restart
        if now != s.settings.running {
            ui.add_space(10.0);
            egui::Frame::none().fill(YELLOW.gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0_f32, YELLOW.gamma_multiply(0.4))).rounding(Rounding::same(12.0)).inner_margin(egui::Margin::same(12.0)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Restart needed").strong().color(YELLOW));
                ui.label(RichText::new("The new audio setup is used the next time the app starts. Unsaved project changes are lost when you reopen: save first.").small().color(TEXT));
                ui.add_space(6.0);
                let b = egui::Button::new(RichText::new("Quit & reopen").size(12.5).color(Color32::BLACK)).fill(YELLOW).rounding(Rounding::same(8.0)).min_size(vec2(0.0, 26.0));
                if ui.add(b).clicked() {
                    RESTART_ON_EXIT.store(true, Ordering::SeqCst);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        }
    });
    if !open {
        s.settings_open = false;
    }
}
