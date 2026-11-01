use crate::dialogs;
use crate::panels::{automation::automation_editor, inspector::inspector, mixer::mixer, patterns::patterns_panel, playlist::playlist, roll::piano_roll, samples::samples_panel};
use crate::state::*;
use crate::theme::{self, *};
use crate::widgets::*;
use core_engine::{Command, EngineHandle, PPQ};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Frame, Id, LayerId, Layout, Margin, Order, Rect, RichText, Sense, Shape, Stroke, Ui};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use project_io::ExportFormat;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Panel {
    Samples,
    Patterns,
    Playlist,
    Inspector,
    PianoRoll,
    Mixer,
}

struct Viewer<'a> {
    s: &'a mut State,
}

impl TabViewer for Viewer<'_> {
    type Tab = Panel;

    fn title(&mut self, tab: &mut Panel) -> egui::WidgetText {
        match tab {
            Panel::Samples => "Library",
            Panel::Patterns => "Patterns",
            Panel::Playlist => "Playlist",
            Panel::Inspector => "Inspector",
            Panel::PianoRoll => "Piano Roll",
            Panel::Mixer => "Mixer",
        }
        .into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Panel) {
        // Remember which editor was clicked last so Delete / Ctrl+A act on the right thing.
        if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(ui.max_rect()) {
            match tab {
                Panel::Playlist => self.s.focus_playlist = true,
                Panel::PianoRoll => self.s.focus_playlist = false,
                _ => {}
            }
        }
        Frame::none().inner_margin(Margin::same(10.0)).show(ui, |ui| match tab {
            Panel::Samples => samples_panel(self.s, ui),
            Panel::Patterns => patterns_panel(self.s, ui),
            Panel::Playlist => playlist(self.s, ui),
            Panel::Inspector => inspector(self.s, ui),
            Panel::PianoRoll => piano_roll(self.s, ui),
            Panel::Mixer => mixer(self.s, ui),
        });
    }

    fn closeable(&mut self, _tab: &mut Panel) -> bool {
        false
    }

    fn clear_background(&self, _tab: &Panel) -> bool {
        true
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Rewind,
    Stop,
    Play,
    Record,
}

/// Transport button painted with shapes (no icon font needed).
fn icon_button(ui: &mut Ui, icon: Icon, active: bool, col: Color32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 30.0), Sense::click());
    if active {
        ui.painter().rect_filled(rect, 9.0, Color32::from_rgb(64, 64, 72));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 9.0, Color32::from_rgb(51, 53, 59));
    }
    let c = rect.center();
    let fg = if active { col } else { Color32::from_rgb(200, 200, 208) };
    let p = ui.painter();
    match icon {
        Icon::Rewind => {
            p.rect_filled(Rect::from_center_size(c + vec2(-6.0, 0.0), vec2(2.5, 11.0)), 1.0, fg);
            p.add(Shape::convex_polygon(vec![pos2(c.x + 7.0, c.y - 6.0), pos2(c.x + 7.0, c.y + 6.0), pos2(c.x - 3.0, c.y)], fg, Stroke::NONE));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, vec2(11.0, 11.0)), 2.0, fg);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![pos2(c.x - 5.0, c.y - 7.0), pos2(c.x - 5.0, c.y + 7.0), pos2(c.x + 7.0, c.y)], fg, Stroke::NONE));
        }
        Icon::Record => {
            p.circle_filled(c, 6.5, if active { RED } else { RED.gamma_multiply(0.75) });
        }
    }
    resp.clicked()
}

/// Cold-start splash: shown until the plugin scan and the first library scan have finished
/// (at least `SPLASH_MIN` seconds, at most `SPLASH_MAX`), then fades out.
struct Splash {
    start: std::time::Instant,
    fade_from: Option<std::time::Instant>,
    progress: f32,
}

const SPLASH_MIN: f32 = 2.2;
const SPLASH_MAX: f32 = 9.0;
const SPLASH_FADE: f32 = 0.5;

pub struct DawApp {
    dock: DockState<Panel>,
    s: State,
    splash: Option<Splash>,
}

