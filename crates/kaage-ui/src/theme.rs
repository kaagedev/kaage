use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Response,
    Sense, Stroke, TextStyle, Ui, Vec2,
};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

const IBM_PLEX_SANS_BYTES: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf");
const IBM_PLEX_MONO_BYTES: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontPreset {
    IbmPlex,
    IbmPlexMonoAll,
    EguiDefault,
}

impl FontPreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::IbmPlex => "IBM Plex Sans + Mono",
            Self::IbmPlexMonoAll => "IBM Plex Mono Everywhere",
            Self::EguiDefault => "egui Default",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UserSettings {
    pub theme_mode: ThemeMode,
    pub palette: ThemePalette,
    pub font_preset: FontPreset,
    pub ui_scale: f32,
    pub base_font_size: f32,
    pub terminal_font_size: f32,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::ObsidianAmber,
            palette: preset_palette(ThemeMode::ObsidianAmber),
            font_preset: FontPreset::IbmPlex,
            ui_scale: 1.0,
            base_font_size: 15.0,
            terminal_font_size: 16.0,
        }
    }
}

static USER_SETTINGS: OnceLock<RwLock<UserSettings>> = OnceLock::new();

fn settings_lock() -> &'static RwLock<UserSettings> {
    USER_SETTINGS.get_or_init(|| RwLock::new(UserSettings::default()))
}

pub fn user_settings() -> UserSettings {
    *settings_lock()
        .read()
        .unwrap_or_else(|e| e.into_inner())
}

pub fn terminal_font_size() -> f32 {
    user_settings().terminal_font_size.clamp(10.0, 22.0)
}

/// Register the IBM Plex font family according to the active `FontPreset`.
/// Preserves egui's built-in fallback fonts behind IBM Plex so symbols/glyphs always render.
pub fn configure_fonts_for_preset(ctx: &egui::Context, preset: FontPreset) {
    let mut fonts = FontDefinitions::default();
    match preset {
        FontPreset::IbmPlex => {
            fonts.font_data.insert(
                "ibm_plex_sans".to_owned(),
                Arc::new(FontData::from_static(IBM_PLEX_SANS_BYTES)),
            );
            fonts.font_data.insert(
                "ibm_plex_mono".to_owned(),
                Arc::new(FontData::from_static(IBM_PLEX_MONO_BYTES)),
            );
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "ibm_plex_sans".to_owned());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, "ibm_plex_mono".to_owned());
        }
        FontPreset::IbmPlexMonoAll => {
            fonts.font_data.insert(
                "ibm_plex_mono".to_owned(),
                Arc::new(FontData::from_static(IBM_PLEX_MONO_BYTES)),
            );
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "ibm_plex_mono".to_owned());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, "ibm_plex_mono".to_owned());
        }
        FontPreset::EguiDefault => {}
    }
    ctx.set_fonts(fonts);
}

pub fn configure_fonts(ctx: &egui::Context) {
    configure_fonts_for_preset(ctx, user_settings().font_preset);
}

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

pub fn preset_palette(mode: ThemeMode) -> ThemePalette {
    match mode {
        ThemeMode::ObsidianAmber => ThemePalette {
            is_dark: true,
            canvas_bg: Color32::from_rgb(13, 15, 18),       // #0D0F12
            dot_grid: Color32::from_rgb(38, 45, 56),        // subtle 24px dot-grid
            surface: Color32::from_rgb(22, 25, 30),         // #16191E
            surface_alt: Color32::from_rgb(33, 39, 49),     // #212731
            well: Color32::from_rgb(16, 18, 22),            // #101216
            widget: Color32::from_rgb(34, 40, 50),          // #222832
            widget_hover: Color32::from_rgb(48, 56, 70),    // #303846
            widget_pressed: Color32::from_rgb(24, 28, 36),  // #181C24
            tab_active_bg: Color32::from_rgb(36, 42, 54),   // #242A36
            tab_active_fg: Color32::from_rgb(255, 184, 56), // #FFB838 bright warm amber
            tab_inactive_bg: Color32::from_rgb(20, 23, 28),
            border: Color32::from_rgb(62, 72, 88),          // #3E4858 crisp structural border
            accent: Color32::from_rgb(255, 184, 56),        // #FFB838
            text: Color32::from_rgb(242, 245, 250),         // #F2F5FA high-contrast primary text
            text_muted: Color32::from_rgb(174, 185, 202),   // #AEB9CA >= 8:1 on well/surface
            status_green: Color32::from_rgb(74, 222, 128),  // #4ADE80
            status_red: Color32::from_rgb(248, 113, 113),   // #F87171
            status_amber: Color32::from_rgb(251, 191, 36),  // #FBBF24
            status_sky: Color32::from_rgb(96, 196, 255),    // #60C4FF
            section_bg: Color32::from_rgb(30, 35, 44),
            hover_row: Color32::from_rgb(42, 50, 66),
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
            accent: Color32::from_rgb(24, 78, 138),         // deep BeOS blue for readable text on light bg
            text: Color32::BLACK,
            text_muted: Color32::from_rgb(70, 76, 84),
            status_green: BEOS_STATUS_GREEN,
            status_red: BEOS_STATUS_RED,
            status_amber: Color32::from_rgb(156, 102, 0),
            status_sky: BEOS_DESKTOP_BLUE,
            section_bg: Color32::from_rgb(206, 212, 220),
            hover_row: Color32::from_rgb(207, 224, 245),
        },
    }
}

pub fn palette() -> ThemePalette {
    user_settings().palette
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
    let mut s = user_settings();
    s.theme_mode = mode;
    s.palette = preset_palette(mode);
    apply_user_settings(ctx, s, false);
}

pub fn apply_user_settings(ctx: &egui::Context, mut settings: UserSettings, reload_fonts: bool) {
    settings.ui_scale = settings.ui_scale.clamp(0.85, 1.40);
    settings.base_font_size = settings.base_font_size.clamp(10.0, 18.0);
    settings.terminal_font_size = settings.terminal_font_size.clamp(10.0, 22.0);
    ACTIVE_THEME.store(settings.theme_mode as u8, Ordering::Relaxed);
    {
        let mut guard = settings_lock()
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *guard = settings;
    }

    if reload_fonts {
        configure_fonts_for_preset(ctx, settings.font_preset);
    }
    ctx.set_zoom_factor(settings.ui_scale);

    let p = settings.palette;
    let base = settings.base_font_size;
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new((base - 2.0).max(9.0), FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Body,
            FontId::new(base, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(base, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(base + 4.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new((base - 0.5).max(9.5), FontFamily::Monospace),
        );

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
        style.visuals.widgets.noninteractive.bg_fill = p.surface;
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
        style.visuals.widgets.inactive.bg_fill = p.widget;
        style.visuals.widgets.inactive.weak_bg_fill = p.widget;
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
        style.visuals.widgets.hovered.bg_fill = p.widget_hover;
        style.visuals.widgets.hovered.weak_bg_fill = p.widget_hover;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
        style.visuals.widgets.active.bg_fill = p.widget_pressed;
        style.visuals.widgets.active.weak_bg_fill = p.widget_pressed;
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
        style.visuals.selection.bg_fill = if p.is_dark {
            Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 65)
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
