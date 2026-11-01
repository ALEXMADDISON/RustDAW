//! Playlist / arrangement: 100 tracks, audio + MIDI clips.
//!   drag a sample from the Samples panel (or drop a file from the OS) onto a lane = audio clip, onto a header = sampler instrument
//!   drag a clip to move it (across tracks too), drag its right edge to resize
//!   Shift + drag a clip = drag a copy (the original stays where it is)
//!   double-click an empty lane -> new clip of the selected pattern;  double-click a pattern clip -> piano roll
//!   double-click an audio clip -> sample menu (waves / pitch / BPM);  right-click a clip -> menu
//!   audio clip header: drag the pitch / bpm chips up and down (Shift = fine, double-click = reset);
//!   Alt + drag the right edge of an audio clip = time-stretch it (changes its BPM)
//!   R on a track header arms it for recording (Record button in the transport)
use crate::state::*;
use crate::theme::*;
use crate::widgets::*;
use crate::panels::automation::{automation_context, automation_target_menu};
use core_engine::{eval_points, AutoTarget, Command, PPQ};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Id, RichText, Sense, Shape, Stroke, Ui};

const HEAD_W: f32 = 212.0;
const RULER_H: f32 = 26.0;
const SONG_BARS: u32 = 128;
const SB_H: f32 = 14.0; // horizontal scrollbar height

enum ClipAction {
    Delete(u64),
    Duplicate(u64),
    Split,
    Edit(u64),
    MakeUnique(u64),
    Settings(u64),
    Copy,
    Cut,
    Paste,
}

/// Height of the automation sub-lane under every track (the track's own lane keeps `pl_row_h`).
pub(crate) fn auto_lane_h(row_h: f32) -> f32 {
    (row_h * 0.8).max(44.0)
}

/// Lays out the track rows for this frame. Under its own lane every track gets one automation layer
/// per automated parameter (oldest first); a track without automation has no layer at all (the "+" in its header adds one).
/// Fills `pl_auto_lanes` and `pl_row_y` (top of every row, relative to the first row; one extra entry = total height).
fn compute_layout(s: &mut State, row_h: f32) {
    let auto_h = auto_lane_h(row_h);
    let mut found: Vec<Vec<(AutoTarget, u64)>> = vec![Vec::new(); TRACKS];
    for c in &s.clips {
        if let ClipBody::Automation { auto } = &c.body {
            let l = &mut found[c.track.min(TRACKS - 1)];
            match l.iter_mut().find(|e| e.0 == auto.target) {
                Some(e) => e.1 = e.1.min(c.id),
                None => l.push((auto.target, c.id)),
            }
        }
    }
    s.pl_auto_lanes = found
        .into_iter()
        .map(|mut l| {
            l.sort_by_key(|e| e.1);
            l.into_iter().map(|e| e.0).collect()
        })
        .collect();
    s.pl_row_y.clear();
    let mut y = 0.0_f32;
    for t in 0..TRACKS {
        s.pl_row_y.push(y);
        y += row_h + auto_h * s.pl_auto_lanes[t].len() as f32;
    }
    s.pl_row_y.push(y);
}

/// Index of the track row under `y` (relative to the first row); may be out of range.
fn row_at(s: &State, y: f32) -> i32 {
    s.pl_row_y.partition_point(|&o| o <= y) as i32 - 1
}

/// Vertical extent (relative to the first row) of a clip's lane.
fn clip_y_range(s: &State, c: &UiClip) -> (f32, f32) {
    let t = c.track.min(TRACKS - 1);
    let top = s.pl_row_y.get(t).copied().unwrap_or(0.0);
    let (row_h, auto_h) = (s.pl_row_h, auto_lane_h(s.pl_row_h));
    match &c.body {
        ClipBody::Automation { auto } => {
            let k = s.pl_auto_lanes.get(t).and_then(|l| l.iter().position(|x| *x == auto.target)).unwrap_or(0);
            let a = top + row_h + k as f32 * auto_h;
            (a, a + auto_h)
        }
        _ => (top, top + row_h),
    }
}

/// Colour of automation clips (they sit on any track, so they do not take the track's colour).
const AUTO_COL: Color32 = Color32::from_rgb(255, 160, 64);

