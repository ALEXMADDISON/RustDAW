//! Automation: the editor window for automation clips and the menus that create them.
//!
//! Editor:
//!   double-click on empty space = new point - drag a point = move it (the whole selection moves)
//!   drag on empty space = select points with a rectangle - right-click a point = delete it
//!   Delete = delete selected - Ctrl+A / Ctrl+C / Ctrl+V = select all / copy / paste at the last click
//!   wheel = scroll - Ctrl + wheel = zoom time - Alt + wheel = zoom values - middle-drag = pan
use crate::state::*;
use crate::theme::*;
use core_engine::{eval_points, AutoPoint, AutoTarget, Shape, FX_SLOTS, PPQ};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Id, PointerButton, Pos2, Rect, RichText, Rounding, Sense, Stroke, Ui};
use std::time::{Duration, Instant};

const POINT_R: f32 = 5.0;
const HIT_R: f32 = 10.0;
/// The automation colour (same family as the clips in the playlist).
const CURVE: Color32 = Color32::from_rgb(255, 170, 70);

/// Rounded surface that groups a row of controls.
fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none().fill(CARD).rounding(Rounding::same(10.0)).inner_margin(egui::Margin::symmetric(10.0, 6.0)).show(ui, add).inner
}

/// Small pill with a read-only fact ("4 bars", "7 points").
fn chip(ui: &mut Ui, text: String) {
    egui::Frame::none().fill(CARD_HI).rounding(Rounding::same(20.0)).inner_margin(egui::Margin::symmetric(9.0, 2.0)).show(ui, |ui| {
        ui.label(RichText::new(text).small().color(MUTED));
    });
}

// ---------------------------------------------------------------------------------------------
// creating automation
// ---------------------------------------------------------------------------------------------

/// "Create automation clip" (and "Edit ..." when there already is one) for a single parameter.
/// Used in the right-click menu of sliders and knobs.
pub(crate) fn automation_context(ui: &mut Ui, s: &mut State, target: AutoTarget) {
    if ui.button("Create automation clip").on_hover_text("New automation clip for this parameter at the playhead").clicked() {
        s.create_automation(target);
        ui.close_menu();
    }
    for (n, id) in s.automation_clips_of(target).into_iter().enumerate() {
        if ui.button(format!("Edit automation clip {}", n + 1)).clicked() {
            s.open_auto_editor(id);
            ui.close_menu();
        }
    }
}

/// All parameters of one track (and the master / selected audio clip) as a menu: used by the
/// "+ Automation" button and the track header's right-click menu.
pub(crate) fn automation_target_menu(ui: &mut Ui, s: &mut State, track: usize) {
    let t16 = track as u16;
    ui.label(RichText::new(format!("Automate on {}", s.tracks[track].name)).small().color(MUTED));
    for (label, target) in [("Volume", AutoTarget::TrackGain(t16)), ("Pan", AutoTarget::TrackPan(t16)), ("Pitch (built-in synth / sampler)", AutoTarget::TrackPitch(t16)), ("Master volume", AutoTarget::MasterGain)] {
        if ui.button(label).clicked() {
            s.create_automation(target);
            ui.close_menu();
        }
    }
    // volume of the selected audio clip
    let clip = s.sel_clips.iter().find_map(|&id| {
        let i = s.clip_index(id)?;
        matches!(s.clips[i].body, ClipBody::Audio { .. }).then(|| (id, s.clips[i].name.clone()))
    });
    if let Some((id, name)) = clip {
        if ui.button(format!("Volume of clip \"{name}\"")).clicked() {
            s.create_automation(AutoTarget::ClipGain(id));
            ui.close_menu();
        }
    }
    // VST3 parameters: instrument and insert effects
    let mut plugs: Vec<(usize, String, Vec<(u32, String)>)> = Vec::new();
    for slot in 0..=FX_SLOTS {
        if let Some(c) = s.ctrls.get(&(track, slot)) {
            let label = if slot == 0 { s.tracks[track].inst.label() } else { format!("FX {}: {}", slot, s.tracks[track].fx[slot - 1].clone().unwrap_or_default()) };
            plugs.push((slot, label, c.params.iter().map(|p| (p.id, p.name.clone())).collect()));
        }
    }
    if !plugs.is_empty() {
        ui.separator();
    }
    for (slot, label, params) in plugs {
        ui.menu_button(label, |ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (id, name) in &params {
                    if ui.button(name.as_str()).clicked() {
                        s.create_automation(AutoTarget::Plugin { track: t16, slot: slot as u8, id: *id });
                        ui.close_menu();
                    }
                }
            });
        });
    }
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

