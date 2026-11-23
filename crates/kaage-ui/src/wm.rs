use crate::inspectors::{
    InspectorSpawnRequest, ScopeFilter, StateFilter, render_docker_inspector,
    render_systemd_inspector,
};
use crate::panels::{
    render_ai_jobs, render_http_ports, render_system_monitor, render_tailscale_panel,
};
use crate::terminal::TerminalPane;
use crate::theme::{
    BEOS_PANEL_GRAY, BEOS_TAB_INACTIVE, BEOS_TAB_YELLOW, TOPBAR_HEIGHT, draw_beos_bevel,
};
use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Vec2};
use kaage_proto::{
    ClientMsg, ContainerInfo, DashboardSnapshot, NixosInfo, SystemdUnitInfo, SystemdUnitStatus,
};

pub enum PaneKind {
    Terminal(TerminalPane),
    SystemdInspector {
        filter: String,
        scope_filter: ScopeFilter,
        state_filter: StateFilter,
    },
    DockerInspector {
        filter: String,
    },
    SystemMonitor,
    AiJobs,
    HttpPorts {
        filter: String,
    },
    TailscalePanel,
}

pub struct BeTab {
    pub id: u64,
    pub title: String,
    pub kind: PaneKind,
}

pub struct BeWindowGroup {
    pub id: u64,
    pub pos: Pos2,
    pub size: Vec2,
    pub tab_offset: f32,
    pub active_tab: usize,
    pub tabs: Vec<BeTab>,
    pub shaded: bool,
}

pub struct BeWindowManager {
    pub groups: Vec<BeWindowGroup>,
    next_id: u64,
}

