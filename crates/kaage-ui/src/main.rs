mod clock;
mod deskbar;
mod desktop_icons;
mod file_tracker;
mod inspectors;
mod panels;
mod terminal;
mod theme;
mod tracker;
mod wm;

use deskbar::{BeDeskbar, DeskbarAction};
use desktop_icons::{DesktopIconAction, show_desktop_icons};
use eframe::egui::{self, Color32, CornerRadius, Id, Pos2, Rect, Vec2};
use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use file_tracker::TrackerWindow;
use inspectors::{InspectorSpawnRequest, ScopeFilter, StateFilter};
use kaage_proto::{
    ClientMsg, ContainerInfo, DashboardSnapshot, HostMetrics, NixosInfo, ServerMsg,
    SystemdUnitInfo, SystemdUnitStatus,
};
use terminal::TerminalPane;
use theme::{
    BEOS_DESKTOP_BLUE, BEOS_PANEL_GRAY, BEOS_STATUS_GREEN, BEOS_STATUS_RED, TOPBAR_HEIGHT,
    apply_beos_style, draw_beos_bevel,
};
use tracker::{BeTrackerSidebar, TrackerAction};
use wm::{BeWindowManager, PaneKind};

const WALLPAPER_BYTES: &[u8] = include_bytes!("../../../assets/kaage-wallpaper.jpg");
const MAX_HISTORY_SAMPLES: usize = 60;

struct ToastNotice {
    ok: bool,
    text: String,
    expires_at: f64,
}

struct KaageApp {
    ws_tx: WsSender,
    ws_rx: WsReceiver,
    connected: bool,
    wallpaper: Option<egui::TextureHandle>,
    wm: BeWindowManager,
    deskbar: BeDeskbar,
    tracker: BeTrackerSidebar,
    nixos: Option<NixosInfo>,
    metrics: Option<HostMetrics>,
    dashboard: Option<DashboardSnapshot>,
    cpu_history: Vec<f32>,
    mem_history: Vec<f32>,
    units: Vec<SystemdUnitInfo>,
    selected_unit_status: Option<SystemdUnitStatus>,
    containers: Vec<ContainerInfo>,
    next_pty_id: u32,
    next_tracker_pane_id: u64,
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
        apply_beos_style(&cc.egui_ctx);
        let wallpaper = load_wallpaper_texture(&cc.egui_ctx);

        let ctx_clone = cc.egui_ctx.clone();
        let (ws_tx, ws_rx) = ewebsock::connect_with_wakeup(
            ws_url,
            ewebsock::Options::default(),
            move || ctx_clone.request_repaint(),
        )
        .expect("Failed to open WebSocket connection");

        let tracker = BeTrackerSidebar::new();
        let stage_x = tracker.width() + 24.0;

        let mut wm = BeWindowManager::new();
        let main_gid = wm.open_window(
            "Systemd Services".into(),
            PaneKind::SystemdInspector {
                filter: String::new(),
                scope_filter: ScopeFilter::All,
                state_filter: StateFilter::All,
            },
            Pos2::new(stage_x, TOPBAR_HEIGHT + 12.0),
            Vec2::new(720.0, 420.0),
        );
        wm.add_tab_to_group(main_gid, "GPU / CPU / Mem".into(), PaneKind::SystemMonitor);
        wm.add_tab_to_group(main_gid, "AI Jobs".into(), PaneKind::AiJobs);
        wm.add_tab_to_group(
            main_gid,
            "HTTP Ports".into(),
            PaneKind::HttpPorts {
                filter: String::new(),
            },
        );
        wm.add_tab_to_group(main_gid, "Tailscale".into(), PaneKind::TailscalePanel);

        let initial_pty = 1;
        wm.open_window(
            "Terminal — shell".into(),
            PaneKind::Terminal(TerminalPane::new(initial_pty, 80, 24)),
            Pos2::new(stage_x + 24.0, TOPBAR_HEIGHT + 464.0),
            Vec2::new(700.0, 320.0),
        );

