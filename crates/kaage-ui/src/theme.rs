use egui::{Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};
use std::sync::atomic::{AtomicU8, Ordering};

// Legacy BeOS constants kept for compatibility & Retro BeOS mode
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    ObsidianAmber = 0,
    RetroBeos = 1,
}

impl ThemeMode {
    pub fn toggle(self) -> Self {
        match self {
            Self::ObsidianAmber => Self::RetroBeos,
            Self::RetroBeos => Self::ObsidianAmber,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ObsidianAmber => "Obsidian",
            Self::RetroBeos => "BeOS",
        }
    }
}

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(ThemeMode::ObsidianAmber as u8);

pub fn current_theme_mode() -> ThemeMode {
    if ACTIVE_THEME.load(Ordering::Relaxed) == ThemeMode::RetroBeos as u8 {
        ThemeMode::RetroBeos
    } else {
        ThemeMode::ObsidianAmber
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub is_dark: bool,
    pub canvas_bg: Color32,
    pub dot_grid: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub well: Color32,
    pub widget: Color32,
    pub widget_hover: Color32,
    pub widget_pressed: Color32,
    pub tab_active_bg: Color32,
    pub tab_active_fg: Color32,
    pub tab_inactive_bg: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub status_green: Color32,
    pub status_red: Color32,
    pub status_amber: Color32,
    pub status_sky: Color32,
    pub section_bg: Color32,
    pub hover_row: Color32,
}

pub fn palette() -> ThemePalette {
    match current_theme_mode() {
        ThemeMode::ObsidianAmber => ThemePalette {
            is_dark: true,
            canvas_bg: Color32::from_rgb(13, 15, 18),      // #0D0F12
            dot_grid: Color32::from_rgb(34, 40, 50),       // subtle 24px dot-grid
            surface: Color32::from_rgb(22, 25, 30),        // #16191E
            surface_alt: Color32::from_rgb(31, 36, 45),    // #1F242D
            well: Color32::from_rgb(18, 20, 24),           // #121418
            widget: Color32::from_rgb(31, 36, 45),         // #1F242D
            widget_hover: Color32::from_rgb(42, 49, 61),   // #2A313D
            widget_pressed: Color32::from_rgb(24, 28, 36), // #181C24
            tab_active_bg: Color32::from_rgb(31, 36, 45),  // #1F242D
            tab_active_fg: Color32::from_rgb(245, 166, 35),// #F5A623 warm amber
            tab_inactive_bg: Color32::from_rgb(20, 23, 28),
            border: Color32::from_rgb(42, 47, 58),         // #2A2F3A
            accent: Color32::from_rgb(245, 166, 35),       // #F5A623
            text: Color32::from_rgb(230, 234, 240),
            text_muted: Color32::from_rgb(139, 149, 165),
            status_green: Color32::from_rgb(34, 197, 94),  // #22C55E
            status_red: Color32::from_rgb(239, 68, 68),    // #EF4444
            status_amber: Color32::from_rgb(234, 179, 8),  // #EAB308
            status_sky: Color32::from_rgb(56, 189, 248),   // #38BDF8
            section_bg: Color32::from_rgb(28, 32, 40),
            hover_row: Color32::from_rgb(36, 44, 58),
        },
        ThemeMode::RetroBeos => ThemePalette {
            is_dark: false,
            canvas_bg: BEOS_DESKTOP_BLUE,
            dot_grid: Color32::from_rgba_unmultiplied(255, 255, 255, 16),
            surface: BEOS_PANEL_GRAY,
            surface_alt: BEOS_WIDGET_FACE,
            well: BEOS_WELL_WHITE,
            widget: BEOS_WIDGET_FACE,
            widget_hover: Color32::from_rgb(242, 242, 242),
            widget_pressed: BEOS_PANEL_LO,
            tab_active_bg: BEOS_TAB_YELLOW,
            tab_active_fg: Color32::BLACK,
            tab_inactive_bg: BEOS_TAB_INACTIVE,
            border: BEOS_BORDER_BLACK,
            accent: BEOS_TAB_YELLOW,
            text: Color32::BLACK,
            text_muted: Color32::from_rgb(75, 80, 88),
            status_green: BEOS_STATUS_GREEN,
            status_red: BEOS_STATUS_RED,
            status_amber: Color32::from_rgb(184, 134, 11),
            status_sky: BEOS_DESKTOP_BLUE,
            section_bg: Color32::from_rgb(212, 216, 222),
            hover_row: Color32::from_rgb(207, 224, 245),
        },
    }
}

pub fn text_color() -> Color32 {
    palette().text
}

pub fn muted_text_color() -> Color32 {
    palette().text_muted
}

pub fn card_fill() -> Color32 {
    palette().well
}

pub fn apply_theme(ctx: &egui::Context, mode: ThemeMode) {
    ACTIVE_THEME.store(mode as u8, Ordering::Relaxed);
    let p = palette();
    ctx.all_styles_mut(|style| {
        style.visuals = if p.is_dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        style.visuals.window_corner_radius = CornerRadius::ZERO;
        style.visuals.menu_corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.noninteractive.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.inactive.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.hovered.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.active.corner_radius = CornerRadius::ZERO;
        style.visuals.window_shadow = egui::Shadow::NONE;
        style.visuals.popup_shadow = egui::Shadow::NONE;
        style.visuals.panel_fill = p.surface;
        style.visuals.window_fill = p.surface;
        style.visuals.extreme_bg_color = p.well;
        style.visuals.override_text_color = Some(p.text);
        style.visuals.selection.bg_fill = if p.is_dark {
            Color32::from_rgba_unmultiplied(245, 166, 35, 55)
        } else {
            BEOS_TAB_YELLOW
        };
        style.visuals.selection.stroke = Stroke::new(1.0, p.accent);
    });
}

/// Draw crisp structural border: hairline 1px in Obsidian mode, classic 3D bevel in Retro BeOS mode.
pub fn draw_beos_bevel(painter: &egui::Painter, rect: Rect, pressed: bool) {
    let p = palette();
    if p.is_dark {
        let stroke_col = if pressed {
            Color32::from_rgb(30, 34, 42)
        } else {
            p.border
        };
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0, stroke_col),
            egui::StrokeKind::Inside,
        );
    } else {
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
}

