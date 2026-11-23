use kaage_proto::{
    GpuProcessVital, GpuVital, KernelPsi, MountVital, NicVital, ProcessSignal, ProcessVital,
    PsiMetric, VitalsBucket, VitalsQuery, VitalsSnapshot, VitalsTick,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::PathBuf,
    process::Command,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sysinfo::{Disks, System};
use tokio::sync::broadcast;

const MAX_LIVE_TICKS: usize = 3_600; // 1 hour @ 1s
const MAX_ROLLUP_BUCKETS: usize = 8_640; // 24 hours @ 10s
const ROLLUP_INTERVAL_SECS: usize = 10;

#[derive(Serialize, Deserialize, Default)]
struct PersistedVitals {
    rollups: Vec<VitalsBucket>,
}

struct VitalsStore {
    live_ring: VecDeque<VitalsTick>,
    rollup_ring: VecDeque<VitalsBucket>,
    pending_rollup: Vec<VitalsTick>,
    latest_gpus: Vec<GpuVital>,
    latest_psi: KernelPsi,
    latest_alerts: Vec<String>,
    latest_mounts: Vec<MountVital>,
    latest_nics: Vec<NicVital>,
    latest_processes: Vec<ProcessVital>,
}

impl VitalsStore {
    fn new(rollups: Vec<VitalsBucket>) -> Self {
        let mut rollup_ring = VecDeque::with_capacity(MAX_ROLLUP_BUCKETS);
        for b in rollups.into_iter().rev().take(MAX_ROLLUP_BUCKETS).rev() {
            rollup_ring.push_back(b);
        }
        Self {
            live_ring: VecDeque::with_capacity(MAX_LIVE_TICKS),
            rollup_ring,
            pending_rollup: Vec::with_capacity(ROLLUP_INTERVAL_SECS),
            latest_gpus: Vec::new(),
            latest_psi: KernelPsi::default(),
            latest_alerts: Vec::new(),
            latest_mounts: Vec::new(),
            latest_nics: Vec::new(),
            latest_processes: Vec::new(),
        }
    }
}

pub struct VitalsCollector {
    store: RwLock<VitalsStore>,
    tick_tx: broadcast::Sender<VitalsTick>,
    persist_path: PathBuf,
}

impl VitalsCollector {
    pub fn new() -> Arc<Self> {
        let persist_path = crate::files::expand_path("~/.local/share/kaage/vitals.bin");
        let rollups = load_persisted_rollups(&persist_path);
        let (tick_tx, _) = broadcast::channel(64);
        Arc::new(Self {
            store: RwLock::new(VitalsStore::new(rollups)),
            tick_tx,
            persist_path,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<VitalsTick> {
        self.tick_tx.subscribe()
    }

    pub fn spawn_background(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut sys = System::new_all();
            let mut prev_disk = read_diskstats_totals();
            let mut prev_nics = read_net_dev_totals();
            let mut tick_count: u64 = 0;
            let mut cached_gpus: Vec<GpuVital> = Vec::new();
            let mut cached_dmesg_alerts: Vec<String> = Vec::new();

            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tick_count += 1;

                sys.refresh_cpu_all();
                sys.refresh_memory();
                sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

                let ts = now_unix_secs();

                // GPU telemetry every 2s (lightweight)
                if tick_count == 1 || tick_count.is_multiple_of(2) {
                    cached_gpus = collect_gpu_vitals().await;
                }
                // Kernel ring-buffer alert scan every 15s
                if tick_count == 1 || tick_count.is_multiple_of(15) {
                    cached_dmesg_alerts = tokio::task::spawn_blocking(scan_kernel_alerts)
                        .await
                        .unwrap_or_default();
                }

                // Disk I/O deltas
                let cur_disk = read_diskstats_totals();
                let disk_read_sectors = cur_disk.read_sectors.saturating_sub(prev_disk.read_sectors);
                let disk_write_sectors = cur_disk
                    .write_sectors
                    .saturating_sub(prev_disk.write_sectors);
                let disk_iops = cur_disk.ios.saturating_sub(prev_disk.ios) as u32;
                prev_disk = cur_disk;
                let disk_read_kbps = (disk_read_sectors as f32 * 512.0) / 1024.0;
                let disk_write_kbps = (disk_write_sectors as f32 * 512.0) / 1024.0;

                // Network deltas (physical vs tailscale0)
                let cur_nics = read_net_dev_totals();
                let mut nics = Vec::new();
                let mut net_rx_kbps = 0.0_f32;
                let mut net_tx_kbps = 0.0_f32;
                let mut ts_rx_kbps = 0.0_f32;
                let mut ts_tx_kbps = 0.0_f32;
                for (name, &(rx, tx)) in &cur_nics {
                    if name == "lo" {
                        continue;
                    }
                    let (prx, ptx) = prev_nics.get(name).copied().unwrap_or((rx, tx));
                    let rx_k = rx.saturating_sub(prx) as f32 / 1024.0;
                    let tx_k = tx.saturating_sub(ptx) as f32 / 1024.0;
                    let is_ts = name.starts_with("tailscale");
                    if is_ts {
                        ts_rx_kbps += rx_k;
                        ts_tx_kbps += tx_k;
                    } else {
                        net_rx_kbps += rx_k;
                        net_tx_kbps += tx_k;
                    }
                    nics.push(NicVital {
                        name: name.clone(),
                        is_tailscale: is_ts,
                        rx_kbps: rx_k,
                        tx_kbps: tx_k,
                    });
                }
                nics.sort_by(|a, b| b.is_tailscale.cmp(&a.is_tailscale).then_with(|| a.name.cmp(&b.name)));
                prev_nics = cur_nics;

                // GPU VRAM map by PID
                let mut vram_by_pid: HashMap<u32, u64> = HashMap::new();
                for g in &cached_gpus {
                    for gp in &g.top_processes {
                        *vram_by_pid.entry(gp.pid).or_insert(0) += gp.vram_mb;
                    }
                }

                // Processes + cgroup systemd unit mapping
                let mut processes: Vec<ProcessVital> = sys
                    .processes()
                    .values()
                    .filter(|p| p.thread_kind().is_none())
                    .map(|p| {
                        let pid = p.pid().as_u32();
                        let du = p.disk_usage();
                        ProcessVital {
                            pid,
                            name: p.name().to_string_lossy().into_owned(),
                            unit: read_pid_systemd_unit(pid),
                            cpu_percent: p.cpu_usage(),
                            mem_mb: p.memory() / 1024 / 1024,
                            disk_read_kbps: du.read_bytes as f32 / 1024.0,
                            disk_write_kbps: du.written_bytes as f32 / 1024.0,
                            vram_mb: vram_by_pid.get(&pid).copied().unwrap_or(0),
                        }
                    })
                    .collect();

                // Top consumers for chart header callouts
                let top_cpu_label = processes
                    .iter()
                    .max_by(|a, b| a.cpu_percent.total_cmp(&b.cpu_percent))
                    .map(|p| format!("{} ({:.0}%)", p.name, p.cpu_percent))
                    .unwrap_or_default();
                let top_ram_label = processes
                    .iter()
                    .max_by_key(|p| p.mem_mb)
                    .map(|p| format!("{} ({} MiB)", p.name, p.mem_mb))
                    .unwrap_or_default();
                let top_disk_label = processes
                    .iter()
                    .max_by(|a, b| {
                        (a.disk_read_kbps + a.disk_write_kbps)
                            .total_cmp(&(b.disk_read_kbps + b.disk_write_kbps))
                    })
                    .filter(|p| p.disk_read_kbps + p.disk_write_kbps > 0.5)
                    .map(|p| {
                        format!(
                            "{} ({:.0} KB/s)",
                            p.name,
                            p.disk_read_kbps + p.disk_write_kbps
                        )
                    })
                    .unwrap_or_else(|| "idle".into());
                let top_gpu_label = cached_gpus
                    .iter()
                    .flat_map(|g| g.top_processes.iter())
                    .max_by_key(|p| p.vram_mb)
                    .map(|p| format!("{} ({} MiB)", p.name, p.vram_mb))
                    .or_else(|| {
                        cached_gpus
                            .first()
                            .map(|g| format!("{} ({} MiB)", g.name, g.vram_used_mb))
                    })
                    .unwrap_or_else(|| "none".into());

                // Keep top 40 most active/largest processes for the UI table
                processes.sort_by(|a, b| {
                    b.cpu_percent
                        .total_cmp(&a.cpu_percent)
                        .then_with(|| b.mem_mb.cmp(&a.mem_mb))
                });
                processes.truncate(40);

                // CPU metrics + hwmon package temp
                let cpu_percent = sys.global_cpu_usage();
                let cpu_cores: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
                let cpu_freq_ghz = if sys.cpus().is_empty() {
                    0.0
                } else {
                    let sum_mhz: u64 = sys.cpus().iter().map(|c| c.frequency()).sum();
                    (sum_mhz as f32 / sys.cpus().len() as f32) / 1000.0
                };
                let cpu_temp_c = read_cpu_hwmon_temp();
                let load = System::load_average();

                let primary_gpu = cached_gpus.first();
                let tick = VitalsTick {
                    ts,
                    cpu_percent,
                    cpu_cores,
                    cpu_freq_ghz,
                    cpu_temp_c,
                    load_avg: [load.one as f32, load.five as f32, load.fifteen as f32],
                    mem_used_mb: sys.used_memory() / 1024 / 1024,
                    mem_total_mb: sys.total_memory() / 1024 / 1024,
                    swap_used_mb: sys.used_swap() / 1024 / 1024,
                    swap_total_mb: sys.total_swap() / 1024 / 1024,
                    disk_read_kbps,
                    disk_write_kbps,
                    disk_iops,
                    net_rx_kbps,
                    net_tx_kbps,
                    tailscale_rx_kbps: ts_rx_kbps,
                    tailscale_tx_kbps: ts_tx_kbps,
                    gpu_util_percent: primary_gpu.and_then(|g| g.utilization_percent),
                    gpu_vram_used_mb: primary_gpu.map(|g| g.vram_used_mb).unwrap_or(0),
                    gpu_vram_total_mb: primary_gpu.map(|g| g.vram_total_mb).unwrap_or(0),
                    top_cpu_label,
                    top_ram_label,
                    top_disk_label,
                    top_gpu_label,
                };

                // Kernel PSI & alerts
                let psi = read_kernel_psi();
                let mut alerts = cached_dmesg_alerts.clone();
                if psi.memory.some_avg10 > 20.0 {
                    alerts.insert(
                        0,
                        format!(
                            "High memory pressure (PSI some avg10: {:.1}%)",
                            psi.memory.some_avg10
                        ),
                    );
                }
                if psi.io.full_avg10 > 20.0 {
                    alerts.insert(
                        0,
                        format!("High I/O stall pressure (PSI full avg10: {:.1}%)", psi.io.full_avg10),
                    );
                }

                // Storage mounts (refresh every 5s)
                let mounts = if tick_count == 1 || tick_count.is_multiple_of(5) {
                    Some(collect_mounts())
                } else {
                    None
                };

                let mut should_persist = false;
                {
                    let mut st = this.store.write().unwrap();
                    if st.live_ring.len() >= MAX_LIVE_TICKS {
                        st.live_ring.pop_front();
                    }
                    st.live_ring.push_back(tick.clone());
                    st.pending_rollup.push(tick.clone());
                    if st.pending_rollup.len() >= ROLLUP_INTERVAL_SECS {
                        let bucket = aggregate_ticks(&st.pending_rollup);
                        st.pending_rollup.clear();
                        if st.rollup_ring.len() >= MAX_ROLLUP_BUCKETS {
                            st.rollup_ring.pop_front();
                        }
                        st.rollup_ring.push_back(bucket);
                    }
                    st.latest_gpus = cached_gpus.clone();
                    st.latest_psi = psi;
                    st.latest_alerts = alerts;
                    if let Some(m) = mounts {
                        st.latest_mounts = m;
                    }
                    st.latest_nics = nics;
                    st.latest_processes = processes;
                    if tick_count.is_multiple_of(60) {
                        should_persist = true;
                    }
                }

                if should_persist {
                    let snapshot_rollups: Vec<VitalsBucket> = this
                        .store
                        .read()
                        .unwrap()
                        .rollup_ring
                        .iter()
                        .cloned()
                        .collect();
                    let path = this.persist_path.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        save_persisted_rollups(&path, &snapshot_rollups);
                    });
                }

                let _ = this.tick_tx.send(tick);
            }
        });
    }

    pub fn query_snapshot(&self, q: &VitalsQuery) -> VitalsSnapshot {
        let st = self.store.read().unwrap();
        let latest = st.live_ring.back().cloned().unwrap_or_default();
        let now = if latest.ts > 0 {
            latest.ts
        } else {
            now_unix_secs()
        };

        let end_ts = q.end_ts.unwrap_or(now);
        let start_ts = q.start_ts.unwrap_or(end_ts - q.window.secs() as i64);
        let target_buckets = q.buckets.clamp(16, 400) as usize;

        let earliest_live = st.live_ring.front().map(|t| t.ts).unwrap_or(now);
        let buckets = if start_ts >= earliest_live && !st.live_ring.is_empty() {
            let slice: Vec<&VitalsTick> = st
                .live_ring
                .iter()
                .filter(|t| t.ts >= start_ts && t.ts <= end_ts)
                .collect();
            downsample_ticks(&slice, target_buckets)
        } else {
            let mut combined: Vec<VitalsBucket> = st
                .rollup_ring
                .iter()
                .filter(|b| b.ts_end >= start_ts && b.ts_start <= end_ts)
                .cloned()
                .collect();
            // Append any fresher live ticks after the last rollup
            let last_rollup_ts = combined.last().map(|b| b.ts_end).unwrap_or(0);
            let tail: Vec<&VitalsTick> = st
                .live_ring
                .iter()
                .filter(|t| t.ts > last_rollup_ts && t.ts >= start_ts && t.ts <= end_ts)
                .collect();
            if !tail.is_empty() {
                combined.extend(downsample_ticks(&tail, 24));
            }
            downsample_buckets(&combined, target_buckets)
        };

        let earliest_any = st
            .rollup_ring
            .front()
            .map(|b| b.ts_start)
            .unwrap_or(earliest_live)
            .min(earliest_live);
        let history_span_secs = now.saturating_sub(earliest_any).max(0) as u64;

        VitalsSnapshot {
            latest,
            buckets,
            gpus: st.latest_gpus.clone(),
            psi: st.latest_psi,
            alerts: st.latest_alerts.clone(),
            mounts: st.latest_mounts.clone(),
            nics: st.latest_nics.clone(),
            processes: st.latest_processes.clone(),
            history_span_secs,
        }
    }
}

