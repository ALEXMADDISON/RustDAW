//! Patterns window: make, clone, rename and delete patterns.
//!   click = select (the playlist places the selected pattern);  double-click = edit in the piano roll
//!   drag a pattern onto a playlist lane = place a clip;  right-click = menu
use crate::state::*;
use crate::theme::*;
use crate::widgets::*;
use core_engine::Note;
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui};

const ROW_H: f32 = 44.0;

/// Everything a row needs, copied out of the state so the list can be drawn while editing it.
struct Row {
    id: u64,
    name: String,
    notes: Vec<Note>,
    len: u32,
    clips: usize,
}

fn bars_label(len: u32) -> String {
    if len % BAR_TICKS == 0 {
        let b = len / BAR_TICKS;
        format!("{} bar{}", b, if b == 1 { "" } else { "s" })
    } else {
        format!("{:.2} bars", len as f32 / BAR_TICKS as f32)
    }
}

fn begin_rename(s: &mut State, id: u64) {
    if let Some(i) = s.pat_index(id) {
        s.pat_rename_buf = s.patterns[i].name.clone();
        s.pat_rename = Some(id);
    }
}

/// Empty, unused patterns go away at once; anything else asks first.
fn request_delete(s: &mut State, id: u64) {
    let Some(i) = s.pat_index(id) else { return };
    if s.patterns[i].notes.is_empty() && s.pattern_clip_count(id) == 0 {
        s.delete_pattern(id);
    } else {
        s.pat_confirm_delete = Some(id);
    }
}

