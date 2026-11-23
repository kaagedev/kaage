mod nixos;

use axum::{
    Json, Router,
    extract::{
        Path as AxumPath, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use bollard::{
    Docker,
    container::{
        ListContainersOptions, RemoveContainerOptions, RestartContainerOptions,
        StartContainerOptions, StopContainerOptions,
    },
};
use futures_util::{SinkExt, StreamExt};
use kaage_proto::{
    ClientMsg, ContainerAction, ContainerInfo, HostMetrics, NixosInfo, ServerMsg, SystemdUnitInfo,
    SystemdUnitStatus, UnitAction, UnitScope,
};
use nixos::{
    LoadedUnitRecord, UnitFileRecord, detect_nixos_info, is_unit_file_enabled, is_unit_running,
    merge_units_and_files, normalize_service_name, resolve_nix_store_path,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use sysinfo::System;
use tokio::sync::{RwLock, mpsc};
use tower_http::services::ServeDir;
use zbus::zvariant::OwnedObjectPath;

struct AppState {
    allowed_user: Option<String>,
    nixos: NixosInfo,
    system_dbus: RwLock<Option<zbus::Connection>>,
    user_dbus: RwLock<Option<zbus::Connection>>,
    allow_system_backends: bool,
    docker: Option<Docker>,
    dist_dir: PathBuf,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    hostname: String,
    dbus_system: bool,
    dbus_user: bool,
    docker: bool,
    nixos: NixosInfo,
}

#[derive(Deserialize, Default)]
struct UnitsQuery {
    scope: Option<String>,
    enabled: Option<bool>,
    running: Option<bool>,
    state: Option<String>,
    q: Option<String>,
}

#[derive(Deserialize, Default)]
struct ScopeQuery {
    scope: Option<String>,
}

#[derive(Serialize)]
struct UnitControlResponse {
    target: String,
    scope: UnitScope,
    action: String,
    ok: bool,
    message: String,
    status: Option<SystemdUnitStatus>,
}

fn build_router(state: Arc<AppState>) -> Router {
    let dist_dir = state.dist_dir.clone();
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/v1/health", get(http_health))
        .route("/api/v1/nixos", get(http_nixos))
        .route("/api/v1/metrics", get(http_metrics))
        .route("/api/v1/units", get(http_list_units))
        .route("/api/v1/units/{unit}", get(http_unit_status))
        .route("/api/v1/units/{unit}/status", get(http_unit_status))
        .route("/api/v1/units/{unit}/{action}", post(http_control_unit))
        .route("/api/v1/containers", get(http_list_containers))
        .fallback_service(ServeDir::new(dist_dir))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kaaged=info,tower_http=info".into()),
        )
        .init();

    let allowed_user = std::env::var("KAAGE_ALLOWED_USER")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let dist_dir = std::env::var("KAAGE_DIST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("dist"));

    let nixos = detect_nixos_info();
    if nixos.is_nixos {
        tracing::info!(
            "Detected {} (generation: {:?}, system: {:?})",
            nixos.pretty_name,
            nixos.generation,
            nixos.current_system
        );
    }

    let system_dbus = match zbus::Connection::system().await {
        Ok(conn) => {
            tracing::info!("Connected to system D-Bus");
            Some(conn)
        }
        Err(err) => {
            tracing::warn!("System D-Bus unavailable ({err}); will retry or fall back to systemctl");
            None
        }
    };

    let user_dbus = match connect_user_dbus().await {
        Ok(conn) => {
            tracing::info!("Connected to user session D-Bus");
            Some(conn)
        }
        Err(err) => {
            tracing::debug!(
                "User session D-Bus unavailable at startup ({err}); will reconnect lazily"
            );
            None
        }
    };

    // Connects to /var/run/docker.sock or DOCKER_HOST (works with Podman socket too)
    let docker = Docker::connect_with_local_defaults().ok();
    if docker.is_none() {
        tracing::warn!("Docker/Podman socket unavailable; container inspection disabled");
    }

    if let Some(ref user) = allowed_user {
        tracing::info!("Tailscale auth enabled for user: {user}");
    } else {
        tracing::warn!("KAAGE_ALLOWED_USER not set; running without Tailscale-User-Login check");
    }

    let state = Arc::new(AppState {
        allowed_user,
        nixos,
        system_dbus: RwLock::new(system_dbus),
        user_dbus: RwLock::new(user_dbus),
        allow_system_backends: true,
        docker,
        dist_dir: dist_dir.clone(),
    });

    let app = build_router(state);

    let bind_addr = std::env::var("KAAGE_BIND").unwrap_or_else(|_| "127.0.0.1:9090".into());
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("Failed to bind TCP listener");
    tracing::info!(
        "kaaged listening on http://{bind_addr} (serving UI from {})",
        dist_dir.display()
    );
    axum::serve(listener, app).await.expect("axum serve failed");
}

fn current_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse::<u32>().ok();
        }
    }
    None
}

