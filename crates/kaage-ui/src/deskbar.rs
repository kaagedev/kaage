use crate::clock::{ClockMode, format_now};
use crate::inspectors::{ScopeFilter, StateFilter};
use crate::theme::{
    BEOS_BEVEL_DARK, BEOS_BEVEL_LIGHT, BEOS_BORDER_BLACK, BEOS_LED_GOLD, BEOS_LED_GREEN,
    BEOS_LED_RED, BEOS_PANEL_GRAY, BEOS_PANEL_LO, BEOS_STATUS_GREEN, BEOS_TAB_YELLOW,
    BEOS_WIDGET_FACE, TOPBAR_HEIGHT, draw_beos_bevel,
};
use crate::wm::{BeWindowGroup, PaneKind};
use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Vec2};
use kaage_proto::{HostMetrics, NixosInfo, SystemdUnitInfo, VitalsTick};

pub struct BeDeskbar {
    pub menu_open: bool,
    pub clock_mode: ClockMode,
}

pub enum DeskbarAction {
    ToggleTracker,
    TileWindows,
    NewTerminal,
    OpenSystemdInspector {
        scope: ScopeFilter,
        state: StateFilter,
    },
    OpenDockerInspector,
    OpenSystemMonitor,
    OpenVitals,
    OpenBottom,
    OpenAiJobs,
    OpenHttpPorts,
    OpenTailscale,
    OpenFileTracker,
    FocusWindow {
        group_id: u64,
        tab_idx: usize,
    },
}