pub(crate) fn playlist(s: &mut State, ui: &mut Ui) {
    // ---- toolbar ----
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(!s.undo.is_empty(), egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
            s.undo();
        }
        if ui.add_enabled(!s.redo.is_empty(), egui::Button::new("Redo")).on_hover_text("Ctrl+Y / Ctrl+Shift+Z").clicked() {
            s.redo();
        }
        ui.separator();
        ui.checkbox(&mut s.pl_snap_on, "Snap").on_hover_text("Snap clips to the grid. Switch off for free positioning.");
        let snap_on = s.pl_snap_on;
        ui.add_enabled_ui(snap_on, |ui| {
            let cur = SNAPS.iter().find(|(_, v)| *v == s.pl_snap).map(|(n, _)| *n).unwrap_or("Beat");
            egui::ComboBox::from_id_source("pl_snap").width(64.0).selected_text(cur).show_ui(ui, |ui| {
                for (n, v) in SNAPS {
                    ui.selectable_value(&mut s.pl_snap, v, n);
                }
            });
        });
        ui.label(RichText::new("Zoom").color(MUTED));
        ui.spacing_mut().slider_width = 70.0;
        ui.add(egui::Slider::new(&mut s.pl_ppb, 8.0..=160.0).show_value(false)).on_hover_text("Horizontal zoom (Ctrl + wheel)");
        ui.add(egui::Slider::new(&mut s.pl_row_h, 34.0..=110.0).show_value(false)).on_hover_text("Vertical zoom (track height)");
        if ui.button("Split").on_hover_text("Split clips at the playhead").clicked() {
            s.split_at_playhead();
        }
        if ui.button("Duplicate").clicked() {
            s.duplicate_selected();
        }
        if ui.button("Delete").clicked() {
            s.delete_selected_clips();
        }
        if ui.button("+ Pattern clip").on_hover_text("Place the selected pattern on the selected track at the playhead").clicked() {
            let (t, at) = (s.selected_track, s.playhead);
            let pat = s.current_pattern();
            if let Some(id) = s.add_pattern_clip(t, at, pat) {
                s.open_clip(id);
            }
        }
        ui.menu_button("+ Automation", |ui| {
            let t = s.selected_track;
            automation_target_menu(ui, s, t);
        })
        .response
        .on_hover_text("Create an automation clip for a parameter of the selected track");
        if ui.button("Loop clip").on_hover_text("Set the cycle range to the selected clip").clicked() {
            if let Some(i) = s.sel_clips.first().and_then(|&id| s.clip_index(id)) {
                let st = s.clips[i].start;
                let en = st + s.clips[i].len_ticks(s.bpm, s.sample_rate);
                s.cycle = (st, en);
                s.cycle_on = true;
                s.apply_cycle();
            }
        }
        ui.separator();
        ui.label(RichText::new("Rec latency").color(MUTED)).on_hover_text("Trim this much from the start of every take to compensate for audio latency");
        if ui.add(egui::DragValue::new(&mut s.cfg.rec_offset_ms).clamp_range(0.0..=500.0).suffix(" ms")).changed() {
            s.cfg.save();
        }
    });
    ui.add_space(2.0);

    let row_h: f32 = s.pl_row_h;

    // ---- geometry ----
    let panel = ui.available_rect_before_wrap();
    let lane_left = panel.left() + HEAD_W;
    let lane_right = panel.right() - 14.0;
    let ppt = s.pl_ppb / PPQ as f32;
    let total_w = SONG_BARS as f32 * BAR_TICKS as f32 * ppt;
    let max_scroll = (total_w - (lane_right - lane_left)).max(0.0);

    if ui.rect_contains_pointer(panel) {
        let (sd, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        if sd.x != 0.0 {
            s.pl_scroll_x -= sd.x;
        }
        if (zoom - 1.0).abs() > 0.001 {
            s.pl_ppb = (s.pl_ppb * zoom).clamp(8.0, 160.0);
        }
    }
    // Horizontal scrollbar strip along the bottom of the lanes (painted at the end of the frame).
    let sb_track = egui::Rect::from_min_max(pos2(lane_left, panel.bottom() - SB_H), pos2(lane_right, panel.bottom()));
    let view_w = lane_right - lane_left;
    let thumb_w = if total_w > 0.0 { (view_w * view_w / total_w).clamp(36.0, view_w.max(36.0)) } else { view_w };
    let sb_resp = ui.interact(sb_track, Id::new("pl_hscroll"), Sense::click_and_drag());
    if (sb_resp.dragged() || sb_resp.clicked()) && max_scroll > 0.0 {
        if let Some(p) = sb_resp.interact_pointer_pos() {
            let frac = ((p.x - lane_left - thumb_w * 0.5) / (view_w - thumb_w).max(1.0)).clamp(0.0, 1.0);
            s.pl_scroll_x = frac * max_scroll;
        }
    }
    if s.playing && !sb_resp.dragged() {
        let x = s.playhead as f32 * ppt - s.pl_scroll_x;
        if x > (lane_right - lane_left) - 20.0 {
            s.pl_scroll_x = (s.playhead as f32 * ppt - 40.0).max(0.0);
        } else if x < 0.0 {
            s.pl_scroll_x = (s.playhead as f32 * ppt - 40.0).max(0.0);
        }
    }
    s.pl_scroll_x = s.pl_scroll_x.clamp(0.0, max_scroll);
    s.pl_origin_x = lane_left - s.pl_scroll_x;
    s.pl_ppt = ppt;
    let origin = s.pl_origin_x;

    // ---- ruler ----
    let (ruler, _) = ui.allocate_exact_size(vec2(panel.width(), RULER_H), Sense::hover());
    let lane_ruler = egui::Rect::from_min_max(pos2(lane_left, ruler.top()), pos2(lane_right, ruler.bottom()));
    {
        let p = ui.painter();
        p.rect_filled(egui::Rect::from_min_max(ruler.min, pos2(lane_left, ruler.bottom())), 0.0, CARD);
        p.text(pos2(ruler.left() + 14.0, ruler.center().y), Align2::LEFT_CENTER, format!("{} TRACKS", TRACKS), FontId::proportional(10.5), MUTED);
        let lp = p.with_clip_rect(lane_ruler);
        lp.rect_filled(lane_ruler, 0.0, Color32::from_rgb(36, 38, 44));
        let bar_px = BAR_TICKS as f32 * ppt;
        let first = (s.pl_scroll_x / bar_px) as u32;
        for bar in first..SONG_BARS {
            let x = origin + bar as f32 * bar_px;
            if x > lane_right {
                break;
            }
            lp.line_segment([pos2(x, ruler.top() + 6.0), pos2(x, ruler.bottom())], Stroke::new(1.0_f32, Color32::from_rgb(110, 110, 120)));
            lp.text(pos2(x + 5.0, ruler.top() + 9.0), Align2::LEFT_CENTER, format!("{}", bar + 1), FontId::proportional(11.0), TEXT);
            if s.pl_ppb >= 20.0 {
                for b in 1..4 {
                    let bx = x + b as f32 * PPQ as f32 * ppt;
                    lp.line_segment([pos2(bx, ruler.bottom() - 6.0), pos2(bx, ruler.bottom())], Stroke::new(1.0_f32, Color32::from_rgb(80, 80, 90)));
                }
            }
        }
    }
    if s.cycle.1 > s.cycle.0 {
        let (x0, x1) = (origin + s.cycle.0 as f32 * ppt, origin + s.cycle.1 as f32 * ppt);
        let r = egui::Rect::from_min_max(pos2(x0.max(lane_left), ruler.top() + 1.0), pos2(x1.min(lane_right), ruler.bottom() - 1.0));
        if r.width() > 0.0 {
            ui.painter().rect_filled(r, 2.0, YELLOW.gamma_multiply(if s.cycle_on { 0.35 } else { 0.12 }));
        }
    }
    let rresp = ui.interact(lane_ruler, Id::new("pl_ruler"), Sense::click_and_drag());
    let shift = ui.input(|i| i.modifiers.shift);
    if shift && rresp.dragged() {
        if let (Some(o), Some(p)) = (ui.input(|i| i.pointer.press_origin()), rresp.interact_pointer_pos()) {
            let a = s.snap_round((o.x - origin) / ppt, s.eff_snap());
            let b = s.snap_round((p.x - origin) / ppt, s.eff_snap());
            s.cycle = (a.min(b), a.max(b));
            s.cycle_on = s.cycle.1 > s.cycle.0;
            s.apply_cycle();
        }
    } else if rresp.clicked() || rresp.dragged() {
        if let Some(p) = rresp.interact_pointer_pos() {
            let t = s.snap_round((p.x - origin) / ppt, PPQ / 4);
            s.seek(t);
            s.focus_playlist = true;
        }
    }

    // ---- rows ----
    let mut actions: Vec<ClipAction> = Vec::new();
    let auto_h = auto_lane_h(row_h);
    compute_layout(s, row_h);
    let total_h = s.pl_row_y.last().copied().unwrap_or(0.0);
    let area = egui::ScrollArea::vertical().id_source("pl_rows").max_height((ui.available_height() - SB_H - 2.0).max(60.0)).auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (content, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
        s.pl_row0_y = content.top();
        for t in 0..TRACKS {
            let (y0, y1) = (s.pl_row_y[t], s.pl_row_y[t + 1]);
            if y1 < viewport.min.y || y0 > viewport.max.y {
                continue;
            }
            let top = content.top() + y0;
            let head = egui::Rect::from_min_size(pos2(content.left(), top), vec2(HEAD_W, row_h));
            let lane = egui::Rect::from_min_max(pos2(lane_left, top), pos2(lane_right, top + row_h));
            draw_header(ui, s, t, head);
            draw_lane(ui, s, t, lane, 0, None, &mut actions);
            // one automation layer per automated parameter; none while the track has no automation
            let targets: Vec<Option<AutoTarget>> = s.pl_auto_lanes[t].iter().map(|&x| Some(x)).collect();
            for (k, tg) in targets.into_iter().enumerate() {
                let ly = top + row_h + k as f32 * auto_h;
                let ahead = egui::Rect::from_min_size(pos2(content.left(), ly), vec2(HEAD_W, auto_h));
                let alane = egui::Rect::from_min_max(pos2(lane_left, ly), pos2(lane_right, ly + auto_h));
                draw_auto_header(ui, s, t, tg, ahead);
                draw_lane(ui, s, t, alane, k + 1, tg, &mut actions);
            }
        }
    });

    // ---- rubber-band selection ----
    if let Some(m) = s.marquee.take() {
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let down = ui.input(|i| i.pointer.primary_down());
        if let (Some(p), true) = (pointer, down) {
            let tick1 = (p.x - origin) / ppt;
            let row1 = p.y - s.pl_row0_y;
            let (t_lo, t_hi) = (m.tick0.min(tick1), m.tick0.max(tick1));
            let (r_lo, r_hi) = (m.row0.min(row1), m.row0.max(row1));
            let mut sel: Vec<u64> = m.base.clone();
            for c in &s.clips {
                let c0 = c.start as f32;
                let c1 = (c.start + c.len_ticks(s.bpm, s.sample_rate)) as f32;
                // `row0` / `row1` are pixel offsets from the first row
                let (ra, rb) = clip_y_range(s, c);
                if c1 >= t_lo && c0 <= t_hi && rb > r_lo && ra < r_hi && !sel.contains(&c.id) {
                    sel.push(c.id);
                }
            }
            s.sel_clips = sel;
            // draw the rectangle over the lanes
            let view = egui::Rect::from_min_max(pos2(lane_left, area.inner_rect.top()), pos2(lane_right, area.inner_rect.bottom()));
            let a = pos2(origin + m.tick0 * ppt, s.pl_row0_y + m.row0);
            let rect = egui::Rect::from_two_pos(a, p);
            let painter = ui.painter_at(view);
            painter.rect_filled(rect, 2.0, ACCENT.gamma_multiply(0.18));
            painter.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, ACCENT));
            s.marquee = Some(m);
        }
    }

    // Deferred clip move (avoids a clip being interacted with in two rows in one frame).
    if let Some(p) = s.pl_drag_ptr.take() {
        if let Some(ClipDrag::Move { anchor_tick, anchor_row, orig }) = s.clip_drag.take() {
            let tick = ((p.x - origin) / ppt) as i64;
            let dt = ((tick - anchor_tick) as f32 / s.eff_snap() as f32).round() as i64 * s.eff_snap() as i64;
            let row = row_at(s, p.y - s.pl_row0_y);
            for &(id, st, tr) in &orig {
                if let Some(i) = s.clip_index(id) {
                    s.clips[i].start = (st as i64 + dt).max(0) as u32;
                    s.clips[i].track = (tr as i32 + (row - anchor_row)).clamp(0, TRACKS as i32 - 1) as usize;
                }
            }
            s.clip_drag = Some(ClipDrag::Move { anchor_tick, anchor_row, orig });
        }
    }

    for a in actions {
        match a {
            ClipAction::Delete(id) => s.delete_clip(id),
            ClipAction::Duplicate(id) => s.duplicate_clip(id),
            ClipAction::Split => s.split_at_playhead(),
            ClipAction::Edit(id) => s.open_clip(id),
            ClipAction::MakeUnique(id) => s.make_clip_pattern_unique(id),
            ClipAction::Settings(id) => s.open_editor(id),
            ClipAction::Copy => {
                s.copy_selected();
            }
            ClipAction::Cut => s.cut_selected(),
            ClipAction::Paste => s.paste_at_playhead(),
        }
    }

    // ---- horizontal scrollbar ----
    {
        let p = ui.painter();
        p.rect_filled(egui::Rect::from_min_max(pos2(panel.left(), sb_track.top()), pos2(lane_left, sb_track.bottom())), 0.0, CARD);
        p.rect_filled(sb_track, 0.0, Color32::from_rgb(22, 24, 30));
        if max_scroll > 0.0 {
            let x0 = lane_left + (s.pl_scroll_x / max_scroll) * (view_w - thumb_w);
            let thumb = egui::Rect::from_min_max(pos2(x0, sb_track.top() + 2.0), pos2(x0 + thumb_w, sb_track.bottom() - 2.0));
            let col = if sb_resp.dragged() { ACCENT } else if sb_resp.hovered() { Color32::from_rgb(120, 120, 132) } else { Color32::from_rgb(88, 88, 98) };
            p.rect_filled(thumb, 5.0, col);
        } else {
            p.rect_filled(sb_track.shrink2(vec2(0.0, 2.0)), 5.0, Color32::from_rgb(43, 45, 51));
        }
    }

    // ---- playhead over ruler + rows ----
    let x = origin + s.playhead as f32 * ppt;
    if x >= lane_left && x <= lane_right {
        let p = ui.painter_at(egui::Rect::from_min_max(pos2(lane_left, ruler.top()), pos2(lane_right, area.inner_rect.bottom())));
        p.line_segment([pos2(x, ruler.top()), pos2(x, area.inner_rect.bottom())], Stroke::new(1.5_f32, Color32::WHITE));
        p.add(Shape::convex_polygon(vec![pos2(x - 5.0, ruler.top()), pos2(x + 5.0, ruler.top()), pos2(x, ruler.top() + 8.0)], Color32::WHITE, Stroke::NONE));
    }
}

