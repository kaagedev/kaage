mod deskbar;
mod inspectors;
mod terminal;
mod theme;
mod wm;

use deskbar::{BeDeskbar, DeskbarAction};
use eframe::egui::{self, Color32, CornerRadius, Id, Pos2, Rect, Vec2};
use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use inspectors::{InspectorSpawnRequest, ScopeFilter};
use kaage_proto::{ClientMsg, ContainerInfo, HostMetrics, ServerMsg, SystemdUnitInfo};
use terminal::TerminalPane;
use theme::{
    BEOS_DESKTOP_BLUE, BEOS_PANEL_GRAY, BEOS_STATUS_GREEN, BEOS_STATUS_RED, apply_beos_style,
    draw_beos_bevel,
};
use wm::{BeWindowManager, PaneKind};

struct ToastNotice {
    ok: bool,
    text: String,
    expires_at: f64,
}

struct KaageApp {
    ws_tx: WsSender,
    ws_rx: WsReceiver,
    connected: bool,
    wm: BeWindowManager,
    deskbar: BeDeskbar,
    metrics: Option<HostMetrics>,
    units: Vec<SystemdUnitInfo>,
    containers: Vec<ContainerInfo>,
    next_pty_id: u32,
    toast: Option<ToastNotice>,
}

impl KaageApp {
    fn new(cc: &eframe::CreationContext<'_>, ws_url: &str) -> Self {
        apply_beos_style(&cc.egui_ctx);

        let ctx_clone = cc.egui_ctx.clone();
        let (ws_tx, ws_rx) = ewebsock::connect_with_wakeup(
            ws_url,
            ewebsock::Options::default(),
            move || ctx_clone.request_repaint(),
        )
        .expect("Failed to open WebSocket connection");

        let mut wm = BeWindowManager::new();
        wm.open_window(
            "Systemd Services".into(),
            PaneKind::SystemdInspector {
                filter: String::new(),
                scope_filter: ScopeFilter::All,
            },
            Pos2::new(24.0, 24.0),
            Vec2::new(580.0, 340.0),
        );

        let initial_pty = 1;
        wm.open_window(
            "Terminal — shell".into(),
            PaneKind::Terminal(TerminalPane::new(initial_pty, 80, 24)),
            Pos2::new(60.0, 395.0),
            Vec2::new(680.0, 410.0),
        );

        Self {
            ws_tx,
            ws_rx,
            connected: false,
            wm,
            deskbar: BeDeskbar::new(),
            metrics: None,
            units: Vec::new(),
            containers: Vec::new(),
            next_pty_id: 2,
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
        self.send_client_msg(ClientMsg::SpawnPty {
            session_id: 1,
            cols: 80,
            rows: 24,
            cmd: None,
        });
    }

    fn spawn_terminal_window(&mut self, title: String, cmd: Option<Vec<String>>) {
        let session_id = self.next_pty_id;
        self.next_pty_id += 1;

        self.wm.open_window(
            title,
            PaneKind::Terminal(TerminalPane::new(session_id, 80, 24)),
            Pos2::new(110.0 + (session_id as f32 * 18.0), 90.0),
            Vec2::new(680.0, 410.0),
        );

        self.send_client_msg(ClientMsg::SpawnPty {
            session_id,
            cols: 80,
            rows: 24,
            cmd,
        });
    }
}

impl eframe::App for KaageApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);

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
                            ServerMsg::Metrics(m) => self.metrics = Some(m),
                            ServerMsg::UnitsSnapshot(u) => self.units = u,
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
                        }
                    }
                }
                WsEvent::Message(_) => {}
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BEOS_DESKTOP_BLUE))
            .show(ui, |_ui| {});

        let mut outbound = Vec::new();
        let spawn_reqs = self.wm.show(&ctx, &self.units, &self.containers, &mut |msg| {
            outbound.push(msg);
        });
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

        if let Some(action) =
            self.deskbar
                .show(&ctx, self.metrics.as_ref(), self.connected, &self.wm.groups)
        {
            match action {
                DeskbarAction::NewTerminal => {
                    self.spawn_terminal_window("Terminal — shell".into(), None);
                }
                DeskbarAction::OpenSystemdInspector => {
                    self.wm.open_window(
                        "Systemd Services".into(),
                        PaneKind::SystemdInspector {
                            filter: String::new(),
                            scope_filter: ScopeFilter::All,
                        },
                        Pos2::new(48.0, 48.0),
                        Vec2::new(580.0, 340.0),
                    );
                    self.send_client_msg(ClientMsg::ListUnits);
                }
                DeskbarAction::OpenDockerInspector => {
                    self.wm.open_window(
                        "Containers".into(),
                        PaneKind::DockerInspector {
                            filter: String::new(),
                        },
                        Pos2::new(84.0, 84.0),
                        Vec2::new(540.0, 320.0),
                    );
                    self.send_client_msg(ClientMsg::ListContainers);
                }
                DeskbarAction::FocusWindow { group_id, tab_idx } => {
                    self.wm.focus_tab(group_id, tab_idx);
                }
            }
        }

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
        eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| Ok(Box::new(KaageApp::new(cc, &ws_url)))),
            )
            .await
            .expect("Failed to start eframe WebRunner");
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
