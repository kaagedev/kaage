use crate::inspectors::{ScopeFilter, StateFilter};
use crate::theme::{TOPBAR_HEIGHT, be_button, draw_beos_bevel, palette};
use crate::wm::{BeWindowGroup, PaneKind};
use egui::{Color32, CornerRadius, Id, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Vec2};
use kaage_proto::{
    ContainerInfo, DashboardSnapshot, HostMetrics, NixosInfo, SystemdUnitInfo, UnitScope,
};

pub struct BeTrackerSidebar {
    pub visible: bool,
    pub wide: bool,
}

pub enum TrackerAction {
    Hide,
    NewTerminal,
    OpenServices {
        scope: ScopeFilter,
        state: StateFilter,
    },
    OpenContainers,
    OpenSystemMonitor,
    OpenVitals,
    OpenAiJobs,
    OpenHttpPorts,
    OpenTailscale,
    OpenFileTracker,
    OpenHelp,
    OpenSettings,
    OpenJournal {
        user: bool,
    },
    FocusWindow {
        group_id: u64,
        tab_idx: usize,
    },
}

impl BeTrackerSidebar {
    pub fn new() -> Self {
        Self {
            visible: true,
            wide: false,
        }
    }

    pub fn width(&self) -> f32 {
        if !self.visible {
            0.0
        } else if self.wide {
            280.0
        } else {
            224.0
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        nixos: Option<&NixosInfo>,
        metrics: Option<&HostMetrics>,
        units: &[SystemdUnitInfo],
        containers: &[ContainerInfo],
        dashboard: Option<&DashboardSnapshot>,
        groups: &[BeWindowGroup],
    ) -> Option<TrackerAction> {
        if !self.visible {
            return None;
        }

        let p = palette();
        let mut action = None;
        let screen = ctx.content_rect();
        let width = self.width();
        let origin = Pos2::new(8.0, TOPBAR_HEIGHT + 8.0);
        let tab_h = 22.0;
        let body_h = (screen.height() - TOPBAR_HEIGHT - tab_h - 16.0).max(240.0);

        egui::Area::new(Id::unique("kaage_tracker_sidebar"))
            .fixed_pos(origin)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                // Header Tab for Tracker Sidebar
                let tab_w = 152.0_f32.min(width);
                let tab_rect = Rect::from_min_size(origin, Vec2::new(tab_w, tab_h + 1.0));
                {
                    let painter = ui.painter();
                    painter.rect_filled(tab_rect, CornerRadius::ZERO, p.tab_active_bg);
                    draw_beos_bevel(painter, tab_rect, false);
                    if p.is_dark {
                        painter.rect_filled(
                            Rect::from_min_size(tab_rect.min, Vec2::new(tab_rect.width(), 2.0)),
                            CornerRadius::ZERO,
                            p.accent,
                        );
                    }
                }

                // Close gadget (left)
                let close_rect = Rect::from_min_size(
                    Pos2::new(tab_rect.min.x + 5.0, tab_rect.min.y + 5.0),
                    Vec2::splat(11.0),
                );
                draw_beos_bevel(ui.painter(), close_rect, false);
                if ui
                    .interact(close_rect, Id::unique("tracker_close"), Sense::click())
                    .on_hover_text("Hide sidebar")
                    .clicked()
                {
                    action = Some(TrackerAction::Hide);
                }

                // Zoom/width gadget (right)
                let zoom_rect = Rect::from_min_size(
                    Pos2::new(tab_rect.max.x - 16.0, tab_rect.min.y + 5.0),
                    Vec2::splat(11.0),
                );
                draw_beos_bevel(ui.painter(), zoom_rect, self.wide);
                if ui
                    .interact(zoom_rect, Id::unique("tracker_zoom"), Sense::click())
                    .on_hover_text("Toggle sidebar width")
                    .clicked()
                {
                    self.wide = !self.wide;
                }

                ui.painter().text(
                    Pos2::new(close_rect.max.x + 6.0, tab_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Host Nav",
                    egui::FontId::proportional(12.0),
                    p.tab_active_fg,
                );

                // Window Body
                let body_pos = Pos2::new(origin.x, origin.y + tab_h);
                let body_rect = Rect::from_min_size(body_pos, Vec2::new(width, body_h));
                {
                    let painter = ui.painter();
                    painter.rect_filled(body_rect, CornerRadius::ZERO, p.surface);
                    draw_beos_bevel(painter, body_rect, false);
                }

                // Header strip
                let header_h = 38.0;
                let header_rect =
                    Rect::from_min_size(body_rect.min + Vec2::new(3.0, 3.0), Vec2::new(width - 6.0, header_h));
                let mut header_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(header_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                header_ui.add_space(4.0);
                let hostname = metrics
                    .map(|m| m.hostname.as_str())
                    .unwrap_or("kaage-host");
                let subtitle = nixos
                    .filter(|n| n.is_nixos)
                    .map(|n| {
                        n.generation
                            .map(|g| format!("NixOS · gen #{g}"))
                            .unwrap_or_else(|| "NixOS".into())
                    })
                    .unwrap_or_else(|| "Linux Host".into());
                header_ui.vertical(|ui| {
                    ui.label(RichText::new(hostname).strong().size(12.5).color(p.text));
                    ui.label(RichText::new(subtitle).size(10.0).color(p.text_muted));
                });
                header_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(2.0);
                    if be_button(ui, "+ Term").clicked() {
                        action = Some(TrackerAction::NewTerminal);
                    }
                });

                // Recessed well
                let status_h = 20.0;
                let well_rect = Rect::from_min_max(
                    Pos2::new(body_rect.min.x + 4.0, header_rect.max.y + 2.0),
                    Pos2::new(body_rect.max.x - 4.0, body_rect.max.y - status_h - 3.0),
                );
                {
                    let painter = ui.painter();
                    painter.rect_filled(well_rect, CornerRadius::ZERO, p.well);
                    draw_beos_bevel(painter, well_rect, true);
                }

                let total_units = units.len();
                let running_units = units.iter().filter(|u| u.running).count();
                let enabled_units = units.iter().filter(|u| u.enabled).count();
                let sys_units = units.iter().filter(|u| u.scope == UnitScope::System).count();
                let usr_units = units.iter().filter(|u| u.scope == UnitScope::User).count();
                let failed_units = units.iter().filter(|u| u.active_state == "failed").count();
                let running_ctrs = containers.iter().filter(|c| c.state == "running").count();

                let mut well_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(well_rect.shrink(2.0))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                ScrollArea::vertical()
                    .id_salt("tracker_well_scroll")
                    .show(&mut well_ui, |ui| {
                        section_head(ui, "SYSTEMD SERVICES");
                        if tracker_row(ui, "All Services", Some(total_units), false) {
                            action = Some(TrackerAction::OpenServices {
                                scope: ScopeFilter::All,
                                state: StateFilter::All,
                            });
                        }
                        if tracker_row(ui, "Running", Some(running_units), false) {
                            action = Some(TrackerAction::OpenServices {
                                scope: ScopeFilter::All,
                                state: StateFilter::Running,
                            });
                        }
                        if tracker_row(ui, "Enabled", Some(enabled_units), false) {
                            action = Some(TrackerAction::OpenServices {
                                scope: ScopeFilter::All,
                                state: StateFilter::Enabled,
                            });
                        }
                        if tracker_row(ui, "System Units", Some(sys_units), false) {
                            action = Some(TrackerAction::OpenServices {
                                scope: ScopeFilter::System,
                                state: StateFilter::All,
                            });
                        }
                        if tracker_row(ui, "User Units", Some(usr_units), false) {
                            action = Some(TrackerAction::OpenServices {
                                scope: ScopeFilter::User,
                                state: StateFilter::All,
                            });
                        }
                        if tracker_row(ui, "Failed", Some(failed_units), failed_units > 0) {
                            action = Some(TrackerAction::OpenServices {
                                scope: ScopeFilter::All,
                                state: StateFilter::Failed,
                            });
                        }

                        let ai_active = dashboard
                            .map(|d| {
                                d.comfyui.running_jobs.len()
                                    + d.comfyui.pending_jobs.len()
                                    + d.unsloth.jobs.len()
                            })
                            .unwrap_or(0);
                        let port_count = dashboard.map(|d| d.listening_ports.len()).unwrap_or(0);
                        let peer_online = dashboard
                            .map(|d| d.tailscale.peers.iter().filter(|p| p.online).count())
                            .unwrap_or(0);
                        let gpu_count = dashboard.map(|d| d.gpus.len()).unwrap_or(0);

                        section_head(ui, "MONITORING & AI");
                        if tracker_row(ui, "Vitals (Telemetry)", None, false) {
                            action = Some(TrackerAction::OpenVitals);
                        }
                        if tracker_row(ui, "GPU / CPU / Memory", Some(gpu_count), false) {
                            action = Some(TrackerAction::OpenSystemMonitor);
                        }
                        if tracker_row(ui, "ComfyUI & Unsloth", Some(ai_active), false) {
                            action = Some(TrackerAction::OpenAiJobs);
                        }
                        if tracker_row(ui, "HTTP Ports", Some(port_count), false) {
                            action = Some(TrackerAction::OpenHttpPorts);
                        }
                        if tracker_row(ui, "Tailscale Mesh", Some(peer_online), false) {
                            action = Some(TrackerAction::OpenTailscale);
                        }

                        section_head(ui, "CONTAINERS");
                        if tracker_row(
                            ui,
                            &format!("Containers ({running_ctrs} up)"),
                            Some(containers.len()),
                            false,
                        ) {
                            action = Some(TrackerAction::OpenContainers);
                        }

                        section_head(ui, "QUICK LAUNCH");
                        if tracker_row(ui, "Tracker (Files)", None, false) {
                            action = Some(TrackerAction::OpenFileTracker);
                        }
                        if tracker_row(ui, "?   kaage-help", None, false) {
                            action = Some(TrackerAction::OpenHelp);
                        }
                        if tracker_row(ui, "⚙   kaage-settings", None, false) {
                            action = Some(TrackerAction::OpenSettings);
                        }
                        if tracker_row(ui, ">_  New Shell", None, false) {
                            action = Some(TrackerAction::NewTerminal);
                        }
                        if tracker_row(ui, "≡   System Journal", None, false) {
                            action = Some(TrackerAction::OpenJournal { user: false });
                        }
                        if tracker_row(ui, "≡   User Journal", None, false) {
                            action = Some(TrackerAction::OpenJournal { user: true });
                        }

                        section_head(ui, "OPEN WINDOWS");
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
                                    PaneKind::Help(_) => "?",
                                    PaneKind::Settings(_) => "CFG",
                                };
                                let label = format!("[{badge}] {}", tab.title);
                                if tracker_row(ui, &label, None, false) {
                                    action = Some(TrackerAction::FocusWindow {
                                        group_id: group.id,
                                        tab_idx: idx,
                                    });
                                }
                            }
                        }
                    });

                // Tracker Bottom Status Bar
                let status_rect = Rect::from_min_max(
                    Pos2::new(body_rect.min.x + 2.0, body_rect.max.y - status_h - 1.0),
                    Pos2::new(body_rect.max.x - 2.0, body_rect.max.y - 2.0),
                );
                {
                    let painter = ui.painter();
                    painter.rect_filled(status_rect, CornerRadius::ZERO, p.surface_alt);
                    painter.line_segment(
                        [status_rect.left_top(), status_rect.right_top()],
                        Stroke::new(1.0, p.border),
                    );
                    painter.text(
                        Pos2::new(status_rect.min.x + 6.0, status_rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("{running_units} run · {enabled_units} en · {total_units} svc"),
                        egui::FontId::monospace(10.0),
                        p.text_muted,
                    );
                }
            });

        action
    }
}

