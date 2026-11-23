use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
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
    ClientMsg, ContainerAction, ContainerInfo, HostMetrics, ServerMsg, SystemdUnitInfo, UnitAction,
    UnitScope,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use sysinfo::System;
use tokio::sync::mpsc;
use tower_http::services::ServeDir;
use zbus::zvariant::OwnedObjectPath;

struct AppState {
    allowed_user: Option<String>,
    system_dbus: Option<zbus::Connection>,
    user_dbus: Option<zbus::Connection>,
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
}

fn build_router(state: Arc<AppState>) -> Router {
    let dist_dir = state.dist_dir.clone();
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/v1/health", get(http_health))
        .route("/api/v1/metrics", get(http_metrics))
        .route("/api/v1/units", get(http_list_units))
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

    let system_dbus = match zbus::Connection::system().await {
        Ok(conn) => {
            tracing::info!("Connected to system D-Bus");
            Some(conn)
        }
        Err(err) => {
            tracing::warn!("System D-Bus unavailable ({err}); system unit inspection disabled");
            None
        }
    };

    let user_dbus = match zbus::Connection::session().await {
        Ok(conn) => {
            tracing::info!("Connected to user session D-Bus");
            Some(conn)
        }
        Err(err) => {
            tracing::debug!("User session D-Bus unavailable ({err}); user unit inspection disabled");
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
        system_dbus,
        user_dbus,
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

fn check_auth(headers: &HeaderMap, allowed: &Option<String>) -> bool {
    let Some(expected) = allowed else {
        return true;
    };
    headers
        .get("Tailscale-User-Login")
        .and_then(|v| v.to_str().ok())
        == Some(expected.as_str())
}

async fn http_health(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    Json(HealthResponse {
        status: "ok",
        hostname: System::host_name().unwrap_or_else(|| "kaage-host".into()),
        dbus_system: state.system_dbus.is_some(),
        dbus_user: state.user_dbus.is_some(),
        docker: state.docker.is_some(),
    })
    .into_response()
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

async fn http_list_units(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if !check_auth(&headers, &state.allowed_user) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    Json(list_all_systemd_units(&state).await).into_response()
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
            ClientMsg::ControlUnit {
                unit,
                scope,
                action,
            } => {
                let conn = match scope {
                    UnitScope::System => state.system_dbus.as_ref(),
                    UnitScope::User => state.user_dbus.as_ref(),
                };
                let res = match conn {
                    Some(c) => control_systemd_unit(c, &unit, action)
                        .await
                        .map_err(|e| e.to_string()),
                    None => Err(format!("{scope:?} D-Bus connection unavailable")),
                };
                let _ = out_tx.send(ServerMsg::ActionFeedback {
                    target: unit,
                    ok: res.is_ok(),
                    message: res.err().unwrap_or_else(|| "OK".into()),
                });
                let units = list_all_systemd_units(&state).await;
                let _ = out_tx.send(ServerMsg::UnitsSnapshot(units));
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
    if let Some(ref conn) = state.system_dbus {
        match list_systemd_units(conn, UnitScope::System).await {
            Ok(mut units) => all.append(&mut units),
            Err(err) => tracing::warn!("Failed to list system systemd units: {err}"),
        }
    }
    if let Some(ref conn) = state.user_dbus {
        match list_systemd_units(conn, UnitScope::User).await {
            Ok(mut units) => all.append(&mut units),
            Err(err) => tracing::warn!("Failed to list user systemd units: {err}"),
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

    let units: Vec<_> = raw
        .into_iter()
        .filter(|u| u.0.ends_with(".service"))
        .map(|u| SystemdUnitInfo {
            name: u.0,
            description: u.1,
            load_state: u.2,
            active_state: u.3,
            sub_state: u.4,
            scope,
        })
        .collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_state(allowed_user: Option<&str>) -> Arc<AppState> {
        Arc::new(AppState {
            allowed_user: allowed_user.map(str::to_string),
            system_dbus: None,
            user_dbus: None,
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

    #[tokio::test]
    async fn health_endpoint_respects_auth() {
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
