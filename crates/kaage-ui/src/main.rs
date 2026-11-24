mod clock;
mod deskbar;
mod desktop_icons;
mod file_tracker;
mod help;
mod inspectors;
mod palette;
mod panels;
mod session;
mod settings;
mod terminal;
mod theme;
mod tracker;
mod vitals;
mod wm;

use deskbar::{BeDeskbar, DeskbarAction};
use desktop_icons::{DesktopIconAction, show_desktop_icons};
use eframe::egui::{self, Color32, CornerRadius, Id, Pos2, Rect, Stroke, Vec2};
use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use file_tracker::TrackerWindow;
use help::HelpWindow;
use inspectors::{InspectorSpawnRequest, ScopeFilter, StateFilter};
use kaage_proto::{
    AppDataSnapshot, ClientMsg, ContainerInfo, DashboardSnapshot, FileOp, HostMetrics, NixosInfo,
    ServerMsg, SystemdUnitInfo, SystemdUnitStatus, UnitAction, VitalsTick,
};
use palette::{CommandPalette, PaletteAction};
use session::{SavedSession, load_session, save_session_bytes};
use settings::SettingsWindow;
use terminal::TerminalPane;
use theme::{
    TOPBAR_HEIGHT, ThemeMode, apply_theme, apply_user_settings, configure_fonts, current_theme_mode,
    draw_beos_bevel, draw_dot_grid, palette as active_palette, user_settings,
};
use tracker::{BeTrackerSidebar, TrackerAction};
use vitals::VitalsWindow;
use wm::{BeWindowManager, PaneKind};

const WALLPAPER_BYTES: &[u8] = include_bytes!("../../../assets/kaage-wallpaper.jpg");
const MAX_HISTORY_SAMPLES: usize = 60;

#[cfg(target_arch = "wasm32")]
static WASM_PALETTE_TRIGGER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Attach a capture-phase DOM `keydown` listener on `window` for Cmd+/ (macOS) and Ctrl+` (other OSes)
/// so the Command HUD opens reliably in Firefox, Edge, Safari, and Chrome even inside focused terminals.
#[cfg(target_arch = "wasm32")]
fn install_wasm_palette_shortcut_listener(ctx: egui::Context) {
    use std::sync::atomic::Ordering;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let Some(window) = web_sys::window() else {
        return;
    };

    let on_keydown = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
        move |e: web_sys::KeyboardEvent| {
            let cmd_slash = e.meta_key()
                && !e.ctrl_key()
                && !e.alt_key()
                && (e.key() == "/" || e.code() == "Slash");
            let ctrl_backtick = e.ctrl_key()
                && !e.meta_key()
                && !e.alt_key()
                && (e.key() == "`" || e.code() == "Backquote");

            if (cmd_slash || ctrl_backtick) && !e.repeat() {
                e.prevent_default();
                e.stop_propagation();
                WASM_PALETTE_TRIGGER.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            }
        },
    );
    let _ = window.add_event_listener_with_callback_and_bool(
        "keydown",
        on_keydown.as_ref().unchecked_ref(),
        true,
    );
    on_keydown.forget();
}

struct ToastNotice {
    ok: bool,
    text: String,
    expires_at: f64,
    undo_msg: Option<ClientMsg>,
}

struct KaageApp {
    ws_tx: WsSender,
    ws_rx: WsReceiver,
    connected: bool,
    theme_mode: ThemeMode,
    show_wallpaper: bool,
    wallpaper: Option<egui::TextureHandle>,
    wm: BeWindowManager,
    deskbar: BeDeskbar,
    tracker: BeTrackerSidebar,
    cmd_palette: CommandPalette,
    nixos: Option<NixosInfo>,
    metrics: Option<HostMetrics>,
    latest_vital: Option<VitalsTick>,
    dashboard: Option<DashboardSnapshot>,
    appdata: Option<AppDataSnapshot>,
    cpu_history: Vec<f32>,
    mem_history: Vec<f32>,
    units: Vec<SystemdUnitInfo>,
    selected_unit_status: Option<SystemdUnitStatus>,
    containers: Vec<ContainerInfo>,
    next_pty_id: u32,
    next_tracker_pane_id: u64,
    pending_startup_msgs: Vec<ClientMsg>,
    last_saved_session: Vec<u8>,
    last_session_save_at: f64,
    pending_undo: Option<ClientMsg>,
    toast: Option<ToastNotice>,
}