pub fn signal_process(pid: u32, signal: ProcessSignal) -> Result<String, String> {
    if pid <= 1 {
        return Err("Refusing to signal PID <= 1".into());
    }
    let output = match signal {
        ProcessSignal::Term => Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .map_err(|e| e.to_string())?,
        ProcessSignal::Kill => Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output()
            .map_err(|e| e.to_string())?,
        ProcessSignal::Renice(val) => Command::new("renice")
            .args(["-n", &val.clamp(-20, 19).to_string(), "-p", &pid.to_string()])
            .output()
            .map_err(|e| e.to_string())?,
    };
    if output.status.success() {
        Ok(format!("Applied {signal:?} to PID {pid}"))
    } else {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("Failed to apply {signal:?} to PID {pid}")
        } else {
            err
        })
    }
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn load_persisted_rollups(path: &PathBuf) -> Vec<VitalsBucket> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    postcard::from_bytes::<PersistedVitals>(&bytes)
        .map(|p| p.rollups)
        .unwrap_or_default()
}

fn save_persisted_rollups(path: &PathBuf, rollups: &[VitalsBucket]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let payload = PersistedVitals {
        rollups: rollups.to_vec(),
    };
    if let Ok(bytes) = postcard::to_allocvec(&payload) {
        let tmp = path.with_extension("bin.tmp");
        if fs::write(&tmp, &bytes).is_ok() {
            let _ = fs::rename(tmp, path);
        }
    }
}

