use crate::theme::{
    BEOS_LED_GOLD, BEOS_LED_GREEN, BEOS_LED_RED, BEOS_TAB_YELLOW, be_button, be_toggle_button,
    draw_beos_bevel,
};
use egui::{
    Color32, CornerRadius, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2,
    epaint::{Mesh, Vertex},
};
use kaage_proto::{
    ClientMsg, ProcessSignal, ProcessVital, VitalsBucket, VitalsQuery, VitalsSnapshot, VitalsTick,
    VitalsWindow as TimeWindow,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProcSort {
    Cpu,
    Ram,
    Disk,
    Vram,
}

pub enum VitalsSpawnRequest {
    OpenUnitLogs { unit: String },
}

pub struct VitalsWindow {
    pub window: TimeWindow,
    pub zoom_range: Option<(i64, i64)>,
    pub snapshot: Option<VitalsSnapshot>,
    pub show_pulse_cores: bool,
    pub group_by_unit: bool,
    pub proc_filter: String,
    pub proc_sort: ProcSort,
    /// Shared hover position [0.0..1.0] across all subsystem charts
    pub hover_frac: Option<f32>,
    /// Drag-to-zoom state: (start_frac, current_frac)
    pub drag_zoom: Option<(f32, f32)>,
    pub last_refresh_ts: i64,
}

impl VitalsWindow {
    pub fn new(send_msg: &mut dyn FnMut(ClientMsg)) -> Self {
        let win = Self {
            window: TimeWindow::Min5,
            zoom_range: None,
            snapshot: None,
            show_pulse_cores: true,
            group_by_unit: false,
            proc_filter: String::new(),
            proc_sort: ProcSort::Cpu,
            hover_frac: None,
            drag_zoom: None,
            last_refresh_ts: 0,
        };
        win.request_snapshot(send_msg);
        win
    }

    pub fn request_snapshot(&self, send_msg: &mut dyn FnMut(ClientMsg)) {
        let (start_ts, end_ts) = self
            .zoom_range
            .map(|(a, b)| (Some(a), Some(b)))
            .unwrap_or((None, None));
        send_msg(ClientMsg::QueryVitals(VitalsQuery {
            window: self.window,
            buckets: 120,
            start_ts,
            end_ts,
        }));
    }

    pub fn on_snapshot(&mut self, snap: VitalsSnapshot) {
        self.last_refresh_ts = snap.latest.ts;
        self.snapshot = Some(snap);
    }

    pub fn on_live_tick(&mut self, tick: VitalsTick, send_msg: &mut dyn FnMut(ClientMsg)) {
        if let Some(ref mut snap) = self.snapshot {
            snap.latest = tick.clone();
            // When not zoomed and viewing <= 15m, append live bucket smoothly
            if self.zoom_range.is_none()
                && matches!(
                    self.window,
                    TimeWindow::Min1 | TimeWindow::Min5 | TimeWindow::Min15
                )
            {
                let mem_pct = if tick.mem_total_mb > 0 {
                    (tick.mem_used_mb as f32 / tick.mem_total_mb as f32) * 100.0
                } else {
                    0.0
                };
                let vram_pct = if tick.gpu_vram_total_mb > 0 {
                    (tick.gpu_vram_used_mb as f32 / tick.gpu_vram_total_mb as f32) * 100.0
                } else {
                    0.0
                };
                snap.buckets.push(VitalsBucket {
                    ts_start: tick.ts,
                    ts_end: tick.ts,
                    cpu_avg: tick.cpu_percent,
                    cpu_max: tick.cpu_percent,
                    mem_pct_avg: mem_pct,
                    gpu_util_avg: tick.gpu_util_percent.unwrap_or(0.0),
                    gpu_vram_pct_avg: vram_pct,
                    disk_read_kbps: tick.disk_read_kbps,
                    disk_write_kbps: tick.disk_write_kbps,
                    net_rx_kbps: tick.net_rx_kbps,
                    net_tx_kbps: tick.net_tx_kbps,
                    ts_rx_kbps: tick.tailscale_rx_kbps,
                    ts_tx_kbps: tick.tailscale_tx_kbps,
                });
                let cutoff = tick.ts - self.window.secs() as i64;
                while snap.buckets.len() > 2 && snap.buckets[0].ts_end < cutoff {
                    snap.buckets.remove(0);
                }
            }
            // Refresh full snapshot (processes/PSI/GPUs/mounts) every 3s
            if tick.ts - self.last_refresh_ts >= 3 {
                self.last_refresh_ts = tick.ts;
                self.request_snapshot(send_msg);
            }
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        send_msg: &mut dyn FnMut(ClientMsg),
    ) -> Option<VitalsSpawnRequest> {
        let mut spawn_req = None;

        // Top Control Bar
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Window:").strong().color(crate::theme::text_color()));
            for w in [
                TimeWindow::Min1,
                TimeWindow::Min5,
                TimeWindow::Min15,
                TimeWindow::Hour1,
                TimeWindow::Hour6,
                TimeWindow::Hour24,
            ] {
                let active = self.window == w && self.zoom_range.is_none();
                if be_toggle_button(ui, w.label(), active).clicked() {
                    self.window = w;
                    self.zoom_range = None;
                    self.request_snapshot(send_msg);
                }
            }
            if self.zoom_range.is_some() && be_button(ui, "Reset Zoom").clicked() {
                self.zoom_range = None;
                self.request_snapshot(send_msg);
            }
            ui.separator();
            if be_toggle_button(ui, "Pulse Cores", self.show_pulse_cores).clicked() {
                self.show_pulse_cores = !self.show_pulse_cores;
            }
            if be_toggle_button(ui, "Group by Unit", self.group_by_unit).clicked() {
                self.group_by_unit = !self.group_by_unit;
            }
            if be_button(ui, "Refresh").clicked() {
                self.request_snapshot(send_msg);
            }
        });
        ui.separator();

        let pal = crate::theme::palette();
        let Some(snap) = self.snapshot.clone() else {
            ui.label(
                RichText::new("Loading Vitals telemetry history...")
                    .italics()
                    .color(pal.text_muted),
            );
            return None;
        };

        // Reset hover each frame; charts below will populate it if hovered
        let mut next_hover: Option<f32> = None;
        let mut new_zoom: Option<(i64, i64)> = None;

        ScrollArea::vertical()
            .id_salt("vitals_main_scroll")
            .show(ui, |ui| {
                // --- PSI & KERNEL HEALTH STRIP ---
                ui.horizontal_wrapped(|ui| {
                    psi_pill(ui, "PSI CPU", snap.psi.cpu.some_avg10, snap.psi.cpu.full_avg10);
                    psi_pill(
                        ui,
                        "PSI MEM",
                        snap.psi.memory.some_avg10,
                        snap.psi.memory.full_avg10,
                    );
                    psi_pill(ui, "PSI IO", snap.psi.io.some_avg10, snap.psi.io.full_avg10);
                    ui.label(
                        RichText::new(format!(
                            "History: {} · Drag any chart to zoom",
                            fmt_span(snap.history_span_secs)
                        ))
                        .size(10.5)
                        .color(pal.text_muted),
                    );
                });

                for alert in &snap.alerts {
                    let alert_bg = if pal.is_dark {
                        Color32::from_rgba_unmultiplied(248, 113, 113, 35)
                    } else {
                        Color32::from_rgb(249, 216, 216)
                    };
                    egui::Frame::NONE
                        .fill(alert_bg)
                        .stroke(Stroke::new(1.0, pal.status_red))
                        .inner_margin(4.0)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("⚠ {alert}"))
                                    .size(11.0)
                                    .color(pal.status_red),
                            );
                        });
                }
                ui.add_space(4.0);

                // --- SYNCHRONIZED TIME-TRAVEL CHARTS (2x2 + Network) ---
                let buckets = &snap.buckets;
                let latest = &snap.latest;

                let cpu_series: Vec<f32> = buckets.iter().map(|b| b.cpu_avg).collect();
                let mem_series: Vec<f32> = buckets.iter().map(|b| b.mem_pct_avg).collect();
                let gpu_series: Vec<f32> = buckets
                    .iter()
                    .map(|b| {
                        if b.gpu_util_avg > 0.0 {
                            b.gpu_util_avg
                        } else {
                            b.gpu_vram_pct_avg
                        }
                    })
                    .collect();
                let disk_r: Vec<f32> = buckets.iter().map(|b| b.disk_read_kbps).collect();
                let disk_w: Vec<f32> = buckets.iter().map(|b| b.disk_write_kbps).collect();
                let net_rx: Vec<f32> = buckets.iter().map(|b| b.net_rx_kbps).collect();
                let net_tx: Vec<f32> = buckets.iter().map(|b| b.net_tx_kbps).collect();
                let ts_tot: Vec<f32> = buckets
                    .iter()
                    .map(|b| b.ts_rx_kbps + b.ts_tx_kbps)
                    .collect();

                ui.columns(2, |cols| {
                    // 1) CPU Chart
                    let temp_part = latest
                        .cpu_temp_c
                        .map(|t| format!(" · {t:.0}°C"))
                        .unwrap_or_default();
                    let cpu_title = format!(
                        "CPU {:.1}% · {:.2} GHz{temp_part}",
                        latest.cpu_percent, latest.cpu_freq_ghz
                    );
                    let cpu_callout = format!("#1 {}", latest.top_cpu_label);
                    let (h, z) = render_beos_chart(
                        &mut cols[0],
                        "vitals_cpu",
                        &cpu_title,
                        &cpu_callout,
                        &[(
                            &cpu_series,
                            Color32::from_rgb(46, 180, 95),
                            "CPU%",
                        )],
                        Some(100.0),
                        "%",
                        buckets,
                        self.hover_frac,
                        &mut self.drag_zoom,
                    );
                    if h.is_some() {
                        next_hover = h;
                    }
                    if z.is_some() {
                        new_zoom = z;
                    }

                    // 2) GPU & VRAM Chart
                    let gpu_header = if let Some(g) = snap.gpus.first() {
                        let util = g
                            .utilization_percent
                            .map(|u| format!("{u:.0}%"))
                            .unwrap_or_else(|| "VRAM".into());
                        let extra = match (g.temperature_c, g.power_draw_w) {
                            (Some(t), Some(p)) => format!(" · {t:.0}°C · {p:.0}W"),
                            (Some(t), None) => format!(" · {t:.0}°C"),
                            _ => String::new(),
                        };
                        format!(
                            "GPU {} · {:.1}/{:.1} GiB{extra}",
                            util,
                            g.vram_used_mb as f32 / 1024.0,
                            g.vram_total_mb as f32 / 1024.0
                        )
                    } else {
                        "GPU (none)".into()
                    };
                    let gpu_callout = format!("#1 {}", latest.top_gpu_label);
                    let (h, z) = render_beos_chart(
                        &mut cols[1],
                        "vitals_gpu",
                        &gpu_header,
                        &gpu_callout,
                        &[(
                            &gpu_series,
                            Color32::from_rgb(76, 168, 240),
                            "GPU/VRAM%",
                        )],
                        Some(100.0),
                        "%",
                        buckets,
                        self.hover_frac,
                        &mut self.drag_zoom,
                    );
                    if h.is_some() {
                        next_hover = h;
                    }
                    if z.is_some() {
                        new_zoom = z;
                    }
                });

                ui.add_space(4.0);

                ui.columns(2, |cols| {
                    // 3) Memory Chart
                    let mem_title = format!(
                        "Memory {:.1} / {:.1} GiB",
                        latest.mem_used_mb as f32 / 1024.0,
                        latest.mem_total_mb as f32 / 1024.0
                    );
                    let ram_callout = format!("#1 {}", latest.top_ram_label);
                    let (h, z) = render_beos_chart(
                        &mut cols[0],
                        "vitals_mem",
                        &mem_title,
                        &ram_callout,
                        &[(
                            &mem_series,
                            Color32::from_rgb(235, 185, 25),
                            "RAM%",
                        )],
                        Some(100.0),
                        "%",
                        buckets,
                        self.hover_frac,
                        &mut self.drag_zoom,
                    );
                    if h.is_some() {
                        next_hover = h;
                    }
                    if z.is_some() {
                        new_zoom = z;
                    }

                    // 4) Disk I/O Chart
                    let disk_title = format!(
                        "Disk I/O R:{:.0} W:{:.0} KB/s · {} IOPS",
                        latest.disk_read_kbps, latest.disk_write_kbps, latest.disk_iops
                    );
                    let disk_callout = format!("#1 {}", latest.top_disk_label);
                    let (h, z) = render_beos_chart(
                        &mut cols[1],
                        "vitals_disk",
                        &disk_title,
                        &disk_callout,
                        &[
                            (&disk_r, Color32::from_rgb(86, 195, 210), "Read"),
                            (&disk_w, Color32::from_rgb(235, 110, 95), "Write"),
                        ],
                        None,
                        " KB/s",
                        buckets,
                        self.hover_frac,
                        &mut self.drag_zoom,
                    );
                    if h.is_some() {
                        next_hover = h;
                    }
                    if z.is_some() {
                        new_zoom = z;
                    }
                });

                ui.add_space(4.0);

                // 5) Network Chart (Physical vs Tailscale)
                let net_title = format!(
                    "Network Phys ↓{:.0} ↑{:.0} KB/s  ·  tailscale0 ↓{:.0} ↑{:.0} KB/s",
                    latest.net_rx_kbps,
                    latest.net_tx_kbps,
                    latest.tailscale_rx_kbps,
                    latest.tailscale_tx_kbps
                );
                let (h, z) = render_beos_chart(
                    ui,
                    "vitals_net",
                    &net_title,
                    "phys vs tailscale0",
                    &[
                        (&net_rx, Color32::from_rgb(76, 185, 120), "RX"),
                        (&net_tx, Color32::from_rgb(230, 165, 60), "TX"),
                        (&ts_tot, Color32::from_rgb(130, 175, 255), "ts0"),
                    ],
                    None,
                    " KB/s",
                    buckets,
                    self.hover_frac,
                    &mut self.drag_zoom,
                );
                if h.is_some() {
                    next_hover = h;
                }
                if z.is_some() {
                    new_zoom = z;
                }

                // --- BEOS PULSE PER-CORE BAR GRID ---
                if self.show_pulse_cores && !latest.cpu_cores.is_empty() {
                    ui.add_space(6.0);
                    render_pulse_cores(ui, &latest.cpu_cores, latest.load_avg);
                }

                // --- STORAGE MOUNTS & NICS ---
                ui.add_space(6.0);
                ui.columns(2, |cols| {
                    cols[0].label(
                        RichText::new("STORAGE MOUNTS")
                            .strong()
                            .size(10.5)
                            .color(pal.accent),
                    );
                    for m in snap.mounts.iter().take(5) {
                        let frac = if m.total_gb > 0.0 {
                            (m.used_gb / m.total_gb).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        cols[0].add(
                            egui::ProgressBar::new(frac)
                                .corner_radius(CornerRadius::ZERO)
                                .desired_height(15.0)
                                .text(format!(
                                    "{} ({}) {:.0}/{:.0} GB",
                                    m.mount_point, m.fs_type, m.used_gb, m.total_gb
                                )),
                        );
                    }

                    cols[1].label(
                        RichText::new("NETWORK INTERFACES")
                            .strong()
                            .size(10.5)
                            .color(pal.accent),
                    );
                    for nic in snap.nics.iter().take(6) {
                        cols[1].horizontal(|ui| {
                            let tag = if nic.is_tailscale { "[TS]" } else { "[NIC]" };
                            ui.label(
                                RichText::new(format!("{tag} {}", nic.name))
                                    .monospace()
                                    .strong()
                                    .size(11.0)
                                    .color(if nic.is_tailscale {
                                        pal.status_sky
                                    } else {
                                        pal.text
                                    }),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "↓{:.1} KB/s  ↑{:.1} KB/s",
                                    nic.rx_kbps, nic.tx_kbps
                                ))
                                .monospace()
                                .size(10.5)
                                .color(pal.text_muted),
                            );
                        });
                    }
                });

                // --- PROCESS & SYSTEMD UNIT TABLE ---
                ui.add_space(6.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Processes:").strong().color(crate::theme::text_color()));
                    ui.add(egui::TextEdit::singleline(&mut self.proc_filter).desired_width(140.0));
                    for (label, sort) in [
                        ("CPU", ProcSort::Cpu),
                        ("RAM", ProcSort::Ram),
                        ("Disk", ProcSort::Disk),
                        ("VRAM", ProcSort::Vram),
                    ] {
                        if be_toggle_button(ui, label, self.proc_sort == sort).clicked() {
                            self.proc_sort = sort;
                        }
                    }
                });

                let needle = self.proc_filter.to_ascii_lowercase();
                let mut procs: Vec<ProcessVital> = snap
                    .processes
                    .into_iter()
                    .filter(|p| {
                        needle.is_empty()
                            || p.name.to_ascii_lowercase().contains(&needle)
                            || p.unit
                                .as_deref()
                                .unwrap_or("")
                                .to_ascii_lowercase()
                                .contains(&needle)
                    })
                    .collect();

                procs.sort_by(|a, b| match self.proc_sort {
                    ProcSort::Cpu => b.cpu_percent.total_cmp(&a.cpu_percent),
                    ProcSort::Ram => b.mem_mb.cmp(&a.mem_mb),
                    ProcSort::Disk => (b.disk_read_kbps + b.disk_write_kbps)
                        .total_cmp(&(a.disk_read_kbps + a.disk_write_kbps)),
                    ProcSort::Vram => b.vram_mb.cmp(&a.vram_mb),
                });

                if self.group_by_unit {
                    let mut grouped: BTreeMap<String, Vec<ProcessVital>> = BTreeMap::new();
                    for p in procs {
                        let key = p
                            .unit
                            .clone()
                            .unwrap_or_else(|| "(unscoped / user shell)".into());
                        grouped.entry(key).or_default().push(p);
                    }
                    for (unit_name, list) in grouped {
                        let sum_mem: u64 = list.iter().map(|p| p.mem_mb).sum();
                        let sum_cpu: f32 = list.iter().map(|p| p.cpu_percent).sum();
                        egui::CollapsingHeader::new(
                            RichText::new(format!(
                                "{unit_name} ({} procs · CPU {:.1}% · {} MiB)",
                                list.len(),
                                sum_cpu,
                                sum_mem
                            ))
                            .strong()
                            .color(crate::theme::text_color()),
                        )
                        .default_open(unit_name.ends_with(".service"))
                        .show(ui, |ui| {
                            for p in &list {
                                if let Some(req) = render_process_row(ui, p, send_msg) {
                                    spawn_req = Some(req);
                                }
                            }
                        });
                    }
                } else {
                    for p in &procs {
                        if let Some(req) = render_process_row(ui, p, send_msg) {
                            spawn_req = Some(req);
                        }
                    }
                }
            });

        self.hover_frac = next_hover;
        if let Some((start_ts, end_ts)) = new_zoom {
            self.zoom_range = Some((start_ts, end_ts));
            self.request_snapshot(send_msg);
        }

        spawn_req
    }
}

