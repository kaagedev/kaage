use crate::debug_panel::DebugPanelWindow;
use crate::dom_overlay::{
    CopilotCrossAction, CopilotWindow, hide_copilot_overlay, is_dom_overlay_focused,
};
use crate::file_tracker::{TrackerSpawnRequest, TrackerWindow};
use crate::help::HelpWindow;
use crate::inspectors::{
    InspectorSpawnRequest, ScopeFilter, StateFilter, render_docker_inspector,
    render_systemd_inspector,
};
use crate::panels::{
    render_ai_jobs, render_http_ports, render_system_monitor, render_tailscale_panel,
    render_tailscale_services,
};
use crate::settings::SettingsWindow;
use crate::terminal::TerminalPane;
use crate::theme::{TOPBAR_HEIGHT, draw_beos_bevel, palette};
use crate::vitals::{VitalsSpawnRequest, VitalsWindow};
use egui::{Color32, CornerRadius, Id, Key, Pos2, Rect, Sense, Stroke, Vec2};
use kaage_proto::{
    AppDataSnapshot, ClientMsg, ContainerInfo, CopilotStatus, DashboardSnapshot, DebugLogEntry,
    NixosInfo, SystemdUnitInfo, SystemdUnitStatus,
};
use std::collections::VecDeque;

pub const WORKSPACE_NAMES: [&str; 4] = ["1: Ops", "2: AI", "3: Files", "4: Logs"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TilingLayout {
    #[default]
    Bsp,
    Columns,
    Rows,
    Grid,
    Focus,
    Float,
}

impl TilingLayout {
    pub const ALL: [TilingLayout; 6] = [
        TilingLayout::Bsp,
        TilingLayout::Columns,
        TilingLayout::Rows,
        TilingLayout::Grid,
        TilingLayout::Focus,
        TilingLayout::Float,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TilingLayout::Bsp => "BSP",
            TilingLayout::Columns => "Cols",
            TilingLayout::Rows => "Rows",
            TilingLayout::Grid => "Grid",
            TilingLayout::Focus => "Focus",
            TilingLayout::Float => "Float",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            TilingLayout::Bsp => "BSP Tiling — balanced recursive split",
            TilingLayout::Columns => "Columns — side-by-side vertical panes",
            TilingLayout::Rows => "Rows — stacked horizontal panes",
            TilingLayout::Grid => "Grid — balanced multi-column tile grid",
            TilingLayout::Focus => "Focus — maximize active pane (Alt+H/L to cycle)",
            TilingLayout::Float => "Float — free-floating draggable & resizable windows",
        }
    }

    pub fn to_id(self) -> u8 {
        match self {
            TilingLayout::Bsp => 0,
            TilingLayout::Columns => 1,
            TilingLayout::Rows => 2,
            TilingLayout::Grid => 3,
            TilingLayout::Focus => 4,
            TilingLayout::Float => 5,
        }
    }

    pub fn from_id(id: u8) -> Self {
        match id {
            1 => TilingLayout::Columns,
            2 => TilingLayout::Rows,
            3 => TilingLayout::Grid,
            4 => TilingLayout::Focus,
            5 => TilingLayout::Float,
            _ => TilingLayout::Bsp,
        }
    }
}

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
    TailscaleServices {
        filter: String,
        local_port_input: String,
        https_port_input: String,
    },
    FileTracker(Box<TrackerWindow>),
    Vitals(Box<VitalsWindow>),
    Help(HelpWindow),
    Settings(SettingsWindow),
    Copilot(Box<CopilotWindow>),
    DebugPanel(DebugPanelWindow),
}

pub struct BeTab {
    pub id: u64,
    pub title: String,
    pub kind: PaneKind,
}

pub struct BeWindowGroup {
    pub id: u64,
    pub workspace: usize,
    pub floating: bool,
    pub pos: Pos2,
    pub size: Vec2,
    pub tab_offset: f32,
    pub active_tab: usize,
    pub tabs: Vec<BeTab>,
    pub shaded: bool,
}

