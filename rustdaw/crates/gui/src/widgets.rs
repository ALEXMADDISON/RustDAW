use crate::theme::*;
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Frame, Id, Margin, Painter, Rect, Sense, Shape, Stroke, Ui};

/// Draw a min/max peak envelope (`peaks`) filling `rect`.
pub fn draw_wave(p: &Painter, rect: Rect, peaks: &[(f32, f32)], col: Color32) {
    if peaks.is_empty() {
        return;
    }
    let mid = rect.center().y;
    let h = rect.height() * 0.5;
    let n = peaks.len() as f32;
    let w: f32 = (rect.width() / n).max(1.0);
    for (i, (lo, hi)) in peaks.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) / n * rect.width();
        let top = (mid - hi.min(1.0) * h).min(mid - 0.5);
        let bot = (mid - lo.max(-1.0) * h).max(mid + 0.5);
        p.line_segment([pos2(x, top), pos2(x, bot)], Stroke::new(w, col));
    }
}

/// Draw the part `[from, to)` (0..1) of a dense peak array into `rect`, one column per ~1.5 px.
pub fn draw_wave_range(p: &Painter, rect: Rect, peaks: &[(f32, f32)], from: f32, to: f32, col: Color32) {
    if peaks.is_empty() || rect.width() < 1.0 {
        return;
    }
    let n = peaks.len();
    let cols = (rect.width() / 1.5).ceil().max(1.0) as usize;
    let mid = rect.center().y;
    let h = rect.height() * 0.5;
    for c in 0..cols {
        let f0 = from + (to - from) * c as f32 / cols as f32;
        let f1 = from + (to - from) * (c + 1) as f32 / cols as f32;
        let a = ((f0 * n as f32) as usize).min(n - 1);
        let b = ((f1 * n as f32).ceil() as usize).clamp(a + 1, n);
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for &(l, h2) in &peaks[a..b] {
            lo = lo.min(l);
            hi = hi.max(h2);
        }
        let x = rect.left() + (c as f32 + 0.5) / cols as f32 * rect.width();
        p.line_segment([pos2(x, (mid - hi.min(1.0) * h).min(mid - 0.5)), pos2(x, (mid - lo.max(-1.0) * h).max(mid + 0.5))], Stroke::new(1.5_f32, col));
    }
}

pub fn dashed_rect(p: &Painter, r: Rect, col: Color32) {
    let stroke = Stroke::new(1.5_f32, col);
    let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in c.windows(2) {
        p.extend(Shape::dashed_line(&[w[0], w[1]], stroke, 7.0, 5.0));
    }
}

pub fn level_color(v: f32) -> Color32 {
    if v > 1.0 {
        RED
    } else if v > 0.75 {
        YELLOW
    } else {
        GREEN
    }
}

pub fn meter(ui: &mut Ui, l: f32, r: f32, h: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(16.0, h), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, BG);
    for (i, v) in [l, r].into_iter().enumerate() {
        let w = 6.0;
        let x = rect.left() + 1.5 + i as f32 * (w + 1.0);
        let fill = (v / 1.2).clamp(0.0, 1.0) * (h - 3.0);
        ui.painter().rect_filled(Rect::from_min_max(pos2(x, rect.bottom() - 1.5 - fill), pos2(x + w, rect.bottom() - 1.5)), 2.0, level_color(v));
    }
}

pub fn meter_h(ui: &mut Ui, v: f32, w: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 7.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.5, LCD_BG);
    let fw = (v / 1.2).clamp(0.0, 1.0) * rect.width();
    ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(fw, rect.height())), 3.5, level_color(v));
}

pub fn card() -> Frame {
    Frame::none().fill(CARD).rounding(8.0).inner_margin(Margin::symmetric(10.0, 8.0))
}

/// Small custom toggle button (M / S / R style) drawn in `rect`.
pub fn mini_btn(ui: &mut Ui, rect: Rect, id: Id, label: &str, on: bool, on_col: Color32) -> bool {
    let r = ui.interact(rect, id, Sense::click());
    let fill = if on { on_col } else if r.hovered() { CARD_HI } else { Color32::from_rgb(61, 63, 69) };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), if on { Color32::BLACK } else { TEXT });
    r.clicked()
}

pub fn small_caps(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(MUTED).strong());
}
