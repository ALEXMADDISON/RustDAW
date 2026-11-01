//! Piano roll.
//! Edits a pattern: open one by double-clicking it in the Patterns window or a pattern clip in the
//! playlist. Every playlist clip that uses the pattern follows the edits.
//!   click empty cell      -> create a note (and drag straight away to move it)
//!   drag a note           -> move (pitch + time, snapped); the whole selection moves together
//!   drag a note's right edge -> resize
//!   right-click / right-drag -> delete notes under the pointer
//!   shift + drag empty    -> rubber-band select;  ctrl + click -> add to selection
//!   Delete / Ctrl+A       -> delete / select all (handled in app.rs)
use crate::state::*;
use crate::theme::*;
use core_engine::{tools, Note, PPQ};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CursorIcon, FontId, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui};

const KEY_LOW: u8 = 24; // C1
const KEY_HIGH: u8 = 97; // exclusive (up to C7)
const KEY_W: f32 = 52.0;
const KEY_H: f32 = 15.0;
const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Topmost note (of `ch`) under (tick, key); bool = pointer is on its right-edge handle.
fn hit_note(notes: &[Note], tick: u32, key: u8, edge: u32) -> Option<(usize, bool)> {
    for (i, n) in notes.iter().enumerate().rev() {
        if n.key == key && tick >= n.start && tick < n.start + n.len {
            return Some((i, n.len > edge * 2 && tick + edge >= n.start + n.len));
        }
    }
    None
}

fn create_note(s: &mut State, tick: u32, key: u8) -> usize {
    let pt = s.roll_len;
    let start = (tick / s.snap) * s.snap;
    let len = s.last_len.min(pt.saturating_sub(start)).max(1);
    s.roll_notes.push(Note { key, velocity: 100, start, len });
    s.roll_sel = vec![s.roll_notes.len() - 1];
    s.roll_notes.len() - 1
}