impl BeWindowManager {
    pub fn new() -> Self {
        Self {
            groups: Vec::new(),
            next_id: 1,
        }
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn open_window(&mut self, title: String, kind: PaneKind, pos: Pos2, size: Vec2) -> u64 {
        let id = self.alloc_id();
        self.groups.push(BeWindowGroup {
            id,
            pos,
            size,
            tab_offset: 0.0,
            active_tab: 0,
            tabs: vec![BeTab { id, title, kind }],
            shaded: false,
        });
        id
    }

    pub fn add_tab_to_group(&mut self, group_id: u64, title: String, kind: PaneKind) {
        let tab_id = self.alloc_id();
        if let Some(group) = self.groups.iter_mut().find(|g| g.id == group_id) {
            group.tabs.push(BeTab {
                id: tab_id,
                title,
                kind,
            });
        }
    }

    pub fn focus_matching_tab(&mut self, predicate: impl Fn(&PaneKind) -> bool) -> bool {
        for g_idx in 0..self.groups.len() {
            for t_idx in 0..self.groups[g_idx].tabs.len() {
                if predicate(&self.groups[g_idx].tabs[t_idx].kind) {
                    let gid = self.groups[g_idx].id;
                    self.focus_tab(gid, t_idx);
                    return true;
                }
            }
        }
        false
    }

    pub fn focus_tab_by_title(&mut self, title: &str) -> bool {
        for g_idx in 0..self.groups.len() {
            for t_idx in 0..self.groups[g_idx].tabs.len() {
                if self.groups[g_idx].tabs[t_idx].title == title {
                    let gid = self.groups[g_idx].id;
                    self.focus_tab(gid, t_idx);
                    return true;
                }
            }
        }
        false
    }

    pub fn focus_tab(&mut self, group_id: u64, tab_idx: usize) {
        if let Some(pos) = self.groups.iter().position(|g| g.id == group_id) {
            let mut group = self.groups.remove(pos);
            group.shaded = false;
            if tab_idx < group.tabs.len() {
                group.active_tab = tab_idx;
            }
            // Move focused group to top of z-order
            self.groups.push(group);
        }
    }

    pub fn open_or_focus_systemd(
        &mut self,
        scope: ScopeFilter,
        state: StateFilter,
        sidebar_width: f32,
    ) {
        for g_idx in 0..self.groups.len() {
            for t_idx in 0..self.groups[g_idx].tabs.len() {
                if let PaneKind::SystemdInspector {
                    scope_filter,
                    state_filter,
                    ..
                } = &mut self.groups[g_idx].tabs[t_idx].kind
                {
                    *scope_filter = scope;
                    *state_filter = state;
                    let gid = self.groups[g_idx].id;
                    self.focus_tab(gid, t_idx);
                    return;
                }
            }
        }
        let start_x = if sidebar_width > 0.0 {
            sidebar_width + 24.0
        } else {
            24.0
        };
        self.open_window(
            "Systemd Services".into(),
            PaneKind::SystemdInspector {
                filter: String::new(),
                scope_filter: scope,
                state_filter: state,
            },
            Pos2::new(start_x, TOPBAR_HEIGHT + 12.0),
            Vec2::new(680.0, 420.0),
        );
    }

    pub fn tile_windows(&mut self, screen: Rect, sidebar_width: f32) {
        let count = self.groups.len();
        if count == 0 {
            return;
        }
        let left = if sidebar_width > 0.0 {
            sidebar_width + 22.0
        } else {
            14.0
        };
        let top = TOPBAR_HEIGHT + 12.0;
        let right_margin = if screen.width() >= 760.0 { 104.0 } else { 14.0 };
        let avail_w = (screen.width() - left - right_margin).max(320.0);
        let avail_h = (screen.height() - top - 16.0).max(260.0);
        let tab_h = 21.0;

        let cols = if count == 1 { 1 } else { 2 };
        let rows = count.div_ceil(cols);
        let cell_w = ((avail_w - (cols as f32 - 1.0) * 12.0) / cols as f32).max(260.0);
        let cell_h = ((avail_h - (rows as f32 - 1.0) * 12.0) / rows as f32 - tab_h).max(160.0);

        for (i, group) in self.groups.iter_mut().enumerate() {
            let col = i % cols;
            let row = i / cols;
            group.shaded = false;
            group.pos = Pos2::new(
                left + col as f32 * (cell_w + 12.0),
                top + row as f32 * (cell_h + tab_h + 12.0),
            );
            group.size = Vec2::new(cell_w, cell_h);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        nixos: Option<&NixosInfo>,
        units: &[SystemdUnitInfo],
        selected_unit_status: &mut Option<SystemdUnitStatus>,
        containers: &[ContainerInfo],
        dashboard: Option<&DashboardSnapshot>,
        cpu_history: &[f32],
        mem_history: &[f32],
        send_msg: &mut dyn FnMut(ClientMsg),
    ) -> Vec<InspectorSpawnRequest> {
        let mut spawn_requests = Vec::new();
        let mut merge_req: Option<(u64, usize, u64)> = None;
        let mut detach_req: Option<(u64, usize, Pos2)> = None;
        let mut close_req: Option<(u64, usize)> = None;
        let mut bring_to_front: Option<u64> = None;

        let header_zones: Vec<(u64, Rect)> = self
            .groups
            .iter()
            .map(|g| (g.id, Rect::from_min_size(g.pos, Vec2::new(g.size.x, 22.0))))
            .collect();

        for group in &mut self.groups {
            let tab_height = 21.0;
            let body_pos = Pos2::new(group.pos.x, group.pos.y + tab_height);

            egui::Area::new(Id::unique(("be_group", group.id)))
                .current_pos(group.pos)
                .order(egui::Order::Middle)
                .show(ctx, |ui| {
                    let mut cursor_x = group.pos.x + group.tab_offset;

                    for (idx, tab) in group.tabs.iter().enumerate() {
                        let is_active = idx == group.active_tab;
                        let tab_width = (tab.title.len() as f32 * 7.8 + 44.0).max(96.0);
                        let tab_rect = Rect::from_min_size(
                            Pos2::new(cursor_x, group.pos.y),
                            Vec2::new(tab_width, tab_height + 1.0),
                        );

                        let fill = if is_active {
                            BEOS_TAB_YELLOW
                        } else {
                            BEOS_TAB_INACTIVE
                        };
                        {
                            let painter = ui.painter();
                            painter.rect_filled(tab_rect, CornerRadius::ZERO, fill);
                            draw_beos_bevel(painter, tab_rect, false);
                        }

                        // BeOS Left Close Box (non-overlapping hit rect so tab drag never swallows clicks)
                        let close_rect = Rect::from_min_size(
                            Pos2::new(tab_rect.min.x + 5.0, tab_rect.min.y + 5.0),
                            Vec2::splat(12.0),
                        );
                        let close_hit_rect = close_rect.expand(2.0);
                        let close_resp = ui
                            .interact(
                                close_hit_rect,
                                Id::unique(("be_close", group.id, tab.id)),
                                Sense::click(),
                            )
                            .on_hover_text("Close");
                        let close_pressed = close_resp.is_pointer_button_down_on();
                        if close_resp.hovered() {
                            ui.painter().rect_filled(
                                close_rect,
                                CornerRadius::ZERO,
                                Color32::from_rgb(255, 224, 102),
                            );
                        }
                        draw_beos_bevel(ui.painter(), close_rect, close_pressed);
                        if close_resp.hovered() {
                            let c = close_rect.center();
                            let d = 2.5;
                            let stroke = egui::Stroke::new(1.2, Color32::BLACK);
                            ui.painter().line_segment(
                                [Pos2::new(c.x - d, c.y - d), Pos2::new(c.x + d, c.y + d)],
                                stroke,
                            );
                            ui.painter().line_segment(
                                [Pos2::new(c.x + d, c.y - d), Pos2::new(c.x - d, c.y + d)],
                                stroke,
                            );
                        }
                        if close_resp.clicked() {
                            close_req = Some((group.id, idx));
                        }

                        // Tab Title
                        ui.painter().text(
                            Pos2::new(close_rect.max.x + 6.0, tab_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &tab.title,
                            egui::FontId::proportional(13.0),
                            Color32::BLACK,
                        );

                        let tab_drag_rect = Rect::from_min_max(
                            Pos2::new(close_hit_rect.max.x + 1.0, tab_rect.min.y),
                            tab_rect.max,
                        );
                        let tab_resp = ui.interact(
                            tab_drag_rect,
                            Id::unique(("be_tab", group.id, tab.id)),
                            Sense::click_and_drag(),
                        );

                        if tab_resp.clicked() {
                            group.active_tab = idx;
                            bring_to_front = Some(group.id);
                        }
                        if tab_resp.double_clicked() {
                            group.shaded = !group.shaded;
                        }
                        if tab_resp.dragged() {
                            bring_to_front = Some(group.id);
                            let delta = tab_resp.drag_delta();
                            let shift_held = ui.input(|i| i.modifiers.shift);

                            if shift_held {
                                let max_offset = (group.size.x - tab_width).max(0.0);
                                group.tab_offset =
                                    (group.tab_offset + delta.x).clamp(0.0, max_offset);
                            } else if group.tabs.len() > 1 && delta.length() > 14.0 {
                                if let Some(ptr) = ui.input(|i| i.pointer.interact_pos()) {
                                    detach_req = Some((group.id, idx, ptr));
                                }
                            } else {
                                group.pos += delta;
                                group.pos.y = group.pos.y.max(TOPBAR_HEIGHT + 2.0);
                            }
                        }

                        if tab_resp.drag_stopped()
                            && let Some(ptr) = ui.input(|i| i.pointer.interact_pos())
                        {
                            for (target_id, zone) in &header_zones {
                                if *target_id != group.id && zone.contains(ptr) {
                                    merge_req = Some((group.id, idx, *target_id));
                                    break;
                                }
                            }
                        }

                        cursor_x += tab_width - 1.0;
                    }

                    if !group.shaded {
                        let body_rect = Rect::from_min_size(body_pos, group.size);
                        {
                            let painter = ui.painter();
                            painter.rect_filled(body_rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                            draw_beos_bevel(painter, body_rect, false);
                        }

                        let inner_rect = body_rect.shrink(4.0);
                        let mut child_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(inner_rect)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );

                        if let Some(active) = group.tabs.get_mut(group.active_tab) {
                            match &mut active.kind {
                                PaneKind::Terminal(term) => {
                                    term.ui(&mut child_ui, send_msg);
                                }
                                PaneKind::SystemdInspector {
                                    filter,
                                    scope_filter,
                                    state_filter,
                                } => {
                                    if let Some(req) = render_systemd_inspector(
                                        &mut child_ui,
                                        filter,
                                        scope_filter,
                                        state_filter,
                                        nixos,
                                        units,
                                        selected_unit_status,
                                        send_msg,
                                    ) {
                                        spawn_requests.push(req);
                                    }
                                }
                                PaneKind::DockerInspector { filter } => {
                                    if let Some(req) = render_docker_inspector(
                                        &mut child_ui,
                                        filter,
                                        containers,
                                        send_msg,
                                    ) {
                                        spawn_requests.push(req);
                                    }
                                }
                                PaneKind::SystemMonitor => {
                                    if let Some(req) = render_system_monitor(
                                        &mut child_ui,
                                        dashboard,
                                        cpu_history,
                                        mem_history,
                                        send_msg,
                                    ) {
                                        spawn_requests.push(req);
                                    }
                                }
                                PaneKind::AiJobs => {
                                    if let Some(req) =
                                        render_ai_jobs(&mut child_ui, dashboard, send_msg)
                                    {
                                        spawn_requests.push(req);
                                    }
                                }
                                PaneKind::HttpPorts { filter } => {
                                    render_http_ports(&mut child_ui, filter, dashboard, send_msg);
                                }
                                PaneKind::TailscalePanel => {
                                    render_tailscale_panel(&mut child_ui, dashboard, send_msg);
                                }
                            }
                        }

                        // Bottom-right resize grip
                        let grip_rect = Rect::from_min_size(
                            Pos2::new(body_rect.max.x - 14.0, body_rect.max.y - 14.0),
                            Vec2::splat(14.0),
                        );
                        draw_beos_bevel(ui.painter(), grip_rect, false);
                        let resize_resp = ui.interact(
                            grip_rect,
                            Id::unique(("be_resize", group.id)),
                            Sense::drag(),
                        );
                        if resize_resp.dragged() {
                            group.size = (group.size + resize_resp.drag_delta())
                                .max(Vec2::new(260.0, 140.0));
                        }
                    }
                });
        }

        if let Some((gid, idx)) = close_req {
            self.close_tab(gid, idx, send_msg);
        }
        if let Some((src, idx, dst)) = merge_req {
            self.stack_tab(src, idx, dst);
        }
        if let Some((src, idx, pos)) = detach_req {
            self.detach_tab(src, idx, pos);
        }
        if let Some(gid) = bring_to_front
            && let Some(pos) = self.groups.iter().position(|g| g.id == gid)
        {
            let g = self.groups.remove(pos);
            self.groups.push(g);
        }

        spawn_requests
    }

    fn close_tab(&mut self, group_id: u64, tab_idx: usize, send_msg: &mut dyn FnMut(ClientMsg)) {
        let Some(g_pos) = self.groups.iter().position(|g| g.id == group_id) else {
            return;
        };
        if tab_idx >= self.groups[g_pos].tabs.len() {
            return;
        }
        let tab = self.groups[g_pos].tabs.remove(tab_idx);
        if let PaneKind::Terminal(term) = tab.kind {
            send_msg(ClientMsg::ClosePty {
                session_id: term.session_id,
            });
        }
        if self.groups[g_pos].tabs.is_empty() {
            self.groups.remove(g_pos);
        } else {
            self.groups[g_pos].active_tab = self.groups[g_pos]
                .active_tab
                .min(self.groups[g_pos].tabs.len() - 1);
        }
    }

    fn stack_tab(&mut self, src_id: u64, tab_idx: usize, dst_id: u64) {
        let Some(src_pos) = self.groups.iter().position(|g| g.id == src_id) else {
            return;
        };
        if tab_idx >= self.groups[src_pos].tabs.len() {
            return;
        }
        let tab = self.groups[src_pos].tabs.remove(tab_idx);
        if self.groups[src_pos].tabs.is_empty() {
            self.groups.remove(src_pos);
        } else {
            self.groups[src_pos].active_tab = self.groups[src_pos]
                .active_tab
                .min(self.groups[src_pos].tabs.len() - 1);
        }
        if let Some(dst) = self.groups.iter_mut().find(|g| g.id == dst_id) {
            dst.tabs.push(tab);
            dst.active_tab = dst.tabs.len() - 1;
        }
    }

    fn detach_tab(&mut self, src_id: u64, tab_idx: usize, pos: Pos2) {
        let Some(src) = self.groups.iter_mut().find(|g| g.id == src_id) else {
            return;
        };
        if src.tabs.len() <= 1 || tab_idx >= src.tabs.len() {
            return;
        }
        let tab = src.tabs.remove(tab_idx);
        src.active_tab = src.active_tab.min(src.tabs.len() - 1);
        let new_group_id = self.alloc_id();
        self.groups.push(BeWindowGroup {
            id: new_group_id,
            pos,
            size: Vec2::new(620.0, 380.0),
            tab_offset: 0.0,
            active_tab: 0,
            tabs: vec![tab],
            shaded: false,
        });
    }
}
