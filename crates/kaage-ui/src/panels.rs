use crate::inspectors::InspectorSpawnRequest;
use crate::theme::{
    BEOS_BORDER_BLACK, BEOS_LED_GOLD, BEOS_LED_GREEN, BEOS_LED_RED, be_button, draw_beos_bevel,
    palette,
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
) -> Option<InspectorSpawnRequest> {
    let pal = palette();
    let spawn_req = None;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Host Compute: GPU · CPU · Memory")
                .strong()
                .color(pal.text),
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
                .color(pal.text_muted),
        );
        return spawn_req;
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
                        .color(pal.text_muted),
                );
            } else {
                for gpu in &dash.gpus {
                    be_card(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&gpu.name).strong().color(pal.text));
                            ui.label(
                                RichText::new(format!("[{}]", gpu.source))
                                    .monospace()
                                    .size(10.0)
                                    .color(pal.status_sky),
                            );
                        });

                        if let Some(util) = gpu.utilization_percent {
                            progress_row(
                                ui,
                                "GPU Util",
                                util / 100.0,
                                &format!("{util:.0}%"),
                                pal.status_green,
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
                                pal.status_sky,
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
                                    .color(pal.text_muted),
                            );
                        }
                        if let Some(ref note) = gpu.note {
                            ui.label(
                                RichText::new(note)
                                    .size(10.5)
                                    .italics()
                                    .color(pal.status_amber),
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
                    ui.label(RichText::new(brand).strong().size(11.5).color(pal.text));
                });
                progress_row(
                    ui,
                    "Total CPU",
                    (dash.cpu_percent / 100.0).clamp(0.0, 1.0),
                    &format!(
                        "{:.1}%  ·  load {:.2} / {:.2} / {:.2}",
                        dash.cpu_percent, dash.load_avg[0], dash.load_avg[1], dash.load_avg[2]
                    ),
                    pal.status_green,
                );

                if cpu_history.len() > 1 {
                    draw_sparkline(ui, cpu_history, pal.status_green, "CPU History");
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
                                p.rect_filled(rect, CornerRadius::ZERO, pal.surface_alt);
                                p.rect_filled(
                                    Rect::from_min_size(
                                        rect.min,
                                        Vec2::new(rect.width() * frac, rect.height()),
                                    ),
                                    CornerRadius::ZERO,
                                    if usage > 80.0 {
                                        pal.status_red
                                    } else {
                                        pal.status_green
                                    },
                                );
                                p.rect_stroke(
                                    rect,
                                    CornerRadius::ZERO,
                                    Stroke::new(1.0, pal.border),
                                    egui::StrokeKind::Inside,
                                );
                                p.text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    format!("{usage:.0}%"),
                                    egui::FontId::monospace(9.0),
                                    pal.text,
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
                    pal.status_amber,
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
                    draw_sparkline(ui, mem_history, pal.status_amber, "RAM History");
                }

                if !dash.top_processes.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Top Memory Processes")
                            .strong()
                            .size(11.0)
                            .color(pal.accent),
                    );
                    for p in &dash.top_processes {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:>6}", p.pid))
                                    .monospace()
                                    .size(10.5)
                                    .color(pal.text_muted),
                            );
                            ui.label(RichText::new(&p.name).size(11.0).color(pal.text));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("{} MiB  |  CPU {:.1}%", p.mem_mb, p.cpu_percent))
                                        .monospace()
                                        .size(10.5)
                                        .color(pal.text),
                                );
                            });
                        });
                    }
                }
            });
        });

    spawn_req
}