fn tick_to_bucket(t: &VitalsTick) -> VitalsBucket {
    let mem_pct = if t.mem_total_mb > 0 {
        (t.mem_used_mb as f32 / t.mem_total_mb as f32) * 100.0
    } else {
        0.0
    };
    let vram_pct = if t.gpu_vram_total_mb > 0 {
        (t.gpu_vram_used_mb as f32 / t.gpu_vram_total_mb as f32) * 100.0
    } else {
        0.0
    };
    VitalsBucket {
        ts_start: t.ts,
        ts_end: t.ts,
        cpu_avg: t.cpu_percent,
        cpu_max: t.cpu_percent,
        mem_pct_avg: mem_pct,
        gpu_util_avg: t.gpu_util_percent.unwrap_or(0.0),
        gpu_vram_pct_avg: vram_pct,
        disk_read_kbps: t.disk_read_kbps,
        disk_write_kbps: t.disk_write_kbps,
        net_rx_kbps: t.net_rx_kbps,
        net_tx_kbps: t.net_tx_kbps,
        ts_rx_kbps: t.tailscale_rx_kbps,
        ts_tx_kbps: t.tailscale_tx_kbps,
    }
}

pub fn aggregate_ticks(ticks: &[VitalsTick]) -> VitalsBucket {
    if ticks.is_empty() {
        return VitalsBucket::default();
    }
    let n = ticks.len() as f32;
    let mut cpu_sum = 0.0;
    let mut cpu_max = 0.0_f32;
    let mut mem_sum = 0.0;
    let mut gpu_sum = 0.0;
    let mut vram_sum = 0.0;
    let mut dr_sum = 0.0;
    let mut dw_sum = 0.0;
    let mut nrx_sum = 0.0;
    let mut ntx_sum = 0.0;
    let mut trx_sum = 0.0;
    let mut ttx_sum = 0.0;

    for t in ticks {
        cpu_sum += t.cpu_percent;
        cpu_max = cpu_max.max(t.cpu_percent);
        if t.mem_total_mb > 0 {
            mem_sum += (t.mem_used_mb as f32 / t.mem_total_mb as f32) * 100.0;
        }
        gpu_sum += t.gpu_util_percent.unwrap_or(0.0);
        if t.gpu_vram_total_mb > 0 {
            vram_sum += (t.gpu_vram_used_mb as f32 / t.gpu_vram_total_mb as f32) * 100.0;
        }
        dr_sum += t.disk_read_kbps;
        dw_sum += t.disk_write_kbps;
        nrx_sum += t.net_rx_kbps;
        ntx_sum += t.net_tx_kbps;
        trx_sum += t.tailscale_rx_kbps;
        ttx_sum += t.tailscale_tx_kbps;
    }

    VitalsBucket {
        ts_start: ticks.first().map(|t| t.ts).unwrap_or(0),
        ts_end: ticks.last().map(|t| t.ts).unwrap_or(0),
        cpu_avg: cpu_sum / n,
        cpu_max,
        mem_pct_avg: mem_sum / n,
        gpu_util_avg: gpu_sum / n,
        gpu_vram_pct_avg: vram_sum / n,
        disk_read_kbps: dr_sum / n,
        disk_write_kbps: dw_sum / n,
        net_rx_kbps: nrx_sum / n,
        net_tx_kbps: ntx_sum / n,
        ts_rx_kbps: trx_sum / n,
        ts_tx_kbps: ttx_sum / n,
    }
}