        Self {
            ws_tx,
            ws_rx,
            connected: false,
            wallpaper,
            wm,
            deskbar: BeDeskbar::new(),
            tracker,
            nixos: None,
            metrics: None,
            dashboard: None,
            cpu_history: Vec::with_capacity(MAX_HISTORY_SAMPLES),
            mem_history: Vec::with_capacity(MAX_HISTORY_SAMPLES),
            units: Vec::new(),
            selected_unit_status: None,
            containers: Vec::new(),
            next_pty_id: 2,
            next_tracker_pane_id: 1,
            toast: None,
        }
    }

    fn send_client_msg(&mut self, msg: ClientMsg) {
        if let Ok(bytes) = postcard::to_allocvec(&msg) {
            self.ws_tx.send(WsMessage::Binary(bytes));
        }
    }

    fn send_initial_sync(&mut self) {
        self.send_client_msg(ClientMsg::ListUnits);
        self.send_client_msg(ClientMsg::ListContainers);
        self.send_client_msg(ClientMsg::RefreshDashboard);
        self.send_client_msg(ClientMsg::SpawnPty {
            session_id: 1,
            cols: 80,
            rows: 24,
            cmd: None,
        });
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

    fn open_bottom(&mut self) {
        if self.wm.focus_tab_by_title("Bottom (btm)") {
            return;
        }
        let session_id = self.next_pty_id;
        self.next_pty_id += 1;

        self.wm.open_window(
            "Bottom (btm)".into(),
            PaneKind::Terminal(TerminalPane::new(session_id, 100, 30)),
            self.stage_origin(28.0, 24.0),
            Vec2::new(860.0, 520.0),
        );

        self.send_client_msg(ClientMsg::SpawnPty {
            session_id,
            cols: 100,
            rows: 30,
            cmd: Some(vec![
                "btm".into(),
                "--dot_marker".into(),
                "--color".into(),
                "gruvbox".into(),
            ]),
        });
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

    fn open_file_tracker(&mut self) {
        if self
            .wm
            .focus_matching_tab(|k| matches!(k, PaneKind::FileTracker(_)))
        {
            return;
        }
        let left_id = self.next_tracker_pane_id;
        let right_id = left_id + 1;
        self.next_tracker_pane_id += 2;

        let mut pending = Vec::new();
        let tracker = TrackerWindow::new(left_id, right_id, &mut |msg| pending.push(msg));
        for msg in pending {
            self.send_client_msg(msg);
        }

        self.wm.open_window(
            "Tracker — Files".into(),
            PaneKind::FileTracker(Box::new(tracker)),
            self.stage_origin(16.0, 16.0),
            Vec2::new(920.0, 560.0),
        );
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
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

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
                    });
                }
                WsEvent::Message(WsMessage::Binary(bytes)) => {
                    if let Ok(server_msg) = postcard::from_bytes::<ServerMsg>(&bytes) {
                        match server_msg {
                            ServerMsg::NixosInfo(info) => self.nixos = Some(info),
                            ServerMsg::Metrics(m) => {
                                Self::push_history(&mut self.cpu_history, m.cpu_percent);
                                if m.mem_total_mb > 0 {
                                    let pct = (m.mem_used_mb as f32 / m.mem_total_mb as f32) * 100.0;
                                    Self::push_history(&mut self.mem_history, pct);
                                }
                                self.metrics = Some(m);
                            }
                            ServerMsg::Dashboard(d) => {
                                self.dashboard = Some(d);
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
                                self.toast = Some(ToastNotice {
                                    ok,
                                    text: format!("{target}: {message}"),
                                    expires_at: now + 4.5,
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
                        }
                    }
                }
                WsEvent::Message(_) => {}
            }
        }

        // Desktop Canvas + kaage Wallpaper
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BEOS_DESKTOP_BLUE))
            .show(ui, |ui| {
                if let Some(tex) = &self.wallpaper {
                    let rect = ui.max_rect();
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
                        ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
                    }
                }
            });

        // Classic BeOS Desktop Icons (right edge)
        if let Some(icon_act) = show_desktop_icons(&ctx) {
            match icon_act {
                DesktopIconAction::OpenFileTracker => self.open_file_tracker(),
                DesktopIconAction::OpenHostServices => {
                    self.wm.open_or_focus_systemd(
                        ScopeFilter::All,
                        StateFilter::All,
                        self.tracker.width(),
                    );
                    self.send_client_msg(ClientMsg::ListUnits);
                }
                DesktopIconAction::OpenSystemMonitor => self.open_system_monitor(),
                DesktopIconAction::OpenBottom => self.open_bottom(),
                DesktopIconAction::OpenAiJobs => self.open_ai_jobs(),
                DesktopIconAction::OpenHttpPorts => self.open_http_ports(),
                DesktopIconAction::OpenTailscale => self.open_tailscale(),
                DesktopIconAction::OpenTerminal => {
                    self.spawn_terminal_window("Terminal — shell".into(), None);
                }
            }
        }

        // Left Floating Window: BeOS Tracker Sidebar
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
                TrackerAction::Hide => {
                    self.tracker.visible = false;
                }
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
                TrackerAction::OpenBottom => self.open_bottom(),
                TrackerAction::OpenAiJobs => self.open_ai_jobs(),
                TrackerAction::OpenHttpPorts => self.open_http_ports(),
                TrackerAction::OpenTailscale => self.open_tailscale(),
                TrackerAction::OpenFileTracker => self.open_file_tracker(),
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

        // Floating BeOS Windows
        let mut outbound = Vec::new();
        let spawn_reqs = self.wm.show(
            &ctx,
            self.nixos.as_ref(),
            &self.units,
            &mut self.selected_unit_status,
            &self.containers,
            self.dashboard.as_ref(),
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

        // Full-Width Classic BeOS Topbar / Deskbar
        if let Some(action) = self.deskbar.show(
            &ctx,
            self.nixos.as_ref(),
            self.metrics.as_ref(),
            &self.units,
            self.connected,
            self.tracker.visible,
            &self.wm.groups,
        ) {
            match action {
                DeskbarAction::ToggleTracker => {
                    self.tracker.visible = !self.tracker.visible;
                }
                DeskbarAction::TileWindows => {
                    let screen = ctx.content_rect();
                    self.wm.tile_windows(screen, self.tracker.width());
                }
                DeskbarAction::NewTerminal => {
                    self.spawn_terminal_window("Terminal — shell".into(), None);
                }
                DeskbarAction::OpenSystemdInspector { scope, state } => {
                    self.wm
                        .open_or_focus_systemd(scope, state, self.tracker.width());
                    self.send_client_msg(ClientMsg::ListUnits);
                }
                DeskbarAction::OpenDockerInspector => self.open_containers_window(),
                DeskbarAction::OpenSystemMonitor => self.open_system_monitor(),
                DeskbarAction::OpenBottom => self.open_bottom(),
                DeskbarAction::OpenAiJobs => self.open_ai_jobs(),
                DeskbarAction::OpenHttpPorts => self.open_http_ports(),
                DeskbarAction::OpenTailscale => self.open_tailscale(),
                DeskbarAction::OpenFileTracker => self.open_file_tracker(),
                DeskbarAction::FocusWindow { group_id, tab_idx } => {
                    self.wm.focus_tab(group_id, tab_idx);
                }
            }
        }

        // Action Toast Notice (bottom-right)
        if let Some(toast) = &self.toast {
            if now < toast.expires_at {
                let screen = ctx.content_rect();
                let toast_size = Vec2::new(340.0, 28.0);
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
                        painter.rect_filled(rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                        draw_beos_bevel(painter, rect, false);
                        let accent = if toast.ok {
                            BEOS_STATUS_GREEN
                        } else {
                            BEOS_STATUS_RED
                        };
                        painter.rect_filled(
                            Rect::from_min_size(
                                Pos2::new(rect.min.x + 2.0, rect.min.y + 2.0),
                                Vec2::new(5.0, rect.height() - 4.0),
                            ),
                            CornerRadius::ZERO,
                            accent,
                        );
                        painter.text(
                            Pos2::new(rect.min.x + 12.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &toast.text,
                            egui::FontId::proportional(12.0),
                            Color32::BLACK,
                        );
                    });
            } else {
                self.toast = None;
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