/// Draw subtle 24px obsidian dot-grid on the desktop canvas.
pub fn draw_dot_grid(painter: &egui::Painter, rect: Rect) {
    let p = palette();
    if !p.is_dark {
        return;
    }
    let step = 24.0;
    let mut y = rect.min.y + 12.0;
    while y < rect.max.y {
        let mut x = rect.min.x + 12.0;
        while x < rect.max.x {
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(x, y), Vec2::splat(1.2)),
                CornerRadius::ZERO,
                p.dot_grid,
            );
            x += step;
        }
        y += step;
    }
}

/// Tactile Neo-Brutalist / BeOS button.
pub fn be_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette();
    let desired = Vec2::new((label.len() as f32 * 7.2 + 16.0).max(46.0), 22.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    if ui.is_rect_visible(rect) {
        let pressed = response.is_pointer_button_down_on();
        let fill = if pressed {
            p.widget_pressed
        } else if response.hovered() {
            p.widget_hover
        } else {
            p.widget
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, fill);
        draw_beos_bevel(painter, rect, pressed);
        if p.is_dark && response.hovered() {
            painter.rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }

        let text_offset = if pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
        painter.text(
            rect.center() + text_offset,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            p.text,
        );
    }
    response
}

/// Toggleable Neo-Brutalist / BeOS button (warm amber accent when active).
pub fn be_toggle_button(ui: &mut Ui, label: &str, active: bool) -> Response {
    let p = palette();
    let desired = Vec2::new((label.len() as f32 * 7.2 + 14.0).max(42.0), 22.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    if ui.is_rect_visible(rect) {
        let pressed = active || response.is_pointer_button_down_on();
        let fill = if active {
            if p.is_dark {
                Color32::from_rgb(46, 38, 22)
            } else {
                BEOS_TAB_YELLOW
            }
        } else if response.hovered() {
            p.widget_hover
        } else {
            p.widget
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, fill);
        draw_beos_bevel(painter, rect, pressed);
        if active && p.is_dark {
            painter.rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }

        let text_offset = if pressed && !p.is_dark {
            Vec2::splat(1.0)
        } else {
            Vec2::ZERO
        };
        let fg = if active && p.is_dark { p.accent } else { p.text };
        painter.text(
            rect.center() + text_offset,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.5),
            fg,
        );
    }
    response
}