#[allow(clippy::too_many_arguments)]
fn render_beos_chart(
    ui: &mut Ui,
    id_salt: &str,
    title: &str,
    callout: &str,
    series: &[(&[f32], Color32, &str)],
    fixed_max: Option<f32>,
    unit_suffix: &str,
    buckets: &[VitalsBucket],
    shared_hover: Option<f32>,
    drag_zoom: &mut Option<(f32, f32)>,
) -> (Option<f32>, Option<(i64, i64)>) {
    let mut hovered_frac = None;
    let mut zoom_result = None;

    let pal = crate::theme::palette();
    egui::Frame::NONE
        .fill(pal.well)
        .stroke(Stroke::new(1.0, pal.border))
        .inner_margin(4.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong().size(11.0).color(crate::theme::text_color()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(callout)
                            .monospace()
                            .size(10.0)
                            .color(pal.status_sky),
                    );
                });
            });

            let avail_w = ui.available_width().max(120.0);
            let (rect, resp) = ui.allocate_exact_size(
                Vec2::new(avail_w, 72.0),
                Sense::click_and_drag(),
            );
            let painter = ui.painter_at(rect);

            // Recessed dark BeOS telemetry well
            painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(20, 24, 30));
            draw_beos_bevel(&painter, rect, true);

            // Subtle horizontal gridlines
            let inner = rect.shrink(3.0);
            for frac in [0.25, 0.5, 0.75] {
                let y = inner.min.y + inner.height() * frac;
                painter.line_segment(
                    [Pos2::new(inner.min.x, y), Pos2::new(inner.max.x, y)],
                    Stroke::new(0.7, Color32::from_rgb(38, 45, 56)),
                );
            }

            let y_max = fixed_max.unwrap_or_else(|| {
                series
                    .iter()
                    .flat_map(|(vals, _, _)| vals.iter().copied())
                    .fold(10.0_f32, f32::max)
                    * 1.15
            });

            // Draw each series with Mesh gradient area fill + crisp polyline
            for (vals, color, _) in series {
                let n = vals.len();
                if n < 2 {
                    continue;
                }
                let step = inner.width() / (n - 1) as f32;
                let pts: Vec<Pos2> = vals
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| {
                        let norm = (v / y_max.max(0.001)).clamp(0.0, 1.0);
                        Pos2::new(
                            inner.min.x + i as f32 * step,
                            inner.max.y - norm * inner.height(),
                        )
                    })
                    .collect();

                let mut mesh = Mesh::default();
                let top_col =
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 72);
                let bot_col =
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 8);
                for (i, pt) in pts.iter().enumerate() {
                    let base = (i * 2) as u32;
                    mesh.vertices.push(Vertex {
                        pos: *pt,
                        uv: Pos2::ZERO,
                        color: top_col,
                    });
                    mesh.vertices.push(Vertex {
                        pos: Pos2::new(pt.x, inner.max.y),
                        uv: Pos2::ZERO,
                        color: bot_col,
                    });
                    if i + 1 < pts.len() {
                        mesh.indices
                            .extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
                    }
                }
                painter.add(egui::Shape::mesh(mesh));
                painter.add(egui::Shape::line(pts, Stroke::new(1.5, *color)));
            }

            // Hover & Drag-to-Zoom interaction
            if let Some(pos) = resp.hover_pos() {
                let f = ((pos.x - inner.min.x) / inner.width().max(1.0)).clamp(0.0, 1.0);
                hovered_frac = Some(f);
            }
            if resp.drag_started()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let f = ((pos.x - inner.min.x) / inner.width().max(1.0)).clamp(0.0, 1.0);
                *drag_zoom = Some((f, f));
            }
            if resp.dragged()
                && let Some(pos) = resp.interact_pointer_pos()
                && let Some((start, _)) = *drag_zoom
            {
                let f = ((pos.x - inner.min.x) / inner.width().max(1.0)).clamp(0.0, 1.0);
                *drag_zoom = Some((start, f));
            }
            if resp.drag_stopped()
                && let Some((a, b)) = drag_zoom.take()
            {
                let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                if hi - lo > 0.04 && buckets.len() >= 2 {
                    let i0 = ((lo * (buckets.len() - 1) as f32).round() as usize)
                        .min(buckets.len() - 1);
                    let i1 = ((hi * (buckets.len() - 1) as f32).round() as usize)
                        .min(buckets.len() - 1);
                    let ts0 = buckets[i0].ts_start;
                    let ts1 = buckets[i1].ts_end;
                    if ts1 > ts0 {
                        zoom_result = Some((ts0, ts1));
                    }
                }
            }

            // Draw drag-to-zoom selection highlight
            if let Some((a, b)) = *drag_zoom {
                let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                let sel = Rect::from_min_max(
                    Pos2::new(inner.min.x + lo * inner.width(), inner.min.y),
                    Pos2::new(inner.min.x + hi * inner.width(), inner.max.y),
                );
                painter.rect_filled(
                    sel,
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(255, 203, 0, 55),
                );
            }

            // Synchronized Yellow Crosshair Scrubber
            let active_hover = hovered_frac.or(shared_hover);
            if let Some(f) = active_hover {
                let x = inner.min.x + f * inner.width();
                painter.line_segment(
                    [Pos2::new(x, inner.min.y), Pos2::new(x, inner.max.y)],
                    Stroke::new(1.2, BEOS_TAB_YELLOW),
                );
                if !buckets.is_empty() {
                    let idx = ((f * (buckets.len().saturating_sub(1)) as f32).round() as usize)
                        .min(buckets.len() - 1);
                    let summary: Vec<String> = series
                        .iter()
                        .filter_map(|(vals, _, name)| {
                            vals.get(idx)
                                .map(|v| format!("{name} {:.1}{unit_suffix}", v))
                        })
                        .collect();
                    let label = summary.join(" · ");
                    painter.text(
                        Pos2::new(inner.min.x + 4.0, inner.min.y + 3.0),
                        egui::Align2::LEFT_TOP,
                        label,
                        egui::FontId::monospace(9.5),
                        BEOS_TAB_YELLOW,
                    );
                }
            }
            let _ = id_salt;
        });

    (hovered_frac, zoom_result)
}

