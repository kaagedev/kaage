use crate::inspectors::InspectorSpawnRequest;
use crate::theme::{
    BEOS_BEVEL_DARK, BEOS_BEVEL_LIGHT, BEOS_BORDER_BLACK, BEOS_DESKTOP_BLUE, BEOS_LED_GOLD,
    BEOS_LED_GREEN, BEOS_LED_RED, BEOS_STATUS_GREEN, BEOS_STATUS_RED, BEOS_TAB_YELLOW,
    BEOS_WELL_WHITE, be_button,
};
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};
use kaage_proto::{ClientMsg, DashboardSnapshot};

/// System Monitor panel: GPU usage & VRAM, CPU consumption & per-core bars, Memory/Swap & top processes.
pub fn render_system_monitor(
    ui: &mut Ui,
    dashboard: Option<&DashboardSnapshot>,
    cpu_history: &[f32],
    mem_history: &[f32],
    send_msg: &mut dyn FnMut(ClientMsg),
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Host Compute: GPU · CPU · Memory")
                .strong()
                .color(Color32::BLACK),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if be_button(ui, "Refresh").clicked() {
                send_msg(ClientMsg::RefreshDashboard);
            }
        });
    });
    ui.separator();

    let Some(dash) = dashboard else {
        ui.label(
            RichText::new("Collecting host telemetry...")
                .italics()
                .color(Color32::from_rgb(70, 70, 70)),
        );
        return;
    };

    ScrollArea::vertical()
        .id_salt("sysmon_scroll")
        .show(ui, |ui| {
            // --- GPU SECTION ---
            section_banner(ui, "GPU");
            if dash.gpus.is_empty() {
                ui.label(
                    RichText::new("No GPU metrics detected.")
                        .italics()
                        .size(11.5)
                        .color(Color32::from_rgb(80, 80, 80)),
                );
            } else {
                for gpu in &dash.gpus {
                    be_card(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&gpu.name).strong().color(Color32::BLACK));
                            ui.label(
                                RichText::new(format!("[{}]", gpu.source))
                                    .monospace()
                                    .size(10.0)
                                    .color(Color32::from_rgb(70, 90, 120)),
                            );
                        });

                        if let Some(util) = gpu.utilization_percent {
                            progress_row(
                                ui,
                                "GPU Util",
                                util / 100.0,
                                &format!("{util:.0}%"),
                                BEOS_STATUS_GREEN,
                            );
                        }

                        if gpu.vram_total_mb > 0 {
                            let frac = (gpu.vram_used_mb as f32 / gpu.vram_total_mb as f32).clamp(0.0, 1.0);
                            progress_row(
                                ui,
                                "VRAM",
                                frac,
                                &format!(
                                    "{:.1} / {:.1} GiB ({:.0}%)",
                                    gpu.vram_used_mb as f32 / 1024.0,
                                    gpu.vram_total_mb as f32 / 1024.0,
                                    frac * 100.0
                                ),
                                BEOS_DESKTOP_BLUE,
                            );
                        }

                        let mut extras = Vec::new();
                        if let Some(t) = gpu.temperature_c {
                            extras.push(format!("Temp: {t:.0}°C"));
                        }
                        if let Some(p) = gpu.power_draw_w {
                            if let Some(lim) = gpu.power_limit_w {
                                extras.push(format!("Power: {p:.0}W / {lim:.0}W"));
                            } else {
                                extras.push(format!("Power: {p:.0}W"));
                            }
                        }
                        if !extras.is_empty() {
                            ui.label(
                                RichText::new(extras.join("   •   "))
                                    .monospace()
                                    .size(11.0)
                                    .color(Color32::from_rgb(55, 55, 55)),
                            );
                        }
                        if let Some(ref note) = gpu.note {
                            ui.label(
                                RichText::new(note)
                                    .size(10.5)
                                    .italics()
                                    .color(Color32::from_rgb(120, 80, 20)),
                            );
                        }
                    });
                    ui.add_space(4.0);
                }
            }

            // --- CPU SECTION ---
            ui.add_space(4.0);
            section_banner(ui, "CPU CONSUMPTION");
            be_card(ui, |ui| {
                ui.horizontal(|ui| {
                    let brand = if dash.cpu_brand.is_empty() {
                        format!("{} logical cores", dash.cpu_cores.len())
                    } else {
                        format!("{} ({} threads)", dash.cpu_brand, dash.cpu_cores.len())
                    };
                    ui.label(RichText::new(brand).strong().size(11.5).color(Color32::BLACK));
                });
                progress_row(
                    ui,
                    "Total CPU",
                    (dash.cpu_percent / 100.0).clamp(0.0, 1.0),
                    &format!(
                        "{:.1}%  ·  load {:.2} / {:.2} / {:.2}",
                        dash.cpu_percent, dash.load_avg[0], dash.load_avg[1], dash.load_avg[2]
                    ),
                    BEOS_STATUS_GREEN,
                );

                if cpu_history.len() > 1 {
                    draw_sparkline(ui, cpu_history, BEOS_STATUS_GREEN, "CPU History");
                }

                // Per-core compact mini bar grid
                if !dash.cpu_cores.is_empty() {
                    ui.add_space(3.0);
                    let cols = 8usize.min(dash.cpu_cores.len().max(1));
                    let avail = ui.available_width();
                    let cell_w = ((avail - (cols as f32 - 1.0) * 4.0) / cols as f32).max(34.0);
                    for chunk in dash.cpu_cores.chunks(cols) {
                        ui.horizontal(|ui| {
                            for &usage in chunk {
                                let frac = (usage / 100.0).clamp(0.0, 1.0);
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::new(cell_w, 12.0), Sense::hover());
                                let p = ui.painter();
                                p.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(228, 228, 228));
                                p.rect_filled(
                                    Rect::from_min_size(
                                        rect.min,
                                        Vec2::new(rect.width() * frac, rect.height()),
                                    ),
                                    CornerRadius::ZERO,
                                    if usage > 80.0 {
                                        BEOS_STATUS_RED
                                    } else {
                                        BEOS_STATUS_GREEN
                                    },
                                );
                                p.rect_stroke(
                                    rect,
                                    CornerRadius::ZERO,
                                    Stroke::new(1.0, BEOS_BORDER_BLACK),
                                    egui::StrokeKind::Inside,
                                );
                                p.text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    format!("{usage:.0}%"),
                                    egui::FontId::monospace(9.0),
                                    Color32::BLACK,
                                );
                            }
                        });
                    }
                }
            });

            // --- MEMORY SECTION ---
            ui.add_space(6.0);
            section_banner(ui, "MEMORY CONSUMPTION");
            be_card(ui, |ui| {
                let mem_frac = if dash.mem_total_mb > 0 {
                    (dash.mem_used_mb as f32 / dash.mem_total_mb as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                progress_row(
                    ui,
                    "RAM",
                    mem_frac,
                    &format!(
                        "{:.1} / {:.1} GiB ({:.0}%)",
                        dash.mem_used_mb as f32 / 1024.0,
                        dash.mem_total_mb as f32 / 1024.0,
                        mem_frac * 100.0
                    ),
                    BEOS_TAB_YELLOW,
                );
                if dash.swap_total_mb > 0 {
                    let swap_frac =
                        (dash.swap_used_mb as f32 / dash.swap_total_mb as f32).clamp(0.0, 1.0);
                    progress_row(
                        ui,
                        "Swap",
                        swap_frac,
                        &format!(
                            "{:.1} / {:.1} GiB ({:.0}%)",
                            dash.swap_used_mb as f32 / 1024.0,
                            dash.swap_total_mb as f32 / 1024.0,
                            swap_frac * 100.0
                        ),
                        Color32::from_rgb(130, 130, 180),
                    );
                }
                if mem_history.len() > 1 {
                    draw_sparkline(ui, mem_history, Color32::from_rgb(200, 145, 0), "RAM History");
                }

                if !dash.top_processes.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Top Memory Processes")
                            .strong()
                            .size(11.0)
                            .color(Color32::from_rgb(50, 50, 50)),
                    );
                    for p in &dash.top_processes {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:>6}", p.pid))
                                    .monospace()
                                    .size(10.5)
                                    .color(Color32::from_rgb(85, 85, 85)),
                            );
                            ui.label(RichText::new(&p.name).size(11.0).color(Color32::BLACK));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("{} MiB  |  CPU {:.1}%", p.mem_mb, p.cpu_percent))
                                        .monospace()
                                        .size(10.5)
                                        .color(Color32::from_rgb(45, 45, 45)),
                                );
                            });
                        });
                    }
                }
            });
        });
}

