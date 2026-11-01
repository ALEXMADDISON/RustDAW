use eframe::egui::{self, Color32, Rounding, Stroke};

// Modern dark palette: deep blue-tinted charcoal surfaces, soft indigo accent, pastel signal colours.
pub const BG: Color32 = Color32::from_rgb(12, 13, 17);
pub const PANEL: Color32 = Color32::from_rgb(20, 21, 27);
pub const CARD: Color32 = Color32::from_rgb(29, 31, 40);
pub const CARD_HI: Color32 = Color32::from_rgb(42, 45, 57);
pub const STROKE: Color32 = Color32::from_rgb(44, 47, 60);
pub const TEXT: Color32 = Color32::from_rgb(232, 234, 242);
pub const MUTED: Color32 = Color32::from_rgb(124, 130, 148);
pub const ACCENT: Color32 = Color32::from_rgb(110, 142, 255);
pub const GREEN: Color32 = Color32::from_rgb(52, 211, 153);
pub const YELLOW: Color32 = Color32::from_rgb(250, 204, 21);
pub const RED: Color32 = Color32::from_rgb(248, 95, 100);
#[allow(dead_code)]
pub const ORANGE: Color32 = Color32::from_rgb(251, 146, 60);
pub const LCD_BG: Color32 = Color32::from_rgb(8, 10, 15);
pub const LCD_FG: Color32 = Color32::from_rgb(140, 210, 255);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0_f32, STROKE);
    v.window_rounding = Rounding::same(14.0);
    v.menu_rounding = Rounding::same(10.0);
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.5);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;

    let r = Rounding::same(8.0);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, PANEL),
        (&mut v.widgets.inactive, CARD),
        (&mut v.widgets.hovered, CARD_HI),
        (&mut v.widgets.active, ACCENT),
        (&mut v.widgets.open, CARD_HI),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.rounding = r;
        w.bg_stroke = Stroke::new(1.0_f32, STROKE);
        w.fg_stroke = Stroke::new(1.0_f32, TEXT);
    }
    // Flat, borderless controls; borders only appear on hover.
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.55));
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 7.0);
        s.spacing.button_padding = egui::vec2(12.0, 5.0);
        s.spacing.interact_size = egui::vec2(40.0, 26.0);
        s.spacing.slider_width = 130.0;
        s.spacing.combo_width = 90.0;
        use egui::{FontId, TextStyle::*};
        s.text_styles.insert(Body, FontId::proportional(13.5));
        s.text_styles.insert(Button, FontId::proportional(13.0));
        s.text_styles.insert(Small, FontId::proportional(11.0));
        s.text_styles.insert(Heading, FontId::proportional(18.0));
        s.text_styles.insert(Monospace, FontId::monospace(13.0));
    });
}