fn draw_header(ui: &mut Ui, s: &mut State, t: usize, rect: egui::Rect) {
    s.pl_heads.push((t, rect));
    let sel = s.selected_track == t;
    let col = track_color(t);
    let drop_hot = s.drag.is_some() && ui.ctx().pointer_latest_pos().map_or(false, |p| rect.contains(p));
    {
        let p = ui.painter();
        p.rect_filled(rect, 0.0, if sel { Color32::from_rgb(62, 64, 74) } else { CARD });
        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0_f32, BG));
        p.rect_filled(egui::Rect::from_min_size(rect.min, vec2(5.0, rect.height())), 0.0, col);
        p.circle_filled(pos2(rect.left() + 20.0, rect.top() + 14.0), 5.0, col);
        let mut name = s.tracks[t].name.clone();
        if name.chars().count() > 12 {
            name = format!("{}\u{2026}", name.chars().take(11).collect::<String>());
        }
        p.text(pos2(rect.left() + 32.0, rect.top() + 14.0), Align2::LEFT_CENTER, name, FontId::proportional(12.5), TEXT);
        let mut inst = s.tracks[t].inst.label();
        if inst.chars().count() > 16 {
            inst = format!("{}\u{2026}", inst.chars().take(15).collect::<String>());
        }
        p.text(pos2(rect.right() - 34.0, rect.top() + 14.0), Align2::RIGHT_CENTER, inst, FontId::proportional(10.0), MUTED);
        if drop_hot {
            p.rect_stroke(rect.shrink(1.0), 4.0, Stroke::new(2.0_f32, ACCENT));
        }
    }
    let name_area = egui::Rect::from_min_size(rect.min + vec2(6.0, 0.0), vec2(rect.width() - 38.0, 26.0));
    let name_resp = ui.interact(name_area, Id::new(("th", t)), Sense::click());
    if name_resp.clicked() {
        s.selected_track = t;
        s.focus_playlist = true;
    }
    name_resp.context_menu(|ui| {
        s.selected_track = t;
        automation_target_menu(ui, s, t);
    });
    // "+" shortcut: add an automation clip for a parameter of this track
    let plus = egui::Rect::from_min_size(pos2(rect.right() - 30.0, rect.top() + 4.0), vec2(22.0, 20.0));
    ui.allocate_ui_at_rect(plus, |ui| {
        ui.spacing_mut().button_padding = vec2(5.0, 1.0);
        ui.menu_button(RichText::new("+").strong().color(AUTO_COL), |ui| {
            s.selected_track = t;
            automation_target_menu(ui, s, t);
        })
        .response
        .on_hover_text("Add automation for this track");
    });
    let y = rect.top() + 29.0;
    let btn = |i: usize| egui::Rect::from_min_size(pos2(rect.left() + 14.0 + i as f32 * 26.0, y), vec2(22.0, 18.0));
    if mini_btn(ui, btn(0), Id::new(("m", t)), "M", s.tracks[t].mute, ACCENT) {
        s.tracks[t].mute = !s.tracks[t].mute;
        s.handle.send(Command::SetTrackMute { track: t, mute: s.tracks[t].mute });
    }
    if mini_btn(ui, btn(1), Id::new(("s", t)), "S", s.tracks[t].solo, YELLOW) {
        s.tracks[t].solo = !s.tracks[t].solo;
        s.handle.send(Command::SetTrackSolo { track: t, solo: s.tracks[t].solo });
    }
    if mini_btn(ui, btn(2), Id::new(("r", t)), "R", s.tracks[t].arm, RED) {
        s.tracks[t].arm = !s.tracks[t].arm;
        s.selected_track = t;
    }
    let sl = egui::Rect::from_min_size(pos2(rect.left() + 98.0, y), vec2(rect.width() - 108.0, 18.0));
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = sl.width() - 6.0;
        let gain_resp = ui.put(sl, egui::Slider::new(&mut s.tracks[t].gain, 0.0..=1.5).show_value(false));
        if gain_resp.changed() {
            s.handle.send(Command::SetTrackGain { track: t, gain: s.tracks[t].gain });
        }
        gain_resp.context_menu(|ui| {
            s.selected_track = t;
            automation_context(ui, s, AutoTarget::TrackGain(t as u16));
        });
    });
}