fn render_pulse_cores(ui: &mut Ui, cores: &[f32], load_avg: [f32; 3]) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "BeOS Pulse ({} cores) · load {:.2} / {:.2} / {:.2}",
                cores.len(),
                load_avg[0],
                load_avg[1],
                load_avg[2]
            ))
            .strong()
            .size(11.0)
            .color(crate::theme::text_color()),
        );
    });
    let avail_w = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, 46.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(18, 22, 28));
    draw_beos_bevel(&painter, rect, true);

    let inner = rect.shrink(4.0);
    let n = cores.len().max(1);
    let gap = 2.0;
    let bar_w = ((inner.width() - gap * (n as f32 - 1.0)) / n as f32).max(4.0);
    for (i, &usage) in cores.iter().enumerate() {
        let x = inner.min.x + i as f32 * (bar_w + gap);
        let frac = (usage / 100.0).clamp(0.0, 1.0);
        let h = (inner.height() * frac).max(2.0);
        let bar_rect = Rect::from_min_max(
            Pos2::new(x, inner.max.y - h),
            Pos2::new(x + bar_w, inner.max.y),
        );
        let col = if usage > 85.0 {
            BEOS_LED_RED
        } else if usage > 55.0 {
            BEOS_LED_GOLD
        } else {
            BEOS_LED_GREEN
        };
        painter.rect_filled(bar_rect, CornerRadius::ZERO, col);
    }
}

