use crate::theme::{
    BEOS_BORDER_BLACK, BEOS_PANEL_GRAY, BEOS_STATUS_GREEN, BEOS_STATUS_RED, BEOS_TAB_YELLOW,
    draw_beos_bevel,
};
use crate::wm::{BeWindowGroup, PaneKind};
use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Vec2};
use kaage_proto::HostMetrics;

pub struct BeDeskbar {
    pub menu_open: bool,
}

pub enum DeskbarAction {
    NewTerminal,
    OpenSystemdInspector,
    OpenDockerInspector,
    FocusWindow { group_id: u64, tab_idx: usize },
}

impl BeDeskbar {
    pub fn new() -> Self {
        Self { menu_open: false }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        metrics: Option<&HostMetrics>,
        connected: bool,
        groups: &[BeWindowGroup],
    ) -> Option<DeskbarAction> {
        let mut action = None;
        let screen = ctx.content_rect();
        let bar_width = 240.0;
        let top_right = Pos2::new((screen.max.x - bar_width - 8.0).max(8.0), 8.0);

        egui::Area::new(Id::unique("kaage_deskbar"))
            .fixed_pos(top_right)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let header_rect = Rect::from_min_size(top_right, Vec2::new(bar_width, 28.0));
                let painter = ui.painter();
                painter.rect_filled(header_rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                draw_beos_bevel(painter, header_rect, false);

                // Left: Kaage Crow Menu Button
                let menu_btn_rect = Rect::from_min_size(top_right, Vec2::new(78.0, 28.0));
                let menu_resp =
                    ui.interact(menu_btn_rect, Id::unique("deskbar_crow_btn"), Sense::click());
                if menu_resp.clicked() {
                    self.menu_open = !self.menu_open;
                }
                draw_beos_bevel(painter, menu_btn_rect, self.menu_open);
                draw_crow_emblem(
                    painter,
                    Pos2::new(menu_btn_rect.min.x + 14.0, menu_btn_rect.center().y),
                );
                painter.text(
                    Pos2::new(menu_btn_rect.min.x + 28.0, menu_btn_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "kaage",
                    egui::FontId::proportional(13.5),
                    Color32::BLACK,
                );

                // Right: Live Host CPU/RAM Mini-Meters + Connection Dot
                let tray_rect = Rect::from_min_max(
                    Pos2::new(menu_btn_rect.max.x, top_right.y),
                    header_rect.max,
                );
                let dot_color = if connected {
                    BEOS_STATUS_GREEN
                } else {
                    BEOS_STATUS_RED
                };
                painter.circle_filled(
                    Pos2::new(tray_rect.min.x + 9.0, tray_rect.center().y),
                    3.5,
                    dot_color,
                );

                if let Some(m) = metrics {
                    let cpu_frac = (m.cpu_percent / 100.0).clamp(0.0, 1.0);
                    let mem_frac = if m.mem_total_mb > 0 {
                        (m.mem_used_mb as f32 / m.mem_total_mb as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };

                    let bar_x = tray_rect.min.x + 18.0;
                    let bar_w = 64.0;

                    // CPU bar
                    let cpu_bar = Rect::from_min_size(
                        Pos2::new(bar_x, tray_rect.min.y + 6.0),
                        Vec2::new(bar_w, 6.0),
                    );
                    painter.rect_filled(cpu_bar, CornerRadius::ZERO, Color32::from_rgb(70, 70, 70));
                    painter.rect_filled(
                        Rect::from_min_size(cpu_bar.min, Vec2::new(bar_w * cpu_frac, 6.0)),
                        CornerRadius::ZERO,
                        BEOS_STATUS_GREEN,
                    );

                    // RAM bar
                    let mem_bar = Rect::from_min_size(
                        Pos2::new(bar_x, tray_rect.min.y + 15.0),
                        Vec2::new(bar_w, 6.0),
                    );
                    painter.rect_filled(mem_bar, CornerRadius::ZERO, Color32::from_rgb(70, 70, 70));
                    painter.rect_filled(
                        Rect::from_min_size(mem_bar.min, Vec2::new(bar_w * mem_frac, 6.0)),
                        CornerRadius::ZERO,
                        BEOS_TAB_YELLOW,
                    );

                    painter.text(
                        Pos2::new(tray_rect.max.x - 8.0, tray_rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        format!("{:.0}%", m.cpu_percent),
                        egui::FontId::monospace(11.5),
                        Color32::BLACK,
                    );

                    let tray_resp = ui.interact(
                        tray_rect,
                        Id::unique("deskbar_metrics_tooltip"),
                        Sense::hover(),
                    );
                    tray_resp.on_hover_text(format!(
                        "{}\nCPU: {:.1}%\nRAM: {} / {} MiB\nUptime: {}",
                        m.hostname,
                        m.cpu_percent,
                        m.mem_used_mb,
                        m.mem_total_mb,
                        format_uptime(m.uptime_secs),
                    ));
                } else {
                    painter.text(
                        Pos2::new(tray_rect.max.x - 8.0, tray_rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        if connected { "sync..." } else { "offline" },
                        egui::FontId::monospace(11.0),
                        Color32::from_rgb(60, 60, 60),
                    );
                }

                // Stacked Running Window / Tab Trays underneath Deskbar
                let mut row_y = header_rect.max.y - 1.0;
                for group in groups {
                    for (idx, tab) in group.tabs.iter().enumerate() {
                        let item_rect = Rect::from_min_size(
                            Pos2::new(top_right.x + 18.0, row_y),
                            Vec2::new(bar_width - 18.0, 21.0),
                        );
                        let is_active = idx == group.active_tab && !group.shaded;
                        let fill = if is_active {
                            Color32::from_rgb(236, 236, 236)
                        } else {
                            BEOS_PANEL_GRAY
                        };
                        painter.rect_filled(item_rect, CornerRadius::ZERO, fill);
                        draw_beos_bevel(painter, item_rect, false);

                        let badge = match tab.kind {
                            PaneKind::Terminal(_) => ">_",
                            PaneKind::SystemdInspector { .. } => "SYS",
                            PaneKind::DockerInspector { .. } => "CTR",
                        };
                        painter.text(
                            Pos2::new(item_rect.min.x + 6.0, item_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            format!("[{badge}] {}", tab.title),
                            egui::FontId::proportional(12.0),
                            Color32::BLACK,
                        );

                        let resp = ui.interact(
                            item_rect,
                            Id::unique(("deskbar_item", group.id, tab.id)),
                            Sense::click(),
                        );
                        if resp.clicked() {
                            action = Some(DeskbarAction::FocusWindow {
                                group_id: group.id,
                                tab_idx: idx,
                            });
                        }

                        row_y += 20.0;
                    }
                }

                // Dropdown Launcher Menu when Kaage button is toggled
                if self.menu_open {
                    let menu_pos = Pos2::new((top_right.x - 165.0).max(8.0), top_right.y);
                    let menu_rect = Rect::from_min_size(menu_pos, Vec2::new(164.0, 82.0));
                    painter.rect_filled(menu_rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                    draw_beos_bevel(painter, menu_rect, false);

                    let items = [
                        ("New Terminal", DeskbarAction::NewTerminal),
                        ("Systemd Services", DeskbarAction::OpenSystemdInspector),
                        ("Containers", DeskbarAction::OpenDockerInspector),
                    ];
                    for (i, (label, act)) in items.into_iter().enumerate() {
                        let r = Rect::from_min_size(
                            Pos2::new(menu_pos.x + 3.0, menu_pos.y + 3.0 + (i as f32 * 25.0)),
                            Vec2::new(158.0, 24.0),
                        );
                        let resp = ui.interact(r, Id::unique(("deskbar_menu", i)), Sense::click());
                        if resp.hovered() {
                            painter.rect_filled(r, CornerRadius::ZERO, BEOS_TAB_YELLOW);
                        }
                        painter.text(
                            Pos2::new(r.min.x + 8.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::proportional(12.5),
                            Color32::BLACK,
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