pub(crate) fn patterns_panel(s: &mut State, ui: &mut Ui) {
    // ---- toolbar ----
    let selected = s.selected_pattern.filter(|&p| s.pat_index(p).is_some());
    ui.horizontal_wrapped(|ui| {
        if ui.button("+ New").on_hover_text("Create an empty pattern").clicked() {
            let id = s.add_pattern();
            s.open_pattern(id, None);
        }
        ui.add_enabled_ui(selected.is_some(), |ui| {
            if ui.button("Clone").on_hover_text("Copy the selected pattern").clicked() {
                if let Some(p) = selected {
                    s.clone_pattern(p);
                }
            }
            if ui.button("Rename").clicked() {
                if let Some(p) = selected {
                    begin_rename(s, p);
                }
            }
            if ui.button("Delete").on_hover_text("Delete the selected pattern and its playlist clips").clicked() {
                if let Some(p) = selected {
                    request_delete(s, p);
                }
            }
        });
    });
    ui.label(RichText::new("Double-click: edit \u{b7} drag onto a playlist lane: place \u{b7} right-click: more").small().color(MUTED));
    ui.add_space(4.0);

    // ---- delete confirmation ----
    if let Some(id) = s.pat_confirm_delete {
        match s.pat_index(id) {
            None => s.pat_confirm_delete = None,
            Some(i) => {
                let name = s.patterns[i].name.clone();
                let n = s.pattern_clip_count(id);
                let mut yes = false;
                let mut no = false;
                card().show(ui, |ui| {
                    ui.label(RichText::new(format!("Delete \"{name}\"?")).strong().color(RED));
                    ui.label(RichText::new(if n > 0 { format!("Its {n} clip(s) in the playlist are removed too.") } else { "This pattern has notes in it.".to_string() }).small().color(MUTED));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            yes = true;
                        }
                        if ui.button("Cancel").clicked() {
                            no = true;
                        }
                    });
                });
                if yes {
                    s.delete_pattern(id);
                } else if no {
                    s.pat_confirm_delete = None;
                }
                ui.add_space(4.0);
            }
        }
    }

    // ---- list ----
    let rows: Vec<Row> = s.patterns.iter().map(|p| Row { id: p.id, name: p.name.clone(), notes: p.notes.clone(), len: p.len, clips: s.pattern_clip_count(p.id) }).collect();
    if rows.is_empty() {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No patterns yet").color(MUTED));
        });
        return;
    }
    egui::ScrollArea::vertical().id_source("patterns_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        for (idx, row) in rows.iter().enumerate() {
            let id = row.id;
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
            let sel = s.selected_pattern == Some(id);
            let editing = s.editing_pattern == Some(id);
            let col = track_color(idx);
            {
                let p = ui.painter();
                let bg = if sel {
                    CARD_HI
                } else if resp.hovered() {
                    Color32::from_rgb(36, 38, 48)
                } else {
                    CARD
                };
                p.rect_filled(rect, 8.0, bg);
                if sel {
                    p.rect_stroke(rect, 8.0, Stroke::new(1.0_f32, ACCENT));
                }
                p.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(4.0, rect.height() - 12.0)), 2.0, col);
                // tiny piano-roll preview
                let pv = Rect::from_min_size(pos2(rect.right() - 92.0, rect.top() + 6.0), vec2(84.0, rect.height() - 12.0));
                p.rect_filled(pv, 4.0, Color32::from_black_alpha(70));
                if !row.notes.is_empty() {
                    let lo = row.notes.iter().map(|n| n.key).min().unwrap_or(60);
                    let hi = row.notes.iter().map(|n| n.key).max().unwrap_or(72);
                    let span = ((hi - lo) as f32).max(11.0);
                    let plen = row.len.max(1) as f32;
                    for n in &row.notes {
                        let xa = pv.left() + 2.0 + n.start as f32 / plen * (pv.width() - 4.0);
                        let xb = (pv.left() + 2.0 + (n.start + n.len).min(row.len) as f32 / plen * (pv.width() - 4.0)).max(xa + 1.5);
                        let y = pv.bottom() - 3.0 - ((n.key - lo) as f32 / span) * (pv.height() - 6.0);
                        p.rect_filled(Rect::from_min_max(pos2(xa, y - 1.5), pos2(xb, y + 1.0)), 1.0, col);
                    }
                }
            }
            if s.pat_rename == Some(id) {
                let te = Rect::from_min_size(pos2(rect.left() + 14.0, rect.top() + 4.0), vec2((rect.width() - 120.0).max(60.0), 20.0));
                let r = ui.put(te, egui::TextEdit::singleline(&mut s.pat_rename_buf));
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if r.lost_focus() {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        s.pat_rename = None;
                    } else {
                        let name = s.pat_rename_buf.clone();
                        s.rename_pattern(id, &name);
                        s.pat_rename = None;
                    }
                }
            } else {
                let mut name = row.name.clone();
                if name.chars().count() > 20 {
                    name = format!("{}\u{2026}", name.chars().take(19).collect::<String>());
                }
                ui.painter().text(pos2(rect.left() + 14.0, rect.top() + 14.0), Align2::LEFT_CENTER, name, FontId::proportional(13.0), TEXT);
            }
            let sub = format!("{} note{} \u{b7} {} \u{b7} {} clip{}", row.notes.len(), if row.notes.len() == 1 { "" } else { "s" }, bars_label(row.len), row.clips, if row.clips == 1 { "" } else { "s" });
            ui.painter().text(pos2(rect.left() + 14.0, rect.bottom() - 11.0), Align2::LEFT_CENTER, sub, FontId::proportional(10.0), if editing { ACCENT } else { MUTED });

            if resp.clicked() {
                s.selected_pattern = Some(id);
            }
            if resp.double_clicked() {
                // open it in the roll, with one of its clips as the playing track when there is one
                let clip = s.clips.iter().find(|c| matches!(&c.body, ClipBody::Pattern { pat, .. } if *pat == id)).map(|c| c.id);
                s.open_pattern(id, clip);
            }
            if resp.drag_started() {
                s.pat_drag = Some(id);
                s.selected_pattern = Some(id);
            }
            resp.context_menu(|ui| {
                if ui.button("Edit in piano roll").clicked() {
                    let clip = s.clips.iter().find(|c| matches!(&c.body, ClipBody::Pattern { pat, .. } if *pat == id)).map(|c| c.id);
                    s.open_pattern(id, clip);
                    ui.close_menu();
                }
                if ui.button("Place at playhead").on_hover_text("New clip on the selected track").clicked() {
                    let (t, at) = (s.selected_track, s.playhead);
                    s.add_pattern_clip(t, at, id);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Rename").clicked() {
                    begin_rename(s, id);
                    ui.close_menu();
                }
                if ui.button("Clone").clicked() {
                    s.clone_pattern(id);
                    ui.close_menu();
                }
                if ui.button("Delete").clicked() {
                    request_delete(s, id);
                    ui.close_menu();
                }
            });
        }
    });
}