/// AI Jobs panel: ComfyUI running & pending queue + Unsloth Studio jobs & workers.
pub fn render_ai_jobs(
    ui: &mut Ui,
    dashboard: Option<&DashboardSnapshot>,
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<InspectorSpawnRequest> {
    let mut spawn_req = None;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("AI Workloads: ComfyUI · Unsloth Studio")
                .strong()
                .color(Color32::BLACK),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if be_button(ui, "Refresh").clicked() {
                send_msg(ClientMsg::RefreshDashboard);
            }
        });
    });
    ui.separator();

    let Some(dash) = dashboard else {
        ui.label(
            RichText::new("Querying ComfyUI and Unsloth Studio...")
                .italics()
                .color(Color32::from_rgb(70, 70, 70)),
        );
        return None;
    };

    ScrollArea::vertical()
        .id_salt("ai_jobs_scroll")
        .show(ui, |ui| {
            // --- COMFYUI ---
            section_banner(ui, "COMFYUI JOBS");
            be_card(ui, |ui| {
                let c = &dash.comfyui;
                ui.horizontal(|ui| {
                    status_led(ui, if c.reachable { BEOS_LED_GREEN } else { BEOS_LED_RED });
                    let ver = if c.version.is_empty() {
                        String::new()
                    } else {
                        format!(" v{}", c.version)
                    };
                    ui.label(
                        RichText::new(format!("ComfyUI{ver}"))
                            .strong()
                            .color(Color32::BLACK),
                    );
                    ui.label(
                        RichText::new(&c.url)
                            .monospace()
                            .size(10.5)
                            .color(Color32::from_rgb(75, 75, 75)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if be_button(ui, "Logs").clicked() {
                            spawn_req = Some(InspectorSpawnRequest::OpenCommandTab {
                                title: "logs: comfyui.service".into(),
                                cmd: vec![
                                    "journalctl".into(),
                                    "--user".into(),
                                    "-u".into(),
                                    "comfyui.service".into(),
                                    "-n".into(),
                                    "100".into(),
                                    "-f".into(),
                                ],
                            });
                        }
                    });
                });

                if let Some(ref err) = c.error {
                    ui.label(
                        RichText::new(format!("Offline: {err}"))
                            .size(11.0)
                            .color(BEOS_STATUS_RED),
                    );
                } else {
                    ui.label(
                        RichText::new(format!(
                            "Running: {}   •   Queued/Pending: {}",
                            c.running_jobs.len(),
                            c.pending_jobs.len()
                        ))
                        .size(11.5)
                        .color(Color32::BLACK),
                    );
                    if c.running_jobs.is_empty() && c.pending_jobs.is_empty() {
                        ui.label(
                            RichText::new("Queue is idle — no active or pending ComfyUI jobs.")
                                .italics()
                                .size(11.0)
                                .color(Color32::from_rgb(80, 80, 80)),
                        );
                    } else {
                        for job in c.running_jobs.iter().chain(c.pending_jobs.iter()) {
                            ui.horizontal(|ui| {
                                let col = if job.state == "running" {
                                    BEOS_LED_GREEN
                                } else {
                                    BEOS_LED_GOLD
                                };
                                status_led(ui, col);
                                ui.label(RichText::new(&job.title).strong().size(11.5).color(Color32::BLACK));
                                ui.label(
                                    RichText::new(format!("[{}] {}", job.state, job.detail))
                                        .monospace()
                                        .size(10.5)
                                        .color(Color32::from_rgb(65, 65, 65)),
                                );
                            });
                        }
                    }
                }
            });

            ui.add_space(6.0);

            // --- UNSLOTH ---
            section_banner(ui, "UNSLOTH STUDIO JOBS & WORKERS");
            be_card(ui, |ui| {
                let u = &dash.unsloth;
                ui.horizontal(|ui| {
                    status_led(ui, if u.reachable { BEOS_LED_GREEN } else { BEOS_LED_RED });
                    ui.label(
                        RichText::new(&u.service_name)
                            .strong()
                            .color(Color32::BLACK),
                    );
                    ui.label(
                        RichText::new(&u.url)
                            .monospace()
                            .size(10.5)
                            .color(Color32::from_rgb(75, 75, 75)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if be_button(ui, "Logs").clicked() {
                            spawn_req = Some(InspectorSpawnRequest::OpenCommandTab {
                                title: "logs: unsloth-studio.service".into(),
                                cmd: vec![
                                    "journalctl".into(),
                                    "--user".into(),
                                    "-u".into(),
                                    "unsloth-studio.service".into(),
                                    "-n".into(),
                                    "100".into(),
                                    "-f".into(),
                                ],
                            });
                        }
                    });
                });

                if let Some(ref err) = u.error {
                    ui.label(
                        RichText::new(format!("API unreachable: {err}"))
                            .size(11.0)
                            .color(BEOS_STATUS_RED),
                    );
                }
                if u.jobs.is_empty() {
                    ui.label(
                        RichText::new("No active Unsloth jobs or worker processes detected.")
                            .italics()
                            .size(11.0)
                            .color(Color32::from_rgb(80, 80, 80)),
                    );
                } else {
                    for job in &u.jobs {
                        ui.horizontal(|ui| {
                            let col = if job.state == "active" {
                                BEOS_LED_GREEN
                            } else {
                                BEOS_LED_GOLD
                            };
                            status_led(ui, col);
                            ui.label(
                                RichText::new(&job.id)
                                    .monospace()
                                    .size(10.5)
                                    .color(Color32::from_rgb(80, 80, 80)),
                            );
                            ui.label(RichText::new(&job.title).strong().size(11.5).color(Color32::BLACK));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(&job.detail)
                                        .monospace()
                                        .size(10.5)
                                        .color(Color32::from_rgb(55, 55, 55)),
                                );
                            });
                        });
                    }
                }
            });
        });

    spawn_req
}