pub struct BeWindowManager {
    pub groups: Vec<BeWindowGroup>,
    pub active_workspace: usize,
    pub workspace_layouts: [TilingLayout; 4],
    pub focused_group: Option<u64>,
    next_id: u64,
}

impl BeWindowManager {
    pub fn new() -> Self {
        Self {
            groups: Vec::new(),
            active_workspace: 0,
            workspace_layouts: [TilingLayout::Bsp; 4],
            focused_group: None,
            next_id: 1,
        }
    }

    pub fn active_layout(&self) -> TilingLayout {
        self.workspace_layouts[self.active_workspace.min(3)]
    }

    pub fn set_active_layout(&mut self, layout: TilingLayout) {
        let ws = self.active_workspace.min(3);
        let prev = self.workspace_layouts[ws];
        self.workspace_layouts[ws] = layout;
        if layout != TilingLayout::Float {
            for g in &mut self.groups {
                if !g.floating && g.workspace == ws {
                    g.shaded = false;
                }
            }
        } else if prev == TilingLayout::Focus {
            // Cascade overlapping Focus-mode panes slightly when entering Float mode
            let mut offset = 0.0;
            for g in &mut self.groups {
                if !g.floating && g.workspace == ws {
                    g.pos.x += offset;
                    g.pos.y += offset;
                    g.size = Vec2::new(
                        (g.size.x * 0.72).max(480.0),
                        (g.size.y * 0.72).max(320.0),
                    );
                    offset += 26.0;
                }
            }
        }
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn open_window(&mut self, title: String, kind: PaneKind, pos: Pos2, size: Vec2) -> u64 {
        self.open_window_in_workspace(self.active_workspace, false, title, kind, pos, size)
    }

    pub fn open_window_in_workspace(
        &mut self,
        workspace: usize,
        floating: bool,
        title: String,
        kind: PaneKind,
        pos: Pos2,
        size: Vec2,
    ) -> u64 {
        let id = self.alloc_id();
        self.groups.push(BeWindowGroup {
            id,
            workspace,
            floating,
            pos,
            size,
            tab_offset: 0.0,
            active_tab: 0,
            tabs: vec![BeTab { id, title, kind }],
            shaded: false,
        });
        if workspace == self.active_workspace || floating {
            self.focused_group = Some(id);
        }
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
                    let ws = self.groups[g_idx].workspace;
                    if !self.groups[g_idx].floating {
                        self.active_workspace = ws;
                    }
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
                    let ws = self.groups[g_idx].workspace;
                    if !self.groups[g_idx].floating {
                        self.active_workspace = ws;
                    }
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
            if !group.floating {
                self.active_workspace = group.workspace;
            }
            self.focused_group = Some(group.id);
            self.groups.push(group);
        }
    }