/// Left cell of an automation layer (under the track's own header): names the parameter it holds.
fn draw_auto_header(ui: &mut Ui, s: &mut State, t: usize, target: Option<AutoTarget>, rect: egui::Rect) {
    let sel = s.selected_track == t;
    let label = match target {
        Some(tg) => {
            let n = s.target_name(tg);
            if n.chars().count() > 28 {
                format!("{}\u{2026}", n.chars().take(27).collect::<String>())
            } else {
                n
            }
        }
        None => "Automation".to_string(),
    };
    {
        let p = ui.painter();
        p.rect_filled(rect, 0.0, if sel { Color32::from_rgb(52, 48, 46) } else { Color32::from_rgb(34, 32, 32) });
        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0_f32, BG));
        p.rect_filled(egui::Rect::from_min_size(rect.min, vec2(5.0, rect.height())), 0.0, AUTO_COL.gamma_multiply(0.7));
        p.text(pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(11.0), AUTO_COL.gamma_multiply(if target.is_some() { 1.0 } else { 0.7 }));
    }
    let resp = ui.interact(rect, Id::new(("ath", t, rect.top() as i32)), Sense::click());
    if resp.clicked() {
        s.selected_track = t;
        s.focus_playlist = true;
    }
    resp.context_menu(|ui| {
        s.selected_track = t;
        automation_target_menu(ui, s, t);
    });
}