impl BeDeskbar {
    pub fn new() -> Self {
        Self {
            menu_open: false,
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
        units: &[SystemdUnitInfo],
        connected: bool,
        tracker_visible: bool,
        groups: &[BeWindowGroup],
    ) -> Option<DeskbarAction> {
        let mut action = None;
        let screen = ctx.content_rect();
        let bar_rect = Rect::from_min_size(screen.min, Vec2::new(screen.width(), TOPBAR_HEIGHT));

        egui::Area::new(Id::unique("kaage_topbar"))
            .fixed_pos(bar_rect.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let painter = ui.painter();
                painter.rect_filled(bar_rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                painter.line_segment(
                    [bar_rect.left_top(), bar_rect.right_top()],
                    Stroke::new(1.0, BEOS_BEVEL_LIGHT),
                );
                painter.line_segment(
                    [
                        Pos2::new(bar_rect.min.x, bar_rect.max.y - 2.0),
                        Pos2::new(bar_rect.max.x, bar_rect.max.y - 2.0),
                    ],
                    Stroke::new(1.0, BEOS_BEVEL_DARK),
                );
                painter.line_segment(
                    [bar_rect.left_bottom(), bar_rect.right_bottom()],
                    Stroke::new(1.0, BEOS_BORDER_BLACK),
                );

                // --- LEFT SIDE: Crow Menu + Tracker Toggle + Running Window Buttons ---
                let mut left_x = bar_rect.min.x + 6.0;
                let btn_y = bar_rect.min.y + 3.0;
                let btn_h = TOPBAR_HEIGHT - 6.0;

                // 1) Crow "kaage" Menu Button
                let menu_btn_rect = Rect::from_min_size(Pos2::new(left_x, btn_y), Vec2::new(76.0, btn_h));
                let menu_resp =
                    ui.interact(menu_btn_rect, Id::unique("topbar_crow_btn"), Sense::click());
                if menu_resp.clicked() {
                    self.menu_open = !self.menu_open;
                }
                let menu_fill = if self.menu_open {
                    BEOS_PANEL_LO
                } else if menu_resp.hovered() {
                    Color32::from_rgb(240, 240, 240)
                } else {
                    BEOS_WIDGET_FACE
                };
                painter.rect_filled(menu_btn_rect, CornerRadius::ZERO, menu_fill);
                draw_beos_bevel(painter, menu_btn_rect, self.menu_open);
                draw_crow_emblem(
                    painter,
                    Pos2::new(menu_btn_rect.min.x + 14.0, menu_btn_rect.center().y),
                );
                painter.text(
                    Pos2::new(menu_btn_rect.min.x + 28.0, menu_btn_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "kaage",
                    egui::FontId::proportional(12.5),
                    Color32::BLACK,
                );
                left_x = menu_btn_rect.max.x + 6.0;

                // 2) Tracker Toggle Button (acedop-style with BeOS folder icon)
                let tracker_rect = Rect::from_min_size(Pos2::new(left_x, btn_y), Vec2::new(76.0, btn_h));
                let tracker_resp = ui
                    .interact(tracker_rect, Id::unique("topbar_tracker_btn"), Sense::click())
                    .on_hover_text("Show / Hide Tracker sidebar");
                let tracker_fill = if tracker_visible {
                    BEOS_PANEL_LO
                } else if tracker_resp.hovered() {
                    Color32::from_rgb(240, 240, 240)
                } else {
                    BEOS_WIDGET_FACE
                };
                painter.rect_filled(tracker_rect, CornerRadius::ZERO, tracker_fill);
                draw_beos_bevel(painter, tracker_rect, tracker_visible);
                draw_mini_folder(
                    painter,
                    Pos2::new(tracker_rect.min.x + 7.0, tracker_rect.center().y - 5.0),
                );
                painter.text(
                    Pos2::new(tracker_rect.min.x + 23.0, tracker_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Tracker",
                    egui::FontId::proportional(12.0),
                    Color32::BLACK,
                );
                if tracker_resp.clicked() {
                    self.menu_open = false;
                    action = Some(DeskbarAction::ToggleTracker);
                }
                left_x = tracker_rect.max.x + 6.0;

                // --- RIGHT SIDE (right-to-left): Clock, CPU/RAM, Services LED, NixOS LED, WS LED ---
                let mut right_x = bar_rect.max.x - 6.0;

                // 1) Clickable Live DateTime Clock
                let clock_text = format_now(self.clock_mode);
                let clock_w = (clock_text.len() as f32 * 6.8 + 16.0).max(58.0);
                let clock_rect = Rect::from_min_size(
                    Pos2::new(right_x - clock_w, btn_y),
                    Vec2::new(clock_w, btn_h),
                );
                draw_recessed_box(painter, clock_rect, Color32::from_rgb(207, 207, 207));
                painter.text(
                    clock_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &clock_text,
                    egui::FontId::monospace(11.0),
                    Color32::BLACK,
                );
                if ui
                    .interact(clock_rect, Id::unique("topbar_clock"), Sense::click())
                    .on_hover_text("Click to cycle: HH:MM / HH:MM:SS / Date & Time")
                    .clicked()
                {
                    self.clock_mode = self.clock_mode.next();
                }
                right_x = clock_rect.min.x - 6.0;

                // 2) Live CPU / RAM Mini-Meters Tray
                let meters_w = 114.0;
                if right_x - meters_w > left_x + 40.0 {
                    let meters_rect = Rect::from_min_size(
                        Pos2::new(right_x - meters_w, btn_y),
                        Vec2::new(meters_w, btn_h),
                    );
                    draw_recessed_box(painter, meters_rect, Color32::from_rgb(210, 210, 210));
                    let ws_led_color = if connected {
                        BEOS_LED_GREEN
                    } else {
                        BEOS_LED_RED
                    };
                    draw_beos_led(
                        painter,
                        Pos2::new(meters_rect.min.x + 5.0, meters_rect.center().y - 4.0),
                        ws_led_color,
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
                                    (v.gpu_vram_used_mb as f32 / v.gpu_vram_total_mb as f32)
                                        .clamp(0.0, 1.0)
                                } else {
                                    0.0
                                }
                            })
                            .unwrap_or(0.0);
                        let bar_x = meters_rect.min.x + 17.0;
                        let bar_w = 54.0;
                        let cpu_bar = Rect::from_min_size(
                            Pos2::new(bar_x, meters_rect.min.y + 3.0),
                            Vec2::new(bar_w, 4.0),
                        );
                        painter.rect_filled(cpu_bar, CornerRadius::ZERO, Color32::from_rgb(70, 70, 70));
                        painter.rect_filled(
                            Rect::from_min_size(cpu_bar.min, Vec2::new(bar_w * cpu_frac, 4.0)),
                            CornerRadius::ZERO,
                            BEOS_STATUS_GREEN,
                        );
                        let mem_bar = Rect::from_min_size(
                            Pos2::new(bar_x, meters_rect.min.y + 9.0),
                            Vec2::new(bar_w, 4.0),
                        );
                        painter.rect_filled(mem_bar, CornerRadius::ZERO, Color32::from_rgb(70, 70, 70));
                        painter.rect_filled(
                            Rect::from_min_size(mem_bar.min, Vec2::new(bar_w * mem_frac, 4.0)),
                            CornerRadius::ZERO,
                            BEOS_TAB_YELLOW,
                        );
                        let gpu_bar = Rect::from_min_size(
                            Pos2::new(bar_x, meters_rect.min.y + 15.0),
                            Vec2::new(bar_w, 4.0),
                        );
                        painter.rect_filled(gpu_bar, CornerRadius::ZERO, Color32::from_rgb(70, 70, 70));
                        painter.rect_filled(
                            Rect::from_min_size(gpu_bar.min, Vec2::new(bar_w * gpu_frac, 4.0)),
                            CornerRadius::ZERO,
                            Color32::from_rgb(76, 168, 240),
                        );
                        painter.text(
                            Pos2::new(meters_rect.max.x - 5.0, meters_rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!("{:.0}%", m.cpu_percent),
                            egui::FontId::monospace(10.5),
                            Color32::BLACK,
                        );
                        let gpu_tip = latest_vital
                            .map(|v| {
                                format!(
                                    "\nGPU VRAM: {} / {} MiB",
                                    v.gpu_vram_used_mb, v.gpu_vram_total_mb
                                )
                            })
                            .unwrap_or_default();
                        if ui
                            .interact(meters_rect, Id::unique("topbar_metrics"), Sense::click())
                            .on_hover_text(format!(
                                "{}\nCPU: {:.1}%\nRAM: {} / {} MiB{gpu_tip}\nUptime: {}\n(Click to open Vitals)",
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
                            egui::FontId::monospace(10.5),
                            Color32::from_rgb(60, 60, 60),
                        );
                    }
                    right_x = meters_rect.min.x - 6.0;
                }

                // 3) Services LED Tray Pill
                let running_units = units.iter().filter(|u| u.running).count();
                let failed_units = units.iter().filter(|u| u.active_state == "failed").count();
                let svc_label = format!("Svc: {running_units}/{}", units.len());
                let svc_w = (svc_label.len() as f32 * 6.5 + 24.0).max(80.0);
                if right_x - svc_w > left_x + 40.0 {
                    let svc_rect = Rect::from_min_size(
                        Pos2::new(right_x - svc_w, btn_y),
                        Vec2::new(svc_w, btn_h),
                    );
                    draw_recessed_box(painter, svc_rect, Color32::from_rgb(210, 210, 210));
                    let led_col = if failed_units > 0 {
                        BEOS_LED_RED
                    } else if running_units > 0 {
                        BEOS_LED_GREEN
                    } else {
                        BEOS_LED_GOLD
                    };
                    draw_beos_led(
                        painter,
                        Pos2::new(svc_rect.min.x + 5.0, svc_rect.center().y - 4.0),
                        led_col,
                    );
                    painter.text(
                        Pos2::new(svc_rect.min.x + 18.0, svc_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &svc_label,
                        egui::FontId::proportional(11.0),
                        Color32::BLACK,
                    );
                    if ui
                        .interact(svc_rect, Id::unique("topbar_svc_pill"), Sense::click())
                        .on_hover_text(format!(
                            "Systemd Services: {running_units} running, {failed_units} failed (click to inspect)"
                        ))
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
                    right_x = svc_rect.min.x - 6.0;
                }

                // 4) NixOS LED Tray Pill
                if let Some(info) = nixos
                    && info.is_nixos
                {
                    let nix_label = info
                        .generation
                        .map(|g| format!("NixOS #{g}"))
                        .unwrap_or_else(|| "NixOS".into());
                    let nix_w = (nix_label.len() as f32 * 6.6 + 24.0).max(74.0);
                    if right_x - nix_w > left_x + 40.0 {
                        let nix_rect = Rect::from_min_size(
                            Pos2::new(right_x - nix_w, btn_y),
                            Vec2::new(nix_w, btn_h),
                        );
                        draw_recessed_box(painter, nix_rect, Color32::from_rgb(210, 210, 210));
                        draw_beos_led(
                            painter,
                            Pos2::new(nix_rect.min.x + 5.0, nix_rect.center().y - 4.0),
                            BEOS_LED_GREEN,
                        );
                        painter.text(
                            Pos2::new(nix_rect.min.x + 18.0, nix_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &nix_label,
                            egui::FontId::proportional(11.0),
                            Color32::BLACK,
                        );
                        let sys_path = info.current_system.as_deref().unwrap_or("n/a");
                        ui.interact(nix_rect, Id::unique("topbar_nixos_pill"), Sense::hover())
                            .on_hover_text(format!("{}\nSystem: {sys_path}", info.pretty_name));
                        right_x = nix_rect.min.x - 6.0;
                    }
                }

                // --- MIDDLE STRIP: Running Window Buttons (fill space between left_x and right_x) ---
                let mut win_x = left_x + 4.0;
                for group in groups {
                    for (idx, tab) in group.tabs.iter().enumerate() {
                        let badge = match tab.kind {
                            PaneKind::Terminal(_) => ">_",
                            PaneKind::SystemdInspector { .. } => "SYS",
                            PaneKind::DockerInspector { .. } => "CTR",
                            PaneKind::SystemMonitor => "MON",
                            PaneKind::AiJobs => "AI",
                            PaneKind::HttpPorts { .. } => "NET",
                            PaneKind::TailscalePanel => "TS",
                            PaneKind::FileTracker(_) => "TRK",
                            PaneKind::Vitals(_) => "VIT",
                        };
                        let short_title: String = tab.title.chars().take(18).collect();
                        let label = format!("[{badge}] {short_title}");
                        let w = (label.len() as f32 * 6.4 + 16.0).clamp(84.0, 152.0);
                        if win_x + w > right_x - 4.0 {
                            break;
                        }
                        let item_rect = Rect::from_min_size(Pos2::new(win_x, btn_y), Vec2::new(w, btn_h));
                        let is_active = idx == group.active_tab && !group.shaded;
                        let resp = ui.interact(
                            item_rect,
                            Id::unique(("topbar_win", group.id, tab.id)),
                            Sense::click(),
                        );
                        let fill = if is_active {
                            BEOS_PANEL_LO
                        } else if resp.hovered() {
                            Color32::from_rgb(240, 240, 240)
                        } else {
                            BEOS_WIDGET_FACE
                        };
                        painter.rect_filled(item_rect, CornerRadius::ZERO, fill);
                        draw_beos_bevel(painter, item_rect, is_active);
                        painter.text(
                            Pos2::new(item_rect.min.x + 6.0, item_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &label,
                            egui::FontId::proportional(11.5),
                            Color32::BLACK,
                        );
                        if resp.clicked() {
                            self.menu_open = false;
                            action = Some(DeskbarAction::FocusWindow {
                                group_id: group.id,
                                tab_idx: idx,
                            });
                        }
                        win_x = item_rect.max.x + 4.0;
                    }
                }

                // --- DROPDOWN LAUNCHER MENU ---
                if self.menu_open {
                    let menu_pos = Pos2::new(bar_rect.min.x + 6.0, bar_rect.max.y);
                    let menu_items: Vec<(&str, &str, DeskbarAction)> = vec![
                        ("Tracker (Files)", "TRK", DeskbarAction::OpenFileTracker),
                        ("New Terminal", ">_", DeskbarAction::NewTerminal),
                        (
                            "Systemd Services",
                            "All",
                            DeskbarAction::OpenSystemdInspector {
                                scope: ScopeFilter::All,
                                state: StateFilter::All,
                            },
                        ),
                        (
                            "Running Services",
                            "Run",
                            DeskbarAction::OpenSystemdInspector {
                                scope: ScopeFilter::All,
                                state: StateFilter::Running,
                            },
                        ),
                        (
                            "Enabled Services",
                            "En",
                            DeskbarAction::OpenSystemdInspector {
                                scope: ScopeFilter::All,
                                state: StateFilter::Enabled,
                            },
                        ),
                        ("Containers", "CTR", DeskbarAction::OpenDockerInspector),
                        ("GPU / CPU / Memory", "MON", DeskbarAction::OpenSystemMonitor),
                        ("Vitals (Telemetry)", "VIT", DeskbarAction::OpenVitals),
                        ("Bottom (btm)", "BTM", DeskbarAction::OpenBottom),
                        ("ComfyUI & Unsloth", "AI", DeskbarAction::OpenAiJobs),
                        ("HTTP Ports", "NET", DeskbarAction::OpenHttpPorts),
                        ("Tailscale Mesh", "TS", DeskbarAction::OpenTailscale),
                        ("Show / Hide Tracker", "Left", DeskbarAction::ToggleTracker),
                        ("Tile Windows", "Grid", DeskbarAction::TileWindows),
                    ];
                    let menu_h = menu_items.len() as f32 * 24.0 + 8.0;
                    let menu_rect = Rect::from_min_size(menu_pos, Vec2::new(196.0, menu_h));
                    painter.rect_filled(menu_rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                    draw_beos_bevel(painter, menu_rect, false);

                    for (i, (label, meta, act)) in menu_items.into_iter().enumerate() {
                        let r = Rect::from_min_size(
                            Pos2::new(menu_pos.x + 3.0, menu_pos.y + 4.0 + i as f32 * 24.0),
                            Vec2::new(190.0, 23.0),
                        );
                        let resp = ui.interact(r, Id::unique(("topbar_menu", i)), Sense::click());
                        if resp.hovered() {
                            painter.rect_filled(r, CornerRadius::ZERO, BEOS_TAB_YELLOW);
                        }
                        painter.text(
                            Pos2::new(r.min.x + 8.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(12.0),
                            Color32::BLACK,
                        );
                        painter.text(
                            Pos2::new(r.max.x - 8.0, r.center().y),
                            egui::Align2::RIGHT_CENTER,
                            meta,
                            egui::FontId::monospace(10.5),
                            Color32::from_rgb(75, 75, 75),
                        );
                        if resp.clicked() {
                            self.menu_open = false;
                            action = Some(act);
                        }
                    }
                }
            });

        action
    }
}

fn draw_recessed_box(painter: &egui::Painter, rect: Rect, fill: Color32) {
    painter.rect_filled(rect, CornerRadius::ZERO, fill);
    painter.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, BEOS_BEVEL_DARK));
    painter.line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(1.0, BEOS_BEVEL_DARK));
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, BEOS_BEVEL_LIGHT),
    );
    painter.line_segment(
        [rect.right_top(), rect.right_bottom()],
        Stroke::new(1.0, BEOS_BEVEL_LIGHT),
    );
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

fn draw_mini_folder(painter: &egui::Painter, top_left: Pos2) {
    let tab = Rect::from_min_size(top_left, Vec2::new(5.5, 2.5));
    let body = Rect::from_min_size(top_left + Vec2::new(0.0, 2.0), Vec2::new(12.0, 8.0));
    painter.rect_filled(tab, CornerRadius::ZERO, BEOS_TAB_YELLOW);
    painter.rect_filled(body, CornerRadius::ZERO, BEOS_TAB_YELLOW);
    painter.rect_stroke(
        body,
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
    let body = vec![
        Pos2::new(center.x - 7.0, center.y + 4.0),
        Pos2::new(center.x - 2.0, center.y - 5.0),
        Pos2::new(center.x + 3.0, center.y - 5.0),
        Pos2::new(center.x + 8.0, center.y - 2.0), // Beak tip
        Pos2::new(center.x + 3.0, center.y),
        Pos2::new(center.x + 1.0, center.y + 5.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        body,
        BEOS_BORDER_BLACK,
        Stroke::NONE,
    ));
    // Amber eye dot
    painter.circle_filled(
        Pos2::new(center.x + 2.0, center.y - 3.0),
        1.2,
        BEOS_TAB_YELLOW,
    );
}