async fn connect_user_dbus() -> zbus::Result<zbus::Connection> {
    if let Ok(conn) = zbus::Connection::session().await {
        return Ok(conn);
    }
    // On NixOS, kaaged.service starts at boot before /run/user/<uid>/bus may exist.
    // Probe the standard user runtime bus socket directly when DBUS_SESSION_BUS_ADDRESS wasn't set.
    let bus_path = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(|d| PathBuf::from(d).join("bus"))
        .or_else(|| current_uid().map(|uid| PathBuf::from(format!("/run/user/{uid}/bus"))));

    if let Some(path) = bus_path
        && path.exists()
    {
        let addr = format!("unix:path={}", path.display());
        return zbus::connection::Builder::address(addr.as_str())?
            .build()
            .await;
    }

    zbus::Connection::session().await
}

async fn get_dbus(state: &AppState, scope: UnitScope) -> Option<zbus::Connection> {
    let slot = match scope {
        UnitScope::System => &state.system_dbus,
        UnitScope::User => &state.user_dbus,
    };

    if let Some(conn) = slot.read().await.as_ref() {
        return Some(conn.clone());
    }

    if !state.allow_system_backends {
        return None;
    }

    let connected = match scope {
        UnitScope::System => zbus::Connection::system().await.ok(),
        UnitScope::User => connect_user_dbus().await.ok(),
    };

    if let Some(conn) = connected {
        tracing::info!("Lazily connected to {scope:?} D-Bus");
        *slot.write().await = Some(conn.clone());
        return Some(conn);
    }

    None
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

fn parse_scope_str(s: &str) -> Option<UnitScope> {
    match s.trim().to_ascii_lowercase().as_str() {
        "system" | "sys" => Some(UnitScope::System),
        "user" | "usr" => Some(UnitScope::User),
        _ => None,
    }
}

fn parse_unit_action(s: &str) -> Option<UnitAction> {
    match s.trim().to_ascii_lowercase().as_str() {
        "start" => Some(UnitAction::Start),
        "stop" => Some(UnitAction::Stop),
        "restart" => Some(UnitAction::Restart),
        "reload" => Some(UnitAction::Reload),
        _ => None,
    }
}

async fn http_health(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let dbus_system = get_dbus(&state, UnitScope::System).await.is_some();
    let dbus_user = get_dbus(&state, UnitScope::User).await.is_some();
    Json(HealthResponse {
        status: "ok",
        hostname: System::host_name().unwrap_or_else(|| "kaage-host".into()),
        dbus_system,
        dbus_user,
        docker: state.docker.is_some(),
        nixos: state.nixos.clone(),
    })
    .into_response()
}

async fn http_nixos(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    Json(state.nixos.clone()).into_response()
}

async fn http_metrics(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    Json(HostMetrics {
        hostname: System::host_name().unwrap_or_else(|| "kaage-host".into()),
        cpu_percent: sys.global_cpu_usage(),
        mem_used_mb: sys.used_memory() / 1024 / 1024,
        mem_total_mb: sys.total_memory() / 1024 / 1024,
        uptime_secs: System::uptime(),
    })
    .into_response()
}

fn filter_units(units: Vec<SystemdUnitInfo>, query: &UnitsQuery) -> Vec<SystemdUnitInfo> {
    let scope_filter = query.scope.as_deref().and_then(parse_scope_str);
    let state_filter = query
        .state
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    let needle = query
        .q
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    units
        .into_iter()
        .filter(|u| {
            if let Some(scope) = scope_filter
                && u.scope != scope
            {
                return false;
            }
            if let Some(enabled) = query.enabled
                && u.enabled != enabled
            {
                return false;
            }
            if let Some(running) = query.running
                && u.running != running
            {
                return false;
            }
            if let Some(ref st) = state_filter
                && u.active_state.to_ascii_lowercase() != *st
                && u.sub_state.to_ascii_lowercase() != *st
            {
                return false;
            }
            if let Some(ref q) = needle
                && !u.name.to_ascii_lowercase().contains(q)
                && !u.description.to_ascii_lowercase().contains(q)
            {
                return false;
            }
            true
        })
        .collect()
}

async fn http_list_units(
    headers: HeaderMap,
    Query(query): Query<UnitsQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let units = list_all_systemd_units(&state).await;
    Json(filter_units(units, &query)).into_response()
}

async fn resolve_unit_scope(
    state: &AppState,
    normalized_unit: &str,
    explicit: Option<UnitScope>,
) -> UnitScope {
    if let Some(scope) = explicit {
        return scope;
    }
    let units = list_all_systemd_units(state).await;
    if units
        .iter()
        .any(|u| u.name == normalized_unit && u.scope == UnitScope::System)
    {
        return UnitScope::System;
    }
    if units
        .iter()
        .any(|u| u.name == normalized_unit && u.scope == UnitScope::User)
    {
        return UnitScope::User;
    }
    UnitScope::System
}

async fn http_unit_status(
    headers: HeaderMap,
    AxumPath(unit): AxumPath<String>,
    Query(query): Query<ScopeQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let normalized = normalize_service_name(&unit);
    let explicit_scope = query.scope.as_deref().and_then(parse_scope_str);
    let scope = resolve_unit_scope(&state, &normalized, explicit_scope).await;

    match get_unit_status(&state, &normalized, scope).await {
        Ok(status) => Json(status).into_response(),
        Err(err) => (StatusCode::NOT_FOUND, err).into_response(),
    }
}

async fn http_control_unit(
    headers: HeaderMap,
    AxumPath((unit, action_str)): AxumPath<(String, String)>,
    Query(query): Query<ScopeQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let Some(action) = parse_unit_action(&action_str) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("Unsupported action '{action_str}'; expected start, stop, restart, or reload"),
        )
            .into_response();
    };
    let normalized = normalize_service_name(&unit);
    let explicit_scope = query.scope.as_deref().and_then(parse_scope_str);
    let scope = resolve_unit_scope(&state, &normalized, explicit_scope).await;

    let res = control_systemd_unit(&state, &normalized, scope, action).await;
    if res.is_ok() {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let status = get_unit_status(&state, &normalized, scope).await.ok();

    let (code, ok, message) = match res {
        Ok(()) => (StatusCode::OK, true, "OK".to_string()),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, false, e),
    };

    (
        code,
        Json(UnitControlResponse {
            target: normalized,
            scope,
            action: action_str.to_ascii_lowercase(),
            ok,
            message,
            status,
        }),
    )
        .into_response()
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
    child: Box<dyn Child + Send + Sync>,
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerMsg>();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if let Ok(bytes) = postcard::to_allocvec(&msg)
                && ws_tx.send(Message::Binary(bytes.into())).await.is_err()
            {
                break;
            }
        }
    });

    // Send NixOS host metadata immediately on connect
    let _ = out_tx.send(ServerMsg::NixosInfo(state.nixos.clone()));

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

    while let Some(Ok(msg)) = ws_rx.next().await {
        let Message::Binary(bytes) = msg else {
            continue;
        };
        let Ok(cmd) = postcard::from_bytes::<ClientMsg>(&bytes) else {
            continue;
        };

        match cmd {
            ClientMsg::ListUnits => {
                let units = list_all_systemd_units(&state).await;
                let _ = out_tx.send(ServerMsg::UnitsSnapshot(units));
            }
            ClientMsg::GetUnitStatus { unit, scope } => {
                let normalized = normalize_service_name(&unit);
                match get_unit_status(&state, &normalized, scope).await {
                    Ok(status) => {
                        let _ = out_tx.send(ServerMsg::UnitStatus(status));
                    }
                    Err(err) => {
                        let _ = out_tx.send(ServerMsg::ActionFeedback {
                            target: normalized,
                            ok: false,
                            message: err,
                        });
                    }
                }
            }
            ClientMsg::ControlUnit {
                unit,
                scope,
                action,
            } => {
                let normalized = normalize_service_name(&unit);
                let res = control_systemd_unit(&state, &normalized, scope, action).await;
                if res.is_ok() {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                let _ = out_tx.send(ServerMsg::ActionFeedback {
                    target: normalized.clone(),
                    ok: res.is_ok(),
                    message: res.err().unwrap_or_else(|| "OK".into()),
                });
                let units = list_all_systemd_units(&state).await;
                let _ = out_tx.send(ServerMsg::UnitsSnapshot(units));
                if let Ok(status) = get_unit_status(&state, &normalized, scope).await {
                    let _ = out_tx.send(ServerMsg::UnitStatus(status));
                }
            }
            ClientMsg::ListContainers => {
                let list = match &state.docker {
                    Some(docker) => list_containers(docker).await,
                    None => Vec::new(),
                };
                let _ = out_tx.send(ServerMsg::ContainersSnapshot(list));
            }
            ClientMsg::ControlContainer { id, action } => {
                if let Some(docker) = &state.docker {
                    let res = control_container(docker, &id, action).await;
                    let _ = out_tx.send(ServerMsg::ActionFeedback {
                        target: id,
                        ok: res.is_ok(),
                        message: res
                            .err()
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "OK".into()),
                    });
                    let list = list_containers(docker).await;
                    let _ = out_tx.send(ServerMsg::ContainersSnapshot(list));
                } else {
                    let _ = out_tx.send(ServerMsg::ActionFeedback {
                        target: id,
                        ok: false,
                        message: "Docker/Podman socket unavailable".into(),
                    });
                }
            }
            ClientMsg::SpawnPty {
                session_id,
                cols,
                rows,
                cmd,
            } => {
                let cols = cols.clamp(1, 500);
                let rows = rows.clamp(1, 200);
                let pty_system = native_pty_system();
                let Ok(pair) = pty_system.openpty(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                }) else {
                    let _ = out_tx.send(ServerMsg::PtyExited { session_id });
                    continue;
                };

                let mut command = match cmd {
                    Some(args) if !args.is_empty() => {
                        let program = resolve_command_binary(&args[0]);
                        let mut c = CommandBuilder::new(program);
                        c.args(&args[1..]);
                        c
                    }
                    _ => CommandBuilder::new(
                        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                    ),
                };
                command.env("TERM", "xterm-256color");
                command.env("COLORTERM", "truecolor");
                if let Ok(home) = std::env::var("HOME") {
                    command.cwd(home);
                }

                let Ok(child) = pair.slave.spawn_command(command) else {
                    let _ = out_tx.send(ServerMsg::PtyOutput {
                        session_id,
                        data: b"Failed to spawn command\r\n".to_vec(),
                    });
                    let _ = out_tx.send(ServerMsg::PtyExited { session_id });
                    continue;
                };
                drop(pair.slave);

                let Ok(mut reader) = pair.master.try_clone_reader() else {
                    let _ = out_tx.send(ServerMsg::PtyExited { session_id });
                    continue;
                };
                let Ok(writer) = pair.master.take_writer() else {
                    let _ = out_tx.send(ServerMsg::PtyExited { session_id });
                    continue;
                };

                ptys.lock().unwrap().insert(
                    session_id,
                    PtySession {
                        master: pair.master,
                        writer,
                        child,
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
                                if tx_clone
                                    .send(ServerMsg::PtyOutput {
                                        session_id,
                                        data: buf[..n].to_vec(),
                                    })
                                    .is_err()
                                {
                                    break;
                                }
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
                let cols = cols.clamp(1, 500);
                let rows = rows.clamp(1, 200);
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

    // Kill and clean up any live PTY child processes on disconnect
    ptys.lock().unwrap().clear();
    metrics_task.abort();
    send_task.abort();
}

fn resolve_command_binary(program: &str) -> String {
    if program == "docker"
        && let Ok(cli) = std::env::var("KAAGE_CONTAINER_CLI")
        && !cli.trim().is_empty()
    {
        return cli;
    }
    program.to_string()
}

async fn list_all_systemd_units(state: &AppState) -> Vec<SystemdUnitInfo> {
    let mut all = Vec::new();
    for scope in [UnitScope::System, UnitScope::User] {
        if let Some(conn) = get_dbus(state, scope).await {
            match list_systemd_units(&conn, scope).await {
                Ok(mut units) => all.append(&mut units),
                Err(err) => tracing::warn!("Failed to list {scope:?} systemd units: {err}"),
            }
        }
    }
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

async fn list_systemd_units(
    conn: &zbus::Connection,
    scope: UnitScope,
) -> zbus::Result<Vec<SystemdUnitInfo>> {
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;

    let raw_units: Vec<(
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

    let loaded: Vec<LoadedUnitRecord> = raw_units
        .into_iter()
        .map(|u| LoadedUnitRecord {
            name: u.0,
            description: u.1,
            load_state: u.2,
            active_state: u.3,
            sub_state: u.4,
        })
        .collect();

    let raw_files: Vec<(String, String)> = match proxy.call("ListUnitFiles", &()).await {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!("ListUnitFiles failed for {scope:?}: {err}");
            Vec::new()
        }
    };

    let unit_files: Vec<UnitFileRecord> = raw_files
        .into_iter()
        .map(|(path, state)| UnitFileRecord { path, state })
        .collect();

    Ok(merge_units_and_files(
        loaded,
        unit_files,
        scope,
        &resolve_nix_store_path,
    ))
}

async fn run_systemctl_status(unit: &str, scope: UnitScope) -> Option<String> {
    let mut cmd = tokio::process::Command::new("systemctl");
    if scope == UnitScope::User {
        cmd.arg("--user");
    }
    cmd.args(["status", unit, "--no-pager", "-n", "12"]);
    let output = cmd.output().await.ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.trim().is_empty() {
        stdout.trim_end().to_string()
    } else if stdout.trim().is_empty() {
        stderr.trim_end().to_string()
    } else {
        format!("{}\n{}", stdout.trim_end(), stderr.trim_end())
    };
    if combined.is_empty() {
        None
    } else {
        Some(combined)
    }
}

async fn get_unit_status(
    state: &AppState,
    unit: &str,
    scope: UnitScope,
) -> Result<SystemdUnitStatus, String> {
    let Some(conn) = get_dbus(state, scope).await else {
        return Err(format!("{scope:?} D-Bus connection unavailable"));
    };

    let manager = zbus::Proxy::new(
        &conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .map_err(|e| e.to_string())?;

    let unit_path: OwnedObjectPath = manager
        .call("LoadUnit", &(unit,))
        .await
        .map_err(|e| e.to_string())?;

    let unit_proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.systemd1",
        unit_path.as_str(),
        "org.freedesktop.systemd1.Unit",
    )
    .await
    .map_err(|e| e.to_string())?;

    let description: String = unit_proxy
        .get_property("Description")
        .await
        .unwrap_or_default();
    let load_state: String = unit_proxy
        .get_property("LoadState")
        .await
        .unwrap_or_default();
    let active_state: String = unit_proxy
        .get_property("ActiveState")
        .await
        .unwrap_or_default();
    let sub_state: String = unit_proxy
        .get_property("SubState")
        .await
        .unwrap_or_default();
    let unit_file_state: String = unit_proxy
        .get_property("UnitFileState")
        .await
        .unwrap_or_default();
    let fragment_path: String = unit_proxy
        .get_property("FragmentPath")
        .await
        .unwrap_or_default();
    let active_enter_timestamp_usec: u64 = unit_proxy
        .get_property("ActiveEnterTimestamp")
        .await
        .unwrap_or(0);

    let (main_pid, memory_current, result) = if unit.ends_with(".service") {
        match zbus::Proxy::new(
            &conn,
            "org.freedesktop.systemd1",
            unit_path.as_str(),
            "org.freedesktop.systemd1.Service",
        )
        .await
        {
            Ok(svc_proxy) => {
                let pid: u32 = svc_proxy.get_property("MainPID").await.unwrap_or(0);
                let mem_raw: u64 = svc_proxy
                    .get_property("MemoryCurrent")
                    .await
                    .unwrap_or(u64::MAX);
                let mem = if mem_raw == u64::MAX {
                    None
                } else {
                    Some(mem_raw)
                };
                let res: String = svc_proxy.get_property("Result").await.unwrap_or_default();
                (pid, mem, res)
            }
            Err(_) => (0, None, String::new()),
        }
    } else {
        (0, None, String::new())
    };

    let nix_store_path = resolve_nix_store_path(&fragment_path);
    let nixos_managed = nix_store_path.is_some();
    let enabled = is_unit_file_enabled(&unit_file_state);
    let running = is_unit_running(&active_state, &sub_state);

    let status_text = run_systemctl_status(unit, scope).await.unwrap_or_else(|| {
        format!(
            "{unit} - {description}\n  Loaded: {load_state} ({fragment_path}; {unit_file_state})\n  Active: {active_state} ({sub_state})\n  Main PID: {main_pid}"
        )
    });

    Ok(SystemdUnitStatus {
        name: unit.to_string(),
        scope,
        description,
        load_state,
        active_state,
        sub_state,
        unit_file_state,
        enabled,
        running,
        fragment_path,
        nix_store_path,
        nixos_managed,
        main_pid,
        memory_current,
        active_enter_timestamp_usec,
        result,
        status_text,
    })
}

async fn control_systemd_unit(
    state: &AppState,
    unit: &str,
    scope: UnitScope,
    action: UnitAction,
) -> Result<(), String> {
    if let Some(conn) = get_dbus(state, scope).await {
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
        )
        .await
        .map_err(|e| e.to_string())?;

        let method = match action {
            UnitAction::Start => "StartUnit",
            UnitAction::Stop => "StopUnit",
            UnitAction::Restart => "RestartUnit",
            UnitAction::Reload => "ReloadUnit",
        };

        let _: OwnedObjectPath = proxy
            .call(method, &(unit, "replace"))
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    if !state.allow_system_backends {
        return Err(format!("{scope:?} D-Bus connection unavailable"));
    }

    // Fallback to systemctl CLI when D-Bus is unavailable
    let verb = match action {
        UnitAction::Start => "start",
        UnitAction::Stop => "stop",
        UnitAction::Restart => "restart",
        UnitAction::Reload => "reload",
    };
    let mut cmd = tokio::process::Command::new("systemctl");
    if scope == UnitScope::User {
        cmd.arg("--user");
    }
    cmd.args([verb, unit]);
    let output = cmd.output().await.map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_state(allowed_user: Option<&str>) -> Arc<AppState> {
        Arc::new(AppState {
            allowed_user: allowed_user.map(str::to_string),
            nixos: NixosInfo {
                is_nixos: true,
                pretty_name: "NixOS 26.11 (Zokor)".into(),
                version: "26.11 (Zokor)".into(),
                current_system: Some("/nix/store/test-nixos-system".into()),
                generation: Some(42),
            },
            system_dbus: RwLock::new(None),
            user_dbus: RwLock::new(None),
            allow_system_backends: false,
            docker: None,
            dist_dir: PathBuf::from("dist"),
        })
    }

    #[test]
    fn auth_allows_all_when_unset() {
        let headers = HeaderMap::new();
        assert!(check_auth(&headers, &None));
    }

    #[test]
    fn auth_enforces_tailscale_header() {
        let allowed = Some("pradeep@btbytes.com".to_string());
        let mut headers = HeaderMap::new();
        assert!(!check_auth(&headers, &allowed));

        headers.insert("Tailscale-User-Login", "someone@other.com".parse().unwrap());
        assert!(!check_auth(&headers, &allowed));

        headers.insert(
            "Tailscale-User-Login",
            "pradeep@btbytes.com".parse().unwrap(),
        );
        assert!(check_auth(&headers, &allowed));
    }

    #[test]
    fn filter_units_supports_enabled_running_scope_and_search() {
        let units = vec![
            SystemdUnitInfo {
                name: "kaaged.service".into(),
                description: "kaage daemon".into(),
                load_state: "loaded".into(),
                active_state: "active".into(),
                sub_state: "running".into(),
                unit_file_state: "enabled".into(),
                enabled: true,
                running: true,
                nixos_managed: true,
                scope: UnitScope::System,
            },
            SystemdUnitInfo {
                name: "emacs.service".into(),
                description: "Emacs daemon".into(),
                load_state: "loaded".into(),
                active_state: "inactive".into(),
                sub_state: "dead".into(),
                unit_file_state: "enabled".into(),
                enabled: true,
                running: false,
                nixos_managed: true,
                scope: UnitScope::User,
            },
            SystemdUnitInfo {
                name: "cups.service".into(),
                description: "CUPS".into(),
                load_state: "loaded".into(),
                active_state: "inactive".into(),
                sub_state: "dead".into(),
                unit_file_state: "disabled".into(),
                enabled: false,
                running: false,
                nixos_managed: true,
                scope: UnitScope::System,
            },
        ];

        let enabled_only = filter_units(
            units.clone(),
            &UnitsQuery {
                enabled: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(enabled_only.len(), 2);

        let running_only = filter_units(
            units.clone(),
            &UnitsQuery {
                running: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(running_only.len(), 1);
        assert_eq!(running_only[0].name, "kaaged.service");

        let user_only = filter_units(
            units,
            &UnitsQuery {
                scope: Some("user".into()),
                ..Default::default()
            },
        );
        assert_eq!(user_only.len(), 1);
        assert_eq!(user_only[0].name, "emacs.service");
    }

    #[tokio::test]
    async fn health_and_nixos_endpoints_respect_auth() {
        let app = build_router(test_state(Some("pradeep@btbytes.com")));

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health")
                    .header("Tailscale-User-Login", "pradeep@btbytes.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["nixos"]["is_nixos"], true);
        assert_eq!(json["nixos"]["generation"], 42);

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/nixos")
                    .header("Tailscale-User-Login", "pradeep@btbytes.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let nixos: NixosInfo = serde_json::from_slice(&body).unwrap();
        assert!(nixos.is_nixos);
        assert_eq!(nixos.generation, Some(42));
    }

    #[tokio::test]
    async fn units_and_containers_return_empty_when_backends_absent() {
        let app = build_router(test_state(None));

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/units")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let units: Vec<SystemdUnitInfo> = serde_json::from_slice(&body).unwrap();
        assert!(units.is_empty());

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/units/kaaged/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/units/kaaged/bogus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/containers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let containers: Vec<ContainerInfo> = serde_json::from_slice(&body).unwrap();
        assert!(containers.is_empty());
    }
}