fn draw_lane(ui: &mut Ui, s: &mut State, t: usize, lane: egui::Rect, lane_k: usize, target: Option<AutoTarget>, actions: &mut Vec<ClipAction>) {
    let auto_lane = lane_k > 0;
    let ppt = s.pl_ppt;
    let origin = s.pl_origin_x;
    let (bpm, sr) = (s.bpm, s.sample_rate);
    let painter = ui.painter_at(lane);
    let bg = if auto_lane {
        if t % 2 == 0 { Color32::from_rgb(34, 29, 26) } else { Color32::from_rgb(38, 33, 29) }
    } else if t % 2 == 0 {
        Color32::from_rgb(26, 28, 34)
    } else {
        Color32::from_rgb(30, 32, 38)
    };
    painter.rect_filled(lane, 0.0, bg);
    if s.selected_track == t {
        painter.rect_filled(lane, 0.0, ACCENT.gamma_multiply(0.06));
    }
    // grid
    let beat_px = PPQ as f32 * ppt;
    let first_beat = (s.pl_scroll_x / beat_px) as u32;
    let mut b = first_beat;
    loop {
        let x = origin + b as f32 * beat_px;
        if x > lane.right() {
            break;
        }
        let bar = b % 4 == 0;
        if bar || s.pl_ppb >= 24.0 {
            painter.line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], Stroke::new(1.0_f32, if bar { Color32::from_rgb(51, 53, 59) } else { Color32::from_rgb(36, 38, 44) }));
        }
        b += 1;
    }
    painter.line_segment([lane.left_bottom(), lane.right_bottom()], Stroke::new(1.0_f32, Color32::from_rgb(17, 19, 25)));

    let lane_resp = ui.interact(lane, Id::new(("lane", t, lane_k)), Sense::click_and_drag());
    // Dragging on empty space draws a selection rectangle (Shift / Ctrl adds to the selection).
    if lane_resp.drag_started() {
        if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
            let additive = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl || i.modifiers.command);
            let base = if additive { s.sel_clips.clone() } else { Vec::new() };
            if !additive {
                s.sel_clips.clear();
            }
            s.marquee = Some(Marquee { tick0: (o.x - origin) / ppt, row0: o.y - s.pl_row0_y, base });
            s.selected_track = t;
            s.focus_playlist = true;
        }
    }
    if auto_lane {
        // right-click on an empty spot of the automation lane: create a clip for a parameter of this track
        lane_resp.context_menu(|ui| {
            s.selected_track = t;
            automation_target_menu(ui, s, t);
        });
    }
    if lane_resp.double_clicked() {
        if auto_lane {
            // automation clips are made from the menu; nothing to place here
        } else if let Some(p) = lane_resp.interact_pointer_pos() {
            let tick = s.snap_round((p.x - origin) / ppt, s.eff_snap());
            let pat = s.current_pattern();
            if let Some(id) = s.add_pattern_clip(t, tick, pat) {
                s.open_clip(id);
            }
        }
    } else if lane_resp.clicked() {
        s.selected_track = t;
        s.sel_clips.clear();
        s.focus_playlist = true;
    }
    if !auto_lane {
        s.pl_lanes.push((t, lane));
    }

    // drop target highlight + insertion marker
    if !auto_lane && (s.drag.is_some() || s.pat_drag.is_some()) {
        if let Some(p) = ui.ctx().pointer_latest_pos() {
            if lane.contains(p) {
                painter.rect_filled(lane, 0.0, ACCENT.gamma_multiply(0.12));
                let x = origin + s.snap_round((p.x - origin) / ppt, s.eff_snap()) as f32 * ppt;
                painter.line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], Stroke::new(2.0_f32, ACCENT));
            }
        }
    }

    // live recording region
    if !auto_lane && s.recording && s.rec_tracks.contains(&t) {
        let x0 = origin + s.rec_start as f32 * ppt;
        let x1 = origin + s.playhead.max(s.rec_start) as f32 * ppt;
        let r = egui::Rect::from_min_max(pos2(x0, lane.top() + 3.0), pos2(x1.max(x0 + 2.0), lane.bottom() - 3.0));
        painter.rect_filled(r, 5.0, RED.gamma_multiply(0.55));
        painter.text(r.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, "Recording\u{2026}", FontId::proportional(11.5), Color32::WHITE);
    }

    // clips on this track
    let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
    let idxs: Vec<usize> = (0..s.clips.len()).filter(|&i| s.clips[i].track == t && matches!(s.clips[i].body, ClipBody::Automation { .. }) == auto_lane
        && (!auto_lane || matches!(&s.clips[i].body, ClipBody::Automation { auto } if Some(auto.target) == target))).collect();
    for ci in idxs {
        let (id, start, mut len, name) = {
            let c = &s.clips[ci];
            (c.id, c.start, c.len_ticks(bpm, sr), c.name.clone())
        };
        // While a time-stretch is still rendering in the background, show the length the clip is
        // about to get (the waveform is scaled to it), so the clip follows the mouse immediately.
        if let Some(&want) = s.stretch_preview.get(&id) {
            if matches!(&s.clips[ci].body, ClipBody::Audio { pending: true, .. }) {
                len = want;
            } else {
                s.stretch_preview.remove(&id);
            }
        }
        let x0 = origin + start as f32 * ppt;
        let x1 = x0 + len as f32 * ppt;
        let r = egui::Rect::from_min_max(pos2(x0, lane.top() + 3.0), pos2(x1.max(x0 + 4.0), lane.bottom() - 3.0));
        if !r.intersects(lane) {
            continue;
        }
        let selected = s.sel_clips.contains(&id);
        let is_auto = matches!(s.clips[ci].body, ClipBody::Automation { .. });
        let col = if is_auto { AUTO_COL } else { track_color(t) };
        let cp = painter.with_clip_rect(r.intersect(lane));
        let hh = if is_auto { 13.0_f32 } else { 16.0 };
        let header = egui::Rect::from_min_size(r.min, vec2(r.width(), hh));
        let body = egui::Rect::from_min_max(pos2(r.left(), r.top() + hh), r.max);
        // automation clips are see-through (the grid shines through), clips with sound are solid
        let (body_a, head_a) = if is_auto { (if selected { 0.34 } else { 0.18 }, if selected { 0.6 } else { 0.4 }) } else { (if selected { 0.75 } else { 0.45 }, if selected { 1.0 } else { 0.78 }) };
        cp.rect_filled(r, 7.0, col.gamma_multiply(body_a));
        cp.rect_filled(header, 7.0, col.gamma_multiply(head_a));
        let audio_fx: Option<(AudioFx, bool)> = s.clip_fx(id);
        let label = match audio_fx {
            Some((_, true)) => format!("{name}  \u{2026}"),
            _ => name.clone(),
        };
        cp.text(header.left_center() + vec2(7.0, 0.0), Align2::LEFT_CENTER, &label, FontId::proportional(if is_auto { 10.0 } else { 11.0 }), if is_auto { Color32::from_white_alpha(235) } else { Color32::from_black_alpha(220) });
        match &s.clips[ci].body {
            ClipBody::Audio { sample, peaks, offset, frames, fx, .. } => {
                if fx.show_wave {
                    let total = sample.len().max(1) as f32;
                    draw_wave_range(&cp, body.shrink2(vec2(0.0, 2.0)), peaks, *offset as f32 / total, (*offset + *frames) as f32 / total, Color32::from_white_alpha(if selected { 230 } else { 170 }));
                }
            }
            ClipBody::Automation { auto } => {
                // the curve, drawn inside the clip
                let (top, bot) = (body.top() + 4.0, body.bottom() - 4.0);
                let y_of = |v: f32| bot - v.clamp(0.0, 1.0) * (bot - top);
                let steps = ((r.width() / 3.0).ceil() as usize).clamp(2, 500);
                let mut line: Vec<egui::Pos2> = Vec::with_capacity(steps + 1);
                for k in 0..=steps {
                    let tick = auto.len as f32 * k as f32 / steps as f32;
                    line.push(pos2(x0 + tick * ppt, y_of(eval_points(&auto.points, tick))));
                }
                cp.add(Shape::line(line, Stroke::new(1.8_f32, Color32::from_rgb(255, 205, 140))));
                if r.width() > 40.0 {
                    for p in &auto.points {
                        cp.circle_filled(pos2(x0 + p.tick as f32 * ppt, y_of(p.value)), 2.8, Color32::WHITE);
                    }
                }
            }
            ClipBody::Pattern { pat, .. } => match s.patterns.iter().find(|p| p.id == *pat) {
                Some(p) if !p.notes.is_empty() => {
                    let lo = p.notes.iter().map(|n| n.key).min().unwrap_or(60);
                    let hi = p.notes.iter().map(|n| n.key).max().unwrap_or(72);
                    let span = ((hi - lo) as f32).max(11.0);
                    let bh = body.height() - 8.0;
                    let plen = p.len.max(1);
                    let mut base: u32 = 0;
                    while base < len {
                        for n in &p.notes {
                            let st = base + n.start;
                            if st >= len {
                                continue;
                            }
                            let xa = x0 + st as f32 * ppt;
                            let end = (st + n.len).min(len).min(base + plen);
                            let xb = (x0 + end as f32 * ppt).max(xa + 1.5);
                            let y = body.bottom() - 4.0 - ((n.key - lo) as f32 / span) * bh;
                            cp.rect_filled(egui::Rect::from_min_max(pos2(xa, y - 2.0), pos2(xb, y + 1.0)), 1.0, Color32::from_white_alpha(220));
                        }
                        base += plen;
                        if base < len {
                            let bx = x0 + base as f32 * ppt;
                            cp.line_segment([pos2(bx, body.top()), pos2(bx, body.bottom())], Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
                        }
                    }
                }
                _ => {
                    cp.text(body.center(), Align2::CENTER_CENTER, "empty", FontId::proportional(10.0), Color32::from_white_alpha(120));
                }
            },
        }
        // parts of an automation clip that a later clip of the same parameter takes over
        if let ClipBody::Automation { auto } = &s.clips[ci].body {
            for (j, o) in s.clips.iter().enumerate() {
                if let ClipBody::Automation { auto: oa } = &o.body {
                    if j != ci && oa.target == auto.target && (o.start > start || (o.start == start && j > ci)) {
                        let (a, b) = (o.start.max(start), (o.start + oa.len).min(start + len));
                        if b > a {
                            let rr = egui::Rect::from_min_max(pos2(origin + a as f32 * ppt, body.top()), pos2(origin + b as f32 * ppt, r.bottom()));
                            cp.rect_filled(rr, 0.0, Color32::from_black_alpha(140));
                            cp.text(rr.center_top() + vec2(0.0, 3.0), Align2::CENTER_TOP, "overridden", FontId::proportional(9.0), Color32::from_white_alpha(170));
                        }
                    }
                }
            }
        }
        if audio_fx.is_some() {
            let (trim_l, trim_r) = match &s.clips[ci].body {
                ClipBody::Audio { sample, offset, frames, .. } => (*offset > 0, (*offset + *frames) < sample.len() as u32),
                _ => (false, false),
            };
            let hot = selected || ui.rect_contains_pointer(r);
            // Faint ghost of the audio that is trimmed away, just outside the clip's edges. Only while
            // a trim handle of this clip is being dragged: otherwise it looks like the clip has a
            // "tail" (e.g. the other half of a split sample shining through).
            let trimming = selected && matches!(s.clip_drag, Some(ClipDrag::TrimStart { .. }) | Some(ClipDrag::Resize));
            if trimming {
                if let ClipBody::Audio { sample, peaks, offset, frames, .. } = &s.clips[ci].body {
                    let total = sample.len().max(1) as f32;
                    let gp = painter.with_clip_rect(lane);
                    let ghost = Color32::from_white_alpha(70);
                    if *offset > 0 {
                        let w = frames_to_ticks(*offset, bpm, sr) as f32 * ppt;
                        if w > 2.0 {
                            let gr = egui::Rect::from_min_max(pos2(r.left() - w, r.top() + 16.0), pos2(r.left(), r.bottom()));
                            gp.rect_filled(gr, 0.0, col.gamma_multiply(0.2));
                            draw_wave_range(&gp, gr.shrink2(vec2(0.0, 2.0)), peaks, 0.0, *offset as f32 / total, ghost);
                        }
                    }
                    let tail = (sample.len() as u32).saturating_sub(*offset + *frames);
                    if tail > 0 {
                        let w = frames_to_ticks(tail, bpm, sr) as f32 * ppt;
                        if w > 2.0 {
                            let gr = egui::Rect::from_min_max(pos2(r.right(), r.top() + 16.0), pos2(r.right() + w, r.bottom()));
                            gp.rect_filled(gr, 0.0, col.gamma_multiply(0.2));
                            draw_wave_range(&gp, gr.shrink2(vec2(0.0, 2.0)), peaks, (*offset + *frames) as f32 / total, 1.0, ghost);
                        }
                    }
                }
            }
            if hot && r.width() > 24.0 {
                // top grip = time-stretch, bottom corners = trim start / end
                let g = egui::Rect::from_center_size(pos2(r.center().x, r.top() + 3.0), vec2(26.0_f32.min(r.width() - 8.0), 3.0));
                cp.rect_filled(g, 1.5, Color32::from_black_alpha(140));
                let tri = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| egui::Shape::convex_polygon(vec![a, b, c], Color32::from_white_alpha(210), Stroke::NONE);
                cp.add(tri(pos2(r.left() + 2.0, r.bottom() - 2.0), pos2(r.left() + 13.0, r.bottom() - 2.0), pos2(r.left() + 2.0, r.bottom() - 13.0)));
                cp.add(tri(pos2(r.right() - 2.0, r.bottom() - 2.0), pos2(r.right() - 13.0, r.bottom() - 2.0), pos2(r.right() - 2.0, r.bottom() - 13.0)));
            }
            // amber edge = some of the audio is trimmed away (restore it by dragging the handle back)
            let amber = Color32::from_rgb(255, 196, 72);
            if trim_l {
                cp.rect_filled(egui::Rect::from_min_size(pos2(r.left(), r.top() + 16.0), vec2(3.0, (r.height() - 16.0).max(1.0))), 0.0, amber);
            }
            if trim_r {
                cp.rect_filled(egui::Rect::from_min_size(pos2(r.right() - 3.0, r.top() + 16.0), vec2(3.0, (r.height() - 16.0).max(1.0))), 0.0, amber);
            }
        }
        // pitch / bpm chips in the header of audio clips (drag them like in FL Studio)
        let chips: Option<(egui::Rect, egui::Rect)> = if audio_fx.is_some() && r.width() >= 210.0 {
            let cy = header.center().y;
            let bpm_r = egui::Rect::from_center_size(pos2(header.right() - 8.0 - 31.0, cy), vec2(62.0, 13.0));
            let pit_r = egui::Rect::from_center_size(pos2(bpm_r.left() - 4.0 - 27.0, cy), vec2(54.0, 13.0));
            Some((pit_r, bpm_r))
        } else {
            None
        };
        if let (Some((pit_r, bpm_r)), Some((fx, _))) = (chips, audio_fx) {
            let on_p = fx.semitones.abs() >= 1e-3;
            cp.rect_filled(pit_r, 4.0, Color32::from_black_alpha(if on_p { 150 } else { 70 }));
            let ptxt = if !on_p {
                "pitch".to_string()
            } else if (fx.semitones - fx.semitones.round()).abs() < 1e-3 {
                format!("{:+.0} st", fx.semitones)
            } else {
                format!("{:+.1} st", fx.semitones)
            };
            cp.text(pit_r.center(), Align2::CENTER_CENTER, ptxt, FontId::proportional(9.5), Color32::from_white_alpha(if on_p { 255 } else { 150 }));
            let on_b = fx.sample_bpm.is_some();
            cp.rect_filled(bpm_r, 4.0, Color32::from_black_alpha(if on_b { 150 } else { 70 }));
            let btxt = match fx.sample_bpm {
                Some(b) => format!("{b:.1} bpm"),
                None => "bpm".to_string(),
            };
            cp.text(bpm_r.center(), Align2::CENTER_CENTER, btxt, FontId::proportional(9.5), Color32::from_white_alpha(if on_b { 255 } else { 150 }));
        }
        cp.rect_stroke(r, 7.0, if selected { Stroke::new(1.5_f32, Color32::WHITE) } else if is_auto { Stroke::new(1.0_f32, col.gamma_multiply(0.6)) } else { Stroke::new(1.0_f32, Color32::from_white_alpha(38)) });

        // interaction
        let bresp = ui.interact(r, Id::new(("clip", id)), Sense::click_and_drag());
        let hr = egui::Rect::from_min_max(pos2((r.right() - 8.0).max(r.left()), r.top()), r.max);
        let hresp = ui.interact(hr, Id::new(("clip_h", id)), Sense::drag());
        if hresp.hovered() || hresp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            if audio_fx.is_some() {
                hresp.clone().on_hover_text("Drag: trim \u{b7} Alt + drag: time-stretch (changes the BPM)");
            }
        } else if bresp.hovered() || bresp.dragged() {
            ui.ctx().set_cursor_icon(if ui.input(|i| i.modifiers.shift) { CursorIcon::Copy } else { CursorIcon::Grab });
        }

        // chips sit on top of the clip, so they are registered after it
        if let (Some((pit_r, bpm_r)), Some((fx, _))) = (chips, audio_fx) {
            let fine = ui.input(|i| i.modifiers.shift);
            let press = ui.input(|i| i.pointer.press_origin());
            let presp = ui.interact(pit_r, Id::new(("clip_pitch", id)), Sense::click_and_drag());
            if presp.hovered() || presp.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            presp.clone().on_hover_text("Pitch: drag up / down (Shift = fine) \u{b7} double-click = reset");
            if presp.drag_started() {
                s.fx_drag_start = Some(fx.semitones);
                s.sel_clips = vec![id];
                s.focus_playlist = true;
            }
            if presp.dragged() {
                if let (Some(start), Some(o), Some(p)) = (s.fx_drag_start, press, presp.interact_pointer_pos()) {
                    let dy: f32 = o.y - p.y;
                    let v: f32 = if fine { ((start + dy * 0.02) * 20.0).round() / 20.0 } else { (start + dy / 8.0).round() };
                    s.set_clip_fx(id, |f| f.semitones = v);
                }
            }
            if presp.drag_stopped() {
                s.fx_drag_start = None;
            }
            if presp.double_clicked() {
                s.set_clip_fx(id, |f| f.semitones = 0.0);
            }

            let bresp2 = ui.interact(bpm_r, Id::new(("clip_bpm", id)), Sense::click_and_drag());
            if bresp2.hovered() || bresp2.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            bresp2.clone().on_hover_text("Sample BPM: drag up / down (Shift = fine). The clip then follows the project tempo \u{b7} double-click = off");
            if bresp2.drag_started() {
                s.fx_drag_start = Some(fx.sample_bpm.unwrap_or(s.bpm));
                s.sel_clips = vec![id];
                s.focus_playlist = true;
            }
            if bresp2.dragged() {
                if let (Some(start), Some(o), Some(p)) = (s.fx_drag_start, press, bresp2.interact_pointer_pos()) {
                    let dy: f32 = o.y - p.y;
                    let v: f32 = if fine { ((start + dy * 0.02) * 100.0).round() / 100.0 } else { (start + dy / 4.0).round() };
                    s.set_clip_fx(id, |f| f.sample_bpm = Some(v));
                }
            }
            if bresp2.drag_stopped() {
                s.fx_drag_start = None;
            }
            if bresp2.double_clicked() {
                s.set_clip_fx(id, |f| f.sample_bpm = None);
            }
        }

        // Top handle = time-stretch, bottom-left = trim start, bottom-right = trim end.
        if audio_fx.is_some() && r.width() > 24.0 {
            let handle_h = 8.0_f32;
            let top_r = egui::Rect::from_min_max(r.min, pos2(r.right(), r.top() + handle_h));
            let bl_r = egui::Rect::from_min_max(pos2(r.left(), r.bottom() - handle_h), pos2(r.center().x, r.bottom()));
            let br_r = egui::Rect::from_min_max(pos2(r.center().x, r.bottom() - handle_h), r.max);

            let tr = ui.interact(top_r, Id::new(("clip_stretch", id)), Sense::drag());
            if tr.hovered() || tr.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            tr.clone().on_hover_text("Drag: time-stretch the clip to fit the tempo (keeps the pitch in Stretch mode)");
            if tr.drag_started() {
                s.sel_clips = vec![id];
                s.clip_drag = Some(ClipDrag::Stretch);
                s.focus_playlist = true;
            }
            if tr.dragged() {
                if let Some(p) = tr.interact_pointer_pos() {
                    let end = s.snap_round((p.x - origin) / ppt, s.eff_snap());
                    if let Some((ticks, new_bpm)) = stretch_clip(s, id, end) {
                        let msg = format!("Stretch: {}  \u{b7}  {:.2} BPM (project {:.1})", dur_label(ticks, s.bpm), new_bpm, s.bpm);
                        s.drag_hint = Some((p + vec2(14.0, -28.0), msg));
                    }
                }
            }
            if tr.drag_stopped() {
                s.clip_drag = None;
                s.push_clips();
            }

            let tl = ui.interact(bl_r, Id::new(("clip_trim_l", id)), Sense::drag());
            if tl.hovered() || tl.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            tl.clone().on_hover_text("Drag: trim the start (does not change the speed or BPM)");
            if tl.drag_started() {
                let (o, f) = match &s.clips[ci].body {
                    ClipBody::Audio { offset, frames, .. } => (*offset, *frames),
                    _ => (0, 1),
                };
                s.checkpoint();
                s.sel_clips = vec![id];
                s.clip_drag = Some(ClipDrag::TrimStart { start, offset: o, frames: f });
                s.focus_playlist = true;
            }
            if tl.dragged() {
                let info = match &s.clip_drag {
                    Some(ClipDrag::TrimStart { start, offset, frames }) => Some((*start, *offset, *frames)),
                    _ => None,
                };
                if let (Some((os, oo, of)), Some(p)) = (info, tl.interact_pointer_pos()) {
                    let tick = s.snap_round((p.x - origin) / ppt, s.eff_snap());
                    s.trim_clip_start_to(id, tick, os, oo, of);
                    let removed = match &s.clips[ci].body {
                        ClipBody::Audio { offset, .. } => *offset as f32 / sr * 1000.0,
                        _ => 0.0,
                    };
                    s.drag_hint = Some((p + vec2(14.0, -28.0), format!("Trim start: {removed:.0} ms removed")));
                }
            }
            if tl.drag_stopped() {
                s.clip_drag = None;
                s.push_clips();
            }

            let te = ui.interact(br_r, Id::new(("clip_trim_r", id)), Sense::drag());
            if te.hovered() || te.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            te.clone().on_hover_text("Drag: trim the end (does not change the speed or BPM)");
            if te.drag_started() {
                s.checkpoint();
                s.sel_clips = vec![id];
                s.clip_drag = Some(ClipDrag::Resize);
                s.focus_playlist = true;
            }
            if te.dragged() {
                if let Some(p) = te.interact_pointer_pos() {
                    let end = s.snap_round((p.x - origin) / ppt, s.eff_snap());
                    resize_clip(s, id, end);
                    let removed = match &s.clips[ci].body {
                        ClipBody::Audio { sample, offset, frames, .. } => (sample.len() as u32).saturating_sub(*offset + *frames) as f32 / sr * 1000.0,
                        _ => 0.0,
                    };
                    s.drag_hint = Some((p + vec2(14.0, -28.0), format!("Trim end: {removed:.0} ms removed")));
                }
            }
            if te.drag_stopped() {
                s.clip_drag = None;
                s.push_clips();
            }
        }

        let is_pattern = matches!(s.clips[ci].body, ClipBody::Pattern { .. });
        if is_pattern && (hresp.hovered() || hresp.dragged()) {
            hresp.clone().on_hover_text("Drag: continue the pattern (the new space is empty, ready for notes)\nCtrl+drag: repeat the pattern instead");
        }
        if hresp.drag_started() {
            if !selected {
                s.sel_clips = vec![id];
            }
            s.checkpoint();
            s.clip_drag = Some(match &s.clips[ci].body {
                ClipBody::Pattern { pat, .. } => ClipDrag::Extend { orig_plen: s.pat_index(*pat).map(|pi| s.patterns[pi].len).unwrap_or(BAR_TICKS) },
                _ => ClipDrag::Resize,
            });
            s.focus_playlist = true;
        }
        if hresp.dragged() {
            if let Some(p) = hresp.interact_pointer_pos() {
                let end = s.snap_round((p.x - origin) / ppt, s.eff_snap());
                if is_pattern {
                    let repeat = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
                    let orig_plen = match &s.clip_drag {
                        Some(ClipDrag::Extend { orig_plen }) => *orig_plen,
                        _ => BAR_TICKS,
                    };
                    if let Some((ticks, continued)) = extend_pattern_clip(s, id, end, repeat, orig_plen) {
                        let msg = format!("{}  {}", if continued { "Continue pattern:" } else if repeat { "Repeat:" } else { "Length:" }, dur_label(ticks, s.bpm));
                        s.drag_hint = Some((p + vec2(14.0, -28.0), msg));
                    }
                } else if audio_fx.is_some() && ui.input(|i| i.modifiers.alt) {
                    let _ = stretch_clip(s, id, end);
                } else {
                    resize_clip(s, id, end);
                }
            }
        }
        if hresp.drag_stopped() {
            s.clip_drag = None;
            s.push_clips();
        }

        if bresp.drag_started() {
            // Shift + drag: leave the clip(s) where they are and drag copies instead
            let copy_drag = ui.input(|i| i.modifiers.shift);
            if copy_drag {
                s.checkpoint();
                let src = if selected { s.sel_clips.clone() } else { vec![id] };
                s.sel_clips = s.clone_clips_in_place(&src);
            } else if !selected {
                if ctrl {
                    s.sel_clips.push(id);
                } else {
                    s.sel_clips = vec![id];
                }
            }
            s.selected_track = t;
            s.focus_playlist = true;
            if let Some(o) = ui.input(|i| i.pointer.press_origin()) {
                let orig: Vec<_> = s.sel_clips.iter().filter_map(|&cid| s.clip_index(cid).map(|i| (cid, s.clips[i].start, s.clips[i].track))).collect();
                if !copy_drag {
                    s.checkpoint();
                }
                s.clip_drag = Some(ClipDrag::Move { anchor_tick: ((o.x - origin) / ppt) as i64, anchor_row: row_at(s, o.y - s.pl_row0_y), orig });
            }
        }
        if bresp.dragged() {
            s.pl_drag_ptr = bresp.interact_pointer_pos();
        }
        if bresp.drag_stopped() {
            s.clip_drag = None;
            s.pl_drag_ptr = None;
            s.push_clips();
        }
        if bresp.double_clicked() {
            if audio_fx.is_some() {
                // Sample Editor window
                s.open_editor(id);
                s.selected_track = t;
                s.focus_playlist = true;
            } else {
                s.open_clip(id);
            }
        } else if bresp.clicked() {
            if ctrl {
                if let Some(pos) = s.sel_clips.iter().position(|&x| x == id) {
                    s.sel_clips.remove(pos);
                } else {
                    s.sel_clips.push(id);
                }
            } else {
                s.sel_clips = vec![id];
            }
            s.selected_track = t;
            s.focus_playlist = true;
        }
        if bresp.secondary_clicked() && !selected {
            s.sel_clips = vec![id];
        }
        bresp.context_menu(|ui| {
            if audio_fx.is_some() {
                if ui.button("Sample settings\u{2026}").clicked() {
                    actions.push(ClipAction::Settings(id));
                    ui.close_menu();
                }
            } else if is_auto {
                if ui.button("Edit automation").clicked() {
                    actions.push(ClipAction::Edit(id));
                    ui.close_menu();
                }
            } else {
                if ui.button("Edit pattern").clicked() {
                    actions.push(ClipAction::Edit(id));
                    ui.close_menu();
                }
                if ui.button("Make pattern unique").on_hover_text("Give this clip its own copy of the pattern").clicked() {
                    actions.push(ClipAction::MakeUnique(id));
                    ui.close_menu();
                }
            }
            ui.separator();
            if ui.button("Copy").clicked() {
                actions.push(ClipAction::Copy);
                ui.close_menu();
            }
            if ui.button("Cut").clicked() {
                actions.push(ClipAction::Cut);
                ui.close_menu();
            }
            if ui.button("Paste at playhead").clicked() {
                actions.push(ClipAction::Paste);
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Duplicate").clicked() {
                actions.push(ClipAction::Duplicate(id));
                ui.close_menu();
            }
            if ui.button("Split at playhead").clicked() {
                actions.push(ClipAction::Split);
                ui.close_menu();
            }
            if ui.button("Delete").clicked() {
                actions.push(ClipAction::Delete(id));
                ui.close_menu();
            }
        });
    }
}