/// HTTP Ports & Listening Services panel.
pub fn render_http_ports(
    ui: &mut Ui,
    filter: &mut String,
    dashboard: Option<&DashboardSnapshot>,
    send_msg: &mut dyn FnMut(ClientMsg),
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Filter:").color(Color32::BLACK));
        ui.add(egui::TextEdit::singleline(filter).desired_width(160.0));
        if be_button(ui, "Refresh").clicked() {
            send_msg(ClientMsg::RefreshDashboard);
        }
    });
    ui.separator();

    let Some(dash) = dashboard else {
        ui.label(
            RichText::new("Scanning listening ports...")
                .italics()
                .color(Color32::from_rgb(70, 70, 70)),
        );
        return;
    };

    let needle = filter.to_ascii_lowercase();
    ScrollArea::vertical()
        .id_salt("http_ports_scroll")
        .show(ui, |ui| {
            let mut shown = 0usize;
            for p in &dash.listening_ports {
                let port_str = p.port.to_string();
                if !needle.is_empty()
                    && !port_str.contains(&needle)
                    && !p.label.to_ascii_lowercase().contains(&needle)
                    && !p.process.to_ascii_lowercase().contains(&needle)
                    && !p
                        .tailscale_url
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&needle)
                {
                    continue;
                }
                shown += 1;

                be_card(ui, |ui| {
                    ui.horizontal(|ui| {
                        let led = if p.tailscale_url.is_some() {
                            BEOS_LED_GREEN
                        } else {
                            BEOS_LED_GOLD
                        };
                        status_led(ui, led);
                        ui.label(
                            RichText::new(format!(":{}", p.port))
                                .monospace()
                                .strong()
                                .size(12.5)
                                .color(BEOS_DESKTOP_BLUE),
                        );
                        ui.label(RichText::new(&p.label).strong().color(Color32::BLACK));
                        if !p.process.is_empty() {
                            let pid_part = p
                                .pid
                                .map(|pid| format!(" (pid {pid})"))
                                .unwrap_or_default();
                            ui.label(
                                RichText::new(format!("{}{pid_part}", p.process))
                                    .monospace()
                                    .size(10.5)
                                    .color(Color32::from_rgb(75, 75, 75)),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(&p.bind_addr)
                                    .monospace()
                                    .size(10.5)
                                    .color(Color32::from_rgb(80, 80, 80)),
                            );
                        });
                    });
                    if let Some(ref url) = p.tailscale_url {
                        ui.label(
                            RichText::new(format!("Tailscale HTTPS: {url}"))
                                .monospace()
                                .size(10.5)
                                .color(BEOS_STATUS_GREEN),
                        );
                    }
                });
                ui.add_space(3.0);
            }
            if shown == 0 {
                ui.label(
                    RichText::new("No matching listening ports.")
                        .italics()
                        .color(Color32::from_rgb(80, 80, 80)),
                );
            }
        });
}

