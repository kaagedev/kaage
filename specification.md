# kaage — BeOS-Style Service & Terminal Webtop

**kaage** (ಕಾಗೆ / 影) is a single-user, BeOS-inspired browser desktop environment built in Rust for inspecting and controlling a Linux host's services, containers, metrics, and interactive terminals over Tailscale.

## 1. Architecture Overview

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Browser (WASM / WebGPU) — kaage-ui                                         │
│ ┌───────────────────────────────────┐ ┌──────────────────────────────────┐ │
│ │ BeOS Stack & Tile Window Manager  │ │ BeOS Deskbar (Top-Right)         │ │
│ │ • Sliding yellow tabs (Shift+Drag)│ │ • Geometric Crow Menu            │ │
│ │ • Tab stacking & detaching        │ │ • Live CPU/RAM sparkbars + Clock │ │
│ │ • 3D beveled borders & widgets    │ │ • Active Window / Tab Switcher   │ │
│ └─────────────────┬─────────────────┘ └──────────────────────────────────┘ │
│                   │                                                        │
│   ┌───────────────┴───────────────┐                                        │
│   ▼                               ▼                                        │
│ ┌─────────────────────────┐ ┌────────────────────────────────────────────┐ │
│ │ Service & Container     │ │ libghostty-vt Terminal Renderer            │ │
│ │ Inspectors              │ │ • Zig-compiled Ghostty VT engine           │ │
│ │ • Systemd units (zbus)  │ │ • Kitty graphics RGBA texture placements   │ │
│ │ • Containers (bollard)  │ │ • 1-click journalctl / container exec tabs │ │
│ └───────────┬─────────────┘ └─────────────────────┬──────────────────────┘ │
└─────────────┼─────────────────────────────────────┼────────────────────────┘
              │      Single Multiplexed WebSocket   │
              │      (`postcard` binary frames)     │
              └──────────────────┬──────────────────┘
                                 ▼
┌────────────────────────────────────────────────────────────────────────────┐
│ Linux Host Daemon — kaaged (axum + tokio, behind `tailscale serve`)        │
│ • Auth: Verifies `Tailscale-User-Login` header against `KAAGE_ALLOWED_USER`│
│ • Systemd Control: D-Bus (`zbus`) list, start, stop, restart, reload       │
│ • Container Control: Docker/Podman socket (`bollard`) list, lifecycle, exec│
│ • Telemetry: `sysinfo` CPU/RAM/Load ring stream                            │
│ • PTY Multiplexer: `portable-pty` interactive shells & unit log streams    │
└────────────────────────────────────────────────────────────────────────────┘
```

## 2. Workspace Directory Structure

```text
kaage/
├── Cargo.toml
├── index.html
└── crates/
    ├── kaage-proto/
    │   ├── Cargo.toml
    │   └── src/
    │       └── lib.rs
    ├── kaaged/
    │   ├── Cargo.toml
    │   └── src/
    │       └── main.rs
    └── kaage-ui/
        ├── Cargo.toml
        └── src/
            ├── main.rs
            ├── theme.rs
            ├── wm.rs
            ├── deskbar.rs
            ├── inspectors.rs
            └── terminal.rs
```

## 3. Workspace & Crate Manifests

### `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = [
    "crates/kaage-proto",
    "crates/kaaged",
    "crates/kaage-ui",
]

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
postcard = { version = "1.1", features = ["alloc"] }
```

### `index.html`

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>kaage</title>
  <link data-trunk rel="rust" href="crates/kaage-ui/Cargo.toml" data-wasm-opt="z" />
  <style>
    html, body {
      margin: 0;
      padding: 0;
      width: 100%;
      height: 100%;
      overflow: hidden;
      background-color: #336698;
    }
    canvas {
      width: 100%;
      height: 100%;
      display: block;
    }
  </style>
</head>
<body>
  <canvas id="kaage_canvas"></canvas>
</body>
</html>
```

### `crates/kaage-proto/Cargo.toml`

```toml
[package]
name = "kaage-proto"
version = "0.1.0"
edition = "2024"

[dependencies]
serde.workspace = true
postcard.workspace = true
```

### `crates/kaaged/Cargo.toml`

```toml
[package]
name = "kaaged"
version = "0.1.0"
edition = "2024"

[dependencies]
kaage-proto = { path = "../kaage-proto" }
serde.workspace = true
postcard.workspace = true
tokio = { version = "1.44", features = ["full"] }
axum = { version = "0.8", features = ["ws"] }
tower-http = { version = "0.6", features = ["fs", "cors"] }
zbus = { version = "5.5", default-features = false, features = ["tokio"] }
bollard = "0.18"
portable-pty = "0.9"
sysinfo = "0.34"
futures-util = "0.3"
tracing = "0.1"
tracing-subscriber = "0.3"
```

### `crates/kaage-ui/Cargo.toml`

```toml
[package]
name = "kaage-ui"
version = "0.1.0"
edition = "2024"

[dependencies]
kaage-proto = { path = "../kaage-proto" }
serde.workspace = true
postcard.workspace = true
eframe = { version = "0.37", default-features = false, features = ["wgpu", "default_fonts"] }
egui = "0.37"
ghostty-vt = "0.8"
ewebsock = "0.8"
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
web-sys = { version = "0.3", features = ["Window", "Location", "Document", "HtmlCanvasElement"] }
```