pub fn downsample_ticks(ticks: &[&VitalsTick], max_buckets: usize) -> Vec<VitalsBucket> {
    if ticks.is_empty() || max_buckets == 0 {
        return Vec::new();
    }
    if ticks.len() <= max_buckets {
        return ticks.iter().map(|t| tick_to_bucket(t)).collect();
    }
    let step = (ticks.len() as f64) / (max_buckets as f64);
    let mut out = Vec::with_capacity(max_buckets);
    for i in 0..max_buckets {
        let start = (i as f64 * step).floor() as usize;
        let end = (((i + 1) as f64 * step).floor() as usize)
            .max(start + 1)
            .min(ticks.len());
        let chunk: Vec<VitalsTick> = ticks[start..end].iter().map(|t| (*t).clone()).collect();
        out.push(aggregate_ticks(&chunk));
    }
    out
}

pub fn downsample_buckets(buckets: &[VitalsBucket], max_buckets: usize) -> Vec<VitalsBucket> {
    if buckets.is_empty() || max_buckets == 0 {
        return Vec::new();
    }
    if buckets.len() <= max_buckets {
        return buckets.to_vec();
    }
    let step = (buckets.len() as f64) / (max_buckets as f64);
    let mut out = Vec::with_capacity(max_buckets);
    for i in 0..max_buckets {
        let start = (i as f64 * step).floor() as usize;
        let end = (((i + 1) as f64 * step).floor() as usize)
            .max(start + 1)
            .min(buckets.len());
        let slice = &buckets[start..end];
        let n = slice.len() as f32;
        out.push(VitalsBucket {
            ts_start: slice.first().map(|b| b.ts_start).unwrap_or(0),
            ts_end: slice.last().map(|b| b.ts_end).unwrap_or(0),
            cpu_avg: slice.iter().map(|b| b.cpu_avg).sum::<f32>() / n,
            cpu_max: slice.iter().map(|b| b.cpu_max).fold(0.0_f32, f32::max),
            mem_pct_avg: slice.iter().map(|b| b.mem_pct_avg).sum::<f32>() / n,
            gpu_util_avg: slice.iter().map(|b| b.gpu_util_avg).sum::<f32>() / n,
            gpu_vram_pct_avg: slice.iter().map(|b| b.gpu_vram_pct_avg).sum::<f32>() / n,
            disk_read_kbps: slice.iter().map(|b| b.disk_read_kbps).sum::<f32>() / n,
            disk_write_kbps: slice.iter().map(|b| b.disk_write_kbps).sum::<f32>() / n,
            net_rx_kbps: slice.iter().map(|b| b.net_rx_kbps).sum::<f32>() / n,
            net_tx_kbps: slice.iter().map(|b| b.net_tx_kbps).sum::<f32>() / n,
            ts_rx_kbps: slice.iter().map(|b| b.ts_rx_kbps).sum::<f32>() / n,
            ts_tx_kbps: slice.iter().map(|b| b.ts_tx_kbps).sum::<f32>() / n,
        });
    }
    out
}