/// Tailscale Information panel: self node, MagicDNS, IPs, Tailscale Serve proxies, and Tailnet peers.
pub fn render_tailscale_panel(
    ui: &mut Ui,
    dashboard: Option<&DashboardSnapshot>,
    send_msg: &mut dyn FnMut(ClientMsg),
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Tailscale Mesh & Serve Status")
                .strong()
                .color(Color32::BLACK),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if be_button(ui, "Refresh").clicked() {
                send_msg(ClientMsg::RefreshDashboard);
            }
        });
    });
    ui.separator();

    let Some(dash) = dashboard else {
        ui.label(
            RichText::new("Querying Tailscale daemon...")
                .italics()
                .color(Color32::from_rgb(70, 70, 70)),
        );
        return;
    };

    let ts = &dash.tailscale;
    if !ts.available {
        ui.label(
            RichText::new(format!(
                "Tailscale unavailable: {}",
                ts.error.as_deref().unwrap_or("not running")
            ))
            .color(BEOS_STATUS_RED),
        );
        return;
    }

    ScrollArea::vertical()
        .id_salt("tailscale_panel_scroll")
        .show(ui, |ui| {
            // Self node card
            section_banner(ui, "THIS NODE");
            be_card(ui, |ui| {
                ui.horizontal(|ui| {
                    let ok = ts.backend_state == "Running";
                    status_led(ui, if ok { BEOS_LED_GREEN } else { BEOS_LED_RED });
                    ui.label(
                        RichText::new(format!("{}  ({})", ts.hostname, ts.backend_state))
                            .strong()
                            .color(Color32::BLACK),
                    );
                    ui.label(
                        RichText::new(format!("v{}", ts.version))
                            .monospace()
                            .size(10.5)
                            .color(Color32::from_rgb(75, 75, 75)),
                    );
                });
                if !ts.dns_name.is_empty() {
                    ui.label(
                        RichText::new(format!("MagicDNS: {}", ts.dns_name))
                            .monospace()
                            .size(11.0)
                            .color(BEOS_DESKTOP_BLUE),
                    );
                }
                if !ts.tailscale_ips.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "IPs: {}   •   DERP: {}",
                            ts.tailscale_ips.join(", "),
                            if ts.relay.is_empty() { "direct" } else { &ts.relay }
                        ))
                        .monospace()
                        .size(10.5)
                        .color(Color32::from_rgb(55, 55, 55)),
                    );
                }
            });

            // Tailscale Serve endpoints
            ui.add_space(6.0);
            section_banner(ui, &format!("TAILSCALE SERVE ({})", ts.serves.len()));
            be_card(ui, |ui| {
                if ts.serves.is_empty() {
                    ui.label(
                        RichText::new("No active tailscale serve routes.")
                            .italics()
                            .size(11.0)
                            .color(Color32::from_rgb(80, 80, 80)),
                    );
                } else {
                    for s in &ts.serves {
                        ui.horizontal(|ui| {
                            status_led(ui, BEOS_LED_GREEN);
                            ui.label(
                                RichText::new(&s.https_url)
                                    .monospace()
                                    .size(11.0)
                                    .color(BEOS_DESKTOP_BLUE),
                            );
                            ui.label(RichText::new("→").color(Color32::from_rgb(90, 90, 90)));
                            ui.label(
                                RichText::new(&s.target)
                                    .monospace()
                                    .size(10.5)
                                    .color(Color32::from_rgb(55, 55, 55)),
                            );
                        });
                    }
                }
            });

            // Tailnet Peers
            let online_peers = ts.peers.iter().filter(|p| p.online).count();
            ui.add_space(6.0);
            section_banner(
                ui,
                &format!("TAILNET PEERS ({online_peers} online / {} total)", ts.peers.len()),
            );
            be_card(ui, |ui| {
                if ts.peers.is_empty() {
                    ui.label(
                        RichText::new("No peers reported.")
                            .italics()
                            .size(11.0)
                            .color(Color32::from_rgb(80, 80, 80)),
                    );
                } else {
                    for peer in &ts.peers {
                        ui.horizontal(|ui| {
                            let led = if peer.online {
                                BEOS_LED_GREEN
                            } else {
                                Color32::from_rgb(160, 160, 160)
                            };
                            status_led(ui, led);
                            ui.label(
                                RichText::new(&peer.hostname)
                                    .strong()
                                    .size(11.5)
                                    .color(Color32::BLACK),
                            );
                            ui.label(
                                RichText::new(format!("[{}]", peer.os))
                                    .monospace()
                                    .size(10.0)
                                    .color(Color32::from_rgb(80, 80, 80)),
                            );
                            ui.label(
                                RichText::new(&peer.tailscale_ip)
                                    .monospace()
                                    .size(10.5)
                                    .color(Color32::from_rgb(55, 55, 55)),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let relay = if peer.relay.is_empty() {
                                    "direct"
                                } else {
                                    &peer.relay
                                };
                                ui.label(
                                    RichText::new(format!(
                                        "{relay} · ↓{} ↑{}",
                                        fmt_bytes(peer.rx_bytes),
                                        fmt_bytes(peer.tx_bytes)
                                    ))
                                    .monospace()
                                    .size(10.0)
                                    .color(Color32::from_rgb(75, 75, 75)),
                                );
                            });
                        });
                    }
                }
            });
        });
}