fn resize_clip(s: &mut State, id: u64, end_tick: u32) {
    let (bpm, sr) = (s.bpm, s.sample_rate);
    let min = (s.pl_snap.max(PPQ / 16)).max(1);
    let Some(i) = s.clip_index(id) else { return };
    let start = s.clips[i].start;
    let want = end_tick.saturating_sub(start).max(min);
    match &mut s.clips[i].body {
        ClipBody::Audio { sample, offset, frames, .. } => {
            let max_frames = (sample.len() as u32).saturating_sub(*offset).max(1);
            *frames = ticks_to_frames(want, bpm, sr).clamp(1, max_frames);
        }
        ClipBody::Pattern { len, .. } => *len = want,
        ClipBody::Automation { auto } => auto.len = want,
    }
}

/// Right-edge drag of a pattern clip.
///  * past the pattern's end: the pattern itself is *continued*: it becomes longer (new empty space to write
///    notes in) and every clip that shows exactly one run of it follows; nothing is copied;
///  * Ctrl: the content is repeated (the clip gets longer, the pattern does not);
///  * back inside the pattern: only the clip is shortened.
/// `orig_plen` is the pattern length when the drag began, so one drag can go back and forth.
/// Returns (new clip length in ticks, whether the pattern itself was continued).
pub(crate) fn extend_pattern_clip(s: &mut State, id: u64, end_tick: u32, repeat: bool, orig_plen: u32) -> Option<(u32, bool)> {
    let i = s.clip_index(id)?;
    let min = (s.pl_snap.max(PPQ / 16)).max(1);
    let want = end_tick.saturating_sub(s.clips[i].start).max(min);
    let ClipBody::Pattern { pat, .. } = &s.clips[i].body else { return None };
    let pat = *pat;
    let pi = s.pat_index(pat)?;
    let continued = !repeat && want > orig_plen;
    let new_plen = if continued { want } else { orig_plen };
    let cur_plen = s.patterns[pi].len;
    if new_plen != cur_plen {
        for c in &mut s.clips {
            if c.id == id {
                continue;
            }
            if let ClipBody::Pattern { pat: p, len } = &mut c.body {
                if *p == pat && *len == cur_plen {
                    *len = new_plen;
                }
            }
        }
        s.patterns[pi].len = new_plen;
        if s.editing_pattern == Some(pat) {
            s.roll_len = new_plen;
        }
    }
    if let ClipBody::Pattern { len, .. } = &mut s.clips[i].body {
        *len = want;
    }
    Some((want, continued))
}