pub(crate) fn piano_roll(s: &mut State, ui: &mut Ui) {
    let Some(pid) = s.editing_pattern.filter(|&p| s.pat_index(p).is_some()) else {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No pattern open").size(16.0).color(MUTED));
            ui.label(RichText::new("Double-click a pattern in the Patterns window or a pattern clip in the Playlist.").color(MUTED));
            ui.add_space(8.0);
            if ui.button("+ New pattern").clicked() {
                let p = s.add_pattern();
                s.open_pattern(p, None);
            }
        });
        return;
    };
    let pname: String = s.pat_index(pid).map(|i| s.patterns[i].name.clone()).unwrap_or_default();
    // The clip the pattern was opened from decides the track that plays the notes.
    let clip_ctx = s.editing_clip.and_then(|id| s.clip_index(id));
    let track: usize = clip_ctx.map(|ci| s.clips[ci].track).unwrap_or(s.selected_track);
    let clip_start: Option<u32> = clip_ctx.map(|ci| s.clips[ci].start);
    let clip_len: Option<u32> = clip_ctx.map(|ci| s.clips[ci].len_ticks(s.bpm, s.sample_rate));
    let mut changed = false;
    let mut switch_to: Option<u64> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{} · {}", s.tracks[track].name, pname)).strong().color(track_color(track)));
        egui::ComboBox::from_id_source("roll_pat").width(120.0).selected_text(pname.as_str()).show_ui(ui, |ui| {
            for p in &s.patterns {
                if ui.selectable_label(p.id == pid, p.name.as_str()).clicked() {
                    switch_to = Some(p.id);
                }
            }
        });
        ui.label(RichText::new("Snap").color(MUTED));
        egui::ComboBox::from_id_source("snap").width(60.0).selected_text(match s.snap {
            x if x == PPQ => "1/4",
            x if x == PPQ / 2 => "1/8",
            x if x == PPQ / 4 => "1/16",
            _ => "1/32",
        }).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.snap, PPQ, "1/4");
            ui.selectable_value(&mut s.snap, PPQ / 2, "1/8");
            ui.selectable_value(&mut s.snap, PPQ / 4, "1/16");
            ui.selectable_value(&mut s.snap, PPQ / 8, "1/32");
        });
        ui.label(RichText::new("Length").color(MUTED));
        let before = s.roll_len;
        let bars = (s.roll_len / BAR_TICKS).max(1);
        egui::ComboBox::from_id_source("bars").width(64.0).selected_text(format!("{} bar", bars)).show_ui(ui, |ui| {
            for b in [1u32, 2, 4, 8, 16] {
                if ui.selectable_label(bars == b, format!("{b} bar")).clicked() {
                    s.roll_len = b * BAR_TICKS;
                }
            }
        });
        if s.roll_len != before {
            s.roll_notes.retain(|n| n.start < s.roll_len);
            changed = true;
        }
        ui.label(RichText::new("Scale").color(MUTED));
        egui::ComboBox::from_id_source("root").width(48.0).selected_text(NOTE_NAMES[s.root as usize]).show_ui(ui, |ui| {
            for r in 0..12u8 {
                ui.selectable_value(&mut s.root, r, NOTE_NAMES[r as usize]);
            }
        });
        egui::ComboBox::from_id_source("scale").width(92.0).selected_text(match s.scale {
            Scale::Chromatic => "Chromatic",
            Scale::Major => "Major",
            Scale::Minor => "Minor",
        }).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.scale, Scale::Chromatic, "Chromatic");
            ui.selectable_value(&mut s.scale, Scale::Major, "Major");
            ui.selectable_value(&mut s.scale, Scale::Minor, "Minor");
        });
        if ui.button("Quantize").clicked() {
            let snap = s.snap;
            if s.roll_sel.is_empty() {
                tools::quantize(&mut s.roll_notes, snap);
            } else {
                for i in s.roll_sel.clone() {
                    tools::quantize(std::slice::from_mut(&mut s.roll_notes[i]), snap);
                }
            }
            changed = true;
        }
        if ui.button("Chop \u{d7}2").clicked() {
            tools::chop(&mut s.roll_notes, 2);
            s.roll_sel.clear();
            changed = true;
        }
        if ui.button("Clear").clicked() {
            s.roll_notes.clear();
            s.roll_sel.clear();
            changed = true;
        }
        ui.label(RichText::new("Zoom").color(MUTED));
        ui.add(egui::Slider::new(&mut s.zoom, 0.06..=0.6).show_value(false));
    });
    if let Some(p) = switch_to {
        if changed {
            s.sync_roll();
        }
        let clip = s.clips.iter().find(|c| matches!(&c.body, ClipBody::Pattern { pat, .. } if *pat == p)).map(|c| c.id);
        s.open_pattern(p, clip);
        return;
    }
    ui.label(RichText::new("Click: add note \u{b7} drag note: move \u{b7} drag right edge: resize \u{b7} right-click: delete \u{b7} Shift+drag: select").small().color(MUTED));
    ui.add_space(2.0);

    let ptick = s.roll_len;
    let ppt = s.zoom;
    let rows = (KEY_HIGH - KEY_LOW) as f32;
    let size = vec2(KEY_W + ptick as f32 * ppt + 20.0, rows * KEY_H);
    let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
    let shift = ui.input(|i| i.modifiers.shift);

    let mut area = egui::ScrollArea::both().id_source("roll_scroll").auto_shrink([false, false]);
    if !s.roll_init {
        s.roll_init = true;
        area = area.vertical_scroll_offset((KEY_HIGH - 1 - 79) as f32 * KEY_H);
    }
    area.show(ui, |ui| {
        let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = resp.rect;
        let left = rect.left() + KEY_W;
        let top = rect.top();
        let right = left + ptick as f32 * ppt;
        let x_of = |tick: f32| left + tick * ppt;
        let y_of = |key: u8| top + (KEY_HIGH - 1 - key) as f32 * KEY_H;
        let tk = |p: Pos2| -> (u32, u8) {
            let tick = ((p.x - left) / ppt).max(0.0) as u32;
            let row = (((p.y - top) / KEY_H) as i32).clamp(0, rows as i32 - 1);
            (tick, (KEY_HIGH as i32 - 1 - row) as u8)
        };
        let edge = (8.0 / ppt) as u32;

        // --- background rows + keyboard ---
        for key in KEY_LOW..KEY_HIGH {
            let black = [1, 3, 6, 8, 10].contains(&(key % 12));
            let in_scale = s.scale.contains(key, s.root);
            let row = Rect::from_min_size(pos2(left, y_of(key)), vec2(right - left, KEY_H));
            let mut bg = if black { Color32::from_rgb(22, 24, 30) } else { Color32::from_rgb(31, 33, 39) };
            if s.scale != Scale::Chromatic && in_scale {
                bg = Color32::from_rgb(bg.r() + 8, bg.g() + 14, bg.b() + 24);
            }
            painter.rect_filled(row, 0.0, bg);
            painter.line_segment([pos2(left, y_of(key) + KEY_H), pos2(right, y_of(key) + KEY_H)], Stroke::new(1.0_f32, Color32::from_rgb(38, 40, 46)));
            let kr = Rect::from_min_size(pos2(rect.left(), y_of(key)), vec2(KEY_W, KEY_H));
            painter.rect_filled(kr, 0.0, if black { Color32::from_rgb(13, 15, 21) } else { Color32::from_rgb(225, 227, 233) });
            painter.rect_stroke(kr, 0.0, Stroke::new(1.0_f32, Color32::from_rgb(55, 57, 63)));
            if key % 12 == 0 {
                painter.text(kr.right_center() - vec2(5.0, 0.0), Align2::RIGHT_CENTER, format!("C{}", key / 12 - 1), FontId::proportional(10.0), Color32::from_rgb(49, 51, 57));
            }
        }
        // --- grid ---
        let mut t = 0;
        while t <= ptick {
            let col = if t % BAR_TICKS == 0 { Color32::from_rgb(112, 112, 122) } else if t % PPQ == 0 { Color32::from_rgb(70, 70, 78) } else { Color32::from_rgb(43, 45, 51) };
            painter.line_segment([pos2(x_of(t as f32), top), pos2(x_of(t as f32), top + size.y)], Stroke::new(1.0_f32, col));
            t += PPQ / 4;
        }

        // --- notes ---
        for (i, n) in s.roll_notes.iter().enumerate() {
            if n.key < KEY_LOW || n.key >= KEY_HIGH {
                continue;
            }
            let own = true;
            let sel = s.roll_sel.contains(&i);
            let r = Rect::from_min_size(pos2(x_of(n.start as f32), y_of(n.key)), vec2((n.len as f32 * ppt).max(4.0), KEY_H)).shrink2(vec2(0.0, 1.0));
            let mut col = track_color(track);
            if !own {
                col = col.gamma_multiply(0.35);
            } else if sel {
                col = col.gamma_multiply(1.25);
            }
            painter.rect_filled(r, 3.0, col);
            painter.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 3.0)), 3.0, Color32::from_white_alpha(70));
            if own {
                painter.rect_filled(Rect::from_min_max(pos2(r.right() - 3.0, r.top()), r.max), 2.0, Color32::from_black_alpha(70));
            }
            if sel {
                painter.rect_stroke(r, 3.0, Stroke::new(1.5_f32, Color32::WHITE));
            }
        }

        // --- hover affordances + cursor ---
        if s.roll_drag.is_none() {
            if let Some(p) = resp.hover_pos() {
                if p.x >= left && p.x <= right {
                    let (tick, key) = tk(p);
                    match hit_note(&s.roll_notes, tick, key, edge) {
                        Some((_, true)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                        Some((_, false)) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                        None => {
                            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                            let start = (tick / s.snap) * s.snap;
                            let g = Rect::from_min_size(pos2(x_of(start as f32), y_of(key)), vec2((s.last_len as f32 * ppt).max(4.0), KEY_H)).shrink2(vec2(0.0, 1.0));
                            painter.rect_filled(g, 3.0, track_color(track).gamma_multiply(0.3));
                        }
                    }
                }
            }
        }

        // --- right button: delete under pointer (click or drag = eraser) ---
        if (resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.secondary_down())) || resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if p.x >= left {
                    let (tick, key) = tk(p);
                    if let Some((i, _)) = hit_note(&s.roll_notes, tick, key, edge) {
                        s.remove_note(i);
                        changed = true;
                    }
                }
            }
        }

        // --- primary drag start ---
        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
                if o.x >= left {
                    let (tick, key) = tk(o);
                    match hit_note(&s.roll_notes, tick, key, edge) {
                        Some((i, is_edge)) => {
                            if !s.roll_sel.contains(&i) {
                                if ctrl {
                                    s.roll_sel.push(i);
                                } else {
                                    s.roll_sel = vec![i];
                                }
                            }
                            let orig: Vec<_> = s.roll_sel.iter().map(|&j| (j, s.roll_notes[j].start, s.roll_notes[j].key, s.roll_notes[j].len)).collect();
                            s.roll_drag = Some(if is_edge {
                                RollDrag::Resize { anchor_tick: tick, orig }
                            } else {
                                RollDrag::Move { anchor_tick: tick, anchor_key: key as i32, orig }
                            });
                            if !is_edge {
                                let k = s.roll_notes[i].key;
                                s.start_audition(track, k);
                            }
                        }
                        None if shift => s.roll_drag = Some(RollDrag::Marquee { from: o, to: o }),
                        None => {
                            // New note created under the press point; dragging moves it.
                            let i = create_note(s, tick, key);
                            let n = s.roll_notes[i];
                            s.roll_drag = Some(RollDrag::Move { anchor_tick: tick, anchor_key: key as i32, orig: vec![(i, n.start, n.key, n.len)] });
                            s.start_audition(track, key);
                        }
                    }
                }
            }
        } else if resp.dragged_by(PointerButton::Primary) {
            if let (Some(p), Some(drag)) = (resp.interact_pointer_pos(), s.roll_drag.take()) {
                let (tick, key) = tk(p);
                let snap = s.snap;
                let drag = match drag {
                    RollDrag::Move { anchor_tick, anchor_key, orig } => {
                        let dt = s.snap_round((tick as f32 - anchor_tick as f32).abs(), snap) as i64 * if tick >= anchor_tick { 1 } else { -1 };
                        let dk = key as i32 - anchor_key;
                        let first_key_before = orig.first().map(|o| s.roll_notes[o.0].key);
                        for &(i, st, k, len) in &orig {
                            s.roll_notes[i].start = (st as i64 + dt).clamp(0, ptick as i64 - len as i64) as u32;
                            s.roll_notes[i].key = (k as i32 + dk).clamp(KEY_LOW as i32, KEY_HIGH as i32 - 1) as u8;
                        }
                        if let (Some(before), Some(o)) = (first_key_before, orig.first()) {
                            let now = s.roll_notes[o.0].key;
                            if now != before {
                                s.start_audition(track, now);
                            }
                        }
                        RollDrag::Move { anchor_tick, anchor_key, orig }
                    }
                    RollDrag::Resize { anchor_tick, orig } => {
                        let dt = s.snap_round((tick as f32 - anchor_tick as f32).abs(), snap) as i64 * if tick >= anchor_tick { 1 } else { -1 };
                        for &(i, st, _, len) in &orig {
                            s.roll_notes[i].len = (len as i64 + dt).clamp(snap as i64, (ptick - st) as i64) as u32;
                        }
                        if let Some(o) = orig.first() {
                            s.last_len = s.roll_notes[o.0].len;
                        }
                        RollDrag::Resize { anchor_tick, orig }
                    }
                    RollDrag::Marquee { from, .. } => RollDrag::Marquee { from, to: p },
                };
                s.roll_drag = Some(drag);
            }
        }

        // --- drag end ---
        if resp.drag_stopped() {
            match s.roll_drag.take() {
                Some(RollDrag::Marquee { from, to }) => {
                    let sel = Rect::from_two_pos(from, to);
                    s.roll_sel = (0..s.roll_notes.len())
                        .filter(|&i| {
                            let n = s.roll_notes[i];
                            Rect::from_min_size(pos2(x_of(n.start as f32), y_of(n.key)), vec2(n.len as f32 * ppt, KEY_H)).intersects(sel)
                        })
                        .collect();
                }
                Some(RollDrag::Move { orig, .. }) => {
                    if let Some(o) = orig.first() {
                        s.last_len = s.roll_notes[o.0].len;
                    }
                    changed = true;
                }
                Some(RollDrag::Resize { .. }) => changed = true,
                None => {}
            }
            s.stop_audition();
        }
        if let Some(RollDrag::Marquee { from, to }) = &s.roll_drag {
            let r = Rect::from_two_pos(*from, *to);
            painter.rect_filled(r, 2.0, ACCENT.gamma_multiply(0.18));
            painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, ACCENT));
        }

        // --- plain click ---
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                let (tick, key) = tk(p);
                if p.x < left {
                    s.start_audition(track, key); // click on the keyboard
                } else {
                    match hit_note(&s.roll_notes, tick, key, edge) {
                        Some((i, _)) => {
                            if ctrl {
                                if let Some(pos) = s.roll_sel.iter().position(|&j| j == i) {
                                    s.roll_sel.remove(pos);
                                } else {
                                    s.roll_sel.push(i);
                                }
                            } else {
                                s.roll_sel = vec![i];
                            }
                            s.start_audition(track, key);
                        }
                        None => {
                            create_note(s, tick, key);
                            s.start_audition(track, key);
                            changed = true;
                        }
                    }
                }
            }
        }

        // --- playhead ---
        if s.playing {
            if let (Some(cs), Some(cl)) = (clip_start, clip_len) {
                let rel = s.playhead as i64 - cs as i64;
                if rel >= 0 && (rel as u32) < cl {
                    // the pattern repeats inside a longer clip
                    let x = x_of((rel as u32 % ptick.max(1)) as f32);
                    painter.line_segment([pos2(x, top), pos2(x, top + size.y)], Stroke::new(2.0_f32, Color32::WHITE));
                }
            }
        }
    });
    if changed {
        s.sync_roll();
    }
}