/// AI Jobs panel: ComfyUI running & pending queue + Unsloth Studio jobs & workers.
pub fn render_ai_jobs(
    ui: &mut Ui,
    dashboard: Option<&DashboardSnapshot>,
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<InspectorSpawnRequest> {
    let pal = palette();
    let mut spawn_req = None;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("AI Workloads: ComfyUI · Unsloth Studio")
                .strong()
                .color(pal.text),
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
                .color(pal.text_muted),
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
                            .color(pal.text),
                    );
                    ui.label(
                        RichText::new(&c.url)
                            .monospace()
                            .size(10.5)
                            .color(pal.text_muted),
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
                            .color(pal.status_red),
                    );
                } else {
                    ui.label(
                        RichText::new(format!(
                            "Running: {}   •   Queued/Pending: {}",
                            c.running_jobs.len(),
                            c.pending_jobs.len()
                        ))
                        .size(11.5)
                        .color(pal.text),
                    );
                    if c.running_jobs.is_empty() && c.pending_jobs.is_empty() {
                        ui.label(
                            RichText::new("Queue is idle — no active or pending ComfyUI jobs.")
                                .italics()
                                .size(11.0)
                                .color(pal.text_muted),
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
                                ui.label(RichText::new(&job.title).strong().size(11.5).color(pal.text));
                                ui.label(
                                    RichText::new(format!("[{}] {}", job.state, job.detail))
                                        .monospace()
                                        .size(10.5)
                                        .color(pal.text_muted),
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
                            .color(pal.text),
                    );
                    ui.label(
                        RichText::new(&u.url)
                            .monospace()
                            .size(10.5)
                            .color(pal.text_muted),
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
                            .color(pal.status_red),
                    );
                }
                if u.jobs.is_empty() {
                    ui.label(
                        RichText::new("No active Unsloth jobs or worker processes detected.")
                            .italics()
                            .size(11.0)
                            .color(pal.text_muted),
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
                                    .color(pal.text_muted),
                            );
                            ui.label(RichText::new(&job.title).strong().size(11.5).color(pal.text));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(&job.detail)
                                        .monospace()
                                        .size(10.5)
                                        .color(pal.text_muted),
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
    let pal = palette();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Filter:").color(pal.text));
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
                .color(pal.text_muted),
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
                                .color(pal.status_sky),
                        );
                        ui.label(RichText::new(&p.label).strong().color(pal.text));
                        if !p.process.is_empty() {
                            let pid_part = p
                                .pid
                                .map(|pid| format!(" (pid {pid})"))
                                .unwrap_or_default();
                            ui.label(
                                RichText::new(format!("{}{pid_part}", p.process))
                                    .monospace()
                                    .size(10.5)
                                    .color(pal.text_muted),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(ref url) = p.tailscale_url {
                                let https_port = extract_https_port(url);
                                if be_button(ui, "Unregister").clicked() {
                                    send_msg(ClientMsg::UnregisterTailscaleServe { https_port });
                                }
                            } else if be_button(ui, "Expose TS")
                                .on_hover_text(format!(
                                    "Register local :{} on Tailscale HTTPS :{}",
                                    p.port, p.port
                                ))
                                .clicked()
                            {
                                send_msg(ClientMsg::RegisterTailscaleServe {
                                    https_port: p.port,
                                    local_port: p.port,
                                });
                            }
                            ui.label(
                                RichText::new(&p.bind_addr)
                                    .monospace()
                                    .size(10.5)
                                    .color(pal.text_muted),
                            );
                        });
                    });
                    if let Some(ref url) = p.tailscale_url {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new("Tailscale HTTPS:")
                                    .monospace()
                                    .size(10.5)
                                    .color(pal.text_muted),
                            );
                            ui.hyperlink_to(
                                RichText::new(url)
                                    .monospace()
                                    .strong()
                                    .size(10.5)
                                    .color(pal.status_green),
                                url,
                            );
                        });
                    }
                });
                ui.add_space(3.0);
            }
            if shown == 0 {
                ui.label(
                    RichText::new("No matching listening ports.")
                        .italics()
                        .color(pal.text_muted),
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
    let pal = palette();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Tailscale Mesh & Serve Status")
                .strong()
                .color(pal.text),
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
                .color(pal.text_muted),
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
            .color(pal.status_red),
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
                            .color(pal.text),
                    );
                    ui.label(
                        RichText::new(format!("v{}", ts.version))
                            .monospace()
                            .size(10.5)
                            .color(pal.text_muted),
                    );
                });
                if !ts.dns_name.is_empty() {
                    let node_url = format!("https://{}", ts.dns_name);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("MagicDNS:")
                                .monospace()
                                .size(11.0)
                                .color(pal.text_muted),
                        );
                        ui.hyperlink_to(
                            RichText::new(&ts.dns_name)
                                .monospace()
                                .strong()
                                .size(11.0)
                                .color(pal.status_sky),
                            &node_url,
                        );
                    });
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
                        .color(pal.text_muted),
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
                            .color(pal.text_muted),
                    );
                } else {
                    for s in &ts.serves {
                        ui.horizontal(|ui| {
                            status_led(ui, BEOS_LED_GREEN);
                            ui.hyperlink_to(
                                RichText::new(&s.https_url)
                                    .monospace()
                                    .strong()
                                    .size(11.0)
                                    .color(pal.status_sky),
                                &s.https_url,
                            );
                            ui.label(RichText::new("→").color(pal.text_muted));
                            ui.label(
                                RichText::new(&s.target)
                                    .monospace()
                                    .size(10.5)
                                    .color(pal.text),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let https_port = extract_https_port(&s.https_url);
                                    if be_button(ui, "Unregister").clicked() {
                                        send_msg(ClientMsg::UnregisterTailscaleServe {
                                            https_port,
                                        });
                                    }
                                },
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
                            .color(pal.text_muted),
                    );
                } else {
                    for peer in &ts.peers {
                        ui.horizontal(|ui| {
                            let led = if peer.online {
                                BEOS_LED_GREEN
                            } else {
                                Color32::from_rgb(130, 138, 150)
                            };
                            status_led(ui, led);
                            if !peer.dns_name.is_empty() {
                                let peer_url = format!("https://{}", peer.dns_name);
                                ui.hyperlink_to(
                                    RichText::new(&peer.hostname)
                                        .strong()
                                        .size(11.5)
                                        .color(pal.status_sky),
                                    &peer_url,
                                )
                                .on_hover_text(&peer_url);
                            } else {
                                ui.label(
                                    RichText::new(&peer.hostname)
                                        .strong()
                                        .size(11.5)
                                        .color(pal.text),
                                );
                            }
                            ui.label(
                                RichText::new(format!("[{}]", peer.os))
                                    .monospace()
                                    .size(10.0)
                                    .color(pal.text_muted),
                            );
                            ui.label(
                                RichText::new(&peer.tailscale_ip)
                                    .monospace()
                                    .size(10.5)
                                    .color(pal.text),
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
                                    .color(pal.text_muted),
                                );
                            });
                        });
                    }
                }
            });
        });
}