    pub fn toggle_pin_active(&mut self) {
        if let Some(gid) = self.focused_group
            && let Some(g) = self.groups.iter_mut().find(|g| g.id == gid)
        {
            g.floating = !g.floating;
            if !g.floating {
                g.workspace = self.active_workspace;
            }
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

    pub fn tile_windows(&mut self, _screen: Rect, _sidebar_width: f32) {
        for group in &mut self.groups {
            if group.workspace == self.active_workspace {
                group.floating = false;
                group.shaded = false;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        sidebar_width: f32,
        modal_open: bool,
        nixos: Option<&NixosInfo>,
        units: &[SystemdUnitInfo],
        selected_unit_status: &mut Option<SystemdUnitStatus>,
        containers: &[ContainerInfo],
        dashboard: Option<&DashboardSnapshot>,
        appdata: Option<&AppDataSnapshot>,
        copilot_status: Option<&CopilotStatus>,
        debug_logs: &mut VecDeque<DebugLogEntry>,
        cpu_history: &[f32],
        mem_history: &[f32],
        send_msg: &mut dyn FnMut(ClientMsg),
    ) -> (Vec<InspectorSpawnRequest>, Vec<CopilotCrossAction>) {
        let p = palette();
        let screen = ctx.content_rect();
        let is_mobile = screen.width() < 768.0;

        // Keyboard workspace switching (Alt+1..4) and pane navigation (Alt+H/J/K/L)
        if !modal_open && !is_dom_overlay_focused() {
            ctx.input(|i| {
                if i.modifiers.alt {
                    if i.key_pressed(Key::Num1) {
                        self.active_workspace = 0;
                    } else if i.key_pressed(Key::Num2) {
                        self.active_workspace = 1;
                    } else if i.key_pressed(Key::Num3) {
                        self.active_workspace = 2;
                    } else if i.key_pressed(Key::Num4) {
                        self.active_workspace = 3;
                    } else if i.key_pressed(Key::L) || i.key_pressed(Key::J) || i.key_pressed(Key::ArrowRight) {
                        self.cycle_focus(1);
                    } else if i.key_pressed(Key::H) || i.key_pressed(Key::K) || i.key_pressed(Key::ArrowLeft) {
                        self.cycle_focus(-1);
                    }
                }
            });
        }

        // Compute stage rect for BSP tiling (respecting topbar, optional sidebar, right desktop icons)
        let left_margin = if is_mobile {
            4.0
        } else if sidebar_width > 0.0 {
            sidebar_width + 18.0
        } else {
            8.0
        };
        let right_margin = if is_mobile {
            4.0
        } else if screen.width() >= 1120.0 {
            98.0
        } else {
            8.0
        };
        let bottom_margin = if is_mobile { 38.0 } else { 8.0 };
        let stage_rect = Rect::from_min_max(
            Pos2::new(screen.min.x + left_margin, screen.min.y + TOPBAR_HEIGHT + 8.0),
            Pos2::new(
                (screen.max.x - right_margin).max(screen.min.x + left_margin + 240.0),
                (screen.max.y - bottom_margin).max(screen.min.y + TOPBAR_HEIGHT + 180.0),
            ),
        );

        let tab_height = 22.0;

        // Assign deterministic layout rects to all non-floating groups in the active workspace
        let active_layout = self.active_layout();
        let is_focus_mode = is_mobile || active_layout == TilingLayout::Focus;
        let is_float_mode = !is_mobile && active_layout == TilingLayout::Float;

        let tiled_indices: Vec<usize> = self
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| !g.floating && g.workspace == self.active_workspace)
            .map(|(idx, _)| idx)
            .collect();

        let focus_visible_tiled_id = if is_focus_mode {
            let focused_tiled = self.focused_group.filter(|gid| {
                tiled_indices.iter().any(|&idx| self.groups[idx].id == *gid)
            });
            let chosen = focused_tiled.or_else(|| tiled_indices.last().map(|&idx| self.groups[idx].id));
            if let Some(cid) = chosen {
                if is_mobile {
                    self.focused_group = Some(cid);
                }
                if let Some(g) = self.groups.iter_mut().find(|g| g.id == cid) {
                    g.pos = stage_rect.min;
                    g.size = Vec2::new(stage_rect.width(), (stage_rect.height() - tab_height).max(160.0));
                    g.shaded = false;
                }
            }
            chosen
        } else {
            if !is_float_mode && !tiled_indices.is_empty() {
                // Keep tile slot positions stable by group ID so focusing/raising a window
                // in z-order never swaps its physical slot in BSP/Cols/Rows/Grid layouts.
                let mut slot_indices = tiled_indices.clone();
                slot_indices.sort_by_key(|&idx| self.groups[idx].id);
                let rects = compute_layout_rects(active_layout, stage_rect, slot_indices.len());
                for (slot, &g_idx) in slot_indices.iter().enumerate() {
                    if let Some(r) = rects.get(slot) {
                        self.groups[g_idx].pos = r.min;
                        self.groups[g_idx].size =
                            Vec2::new(r.width().max(240.0), (r.height() - tab_height).max(120.0));
                        self.groups[g_idx].tab_offset = 0.0;
                    }
                }
            }
            None
        };

        let mut spawn_requests = Vec::new();
        let mut copilot_actions = Vec::new();
        let mut copilot_visible_this_frame = false;
        let any_dragging = ctx.input(|i| i.pointer.any_down());
        let mut merge_req: Option<(u64, usize, u64)> = None;
        let mut detach_req: Option<(u64, usize, Pos2)> = None;
        let mut close_req: Option<(u64, usize)> = None;
        let mut bring_to_front: Option<u64> = None;

        // Detect clicks anywhere inside a window (tab bar OR body) to select and raise it to top.
        if !modal_open
            && ctx.input(|i| i.pointer.any_pressed())
            && let Some(ptr) = ctx.input(|i| i.pointer.interact_pos())
            && ptr.y > TOPBAR_HEIGHT
            && ptr.x > sidebar_width
        {
            // Check floating (Order::Foreground) windows first, then workspace windows (Order::Middle),
            // iterating top-to-bottom (reverse of `self.groups`).
            for pass_floating in [true, false] {
                if bring_to_front.is_some() {
                    break;
                }
                for g in self.groups.iter().rev() {
                    if g.floating != pass_floating {
                        continue;
                    }
                    if !g.floating && g.workspace != self.active_workspace {
                        continue;
                    }
                    if is_mobile && Some(g.id) != self.focused_group {
                        continue;
                    }
                    if !is_mobile
                        && is_focus_mode
                        && !g.floating
                        && Some(g.id) != focus_visible_tiled_id
                    {
                        continue;
                    }
                    let footprint = if g.shaded {
                        Rect::from_min_size(g.pos, Vec2::new(g.size.x, tab_height + 2.0))
                    } else {
                        Rect::from_min_size(g.pos, Vec2::new(g.size.x, tab_height + g.size.y))
                    };
                    if footprint.contains(ptr) {
                        bring_to_front = Some(g.id);
                        break;
                    }
                }
            }
        }

        let header_zones: Vec<(u64, Rect)> = self
            .groups
            .iter()
            .filter(|g| g.floating || g.workspace == self.active_workspace)
            .map(|g| (g.id, Rect::from_min_size(g.pos, Vec2::new(g.size.x, tab_height + 2.0))))
            .collect();

        let focused_id = bring_to_front.or(self.focused_group);
        let mut rendered_layers: Vec<(u64, bool, egui::LayerId)> = Vec::new();

        for group in &mut self.groups {
            if !group.floating && group.workspace != self.active_workspace {
                continue;
            }
            if is_mobile && Some(group.id) != focused_id {
                continue;
            }
            if !is_mobile && is_focus_mode && !group.floating && Some(group.id) != focus_visible_tiled_id {
                continue;
            }

            let is_focused = Some(group.id) == focused_id;
            let body_pos = Pos2::new(group.pos.x, group.pos.y + tab_height);
            let area_order = if group.floating {
                egui::Order::Foreground
            } else {
                egui::Order::Middle
            };

            let area_resp = egui::Area::new(Id::unique(("be_group", group.id)))
                .fixed_pos(group.pos)
                .order(area_order)
                .show(ctx, |ui| {
                    // Register the full window footprint with the Area so egui's layer hit-testing
                    // (ScrollArea mouse-wheel, hover, and pointer routing) recognizes this window.
                    let footprint = if group.shaded {
                        Rect::from_min_size(group.pos, Vec2::new(group.size.x, tab_height + 2.0))
                    } else {
                        Rect::from_min_size(
                            group.pos,
                            Vec2::new(group.size.x, tab_height + group.size.y),
                        )
                    };
                    ui.expand_to_include_rect(footprint);

                    let mut cursor_x = group.pos.x + group.tab_offset;

                    for (idx, tab) in group.tabs.iter().enumerate() {
                        let is_active = idx == group.active_tab;
                        let pin_extra = if is_active { 26.0 } else { 0.0 };
                        let tab_width = (tab.title.len() as f32 * 7.4 + 42.0 + pin_extra).max(96.0);
                        let tab_rect = Rect::from_min_size(
                            Pos2::new(cursor_x, group.pos.y),
                            Vec2::new(tab_width, tab_height + 1.0),
                        );

                        let fill = if is_active {
                            p.tab_active_bg
                        } else {
                            p.tab_inactive_bg
                        };
                        {
                            let painter = ui.painter();
                            painter.rect_filled(tab_rect, CornerRadius::ZERO, fill);
                            draw_beos_bevel(painter, tab_rect, false);
                            if is_active && p.is_dark {
                                painter.rect_filled(
                                    Rect::from_min_size(tab_rect.min, Vec2::new(tab_rect.width(), 2.0)),
                                    CornerRadius::ZERO,
                                    p.accent,
                                );
                            }
                        }

                        // Left Close Box (non-overlapping hit rect)
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
                                p.status_red,
                            );
                        }
                        draw_beos_bevel(ui.painter(), close_rect, close_pressed);
                        if close_resp.hovered() {
                            let c = close_rect.center();
                            let d = 2.5;
                            let stroke = Stroke::new(1.2, Color32::WHITE);
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

                        // Right Pin-to-Float (PiP) Toggle on active tab
                        let mut drag_right_x = tab_rect.max.x;
                        if is_active && !is_mobile {
                            let pin_rect = Rect::from_min_size(
                                Pos2::new(tab_rect.max.x - 24.0, tab_rect.min.y + 4.0),
                                Vec2::new(20.0, 14.0),
                            );
                            let pin_resp = ui
                                .interact(
                                    pin_rect,
                                    Id::unique(("be_pin", group.id, tab.id)),
                                    Sense::click(),
                                )
                                .on_hover_text(if group.floating {
                                    "Snap back into BSP tiling"
                                } else {
                                    "Pin as floating PiP overlay (stays across workspaces)"
                                });
                            ui.painter().rect_filled(
                                pin_rect,
                                CornerRadius::ZERO,
                                if group.floating { p.accent } else { p.widget },
                            );
                            ui.painter().text(
                                pin_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                if group.floating { "PIP" } else { "FLT" },
                                egui::FontId::monospace(8.5),
                                if group.floating { Color32::BLACK } else { p.text_muted },
                            );
                            if pin_resp.clicked() {
                                group.floating = !group.floating;
                                if !group.floating {
                                    group.workspace = self.active_workspace;
                                }
                            }
                            drag_right_x = pin_rect.min.x - 2.0;
                        }

                        // Tab Title
                        ui.painter().text(
                            Pos2::new(close_rect.max.x + 6.0, tab_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &tab.title,
                            egui::FontId::proportional(12.5),
                            if is_active { p.tab_active_fg } else { p.text_muted },
                        );

                        let tab_drag_rect = Rect::from_min_max(
                            Pos2::new(close_hit_rect.max.x + 1.0, tab_rect.min.y),
                            Pos2::new(drag_right_x.max(close_hit_rect.max.x + 4.0), tab_rect.max.y),
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
                        if tab_resp.double_clicked() && (group.floating || is_float_mode) {
                            group.shaded = !group.shaded;
                        }
                        if tab_resp.dragged() {
                            bring_to_front = Some(group.id);
                            let delta = tab_resp.drag_delta();
                            if group.tabs.len() > 1 && delta.length() > 14.0 {
                                if let Some(ptr) = ui.input(|i| i.pointer.interact_pos()) {
                                    detach_req = Some((group.id, idx, ptr));
                                }
                            } else if group.floating || is_float_mode {
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
                            painter.rect_filled(body_rect, CornerRadius::ZERO, p.surface);
                            draw_beos_bevel(painter, body_rect, false);
                            if is_focused && p.is_dark {
                                painter.rect_stroke(
                                    body_rect,
                                    CornerRadius::ZERO,
                                    Stroke::new(1.0, p.accent),
                                    egui::StrokeKind::Inside,
                                );
                            }
                        }

                        let inner_rect = body_rect.shrink(5.0);
                        let mut child_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(inner_rect)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        child_ui.set_clip_rect(inner_rect);

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
                                PaneKind::TailscaleServices {
                                    filter,
                                    local_port_input,
                                    https_port_input,
                                } => {
                                    render_tailscale_services(
                                        &mut child_ui,
                                        filter,
                                        local_port_input,
                                        https_port_input,
                                        dashboard,
                                        send_msg,
                                    );
                                }
                                PaneKind::FileTracker(tracker) => {
                                    if let Some(req) = tracker.ui(&mut child_ui, send_msg) {
                                        match req {
                                            TrackerSpawnRequest::OpenTerminalInDir { title, dir } => {
                                                let safe_dir = dir.replace('\'', "'\\''");
                                                spawn_requests.push(
                                                    InspectorSpawnRequest::OpenCommandTab {
                                                        title,
                                                        cmd: vec![
                                                            "sh".into(),
                                                            "-c".into(),
                                                            format!(
                                                                "cd '{safe_dir}' && exec \"${{SHELL:-/bin/sh}}\""
                                                            ),
                                                        ],
                                                    },
                                                );
                                            }
                                            TrackerSpawnRequest::OpenUnitLogs { unit } => {
                                                spawn_requests.push(
                                                    InspectorSpawnRequest::OpenCommandTab {
                                                        title: format!("logs: {unit}"),
                                                        cmd: vec![
                                                            "journalctl".into(),
                                                            "-u".into(),
                                                            unit,
                                                            "-n".into(),
                                                            "100".into(),
                                                            "-f".into(),
                                                        ],
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                                PaneKind::Vitals(vitals) => {
                                    if let Some(req) = vitals.ui(&mut child_ui, send_msg) {
                                        match req {
                                            VitalsSpawnRequest::OpenUnitLogs { unit } => {
                                                spawn_requests.push(
                                                    InspectorSpawnRequest::OpenCommandTab {
                                                        title: format!("logs: {unit}"),
                                                        cmd: vec![
                                                            "journalctl".into(),
                                                            "-u".into(),
                                                            unit,
                                                            "-n".into(),
                                                            "100".into(),
                                                            "-f".into(),
                                                        ],
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                                PaneKind::Help(help) => {
                                    help.ui(&mut child_ui);
                                }
                                PaneKind::Settings(settings) => {
                                    settings.ui(&mut child_ui, appdata, copilot_status, send_msg);
                                }
                                PaneKind::Copilot(copilot) => {
                                    if !modal_open {
                                        copilot_visible_this_frame = true;
                                        let z = if group.floating {
                                            600
                                        } else if is_focused {
                                            500
                                        } else {
                                            450
                                        };
                                        let acts = copilot.ui(
                                            &mut child_ui,
                                            inner_rect,
                                            copilot_status,
                                            any_dragging,
                                            z,
                                        );
                                        copilot_actions.extend(acts);
                                    }
                                }
                                PaneKind::DebugPanel(dbg) => {
                                    dbg.ui(&mut child_ui, debug_logs, send_msg);
                                }
                            }
                        }

                        // Resize grip for floating PiP windows or Float layout mode
                        if group.floating || is_float_mode {
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
                                    .max(Vec2::new(280.0, 160.0));
                            }
                        }
                    }
                });
            rendered_layers.push((group.id, group.floating, area_resp.response.layer_id));
        }

        // Mobile bottom tab pill deck (< 768px)
        if is_mobile {
            let deck_rect = Rect::from_min_size(
                Pos2::new(screen.min.x, screen.max.y - 34.0),
                Vec2::new(screen.width(), 34.0),
            );
            let mut mobile_select = None;
            egui::Area::new(Id::unique("kaage_mobile_deck"))
                .fixed_pos(deck_rect.min)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    ui.painter().rect_filled(deck_rect, CornerRadius::ZERO, p.surface);
                    draw_beos_bevel(ui.painter(), deck_rect, false);
                    let mut x = deck_rect.min.x + 6.0;
                    for g in &self.groups {
                        if !g.floating && g.workspace != self.active_workspace {
                            continue;
                        }
                        if let Some(tab) = g.tabs.get(g.active_tab) {
                            let w = (tab.title.len() as f32 * 6.5 + 16.0).clamp(64.0, 128.0);
                            let r = Rect::from_min_size(Pos2::new(x, deck_rect.min.y + 5.0), Vec2::new(w, 24.0));
                            let active = Some(g.id) == focused_id;
                            ui.painter().rect_filled(
                                r,
                                CornerRadius::ZERO,
                                if active { p.tab_active_bg } else { p.widget },
                            );
                            if active {
                                ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
                            }
                            ui.painter().text(
                                r.center(),
                                egui::Align2::CENTER_CENTER,
                                &tab.title,
                                egui::FontId::proportional(11.0),
                                if active { p.tab_active_fg } else { p.text },
                            );
                            if ui.interact(r, Id::unique(("mobile_pill", g.id)), Sense::click()).clicked() {
                                mobile_select = Some(g.id);
                            }
                            x = r.max.x + 4.0;
                        }
                    }
                });
            if let Some(gid) = mobile_select {
                self.focused_group = Some(gid);
            }
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
        if let Some(gid) = bring_to_front {
            self.focused_group = Some(gid);
            if let Some(pos) = self.groups.iter().position(|g| g.id == gid) {
                let g = self.groups.remove(pos);
                self.groups.push(g);
            }
        }

        // Sync egui's internal Area layer stack to match `self.groups` z-order so the
        // selected/focused window is ALWAYS rendered on top of all sibling windows.
        for g in &self.groups {
            if let Some(&(_, _, layer_id)) = rendered_layers.iter().find(|(id, _, _)| *id == g.id) {
                ctx.move_to_top(layer_id);
            }
        }
        if let Some(fgid) = self.focused_group
            && let Some(&(_, _, layer_id)) = rendered_layers.iter().find(|(id, _, _)| *id == fgid)
        {
            ctx.move_to_top(layer_id);
        }

        if !copilot_visible_this_frame || modal_open {
            hide_copilot_overlay();
        }

        (spawn_requests, copilot_actions)
    }

    fn cycle_focus(&mut self, dir: isize) {
        let visible: Vec<u64> = self
            .groups
            .iter()
            .filter(|g| g.floating || g.workspace == self.active_workspace)
            .map(|g| g.id)
            .collect();
        if visible.is_empty() {
            return;
        }
        let cur = self
            .focused_group
            .and_then(|id| visible.iter().position(|&v| v == id))
            .unwrap_or(0);
        let next = (cur as isize + dir).rem_euclid(visible.len() as isize) as usize;
        self.focused_group = Some(visible[next]);
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
            self.focused_group = Some(dst.id);
        }
    }

    fn detach_tab(&mut self, src_id: u64, tab_idx: usize, pos: Pos2) {
        let Some(src) = self.groups.iter_mut().find(|g| g.id == src_id) else {
            return;
        };
        if src.tabs.len() <= 1 || tab_idx >= src.tabs.len() {
            return;
        }
        let ws = src.workspace;
        let tab = src.tabs.remove(tab_idx);
        src.active_tab = src.active_tab.min(src.tabs.len() - 1);
        let new_group_id = self.alloc_id();
        self.groups.push(BeWindowGroup {
            id: new_group_id,
            workspace: ws,
            floating: false,
            pos,
            size: Vec2::new(620.0, 380.0),
            tab_offset: 0.0,
            active_tab: 0,
            tabs: vec![tab],
            shaded: false,
        });
        self.focused_group = Some(new_group_id);
    }
}

/// Compute deterministic tiling rects for the active predefined layout.
fn compute_layout_rects(layout: TilingLayout, area: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![area];
    }
    let gap = 6.0;
    match layout {
        TilingLayout::Bsp | TilingLayout::Focus | TilingLayout::Float => {
            compute_bsp_rects(area, count)
        }
        TilingLayout::Columns => {
            let col_w = ((area.width() - gap * (count - 1) as f32) / count as f32).max(120.0);
            (0..count)
                .map(|i| {
                    let x = area.min.x + i as f32 * (col_w + gap);
                    let max_x = if i + 1 == count {
                        area.max.x
                    } else {
                        x + col_w
                    };
                    Rect::from_min_max(Pos2::new(x, area.min.y), Pos2::new(max_x, area.max.y))
                })
                .collect()
        }
        TilingLayout::Rows => {
            let row_h = ((area.height() - gap * (count - 1) as f32) / count as f32).max(96.0);
            (0..count)
                .map(|i| {
                    let y = area.min.y + i as f32 * (row_h + gap);
                    let max_y = if i + 1 == count {
                        area.max.y
                    } else {
                        y + row_h
                    };
                    Rect::from_min_max(Pos2::new(area.min.x, y), Pos2::new(area.max.x, max_y))
                })
                .collect()
        }
        TilingLayout::Grid => {
            let cols = (count as f32).sqrt().ceil() as usize;
            let rows = count.div_ceil(cols);
            let row_h = ((area.height() - gap * (rows - 1) as f32) / rows as f32).max(96.0);
            let mut out = Vec::with_capacity(count);
            for r in 0..rows {
                let items_in_row = if r + 1 == rows {
                    count - r * cols
                } else {
                    cols
                };
                let col_w = ((area.width() - gap * (items_in_row - 1) as f32)
                    / items_in_row as f32)
                    .max(120.0);
                let y = area.min.y + r as f32 * (row_h + gap);
                let max_y = if r + 1 == rows { area.max.y } else { y + row_h };
                for c in 0..items_in_row {
                    let x = area.min.x + c as f32 * (col_w + gap);
                    let max_x = if c + 1 == items_in_row {
                        area.max.x
                    } else {
                        x + col_w
                    };
                    out.push(Rect::from_min_max(Pos2::new(x, y), Pos2::new(max_x, max_y)));
                }
            }
            out
        }
    }
}

/// Deterministic recursive Binary-Space Partitioning (BSP) layout for `count` panes.
fn compute_bsp_rects(area: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![area];
    }
    let gap = 6.0;
    let mut out = Vec::with_capacity(count);
    let mut rem = area;
    for i in 0..count {
        if i + 1 == count {
            out.push(rem);
            break;
        }
        let split_horizontal = rem.width() >= rem.height();
        if split_horizontal {
            let ratio = if i == 0 && count == 2 { 0.54 } else { 0.5 };
            let split_x = rem.min.x + (rem.width() - gap) * ratio;
            out.push(Rect::from_min_max(rem.min, Pos2::new(split_x, rem.max.y)));
            rem = Rect::from_min_max(Pos2::new(split_x + gap, rem.min.y), rem.max);
        } else {
            let split_y = rem.min.y + (rem.height() - gap) * 0.5;
            out.push(Rect::from_min_max(rem.min, Pos2::new(rem.max.x, split_y)));
            rem = Rect::from_min_max(Pos2::new(rem.min.x, split_y + gap), rem.max);
        }
    }
    out
}
