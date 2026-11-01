//! Inspector for the selected track: instrument, insert-effect slots (VST3), volume / pan faders
//! and VST3 folder management.
//!
//! Look: every group sits on a rounded card; menus are pill buttons that open searchable lists with
//! one row per plugin; volume / pan are custom faders (double-click = reset, right-click = automation).
use crate::dialogs;
use crate::panels::automation::automation_context;
use crate::state::*;
use crate::theme::*;
use crate::widgets::*;
use core_engine::{AutoTarget, Command, FX_SLOTS};
use dsp::Waveform;
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Id, Layout, Rect, RichText, Rounding, Sense, Stroke, Ui};

enum Pick {
    LoadPlugin(usize, usize),
    Basic,
    NoInstrument,
    Remove(usize),
    OpenEditor(usize),
    OpenParams(usize),
}

/// Recessed background used for tracks / segmented controls.
const WELL: Color32 = Color32::from_rgb(16, 17, 23);

// ---------------------------------------------------------------------------------------------
// small widgets
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Look {
    Primary,
    Soft,
    Danger,
}

/// Flat rounded button: filled accent (primary), neutral (soft) or red-tinted (danger).
fn btn(ui: &mut Ui, text: &str, look: Look) -> egui::Response {
    let (fill, fg) = match look {
        Look::Primary => (ACCENT, Color32::WHITE),
        Look::Soft => (CARD_HI, TEXT),
        Look::Danger => (RED.gamma_multiply(0.16), RED),
    };
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(9.0, 4.0);
        ui.add(egui::Button::new(RichText::new(text).size(12.5).color(fg)).fill(fill).rounding(Rounding::same(8.0)).min_size(vec2(0.0, 26.0)))
    })
    .inner
}

/// A rounded card with a small caps title.
fn section<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let r = egui::Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(12.0))
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(10.5).strong().color(MUTED));
            ui.add_space(4.0);
            add(ui)
        })
        .inner;
    ui.add_space(8.0);
    r
}

fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}\u{2026}", s.chars().take(n.saturating_sub(1)).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Wide pill button that opens a popup menu. `dot` is the little status colour on its left
/// (`None` draws a "+" instead, used for empty slots).
fn pill_menu(ui: &mut Ui, dot: Option<Color32>, text: &str, muted: bool, width: f32, height: f32, add: impl FnOnce(&mut Ui)) {
    let label = RichText::new(shorten(text, 26)).size(13.0).color(if muted { MUTED } else { TEXT });
    let out = ui.scope(|ui| {
        // the button takes its minimum size from `interact_size`; the look comes from the widget visuals
        ui.spacing_mut().button_padding = vec2(30.0, 6.0);
        ui.spacing_mut().interact_size = vec2(width, height);
        let v = ui.visuals_mut();
        for (w, fill) in [(&mut v.widgets.inactive, CARD_HI), (&mut v.widgets.hovered, Color32::from_rgb(54, 58, 72)), (&mut v.widgets.active, Color32::from_rgb(54, 58, 72)), (&mut v.widgets.open, Color32::from_rgb(54, 58, 72))] {
            w.weak_bg_fill = fill;
            w.bg_fill = fill;
            w.rounding = Rounding::same(10.0);
        }
        ui.menu_button(label, |ui| {
            ui.set_min_width(290.0);
            add(ui);
        })
    });
    let rect = out.inner.response.rect;
    let p = ui.painter();
    let cy = rect.center().y;
    match dot {
        Some(c) => {
            p.circle_filled(pos2(rect.left() + 15.0, cy), 6.0, c.gamma_multiply(0.28));
            p.circle_filled(pos2(rect.left() + 15.0, cy), 3.5, c);
        }
        None => {
            let st = Stroke::new(1.6_f32, MUTED);
            p.line_segment([pos2(rect.left() + 11.0, cy), pos2(rect.left() + 19.0, cy)], st);
            p.line_segment([pos2(rect.left() + 15.0, cy - 4.0), pos2(rect.left() + 15.0, cy + 4.0)], st);
        }
    }
    // chevron
    let c = pos2(rect.right() - 16.0, cy);
    let st = Stroke::new(1.6_f32, MUTED);
    p.line_segment([c + vec2(-4.0, -2.0), c + vec2(0.0, 2.5)], st);
    p.line_segment([c + vec2(0.0, 2.5), c + vec2(4.0, -2.0)], st);
}