fn load_wallpaper_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory_with_format(WALLPAPER_BYTES, image::ImageFormat::Jpeg)
        .ok()?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(
        "kaage_wallpaper",
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

impl KaageApp {
    fn new(cc: &eframe::CreationContext<'_>, ws_url: &str) -> Self {
        let saved = load_session();
        let (theme_mode, show_wallpaper) = if let Some(ref s) = saved {
            let us = s.to_user_settings();
            apply_user_settings(&cc.egui_ctx, us, true);
            (us.theme_mode, s.show_wallpaper)
        } else {
            configure_fonts(&cc.egui_ctx);
            let mode = ThemeMode::ObsidianAmber;
            apply_theme(&cc.egui_ctx, mode);
            (mode, true)
        };
        let wallpaper = load_wallpaper_texture(&cc.egui_ctx);
        #[cfg(target_arch = "wasm32")]
        install_wasm_palette_shortcut_listener(cc.egui_ctx.clone());

        let ctx_clone = cc.egui_ctx.clone();
        let (ws_tx, ws_rx) = ewebsock::connect_with_wakeup(
            ws_url,
            ewebsock::Options::default(),
            move || ctx_clone.request_repaint(),
        )
        .expect("Failed to open WebSocket connection");

        let mut tracker = BeTrackerSidebar::new();
        let mut wm = BeWindowManager::new();
        let mut next_pty_id = 1u32;
        let mut next_tracker_pane_id = 1u64;
        let mut pending_startup_msgs = Vec::new();

        if let Some(ref s) = saved
            && !s.groups.is_empty()
        {
            tracker.visible = s.tracker_visible;
            tracker.wide = s.tracker_wide;
            pending_startup_msgs = s.restore_wm(&mut wm, &mut next_pty_id, &mut next_tracker_pane_id);
        } else {
            let stage_x = tracker.width() + 24.0;

            // Workspace 0 ("1: Ops"): Systemd Services + Terminal shell (BSP tiled side-by-side)
            let ops_gid = wm.open_window_in_workspace(
                0,
                false,
                "Systemd Services".into(),
                PaneKind::SystemdInspector {
                    filter: String::new(),
                    scope_filter: ScopeFilter::All,
                    state_filter: StateFilter::All,
                },
                Pos2::new(stage_x, TOPBAR_HEIGHT + 12.0),
                Vec2::new(720.0, 420.0),
            );
            wm.add_tab_to_group(
                ops_gid,
                "Tailscale Services".into(),
                PaneKind::TailscaleServices {
                    filter: String::new(),
                    local_port_input: String::new(),
                    https_port_input: String::new(),
                },
            );
            wm.add_tab_to_group(ops_gid, "HTTP Ports".into(), PaneKind::HttpPorts { filter: String::new() });
            wm.add_tab_to_group(ops_gid, "Tailscale".into(), PaneKind::TailscalePanel);

            let initial_pty = next_pty_id;
            next_pty_id += 1;
            wm.open_window_in_workspace(
                0,
                false,
                "Terminal — shell".into(),
                PaneKind::Terminal(TerminalPane::new(initial_pty, 80, 24)),
                Pos2::new(stage_x + 24.0, TOPBAR_HEIGHT + 464.0),
                Vec2::new(700.0, 320.0),
            );
            pending_startup_msgs.push(ClientMsg::SpawnPty {
                session_id: initial_pty,
                cols: 80,
                rows: 24,
                cmd: None,
            });

            // Workspace 1 ("2: AI"): AI Jobs + GPU/CPU/Mem Monitor
            wm.open_window_in_workspace(
                1,
                false,
                "AI Jobs".into(),
                PaneKind::AiJobs,
                Pos2::new(stage_x, TOPBAR_HEIGHT + 12.0),
                Vec2::new(620.0, 420.0),
            );
            wm.open_window_in_workspace(
                1,
                false,
                "GPU / CPU / Mem".into(),
                PaneKind::SystemMonitor,
                Pos2::new(stage_x, TOPBAR_HEIGHT + 12.0),
                Vec2::new(620.0, 420.0),
            );

            wm.active_workspace = 0;
        }

        let initial_saved = SavedSession::capture(
            &wm,
            tracker.visible,
            tracker.wide,
            show_wallpaper,
            user_settings(),
        );
        let last_saved_session = postcard::to_allocvec(&initial_saved).unwrap_or_default();

        Self {
            ws_tx,
            ws_rx,
            connected: false,
            theme_mode,
            show_wallpaper,
            wallpaper,
            wm,
            deskbar: BeDeskbar::new(),
            tracker,
            cmd_palette: CommandPalette::new(),
            nixos: None,
            metrics: None,
            latest_vital: None,
            dashboard: None,
            appdata: None,
            cpu_history: Vec::with_capacity(MAX_HISTORY_SAMPLES),
            mem_history: Vec::with_capacity(MAX_HISTORY_SAMPLES),
            units: Vec::new(),
            selected_unit_status: None,
            containers: Vec::new(),
            next_pty_id,
            next_tracker_pane_id,
            pending_startup_msgs,
            last_saved_session,
            last_session_save_at: 0.0,
            pending_undo: None,
            toast: None,
        }
    }

    fn send_client_msg(&mut self, msg: ClientMsg) {
        // Track reversible actions for the interactive Undo button on ActionFeedback toasts
        match &msg {
            ClientMsg::ControlUnit {
                unit,
                scope,
                action: UnitAction::Stop,
            } => {
                self.pending_undo = Some(ClientMsg::ControlUnit {
                    unit: unit.clone(),
                    scope: *scope,
                    action: UnitAction::Start,
                });
            }
            ClientMsg::PerformFileOp {
                pane_id,
                op: FileOp::Trash { .. } | FileOp::Move { .. } | FileOp::Rename { .. },
            } => {
                self.pending_undo = Some(ClientMsg::UndoLastFileOp { pane_id: *pane_id });
            }
            _ => {}
        }
        if let Ok(bytes) = postcard::to_allocvec(&msg) {
            self.ws_tx.send(WsMessage::Binary(bytes));
        }
    }

    fn send_initial_sync(&mut self) {
        self.send_client_msg(ClientMsg::ListUnits);
        self.send_client_msg(ClientMsg::ListContainers);
        self.send_client_msg(ClientMsg::RefreshDashboard);
        self.send_client_msg(ClientMsg::GetAppDataState);
        let startup = std::mem::take(&mut self.pending_startup_msgs);
        for msg in startup {
            self.send_client_msg(msg);
        }
    }

    fn stage_origin(&self, offset_x: f32, offset_y: f32) -> Pos2 {
        let base_x = if self.tracker.visible {
            self.tracker.width() + 24.0
        } else {
            24.0
        };
        Pos2::new(base_x + offset_x, TOPBAR_HEIGHT + 16.0 + offset_y)
    }

    fn spawn_terminal_window(&mut self, title: String, cmd: Option<Vec<String>>) {
        let session_id = self.next_pty_id;
        self.next_pty_id += 1;

        self.wm.open_window(
            title,
            PaneKind::Terminal(TerminalPane::new(session_id, 80, 24)),
            self.stage_origin(
                18.0 + ((session_id % 6) as f32 * 22.0),
                32.0 + ((session_id % 6) as f32 * 18.0),
            ),
            Vec2::new(680.0, 400.0),
        );

        self.send_client_msg(ClientMsg::SpawnPty {
            session_id,
            cols: 80,
            rows: 24,
            cmd,
        });
    }

    fn open_containers_window(&mut self) {
        if !self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::DockerInspector { .. }))
        {
            self.wm.open_window(
                "Containers".into(),
                PaneKind::DockerInspector {
                    filter: String::new(),
                },
                self.stage_origin(16.0, 24.0),
                Vec2::new(560.0, 340.0),
            );
        }
        self.send_client_msg(ClientMsg::ListContainers);
    }

    fn open_system_monitor(&mut self) {
        if !self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::SystemMonitor))
        {
            self.wm.open_window(
                "GPU / CPU / Mem".into(),
                PaneKind::SystemMonitor,
                self.stage_origin(20.0, 20.0),
                Vec2::new(640.0, 460.0),
            );
        }
        self.send_client_msg(ClientMsg::RefreshDashboard);
    }

    fn open_ai_jobs(&mut self) {
        if !self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::AiJobs))
        {
            self.wm.open_window(
                "AI Jobs".into(),
                PaneKind::AiJobs,
                self.stage_origin(36.0, 36.0),
                Vec2::new(620.0, 400.0),
            );
        }
        self.send_client_msg(ClientMsg::RefreshDashboard);
    }

    fn open_http_ports(&mut self) {
        if !self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::HttpPorts { .. }))
        {
            self.wm.open_window(
                "HTTP Ports".into(),
                PaneKind::HttpPorts {
                    filter: String::new(),
                },
                self.stage_origin(48.0, 48.0),
                Vec2::new(640.0, 380.0),
            );
        }
        self.send_client_msg(ClientMsg::RefreshDashboard);
    }

    fn open_tailscale(&mut self) {
        if !self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::TailscalePanel))
        {
            self.wm.open_window(
                "Tailscale".into(),
                PaneKind::TailscalePanel,
                self.stage_origin(60.0, 60.0),
                Vec2::new(660.0, 420.0),
            );
        }
        self.send_client_msg(ClientMsg::RefreshDashboard);
    }

    fn open_tailscale_services(&mut self) {
        if !self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::TailscaleServices { .. }))
        {
            self.wm.open_window(
                "Tailscale Services".into(),
                PaneKind::TailscaleServices {
                    filter: String::new(),
                    local_port_input: String::new(),
                    https_port_input: String::new(),
                },
                self.stage_origin(48.0, 40.0),
                Vec2::new(720.0, 500.0),
            );
        }
        self.send_client_msg(ClientMsg::RefreshDashboard);
    }

    fn open_file_tracker(&mut self) {
        if self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::FileTracker(_)))
        {
            return;
        }
        self.wm.active_workspace = 2; // "3: Files"
        let left_id = self.next_tracker_pane_id;
        let right_id = left_id + 1;
        self.next_tracker_pane_id += 2;

        let mut pending = Vec::new();
        let tracker = TrackerWindow::new(left_id, right_id, &mut |msg| pending.push(msg));
        for msg in pending {
            self.send_client_msg(msg);
        }

        self.wm.open_window_in_workspace(
            2,
            false,
            "Tracker — Files".into(),
            PaneKind::FileTracker(Box::new(tracker)),
            self.stage_origin(16.0, 16.0),
            Vec2::new(920.0, 560.0),
        );
    }

    fn open_vitals(&mut self) {
        if self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::Vitals(_)))
        {
            return;
        }
        let mut pending = Vec::new();
        let vitals = VitalsWindow::new(&mut |msg| pending.push(msg));
        for msg in pending {
            self.send_client_msg(msg);
        }
        self.wm.open_window(
            "Kaage Vitals".into(),
            PaneKind::Vitals(Box::new(vitals)),
            self.stage_origin(20.0, 16.0),
            Vec2::new(940.0, 620.0),
        );
    }

    fn open_help(&mut self) {
        if self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::Help(_)))
        {
            return;
        }
        self.wm.open_window(
            "kaage-help".into(),
            PaneKind::Help(HelpWindow::new()),
            self.stage_origin(24.0, 20.0),
            Vec2::new(720.0, 480.0),
        );
    }

    fn open_settings(&mut self) {
        self.send_client_msg(ClientMsg::GetAppDataState);
        if self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::Settings(_)))
        {
            return;
        }
        self.wm.open_window(
            "kaage-settings".into(),
            PaneKind::Settings(SettingsWindow::new()),
            self.stage_origin(28.0, 24.0),
            Vec2::new(760.0, 520.0),
        );
    }

    fn open_appdata_settings(&mut self) {
        self.send_client_msg(ClientMsg::GetAppDataState);
        for group in &mut self.wm.groups {
            for (idx, tab) in group.tabs.iter_mut().enumerate() {
                if let PaneKind::Settings(settings) = &mut tab.kind {
                    settings.tab = settings::SettingsTab::AppData;
                    group.active_tab = idx;
                    self.wm.active_workspace = group.workspace;
                    self.wm.focused_group = Some(group.id);
                    return;
                }
            }
        }
        self.wm.open_window(
            "kaage-settings".into(),
            PaneKind::Settings(SettingsWindow::new_on_appdata()),
            self.stage_origin(28.0, 24.0),
            Vec2::new(760.0, 520.0),
        );
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.theme_mode = current_theme_mode().toggle();
        apply_theme(ctx, self.theme_mode);
    }

    fn push_history(buf: &mut Vec<f32>, val: f32) {
        if buf.len() >= MAX_HISTORY_SAMPLES {
            buf.remove(0);
        }
        buf.push(val);
    }
}