impl DawApp {
    fn new(handle: EngineHandle, sample_rate: f32, input: Option<AudioInput>) -> Self {
        // Library + Patterns | Playlist (+ Piano Roll / Mixer below) | Inspector
        let mut dock = DockState::new(vec![Panel::Samples]);
        let surface = dock.main_surface_mut();
        let [left, main] = surface.split_right(NodeIndex::root(), 0.17, vec![Panel::Playlist]);
        let [pl, _] = surface.split_right(main, 0.79, vec![Panel::Inspector]);
        surface.split_below(pl, 0.55, vec![Panel::PianoRoll, Panel::Mixer]);
        // Patterns window under the Library
        surface.split_below(left, 0.60, vec![Panel::Patterns]);
        Self { dock, s: State::new(handle, sample_rate, input), splash: Some(Splash { start: std::time::Instant::now(), fade_from: None, progress: 0.0 }) }
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(66.0)
            .frame(Frame::none().fill(Color32::from_rgb(36, 38, 44)).inner_margin(Margin::symmetric(14.0, 10.0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("RUST DAW").strong().size(14.0).color(MUTED));
                    ui.add_space(14.0);

                    ui.scope(|ui| {
                        ui.set_max_height(40.0);
                        Frame::none().fill(Color32::from_rgb(24, 26, 32)).rounding(12.0).inner_margin(Margin::symmetric(5.0, 4.0)).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if icon_button(ui, Icon::Rewind, false, TEXT) {
                                self.s.seek(0);
                            }
                            if icon_button(ui, Icon::Stop, false, TEXT) {
                                self.s.stop_all();
                            }
                            if icon_button(ui, Icon::Play, self.s.playing, GREEN) {
                                self.s.toggle_play();
                            }
                            if icon_button(ui, Icon::Record, self.s.recording, RED) {
                                if self.s.recording {
                                    self.s.stop_all();
                                } else {
                                    self.s.start_record();
                                }
                            }
                        });
                    });

                    ui.add_space(12.0);
                    ui.scope(|ui| {
                        ui.set_max_height(46.0);
                        Frame::none().fill(LCD_BG).rounding(9.0).stroke(Stroke::new(1.0_f32, STROKE)).inner_margin(Margin::symmetric(16.0, 3.0)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let t = self.s.playhead;
                                let (bar, beat, tick) = (t / BAR_TICKS + 1, (t % BAR_TICKS) / PPQ + 1, t % PPQ);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new("BAR   BEAT   TICK").size(9.0).color(MUTED));
                                    ui.label(RichText::new(format!("{:>3}   {}   {:03}", bar, beat, tick)).monospace().size(19.0).color(LCD_FG));
                                });
                                ui.add_space(20.0);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new("TIME").size(9.0).color(MUTED));
                                    let secs = t as f64 / PPQ as f64 * 60.0 / self.s.bpm as f64;
                                    ui.label(RichText::new(format!("{:02}:{:04.1}", (secs / 60.0) as u32, secs % 60.0)).monospace().size(19.0).color(LCD_FG));
                                });
                                ui.add_space(20.0);
                                ui.vertical(|ui| {
                                    ui.label(RichText::new("TEMPO").size(9.0).color(MUTED));
                                    if ui.add(egui::DragValue::new(&mut self.s.bpm).clamp_range(40.0..=300.0).speed(0.5).max_decimals(1)).changed() {
                                        self.s.handle.send(Command::SetBpm(self.s.bpm));
                                        self.s.on_bpm_changed();
                                    }
                                });
                            });
                        });
                    });

                    ui.add_space(12.0);
                    ui.scope(|ui| {
                        ui.set_max_height(32.0);
                        let on = self.s.cycle_on;
                        let b = egui::Button::new(RichText::new("Cycle").color(if on { Color32::BLACK } else { TEXT })).fill(if on { YELLOW } else { Color32::from_rgb(24, 26, 32) }).rounding(10.0);
                        if ui.add(b).on_hover_text("Loop the cycle range (Shift+drag on the ruler to set it)").clicked() {
                            self.s.cycle_on = !self.s.cycle_on;
                            self.s.apply_cycle();
                        }
                        let b = egui::Button::new(RichText::new("Save").color(TEXT)).fill(Color32::from_rgb(24, 26, 32)).rounding(10.0);
                        if ui.add(b).on_hover_text("Save the project (Ctrl+S): tempo, patterns, playlist and sample settings").clicked() {
                            self.save_project_ui();
                        }
                        let b = egui::Button::new(RichText::new("Open").color(TEXT)).fill(Color32::from_rgb(24, 26, 32)).rounding(10.0);
                        if ui.add(b).on_hover_text("Open a saved project").clicked() {
                            self.open_project_ui();
                        }
                        let b = egui::Button::new(RichText::new("Export").color(TEXT)).fill(Color32::from_rgb(24, 26, 32)).rounding(10.0);
                        if ui.add(b).on_hover_text("Render the project to WAV or MP3").clicked() {
                            self.s.export.open = true;
                        }
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (ml, mr) = self.s.meters.master();
                        meter_h(ui, ml.max(mr), 90.0);
                        ui.label(RichText::new("OUT").size(10.0).color(MUTED));
                        ui.add_space(14.0);
                        meter_h(ui, self.s.in_peak, 70.0);
                        ui.label(RichText::new(if self.s.input.is_some() { "IN" } else { "NO IN" }).size(10.0).color(MUTED));
                    });
                });
            });
    }

    fn overlays(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("drop_overlay")));
            let r = screen.shrink(12.0);
            p.rect_filled(r, 16.0, Color32::from_rgba_unmultiplied(10, 10, 12, 220));
            dashed_rect(&p, r, ACCENT);
            p.text(r.center() - vec2(0.0, 14.0), Align2::CENTER_CENTER, "Drop to import", FontId::proportional(30.0), TEXT);
            p.text(r.center() + vec2(0.0, 22.0), Align2::CENTER_CENTER, "On a track lane: audio clip \u{b7} on a track header: sampler instrument \u{b7} anywhere else: add to the library", FontId::proportional(14.0), MUTED);
        }

        if let Some(id) = self.s.drag {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let entry = self.s.lib_pos(id);
            if let (Some(pos), Some(i)) = (ctx.pointer_latest_pos(), entry) {
                let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("drag_ghost")));
                let r = Rect::from_min_size(pos + vec2(14.0, 10.0), vec2(200.0, 34.0));
                p.rect_filled(r, 9.0, CARD_HI);
                p.rect_stroke(r, 9.0, Stroke::new(1.5_f32, ACCENT));
                p.text(r.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, &self.s.library[i].name, FontId::proportional(13.0), TEXT);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.s.drag = None;
            } else if ctx.input(|i| i.pointer.any_released()) {
                let done = ctx.pointer_latest_pos().map_or(false, |p| self.s.drop_sample_at(id, p));
                if !done {
                    self.s.toast("Drop onto a track lane or a track header", false);
                }
                self.s.drag = None;
            }
        }

        if let Some((pos, text)) = self.s.drag_hint.clone() {
            let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("drag_hint")));
            let w = text.chars().count() as f32 * 6.6 + 18.0;
            let r = Rect::from_min_size(pos, vec2(w, 24.0));
            p.rect_filled(r, 7.0, Color32::from_black_alpha(225));
            p.rect_stroke(r, 7.0, Stroke::new(1.0_f32, ACCENT));
            p.text(r.left_center() + vec2(9.0, 0.0), Align2::LEFT_CENTER, text, FontId::proportional(12.0), TEXT);
        }

        if let Some(pid) = self.s.pat_drag {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let name = self.s.pat_index(pid).map(|i| self.s.patterns[i].name.clone());
            if let (Some(pos), Some(name)) = (ctx.pointer_latest_pos(), name) {
                let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("pat_drag_ghost")));
                let r = Rect::from_min_size(pos + vec2(14.0, 10.0), vec2(170.0, 30.0));
                p.rect_filled(r, 9.0, CARD_HI);
                p.rect_stroke(r, 9.0, Stroke::new(1.5_f32, GREEN));
                p.text(r.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, name, FontId::proportional(13.0), TEXT);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.s.pat_drag = None;
            } else if ctx.input(|i| i.pointer.any_released()) {
                let done = ctx.pointer_latest_pos().map_or(false, |p| self.s.drop_pattern_at(pid, p));
                if !done {
                    self.s.toast("Drop a pattern onto a track lane in the Playlist", false);
                }
                self.s.pat_drag = None;
            }
        }

        if let Some((msg, is_err, t)) = self.s.toast.clone() {
            if t.elapsed() < Duration::from_millis(3800) {
                let galley = ctx.fonts(|f| f.layout_no_wrap(msg, FontId::proportional(14.0), TEXT));
                let size = galley.size() + vec2(32.0, 20.0);
                let r = Rect::from_min_size(pos2(screen.right() - size.x - 20.0, screen.bottom() - size.y - 20.0), size);
                let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("toast")));
                p.rect_filled(r, 12.0, CARD_HI);
                p.rect_stroke(r, 12.0, Stroke::new(1.5_f32, if is_err { RED } else { ACCENT }));
                p.galley(r.min + vec2(16.0, 10.0), galley, TEXT);
            } else {
                self.s.toast = None;
            }
        }
    }

    fn project_default_path() -> Option<std::path::PathBuf> {
        project_io::Config::path().and_then(|p| p.parent().map(|d| d.join("projects").join("project.rdaw")))
    }

    fn save_project_ui(&mut self) {
        let path = match self.s.project_path.clone() {
            Some(p) => Some(p),
            None if dialogs::has_native_dialogs() => dialogs::pick_save("project.rdaw", "rdaw", None),
            None => Self::project_default_path(),
        };
        let Some(path) = path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match self.s.save_project(&path) {
            Ok(()) => {
                self.s.project_path = Some(path.clone());
                self.s.toast(format!("Saved {}", path.display()), false);
            }
            Err(e) => self.s.toast(format!("Couldn't save the project: {e}"), true),
        }
    }

    fn open_project_ui(&mut self) {
        let path = if dialogs::has_native_dialogs() { dialogs::pick_open("rdaw") } else { Self::project_default_path().filter(|p| p.exists()) };
        let Some(path) = path else {
            self.s.toast("No saved project found", false);
            return;
        };
        match self.s.load_project(&path) {
            Ok(()) => self.s.toast(format!("Opened {}", path.display()), false),
            Err(e) => self.s.toast(format!("Couldn't open the project: {e}"), true),
        }
    }

    /// Sample Editor: opened by double-clicking an audio clip in the playlist. Changes are applied
    /// to the clip live (so they can be heard); Cancel restores it, Apply / closing keeps them.
    fn sample_editor(&mut self, ctx: &egui::Context) {
        let Some(id) = self.s.editor.as_ref().map(|e| e.id) else { return };
        let Some(ci) = self.s.clip_index(id) else {
            self.s.stop_clip_preview();
            self.s.editor = None;
            return;
        };
        let (peaks, len, offset, frames, fx, pending, orig_secs) = match &self.s.clips[ci].body {
            ClipBody::Audio { peaks, sample, offset, frames, fx, pending, orig, .. } => (peaks.clone(), sample.len() as u32, *offset, *frames, *fx, *pending, orig.duration_secs()),
            _ => {
                self.s.editor = None;
                return;
            }
        };
        let name = self.s.clips[ci].name.clone();
        let track = self.s.clips[ci].track;
        let (sr, bpm) = (self.s.sample_rate, self.s.bpm);
        let start_tick = self.s.clips[ci].start;
        let clip_ticks = self.s.clips[ci].len_ticks(bpm, sr);
        let (playing, playhead) = (self.s.playing, self.s.playhead);
        let previewing = self.s.editor.as_ref().map_or(false, |e| e.preview.is_some());

        let mut nf = fx;
        let mut new_proj_bpm: Option<f32> = None;
        let (mut detect, mut apply, mut cancel, mut reset, mut toggle_preview) = (false, false, false, false, false);
        let mut trim_start: Option<u32> = None; // new start offset in frames
        let mut trim_end: Option<u32> = None; // new "removed from the end" in frames
        let mut open = true;
        let mut auto_clip_volume = false;
        let len_f = len.max(1) as f32;

        egui::Window::new(format!("Sample Editor \u{2014} {name}"))
            .id(Id::new("sample_editor"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(600.0)
            .default_pos(pos2(280.0, 110.0))
            .show(ctx, |ui| {
                // ---- numbers: length and position ----
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Length {:.3} s", frames as f32 / sr)).strong());
                    ui.label(RichText::new(format!("(whole sample {:.3} s, original file {:.3} s)", len as f32 / sr, orig_secs)).color(MUTED));
                    let inside = playing && playhead >= start_tick && playhead < start_tick + clip_ticks;
                    let pos_s = if inside { ticks_to_frames(playhead - start_tick, bpm, sr) as f32 / sr } else { 0.0 };
                    ui.label(format!("Position {pos_s:.3} s"));
                    if pending {
                        ui.label(RichText::new("rendering\u{2026}").color(ACCENT));
                    }
                });

                // ---- waveform with trim handles, fades and playback position ----
                let (wrect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 130.0), Sense::hover());
                {
                    let p = ui.painter_at(wrect);
                    p.rect_filled(wrect, 6.0, Color32::from_rgb(18, 20, 26));
                    draw_wave_range(&p, wrect.shrink2(vec2(0.0, 4.0)), &peaks, 0.0, 1.0, track_color(track).gamma_multiply(0.95));
                    let x_of = |frame: f32| wrect.left() + wrect.width() * frame / len_f;
                    let (fx0, fx1) = (x_of(offset as f32), x_of((offset + frames) as f32));
                    // trimmed-away parts are dimmed
                    p.rect_filled(Rect::from_min_max(wrect.min, pos2(fx0, wrect.bottom())), 0.0, Color32::from_black_alpha(165));
                    p.rect_filled(Rect::from_min_max(pos2(fx1, wrect.top()), wrect.max), 0.0, Color32::from_black_alpha(165));
                    // fades
                    let fi = x_of(offset as f32 + nf.fade_in_ms * sr / 1000.0).min(fx1);
                    let fo = x_of((offset + frames) as f32 - nf.fade_out_ms * sr / 1000.0).max(fx0);
                    if nf.fade_in_ms > 0.0 {
                        p.line_segment([pos2(fx0, wrect.bottom() - 2.0), pos2(fi, wrect.top() + 2.0)], Stroke::new(1.5_f32, ACCENT));
                    }
                    if nf.fade_out_ms > 0.0 {
                        p.line_segment([pos2(fo, wrect.top() + 2.0), pos2(fx1, wrect.bottom() - 2.0)], Stroke::new(1.5_f32, ACCENT));
                    }
                    // playback position
                    if playing && playhead >= start_tick && playhead < start_tick + clip_ticks {
                        let e = ticks_to_frames(playhead - start_tick, bpm, sr) as f32;
                        let at = if nf.reverse { (offset + frames) as f32 - e } else { offset as f32 + e };
                        let x = x_of(at);
                        p.line_segment([pos2(x, wrect.top()), pos2(x, wrect.bottom())], Stroke::new(2.0_f32, Color32::WHITE));
                    }
                    if nf.reverse {
                        p.text(wrect.right_top() + vec2(-8.0, 8.0), Align2::RIGHT_TOP, "REVERSED", FontId::proportional(11.0), ACCENT);
                    }
                    let amber = Color32::from_rgb(255, 196, 72);
                    for x in [fx0, fx1] {
                        p.line_segment([pos2(x, wrect.top()), pos2(x, wrect.bottom())], Stroke::new(2.0_f32, amber));
                    }
                }
                let fx0 = wrect.left() + wrect.width() * offset as f32 / len_f;
                let fx1 = wrect.left() + wrect.width() * (offset + frames) as f32 / len_f;
                for (k, x) in [fx0, fx1].into_iter().enumerate() {
                    let hr = Rect::from_center_size(pos2(x, wrect.center().y), vec2(14.0, wrect.height()));
                    let hresp = ui.interact(hr, Id::new(("ed_trim", k)), Sense::drag());
                    if hresp.hovered() || hresp.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    hresp.clone().on_hover_text(if k == 0 { "Drag: start offset (trim the start)" } else { "Drag: end offset (trim the end)" });
                    if hresp.dragged() {
                        if let Some(pt) = hresp.interact_pointer_pos() {
                            let frame = (((pt.x - wrect.left()) / wrect.width().max(1.0)).clamp(0.0, 1.0) * len_f) as u32;
                            if k == 0 {
                                trim_start = Some(frame);
                            } else {
                                trim_end = Some(len.saturating_sub(frame));
                            }
                        }
                    }
                }
                ui.add_space(4.0);

                // ---- settings ----
                egui::Grid::new("ed_grid").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
                    ui.label("Source BPM");
                    ui.horizontal(|ui| {
                        let mut b: f32 = nf.sample_bpm.unwrap_or(bpm);
                        if ui.add(egui::DragValue::new(&mut b).clamp_range(40.0..=300.0).speed(0.1).max_decimals(2)).changed() {
                            nf.sample_bpm = Some(b);
                        }
                        if ui.button("Detect BPM").on_hover_text("Reads a tempo from the file name, otherwise analyses the audio").clicked() {
                            detect = true;
                        }
                        if ui.small_button("\u{f7}2").on_hover_text("Half tempo (fix an octave error)").clicked() {
                            nf.sample_bpm = Some((b / 2.0).max(40.0));
                        }
                        if ui.small_button("\u{d7}2").on_hover_text("Double tempo (fix an octave error)").clicked() {
                            nf.sample_bpm = Some((b * 2.0).min(300.0));
                        }
                        if nf.sample_bpm.is_some() && ui.small_button("Off").on_hover_text("Stop following the project tempo").clicked() {
                            nf.sample_bpm = None;
                        }
                    });
                    ui.end_row();

                    ui.label("Target BPM");
                    ui.horizontal(|ui| {
                        let mut tb: f32 = bpm;
                        if ui.add(egui::DragValue::new(&mut tb).clamp_range(40.0..=300.0).speed(0.1).max_decimals(2)).changed() {
                            new_proj_bpm = Some(tb);
                        }
                        let note = match nf.sample_bpm {
                            Some(b) => format!("project tempo \u{b7} playback speed \u{d7}{:.3}", bpm / b),
                            None => "project tempo \u{b7} Source BPM is off, so no tempo sync".to_string(),
                        };
                        ui.label(RichText::new(note).small().color(MUTED));
                    });
                    ui.end_row();

                    ui.label("Pitch");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut nf.semitones).clamp_range(-24.0..=24.0).speed(0.05).max_decimals(2).suffix(" st"));
                        if ui.small_button("Reset").clicked() {
                            nf.semitones = 0.0;
                        }
                    });
                    ui.end_row();

                    ui.label("Pitch / stretch mode");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut nf.tape, false, "Stretch").on_hover_text("Pitch and tempo are independent (time-stretch + pitch-shift)");
                        ui.selectable_value(&mut nf.tape, true, "Resample").on_hover_text("Pitch follows speed, like a turntable");
                    });
                    ui.end_row();

                    ui.label("Stretch algorithm");
                    ui.add_enabled_ui(!nf.tape, |ui| {
                        egui::ComboBox::from_id_source("ed_quality").selected_text(nf.quality.label()).show_ui(ui, |ui| {
                            for q in dsp::StretchQuality::ALL {
                                ui.selectable_value(&mut nf.quality, q, q.label());
                            }
                        });
                    });
                    ui.end_row();

                    ui.label("Gain / volume");
                    let gain_resp = ui.add(egui::DragValue::new(&mut nf.gain_db).clamp_range(-60.0..=12.0).speed(0.1).max_decimals(1).suffix(" dB"));
                    gain_resp.on_hover_text("Right-click: automate the volume of this clip").context_menu(|ui| {
                        if ui.button("Create volume automation for this clip").clicked() {
                            auto_clip_volume = true;
                            ui.close_menu();
                        }
                    });
                    ui.end_row();

                    ui.label("Pan");
                    ui.horizontal(|ui| {
                        let mut pan_pct: f32 = nf.pan * 100.0;
                        if ui.add(egui::DragValue::new(&mut pan_pct).clamp_range(-100.0..=100.0).speed(0.5).max_decimals(0).suffix(" %")).changed() {
                            nf.pan = pan_pct / 100.0;
                        }
                        ui.label(RichText::new("\u{2212} left \u{b7} right +").small().color(MUTED));
                    });
                    ui.end_row();

                    ui.label("Start offset");
                    ui.horizontal(|ui| {
                        let mut ms: f32 = offset as f32 / sr * 1000.0;
                        if ui.add(egui::DragValue::new(&mut ms).clamp_range(0.0..=(len as f32 / sr * 1000.0)).speed(1.0).max_decimals(1).suffix(" ms")).changed() {
                            trim_start = Some((ms / 1000.0 * sr) as u32);
                        }
                        ui.label(RichText::new("trims the start, speed is unchanged").small().color(MUTED));
                    });
                    ui.end_row();

                    ui.label("End offset");
                    ui.horizontal(|ui| {
                        let mut ms: f32 = len.saturating_sub(offset + frames) as f32 / sr * 1000.0;
                        if ui.add(egui::DragValue::new(&mut ms).clamp_range(0.0..=(len as f32 / sr * 1000.0)).speed(1.0).max_decimals(1).suffix(" ms")).changed() {
                            trim_end = Some((ms / 1000.0 * sr) as u32);
                        }
                        ui.label(RichText::new("removed from the end", ).small().color(MUTED));
                    });
                    ui.end_row();

                    let fade_max: f32 = (frames as f32 / sr * 1000.0).clamp(1.0, 20_000.0);
                    ui.label("Fade in");
                    ui.add(egui::DragValue::new(&mut nf.fade_in_ms).clamp_range(0.0..=fade_max).speed(1.0).max_decimals(0).suffix(" ms"));
                    ui.end_row();
                    ui.label("Fade out");
                    ui.add(egui::DragValue::new(&mut nf.fade_out_ms).clamp_range(0.0..=fade_max).speed(1.0).max_decimals(0).suffix(" ms"));
                    ui.end_row();

                    ui.label("Options");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut nf.reverse, "Reverse");
                        ui.checkbox(&mut nf.show_wave, "Show waves in the playlist");
                    });
                    ui.end_row();
                });

                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(if previewing { "Stop preview" } else { "Preview" }).on_hover_text("Loops this clip so you can hear the result before applying").clicked() {
                        toggle_preview = true;
                    }
                    if ui.button("Reset").on_hover_text("Back to the untouched sample: no pitch, stretch, trim, fades, gain or pan").clicked() {
                        reset = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new("Apply").strong()).fill(ACCENT)).clicked() {
                            apply = true;
                        }
                        if ui.button("Cancel").on_hover_text("Discard everything done since the editor was opened").clicked() {
                            cancel = true;
                        }
                    });
                });
            });

        // ---- hand the results to the state ----
        if reset {
            self.s.set_clip_trim(id, Some(0), Some(0));
            self.s.set_clip_fx(id, |f| {
                let show = f.show_wave;
                *f = AudioFx { show_wave: show, ..AudioFx::default() };
            });
        } else if nf != fx {
            self.s.set_clip_fx(id, |f| *f = nf);
        }
        if trim_start.is_some() || trim_end.is_some() {
            self.s.set_clip_trim(id, trim_start, trim_end);
        }
        if let Some(b) = new_proj_bpm {
            self.s.bpm = b;
            self.s.handle.send(Command::SetBpm(b));
            self.s.on_bpm_changed();
        }
        if detect {
            self.s.detect_clip_bpm(id);
        }
        if auto_clip_volume {
            self.s.create_automation(core_engine::AutoTarget::ClipGain(id));
        }
        if toggle_preview {
            if previewing {
                self.s.stop_clip_preview();
            } else {
                self.s.start_clip_preview();
            }
        }
        if cancel {
            self.s.close_editor(false);
        } else if apply || !open {
            self.s.close_editor(true);
        }
    }

    /// Generic parameter editors for loaded VST3 plugins (one window per plugin).
    fn plugin_windows(&mut self, ctx: &egui::Context) {
        for (t, slot) in self.s.open_params.clone() {
            let title = if slot == 0 { self.s.tracks[t].inst.label() } else { self.s.tracks[t].fx[slot - 1].clone().unwrap_or_default() };
            let Some(ctrl) = self.s.ctrls.get(&(t, slot)) else {
                self.s.open_params.retain(|k| *k != (t, slot));
                continue;
            };
            let mut open = true;
            let mut changes: Vec<(u32, f64)> = Vec::new();
            let mut auto_req: Option<core_engine::AutoTarget> = None;
            let mut auto_edit: Option<u64> = None;
            egui::Window::new(format!("{} \u{b7} {}", self.s.tracks[t].name, title)).id(Id::new(("plug", t, slot))).open(&mut open).default_width(400.0).default_height(440.0).show(ctx, |ui| {
                let n = ctrl.params.len();
                if n == 0 {
                    ui.label(RichText::new("This plugin has no parameters.").color(MUTED));
                }
                ui.label(RichText::new("Generic parameter list. Use \"Open editor\" for the plugin's own window.").small().color(MUTED));
                egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 26.0, n, |ui, range| {
                    for i in range {
                        let p = &ctrl.params[i];
                        let mut v = ctrl.get(p.id);
                        ui.horizontal(|ui| {
                            ui.add_sized([150.0, 22.0], egui::Label::new(RichText::new(&p.name)).truncate(true));
                            let (changed, wresp) = if p.steps == 1 {
                                let mut on = v > 0.5;
                                let r = ui.checkbox(&mut on, "");
                                let c = r.changed();
                                if c {
                                    v = if on { 1.0 } else { 0.0 };
                                }
                                (c, r)
                            } else {
                                let id = p.id;
                                ui.spacing_mut().slider_width = 150.0;
                                let r = ui.add(egui::Slider::new(&mut v, 0.0..=1.0).custom_formatter(move |x, _| ctrl.display(id, x)).custom_parser(|_| None));
                                (r.changed(), r)
                            };
                            // right-click a parameter: automate it
                            let target = core_engine::AutoTarget::Plugin { track: t as u16, slot: slot as u8, id: p.id };
                            wresp.context_menu(|ui| {
                                if ui.button("Create automation clip").on_hover_text("New automation clip for this parameter at the playhead").clicked() {
                                    auto_req = Some(target);
                                    ui.close_menu();
                                }
                                for (n, cid) in self.s.automation_clips_of(target).into_iter().enumerate() {
                                    if ui.button(format!("Edit automation clip {}", n + 1)).clicked() {
                                        auto_edit = Some(cid);
                                        ui.close_menu();
                                    }
                                }
                            });
                            if changed {
                                ctrl.set(p.id, v);
                                changes.push((p.id, v));
                            }
                        });
                    }
                });
            });
            for (id, value) in changes {
                self.s.handle.send(Command::SetPluginParam { track: t, slot, id, value });
            }
            if let Some(target) = auto_req {
                self.s.create_automation(target);
            }
            if let Some(cid) = auto_edit {
                self.s.open_auto_editor(cid);
            }
            if !open {
                self.s.open_params.retain(|k| *k != (t, slot));
            }
        }
    }

    fn splash_screen(&mut self, ctx: &egui::Context) {
        let Some(sp) = &mut self.splash else { return };
        let t = sp.start.elapsed().as_secs_f32();
        let plugins_done = self.s.scan_rx.is_none();
        let library_done = self.s.cfg.watch_folders.iter().all(|d| self.s.lib_scanned.contains(d));
        if sp.fade_from.is_none() && ((plugins_done && library_done && t >= SPLASH_MIN) || t >= SPLASH_MAX) {
            sp.fade_from = Some(std::time::Instant::now());
        }
        let fade = match sp.fade_from {
            Some(f) => 1.0 - (f.elapsed().as_secs_f32() / SPLASH_FADE).clamp(0.0, 1.0),
            None => 1.0,
        };
        if fade <= 0.0 {
            self.splash = None;
            return;
        }
        let status = if !plugins_done {
            "Scanning VST3 plugins\u{2026}"
        } else if !library_done {
            "Indexing sample library\u{2026}"
        } else {
            "Ready"
        };
        let target: f32 = 0.30 + if plugins_done { 0.35 } else { 0.0 } + if library_done { 0.35 } else { 0.0 };
        // ease towards the target, and never finish before the minimum time has passed
        let cap = (t / SPLASH_MIN).clamp(0.0, 1.0) * 1.0;
        let want = target.min(0.30 + cap * 0.70);
        sp.progress += (want - sp.progress) * 0.08;
        let progress = if sp.fade_from.is_some() { 1.0 } else { sp.progress };

        let screen = ctx.screen_rect();
        let a = |c: Color32| c.gamma_multiply(fade);
        egui::Area::new(Id::new("splash_area")).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            // swallow all input while the splash is up
            ui.allocate_rect(Rect::from_min_size(screen.min, screen.size()), Sense::click_and_drag());
            let p = ui.painter();
            p.rect_filled(screen, 0.0, a(BG));
            // soft glow behind the logo
            let c = screen.center() - vec2(0.0, 30.0);
            for i in 0..6 {
                let r = 150.0 - i as f32 * 22.0;
                p.circle_filled(c, r, a(ACCENT.gamma_multiply(0.018 + 0.006 * i as f32)));
            }
            // animated equaliser bars = the loading animation
            let n = 9;
            let (bw, gap) = (9.0, 7.0);
            let total_w = n as f32 * bw + (n - 1) as f32 * gap;
            for i in 0..n {
                let ph = t * 3.4 - i as f32 * 0.55;
                let h = 14.0 + 52.0 * (0.5 + 0.5 * ph.sin()) * (0.55 + 0.45 * (t * 1.3 + i as f32).cos().abs());
                let x = c.x - total_w * 0.5 + i as f32 * (bw + gap);
                let f = i as f32 / (n - 1) as f32;
                let col = Color32::from_rgb((ACCENT.r() as f32 * (1.0 - f) + GREEN.r() as f32 * f) as u8, (ACCENT.g() as f32 * (1.0 - f) + GREEN.g() as f32 * f) as u8, (ACCENT.b() as f32 * (1.0 - f) + GREEN.b() as f32 * f) as u8);
                p.rect_filled(Rect::from_center_size(pos2(x + bw * 0.5, c.y), vec2(bw, h)), 4.5, a(col));
            }
            p.text(c + vec2(0.0, 82.0), Align2::CENTER_CENTER, "RUST DAW", FontId::proportional(38.0), a(TEXT));
            p.text(c + vec2(0.0, 116.0), Align2::CENTER_CENTER, "DIGITAL AUDIO WORKSTATION", FontId::proportional(11.5), a(MUTED));
            // progress bar
            let bar = Rect::from_center_size(c + vec2(0.0, 168.0), vec2(300.0, 4.0));
            p.rect_filled(bar, 2.0, a(CARD));
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * progress.clamp(0.0, 1.0), bar.height()));
            p.rect_filled(fill, 2.0, a(ACCENT));
            // shimmer on the bar while loading
            if sp.fade_from.is_none() {
                let sx = bar.left() + ((t * 0.9).fract()) * bar.width();
                p.rect_filled(Rect::from_min_max(pos2(sx.min(fill.right()), bar.top()), pos2((sx + 40.0).min(fill.right()), bar.bottom())), 2.0, a(Color32::from_white_alpha(70)));
            }
            p.text(c + vec2(0.0, 190.0), Align2::CENTER_CENTER, status, FontId::proportional(12.5), a(MUTED));
        });
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        if !self.s.export.open {
            return;
        }
        let mut open = true;
        let (cycle_on, end) = (self.s.cycle_on, self.s.clips_end_tick());
        let (bpm, cycle) = (self.s.bpm, self.s.cycle);
        let mut start_it = false;
        let mut cancel_it = false;
        {
            let ex = &mut self.s.export;
            egui::Window::new("Export audio").open(&mut open).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Format").color(MUTED));
                    let before = ex.format;
                    egui::ComboBox::from_id_source("export_fmt").selected_text(ex.format.label()).show_ui(ui, |ui| {
                        for f in [ExportFormat::Wav16, ExportFormat::Wav24, ExportFormat::WavFloat32, ExportFormat::Mp3(128), ExportFormat::Mp3(192), ExportFormat::Mp3(256), ExportFormat::Mp3(320)] {
                            ui.selectable_value(&mut ex.format, f, f.label());
                        }
                    });
                    if ex.format.extension() != before.extension() {
                        let p = std::path::PathBuf::from(ex.path.trim()).with_extension(ex.format.extension());
                        ex.path = p.to_string_lossy().into_owned();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Range").color(MUTED));
                    ui.radio_value(&mut ex.whole_project, true, "Whole project");
                    ui.add_enabled_ui(cycle_on, |ui| ui.radio_value(&mut ex.whole_project, false, "Cycle range"));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Tail").color(MUTED)).on_hover_text("Extra seconds rendered after the end so reverb and delay tails are not cut off");
                    ui.add(egui::DragValue::new(&mut ex.tail_secs).clamp_range(0.0..=30.0).suffix(" s"));
                });
                let (a, b) = if ex.whole_project || !cycle_on { (0, end) } else { cycle };
                let secs = b.saturating_sub(a) as f64 / PPQ as f64 * 60.0 / bpm as f64 + ex.tail_secs as f64;
                ui.label(RichText::new(format!("Length: {:02}:{:04.1}", (secs / 60.0) as u32, secs % 60.0)).color(MUTED));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut ex.path).desired_width(300.0));
                    if dialogs::has_native_dialogs() && ui.button("Browse\u{2026}").clicked() {
                        let name = std::path::Path::new(&ex.path).file_name().and_then(|n| n.to_str()).unwrap_or("mix").to_string();
                        if let Some(p) = dialogs::pick_save(&name, ex.format.extension(), None) {
                            ex.path = p.to_string_lossy().into_owned();
                        }
                    }
                });
                ui.add_space(6.0);
                if ex.running {
                    let f = ex.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                    ui.add(egui::ProgressBar::new(f).show_percentage());
                    ui.label(RichText::new("Rendering faster than real time. Audio playback is paused meanwhile.").small().color(MUTED));
                    if ui.button("Cancel").clicked() {
                        cancel_it = true;
                    }
                } else if ui.add_sized([120.0, 30.0], egui::Button::new(RichText::new("Export").strong()).fill(ACCENT)).clicked() {
                    start_it = true;
                }
            });
        }
        if cancel_it {
            self.s.export.cancel.store(true, Ordering::Relaxed);
        }
        if start_it {
            self.s.stop_all();
            self.s.start_export();
        }
        if !open && !self.s.export.running {
            self.s.export.open = false;
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        // Uses the rectangles recorded during the previous frame.
        let pos = ctx.pointer_latest_pos();
        let mut lane = pos.and_then(|p| self.s.lane_at(p));
        let mut head = pos.and_then(|p| self.s.head_at(p));
        let mut count = 0;
        for f in dropped {
            let Some(path) = f.path else { continue };
            let Some(id) = self.s.import_path(&path) else { continue };
            count += 1;
            if let Some((track, tick)) = lane {
                if let Some(cid) = self.s.add_audio_clip(track, tick, id) {
                    let len = self.s.clip_index(cid).map(|i| self.s.clips[i].len_ticks(self.s.bpm, self.s.sample_rate)).unwrap_or(0);
                    lane = Some((track, tick + len));
                }
            } else if let Some(t) = head.take() {
                self.s.selected_track = t;
                self.s.set_sampler(t, id);
            }
        }
        if count > 1 {
            self.s.toast(format!("Imported {count} samples"), false);
        }
    }
}