## 4. Shared Protocol (`crates/kaage-proto/src/lib.rs`)

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnitAction {
    Start,
    Stop,
    Restart,
    Reload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemdUnitInfo {
    pub name: String,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostMetrics {
    pub hostname: String,
    pub cpu_percent: f32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    ListUnits,
    ControlUnit {
        unit: String,
        action: UnitAction,
    },
    ListContainers,
    ControlContainer {
        id: String,
        action: ContainerAction,
    },
    SpawnPty {
        session_id: u32,
        cols: u16,
        rows: u16,
        cmd: Option<Vec<String>>,
    },
    PtyInput {
        session_id: u32,
        data: Vec<u8>,
    },
    ResizePty {
        session_id: u32,
        cols: u16,
        rows: u16,
    },
    ClosePty {
        session_id: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    Metrics(HostMetrics),
    UnitsSnapshot(Vec<SystemdUnitInfo>),
    ContainersSnapshot(Vec<ContainerInfo>),
    ActionFeedback {
        target: String,
        ok: bool,
        message: String,
    },
    PtyOutput {
        session_id: u32,
        data: Vec<u8>,
    },
    PtyExited {
        session_id: u32,
    },
}
```

## 5. Host Daemon (`crates/kaaged/src/main.rs`)

```rust
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use bollard::{
    container::{
        ListContainersOptions, RemoveContainerOptions, RestartContainerOptions,
        StartContainerOptions, StopContainerOptions,
    },
    Docker,
};
use futures_util::{SinkExt, StreamExt};
use kaage_proto::{
    ClientMsg, ContainerAction, ContainerInfo, HostMetrics, ServerMsg, SystemdUnitInfo, UnitAction,
};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use sysinfo::System;
use tokio::sync::mpsc;
use tower_http::services::ServeDir;
use zbus::zvariant::OwnedObjectPath;

struct AppState {
    allowed_user: Option<String>,
    dbus: zbus::Connection,
    docker: Option<Docker>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let allowed_user = std::env::var("KAAGE_ALLOWED_USER").ok();
    let dbus = zbus::Connection::system()
        .await
        .expect("Failed to connect to system D-Bus");

    // Connects to /var/run/docker.sock or DOCKER_HOST (works with Podman socket too)
    let docker = Docker::connect_with_local_defaults().ok();
    if docker.is_none() {
        tracing::warn!("Docker/Podman socket unavailable; container inspection disabled");
    }

    let state = Arc::new(AppState {
        allowed_user,
        dbus,
        docker,
    });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        // Hookable JSON endpoints for external Tailnet nodes/scripts
        .route("/api/v1/units", get(http_list_units))
        .route("/api/v1/containers", get(http_list_containers))
        .fallback_service(ServeDir::new("dist"))
        .with_state(state);

    let bind_addr = std::env::var("KAAGE_BIND").unwrap_or_else(|_| "127.0.0.1:9090".into());
    let listener = tokio::net::TcpListener::bind(&bind_addr).await.unwrap();
    tracing::info!("kaaged listening on http://{bind_addr}");
    axum::serve(listener, app).await.unwrap();
}

fn check_auth(headers: &HeaderMap, allowed: &Option<String>) -> bool {
    let Some(expected) = allowed else {
        return true;
    };
    headers
        .get("Tailscale-User-Login")
        .and_then(|v| v.to_str().ok())
        == Some(expected.as_str())
}

async fn http_list_units(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    match list_systemd_units(&state.dbus).await {
        Ok(units) => Json(units).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn http_list_containers(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let Some(docker) = &state.docker else {
        return Json(Vec::<ContainerInfo>::new()).into_response();
    };
    Json(list_containers(docker).await).into_response()
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized Tailscale identity").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerMsg>();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if let Ok(bytes) = postcard::to_allocvec(&msg) {
                if ws_tx.send(Message::Binary(bytes.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Background host telemetry ticker (every 1.5s)
    let metrics_tx = out_tx.clone();
    let metrics_task = tokio::spawn(async move {
        let mut sys = System::new_all();
        let hostname = System::host_name().unwrap_or_else(|| "kaage-host".into());
        loop {
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let metrics = HostMetrics {
                hostname: hostname.clone(),
                cpu_percent: sys.global_cpu_usage(),
                mem_used_mb: sys.used_memory() / 1024 / 1024,
                mem_total_mb: sys.total_memory() / 1024 / 1024,
                uptime_secs: System::uptime(),
            };
            if metrics_tx.send(ServerMsg::Metrics(metrics)).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    });

    let ptys: Arc<Mutex<HashMap<u32, PtySession>>> = Arc::new(Mutex::new(HashMap::new()));

    while let Some(Ok(Message::Binary(bytes))) = ws_rx.next().await {
        let Ok(cmd) = postcard::from_bytes::<ClientMsg>(&bytes) else {
            continue;
        };

        match cmd {
            ClientMsg::ListUnits => {
                if let Ok(units) = list_systemd_units(&state.dbus).await {
                    let _ = out_tx.send(ServerMsg::UnitsSnapshot(units));
                }
            }
            ClientMsg::ControlUnit { unit, action } => {
                let res = control_systemd_unit(&state.dbus, &unit, action).await;
                let _ = out_tx.send(ServerMsg::ActionFeedback {
                    target: unit,
                    ok: res.is_ok(),
                    message: res.err().map(|e| e.to_string()).unwrap_or_else(|| "OK".into()),
                });
                if let Ok(units) = list_systemd_units(&state.dbus).await {
                    let _ = out_tx.send(ServerMsg::UnitsSnapshot(units));
                }
            }
            ClientMsg::ListContainers => {
                if let Some(docker) = &state.docker {
                    let list = list_containers(docker).await;
                    let _ = out_tx.send(ServerMsg::ContainersSnapshot(list));
                }
            }
            ClientMsg::ControlContainer { id, action } => {
                if let Some(docker) = &state.docker {
                    let res = control_container(docker, &id, action).await;
                    let _ = out_tx.send(ServerMsg::ActionFeedback {
                        target: id,
                        ok: res.is_ok(),
                        message: res.err().map(|e| e.to_string()).unwrap_or_else(|| "OK".into()),
                    });
                    let list = list_containers(docker).await;
                    let _ = out_tx.send(ServerMsg::ContainersSnapshot(list));
                }
            }
            ClientMsg::SpawnPty {
                session_id,
                cols,
                rows,
                cmd,
            } => {
                let pty_system = native_pty_system();
                let Ok(pair) = pty_system.openpty(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                }) else {
                    continue;
                };

                let mut command = match cmd {
                    Some(args) if !args.is_empty() => {
                        let mut c = CommandBuilder::new(&args[0]);
                        c.args(&args[1..]);
                        c
                    }
                    _ => CommandBuilder::new(
                        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()),
                    ),
                };
                command.env("TERM", "xterm-256color");

                if pair.slave.spawn_command(command).is_err() {
                    continue;
                }
                let Ok(mut reader) = pair.master.try_clone_reader() else {
                    continue;
                };
                let Ok(writer) = pair.master.take_writer() else {
                    continue;
                };

                ptys.lock().unwrap().insert(
                    session_id,
                    PtySession {
                        master: pair.master,
                        writer,
                    },
                );

                let tx_clone = out_tx.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 8192];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => {
                                let _ = tx_clone.send(ServerMsg::PtyExited { session_id });
                                break;
                            }
                            Ok(n) => {
                                let _ = tx_clone.send(ServerMsg::PtyOutput {
                                    session_id,
                                    data: buf[..n].to_vec(),
                                });
                            }
                        }
                    }
                });
            }
            ClientMsg::PtyInput { session_id, data } => {
                if let Some(session) = ptys.lock().unwrap().get_mut(&session_id) {
                    let _ = session.writer.write_all(&data);
                    let _ = session.writer.flush();
                }
            }
            ClientMsg::ResizePty {
                session_id,
                cols,
                rows,
            } => {
                if let Some(session) = ptys.lock().unwrap().get(&session_id) {
                    let _ = session.master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
            ClientMsg::ClosePty { session_id } => {
                ptys.lock().unwrap().remove(&session_id);
            }
        }
    }

    metrics_task.abort();
    send_task.abort();
}

async fn list_systemd_units(conn: &zbus::Connection) -> zbus::Result<Vec<SystemdUnitInfo>> {
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;

    let raw: Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        OwnedObjectPath,
        u32,
        String,
        OwnedObjectPath,
    )> = proxy.call("ListUnits", &()).await?;

    let mut units: Vec<_> = raw
        .into_iter()
        .filter(|u| u.0.ends_with(".service"))
        .map(|u| SystemdUnitInfo {
            name: u.0,
            description: u.1,
            load_state: u.2,
            active_state: u.3,
            sub_state: u.4,
        })
        .collect();
    units.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(units)
}

async fn control_systemd_unit(
    conn: &zbus::Connection,
    unit: &str,
    action: UnitAction,
) -> zbus::Result<()> {
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;

    let method = match action {
        UnitAction::Start => "StartUnit",
        UnitAction::Stop => "StopUnit",
        UnitAction::Restart => "RestartUnit",
        UnitAction::Reload => "ReloadUnit",
    };

    let _: OwnedObjectPath = proxy.call(method, &(unit, "replace")).await?;
    Ok(())
}

async fn list_containers(docker: &Docker) -> Vec<ContainerInfo> {
    let opts = Some(ListContainersOptions::<String> {
        all: true,
        ..Default::default()
    });
    let Ok(items) = docker.list_containers(opts).await else {
        return Vec::new();
    };
    items
        .into_iter()
        .map(|c| {
            let id = c.id.unwrap_or_default();
            let short_id = id.chars().take(12).collect();
            let name = c
                .names
                .and_then(|n| n.into_iter().next())
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string();
            ContainerInfo {
                id: short_id,
                name,
                image: c.image.unwrap_or_default(),
                state: c.state.unwrap_or_default(),
                status: c.status.unwrap_or_default(),
            }
        })
        .collect()
}

async fn control_container(
    docker: &Docker,
    id: &str,
    action: ContainerAction,
) -> Result<(), bollard::errors::Error> {
    match action {
        ContainerAction::Start => {
            docker
                .start_container(id, None::<StartContainerOptions<String>>)
                .await
        }
        ContainerAction::Stop => {
            docker
                .stop_container(id, Some(StopContainerOptions { t: 5 }))
                .await
        }
        ContainerAction::Restart => {
            docker
                .restart_container(id, Some(RestartContainerOptions { t: 5 }))
                .await
        }
        ContainerAction::Remove => {
            docker
                .remove_container(
                    id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await
        }
    }
}
```

## 6. BeOS Theme & 3D Bevel Widgets (`crates/kaage-ui/src/theme.rs`)

```rust
use egui::{Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, Vec2};

pub const BEOS_DESKTOP_BLUE: Color32 = Color32::from_rgb(51, 102, 152);
pub const BEOS_PANEL_GRAY: Color32 = Color32::from_rgb(216, 216, 216);
pub const BEOS_WIDGET_FACE: Color32 = Color32::from_rgb(232, 232, 232);
pub const BEOS_TAB_YELLOW: Color32 = Color32::from_rgb(255, 203, 0);
pub const BEOS_TAB_INACTIVE: Color32 = Color32::from_rgb(230, 230, 230);
pub const BEOS_BEVEL_LIGHT: Color32 = Color32::from_rgb(255, 255, 255);
pub const BEOS_BEVEL_DARK: Color32 = Color32::from_rgb(122, 122, 122);
pub const BEOS_BORDER_BLACK: Color32 = Color32::from_rgb(40, 40, 40);
pub const BEOS_STATUS_GREEN: Color32 = Color32::from_rgb(46, 139, 87);
pub const BEOS_STATUS_RED: Color32 = Color32::from_rgb(178, 34, 34);

pub fn apply_beos_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals.dark_mode = false;
        style.visuals.window_corner_radius = CornerRadius::ZERO;
        style.visuals.menu_corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.noninteractive.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.inactive.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.hovered.corner_radius = CornerRadius::ZERO;
        style.visuals.widgets.active.corner_radius = CornerRadius::ZERO;
        style.visuals.window_shadow = egui::Shadow::NONE;
        style.visuals.popup_shadow = egui::Shadow::NONE;
        style.visuals.panel_fill = BEOS_PANEL_GRAY;
        style.visuals.window_fill = BEOS_PANEL_GRAY;
        style.visuals.selection.bg_fill = BEOS_TAB_YELLOW;
        style.visuals.selection.stroke = Stroke::new(1.0, Color32::BLACK);
    });
}