impl eframe::App for KaageApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let p = active_palette();
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        // Global Command Palette shortcut: Cmd+/ (macOS) or Ctrl+` (Linux/Windows).
        #[cfg(target_arch = "wasm32")]
        {
            if WASM_PALETTE_TRIGGER.swap(false, std::sync::atomic::Ordering::Relaxed) {
                self.cmd_palette.toggle();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let triggered = ctx.input(|i| {
                let cmd_slash = i.modifiers.mac_cmd
                    && !i.modifiers.ctrl
                    && !i.modifiers.alt
                    && i.key_pressed(egui::Key::Slash);
                let ctrl_backtick = i.modifiers.ctrl
                    && !i.modifiers.mac_cmd
                    && !i.modifiers.alt
                    && i.key_pressed(egui::Key::Backtick);
                cmd_slash || ctrl_backtick
            });
            if triggered {
                self.cmd_palette.toggle();
            }
        }

        while let Some(event) = self.ws_rx.try_recv() {
            match event {
                WsEvent::Opened => {
                    self.connected = true;
                    self.send_initial_sync();
                }
                WsEvent::Closed => {
                    self.connected = false;
                }
                WsEvent::Error(err) => {
                    self.connected = false;
                    self.toast = Some(ToastNotice {
                        ok: false,
                        text: format!("WebSocket error: {err}"),
                        expires_at: now + 6.0,
                        undo_msg: None,
                    });
                }
                WsEvent::Message(WsMessage::Binary(bytes)) => {
                    if let Ok(server_msg) = postcard::from_bytes::<ServerMsg>(&bytes) {
                        match server_msg {
                            ServerMsg::NixosInfo(info) => self.nixos = Some(info),
                            ServerMsg::Metrics(m) => {
                                Self::push_history(&mut self.cpu_history, m.cpu_percent);
                                if m.mem_total_mb > 0 {
                                    let pct =
                                        (m.mem_used_mb as f32 / m.mem_total_mb as f32) * 100.0;
                                    Self::push_history(&mut self.mem_history, pct);
                                }
                                self.metrics = Some(m);
                            }
                            ServerMsg::Dashboard(d) => {
                                self.dashboard = Some(d);
                            }
                            ServerMsg::VitalsTick(tick) => {
                                self.latest_vital = Some(tick.clone());
                                let mut pending = Vec::new();
                                for group in &mut self.wm.groups {
                                    for tab in &mut group.tabs {
                                        if let PaneKind::Vitals(v) = &mut tab.kind {
                                            v.on_live_tick(tick.clone(), &mut |m| pending.push(m));
                                        }
                                    }
                                }
                                for m in pending {
                                    self.send_client_msg(m);
                                }
                            }
                            ServerMsg::VitalsSnapshot(snap) => {
                                self.latest_vital = Some(snap.latest.clone());
                                for group in &mut self.wm.groups {
                                    for tab in &mut group.tabs {
                                        if let PaneKind::Vitals(v) = &mut tab.kind {
                                            v.on_snapshot(snap.clone());
                                        }
                                    }
                                }
                            }
                            ServerMsg::UnitsSnapshot(u) => self.units = u,
                            ServerMsg::UnitStatus(st) => self.selected_unit_status = Some(st),
                            ServerMsg::ContainersSnapshot(c) => self.containers = c,
                            ServerMsg::PtyOutput { session_id, data } => {
                                for group in &mut self.wm.groups {
                                    for tab in &mut group.tabs {
                                        if let PaneKind::Terminal(term) = &mut tab.kind
                                            && term.session_id == session_id
                                        {
                                            term.feed_bytes(&data);
                                        }
                                    }
                                }
                            }
                            ServerMsg::PtyExited { session_id } => {
                                for group in &mut self.wm.groups {
                                    for tab in &mut group.tabs {
                                        if let PaneKind::Terminal(term) = &mut tab.kind
                                            && term.session_id == session_id
                                        {
                                            term.feed_bytes(b"\r\n[Process exited]\r\n");
                                        }
                                    }
                                }
                            }
                            ServerMsg::ActionFeedback {
                                target,
                                ok,
                                message,
                            } => {
                                let undo_msg = if ok { self.pending_undo.take() } else { None };
                                self.toast = Some(ToastNotice {
                                    ok,
                                    text: format!("{target}: {message}"),
                                    expires_at: now + 5.5,
                                    undo_msg,
                                });
                            }
                            ServerMsg::DirSnapshot(listing) => {
                                for group in &mut self.wm.groups {
                                    for tab in &mut group.tabs {
                                        if let PaneKind::FileTracker(tracker) = &mut tab.kind {
                                            tracker.on_dir_snapshot(listing.clone());
                                        }
                                    }
                                }
                            }
                            ServerMsg::FilePreview {
                                pane_id: _,
                                path,
                                preview,
                            } => {
                                for group in &mut self.wm.groups {
                                    for tab in &mut group.tabs {
                                        if let PaneKind::FileTracker(tracker) = &mut tab.kind {
                                            tracker.on_preview(&ctx, path.clone(), preview.clone());
                                        }
                                    }
                                }
                            }
                            ServerMsg::WorkspacesSnapshot(list) => {
                                for group in &mut self.wm.groups {
                                    for tab in &mut group.tabs {
                                        if let PaneKind::FileTracker(tracker) = &mut tab.kind {
                                            tracker.workspaces = list.clone();
                                        }
                                    }
                                }
                            }
                            ServerMsg::SearchResults { .. } => {}
                            ServerMsg::AppDataState(snap) => self.appdata = Some(snap),
                        }
                    }
                }
                WsEvent::Message(_) => {}
            }
        }

        // Desktop Canvas: Obsidian Dot-Grid + optional kaage Wallpaper
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.canvas_bg))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                if self.show_wallpaper
                    && let Some(tex) = &self.wallpaper
                {
                    let tex_size = tex.size_vec2();
                    if rect.width() > 0.0
                        && rect.height() > 0.0
                        && tex_size.x > 0.0
                        && tex_size.y > 0.0
                    {
                        let view_aspect = rect.width() / rect.height();
                        let tex_aspect = tex_size.x / tex_size.y;
                        let uv = if view_aspect > tex_aspect {
                            let h = tex_aspect / view_aspect;
                            Rect::from_min_max(
                                Pos2::new(0.0, (1.0 - h) * 0.5),
                                Pos2::new(1.0, (1.0 + h) * 0.5),
                            )
                        } else {
                            let w = view_aspect / tex_aspect;
                            let x_min = (1.0 - w) * 0.72;
                            Rect::from_min_max(Pos2::new(x_min, 0.0), Pos2::new(x_min + w, 1.0))
                        };
                        let tint = if p.is_dark {
                            Color32::from_rgba_unmultiplied(150, 165, 190, 165)
                        } else {
                            Color32::WHITE
                        };
                        ui.painter().image(tex.id(), rect, uv, tint);
                    }
                }
                if p.is_dark {
                    draw_dot_grid(ui.painter(), rect);
                }
            });

        // Desktop Shortcut Icons (right edge, wide screens only)
        if ctx.content_rect().width() >= 1120.0
            && let Some(icon_act) = show_desktop_icons(&ctx)
        {
            match icon_act {
                DesktopIconAction::OpenFileTracker => self.open_file_tracker(),
                DesktopIconAction::OpenVitals => self.open_vitals(),
                DesktopIconAction::OpenHostServices => {
                    self.wm.open_or_focus_systemd(
                        ScopeFilter::All,
                        StateFilter::All,
                        self.tracker.width(),
                    );
                    self.send_client_msg(ClientMsg::ListUnits);
                }
                DesktopIconAction::OpenSystemMonitor => self.open_system_monitor(),
                DesktopIconAction::OpenAiJobs => self.open_ai_jobs(),
                DesktopIconAction::OpenHttpPorts => self.open_http_ports(),
                DesktopIconAction::OpenTailscale => self.open_tailscale(),
                DesktopIconAction::OpenTerminal => {
                    self.spawn_terminal_window("Terminal — shell".into(), None);
                }
            }
        }

        // Left Host Sidebar (auto-hidden on narrow mobile viewports)
        let sidebar_width = if ctx.content_rect().width() < 768.0 {
            0.0
        } else {
            if let Some(tracker_act) = self.tracker.show(
                &ctx,
                self.nixos.as_ref(),
                self.metrics.as_ref(),
                &self.units,
                &self.containers,
                self.dashboard.as_ref(),
                &self.wm.groups,
            ) {
                match tracker_act {
                    TrackerAction::Hide => self.tracker.visible = false,
                    TrackerAction::NewTerminal => {
                        self.spawn_terminal_window("Terminal — shell".into(), None);
                    }
                    TrackerAction::OpenServices { scope, state } => {
                        self.wm
                            .open_or_focus_systemd(scope, state, self.tracker.width());
                        self.send_client_msg(ClientMsg::ListUnits);
                    }
                    TrackerAction::OpenContainers => self.open_containers_window(),
                    TrackerAction::OpenSystemMonitor => self.open_system_monitor(),
                    TrackerAction::OpenVitals => self.open_vitals(),
                    TrackerAction::OpenAiJobs => self.open_ai_jobs(),
                    TrackerAction::OpenHttpPorts => self.open_http_ports(),
                    TrackerAction::OpenTailscale => self.open_tailscale(),
                    TrackerAction::OpenTailscaleServices => self.open_tailscale_services(),
                    TrackerAction::OpenFileTracker => self.open_file_tracker(),
                    TrackerAction::OpenHelp => self.open_help(),
                    TrackerAction::OpenSettings => self.open_settings(),
                    TrackerAction::OpenJournal { user } => {
                        let (title, cmd) = if user {
                            (
                                "journal — user".into(),
                                vec![
                                    "journalctl".into(),
                                    "--user".into(),
                                    "-n".into(),
                                    "100".into(),
                                    "-f".into(),
                                ],
                            )
                        } else {
                            (
                                "journal — system".into(),
                                vec!["journalctl".into(), "-n".into(), "100".into(), "-f".into()],
                            )
                        };
                        self.spawn_terminal_window(title, Some(cmd));
                    }
                    TrackerAction::FocusWindow { group_id, tab_idx } => {
                        self.wm.focus_tab(group_id, tab_idx);
                    }
                }
            }
            self.tracker.width()
        };

        // Hybrid BSP Tiling + Pinned PiP Windows
        let mut outbound = Vec::new();
        let spawn_reqs = self.wm.show(
            &ctx,
            sidebar_width,
            self.nixos.as_ref(),
            &self.units,
            &mut self.selected_unit_status,
            &self.containers,
            self.dashboard.as_ref(),
            self.appdata.as_ref(),
            &self.cpu_history,
            &self.mem_history,
            &mut |msg| {
                outbound.push(msg);
            },
        );
        for msg in outbound {
            self.send_client_msg(msg);
        }
        for req in spawn_reqs {
            match req {
                InspectorSpawnRequest::OpenCommandTab { title, cmd } => {
                    self.spawn_terminal_window(title, Some(cmd));
                }
            }
        }

        // Waybar-style 28px Mission-Control Top Status Strip
        if let Some(action) = self.deskbar.show(
            &ctx,
            self.nixos.as_ref(),
            self.metrics.as_ref(),
            self.latest_vital.as_ref(),
            self.dashboard.as_ref(),
            &self.units,
            self.connected,
            self.tracker.visible,
            self.wm.active_workspace,
        ) {
            match action {
                DeskbarAction::OpenPalette => self.cmd_palette.toggle(),
                DeskbarAction::ToggleTracker => self.tracker.visible = !self.tracker.visible,
                DeskbarAction::ToggleTheme => self.toggle_theme(&ctx),
                DeskbarAction::SwitchWorkspace(ws) => self.wm.active_workspace = ws,
                DeskbarAction::OpenSystemdInspector { scope, state } => {
                    self.wm
                        .open_or_focus_systemd(scope, state, self.tracker.width());
                    self.send_client_msg(ClientMsg::ListUnits);
                }
                DeskbarAction::OpenVitals => self.open_vitals(),
                DeskbarAction::OpenTailscale => self.open_tailscale(),
                DeskbarAction::OpenTailscaleServices => self.open_tailscale_services(),
                DeskbarAction::OpenHelp => self.open_help(),
                DeskbarAction::OpenSettings => self.open_settings(),
            }
        }

        // Global Command Palette (Ctrl+K / Cmd+K)
        if let Some(act) = self.cmd_palette.show(&ctx, &self.units, &self.containers) {
            match act {
                PaletteAction::SwitchWorkspace(ws) => self.wm.active_workspace = ws,
                PaletteAction::OpenVitals => self.open_vitals(),
                PaletteAction::OpenFileTracker => self.open_file_tracker(),
                PaletteAction::OpenSystemd { scope, state } => {
                    self.wm
                        .open_or_focus_systemd(scope, state, self.tracker.width());
                    self.send_client_msg(ClientMsg::ListUnits);
                }
                PaletteAction::OpenSystemMonitor => self.open_system_monitor(),
                PaletteAction::OpenAiJobs => self.open_ai_jobs(),
                PaletteAction::OpenHttpPorts => self.open_http_ports(),
                PaletteAction::OpenTailscale => self.open_tailscale(),
                PaletteAction::OpenTailscaleServices => self.open_tailscale_services(),
                PaletteAction::OpenContainers => self.open_containers_window(),
                PaletteAction::OpenHelp => self.open_help(),
                PaletteAction::OpenSettings => self.open_settings(),
                PaletteAction::OpenAppDataSettings => self.open_appdata_settings(),
                PaletteAction::NewTerminal => {
                    self.spawn_terminal_window("Terminal — shell".into(), None);
                }
                PaletteAction::ToggleTheme => self.toggle_theme(&ctx),
                PaletteAction::ToggleWallpaper => self.show_wallpaper = !self.show_wallpaper,
                PaletteAction::ToggleSidebar => self.tracker.visible = !self.tracker.visible,
                PaletteAction::TogglePinActive => self.wm.toggle_pin_active(),
                PaletteAction::ControlUnit {
                    unit,
                    scope,
                    action,
                } => {
                    self.send_client_msg(ClientMsg::ControlUnit {
                        unit,
                        scope,
                        action,
                    });
                }
                PaletteAction::UnitLogs { unit, scope } => {
                    let mut cmd = vec!["journalctl".into()];
                    if scope == kaage_proto::UnitScope::User {
                        cmd.push("--user".into());
                    }
                    cmd.extend(["-u".into(), unit.clone(), "-n".into(), "100".into(), "-f".into()]);
                    self.spawn_terminal_window(format!("logs: {unit}"), Some(cmd));
                }
                PaletteAction::ControlContainer { id, action } => {
                    self.send_client_msg(ClientMsg::ControlContainer { id, action });
                }
                PaletteAction::ContainerShell { name, id } => {
                    self.spawn_terminal_window(
                        format!("sh: {name}"),
                        Some(vec![
                            "docker".into(),
                            "exec".into(),
                            "-it".into(),
                            id,
                            "/bin/sh".into(),
                        ]),
                    );
                }
            }
        }

        // Interactive Toast Notice with Undo Button (bottom-right)
        let mut trigger_undo: Option<ClientMsg> = None;
        if let Some(toast) = &self.toast {
            if now < toast.expires_at {
                let screen = ctx.content_rect();
                let toast_w: f32 = if toast.undo_msg.is_some() { 400.0 } else { 340.0 };
                let toast_size = Vec2::new(toast_w.min(screen.width() - 24.0), 30.0);
                let toast_pos = Pos2::new(
                    (screen.max.x - toast_size.x - 12.0).max(12.0),
                    (screen.max.y - toast_size.y - 12.0).max(12.0),
                );
                egui::Area::new(Id::unique("kaage_toast"))
                    .fixed_pos(toast_pos)
                    .order(egui::Order::Tooltip)
                    .show(&ctx, |ui| {
                        let rect = Rect::from_min_size(toast_pos, toast_size);
                        let painter = ui.painter();
                        painter.rect_filled(rect, CornerRadius::ZERO, p.surface);
                        draw_beos_bevel(painter, rect, false);
                        let accent = if toast.ok {
                            p.status_green
                        } else {
                            p.status_red
                        };
                        painter.rect_filled(
                            Rect::from_min_size(
                                Pos2::new(rect.min.x + 2.0, rect.min.y + 2.0),
                                Vec2::new(4.0, rect.height() - 4.0),
                            ),
                            CornerRadius::ZERO,
                            accent,
                        );
                        painter.text(
                            Pos2::new(rect.min.x + 12.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &toast.text,
                            egui::FontId::proportional(12.0),
                            p.text,
                        );
                        if let Some(ref undo_action) = toast.undo_msg {
                            let undo_rect = Rect::from_min_size(
                                Pos2::new(rect.max.x - 56.0, rect.min.y + 4.0),
                                Vec2::new(50.0, rect.height() - 8.0),
                            );
                            let resp = ui.interact(
                                undo_rect,
                                Id::unique("kaage_toast_undo"),
                                egui::Sense::click(),
                            );
                            painter.rect_filled(
                                undo_rect,
                                CornerRadius::ZERO,
                                if resp.hovered() { p.widget_hover } else { p.widget },
                            );
                            painter.rect_stroke(
                                undo_rect,
                                CornerRadius::ZERO,
                                Stroke::new(1.0, p.accent),
                                egui::StrokeKind::Inside,
                            );
                            painter.text(
                                undo_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Undo",
                                egui::FontId::monospace(10.5),
                                p.accent,
                            );
                            if resp.clicked() {
                                trigger_undo = Some(undo_action.clone());
                            }
                        }
                    });
            } else {
                self.toast = None;
            }
        }
        if let Some(undo_msg) = trigger_undo {
            self.toast = None;
            self.send_client_msg(undo_msg);
        }

        // Persist window layout, active workspace, sidebar, and user settings to browser localStorage
        // (throttled to 0.5s and only when state actually changes).
        if now - self.last_session_save_at >= 0.5 {
            self.last_session_save_at = now;
            let snap = SavedSession::capture(
                &self.wm,
                self.tracker.visible,
                self.tracker.wide,
                self.show_wallpaper,
                user_settings(),
            );
            if let Ok(bytes) = postcard::to_allocvec(&snap)
                && bytes != self.last_saved_session
            {
                save_session_bytes(&bytes);
                self.last_saved_session = bytes;
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;

    let window = web_sys::window().unwrap();
    let location = window.location();
    let protocol = if location.protocol().unwrap_or_default() == "https:" {
        "wss:"
    } else {
        "ws:"
    };
    let host = location.host().unwrap_or_else(|_| "127.0.0.1:9090".into());
    let ws_url = format!("{protocol}//{host}/ws");

    let document = window.document().unwrap();
    let canvas = document
        .get_element_by_id("kaage_canvas")
        .unwrap()
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .unwrap();

    wasm_bindgen_futures::spawn_local(async move {
        let start_result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| Ok(Box::new(KaageApp::new(cc, &ws_url)))),
            )
            .await;

        if let Err(err) = start_result {
            web_sys::console::error_1(&err);
            if let Some(el) = document.get_element_by_id("kaage_error") {
                let _ = el.set_attribute("style", "display: block;");
                el.set_text_content(Some(&format!(
                    "kaage failed to initialize WebGL2 renderer:\n{err:?}"
                )));
            }
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let ws_url = std::env::var("KAAGE_WS_URL").unwrap_or_else(|_| "ws://127.0.0.1:9090/ws".into());
    let options = eframe::NativeOptions::default();
    let _ = eframe::run_native(
        "kaage",
        options,
        Box::new(move |cc| Ok(Box::new(KaageApp::new(cc, &ws_url)))),
    );
}
