use egui::{Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, Vec2};

pub const BEOS_DESKTOP_BLUE: Color32 = Color32::from_rgb(51, 102, 152);
pub const BEOS_PANEL_GRAY: Color32 = Color32::from_rgb(216, 216, 216);
pub const BEOS_PANEL_LO: Color32 = Color32::from_rgb(196, 196, 196);
pub const BEOS_WIDGET_FACE: Color32 = Color32::from_rgb(232, 232, 232);
pub const BEOS_WELL_WHITE: Color32 = Color32::from_rgb(255, 255, 255);
pub const BEOS_TAB_YELLOW: Color32 = Color32::from_rgb(255, 203, 0);
pub const BEOS_TAB_INACTIVE: Color32 = Color32::from_rgb(230, 230, 230);
pub const BEOS_BEVEL_LIGHT: Color32 = Color32::from_rgb(255, 255, 255);
pub const BEOS_BEVEL_DARK: Color32 = Color32::from_rgb(122, 122, 122);
pub const BEOS_BORDER_BLACK: Color32 = Color32::from_rgb(40, 40, 40);
pub const BEOS_STATUS_GREEN: Color32 = Color32::from_rgb(46, 139, 87);
pub const BEOS_STATUS_RED: Color32 = Color32::from_rgb(178, 34, 34);
pub const BEOS_LED_GREEN: Color32 = Color32::from_rgb(41, 184, 77);
pub const BEOS_LED_GOLD: Color32 = Color32::from_rgb(255, 190, 0);
pub const BEOS_LED_RED: Color32 = Color32::from_rgb(220, 53, 69);
pub const TOPBAR_HEIGHT: f32 = 28.0;

pub fn apply_beos_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals.dark_mode = false;
        style.visuals.window_corner_radius = CornerRadius::ZERO;
        style.visuals.menu_corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.noninteractive.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.inactive.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.hovered.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.active.corner_radius = CornerRadius::ZERO;
        style.visuals.window_shadow = egui::Shadow::NONE;
        style.visuals.popup_shadow = egui::Shadow::NONE;
        style.visuals.panel_fill = BEOS_PANEL_GRAY;
        style.visuals.window_fill = BEOS_PANEL_GRAY;
        style.visuals.selection.bg_fill = BEOS_TAB_YELLOW;
        style.visuals.selection.stroke = Stroke::new(1.0, Color32::BLACK);
    });
}

/// Draws crisp 1px outer black outline + 1px inner 3D bevel highlight/shadow.
pub fn draw_beos_bevel(painter: &egui::Painter, rect: Rect, pressed: bool) {
    let (tl, br) = if pressed {
        (BEOS_BEVEL_DARK, BEOS_BEVEL_LIGHT)
    } else {
        (BEOS_BEVEL_LIGHT, BEOS_BEVEL_DARK)
    };
    let inner = rect.shrink(1.0);
    painter.line_segment([inner.left_top(), inner.right_top()], Stroke::new(1.0, tl));
    painter.line_segment([inner.left_top(), inner.left_bottom()], Stroke::new(1.0, tl));
    painter.line_segment(
        [inner.left_bottom(), inner.right_bottom()],
        Stroke::new(1.0, br),
    );
    painter.line_segment(
        [inner.right_top(), inner.right_bottom()],
        Stroke::new(1.0, br),
    );
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, BEOS_BORDER_BLACK),
        egui::StrokeKind::Inside,
    );
}

/// Tactile BeOS 3D beveled button.
pub fn be_button(ui: &mut Ui, label: &str) -> Response {
    let desired = Vec2::new((label.len() as f32 * 7.5 + 18.0).max(48.0), 22.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    if ui.is_rect_visible(rect) {
        let pressed = response.is_pointer_button_down_on();
        let fill = if pressed {
            Color32::from_rgb(198, 198, 198)
        } else if response.hovered() {
            Color32::from_rgb(242, 242, 242)
        } else {
            BEOS_WIDGET_FACE
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, fill);
        draw_beos_bevel(painter, rect, pressed);

        let text_offset = if pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
        painter.text(
            rect.center() + text_offset,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.5),
            Color32::BLACK,
        );
    }
    response
}

/// Toggleable BeOS 3D beveled button (stays visually depressed when active).
pub fn be_toggle_button(ui: &mut Ui, label: &str, active: bool) -> Response {
    let desired = Vec2::new((label.len() as f32 * 7.5 + 16.0).max(44.0), 22.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    if ui.is_rect_visible(rect) {
        let pressed = active || response.is_pointer_button_down_on();
        let fill = if active {
            BEOS_TAB_YELLOW
        } else if response.hovered() {
            Color32::from_rgb(242, 242, 242)
        } else {
            BEOS_WIDGET_FACE
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, fill);
        draw_beos_bevel(painter, rect, pressed);

        let text_offset = if pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
        painter.text(
            rect.center() + text_offset,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            Color32::BLACK,
        );
    }
    response
}
