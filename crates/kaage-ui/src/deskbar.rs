use crate::clock::{ClockMode, format_now};
use crate::inspectors::{ScopeFilter, StateFilter};
use crate::theme::{
    BEOS_BORDER_BLACK, BEOS_LED_GOLD, BEOS_LED_GREEN, BEOS_LED_RED, BEOS_STATUS_GREEN,
    BEOS_TAB_YELLOW, TOPBAR_HEIGHT, current_theme_mode, draw_beos_bevel, palette,
};
use crate::wm::WORKSPACE_NAMES;
use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Vec2};
use kaage_proto::{DashboardSnapshot, HostMetrics, NixosInfo, SystemdUnitInfo, VitalsTick};

pub struct BeDeskbar {
    pub clock_mode: ClockMode,
}

pub enum DeskbarAction {
    OpenPalette,
    ToggleTracker,
    ToggleTheme,
    SwitchWorkspace(usize),
    OpenSystemdInspector {
        scope: ScopeFilter,
        state: StateFilter,
    },
    OpenVitals,
    OpenTailscale,
    OpenTailscaleServices,
    OpenHelp,
    OpenSettings,
}

impl BeDeskbar {
    pub fn new() -> Self {
        Self {
            clock_mode: ClockMode::DateTime,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        nixos: Option<&NixosInfo>,
        metrics: Option<&HostMetrics>,
        latest_vital: Option<&VitalsTick>,
        dashboard: Option<&DashboardSnapshot>,
        units: &[SystemdUnitInfo],
        connected: bool,
        tracker_visible: bool,
        active_workspace: usize,
    ) -> Option<DeskbarAction> {
        let p = palette();
        let mut action = None;
        let screen = ctx.content_rect();
        let bar_rect = Rect::from_min_size(screen.min, Vec2::new(screen.width(), TOPBAR_HEIGHT));

        egui::Area::new(Id::unique("kaage_topbar"))
            .fixed_pos(bar_rect.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let painter = ui.painter();
                painter.rect_filled(bar_rect, CornerRadius::ZERO, p.surface);
                painter.line_segment(
                    [bar_rect.left_bottom(), bar_rect.right_bottom()],
                    Stroke::new(1.0, p.border),
                );

                let mut left_x = bar_rect.min.x + 6.0;
                let btn_y = bar_rect.min.y + 3.0;
                let btn_h = TOPBAR_HEIGHT - 6.0;

                // 1) Crow "kaage ⇧⇧" Command HUD Trigger
                let cmd_rect = Rect::from_min_size(Pos2::new(left_x, btn_y), Vec2::new(96.0, btn_h));
                let cmd_resp = ui
                    .interact(cmd_rect, Id::unique("topbar_cmd_btn"), Sense::click())
                    .on_hover_text("Open Command Palette (Shift × 2)");
                painter.rect_filled(
                    cmd_rect,
                    CornerRadius::ZERO,
                    if cmd_resp.hovered() { p.widget_hover } else { p.widget },
                );
                draw_beos_bevel(painter, cmd_rect, false);
                draw_crow_emblem(
                    painter,
                    Pos2::new(cmd_rect.min.x + 13.0, cmd_rect.center().y),
                );
                painter.text(
                    Pos2::new(cmd_rect.min.x + 26.0, cmd_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "kaage",
                    egui::FontId::proportional(12.0),
                    p.text,
                );
                painter.text(
                    Pos2::new(cmd_rect.max.x - 6.0, cmd_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    "⇧⇧",
                    egui::FontId::monospace(10.0),
                    p.accent,
                );
                if cmd_resp.clicked() {
                    action = Some(DeskbarAction::OpenPalette);
                }
                left_x = cmd_rect.max.x + 6.0;

                // 2) Sidebar Nav Toggle
                let nav_rect = Rect::from_min_size(Pos2::new(left_x, btn_y), Vec2::new(48.0, btn_h));
                let nav_resp = ui
                    .interact(nav_rect, Id::unique("topbar_tracker_btn"), Sense::click())
                    .on_hover_text("Toggle Host Sidebar");
                painter.rect_filled(
                    nav_rect,
                    CornerRadius::ZERO,
                    if tracker_visible { p.widget_pressed } else { p.widget },
                );
                draw_beos_bevel(painter, nav_rect, tracker_visible);
                painter.text(
                    nav_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Nav",
                    egui::FontId::proportional(11.5),
                    if tracker_visible { p.accent } else { p.text },
                );
                if nav_resp.clicked() {
                    action = Some(DeskbarAction::ToggleTracker);
                }
                left_x = nav_rect.max.x + 8.0;

                // 3) Workspace Switcher Pills: [1: Ops] [2: AI] [3: Files] [4: Logs]
                for (ws_idx, ws_name) in WORKSPACE_NAMES.iter().enumerate() {
                    let w = (ws_name.len() as f32 * 6.5 + 14.0).max(52.0);
                    let r = Rect::from_min_size(Pos2::new(left_x, btn_y), Vec2::new(w, btn_h));
                    let active = ws_idx == active_workspace;
                    let resp = ui
                        .interact(r, Id::unique(("topbar_ws", ws_idx)), Sense::click())
                        .on_hover_text(format!("Switch to {ws_name} (Alt+{})", ws_idx + 1));
                    let fill = if active {
                        if p.is_dark {
                            Color32::from_rgb(46, 38, 22)
                        } else {
                            BEOS_TAB_YELLOW
                        }
                    } else if resp.hovered() {
                        p.widget_hover
                    } else {
                        p.widget
                    };
                    painter.rect_filled(r, CornerRadius::ZERO, fill);
                    draw_beos_bevel(painter, r, active);
                    if active && p.is_dark {
                        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
                    }
                    painter.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        *ws_name,
                        egui::FontId::monospace(10.5),
                        if active && p.is_dark { p.accent } else { p.text },
                    );
                    if resp.clicked() {
                        action = Some(DeskbarAction::SwitchWorkspace(ws_idx));
                    }
                    left_x = r.max.x + 4.0;
                }

                // --- RIGHT SIDE (right-to-left) ---
                let mut right_x = bar_rect.max.x - 6.0;

                // 1) Clickable Live DateTime Clock
                let clock_text = format_now(self.clock_mode);
                let clock_w = (clock_text.len() as f32 * 6.6 + 14.0).max(56.0);
                let clock_rect = Rect::from_min_size(
                    Pos2::new(right_x - clock_w, btn_y),
                    Vec2::new(clock_w, btn_h),
                );
                draw_tray_well(painter, clock_rect);
                painter.text(
                    clock_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &clock_text,
                    egui::FontId::monospace(10.5),
                    p.text,
                );
                if ui
                    .interact(clock_rect, Id::unique("topbar_clock"), Sense::click())
                    .on_hover_text("Click to cycle: HH:MM / HH:MM:SS / Date & Time")
                    .clicked()
                {
                    self.clock_mode = self.clock_mode.next();
                }
                right_x = clock_rect.min.x - 5.0;

                // 2) Theme Switcher Toggle (Obsidian / Retro BeOS)
                let theme_label = format!("◐ {}", current_theme_mode().label());
                let theme_w = (theme_label.len() as f32 * 6.2 + 12.0).max(66.0);
                if right_x - theme_w > left_x + 20.0 {
                    let theme_rect = Rect::from_min_size(
                        Pos2::new(right_x - theme_w, btn_y),
                        Vec2::new(theme_w, btn_h),
                    );
                    let resp = ui
                        .interact(theme_rect, Id::unique("topbar_theme"), Sense::click())
                        .on_hover_text("Switch between Obsidian Amber (Dark) and Retro BeOS (Light)");
                    painter.rect_filled(
                        theme_rect,
                        CornerRadius::ZERO,
                        if resp.hovered() { p.widget_hover } else { p.widget },
                    );
                    draw_beos_bevel(painter, theme_rect, false);
                    painter.text(
                        theme_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        &theme_label,
                        egui::FontId::proportional(10.5),
                        p.text,
                    );
                    if resp.clicked() {
                        action = Some(DeskbarAction::ToggleTheme);
                    }
                    right_x = theme_rect.min.x - 5.0;
                }

                // 2b) "? Help" button linking to kaage-help
                let help_w = 54.0;
                if right_x - help_w > left_x + 20.0 {
                    let help_rect = Rect::from_min_size(
                        Pos2::new(right_x - help_w, btn_y),
                        Vec2::new(help_w, btn_h),
                    );
                    let resp = ui
                        .interact(help_rect, Id::unique("topbar_help"), Sense::click())
                        .on_hover_text("Open kaage-help (Browser/Vim keys, Shortcuts & APIs)");
                    painter.rect_filled(
                        help_rect,
                        CornerRadius::ZERO,
                        if resp.hovered() { p.widget_hover } else { p.widget },
                    );
                    draw_beos_bevel(painter, help_rect, false);
                    painter.text(
                        Pos2::new(help_rect.min.x + 8.0, help_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        "?",
                        egui::FontId::monospace(11.5),
                        p.accent,
                    );
                    painter.text(
                        Pos2::new(help_rect.min.x + 19.0, help_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        "Help",
                        egui::FontId::proportional(11.0),
                        p.text,
                    );
                    if resp.clicked() {
                        action = Some(DeskbarAction::OpenHelp);
                    }
                    right_x = help_rect.min.x - 5.0;
                }

                // 2c) "⚙ Settings" button linking to kaage-settings
                let cfg_w = 68.0;
                if right_x - cfg_w > left_x + 20.0 {
                    let cfg_rect = Rect::from_min_size(
                        Pos2::new(right_x - cfg_w, btn_y),
                        Vec2::new(cfg_w, btn_h),
                    );
                    let resp = ui
                        .interact(cfg_rect, Id::unique("topbar_settings"), Sense::click())
                        .on_hover_text("Open kaage-settings (Typography, Font Size & Color Theme Editor)");
                    painter.rect_filled(
                        cfg_rect,
                        CornerRadius::ZERO,
                        if resp.hovered() { p.widget_hover } else { p.widget },
                    );
                    draw_beos_bevel(painter, cfg_rect, false);
                    painter.text(
                        Pos2::new(cfg_rect.min.x + 7.0, cfg_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        "⚙",
                        egui::FontId::proportional(11.0),
                        p.accent,
                    );
                    painter.text(
                        Pos2::new(cfg_rect.min.x + 19.0, cfg_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        "Settings",
                        egui::FontId::proportional(11.0),
                        p.text,
                    );
                    if resp.clicked() {
                        action = Some(DeskbarAction::OpenSettings);
                    }
                    right_x = cfg_rect.min.x - 5.0;
                }

                // 3) Live 3-bar CPU / RAM / GPU Sparkbars + WS LED
                let meters_w = 114.0;
                if right_x - meters_w > left_x + 20.0 {
                    let meters_rect = Rect::from_min_size(
                        Pos2::new(right_x - meters_w, btn_y),
                        Vec2::new(meters_w, btn_h),
                    );
                    draw_tray_well(painter, meters_rect);
                    draw_beos_led(
                        painter,
                        Pos2::new(meters_rect.min.x + 5.0, meters_rect.center().y - 4.0),
                        if connected { BEOS_LED_GREEN } else { BEOS_LED_RED },
                    );

                    if let Some(m) = metrics {
                        let cpu_frac = (m.cpu_percent / 100.0).clamp(0.0, 1.0);
                        let mem_frac = if m.mem_total_mb > 0 {
                            (m.mem_used_mb as f32 / m.mem_total_mb as f32).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let gpu_frac = latest_vital
                            .map(|v| {
                                if let Some(u) = v.gpu_util_percent {
                                    (u / 100.0).clamp(0.0, 1.0)
                                } else if v.gpu_vram_total_mb > 0 {
                                    (v.gpu_vram_used_mb as f32 / v.gpu_vram_total_mb as f32).clamp(0.0, 1.0)
                                } else {
                                    0.0
                                }
                            })
                            .unwrap_or(0.0);
                        let bar_x = meters_rect.min.x + 17.0;
                        let bar_w = 54.0;
                        let bg_bar = Color32::from_rgb(52, 58, 68);
                        for (dy, frac, col) in [
                            (3.0, cpu_frac, BEOS_STATUS_GREEN),
                            (9.0, mem_frac, p.accent),
                            (15.0, gpu_frac, p.status_sky),
                        ] {
                            let r = Rect::from_min_size(Pos2::new(bar_x, meters_rect.min.y + dy), Vec2::new(bar_w, 4.0));
                            painter.rect_filled(r, CornerRadius::ZERO, bg_bar);
                            painter.rect_filled(
                                Rect::from_min_size(r.min, Vec2::new(bar_w * frac, 4.0)),
                                CornerRadius::ZERO,
                                col,
                            );
                        }
                        painter.text(
                            Pos2::new(meters_rect.max.x - 5.0, meters_rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!("{:.0}%", m.cpu_percent),
                            egui::FontId::monospace(10.0),
                            p.text,
                        );
                        if ui
                            .interact(meters_rect, Id::unique("topbar_metrics"), Sense::click())
                            .on_hover_text(format!(
                                "{}\nCPU: {:.1}% · RAM: {}/{} MiB · Uptime: {}\n(Click to open Vitals)",
                                m.hostname,
                                m.cpu_percent,
                                m.mem_used_mb,
                                m.mem_total_mb,
                                format_uptime(m.uptime_secs),
                            ))
                            .clicked()
                        {
                            action = Some(DeskbarAction::OpenVitals);
                        }
                    } else {
                        painter.text(
                            Pos2::new(meters_rect.max.x - 6.0, meters_rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            if connected { "sync..." } else { "offline" },
                            egui::FontId::monospace(10.0),
                            p.text_muted,
                        );
                    }
                    right_x = meters_rect.min.x - 5.0;
                }

                // 4) Services LED Pill
                let running_units = units.iter().filter(|u| u.running).count();
                let failed_units = units.iter().filter(|u| u.active_state == "failed").count();
                let svc_label = format!("Svc {running_units}/{}", units.len());
                let svc_w = (svc_label.len() as f32 * 6.2 + 22.0).max(74.0);
                if right_x - svc_w > left_x + 20.0 {
                    let svc_rect = Rect::from_min_size(
                        Pos2::new(right_x - svc_w, btn_y),
                        Vec2::new(svc_w, btn_h),
                    );
                    draw_tray_well(painter, svc_rect);
                    let led = if failed_units > 0 {
                        BEOS_LED_RED
                    } else if running_units > 0 {
                        BEOS_LED_GREEN
                    } else {
                        BEOS_LED_GOLD
                    };
                    draw_beos_led(
                        painter,
                        Pos2::new(svc_rect.min.x + 5.0, svc_rect.center().y - 4.0),
                        led,
                    );
                    painter.text(
                        Pos2::new(svc_rect.min.x + 17.0, svc_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &svc_label,
                        egui::FontId::monospace(10.5),
                        p.text,
                    );
                    if ui
                        .interact(svc_rect, Id::unique("topbar_svc_pill"), Sense::click())
                        .clicked()
                    {
                        action = Some(DeskbarAction::OpenSystemdInspector {
                            scope: ScopeFilter::All,
                            state: if failed_units > 0 {
                                StateFilter::Failed
                            } else {
                                StateFilter::Running
                            },
                        });
                    }
                    right_x = svc_rect.min.x - 5.0;
                }

                // 5) Tailscale Node Identity Pill (with NixOS gen tooltip)
                let ts_dns = dashboard
                    .filter(|d| d.tailscale.available && !d.tailscale.dns_name.is_empty())
                    .map(|d| d.tailscale.dns_name.as_str())
                    .or_else(|| metrics.map(|m| m.hostname.as_str()));
                if let Some(node_label) = ts_dns {
                    let short: String = node_label.chars().take(24).collect();
                    let w = (short.len() as f32 * 6.2 + 22.0).max(72.0);
                    if right_x - w > left_x + 20.0 {
                        let r = Rect::from_min_size(
                            Pos2::new(right_x - w, btn_y),
                            Vec2::new(w, btn_h),
                        );
                        draw_tray_well(painter, r);
                        draw_beos_led(
                            painter,
                            Pos2::new(r.min.x + 5.0, r.center().y - 4.0),
                            p.status_sky,
                        );
                        painter.text(
                            Pos2::new(r.min.x + 17.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            &short,
                            egui::FontId::monospace(10.5),
                            p.status_sky,
                        );
                        let nix_tip = nixos
                            .filter(|n| n.is_nixos)
                            .map(|n| format!("{} (gen #{:?})", n.pretty_name, n.generation))
                            .unwrap_or_default();
                        if ui
                            .interact(r, Id::unique("topbar_ts_node"), Sense::click())
                            .on_hover_text(format!("Tailscale Node: {node_label}\n{nix_tip}\n(Click to inspect Tailscale)"))
                            .clicked()
                        {
                            action = Some(DeskbarAction::OpenTailscale);
                        }
                        right_x = r.min.x - 5.0;
                    }
                }

                // --- MIDDLE: Active Port Badges (:8188, :8765, :8888, :9090) ---
                if let Some(dash) = dashboard {
                    let mut px = left_x + 8.0;
                    for port in dash
                        .listening_ports
                        .iter()
                        .filter(|p| p.tailscale_url.is_some())
                        .take(4)
                    {
                        let label = format!(":{}", port.port);
                        let w = (label.len() as f32 * 6.4 + 10.0).max(38.0);
                        if px + w > right_x - 8.0 {
                            break;
                        }
                        let r = Rect::from_min_size(Pos2::new(px, btn_y + 1.0), Vec2::new(w, btn_h - 2.0));
                        draw_tray_well(painter, r);
                        painter.text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            &label,
                            egui::FontId::monospace(10.0),
                            p.status_green,
                        );
                        if ui
                            .interact(r, Id::unique(("topbar_port", port.port)), Sense::click())
                            .on_hover_text(format!(
                                "{} ({})\nClick to open Tailscale Services",
                                port.label,
                                port.tailscale_url.as_deref().unwrap_or("")
                            ))
                            .clicked()
                        {
                            action = Some(DeskbarAction::OpenTailscaleServices);
                        }
                        px = r.max.x + 4.0;
                    }
                }
            });

        action
    }
}

fn draw_tray_well(painter: &egui::Painter, rect: Rect) {
    let p = palette();
    painter.rect_filled(rect, CornerRadius::ZERO, p.well);
    draw_beos_bevel(painter, rect, true);
}

fn draw_beos_led(painter: &egui::Painter, top_left: Pos2, color: Color32) {
    let rect = Rect::from_min_size(top_left, Vec2::splat(8.0));
    painter.rect_filled(rect, CornerRadius::ZERO, color);
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, BEOS_BORDER_BLACK),
        egui::StrokeKind::Inside,
    );
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

/// Crisp geometric Crow silhouette emblem (Kannada: kāge)
fn draw_crow_emblem(painter: &egui::Painter, center: Pos2) {
    let p = palette();
    let body = vec![
        Pos2::new(center.x - 6.0, center.y + 4.0),
        Pos2::new(center.x - 2.0, center.y - 5.0),
        Pos2::new(center.x + 3.0, center.y - 5.0),
        Pos2::new(center.x + 7.0, center.y - 2.0),
        Pos2::new(center.x + 3.0, center.y),
        Pos2::new(center.x + 1.0, center.y + 5.0),
    ];
    let fill = if p.is_dark { p.text } else { BEOS_BORDER_BLACK };
    painter.add(egui::Shape::convex_polygon(body, fill, Stroke::NONE));
    painter.circle_filled(Pos2::new(center.x + 2.0, center.y - 3.0), 1.3, p.accent);
}