#[derive(Default, Clone, Copy)]
struct DiskTotals {
    read_sectors: u64,
    write_sectors: u64,
    ios: u64,
}

fn read_diskstats_totals() -> DiskTotals {
    let Ok(text) = fs::read_to_string("/proc/diskstats") else {
        return DiskTotals::default();
    };
    let mut totals = DiskTotals::default();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 {
            continue;
        }
        let dev = cols[2];
        // Count whole block devices (nvme0n1, sda, vda) and skip partitions / loop / ram
        let is_whole_nvme = dev.starts_with("nvme") && !dev.contains('p');
        let is_whole_sd = (dev.starts_with("sd") || dev.starts_with("vd"))
            && dev.chars().last().is_some_and(|c| c.is_ascii_alphabetic());
        if !is_whole_nvme && !is_whole_sd {
            continue;
        }
        let reads = cols[3].parse::<u64>().unwrap_or(0);
        let read_sec = cols[5].parse::<u64>().unwrap_or(0);
        let writes = cols[7].parse::<u64>().unwrap_or(0);
        let write_sec = cols[9].parse::<u64>().unwrap_or(0);
        totals.ios += reads + writes;
        totals.read_sectors += read_sec;
        totals.write_sectors += write_sec;
    }
    totals
}

fn read_net_dev_totals() -> HashMap<String, (u64, u64)> {
    let mut map = HashMap::new();
    let Ok(text) = fs::read_to_string("/proc/net/dev") else {
        return map;
    };
    for line in text.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let cols: Vec<&str> = rest.split_whitespace().collect();
        if cols.len() < 9 {
            continue;
        }
        let rx = cols[0].parse::<u64>().unwrap_or(0);
        let tx = cols[8].parse::<u64>().unwrap_or(0);
        map.insert(iface.trim().to_string(), (rx, tx));
    }
    map
}