/// One row of a popup list. Returns true when clicked.
fn menu_row(ui: &mut Ui, dot: Color32, title: &str, sub: &str, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(260.0), 30.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 8.0, CARD_HI);
    } else if selected {
        p.rect_filled(rect, 8.0, ACCENT.gamma_multiply(0.16));
    }
    let cy = rect.center().y;
    p.circle_filled(pos2(rect.left() + 14.0, cy), 3.5, dot);
    p.text(pos2(rect.left() + 28.0, cy), Align2::LEFT_CENTER, shorten(title, 26), FontId::proportional(13.0), TEXT);
    if !sub.is_empty() {
        p.text(pos2(rect.right() - 10.0, cy), Align2::RIGHT_CENTER, shorten(sub, 14), FontId::proportional(10.5), MUTED);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

fn menu_heading(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(10.5).strong().color(MUTED));
    ui.add_space(2.0);
}

/// Searchable list of plugins of one kind; returns the chosen index.
fn plugin_menu(ui: &mut Ui, s: &State, want_instrument: bool) -> Option<usize> {
    let mut out = None;
    let sid = Id::new(("plugin_search", want_instrument));
    let mut q: String = ui.data_mut(|d| d.get_temp::<String>(sid)).unwrap_or_default();
    ui.add(egui::TextEdit::singleline(&mut q).hint_text("Search\u{2026}").desired_width(f32::INFINITY));
    ui.data_mut(|d| d.insert_temp(sid, q.clone()));
    let ql = q.trim().to_lowercase();
    ui.add_space(4.0);
    let (mut any_kind, mut any_match) = (false, false);
    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
        for (i, p) in s.plugins.iter().enumerate().filter(|(_, p)| p.is_instrument == want_instrument) {
            any_kind = true;
            if !ql.is_empty() && !p.name.to_lowercase().contains(&ql) && !p.vendor.to_lowercase().contains(&ql) {
                continue;
            }
            any_match = true;
            if menu_row(ui, if want_instrument { ACCENT } else { GREEN }, &p.name, &p.vendor, false) {
                out = Some(i);
                ui.close_menu();
            }
        }
        if !any_kind {
            ui.label(RichText::new(if want_instrument { "No VST3 instruments found" } else { "No VST3 effects found" }).color(MUTED));
            ui.label(RichText::new("Add a plugin folder at the bottom of the inspector").small().color(MUTED));
        } else if !any_match {
            ui.label(RichText::new("No matches").color(MUTED));
        }
    });
    out
}