fn render_process_row(
    ui: &mut Ui,
    p: &ProcessVital,
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<VitalsSpawnRequest> {
    let pal = crate::theme::palette();
    let mut spawn_req = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{:>6}", p.pid))
                .monospace()
                .size(10.5)
                .color(pal.text_muted),
        );
        ui.label(RichText::new(&p.name).strong().size(11.0).color(pal.text));
        if let Some(ref unit) = p.unit
            && ui
                .small_button(RichText::new(format!("● {unit}")).size(10.0).color(pal.status_green))
                .clicked()
        {
            spawn_req = Some(VitalsSpawnRequest::OpenUnitLogs { unit: unit.clone() });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("KILL").clicked() {
                send_msg(ClientMsg::SignalProcess {
                    pid: p.pid,
                    signal: ProcessSignal::Kill,
                });
            }
            if ui.small_button("TERM").clicked() {
                send_msg(ClientMsg::SignalProcess {
                    pid: p.pid,
                    signal: ProcessSignal::Term,
                });
            }
            if ui
                .small_button("+5")
                .on_hover_text("Renice +5 (lower priority)")
                .clicked()
            {
                send_msg(ClientMsg::SignalProcess {
                    pid: p.pid,
                    signal: ProcessSignal::Renice(5),
                });
            }
            let vram_part = if p.vram_mb > 0 {
                format!(" · VRAM {}M", p.vram_mb)
            } else {
                String::new()
            };
            ui.label(
                RichText::new(format!(
                    "CPU {:4.1}% · {:>4}M · IO {:.0}/{:.0}K{vram_part}",
                    p.cpu_percent, p.mem_mb, p.disk_read_kbps, p.disk_write_kbps
                ))
                .monospace()
                .size(10.5)
                .color(pal.text),
            );
        });
    });
    spawn_req
}

fn psi_pill(ui: &mut Ui, label: &str, some10: f32, full10: f32) {
    let led = if full10 > 15.0 || some10 > 30.0 {
        BEOS_LED_RED
    } else if some10 > 5.0 {
        BEOS_LED_GOLD
    } else {
        BEOS_LED_GREEN
    };
    let pal = crate::theme::palette();
    egui::Frame::NONE
        .fill(pal.surface_alt)
        .stroke(Stroke::new(1.0, pal.border))
        .inner_margin(egui::Margin::symmetric(5, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::ZERO, led);
                ui.label(
                    RichText::new(format!("{label} {some10:.1}%/{full10:.1}%"))
                        .monospace()
                        .size(10.0)
                        .color(crate::theme::text_color()),
                );
            });
        });
}

fn fmt_span(secs: u64) -> String {
    if secs >= 3600 {
        format!("{:.1}h", secs as f64 / 3600.0)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}