fn section_head(ui: &mut egui::Ui, title: &str) {
    let p = palette();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 19.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, p.section_bg);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, p.border),
    );
    painter.text(
        Pos2::new(rect.min.x + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::monospace(9.5),
        p.text_muted,
    );
}

fn tracker_row(ui: &mut egui::Ui, label: &str, count: Option<usize>, alert: bool) -> bool {
    let p = palette();
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 21.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::ZERO, p.hover_row);
        }
        let text_color = if alert { p.status_red } else { p.text };
        painter.text(
            Pos2::new(rect.min.x + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.5),
            text_color,
        );
        if let Some(c) = count {
            let pill = Rect::from_min_size(
                Pos2::new(rect.max.x - 32.0, rect.min.y + 3.0),
                Vec2::new(26.0, 15.0),
            );
            painter.rect_filled(
                pill,
                CornerRadius::ZERO,
                if alert {
                    Color32::from_rgba_unmultiplied(239, 68, 68, 45)
                } else {
                    p.surface_alt
                },
            );
            painter.rect_stroke(
                pill,
                CornerRadius::ZERO,
                Stroke::new(1.0, p.border),
                egui::StrokeKind::Inside,
            );
            painter.text(
                pill.center(),
                egui::Align2::CENTER_CENTER,
                c.to_string(),
                egui::FontId::monospace(10.0),
                text_color,
            );
        }
    }
    resp.clicked()
}