/// Horizontal fader with a label and a read-out. Double-click resets, `center` makes it grow from the middle.
#[allow(clippy::too_many_arguments)]
fn fader(ui: &mut Ui, label: &str, value: &mut f32, (min, max): (f32, f32), default: f32, center: bool, readout: String, col: Color32) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click_and_drag());
    let track = Rect::from_min_max(pos2(rect.left() + 8.0, rect.bottom() - 17.0), pos2(rect.right() - 8.0, rect.bottom() - 11.0));
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let f = ((p.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
            let mut v = min + f * (max - min);
            if center && v.abs() < 0.03 * (max - min) {
                v = 0.0;
            }
            if (v - *value).abs() > f32::EPSILON {
                *value = v;
                resp.mark_changed();
            }
        }
    }
    if resp.double_clicked() {
        *value = default;
        resp.mark_changed();
    }
    let frac = |v: f32| ((v - min) / (max - min)).clamp(0.0, 1.0);
    let x_of = |v: f32| track.left() + frac(v) * track.width();
    let hot = resp.hovered() || resp.dragged();
    let p = ui.painter();
    p.text(pos2(rect.left() + 8.0, rect.top() + 2.0), Align2::LEFT_TOP, label, FontId::proportional(10.5), MUTED);
    p.text(pos2(rect.right() - 8.0, rect.top() + 1.0), Align2::RIGHT_TOP, readout, FontId::proportional(12.0), if hot { TEXT } else { TEXT.gamma_multiply(0.85) });
    p.rect_filled(track, 3.0, WELL);
    // the part between the origin and the handle
    let origin = if center { x_of(0.0) } else { track.left() };
    let hx = x_of(*value);
    let (a, b) = (origin.min(hx), origin.max(hx));
    if b - a > 0.5 {
        p.rect_filled(Rect::from_min_max(pos2(a, track.top()), pos2(b, track.bottom())), 3.0, col);
    }
    // marks: default position (unity / centre)
    let dx = x_of(default);
    p.line_segment([pos2(dx, track.top() - 3.0), pos2(dx, track.bottom() + 3.0)], Stroke::new(1.0_f32, Color32::from_white_alpha(46)));
    // handle
    let c = pos2(hx, track.center().y);
    if hot {
        p.circle_filled(c, 12.0, col.gamma_multiply(0.22));
    }
    p.circle_filled(c, 7.5, Color32::WHITE);
    p.circle_filled(c, 4.0, col);
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Pill-shaped segmented control; returns the newly chosen value.
fn segmented<T: PartialEq + Copy>(ui: &mut Ui, items: &[(T, &str)], cur: T) -> Option<T> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().rect_filled(rect, 10.0, WELL);
    let cw = (rect.width() - 4.0) / items.len().max(1) as f32;
    let mut out = None;
    for (i, (v, name)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 2.0 + i as f32 * cw, rect.top() + 2.0), vec2(cw, 26.0));
        let resp = ui.interact(r, Id::new(("inspector_seg", *name)), Sense::click());
        let on = *v == cur;
        if on {
            ui.painter().rect_filled(r, 8.0, ACCENT);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 8.0, CARD_HI);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *name, FontId::proportional(12.0), if on { Color32::WHITE } else { MUTED });
        if resp.clicked() {
            out = Some(*v);
        }
    }
    out
}

fn db_text(g: f32) -> String {
    if g <= 0.0005 {
        "-inf dB".to_string()
    } else {
        format!("{:+.1} dB", 20.0 * g.log10())
    }
}

fn pan_text(p: f32) -> String {
    let pc = (p * 100.0).round() as i32;
    match pc {
        0 => "Center".to_string(),
        x if x < 0 => format!("{}% L", -x),
        x => format!("{x}% R"),
    }
}

// ---------------------------------------------------------------------------------------------
// the panel
// ---------------------------------------------------------------------------------------------

