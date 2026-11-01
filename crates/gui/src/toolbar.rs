//! Building blocks of the macOS-style top toolbar: traffic lights, background,
//! segmented button groups and the settings (three dots) button.
use crate::theme::*;
use eframe::egui::{self, pos2, vec2, Color32, FontId, Frame, Margin, Rect, Response, Rounding, Sense, Shape, Stroke, Ui};

/// Window background of the toolbar: matching the dark DAW background.
pub(crate) fn paint_background(ui: &Ui, rect: Rect) {
    let p = ui.painter();
    p.rect_filled(rect, 0.0, BG);
    p.line_segment([rect.left_top() + vec2(0.0, 0.5), rect.right_top() + vec2(0.0, 0.5)], Stroke::new(1.0_f32, Color32::from_white_alpha(12)));
    p.line_segment([rect.left_bottom() - vec2(0.0, 0.5), rect.right_bottom() - vec2(0.0, 0.5)], Stroke::new(1.0_f32, Color32::from_rgb(12, 13, 17)));
}

/// Red / yellow / green window buttons in inverted order.
pub(crate) fn traffic_lights(ui: &mut Ui, ctx: &egui::Context) {
    let (rect, group) = ui.allocate_exact_size(vec2(66.0, 30.0), Sense::hover());
    let hot = group.hovered() || ui.rect_contains_pointer(rect);
    let maximized = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
    
    // Inverted order: Green (Maximize), Yellow (Minimize), Red (Close)
    let buttons = [
        (Color32::from_rgb(40, 200, 64), 2),  // Green - Maximize
        (Color32::from_rgb(254, 188, 46), 1), // Yellow - Minimize
        (Color32::from_rgb(255, 95, 87), 0),  // Red - Close
    ];

    for (k, (col, action)) in buttons.into_iter().enumerate() {
        let c = pos2(rect.left() + 8.0 + k as f32 * 20.0, rect.center().y);
        let resp = ui.interact(Rect::from_center_size(c, vec2(16.0, 16.0)), egui::Id::new(("traffic", action)), Sense::click());
        let p = ui.painter();
        p.circle_filled(c, 6.5, if resp.is_pointer_button_down_on() { col.gamma_multiply(0.7) } else { col });
        p.circle_stroke(c, 6.5, Stroke::new(1.0_f32, Color32::from_black_alpha(70)));
        if hot {
            let g = Stroke::new(1.3_f32, Color32::from_black_alpha(150));
            match action {
                0 => {
                    p.line_segment([c + vec2(-2.4, -2.4), c + vec2(2.4, 2.4)], g);
                    p.line_segment([c + vec2(-2.4, 2.4), c + vec2(2.4, -2.4)], g);
                }
                1 => { p.line_segment([c + vec2(-2.8, 0.0), c + vec2(2.8, 0.0)], g); }
                _ => {
                    p.line_segment([c + vec2(-2.6, 0.0), c + vec2(2.6, 0.0)], g);
                    p.line_segment([c + vec2(0.0, -2.6), c + vec2(0.0, 2.6)], g);
                }
            }
        }
        if resp.clicked() {
            match action {
                0 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                1 => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                _ => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized)),
            }
        }
    }
}

/// Rounded, recessed container for a row of related controls (like a macOS segmented control).
pub(crate) fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::none()
        .fill(Color32::from_black_alpha(70))
        .stroke(Stroke::new(1.0_f32, Color32::from_white_alpha(14)))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(3.0, 3.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            ui.horizontal(|ui| add(ui)).inner
        })
        .inner
}

/// Text button inside a [`group`]. `active` lights it in `accent`.
pub(crate) fn seg_button(ui: &mut Ui, label: &str, active: bool, accent: Color32) -> Response {
    let font = FontId::proportional(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), TEXT);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 22.0, 26.0), Sense::click());
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 7.0, accent);
    } else if resp.is_pointer_button_down_on() {
        p.rect_filled(rect, 7.0, Color32::from_white_alpha(34));
    } else if resp.hovered() {
        p.rect_filled(rect, 7.0, Color32::from_white_alpha(20));
    }
    p.text(rect.center(), egui::Align2::CENTER_CENTER, label, font, if active { Color32::BLACK } else { TEXT });
    resp
}

/// Simple three dots button for Settings.
pub(crate) fn gear_button(ui: &mut Ui, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let p = ui.painter();
    if active {
        p.circle_filled(rect.center(), 16.0, ACCENT.gamma_multiply(0.45));
    } else if resp.hovered() {
        p.circle_filled(rect.center(), 16.0, Color32::from_white_alpha(22));
    }
    let c = rect.center();
    let col = if active || resp.hovered() { Color32::WHITE } else { Color32::from_rgb(205, 207, 216) };
    
    // Draw three horizontal dots (...)
    let r = 2.0_f32;
    p.circle_filled(c + vec2(-7.0, 0.0), r, col);
    p.circle_filled(c, r, col);
    p.circle_filled(c + vec2(7.0, 0.0), r, col);
    
    resp
}
