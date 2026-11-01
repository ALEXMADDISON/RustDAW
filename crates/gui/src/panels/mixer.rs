use crate::state::*;
use crate::widgets::*;
use crate::panels::automation::automation_context;
use core_engine::{AutoTarget, Command, MAX_TRACKS};
use eframe::egui::{self, RichText, Ui};

/// Strips for every track that is in use (has clips, a non-default instrument, effects, or is armed/selected), plus master.
pub(crate) fn mixer(s: &mut State, ui: &mut Ui) {
    let active: Vec<usize> = (0..TRACKS).filter(|&t| s.track_active(t)).collect();
    egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            for &t in &active {
                let (l, r) = s.meters.peak(t);
                strip(ui, s, Some(t), l, r);
            }
            ui.add_space(10.0);
            let (l, r) = s.meters.master();
            strip(ui, s, None, l, r);
        });
    });
}

fn strip(ui: &mut Ui, s: &mut State, track: Option<usize>, pl: f32, pr: f32) {
    let meter_h = (ui.available_height() - 130.0).clamp(60.0, 170.0);
    card().show(ui, |ui| {
        ui.set_width(66.0);
        ui.vertical_centered(|ui| {
            match track {
                Some(t) => {
                    let mut name = s.tracks[t].name.clone();
                    if name.chars().count() > 8 {
                        name = format!("{}\u{2026}", name.chars().take(7).collect::<String>());
                    }
                    ui.label(RichText::new(name).strong().color(track_color(t)));
                    ui.spacing_mut().slider_width = 56.0;
                    let pan_resp = ui.add(egui::Slider::new(&mut s.tracks[t].pan, -1.0..=1.0).show_value(false).trailing_fill(false));
                    if pan_resp.changed() {
                        s.handle.send(Command::SetTrackPan { track: t, pan: s.tracks[t].pan });
                    }
                    pan_resp.context_menu(|ui| {
                        s.selected_track = t;
                        automation_context(ui, s, AutoTarget::TrackPan(t as u16));
                    });
                }
                None => {
                    ui.label(RichText::new("Master").strong());
                    ui.add_space(24.0);
                }
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().slider_width = meter_h;
                meter(ui, pl, pr, meter_h);
                match track {
                    Some(t) => {
                        let gain_resp = ui.add(egui::Slider::new(&mut s.tracks[t].gain, 0.0..=1.5).vertical().show_value(false));
                        if gain_resp.changed() {
                            s.handle.send(Command::SetTrackGain { track: t, gain: s.tracks[t].gain });
                        }
                        gain_resp.context_menu(|ui| {
                            s.selected_track = t;
                            automation_context(ui, s, AutoTarget::TrackGain(t as u16));
                        });
                    }
                    None => {
                        let master_resp = ui.add(egui::Slider::new(&mut s.master_gain, 0.0..=1.5).vertical().show_value(false));
                        if master_resp.changed() {
                            s.handle.send(Command::SetMasterGain(s.master_gain));
                        }
                        master_resp.context_menu(|ui| {
                            automation_context(ui, s, AutoTarget::MasterGain);
                        });
                    }
                }
            });
            if let Some(t) = track {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    if ui.toggle_value(&mut s.tracks[t].mute, RichText::new("M").small()).changed() {
                        s.handle.send(Command::SetTrackMute { track: t, mute: s.tracks[t].mute });
                    }
                    if ui.toggle_value(&mut s.tracks[t].solo, RichText::new("S").small()).changed() {
                        s.handle.send(Command::SetTrackSolo { track: t, solo: s.tracks[t].solo });
                    }
                });
            }
        });
    });
    let _ = MAX_TRACKS;
}