fn section_banner(ui: &mut Ui, title: &str) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 18.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(206, 212, 220));
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, BEOS_BEVEL_DARK),
    );
    p.text(
        Pos2::new(rect.min.x + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(10.5),
        Color32::from_rgb(35, 45, 60),
    );
}

fn be_card(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(BEOS_WELL_WHITE)
        .stroke(Stroke::new(1.0, BEOS_BEVEL_DARK))
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui);
        });
}

fn status_led(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(9.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, color);
    p.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, BEOS_BORDER_BLACK),
        egui::StrokeKind::Inside,
    );
}

fn progress_row(ui: &mut Ui, label: &str, frac: f32, value_text: &str, fill: Color32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{label:<9}"))
                .monospace()
                .size(11.0)
                .color(Color32::BLACK),
        );
        let bar_w = 150.0_f32.min((ui.available_width() - 160.0).max(80.0));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 13.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(230, 230, 230));
        p.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac.clamp(0.0, 1.0), rect.height())),
            CornerRadius::ZERO,
            fill,
        );
        p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, BEOS_BEVEL_DARK));
        p.line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(1.0, BEOS_BEVEL_DARK));
        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, BEOS_BEVEL_LIGHT));
        p.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, BEOS_BEVEL_LIGHT));
        ui.label(
            RichText::new(value_text)
                .monospace()
                .size(11.0)
                .color(Color32::from_rgb(35, 35, 35)),
        );
    });
}

fn draw_sparkline(ui: &mut Ui, samples: &[f32], color: Color32, title: &str) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(245, 247, 250));
    p.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, BEOS_BEVEL_DARK),
        egui::StrokeKind::Inside,
    );
    let n = samples.len();
    if n >= 2 {
        let step = rect.width() / (n - 1) as f32;
        let pts: Vec<Pos2> = samples
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let y = rect.max.y - 2.0 - (v.clamp(0.0, 100.0) / 100.0) * (rect.height() - 4.0);
                Pos2::new(rect.min.x + i as f32 * step, y)
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
    }
    p.text(
        Pos2::new(rect.min.x + 4.0, rect.min.y + 3.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(9.5),
        Color32::from_rgb(100, 100, 100),
    );
}

fn fmt_bytes(b: u64) -> String {
    if b >= 1024 * 1024 * 1024 {
        format!("{:.1}G", b as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024 * 1024 {
        format!("{:.1}M", b as f64 / (1024.0 * 1024.0))
    } else if b >= 1024 {
        format!("{:.0}K", b as f64 / 1024.0)
    } else {
        format!("{b}B")
    }
}