pub(crate) fn inspector(s: &mut State, ui: &mut Ui) {
    let t = s.selected_track;
    let mut pick: Option<Pick> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;

        // ---- header: colour stripe, editable name, what is loaded ----
        egui::Frame::none().fill(CARD).rounding(Rounding::same(12.0)).inner_margin(egui::Margin::same(12.0)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(5.0, 40.0), Sense::hover());
                ui.painter().rect_filled(r, 2.5, track_color(t));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::TextEdit::singleline(&mut s.tracks[t].name).font(egui::TextStyle::Heading).frame(false).desired_width(ui.available_width()));
                    ui.label(RichText::new(format!("Track {}  \u{b7}  {}", t + 1, s.tracks[t].inst.label())).small().color(MUTED));
                });
            });
        });
        ui.add_space(2.0);

        // ---- instrument ----
        section(ui, "INSTRUMENT", |ui| {
            let (dot, label) = {
                let inst = &s.tracks[t].inst;
                let dot = match inst {
                    InstUi::Vst(_) => ACCENT,
                    InstUi::Basic => GREEN,
                    InstUi::Sampler(_) => YELLOW,
                    InstUi::None => MUTED,
                };
                (dot, inst.label())
            };
            let none = s.tracks[t].inst == InstUi::None;
            let w = ui.available_width();
            pill_menu(ui, Some(dot), &label, none, w, 36.0, |ui| {
                menu_heading(ui, "BUILT-IN");
                if menu_row(ui, GREEN, "Basic Synth", "oscillator", s.tracks[t].inst == InstUi::Basic) {
                    pick = Some(Pick::Basic);
                    ui.close_menu();
                }
                if menu_row(ui, MUTED, "None (audio only)", "", none) {
                    pick = Some(Pick::NoInstrument);
                    ui.close_menu();
                }
                ui.add_space(4.0);
                menu_heading(ui, "VST3 INSTRUMENTS");
                if let Some(i) = plugin_menu(ui, s, true) {
                    pick = Some(Pick::LoadPlugin(0, i));
                }
            });
            if let InstUi::Vst(_) = s.tracks[t].inst {
                ui.horizontal(|ui| {
                    if btn(ui, "Open editor", Look::Primary).clicked() {
                        pick = Some(Pick::OpenEditor(0));
                    }
                    if btn(ui, "Params", Look::Soft).on_hover_text("Generic parameter list").clicked() {
                        pick = Some(Pick::OpenParams(0));
                    }
                    if btn(ui, "Remove", Look::Danger).clicked() {
                        pick = Some(Pick::Remove(0));
                    }
                });
            }
            if s.tracks[t].inst == InstUi::Basic {
                ui.add_space(2.0);
                ui.label(RichText::new("Waveform").small().color(MUTED));
                let cur = s.tracks[t].wave;
                if let Some(w) = segmented(ui, &[(Waveform::Sine, "Sine"), (Waveform::Saw, "Saw"), (Waveform::Square, "Square"), (Waveform::Triangle, "Tri")], cur) {
                    if w != cur {
                        s.tracks[t].wave = w;
                        s.handle.send(Command::SetWaveform { track: t, waveform: w });
                    }
                }
            }
            if let InstUi::Sampler(_) = s.tracks[t].inst {
                ui.label(RichText::new("Drop another sample on the track header to replace it.").small().color(MUTED));
            }
        });

        // ---- inserts ----
        section(ui, "EFFECTS  \u{b7}  VST3", |ui| {
            for slot in 1..=FX_SLOTS {
                let name = s.tracks[t].fx[slot - 1].clone();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    // slot number badge
                    let (b, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                    ui.painter().circle_filled(b.center(), 10.0, if name.is_some() { ACCENT.gamma_multiply(0.35) } else { WELL });
                    ui.painter().text(b.center(), Align2::CENTER_CENTER, slot.to_string(), FontId::proportional(11.0), if name.is_some() { TEXT } else { MUTED });
                    let extra = if name.is_some() { 124.0 } else { 0.0 };
                    let w = (ui.available_width() - extra).max(90.0);
                    let (dot, text, muted) = match &name {
                        Some(n) => (Some(GREEN), n.clone(), false),
                        None => (None, "Add effect".to_string(), true),
                    };
                    pill_menu(ui, dot, &text, muted, w, 30.0, |ui| {
                        menu_heading(ui, "VST3 EFFECTS");
                        if let Some(i) = plugin_menu(ui, s, false) {
                            pick = Some(Pick::LoadPlugin(slot, i));
                        }
                    });
                    if name.is_some() {
                        if btn(ui, "Edit", Look::Soft).on_hover_text("Open the plugin window").clicked() {
                            pick = Some(Pick::OpenEditor(slot));
                        }
                        if btn(ui, "P", Look::Soft).on_hover_text("Generic parameter list").clicked() {
                            pick = Some(Pick::OpenParams(slot));
                        }
                        if btn(ui, "\u{d7}", Look::Danger).on_hover_text("Remove").clicked() {
                            pick = Some(Pick::Remove(slot));
                        }
                    }
                });
            }
        });

        // ---- channel strip ----
        section(ui, "CHANNEL", |ui| {
            let g = s.tracks[t].gain;
            let r = fader(ui, "VOLUME", &mut s.tracks[t].gain, (0.0, 1.5), 0.9, false, db_text(g), ACCENT);
            if r.changed() {
                s.handle.send(Command::SetTrackGain { track: t, gain: s.tracks[t].gain });
            }
            r.context_menu(|ui| automation_context(ui, s, AutoTarget::TrackGain(t as u16)));
            ui.add_space(2.0);
            let pn = s.tracks[t].pan;
            let r = fader(ui, "PAN", &mut s.tracks[t].pan, (-1.0, 1.0), 0.0, true, pan_text(pn), GREEN);
            if r.changed() {
                s.handle.send(Command::SetTrackPan { track: t, pan: s.tracks[t].pan });
            }
            r.context_menu(|ui| automation_context(ui, s, AutoTarget::TrackPan(t as u16)));
            ui.label(RichText::new("Double-click = reset  \u{b7}  right-click = automate").size(10.0).color(MUTED));
        });

        // ---- plugin folders ----
        egui::Frame::none().fill(CARD).rounding(Rounding::same(12.0)).inner_margin(egui::Margin::same(12.0)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::CollapsingHeader::new(RichText::new(format!("VST3 plugins  \u{b7}  {}", s.plugins.len())).strong()).default_open(false).show(ui, |ui| {
                ui.label(RichText::new(if s.scan_rx.is_some() { "Scanning\u{2026}" } else { "Scan finished" }).small().color(MUTED));
                for d in plugin_host::default_vst3_dirs() {
                    ui.label(RichText::new(d.display().to_string()).small().color(MUTED));
                }
                if !s.plugin_errors.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("{} file(s) could not be loaded:", s.plugin_errors.len())).small().color(RED));
                    for (p, e) in &s.plugin_errors {
                        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                        ui.label(RichText::new(format!("{name}: {e}")).small().color(MUTED)).on_hover_text(p.display().to_string());
                    }
                    ui.add_space(4.0);
                }
                let mut rm = None;
                for d in s.cfg.vst_dirs.clone() {
                    ui.horizontal(|ui| {
                        if btn(ui, "\u{d7}", Look::Danger).on_hover_text("Remove this folder").clicked() {
                            rm = Some(d.clone());
                        }
                        ui.label(RichText::new(d.display().to_string()).small());
                    });
                }
                if let Some(d) = rm {
                    s.cfg.vst_dirs.retain(|x| *x != d);
                    s.cfg.save();
                    s.rescan_plugins();
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut s.vst_dir_input).hint_text("extra VST3 folder").desired_width((ui.available_width() - 64.0).max(60.0)));
                    if btn(ui, "Add", Look::Primary).clicked() && !s.vst_dir_input.trim().is_empty() {
                        let d = std::path::PathBuf::from(s.vst_dir_input.trim().trim_matches('"'));
                        s.add_vst_dir(d);
                        s.vst_dir_input.clear();
                    }
                });
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if dialogs::has_native_dialogs() && btn(ui, "Browse\u{2026}", Look::Soft).clicked() {
                        if let Some(d) = dialogs::pick_folder() {
                            s.add_vst_dir(d);
                        }
                    }
                    if btn(ui, "Rescan", Look::Soft).clicked() {
                        s.rescan_plugins();
                    }
                });
            });
        });
    });

    match pick {
        Some(Pick::Basic) => s.set_basic_synth(t),
        Some(Pick::NoInstrument) => s.set_no_instrument(t),
        Some(Pick::LoadPlugin(slot, i)) => s.load_plugin(t, slot, i),
        Some(Pick::Remove(slot)) => s.remove_plugin(t, slot),
        Some(Pick::OpenEditor(slot)) => s.open_plugin_editor(t, slot),
        Some(Pick::OpenParams(slot)) => s.open_generic_params(t, slot),
        None => {}
    }
    let _ = (card as fn() -> egui::Frame, meter_h as fn(&mut Ui, f32, f32));
}