/// Tailscale Services screen: all Tailscale Serve / Funnel services exposed from this node,
/// enriched with backing process health, clickable HTTPS links, copy buttons, and unexposed local ports.
pub fn render_tailscale_services(
    ui: &mut Ui,
    filter: &mut String,
    local_port_input: &mut String,
    https_port_input: &mut String,
    dashboard: Option<&DashboardSnapshot>,
    send_msg: &mut dyn FnMut(ClientMsg),
) {
    let pal = palette();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Tailscale Exposed Services")
                .strong()
                .color(pal.text),
        );
        ui.separator();
        ui.label(RichText::new("Filter:").color(pal.text_muted));
        ui.add(egui::TextEdit::singleline(filter).desired_width(150.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if be_button(ui, "Refresh").clicked() {
                send_msg(ClientMsg::RefreshDashboard);
            }
        });
    });
    ui.separator();

    let Some(dash) = dashboard else {
        ui.label(
            RichText::new("Querying tailscale serve status...")
                .italics()
                .color(pal.text_muted),
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
            .color(pal.status_red),
        );
        return;
    }

    let node_name = if !ts.dns_name.is_empty() {
        ts.dns_name.as_str()
    } else {
        ts.hostname.as_str()
    };

    let online_count = ts
        .serves
        .iter()
        .filter(|s| {
            extract_target_port(&s.target)
                .map(|port| dash.listening_ports.iter().any(|lp| lp.port == port))
                .unwrap_or(false)
        })
        .count();

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Node:").size(11.5).color(pal.text_muted));
        if !ts.dns_name.is_empty() {
            let node_url = format!("https://{}", ts.dns_name);
            ui.hyperlink_to(
                RichText::new(node_name)
                    .monospace()
                    .strong()
                    .size(11.5)
                    .color(pal.status_sky),
                &node_url,
            );
        } else {
            ui.label(
                RichText::new(node_name)
                    .monospace()
                    .strong()
                    .size(11.5)
                    .color(pal.status_sky),
            );
        }
        ui.label(
            RichText::new(format!(
                "• {} exposed • {} backing ports online",
                ts.serves.len(),
                online_count
            ))
            .size(11.0)
            .color(pal.text_muted),
        );
        if !ts.tailscale_ips.is_empty() {
            ui.label(
                RichText::new(format!("• {}", ts.tailscale_ips.join(", ")))
                    .monospace()
                    .size(10.5)
                    .color(pal.text_muted),
            );
        }
    });
    ui.add_space(4.0);

    // Inline Register Custom Local Service Bar
    be_card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Register Local Service:")
                    .strong()
                    .size(11.0)
                    .color(pal.accent),
            );
            ui.label(RichText::new("Local port").size(11.0).color(pal.text_muted));
            ui.add(
                egui::TextEdit::singleline(local_port_input)
                    .hint_text("8000")
                    .desired_width(62.0),
            );
            ui.label(RichText::new("→ HTTPS port").size(11.0).color(pal.text_muted));
            ui.add(
                egui::TextEdit::singleline(https_port_input)
                    .hint_text("same")
                    .desired_width(62.0),
            );
            if be_button(ui, "+ Register").clicked()
                && let Ok(local_port) = local_port_input.trim().parse::<u16>()
                && local_port > 0
            {
                let https_port = https_port_input
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|&p| p > 0)
                    .unwrap_or(local_port);
                send_msg(ClientMsg::RegisterTailscaleServe {
                    https_port,
                    local_port,
                });
                local_port_input.clear();
                https_port_input.clear();
            }
        });
    });
    ui.add_space(4.0);

    let needle = filter.to_ascii_lowercase();
    ScrollArea::vertical()
        .id_salt("tailscale_services_scroll")
        .show(ui, |ui| {
            section_banner(ui, &format!("EXPOSED SERVICES ({})", ts.serves.len()));

            if ts.serves.is_empty() {
                be_card(ui, |ui| {
                    ui.label(
                        RichText::new("No Tailscale Serve routes are currently exposed from this node.")
                            .italics()
                            .color(pal.text_muted),
                    );
                    ui.label(
                        RichText::new("Expose a local service with: tailscale serve --bg --https=<port> http://127.0.0.1:<port>")
                            .monospace()
                            .size(10.5)
                            .color(pal.accent),
                    );
                });
            } else {
                let mut shown = 0usize;
                for serve in &ts.serves {
                    let target_port = extract_target_port(&serve.target);
                    let backing = target_port.and_then(|port| {
                        dash.listening_ports.iter().find(|lp| lp.port == port)
                    });
                    let friendly = backing
                        .map(|lp| lp.label.as_str())
                        .unwrap_or("Exposed Service");
                    let proc_str = backing
                        .filter(|lp| !lp.process.is_empty())
                        .map(|lp| match lp.pid {
                            Some(pid) => format!("{} (pid {pid})", lp.process),
                            None => lp.process.clone(),
                        })
                        .unwrap_or_default();

                    if !needle.is_empty()
                        && !serve.https_url.to_ascii_lowercase().contains(&needle)
                        && !serve.target.to_ascii_lowercase().contains(&needle)
                        && !friendly.to_ascii_lowercase().contains(&needle)
                        && !proc_str.to_ascii_lowercase().contains(&needle)
                    {
                        continue;
                    }
                    shown += 1;

                    let is_up = backing.is_some();
                    be_card(ui, |ui| {
                        ui.horizontal(|ui| {
                            status_led(ui, if is_up { BEOS_LED_GREEN } else { BEOS_LED_RED });
                            ui.label(RichText::new(friendly).strong().color(pal.text));

                            let (scope_label, scope_col) = if serve.funnel {
                                ("[FUNNEL · PUBLIC]", pal.status_amber)
                            } else {
                                ("[TAILNET ONLY]", pal.status_sky)
                            };
                            ui.label(
                                RichText::new(scope_label)
                                    .monospace()
                                    .size(10.0)
                                    .color(scope_col),
                            );

                            ui.label(
                                RichText::new(if is_up { "ONLINE" } else { "TARGET DOWN" })
                                    .monospace()
                                    .size(10.0)
                                    .color(if is_up {
                                        pal.status_green
                                    } else {
                                        pal.status_red
                                    }),
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let https_port = extract_https_port(&serve.https_url);
                                    if be_button(ui, "Unregister")
                                        .on_hover_text(format!(
                                            "tailscale serve --https={https_port} off"
                                        ))
                                        .clicked()
                                    {
                                        send_msg(ClientMsg::UnregisterTailscaleServe {
                                            https_port,
                                        });
                                    }
                                    if be_button(ui, "Open ↗").clicked() {
                                        ui.ctx()
                                            .open_url(egui::OpenUrl::new_tab(&serve.https_url));
                                    }
                                    if be_button(ui, "Copy").clicked() {
                                        ui.ctx().copy_text(serve.https_url.clone());
                                    }
                                },
                            );
                        });

                        ui.horizontal_wrapped(|ui| {
                            ui.hyperlink_to(
                                RichText::new(&serve.https_url)
                                    .monospace()
                                    .strong()
                                    .size(11.5)
                                    .color(pal.accent),
                                &serve.https_url,
                            );
                            let mount = if serve.mount_path.is_empty() {
                                "/"
                            } else {
                                serve.mount_path.as_str()
                            };
                            ui.label(
                                RichText::new(format!(" {mount} → {}", serve.target))
                                    .monospace()
                                    .size(11.0)
                                    .color(pal.text_muted),
                            );
                        });

                        if !proc_str.is_empty() {
                            ui.label(
                                RichText::new(format!("Backing process: {proc_str}"))
                                    .monospace()
                                    .size(10.0)
                                    .color(pal.text_muted),
                            );
                        }
                    });
                    ui.add_space(4.0);
                }

                if shown == 0 {
                    ui.label(
                        RichText::new("No exposed Tailscale services match the current filter.")
                            .italics()
                            .color(pal.text_muted),
                    );
                }
            }

            // Unexposed local ports that could be exposed with `tailscale serve`
            let unexposed: Vec<_> = dash
                .listening_ports
                .iter()
                .filter(|lp| lp.tailscale_url.is_none())
                .filter(|lp| {
                    needle.is_empty()
                        || lp.port.to_string().contains(&needle)
                        || lp.label.to_ascii_lowercase().contains(&needle)
                        || lp.process.to_ascii_lowercase().contains(&needle)
                })
                .collect();

            if !unexposed.is_empty() {
                ui.add_space(8.0);
                section_banner(
                    ui,
                    &format!("LOCAL PORTS NOT EXPOSED VIA TAILSCALE ({})", unexposed.len()),
                );
                be_card(ui, |ui| {
                    for lp in unexposed {
                        let cmd = format!(
                            "tailscale serve --bg --https={} http://127.0.0.1:{}",
                            lp.port, lp.port
                        );
                        ui.horizontal(|ui| {
                            status_led(ui, BEOS_LED_GOLD);
                            ui.label(
                                RichText::new(format!(":{}", lp.port))
                                    .monospace()
                                    .strong()
                                    .color(pal.status_sky),
                            );
                            ui.label(RichText::new(&lp.label).color(pal.text));
                            if !lp.process.is_empty() {
                                ui.label(
                                    RichText::new(format!("({})", lp.process))
                                        .monospace()
                                        .size(10.0)
                                        .color(pal.text_muted),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if be_button(ui, "+ Register")
                                        .on_hover_text(&cmd)
                                        .clicked()
                                    {
                                        send_msg(ClientMsg::RegisterTailscaleServe {
                                            https_port: lp.port,
                                            local_port: lp.port,
                                        });
                                    }
                                    if be_button(ui, "Copy cmd")
                                        .on_hover_text(&cmd)
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(cmd.clone());
                                    }
                                },
                            );
                        });
                    }
                });
            }
        });
}

