use crate::inspectors::{
    InspectorSpawnRequest, ScopeFilter, StateFilter, render_docker_inspector,
    render_systemd_inspector,
};
use crate::terminal::TerminalPane;
use crate::theme::{BEOS_PANEL_GRAY, BEOS_TAB_INACTIVE, BEOS_TAB_YELLOW, draw_beos_bevel};
use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Vec2};
use kaage_proto::{ClientMsg, ContainerInfo, NixosInfo, SystemdUnitInfo, SystemdUnitStatus};

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

    pub fn open_window(&mut self, title: String, kind: PaneKind, pos: Pos2, size: Vec2) {
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

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        nixos: Option<&NixosInfo>,
        units: &[SystemdUnitInfo],
        selected_unit_status: &mut Option<SystemdUnitStatus>,
        containers: &[ContainerInfo],
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

                        // BeOS Left Close Box
                        let close_rect = Rect::from_min_size(
                            Pos2::new(tab_rect.min.x + 5.0, tab_rect.min.y + 5.0),
                            Vec2::splat(11.0),
                        );
                        draw_beos_bevel(ui.painter(), close_rect, false);
                        let close_resp = ui.interact(
                            close_rect,
                            Id::unique(("be_close", group.id, tab.id)),
                            Sense::click(),
                        );
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

                        let tab_resp = ui.interact(
                            tab_rect,
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