/// Draws crisp 1px outer black outline + 1px inner 3D bevel highlight/shadow
pub fn draw_beos_bevel(painter: &egui::Painter, rect: Rect, pressed: bool) {
    let (tl, br) = if pressed {
        (BEOS_BEVEL_DARK, BEOS_BEVEL_LIGHT)
    } else {
        (BEOS_BEVEL_LIGHT, BEOS_BEVEL_DARK)
    };
    let inner = rect.shrink(1.0);
    painter.line_segment([inner.left_top(), inner.right_top()], Stroke::new(1.0, tl));
    painter.line_segment([inner.left_top(), inner.left_bottom()], Stroke::new(1.0, tl));
    painter.line_segment([inner.left_bottom(), inner.right_bottom()], Stroke::new(1.0, br));
    painter.line_segment([inner.right_top(), inner.right_bottom()], Stroke::new(1.0, br));
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, BEOS_BORDER_BLACK),
        egui::StrokeKind::Inside,
    );
}

/// Tactile BeOS 3D beveled button
pub fn be_button(ui: &mut Ui, label: &str) -> Response {
    let desired = Vec2::new((label.len() as f32 * 7.5 + 18.0).max(48.0), 22.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    if ui.is_rect_visible(rect) {
        let pressed = response.is_pointer_button_down_on();
        let fill = if pressed {
            Color32::from_rgb(198, 198, 198)
        } else if response.hovered() {
            Color32::from_rgb(242, 242, 242)
        } else {
            BEOS_WIDGET_FACE
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, fill);
        draw_beos_bevel(painter, rect, pressed);

        let text_offset = if pressed { Vec2::splat(1.0) } else { Vec2::ZERO };
        painter.text(
            rect.center() + text_offset,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.5),
            Color32::BLACK,
        );
    }
    response
}
```

## 7. BeOS Deskbar (`crates/kaage-ui/src/deskbar.rs`)

```rust
use crate::theme::{
    draw_beos_bevel, BEOS_BORDER_BLACK, BEOS_PANEL_GRAY, BEOS_STATUS_GREEN, BEOS_TAB_YELLOW,
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
        groups: &[BeWindowGroup],
    ) -> Option<DeskbarAction> {
        let mut action = None;
        let screen = ctx.screen_rect();
        let bar_width = 230.0;
        let top_right = Pos2::new(screen.max.x - bar_width - 8.0, 8.0);

        egui::Area::new(Id::new("kaage_deskbar"))
            .fixed_pos(top_right)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let header_rect = Rect::from_min_size(top_right, Vec2::new(bar_width, 28.0));
                let painter = ui.painter();
                painter.rect_filled(header_rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                draw_beos_bevel(painter, header_rect, false);

                // Left: Kaage Crow Menu Button
                let menu_btn_rect = Rect::from_min_size(top_right, Vec2::new(76.0, 28.0));
                let menu_resp = ui.interact(
                    menu_btn_rect,
                    Id::new("deskbar_crow_btn"),
                    Sense::click(),
                );
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

                // Right: Live Host CPU/RAM Mini-Meters
                let tray_rect = Rect::from_min_max(
                    Pos2::new(menu_btn_rect.max.x, top_right.y),
                    header_rect.max,
                );
                if let Some(m) = metrics {
                    let cpu_frac = (m.cpu_percent / 100.0).clamp(0.0, 1.0);
                    let mem_frac = if m.mem_total_mb > 0 {
                        (m.mem_used_mb as f32 / m.mem_total_mb as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };

                    // CPU bar
                    let cpu_bar = Rect::from_min_size(
                        Pos2::new(tray_rect.min.x + 8.0, tray_rect.min.y + 6.0),
                        Vec2::new(62.0, 6.0),
                    );
                    painter.rect_filled(cpu_bar, CornerRadius::ZERO, Color32::from_rgb(70, 70, 70));
                    painter.rect_filled(
                        Rect::from_min_size(cpu_bar.min, Vec2::new(62.0 * cpu_frac, 6.0)),
                        CornerRadius::ZERO,
                        BEOS_STATUS_GREEN,
                    );

                    // RAM bar
                    let mem_bar = Rect::from_min_size(
                        Pos2::new(tray_rect.min.x + 8.0, tray_rect.min.y + 15.0),
                        Vec2::new(62.0, 6.0),
                    );
                    painter.rect_filled(mem_bar, CornerRadius::ZERO, Color32::from_rgb(70, 70, 70));
                    painter.rect_filled(
                        Rect::from_min_size(mem_bar.min, Vec2::new(62.0 * mem_frac, 6.0)),
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
                            PaneKind::Terminal { .. } => ">_",
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
                            Id::new(("deskbar_item", group.id, tab.id)),
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
                    let menu_pos = Pos2::new(top_right.x - 165.0, top_right.y);
                    let menu_rect = Rect::from_min_size(menu_pos, Vec2::new(164.0, 82.0));
                    painter.rect_filled(menu_rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                    draw_beos_bevel(painter, menu_rect, false);

                    let items = [
                        ("New Ghostty Terminal", DeskbarAction::NewTerminal),
                        ("Systemd Services", DeskbarAction::OpenSystemdInspector),
                        ("Containers", DeskbarAction::OpenDockerInspector),
                    ];
                    for (i, (label, act)) in items.into_iter().enumerate() {
                        let r = Rect::from_min_size(
                            Pos2::new(menu_pos.x + 3.0, menu_pos.y + 3.0 + (i as f32 * 25.0)),
                            Vec2::new(158.0, 24.0),
                        );
                        let resp = ui.interact(r, Id::new(("deskbar_menu", i)), Sense::click());
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
```

## 8. `libghostty-vt` Terminal Renderer (`crates/kaage-ui/src/terminal.rs`)

```rust
use crate::theme::BEOS_TAB_YELLOW;
use egui::{Color32, CornerRadius, FontId, Key, Pos2, Rect, Vec2};
use ghostty_vt::{Terminal, TerminalOptions};
use kaage_proto::ClientMsg;

pub struct GhosttyPane {
    pub session_id: u32,
    pub term: Terminal,
    pub cols: u16,
    pub rows: u16,
    pub cell_size: Vec2,
}

impl GhosttyPane {
    pub fn new(session_id: u32, cols: u16, rows: u16) -> Self {
        let term = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 10_000,
        })
        .expect("Failed to initialize libghostty-vt terminal");

        Self {
            session_id,
            term,
            cols,
            rows,
            cell_size: Vec2::new(8.4, 17.0),
        }
    }

    pub fn feed_bytes(&mut self, data: &[u8]) {
        self.term.feed(data);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, send_msg: &mut dyn FnMut(ClientMsg)) {
        let avail = ui.available_size();
        let new_cols = ((avail.x / self.cell_size.x).floor() as u16).max(20);
        let new_rows = ((avail.y / self.cell_size.y).floor() as u16).max(5);

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;
            let _ = self.term.resize(new_cols, new_rows);
            send_msg(ClientMsg::ResizePty {
                session_id: self.session_id,
                cols: new_cols,
                rows: new_rows,
            });
        }

        let (rect, response) = ui.allocate_exact_size(avail, egui::Sense::click());
        if response.clicked() {
            ui.memory_mut(|m| m.request_focus(response.id));
        }

        if response.has_focus() {
            let mut outbound = Vec::new();
            ui.input(|i| {
                for event in &i.events {
                    match event {
                        egui::Event::Text(text) => outbound.extend_from_slice(text.as_bytes()),
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            if modifiers.ctrl {
                                if let Some(b) = ctrl_key_to_byte(*key) {
                                    outbound.push(b);
                                }
                            } else {
                                match key {
                                    Key::Enter => outbound.push(b'\r'),
                                    Key::Backspace => outbound.push(0x7f),
                                    Key::Tab => outbound.push(b'\t'),
                                    Key::Escape => outbound.push(0x1b),
                                    Key::ArrowUp => outbound.extend_from_slice(b"\x1b[A"),
                                    Key::ArrowDown => outbound.extend_from_slice(b"\x1b[B"),
                                    Key::ArrowRight => outbound.extend_from_slice(b"\x1b[C"),
                                    Key::ArrowLeft => outbound.extend_from_slice(b"\x1b[D"),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
            });

            if !outbound.is_empty() {
                send_msg(ClientMsg::PtyInput {
                    session_id: self.session_id,
                    data: outbound,
                });
            }
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(18, 20, 24));

        let font = FontId::monospace(14.0);
        if let Ok(state) = self.term.render_state() {
            for (r, row) in state.rows().enumerate() {
                let y = rect.min.y + (r as f32 * self.cell_size.y);
                for (c, cell) in row.cells().enumerate() {
                    let x = rect.min.x + (c as f32 * self.cell_size.x);
                    let cell_rect = Rect::from_min_size(Pos2::new(x, y), self.cell_size);

                    if let Some(bg) = cell.bg_rgb() {
                        painter.rect_filled(
                            cell_rect,
                            CornerRadius::ZERO,
                            Color32::from_rgb(bg.r, bg.g, bg.b),
                        );
                    }

                    if let Some(ch) = cell.char() {
                        if ch != ' ' {
                            let fg = cell
                                .fg_rgb()
                                .map(|rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b))
                                .unwrap_or(Color32::from_rgb(232, 232, 232));

                            painter.text(
                                cell_rect.min,
                                egui::Align2::LEFT_TOP,
                                ch,
                                font.clone(),
                                fg,
                            );
                        }
                    }
                }
            }

            if let Some(cursor) = state.cursor() {
                let cx = rect.min.x + (cursor.col as f32 * self.cell_size.x);
                let cy = rect.min.y + (cursor.row as f32 * self.cell_size.y);
                let cursor_rect = Rect::from_min_size(Pos2::new(cx, cy), self.cell_size);
                painter.rect_filled(
                    cursor_rect,
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(
                        BEOS_TAB_YELLOW.r(),
                        BEOS_TAB_YELLOW.g(),
                        BEOS_TAB_YELLOW.b(),
                        185,
                    ),
                );
            }
        }
    }
}

fn ctrl_key_to_byte(key: Key) -> Option<u8> {
    match key {
        Key::A => Some(1),
        Key::C => Some(3),
        Key::D => Some(4),
        Key::E => Some(5),
        Key::K => Some(11),
        Key::L => Some(12),
        Key::R => Some(18),
        Key::U => Some(21),
        Key::W => Some(23),
        Key::Z => Some(26),
        _ => None,
    }
}
```

## 9. Service & Container Inspectors (`crates/kaage-ui/src/inspectors.rs`)

```rust
use crate::theme::{be_button, BEOS_STATUS_GREEN, BEOS_STATUS_RED};
use egui::{Color32, RichText, ScrollArea, Ui};
use kaage_proto::{ClientMsg, ContainerAction, ContainerInfo, SystemdUnitInfo, UnitAction};

pub enum InspectorSpawnRequest {
    OpenCommandTab { title: String, cmd: Vec<String> },
}

pub fn render_systemd_inspector(
    ui: &mut Ui,
    filter: &mut String,
    units: &[SystemdUnitInfo],
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<InspectorSpawnRequest> {
    let mut spawn_req = None;

    ui.horizontal(|ui| {
        ui.label(RichText::new("Filter:").color(Color32::BLACK));
        ui.text_edit_singleline(filter);
        if be_button(ui, "Refresh").clicked() {
            send_msg(ClientMsg::ListUnits);
        }
    });
    ui.separator();

    ScrollArea::vertical().show(ui, |ui| {
        for unit in units {
            if !filter.is_empty()
                && !unit.name.to_lowercase().contains(&filter.to_lowercase())
                && !unit
                    .description
                    .to_lowercase()
                    .contains(&filter.to_lowercase())
            {
                continue;
            }

            ui.horizontal(|ui| {
                let state_color = if unit.active_state == "active" {
                    BEOS_STATUS_GREEN
                } else {
                    BEOS_STATUS_RED
                };
                ui.label(RichText::new("●").color(state_color));
                ui.label(
                    RichText::new(&unit.name)
                        .strong()
                        .color(Color32::BLACK),
                );
                ui.label(
                    RichText::new(format!("({})", unit.sub_state))
                        .color(Color32::from_rgb(70, 70, 70)),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if be_button(ui, "Logs").clicked() {
                        spawn_req = Some(InspectorSpawnRequest::OpenCommandTab {
                            title: format!("logs: {}", unit.name),
                            cmd: vec![
                                "journalctl".into(),
                                "-u".into(),
                                unit.name.clone(),
                                "-n".into(),
                                "100".into(),
                                "-f".into(),
                            ],
                        });
                    }
                    if be_button(ui, "Restart").clicked() {
                        send_msg(ClientMsg::ControlUnit {
                            unit: unit.name.clone(),
                            action: UnitAction::Restart,
                        });
                    }
                    if unit.active_state == "active" {
                        if be_button(ui, "Stop").clicked() {
                            send_msg(ClientMsg::ControlUnit {
                                unit: unit.name.clone(),
                                action: UnitAction::Stop,
                            });
                        }
                    } else if be_button(ui, "Start").clicked() {
                        send_msg(ClientMsg::ControlUnit {
                            unit: unit.name.clone(),
                            action: UnitAction::Start,
                        });
                    }
                });
            });
            ui.separator();
        }
    });

    spawn_req
}

pub fn render_docker_inspector(
    ui: &mut Ui,
    filter: &mut String,
    containers: &[ContainerInfo],
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<InspectorSpawnRequest> {
    let mut spawn_req = None;

    ui.horizontal(|ui| {
        ui.label(RichText::new("Filter:").color(Color32::BLACK));
        ui.text_edit_singleline(filter);
        if be_button(ui, "Refresh").clicked() {
            send_msg(ClientMsg::ListContainers);
        }
    });
    ui.separator();

    ScrollArea::vertical().show(ui, |ui| {
        for ctr in containers {
            if !filter.is_empty() && !ctr.name.to_lowercase().contains(&filter.to_lowercase()) {
                continue;
            }

            ui.horizontal(|ui| {
                let state_color = if ctr.state == "running" {
                    BEOS_STATUS_GREEN
                } else {
                    BEOS_STATUS_RED
                };
                ui.label(RichText::new("■").color(state_color));
                ui.label(RichText::new(&ctr.name).strong().color(Color32::BLACK));
                ui.label(
                    RichText::new(&ctr.image).color(Color32::from_rgb(75, 75, 75)),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ctr.state == "running" {
                        if be_button(ui, "Shell").clicked() {
                            spawn_req = Some(InspectorSpawnRequest::OpenCommandTab {
                                title: format!("sh: {}", ctr.name),
                                cmd: vec![
                                    "docker".into(),
                                    "exec".into(),
                                    "-it".into(),
                                    ctr.id.clone(),
                                    "/bin/sh".into(),
                                ],
                            });
                        }
                        if be_button(ui, "Restart").clicked() {
                            send_msg(ClientMsg::ControlContainer {
                                id: ctr.id.clone(),
                                action: ContainerAction::Restart,
                            });
                        }
                        if be_button(ui, "Stop").clicked() {
                            send_msg(ClientMsg::ControlContainer {
                                id: ctr.id.clone(),
                                action: ContainerAction::Stop,
                            });
                        }
                    } else if be_button(ui, "Start").clicked() {
                        send_msg(ClientMsg::ControlContainer {
                            id: ctr.id.clone(),
                            action: ContainerAction::Start,
                        });
                    }
                });
            });
            ui.separator();
        }
    });

    spawn_req
}
```

## 10. Stack & Tile Window Manager (`crates/kaage-ui/src/wm.rs`)

```rust
use crate::inspectors::{
    render_docker_inspector, render_systemd_inspector, InspectorSpawnRequest,
};
use crate::terminal::GhosttyPane;
use crate::theme::{
    draw_beos_bevel, BEOS_PANEL_GRAY, BEOS_TAB_INACTIVE, BEOS_TAB_YELLOW,
};
use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Vec2};
use kaage_proto::{ClientMsg, ContainerInfo, SystemdUnitInfo};

pub enum PaneKind {
    Terminal(GhosttyPane),
    SystemdInspector { filter: String },
    DockerInspector { filter: String },
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
        units: &[SystemdUnitInfo],
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

            egui::Area::new(Id::new(("be_group", group.id)))
                .current_pos(group.pos)
                .order(egui::Order::Middle)
                .show(ctx, |ui| {
                    let painter = ui.painter();
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
                        painter.rect_filled(tab_rect, CornerRadius::ZERO, fill);
                        draw_beos_bevel(painter, tab_rect, false);

                        // BeOS Left Close Box
                        let close_rect = Rect::from_min_size(
                            Pos2::new(tab_rect.min.x + 5.0, tab_rect.min.y + 5.0),
                            Vec2::splat(11.0),
                        );
                        draw_beos_bevel(painter, close_rect, false);
                        let close_resp = ui.interact(
                            close_rect,
                            Id::new(("be_close", group.id, tab.id)),
                            Sense::click(),
                        );
                        if close_resp.clicked() {
                            close_req = Some((group.id, idx));
                        }

                        // Tab Title
                        painter.text(
                            Pos2::new(close_rect.max.x + 6.0, tab_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &tab.title,
                            egui::FontId::proportional(13.0),
                            Color32::BLACK,
                        );

                        let tab_resp = ui.interact(
                            tab_rect,
                            Id::new(("be_tab", group.id, tab.id)),
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

                        if tab_resp.drag_stopped() {
                            if let Some(ptr) = ui.input(|i| i.pointer.interact_pos()) {
                                for (target_id, zone) in &header_zones {
                                    if *target_id != group.id && zone.contains(ptr) {
                                        merge_req = Some((group.id, idx, *target_id));
                                        break;
                                    }
                                }
                            }
                        }

                        cursor_x += tab_width - 1.0;
                    }

                    if !group.shaded {
                        let body_rect = Rect::from_min_size(body_pos, group.size);
                        painter.rect_filled(body_rect, CornerRadius::ZERO, BEOS_PANEL_GRAY);
                        draw_beos_bevel(painter, body_rect, false);

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
                                PaneKind::SystemdInspector { filter } => {
                                    if let Some(req) = render_systemd_inspector(
                                        &mut child_ui,
                                        filter,
                                        units,
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
                        let resize_resp = ui.interact(
                            grip_rect,
                            Id::new(("be_resize", group.id)),
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
        if let Some(gid) = bring_to_front {
            if let Some(pos) = self.groups.iter().position(|g| g.id == gid) {
                let g = self.groups.remove(pos);
                self.groups.push(g);
            }
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
```

## 11. Frontend Entrypoint (`crates/kaage-ui/src/main.rs`)

```rust
mod deskbar;
mod inspectors;
mod terminal;
mod theme;
mod wm;

use deskbar::{BeDeskbar, DeskbarAction};
use eframe::egui::{self, Pos2, Vec2};
use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use inspectors::InspectorSpawnRequest;
use kaage_proto::{ClientMsg, ContainerInfo, HostMetrics, ServerMsg, SystemdUnitInfo};
use terminal::GhosttyPane;
use theme::{apply_beos_style, BEOS_DESKTOP_BLUE};
use wm::{BeWindowManager, PaneKind};

struct KaageApp {
    ws_tx: WsSender,
    ws_rx: WsReceiver,
    wm: BeWindowManager,
    deskbar: BeDeskbar,
    metrics: Option<HostMetrics>,
    units: Vec<SystemdUnitInfo>,
    containers: Vec<ContainerInfo>,
    next_pty_id: u32,
}

impl KaageApp {
    fn new(cc: &eframe::CreationContext<'_>, ws_url: &str) -> Self {
        apply_beos_style(&cc.egui_ctx);

        let ctx_clone = cc.egui_ctx.clone();
        let (mut ws_tx, ws_rx) = ewebsock::connect_with_wakeup(
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
            },
            Pos2::new(24.0, 24.0),
            Vec2::new(540.0, 340.0),
        );

        let initial_pty = 1;
        wm.open_window(
            "Ghostty — bash".into(),
            PaneKind::Terminal(GhosttyPane::new(initial_pty, 80, 24)),
            Pos2::new(60.0, 395.0),
            Vec2::new(680.0, 410.0),
        );

        // Request initial snapshots & spawn default terminal PTY
        for msg in [
            ClientMsg::ListUnits,
            ClientMsg::ListContainers,
            ClientMsg::SpawnPty {
                session_id: initial_pty,
                cols: 80,
                rows: 24,
                cmd: None,
            },
        ] {
            if let Ok(bytes) = postcard::to_allocvec(&msg) {
                ws_tx.send(WsMessage::Binary(bytes));
            }
        }

        Self {
            ws_tx,
            ws_rx,
            wm,
            deskbar: BeDeskbar::new(),
            metrics: None,
            units: Vec::new(),
            containers: Vec::new(),
            next_pty_id: 2,
        }
    }

    fn send_client_msg(&mut self, msg: ClientMsg) {
        if let Ok(bytes) = postcard::to_allocvec(&msg) {
            self.ws_tx.send(WsMessage::Binary(bytes));
        }
    }

    fn spawn_terminal_window(&mut self, title: String, cmd: Option<Vec<String>>) {
        let session_id = self.next_pty_id;
        self.next_pty_id += 1;

        self.wm.open_window(
            title,
            PaneKind::Terminal(GhosttyPane::new(session_id, 80, 24)),
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Some(event) = self.ws_rx.try_recv() {
            if let WsEvent::Message(WsMessage::Binary(bytes)) = event {
                if let Ok(server_msg) = postcard::from_bytes::<ServerMsg>(&bytes) {
                    match server_msg {
                        ServerMsg::Metrics(m) => self.metrics = Some(m),
                        ServerMsg::UnitsSnapshot(u) => self.units = u,
                        ServerMsg::ContainersSnapshot(c) => self.containers = c,
                        ServerMsg::PtyOutput { session_id, data } => {
                            for group in &mut self.wm.groups {
                                for tab in &mut group.tabs {
                                    if let PaneKind::Terminal(term) = &mut tab.kind {
                                        if term.session_id == session_id {
                                            term.feed_bytes(&data);
                                        }
                                    }
                                }
                            }
                        }
                        ServerMsg::PtyExited { session_id } => {
                            for group in &mut self.wm.groups {
                                for tab in &mut group.tabs {
                                    if let PaneKind::Terminal(term) = &mut tab.kind {
                                        if term.session_id == session_id {
                                            term.feed_bytes(b"\r\n[Process exited]\r\n");
                                        }
                                    }
                                }
                            }
                        }
                        ServerMsg::ActionFeedback { .. } => {}
                    }
                }
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BEOS_DESKTOP_BLUE))
            .show(ctx, |_ui| {});

        let mut outbound = Vec::new();
        let spawn_reqs = self.wm.show(ctx, &self.units, &self.containers, &mut |msg| {
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

        if let Some(action) = self.deskbar.show(ctx, self.metrics.as_ref(), &self.wm.groups) {
            match action {
                DeskbarAction::NewTerminal => {
                    self.spawn_terminal_window("Ghostty — bash".into(), None);
                }
                DeskbarAction::OpenSystemdInspector => {
                    self.wm.open_window(
                        "Systemd Services".into(),
                        PaneKind::SystemdInspector {
                            filter: String::new(),
                        },
                        Pos2::new(48.0, 48.0),
                        Vec2::new(540.0, 340.0),
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
    let options = eframe::NativeOptions::default();
    let _ = eframe::run_native(
        "kaage",
        options,
        Box::new(|cc| Ok(Box::new(KaageApp::new(cc, "ws://127.0.0.1:9090/ws")))),
    );
}
```

## 12. Build & Tailscale Deployment

### 1. Prerequisites

- Rust toolchain with `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)
- `trunk` (`cargo install trunk`)
- `zig` installed on `PATH` (used automatically by `ghostty-vt` during WASM compilation)

### 2. Compile Frontend & Daemon

```bash
# Build WASM webtop bundle into ./dist
trunk build --release

# Build native host daemon
cargo build --release -p kaaged
```

### 3. Run & Expose over Tailscale

```bash
# Start daemon bound to localhost, locked to your Tailscale identity
KAAGE_ALLOWED_USER="you@example.com" ./target/release/kaaged

# In a separate terminal, expose with automatic Let's Encrypt TLS + identity headers
sudo tailscale serve --bg 9090
```

Access `https://<node-name>.<tailnet-name>.ts.net` from any device on your Tailnet.

## 13. Controls & Window Mechanics Reference

- **Move Window:** Drag any yellow title tab.
- **Slide Yellow Tab Along Top Edge:** Hold `Shift` + drag the yellow title tab.
- **Stack Windows into a Tab Group:** Drag one window's yellow tab onto another window's title bar area and release.
- **Detach Tab into Separate Window:** Drag a tab out of a multi-tab group.
- **Windowshade (Collapse Body):** Double-click any yellow title tab.
- **Resize Window:** Drag the bottom-right corner grip.
- **Deskbar (Top-Right):** Click the `kaage` crow button to launch new Ghostty terminals or service inspectors; click any stacked item underneath to focus and unshade that window tab.