/// Stretch an audio clip so it ends at `end_tick` by changing its sample BPM (the clip then really
/// plays slower / faster). Returns the new length in ticks and the new sample BPM.
pub(crate) fn stretch_clip(s: &mut State, id: u64, end_tick: u32) -> Option<(u32, f32)> {
    let i = s.clip_index(id)?;
    let want = end_tick.saturating_sub(s.clips[i].start).max(PPQ / 8);
    let secs: f32 = match &s.clips[i].body {
        ClipBody::Audio { orig, sample, frames, .. } => orig.duration_secs() * (*frames as f32 / sample.len().max(1) as f32),
        _ => return None,
    };
    if secs < 0.01 {
        return None;
    }
    let bpm: f32 = want as f32 * 60.0 / (PPQ as f32 * secs);
    s.set_clip_fx(id, |f| f.sample_bpm = Some(bpm));
    s.stretch_preview.insert(id, want);
    Some((want, bpm.clamp(40.0, 300.0)))
}

/// "2 bars + 1.50 beats (3.20 s)"
fn dur_label(ticks: u32, bpm: f32) -> String {
    let bars = ticks / BAR_TICKS;
    let beats = (ticks % BAR_TICKS) as f32 / PPQ as f32;
    let secs = ticks as f32 / PPQ as f32 * 60.0 / bpm.max(1.0);
    format!("{bars} bar{} + {beats:.2} beats ({secs:.2} s)", if bars == 1 { "" } else { "s" })
}