impl eframe::App for DawApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.s.poll_engine();
        self.s.service_plugin_editors();
        self.handle_drops(ctx);

        if !ctx.wants_keyboard_input() {
            let (space, del, rec, home, all, dup) = ctx.input(|i| {
                let ctrl = i.modifiers.ctrl || i.modifiers.command;
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.key_pressed(egui::Key::R) && !ctrl,
                    i.key_pressed(egui::Key::Home),
                    ctrl && i.key_pressed(egui::Key::A),
                    ctrl && i.key_pressed(egui::Key::D),
                )
            });
            let (undo, redo, copy, cut, paste, save) = ctx.input(|i| {
                let ctrl = i.modifiers.ctrl || i.modifiers.command;
                let ev_copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                let ev_cut = i.events.iter().any(|e| matches!(e, egui::Event::Cut));
                let ev_paste = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
                (
                    ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    ctrl && (i.key_pressed(egui::Key::Y) || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
                    ev_copy || (ctrl && i.key_pressed(egui::Key::C)),
                    ev_cut || (ctrl && i.key_pressed(egui::Key::X)),
                    ev_paste || (ctrl && i.key_pressed(egui::Key::V)),
                    ctrl && i.key_pressed(egui::Key::S),
                )
            });
            if undo {
                if self.s.editor.is_some() {
                    self.s.close_editor(false); // Ctrl+Z in the Sample Editor = Cancel
                } else {
                    self.s.undo();
                }
            }
            if redo {
                self.s.redo();
            }
            if save {
                self.save_project_ui();
            }
            if self.s.focus_playlist {
                if copy {
                    let n = self.s.copy_selected();
                    if n > 0 {
                        self.s.toast(format!("Copied {n} clip(s)"), false);
                    }
                }
                if cut {
                    self.s.cut_selected();
                }
                if paste {
                    self.s.paste_at_playhead();
                }
            }
            if space {
                self.s.toggle_play();
            }
            if del {
                self.s.delete_selection();
            }
            if all {
                self.s.select_all();
            }
            if dup && self.s.focus_playlist {
                self.s.duplicate_selected();
            }
            if home {
                self.s.seek(0);
            }
            if rec {
                if self.s.recording {
                    self.s.stop_all();
                } else {
                    self.s.start_record();
                }
            }
        }

        self.s.pl_lanes.clear();
        self.s.pl_heads.clear();
        self.s.drag_hint = None;
        self.s.sync_automation_display();
        self.toolbar(ctx);

        let mut style = Style::from_egui(ctx.style().as_ref());
        style.main_surface_border_stroke = Stroke::new(1.0_f32, BG);
        style.tab_bar.bg_fill = Color32::from_rgb(24, 26, 32);
        style.tab_bar.hline_color = BG;
        style.tab.active.bg_fill = PANEL;
        style.tab.active.text_color = TEXT;
        style.tab.active.outline_color = BG;
        style.tab.focused = style.tab.active.clone();
        style.tab.inactive.bg_fill = Color32::from_rgb(24, 26, 32);
        style.tab.inactive.text_color = MUTED;
        style.tab.inactive.outline_color = Color32::TRANSPARENT;
        style.tab.hovered.bg_fill = CARD;
        style.tab.hovered.text_color = TEXT;
        style.tab.tab_body.bg_fill = PANEL;
        style.tab.tab_body.stroke = Stroke::NONE;
        style.separator.color_idle = BG;
        style.separator.color_hovered = ACCENT;
        DockArea::new(&mut self.dock).style(style).show_close_buttons(false).show(ctx, &mut Viewer { s: &mut self.s });

        if self.s.request_roll_tab {
            self.s.request_roll_tab = false;
            if let Some(loc) = self.dock.find_tab(&Panel::PianoRoll) {
                self.dock.set_active_tab(loc);
            }
        }

        self.plugin_windows(ctx);
        self.sample_editor(ctx);
        automation_editor(&mut self.s, ctx);
        self.export_window(ctx);
        self.overlays(ctx);
        self.splash_screen(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.s.recording {
            self.s.finish_recording();
        }
        self.s.handle.send(Command::Stop);
        self.s.cfg.save();
    }
}

/// Open the DAW window. Blocks until it is closed.
pub fn run(handle: EngineHandle, sample_rate: f32, input: Option<AudioInput>) -> eframe::Result<()> {
    run_with(handle, sample_rate, input, Vec::new())
}

/// Like [`run`], but imports `preload` files at startup into the library.
pub fn run_with(handle: EngineHandle, sample_rate: f32, input: Option<AudioInput>, preload: Vec<std::path::PathBuf>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1560.0, 920.0]).with_min_inner_size([1100.0, 650.0]).with_title("Rust DAW"),
        ..Default::default()
    };
    eframe::run_native(
        "Rust DAW",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let mut app = DawApp::new(handle, sample_rate, input);
            for path in &preload {
                app.s.import_path(path);
            }
            if std::env::var_os("RUSTDAW_DEMO").is_some() {
                app.s.demo_scenario();
            }
            Box::new(app)
        }),
    )
}