fn auto_data(s: &mut State, id: u64) -> Option<&mut AutoData> {
    let i = s.clip_index(id)?;
    match &mut s.clips[i].body {
        ClipBody::Automation { auto } => Some(auto),
        _ => None,
    }
}

fn snap_tick(ed: &AutoEditor, t: f32, len: u32) -> u32 {
    let t = t.clamp(0.0, len as f32);
    if ed.snap_h && ed.grid_ticks > 1 {
        let g = ed.grid_ticks as f32;
        ((t / g).round() * g).clamp(0.0, len as f32) as u32
    } else {
        t.round() as u32
    }
}

fn snap_val(ed: &AutoEditor, v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if ed.snap_v && ed.grid_v > 0 {
        let n = ed.grid_v as f32;
        (v * n).round() / n
    } else {
        v
    }
}

/// "3:2:120 (5.25 s)" = bar 3, beat 2, 120 ticks, and the time at the project tempo.
fn time_text(tick: u32, bpm: f32) -> String {
    let bar = tick / BAR_TICKS + 1;
    let beat = (tick % BAR_TICKS) / PPQ + 1;
    let sub = tick % PPQ;
    let secs = tick as f32 / PPQ as f32 * 60.0 / bpm.max(1.0);
    format!("{bar}:{beat}:{sub:03}  ({secs:.2} s)")
}

fn insert_point(points: &mut Vec<AutoPoint>, tick: u32, value: f32, shape: Shape) -> usize {
    if let Some(i) = points.iter().position(|p| p.tick == tick) {
        points[i].value = value;
        points[i].shape = shape;
        return i;
    }
    let i = points.partition_point(|p| p.tick < tick);
    points.insert(i, AutoPoint { tick, value, shape });
    i
}

/// Move the points `orig` (index, original tick, original value) by `dt` ticks and `dv`, keeping
/// them between their unselected neighbours (so the order never changes) and inside 0..=len /
/// 0..1. Returns the movement that was really applied.
fn move_points(points: &mut [AutoPoint], orig: &[(usize, u32, f32)], dt: i64, dv: f32, len: u32) -> (i64, f32) {
    let sel: Vec<usize> = orig.iter().map(|o| o.0).collect();
    let (mut lo, mut hi) = (i64::MIN, i64::MAX);
    let (mut vlo, mut vhi) = (f32::MIN, f32::MAX);
    for &(i, t0, v0) in orig {
        if i >= points.len() {
            continue;
        }
        let prev = (0..i).rev().find(|j| !sel.contains(j)).map(|j| points[j].tick as i64 + 1).unwrap_or(0);
        let next = (i + 1..points.len()).find(|j| !sel.contains(j)).map(|j| points[j].tick as i64 - 1).unwrap_or(len as i64);
        lo = lo.max(prev - t0 as i64);
        hi = hi.min(next - t0 as i64);
        vlo = vlo.max(-v0);
        vhi = vhi.min(1.0 - v0);
    }
    let dt = dt.clamp(lo.min(0), hi.max(0));
    let dv = dv.clamp(vlo.min(0.0), vhi.max(0.0));
    for &(i, t0, v0) in orig {
        if let Some(p) = points.get_mut(i) {
            p.tick = (t0 as i64 + dt).max(0) as u32;
            p.value = (v0 + dv).clamp(0.0, 1.0);
        }
    }
    (dt, dv)
}

// ---------------------------------------------------------------------------------------------
// the editor window
// ---------------------------------------------------------------------------------------------