fn read_pid_systemd_unit(pid: u32) -> Option<String> {
    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    cgroup
        .split('/')
        .find(|seg| seg.trim().ends_with(".service"))
        .map(|s| s.trim().to_string())
}

fn read_cpu_hwmon_temp() -> Option<f32> {
    let entries = fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = fs::read_to_string(dir.join("name"))
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if matches!(name.as_str(), "coretemp" | "k10temp" | "zenpower" | "acpitz")
            && let Ok(raw) = fs::read_to_string(dir.join("temp1_input"))
            && let Ok(millideg) = raw.trim().parse::<f32>()
        {
            return Some(millideg / 1000.0);
        }
    }
    None
}

pub fn parse_psi_file(content: &str) -> PsiMetric {
    let mut m = PsiMetric::default();
    for line in content.lines() {
        let is_some = line.starts_with("some ");
        let is_full = line.starts_with("full ");
        if !is_some && !is_full {
            continue;
        }
        for tok in line.split_whitespace().skip(1) {
            if let Some((k, v)) = tok.split_once('=')
                && let Ok(val) = v.parse::<f32>()
            {
                match (is_some, k) {
                    (true, "avg10") => m.some_avg10 = val,
                    (true, "avg60") => m.some_avg60 = val,
                    (false, "avg10") => m.full_avg10 = val,
                    (false, "avg60") => m.full_avg60 = val,
                    _ => {}
                }
            }
        }
    }
    m
}