fn extract_target_port(target: &str) -> Option<u16> {
    target
        .rsplit(':')
        .next()
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse::<u16>().ok())
}

fn extract_https_port(https_url: &str) -> u16 {
    let without_scheme = https_url
        .strip_prefix("https://")
        .or_else(|| https_url.strip_prefix("http://"))
        .unwrap_or(https_url);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    authority
        .rsplit_once(':')
        .and_then(|(_, port_str)| port_str.parse::<u16>().ok())
        .unwrap_or(443)
}

fn section_banner(ui: &mut Ui, title: &str) {
    let pal = crate::theme::palette();
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 18.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, pal.section_bg);
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, pal.border),
    );
    p.text(
        Pos2::new(rect.min.x + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::monospace(10.0),
        pal.text_muted,
    );
}

fn be_card(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    let pal = crate::theme::palette();
    egui::Frame::NONE
        .fill(pal.well)
        .stroke(Stroke::new(1.0, pal.border))
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
    let pal = palette();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{label:<9}"))
                .monospace()
                .size(11.0)
                .color(pal.text),
        );
        let bar_w = 150.0_f32.min((ui.available_width() - 160.0).max(80.0));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 13.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, pal.surface_alt);
        p.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac.clamp(0.0, 1.0), rect.height())),
            CornerRadius::ZERO,
            fill,
        );
        draw_beos_bevel(p, rect, true);
        ui.label(
            RichText::new(value_text)
                .monospace()
                .size(11.0)
                .color(pal.text),
        );
    });
}

fn draw_sparkline(ui: &mut Ui, samples: &[f32], color: Color32, title: &str) {
    let pal = palette();
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, pal.surface);
    p.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0, pal.border),
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
        pal.text_muted,
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