pub(crate) fn automation_editor(s: &mut State, ctx: &egui::Context) {
    let Some(mut ed) = s.auto_ed.take() else { return };
    let Some(ci) = s.clip_index(ed.id) else { return };
    if !matches!(s.clips[ci].body, ClipBody::Automation { .. }) {
        return;
    }
    let title = format!("Automation \u{2014} {}", match &s.clips[ci].body {
        ClipBody::Automation { auto } => s.target_name(auto.target),
        _ => String::new(),
    });
    let mut open = true;
    egui::Window::new(title).id(Id::new("auto_editor")).open(&mut open).default_size(vec2(780.0, 460.0)).resizable(true).collapsible(false).default_pos(pos2(300.0, 140.0)).frame(egui::Frame::window(&ctx.style()).fill(PANEL).rounding(Rounding::same(16.0)).inner_margin(egui::Margin::same(14.0))).show(ctx, |ui| {
        editor_body(ui, s, &mut ed);
    });
    if ed.pending_push {
        s.push_clips();
        ed.pending_push = false;
    }
    if open {
        s.auto_ed = Some(ed);
    }
}

fn editor_body(ui: &mut Ui, s: &mut State, ed: &mut AutoEditor) {
    let id = ed.id;
    let Some(ci) = s.clip_index(id) else { return };
    let (target, len, clip_start) = match &s.clips[ci].body {
        ClipBody::Automation { auto } => (auto.target, auto.len, s.clips[ci].start),
        _ => return,
    };
    let bpm = s.bpm;
    let mut changed = false;

    // current points (clean up a selection that points past the end)
    let mut points: Vec<AutoPoint> = auto_data(s, id).map(|a| a.points.clone()).unwrap_or_default();
    ed.sel.retain(|&i| i < points.len());

    // ---- toolbar ----
    let mut new_shape: Option<Shape> = None;
    let (mut do_copy, mut do_paste, mut do_delete, mut do_all) = (false, false, false, false);
    // title strip: what is automated, plus a few facts about the clip
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 6.0, CURVE.gamma_multiply(0.35));
        ui.painter().circle_filled(dot.center(), 3.5, CURVE);
        ui.label(RichText::new(s.target_name(target)).size(17.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            chip(ui, format!("{} points", points.len()));
            chip(ui, format!("{:.2} bars", len as f32 / BAR_TICKS as f32));
            chip(ui, format!("starts at bar {}", clip_start / BAR_TICKS + 1));
        });
    });
    ui.add_space(2.0);
    card(ui, |ui| ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut ed.snap_h, "Snap time");
        ui.add_enabled_ui(ed.snap_h, |ui| {
            let cur = SNAPS.iter().find(|(_, v)| *v == ed.grid_ticks).map(|(n, _)| *n).unwrap_or("1/16");
            egui::ComboBox::from_id_source("auto_snap_h").width(58.0).selected_text(cur).show_ui(ui, |ui| {
                for (n, v) in SNAPS {
                    if v > 1 {
                        ui.selectable_value(&mut ed.grid_ticks, v, n);
                    }
                }
            });
        });
        ui.checkbox(&mut ed.snap_v, "Snap value");
        ui.add_enabled_ui(ed.snap_v, |ui| {
            egui::ComboBox::from_id_source("auto_snap_v").width(58.0).selected_text(format!("{} steps", ed.grid_v)).show_ui(ui, |ui| {
                for n in [4u32, 8, 16, 32, 64, 100] {
                    ui.selectable_value(&mut ed.grid_v, n, format!("{n} steps"));
                }
            });
        });
        ui.separator();
        let cur_shape = ed.sel.first().and_then(|&i| points.get(i)).map(|p| p.shape).unwrap_or(s.auto_shape);
        ui.label(RichText::new("Curve").color(MUTED));
        egui::ComboBox::from_id_source("auto_shape").width(120.0).selected_text(cur_shape.label()).show_ui(ui, |ui| {
            for sh in Shape::ALL {
                if ui.selectable_label(sh == cur_shape, sh.label()).clicked() {
                    new_shape = Some(sh);
                }
            }
        })
        .response
        .on_hover_text("How the value travels from the selected point(s) to the next point. Also used for new points.");
        ui.separator();
        do_all = ui.button("Select all").clicked();
        do_copy = ui.button("Copy").on_hover_text("Ctrl+C").clicked();
        do_paste = ui.button("Paste").on_hover_text("Ctrl+V: at the last clicked position").clicked();
        do_delete = ui.add_enabled(!ed.sel.is_empty(), egui::Button::new("Delete")).on_hover_text("Delete / right-click a point").clicked();
        if ui.button("Fit").on_hover_text("Show the whole clip").clicked() {
            ed.fit = true;
            ed.v_lo = 0.0;
            ed.v_hi = 1.0;
        }
    }));
    ui.add_space(4.0);

    // ---- numbers: exact position / value of the selected point, clip length ----
    card(ui, |ui| ui.horizontal_wrapped(|ui| {
        let mut bars = len as f32 / BAR_TICKS as f32;
        ui.label(RichText::new("Length").color(MUTED));
        if ui.add(egui::DragValue::new(&mut bars).clamp_range(0.25..=512.0).speed(0.05).max_decimals(2).suffix(" bars")).changed() {
            let new_len = ((bars * BAR_TICKS as f32).round() as u32).max(PPQ);
            s.checkpoint_coalesced("auto-num");
            if let Some(a) = auto_data(s, id) {
                a.len = new_len;
            }
            changed = true;
        }
        if ed.sel.len() == 1 {
            let i = ed.sel[0];
            let p = points[i];
            let mut beats = p.tick as f32 / PPQ as f32;
            let mut val = p.value;
            ui.separator();
            ui.label(RichText::new("Point").color(MUTED));
            let tick_changed = ui.add(egui::DragValue::new(&mut beats).clamp_range(0.0..=(len as f32 / PPQ as f32)).speed(0.01).max_decimals(3).suffix(" beats")).changed();
            let val_changed = ui.add(egui::DragValue::new(&mut val).clamp_range(0.0..=1.0).speed(0.002).max_decimals(3)).changed();
            ui.label(RichText::new(target.format(val)).strong().color(ACCENT));
            if tick_changed || val_changed {
                s.checkpoint_coalesced("auto-num");
                if let Some(a) = auto_data(s, id) {
                    // stay between the neighbours so the order never changes
                    let lo = if i > 0 { a.points[i - 1].tick + 1 } else { 0 };
                    let hi = if i + 1 < a.points.len() { a.points[i + 1].tick.saturating_sub(1) } else { len };
                    a.points[i].tick = ((beats * PPQ as f32).round() as u32).clamp(lo.min(hi), hi.max(lo));
                    a.points[i].value = val.clamp(0.0, 1.0);
                }
                changed = true;
            }
        } else {
            ui.separator();
            ui.label(RichText::new(format!("{} point(s) selected", ed.sel.len())).color(MUTED));
        }
    }));
    ui.add_space(6.0);

    // ---- canvas ----
    let avail = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(vec2(avail.x.max(200.0), (avail.y - 34.0).max(150.0)), Sense::click_and_drag());
    if ed.fit {
        ed.ppt = ((rect.width() - 60.0) / len.max(1) as f32).clamp(0.004, 6.0);
        ed.scroll = -30.0 / ed.ppt;
        ed.fit = false;
    }

    // zoom / scroll
    if resp.hovered() {
        let (zoom, wheel, mods) = ui.input(|i| (i.zoom_delta(), i.raw_scroll_delta, i.modifiers));
        if let Some(p) = resp.hover_pos() {
            if zoom != 1.0 {
                let t_at = (p.x - rect.left()) / ed.ppt + ed.scroll;
                ed.ppt = (ed.ppt * zoom).clamp(0.004, 6.0);
                ed.scroll = t_at - (p.x - rect.left()) / ed.ppt;
            } else if mods.alt && wheel.y != 0.0 {
                let c = ed.v_lo + (rect.bottom() - p.y) / rect.height() * (ed.v_hi - ed.v_lo);
                let f = (1.0 - wheel.y * 0.003).clamp(0.5, 2.0);
                let range = ((ed.v_hi - ed.v_lo) * f).clamp(0.05, 1.0);
                let frac = (c - ed.v_lo) / (ed.v_hi - ed.v_lo).max(1e-6);
                ed.v_lo = (c - range * frac).clamp(0.0, 1.0 - range);
                ed.v_hi = ed.v_lo + range;
            } else if wheel != egui::Vec2::ZERO {
                ed.scroll -= (wheel.x + wheel.y) / ed.ppt;
            }
        }
    }
    if resp.dragged_by(PointerButton::Middle) {
        ed.scroll -= resp.drag_delta().x / ed.ppt;
    }
    ed.scroll = ed.scroll.clamp(-60.0 / ed.ppt, len.max(1) as f32);

    // coordinate helpers (copies, so they do not borrow `ed`)
    let (ppt, scroll, v_lo, v_hi) = (ed.ppt, ed.scroll, ed.v_lo, ed.v_hi);
    let x_of = move |t: f32| rect.left() + (t - scroll) * ppt;
    let y_of = move |v: f32| rect.bottom() - (v - v_lo) / (v_hi - v_lo).max(1e-6) * rect.height();
    let tick_at = move |x: f32| (x - rect.left()) / ppt + scroll;
    let val_at = move |y: f32| v_lo + (rect.bottom() - y) / rect.height() * (v_hi - v_lo);
    let hit_at = |p: Pos2, pts: &[AutoPoint]| -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, pt) in pts.iter().enumerate() {
            let d = pos2(x_of(pt.tick as f32), y_of(pt.value)).distance(p);
            if d <= HIT_R && best.map_or(true, |b| d < b.1) {
                best = Some((i, d));
            }
        }
        best.map(|b| b.0)
    };

    // keyboard shortcuts only while the pointer is over the canvas (the playlist must not react)
    let active = resp.hovered() || resp.dragged();
    if active {
        s.focus_playlist = false;
        let (del, all, copy, paste) = ui.input(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.command;
            (
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                ctrl && i.key_pressed(egui::Key::A),
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)) || (ctrl && i.key_pressed(egui::Key::C)),
                i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))) || (ctrl && i.key_pressed(egui::Key::V)),
            )
        });
        do_delete |= del && !ed.sel.is_empty();
        do_all |= all;
        do_copy |= copy;
        do_paste |= paste;
    }

    // ---- mouse interaction ----
    let shift = ui.input(|i| i.modifiers.shift);
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            match hit_at(p, &points) {
                Some(i) => {
                    if shift {
                        if let Some(pos) = ed.sel.iter().position(|&x| x == i) {
                            ed.sel.remove(pos);
                        } else {
                            ed.sel.push(i);
                        }
                    } else {
                        ed.sel = vec![i];
                    }
                }
                None => {
                    if !shift {
                        ed.sel.clear();
                    }
                    ed.cursor_tick = snap_tick(ed, tick_at(p.x), len);
                }
            }
        }
    }
    if resp.double_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if hit_at(p, &points).is_none() {
                let (t, v, sh) = (snap_tick(ed, tick_at(p.x), len), snap_val(ed, val_at(p.y)), s.auto_shape);
                s.checkpoint();
                if let Some(a) = auto_data(s, id) {
                    let i = insert_point(&mut a.points, t, v, sh);
                    ed.sel = vec![i];
                    ed.cursor_tick = t;
                    changed = true;
                }
            }
        }
    }
    if resp.secondary_clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(|p| hit_at(p, &points)) {
            if points.len() > 1 {
                s.checkpoint();
                if let Some(a) = auto_data(s, id) {
                    a.points.remove(i);
                    ed.sel.clear();
                    changed = true;
                }
            } else {
                s.toast("An automation clip keeps at least one point", false);
            }
        }
    }
    if resp.drag_started_by(PointerButton::Primary) {
        if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
            match hit_at(o, &points) {
                Some(i) => {
                    if !ed.sel.contains(&i) {
                        if shift {
                            ed.sel.push(i);
                        } else {
                            ed.sel = vec![i];
                        }
                    }
                    // the grabbed point first: it decides the snapping
                    let mut orig: Vec<(usize, u32, f32)> = vec![(i, points[i].tick, points[i].value)];
                    for &j in &ed.sel {
                        if j != i {
                            orig.push((j, points[j].tick, points[j].value));
                        }
                    }
                    s.checkpoint();
                    ed.drag = Some(AutoDrag::Points { orig, grab_tick: tick_at(o.x), grab_val: val_at(o.y) });
                }
                None => {
                    if !shift {
                        ed.sel.clear();
                    }
                    ed.drag = Some(AutoDrag::Band { t0: tick_at(o.x), v0: val_at(o.y) });
                }
            }
        }
    }
    let mut tip: Option<(Pos2, String)> = None;
    let mut band: Option<Rect> = None;
    if resp.dragged_by(PointerButton::Primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            let drag_points = match &ed.drag {
                Some(AutoDrag::Points { orig, grab_tick, grab_val }) => Some((orig.clone(), *grab_tick, *grab_val)),
                _ => None,
            };
            let drag_band = match &ed.drag {
                Some(AutoDrag::Band { t0, v0 }) => Some((*t0, *v0)),
                _ => None,
            };
            if let Some((orig, grab_tick, grab_val)) = drag_points {
                let (anchor, t0, v0) = orig[0];
                let want_t = snap_tick(ed, t0 as f32 + (tick_at(p.x) - grab_tick), len) as i64;
                let want_v = snap_val(ed, v0 + (val_at(p.y) - grab_val));
                if let Some(a) = auto_data(s, id) {
                    move_points(&mut a.points, &orig, want_t - t0 as i64, want_v - v0, len);
                    if let Some(pt) = a.points.get(anchor) {
                        tip = Some((p + vec2(16.0, -34.0), format!("{}  \u{b7}  {}", time_text(pt.tick, bpm), target.format(pt.value))));
                    }
                    changed = true;
                }
            } else if let Some((bt0, bv0)) = drag_band {
                let (bt1, bv1) = (tick_at(p.x), val_at(p.y));
                let (tl, th) = (bt0.min(bt1), bt0.max(bt1));
                let (vl, vh) = (bv0.min(bv1), bv0.max(bv1));
                ed.sel = points.iter().enumerate().filter(|(_, pt)| (pt.tick as f32) >= tl && (pt.tick as f32) <= th && pt.value >= vl && pt.value <= vh).map(|(i, _)| i).collect();
                band = Some(Rect::from_two_pos(pos2(x_of(bt0), y_of(bv0)), p));
            }
        }
    }
    if resp.drag_stopped() {
        ed.drag = None;
        changed = true;
    }

    // ---- toolbar actions ----
    if let Some(sh) = new_shape {
        s.auto_shape = sh;
        if !ed.sel.is_empty() {
            s.checkpoint();
            if let Some(a) = auto_data(s, id) {
                for &i in &ed.sel {
                    if let Some(p) = a.points.get_mut(i) {
                        p.shape = sh;
                    }
                }
            }
            changed = true;
        }
    }
    if do_all {
        ed.sel = (0..points.len()).collect();
    }
    if do_copy && !ed.sel.is_empty() {
        let mut picked: Vec<AutoPoint> = ed.sel.iter().filter_map(|&i| points.get(i).copied()).collect();
        picked.sort_by_key(|p| p.tick);
        let base = picked.first().map(|p| p.tick).unwrap_or(0);
        s.auto_clipboard = picked.into_iter().map(|p| AutoPoint { tick: p.tick - base, ..p }).collect();
        let n = s.auto_clipboard.len();
        s.toast(format!("Copied {n} point(s)"), false);
    }
    if do_delete && !ed.sel.is_empty() {
        if ed.sel.len() < points.len() {
            s.checkpoint();
            let sel = ed.sel.clone();
            if let Some(a) = auto_data(s, id) {
                let mut k = 0;
                a.points.retain(|_| {
                    let keep = !sel.contains(&k);
                    k += 1;
                    keep
                });
            }
            ed.sel.clear();
            changed = true;
        } else {
            s.toast("An automation clip keeps at least one point", false);
        }
    }
    if do_paste && !s.auto_clipboard.is_empty() {
        let clip = s.auto_clipboard.clone();
        let at = ed.cursor_tick;
        let span = clip.last().map(|p| p.tick).unwrap_or(0);
        s.checkpoint();
        if let Some(a) = auto_data(s, id) {
            // the pasted section replaces what was in its time range
            a.points.retain(|p| p.tick < at || p.tick > at + span);
            let mut new_sel: Vec<u32> = Vec::new();
            for p in &clip {
                insert_point(&mut a.points, at + p.tick, p.value, p.shape);
                new_sel.push(at + p.tick);
            }
            // the clip grows when the section sticks out
            if at + span > a.len {
                a.len = ((at + span + PPQ - 1) / PPQ) * PPQ;
            }
            ed.sel = new_sel.iter().filter_map(|t| a.points.iter().position(|p| p.tick == *t)).collect();
        }
        changed = true;
    }

    // ---- paint ----
    points = auto_data(s, id).map(|a| a.points.clone()).unwrap_or_default();
    let len_now = auto_data(s, id).map(|a| a.len).unwrap_or(len);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 12.0, Color32::from_rgb(15, 16, 22));
    // the clip's own range is lighter than the space around it
    let (cx0, cx1) = (x_of(0.0).max(rect.left()), x_of(len_now as f32).min(rect.right()));
    if cx1 > cx0 {
        painter.rect_filled(Rect::from_min_max(pos2(cx0, rect.top()), pos2(cx1, rect.bottom())), 0.0, Color32::from_rgb(24, 26, 35));
    }
    // vertical grid: bars, beats, sixteenths when there is room
    let (t_left, t_right) = (tick_at(rect.left()), tick_at(rect.right()));
    for (step, alpha, label) in [(BAR_TICKS, 40u8, true), (PPQ, 18, false), (PPQ / 4, 8, false)] {
        let px = step as f32 * ppt;
        let min_px: f32 = if label { 6.0 } else { 14.0 };
        if px < min_px {
            continue;
        }
        let mut t = ((t_left.max(0.0) / step as f32).floor() as u32) * step;
        while (t as f32) <= t_right.min(len_now as f32 + step as f32) {
            let x = x_of(t as f32);
            if x >= rect.left() && x <= rect.right() {
                painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0_f32, Color32::from_white_alpha(alpha)));
                if label {
                    painter.text(pos2(x + 4.0, rect.top() + 4.0), Align2::LEFT_TOP, format!("{}", t / BAR_TICKS + 1), FontId::proportional(10.0), MUTED);
                }
            }
            t += step;
        }
    }
    // horizontal grid with value labels
    let div = if ed.snap_v { ed.grid_v.clamp(2, 16) } else { 4 };
    for k in 0..=div {
        let v = k as f32 / div as f32;
        let y = y_of(v);
        if y < rect.top() - 1.0 || y > rect.bottom() + 1.0 {
            continue;
        }
        let edge = k == 0 || k == div;
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0_f32, Color32::from_white_alpha(if edge { 26 } else { 10 })));
        painter.text(pos2(rect.left() + 8.0, y - 2.0), Align2::LEFT_BOTTOM, target.format(v), FontId::proportional(9.5), MUTED.gamma_multiply(0.85));
    }
    // the curve: soft glow, fill that fades out downwards, crisp line on top
    let (xa, xb) = (x_of(0.0).max(rect.left()), x_of(len_now as f32).min(rect.right()));
    if xb > xa {
        let mut line: Vec<Pos2> = Vec::new();
        let mut x = xa;
        while x < xb {
            let t = tick_at(x).clamp(0.0, len_now as f32);
            line.push(pos2(x, y_of(eval_points(&points, t))));
            x += 2.0;
        }
        line.push(pos2(xb, y_of(eval_points(&points, tick_at(xb).clamp(0.0, len_now as f32)))));
        let mut mesh = egui::Mesh::default();
        let (top_c, bot_c) = (Color32::from_rgba_unmultiplied(255, 170, 70, 80), Color32::from_rgba_unmultiplied(255, 170, 70, 0));
        for (k, p) in line.iter().enumerate() {
            mesh.colored_vertex(*p, top_c);
            mesh.colored_vertex(pos2(p.x, rect.bottom()), bot_c);
            if k > 0 {
                let i = (k * 2) as u32;
                mesh.add_triangle(i - 2, i - 1, i);
                mesh.add_triangle(i - 1, i + 1, i);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::line(line.clone(), Stroke::new(7.0_f32, CURVE.gamma_multiply(0.12))));
        painter.add(egui::Shape::line(line, Stroke::new(2.2_f32, CURVE)));
    }
    // crosshair under the pointer
    if let Some(p) = resp.hover_pos() {
        painter.line_segment([pos2(p.x, rect.top()), pos2(p.x, rect.bottom())], Stroke::new(1.0_f32, Color32::from_white_alpha(14)));
    }
    // points
    let hover_pt = resp.hover_pos().and_then(|p| hit_at(p, &points));
    for (i, pt) in points.iter().enumerate() {
        let c = pos2(x_of(pt.tick as f32), y_of(pt.value));
        if !rect.expand(8.0).contains(c) {
            continue;
        }
        let sel = ed.sel.contains(&i);
        let hot = hover_pt == Some(i);
        let r = if sel || hot { POINT_R + 1.5 } else { POINT_R };
        if sel || hot {
            painter.circle_filled(c, r + 5.0, ACCENT.gamma_multiply(if sel { 0.28 } else { 0.16 }));
        }
        painter.circle_filled(c, r, if sel { Color32::WHITE } else { Color32::from_rgb(24, 26, 35) });
        painter.circle_stroke(c, r, Stroke::new(2.0_f32, if sel { ACCENT } else { CURVE }));
        if pt.shape != Shape::Linear {
            painter.text(c + vec2(0.0, 12.0), Align2::CENTER_TOP, pt.shape.label().split(' ').next().unwrap_or(""), FontId::proportional(9.0), MUTED);
        }
    }
    // playback position
    if s.playing && s.playhead >= clip_start {
        let x = x_of((s.playhead - clip_start) as f32);
        if x >= rect.left() && x <= rect.right() {
            painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.5_f32, GREEN));
        }
    }
    // paste position
    let px = x_of(ed.cursor_tick as f32);
    if px >= rect.left() && px <= rect.right() {
        painter.add(egui::Shape::convex_polygon(vec![pos2(px - 5.0, rect.top()), pos2(px + 5.0, rect.top()), pos2(px, rect.top() + 8.0)], ACCENT, Stroke::NONE));
    }
    if let Some(b) = band {
        painter.rect_filled(b, 2.0, ACCENT.gamma_multiply(0.18));
        painter.rect_stroke(b, 2.0, Stroke::new(1.0_f32, ACCENT));
    }
    painter.rect_stroke(rect, 12.0, Stroke::new(1.0_f32, STROKE));
    // time / value of the point under the pointer (or being dragged)
    if tip.is_none() {
        if let (Some(i), Some(p)) = (hover_pt, resp.hover_pos()) {
            let pt = points[i];
            tip = Some((p + vec2(16.0, -34.0), format!("{}  \u{b7}  {}", time_text(pt.tick, bpm), target.format(pt.value))));
        }
    }
    s.drag_hint = tip;

    // ---- bottom row: zoom, scroll, readout ----
    ui.horizontal(|ui| {
        ui.label(RichText::new("Zoom").color(MUTED));
        ui.spacing_mut().slider_width = 110.0;
        let before = ed.ppt;
        ui.add(egui::Slider::new(&mut ed.ppt, 0.004..=6.0).logarithmic(true).show_value(false));
        if (ed.ppt - before).abs() > f32::EPSILON {
            // keep the middle of the view where it was
            let mid = ((rect.width() * 0.5) / before) + ed.scroll;
            ed.scroll = mid - (rect.width() * 0.5) / ed.ppt;
        }
        ui.label(RichText::new("Scroll").color(MUTED));
        ui.add(egui::Slider::new(&mut ed.scroll, (-60.0 / ed.ppt)..=len_now.max(1) as f32).show_value(false));
        if let Some(p) = resp.hover_pos() {
            ui.label(RichText::new(format!("{}  \u{b7}  {}", time_text(tick_at(p.x).max(0.0) as u32, bpm), target.format(val_at(p.y).clamp(0.0, 1.0)))).small().color(MUTED));
        } else {
            ui.label(RichText::new("Double-click: add point \u{b7} drag: move / select \u{b7} right-click point: delete").small().color(MUTED));
        }
    });

    if changed {
        ed.pending_push = true;
    }
    // while a point is being dragged the engine is updated at most every 60 ms
    if ed.pending_push && (ed.drag.is_none() || ed.last_push.elapsed() >= Duration::from_millis(60)) {
        s.push_clips();
        ed.pending_push = false;
        ed.last_push = Instant::now();
    }
}
