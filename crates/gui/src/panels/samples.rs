//! Sample library: files from "watch folders" (auto-updating) plus hand-imported ones.
use crate::dialogs;
use crate::state::*;
use crate::theme::*;
use crate::widgets::*;
use crate::libsearch;
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Id, Layout, RichText, Sense, Ui};
use std::path::PathBuf;

const ROOT_H: f32 = 30.0;
const DIR_H: f32 = 26.0;
const FILE_H: f32 = 74.0;
const INDENT: f32 = 12.0;

pub(crate) fn samples_panel(s: &mut State, ui: &mut Ui) {
    // Drop zone
    let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 10.0, if hovering { ACCENT.gamma_multiply(0.16) } else { CARD });
    dashed_rect(p, rect.shrink(1.0), if hovering { ACCENT } else { STROKE });
    p.text(rect.center() - vec2(0.0, 8.0), Align2::CENTER_CENTER, "Drop audio files here", FontId::proportional(13.5), TEXT);
    p.text(rect.center() + vec2(0.0, 10.0), Align2::CENTER_CENTER, "WAV \u{b7} MP3 \u{b7} FLAC \u{b7} OGG", FontId::proportional(11.0), MUTED);

    // ---- watch folders ----
    ui.add_space(8.0);
    small_caps(ui, "WATCH FOLDERS");
    let mut remove: Option<std::path::PathBuf> = None;
    for dir in s.cfg.watch_folders.clone() {
        let count = s.library.iter().filter(|e| e.folder.as_ref() == Some(&dir)).count();
        ui.horizontal(|ui| {
            let name = dir.file_name().and_then(|n| n.to_str()).map(|n| n.to_string()).unwrap_or_else(|| dir.display().to_string());
            ui.label(RichText::new(name).strong()).on_hover_text(dir.display().to_string());
            ui.label(RichText::new(format!("{count}")).small().color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("\u{d7}").on_hover_text("Stop watching this folder").clicked() {
                    remove = Some(dir.clone());
                }
            });
        });
    }
    if let Some(d) = remove {
        s.remove_watch_folder(&d);
    }
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 110.0).max(60.0);
        let resp = ui.add(egui::TextEdit::singleline(&mut s.folder_input).hint_text("folder path").desired_width(w));
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button("Add").clicked() || enter) && !s.folder_input.trim().is_empty() {
            let d = std::path::PathBuf::from(s.folder_input.trim().trim_matches('"'));
            s.add_watch_folder(d);
            s.folder_input.clear();
        }
        if dialogs::has_native_dialogs() && ui.button("Browse\u{2026}").clicked() {
            if let Some(d) = dialogs::pick_folder() {
                s.add_watch_folder(d);
            }
        }
    });
    if s.cfg.watch_folders.is_empty() {
        ui.label(RichText::new("New files appear here automatically.").small().color(MUTED));
    }

    // ---- library list ----
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        small_caps(ui, "LIBRARY");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("{}", s.library.len())).small().color(MUTED));
        });
    });
    ui.add(egui::TextEdit::singleline(&mut s.lib_filter).hint_text("Search name\u{2026}").desired_width(f32::INFINITY));
    ui.horizontal(|ui| {
        let w = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(60.0);
        ui.add(egui::TextEdit::singleline(&mut s.lib_key).hint_text("Key: Am, F#, C min").desired_width(w))
            .on_hover_text("Finds files with this key in their name (e.g. Pad_Am_120.wav). A bare note matches major and minor.");
        ui.add(egui::TextEdit::singleline(&mut s.lib_len).hint_text("Length: 1-3, <2, >5 s").desired_width(w))
            .on_hover_text("Length in seconds: a range (1-3), a maximum (<2), a minimum (>5) or about a value (2.5)");
    });
    ui.label(RichText::new("Click: play / stop \u{b7} Drag: onto a track lane (audio clip) or a track header (sampler instrument)").small().color(MUTED));
    ui.add_space(2.0);

    // ---- filter ----
    let text = s.lib_filter.trim().to_lowercase();
    let key_text = s.lib_key.trim().to_string();
    let key_q = if key_text.is_empty() { None } else { Some(libsearch::parse_key_query(&key_text)) }; // Some(None) = unparsable
    let len_text = s.lib_len.trim().to_string();
    let len_q = if len_text.is_empty() { None } else { Some(libsearch::parse_len_query(&len_text)) };
    let filtering = !text.is_empty() || key_q.is_some() || len_q.is_some();
    let passes = |e: &LibEntry| -> bool {
        if !text.is_empty() && !e.lname.contains(&text) {
            return false;
        }
        if let Some(q) = key_q {
            match q {
                Some(q) if libsearch::key_matches(e.key, q) => {}
                _ => return false,
            }
        }
        if let Some(q) = len_q {
            match (q, e.duration) {
                (Some((lo, hi)), Some(d)) if d >= lo && d <= hi => {}
                _ => return false,
            }
        }
        true
    };

    // ---- rows (folder tree like FL Studio's browser) ----
    let mut rows: Vec<Row> = Vec::new();
    {
        let mut cur_root: Option<Option<PathBuf>> = None;
        let mut root_open = true;
        let mut stack: Vec<String> = Vec::new();
        let mut stack_paths: Vec<PathBuf> = Vec::new();
        let mut closed_depth: Option<usize> = None;
        // contiguous per-root counts
        let mut counts: Vec<(Option<PathBuf>, usize)> = Vec::new();
        for e in s.library.iter().filter(|e| passes(e)) {
            if counts.last().map_or(false, |(r, _)| *r == e.folder) {
                if let Some(last) = counts.last_mut() {
                    last.1 += 1;
                }
            } else {
                counts.push((e.folder.clone(), 1));
            }
        }
        let mut count_i = 0usize;
        for (idx, e) in s.library.iter().enumerate() {
            if !passes(e) {
                continue;
            }
            if cur_root.as_ref() != Some(&e.folder) {
                cur_root = Some(e.folder.clone());
                let key = e.folder.clone().unwrap_or_else(|| PathBuf::from("<imported>"));
                root_open = filtering || s.lib_open.contains(&key);
                let name = match &e.folder {
                    Some(f) => f.file_name().and_then(|n| n.to_str()).map(|n| n.to_string()).unwrap_or_else(|| f.display().to_string()),
                    None => "Imported".to_string(),
                };
                let count = counts.get(count_i).map(|c| c.1).unwrap_or(0);
                count_i += 1;
                rows.push(Row::Dir { key, name, depth: 0, open: root_open, count: Some(count) });
                stack.clear();
                stack_paths.clear();
                closed_depth = None;
            }
            if !root_open {
                continue;
            }
            let mut comps: Vec<String> = Vec::new();
            if let (Some(f), Some(p)) = (&e.folder, &e.path) {
                if let Ok(rel) = p.strip_prefix(f) {
                    if let Some(parent) = rel.parent() {
                        comps = parent.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                    }
                }
            }
            let common = stack.iter().zip(comps.iter()).take_while(|(a, b)| a == b).count();
            stack.truncate(common);
            stack_paths.truncate(common);
            if matches!(closed_depth, Some(cd) if cd > common) {
                closed_depth = None;
            }
            for c in &comps[common..] {
                let base = stack_paths.last().cloned().or_else(|| e.folder.clone()).unwrap_or_default();
                let path = base.join(c);
                stack.push(c.clone());
                stack_paths.push(path.clone());
                if closed_depth.is_none() {
                    let open = filtering || s.lib_open.contains(&path);
                    rows.push(Row::Dir { key: path, name: c.clone(), depth: stack.len(), open, count: None });
                    if !open {
                        closed_depth = Some(stack.len());
                    }
                }
            }
            if closed_depth.is_none() {
                rows.push(Row::File { idx, depth: stack.len() + 1 });
            }
        }
    }

    // ---- list ----
    let heights: Vec<f32> = rows.iter().map(|r| if matches!(r, Row::Dir { depth: 0, .. }) { ROOT_H } else if matches!(r, Row::Dir { .. }) { DIR_H } else { FILE_H }).collect();
    let total: f32 = heights.iter().sum();
    let mut toggle_play: Option<u64> = None;
    let mut start_drag: Option<u64> = None;
    let mut toggle_dir: Option<PathBuf> = None;
    let mut want_wave: Vec<PathBuf> = Vec::new();
    let playing = s.preview.map(|(id, at, len)| (id, (at.elapsed().as_secs_f32() / len.max(0.01)).clamp(0.0, 1.0)));
    if s.library.is_empty() {
        ui.label(RichText::new("No samples yet. Add a watch folder or drop files here.").small().color(MUTED));
    } else if rows.is_empty() {
        ui.label(RichText::new("Nothing matches. (Length search only knows files that were already measured.)").small().color(MUTED));
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        ui.set_height(total.max(1.0));
        let origin = ui.max_rect().min;
        let w = ui.available_width();
        let mut y = 0.0f32;
        for (ri, row) in rows.iter().enumerate() {
            let h = heights[ri];
            let top = y;
            y += h;
            if top + h < viewport.min.y || top > viewport.max.y {
                continue;
            }
            let rect = egui::Rect::from_min_size(origin + vec2(0.0, top), vec2(w, h - 2.0));
            match row {
                Row::Dir { key, name, depth, open, count } => {
                    let resp = ui.interact(rect, Id::new(("libdir", key)), Sense::click());
                    let root = *depth == 0;
                    let p = ui.painter();
                    p.rect_filled(rect, 6.0, if resp.hovered() { CARD_HI } else if root { CARD } else { Color32::from_rgb(32, 34, 40) });
                    let x = rect.left() + 6.0 + *depth as f32 * INDENT;
                    // disclosure triangle
                    let c = pos2(x + 5.0, rect.center().y);
                    let tri = if *open { vec![pos2(c.x - 4.0, c.y - 2.5), pos2(c.x + 4.0, c.y - 2.5), pos2(c.x, c.y + 3.5)] } else { vec![pos2(c.x - 2.5, c.y - 4.0), pos2(c.x - 2.5, c.y + 4.0), pos2(c.x + 3.5, c.y)] };
                    p.add(egui::Shape::convex_polygon(tri, MUTED, egui::Stroke::NONE));
                    let label = match count {
                        Some(n) => format!("{name}  ({n})"),
                        None => name.clone(),
                    };
                    p.with_clip_rect(rect).text(pos2(x + 16.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(if root { 13.0 } else { 12.0 }), if root { TEXT } else { Color32::from_rgb(200, 200, 208) });
                    if resp.clicked() && !filtering {
                        toggle_dir = Some(key.clone());
                    }
                }
                Row::File { idx, depth } => {
                    let e = &s.library[*idx];
                    let resp = ui.interact(rect, Id::new(("libfile", e.id)), Sense::click_and_drag());
                    let is_playing = playing.map_or(false, |(id, _)| id == e.id);
                    let dragging = s.drag == Some(e.id);
                    let fill = if dragging { ACCENT.gamma_multiply(0.2) } else if is_playing { ACCENT.gamma_multiply(0.14) } else if resp.hovered() { CARD_HI } else { CARD };
                    let p = ui.painter();
                    p.rect_filled(rect, 7.0, fill);
                    if is_playing {
                        p.rect_stroke(rect, 7.0, egui::Stroke::new(1.0_f32, ACCENT));
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    let left = rect.left() + 6.0 + (*depth as f32 - 1.0).max(0.0) * INDENT + 8.0;
                    let cp = p.with_clip_rect(rect);
                    // header line: play icon, name, key, length
                    let icon_c = pos2(left + 5.0, rect.top() + 13.0);
                    if is_playing {
                        cp.rect_filled(egui::Rect::from_center_size(icon_c, vec2(8.0, 8.0)), 1.5, ACCENT);
                    } else {
                        cp.add(egui::Shape::convex_polygon(vec![pos2(icon_c.x - 3.5, icon_c.y - 5.0), pos2(icon_c.x - 3.5, icon_c.y + 5.0), pos2(icon_c.x + 5.0, icon_c.y)], if resp.hovered() { TEXT } else { MUTED }, egui::Stroke::NONE));
                    }
                    let mut right = rect.right() - 8.0;
                    if let Some(d) = e.duration {
                        let g = cp.text(pos2(right, rect.top() + 13.0), Align2::RIGHT_CENTER, format!("{d:.2}s"), FontId::proportional(10.5), MUTED);
                        right = g.left() - 8.0;
                    }
                    if let Some(k) = e.key {
                        let g = cp.text(pos2(right, rect.top() + 13.0), Align2::RIGHT_CENTER, libsearch::key_label(k), FontId::proportional(10.5), ACCENT);
                        right = g.left() - 8.0;
                    }
                    let name_clip = egui::Rect::from_min_max(rect.min, pos2(right.max(left + 20.0), rect.max.y));
                    p.with_clip_rect(name_clip).text(pos2(left + 16.0, rect.top() + 13.0), Align2::LEFT_CENTER, &e.name, FontId::proportional(12.5), if e.failed { RED } else { TEXT });
                    // big waveform
                    let wave = egui::Rect::from_min_max(pos2(rect.left() + 8.0, rect.top() + 28.0), pos2(rect.right() - 8.0, rect.bottom() - 6.0));
                    cp.rect_filled(wave, 4.0, Color32::from_rgb(20, 22, 28));
                    let peaks: Option<&[(f32, f32)]> = match (&e.data, &e.wave) {
                        (Some(d), _) => Some(&d.dense[..]),
                        (None, Some(w)) => Some(&w[..]),
                        _ => None,
                    };
                    match peaks {
                        Some(pk) => draw_wave_range(&cp, wave.shrink2(vec2(2.0, 2.0)), pk, 0.0, 1.0, ACCENT.gamma_multiply(0.95)),
                        None => {
                            cp.line_segment([pos2(wave.left() + 4.0, wave.center().y), pos2(wave.right() - 4.0, wave.center().y)], egui::Stroke::new(1.0_f32, Color32::from_rgb(57, 59, 73)));
                            if e.failed {
                                cp.text(wave.center(), Align2::CENTER_CENTER, "unreadable", FontId::proportional(10.5), RED);
                            }
                        }
                    }
                    if e.wave.is_none() && !e.failed {
                        if let Some(p) = &e.path {
                            want_wave.push(p.clone());
                        }
                    }
                    if let Some((id, frac)) = playing {
                        if id == e.id {
                            let x = wave.left() + frac * wave.width();
                            cp.line_segment([pos2(x, wave.top()), pos2(x, wave.bottom())], egui::Stroke::new(1.5_f32, Color32::WHITE));
                        }
                    }
                    if resp.clicked() {
                        toggle_play = Some(e.id);
                    }
                    if resp.drag_started() {
                        start_drag = Some(e.id);
                    }
                }
            }
        }
    });
    for p in want_wave.into_iter().take(16) {
        s.request_wave(p);
    }
    if let Some(d) = toggle_dir {
        if !s.lib_open.remove(&d) {
            s.lib_open.insert(d);
        }
    }
    if let Some(id) = toggle_play {
        s.toggle_preview(id);
    }
    if let Some(id) = start_drag {
        s.drag = Some(id);
    }
}

enum Row {
    /// depth 0 = watch folder (or "Imported"), deeper = sub-folder.
    Dir { key: PathBuf, name: String, depth: usize, open: bool, count: Option<usize> },
    File { idx: usize, depth: usize },
}