fn read_kernel_psi() -> KernelPsi {
    KernelPsi {
        cpu: fs::read_to_string("/proc/pressure/cpu")
            .map(|s| parse_psi_file(&s))
            .unwrap_or_default(),
        memory: fs::read_to_string("/proc/pressure/memory")
            .map(|s| parse_psi_file(&s))
            .unwrap_or_default(),
        io: fs::read_to_string("/proc/pressure/io")
            .map(|s| parse_psi_file(&s))
            .unwrap_or_default(),
    }
}

fn scan_kernel_alerts() -> Vec<String> {
    let Ok(out) = Command::new("journalctl")
        .args(["-k", "-n", "60", "--no-pager", "-o", "cat"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut alerts = Vec::new();
    for line in text.lines().rev() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("oom-killer")
            || lower.contains("out of memory")
            || lower.contains("xid ")
            || lower.contains("nvme") && lower.contains("reset")
        {
            alerts.push(line.chars().take(120).collect());
            if alerts.len() >= 4 {
                break;
            }
        }
    }
    alerts
}

fn collect_mounts() -> Vec<MountVital> {
    let disks = Disks::new_with_refreshed_list();
    let mut mounts = Vec::new();
    for d in &disks {
        let total = d.total_space();
        if total == 0 {
            continue;
        }
        let fs_type = d.file_system().to_string_lossy().to_string();
        let mp = d.mount_point().display().to_string();
        if mp.starts_with("/nix/store") || mp.starts_with("/run/") || mp.starts_with("/sys") {
            continue;
        }
        let used = total.saturating_sub(d.available_space());
        mounts.push(MountVital {
            mount_point: mp,
            fs_type,
            used_gb: used as f32 / 1_073_741_824.0,
            total_gb: total as f32 / 1_073_741_824.0,
        });
    }
    mounts.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    mounts
}

async fn collect_gpu_vitals() -> Vec<GpuVital> {
    // 1) Try nvidia-smi for GPU stats + per-process compute apps
    let smi = tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,power.limit",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await;

    if let Ok(out) = smi
        && out.status.success()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let parsed = crate::telemetry::parse_nvidia_smi_csv(&stdout);
        let apps = collect_nvidia_compute_apps().await;
        if !parsed.is_empty() {
            return parsed
                .into_iter()
                .map(|g| GpuVital {
                    name: g.name,
                    utilization_percent: g.utilization_percent,
                    vram_used_mb: g.vram_used_mb,
                    vram_total_mb: g.vram_total_mb,
                    temperature_c: g.temperature_c,
                    power_draw_w: g.power_draw_w,
                    power_limit_w: g.power_limit_w,
                    source: g.source,
                    top_processes: apps.clone(),
                })
                .collect();
        }
    }

    // 2) Fallback: ComfyUI CUDA /system_stats (works even during NVML driver mismatch)
    let comfy_url =
        std::env::var("KAAGE_COMFYUI_URL").unwrap_or_else(|_| "http://127.0.0.1:8188".into());
    if let Ok(body) = crate::telemetry::http_get_local_pub(&comfy_url, "/system_stats").await
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
        && let Some(devices) = json.get("devices").and_then(serde_json::Value::as_array)
    {
        let mut gpus = Vec::new();
        for dev in devices {
            let raw_name = dev
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("CUDA GPU");
            let clean_name = raw_name
                .split(" : ")
                .next()
                .unwrap_or(raw_name)
                .trim_start_matches("cuda:0 ")
                .trim()
                .to_string();
            let total = dev
                .get("vram_total")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let free = dev
                .get("vram_free")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if total > 0 {
                gpus.push(GpuVital {
                    name: clean_name,
                    utilization_percent: None,
                    vram_used_mb: total.saturating_sub(free) / 1024 / 1024,
                    vram_total_mb: total / 1024 / 1024,
                    temperature_c: None,
                    power_draw_w: None,
                    power_limit_w: None,
                    source: "comfyui-cuda".into(),
                    top_processes: Vec::new(),
                });
            }
        }
        if !gpus.is_empty() {
            return gpus;
        }
    }

    // 3) Fallback: AMD / DRM sysfs
    let mut drm_gpus = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let dev = entry.path().join("device");
            let busy = fs::read_to_string(dev.join("gpu_busy_percent"))
                .ok()
                .and_then(|s| s.trim().parse::<f32>().ok());
            let vram_used = fs::read_to_string(dev.join("mem_info_vram_used"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|b| b / 1024 / 1024)
                .unwrap_or(0);
            let vram_total = fs::read_to_string(dev.join("mem_info_vram_total"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|b| b / 1024 / 1024)
                .unwrap_or(0);
            if busy.is_some() || vram_total > 0 {
                drm_gpus.push(GpuVital {
                    name: format!("GPU ({name})"),
                    utilization_percent: busy,
                    vram_used_mb: vram_used,
                    vram_total_mb: vram_total,
                    temperature_c: None,
                    power_draw_w: None,
                    power_limit_w: None,
                    source: "drm-sysfs".into(),
                    top_processes: Vec::new(),
                });
            }
        }
    }
    drm_gpus
}

async fn collect_nvidia_compute_apps() -> Vec<GpuProcessVital> {
    let Ok(out) = tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,process_name,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut apps = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if parts.len() < 3 {
            continue;
        }
        let Ok(pid) = parts[0].parse::<u32>() else {
            continue;
        };
        let name = PathBuf::from(parts[1])
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| parts[1].to_string());
        let vram_mb = parts[2].parse::<u64>().unwrap_or(0);
        apps.push(GpuProcessVital {
            pid,
            name,
            vram_mb,
            unit: read_pid_systemd_unit(pid),
        });
    }
    apps.sort_by(|a, b| b.vram_mb.cmp(&a.vram_mb));
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kernel_psi_format() {
        let sample = "some avg10=12.50 avg60=4.20 avg300=1.00 total=12345\nfull avg10=2.10 avg60=0.80 avg300=0.10 total=999\n";
        let psi = parse_psi_file(sample);
        assert!((psi.some_avg10 - 12.5).abs() < 0.01);
        assert!((psi.some_avg60 - 4.2).abs() < 0.01);
        assert!((psi.full_avg10 - 2.1).abs() < 0.01);
        assert!((psi.full_avg60 - 0.8).abs() < 0.01);
    }

    #[test]
    fn downsamples_ticks_preserving_cpu_max_and_window() {
        let ticks: Vec<VitalsTick> = (0..100)
            .map(|i| VitalsTick {
                ts: 1000 + i,
                cpu_percent: if i == 50 { 95.0 } else { 10.0 },
                mem_used_mb: 16_000,
                mem_total_mb: 64_000,
                ..Default::default()
            })
            .collect();
        let refs: Vec<&VitalsTick> = ticks.iter().collect();
        let buckets = downsample_ticks(&refs, 10);
        assert_eq!(buckets.len(), 10);
        assert_eq!(buckets[0].ts_start, 1000);
        assert_eq!(buckets[9].ts_end, 1099);
        assert!((buckets[5].cpu_max - 95.0).abs() < 0.01);
        assert!((buckets[0].mem_pct_avg - 25.0).abs() < 0.01);
    }
}
