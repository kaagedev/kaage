use kaage_proto::{
    AiJobInfo, ComfyUiStatus, DashboardSnapshot, GpuInfo, ListeningPortInfo, ProcessSummary,
    TailscaleInfo, TailscalePeerInfo, TailscaleServeEntry, UnslothStatus,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    time::Duration,
};
use sysinfo::System;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
};

/// Collect full host telemetry: CPU, per-core usage, memory/swap, top processes,
/// GPU metrics, ComfyUI jobs, Unsloth jobs, listening HTTP ports, and Tailscale status.
pub async fn collect_dashboard_snapshot() -> DashboardSnapshot {
    let comfy_url = std::env::var("KAAGE_COMFYUI_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8188".into());
    let unsloth_url = std::env::var("KAAGE_UNSLOTH_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8888".into());

    let (comfy_res, unsloth_health, tailscale, raw_ports, nvidia_smi_res) = tokio::join!(
        collect_comfyui(&comfy_url),
        probe_unsloth_health(&unsloth_url),
        collect_tailscale(),
        collect_raw_listening_ports(),
        collect_nvidia_smi(),
    );

    let (comfyui, comfy_gpus) = comfy_res;

    // Collect CPU, memory, swap, top processes, and Unsloth worker processes via sysinfo
    let mut sys = System::new_all();
    tokio::time::sleep(Duration::from_millis(120)).await;
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let cpu_percent = sys.global_cpu_usage();
    let cpu_cores: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
    let cpu_brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_default();
    let load = System::load_average();
    let load_avg = [load.one, load.five, load.fifteen];

    let mem_used_mb = sys.used_memory() / 1024 / 1024;
    let mem_total_mb = sys.total_memory() / 1024 / 1024;
    let swap_used_mb = sys.used_swap() / 1024 / 1024;
    let swap_total_mb = sys.total_swap() / 1024 / 1024;

    let mut procs: Vec<ProcessSummary> = sys
        .processes()
        .values()
        .filter(|p| p.thread_kind().is_none())
        .map(|p| ProcessSummary {
            pid: p.pid().as_u32(),
            name: p.name().to_string_lossy().into_owned(),
            cpu_percent: p.cpu_usage(),
            mem_mb: p.memory() / 1024 / 1024,
        })
        .collect();
    procs.sort_by(|a, b| b.mem_mb.cmp(&a.mem_mb).then_with(|| a.pid.cmp(&b.pid)));
    procs.truncate(8);

    // Unsloth active worker/training/inference processes
    let mut unsloth_jobs = Vec::new();
    for p in sys.processes().values().filter(|p| p.thread_kind().is_none()) {
        let name = p.name().to_string_lossy();
        let cmd_line: String = p
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let lower = cmd_line.to_ascii_lowercase();
        let is_unsloth = name.to_ascii_lowercase().contains("unsloth")
            || lower.contains("unsloth")
            || lower.contains("llama-server")
            || lower.contains("vllm");
        if is_unsloth {
            let mem_mb = p.memory() / 1024 / 1024;
            let cpu = p.cpu_usage();
            let role = if lower.contains("studio") {
                "studio server"
            } else if lower.contains("llama-server") || lower.contains("vllm") {
                "inference worker"
            } else if lower.contains("train") || lower.contains("sft") || lower.contains("grpo") {
                "training job"
            } else {
                "worker"
            };
            unsloth_jobs.push(AiJobInfo {
                id: format!("pid:{}", p.pid().as_u32()),
                title: format!("{} ({role})", name),
                state: if cpu > 5.0 {
                    "active".into()
                } else {
                    "idle".into()
                },
                detail: format!("CPU {:.1}% · RAM {} MiB", cpu, mem_mb),
            });
        }
    }
    unsloth_jobs.sort_by(|a, b| a.id.cmp(&b.id));

    let unsloth = UnslothStatus {
        reachable: unsloth_health.0,
        url: unsloth_url,
        service_name: unsloth_health.1,
        jobs: unsloth_jobs,
        error: unsloth_health.2,
    };

    // Assemble GPU list: prefer nvidia-smi, then ComfyUI CUDA stats, then /proc/driver/nvidia, then AMD sysfs
    let gpus = match nvidia_smi_res {
        Ok(list) if !list.is_empty() => list,
        Err(smi_err) => {
            if !comfy_gpus.is_empty() {
                comfy_gpus
                    .into_iter()
                    .map(|mut g| {
                        g.note = Some(format!("nvidia-smi unavailable ({smi_err}); VRAM via ComfyUI CUDA"));
                        g
                    })
                    .collect()
            } else {
                let mut fallback = collect_nvidia_proc_fallback(&smi_err);
                if fallback.is_empty() {
                    fallback = collect_amd_sysfs_gpus();
                }
                fallback
            }
        }
        Ok(_) => {
            if !comfy_gpus.is_empty() {
                comfy_gpus
            } else {
                collect_amd_sysfs_gpus()
            }
        }
    };

    // Enrich listening ports with Tailscale Serve HTTPS URLs and friendly labels
    let serve_by_port = build_serve_port_map(&tailscale.serves);
    let listening_ports = enrich_listening_ports(raw_ports, &serve_by_port);

    DashboardSnapshot {
        cpu_percent,
        cpu_cores,
        cpu_brand,
        load_avg,
        mem_used_mb,
        mem_total_mb,
        swap_used_mb,
        swap_total_mb,
        top_processes: procs,
        gpus,
        comfyui,
        unsloth,
        listening_ports,
        tailscale,
    }
}

pub async fn http_get_local_pub(base_url: &str, path: &str) -> Result<String, String> {
    http_get_local(base_url, path).await
}

async fn http_get_local(base_url: &str, path: &str) -> Result<String, String> {
    let stripped = base_url
        .trim()
        .strip_prefix("http://")
        .ok_or_else(|| format!("Unsupported URL scheme: {base_url}"))?;
    let host_port = stripped.split('/').next().unwrap_or(stripped);
    let mut stream = tokio::time::timeout(Duration::from_millis(1200), TcpStream::connect(host_port))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| e.to_string())?;

    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| e.to_string())?;

    let mut buf = Vec::with_capacity(4096);
    tokio::time::timeout(Duration::from_millis(1500), stream.read_to_end(&mut buf))
        .await
        .map_err(|_| "read timeout".to_string())?
        .map_err(|e| e.to_string())?;

    let raw = String::from_utf8_lossy(&buf);
    let Some((headers, body)) = raw.split_once("\r\n\r\n") else {
        return Err("malformed HTTP response".into());
    };
    let status_line = headers.lines().next().unwrap_or("");
    if !status_line.contains(" 200 ") {
        return Err(format!("HTTP status: {status_line}"));
    }

    // Handle chunked transfer encoding if present
    let is_chunked = headers
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));
    if is_chunked {
        Ok(decode_chunked_body(body))
    } else {
        Ok(body.to_string())
    }
}

fn decode_chunked_body(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size_line, after_size)) = rest.split_once("\r\n") {
        let Ok(len) = usize::from_str_radix(size_line.trim(), 16) else {
            break;
        };
        if len == 0 || after_size.len() < len {
            break;
        }
        out.push_str(&after_size[..len]);
        rest = after_size[len..].strip_prefix("\r\n").unwrap_or(&after_size[len..]);
    }
    out
}

async fn collect_comfyui(base_url: &str) -> (ComfyUiStatus, Vec<GpuInfo>) {
    let mut status = ComfyUiStatus {
        url: base_url.to_string(),
        ..Default::default()
    };
    let mut gpus = Vec::new();

    match http_get_local(base_url, "/queue").await {
        Ok(body) => {
            status.reachable = true;
            if let Ok(json) = serde_json::from_str::<Value>(&body) {
                status.running_jobs = parse_comfy_queue_items(json.get("queue_running"), "running");
                status.pending_jobs = parse_comfy_queue_items(json.get("queue_pending"), "pending");
            }
        }
        Err(err) => {
            status.error = Some(err);
            return (status, gpus);
        }
    }

    if let Ok(body) = http_get_local(base_url, "/system_stats").await
        && let Ok(json) = serde_json::from_str::<Value>(&body)
    {
        if let Some(ver) = json
            .get("system")
            .and_then(|s| s.get("comfyui_version"))
            .and_then(Value::as_str)
        {
            status.version = ver.to_string();
        }
        if let Some(devices) = json.get("devices").and_then(Value::as_array) {
            for dev in devices {
                let raw_name = dev
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("CUDA GPU");
                let clean_name = raw_name
                    .split(" : ")
                    .next()
                    .unwrap_or(raw_name)
                    .trim_start_matches("cuda:0 ")
                    .trim_start_matches("cuda:1 ")
                    .trim()
                    .to_string();
                let vram_total = dev.get("vram_total").and_then(Value::as_u64).unwrap_or(0);
                let vram_free = dev.get("vram_free").and_then(Value::as_u64).unwrap_or(0);
                let vram_used = vram_total.saturating_sub(vram_free);
                if vram_total > 0 {
                    gpus.push(GpuInfo {
                        name: clean_name,
                        utilization_percent: None,
                        vram_used_mb: vram_used / 1024 / 1024,
                        vram_total_mb: vram_total / 1024 / 1024,
                        temperature_c: None,
                        power_draw_w: None,
                        power_limit_w: None,
                        source: "comfyui-cuda".into(),
                        note: None,
                    });
                }
            }
        }
    }

    (status, gpus)
}

pub fn parse_comfy_queue_items(val: Option<&Value>, state: &str) -> Vec<AiJobInfo> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .enumerate()
        .map(|(idx, item)| {
            let tuple = item.as_array();
            let queue_num = tuple
                .and_then(|a| a.first())
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("#{}", idx + 1));
            let prompt_id = tuple
                .and_then(|a| a.get(1))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let short_id: String = prompt_id.chars().take(12).collect();
            let node_count = tuple
                .and_then(|a| a.get(2))
                .and_then(Value::as_object)
                .map(|m| m.len())
                .unwrap_or(0);
            AiJobInfo {
                id: short_id.clone(),
                title: format!("Prompt #{queue_num} ({short_id})"),
                state: state.to_string(),
                detail: if node_count > 0 {
                    format!("{node_count} graph nodes")
                } else {
                    "queued workflow".into()
                },
            }
        })
        .collect()
}

async fn probe_unsloth_health(base_url: &str) -> (bool, String, Option<String>) {
    match http_get_local(base_url, "/api/health").await {
        Ok(body) => {
            let service = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|j| j.get("service").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| "Unsloth Studio".into());
            (true, service, None)
        }
        Err(err) => (false, "Unsloth Studio".into(), Some(err)),
    }
}

async fn collect_nvidia_smi() -> Result<Vec<GpuInfo>, String> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,power.limit",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let reason = if !err.is_empty() { err } else { msg };
        return Err(reason.lines().next().unwrap_or("nvidia-smi failed").to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_nvidia_smi_csv(&stdout))
}

pub fn parse_nvidia_smi_csv(csv: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    for line in csv.lines() {
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if parts.len() < 4 {
            continue;
        }
        let name = parts[0].to_string();
        let utilization_percent = parts[1].parse::<f32>().ok();
        let vram_used_mb = parts[2].parse::<f64>().map(|v| v as u64).unwrap_or(0);
        let vram_total_mb = parts[3].parse::<f64>().map(|v| v as u64).unwrap_or(0);
        let temperature_c = parts.get(4).and_then(|s| s.parse::<f32>().ok());
        let power_draw_w = parts.get(5).and_then(|s| s.parse::<f32>().ok());
        let power_limit_w = parts.get(6).and_then(|s| s.parse::<f32>().ok());
        gpus.push(GpuInfo {
            name,
            utilization_percent,
            vram_used_mb,
            vram_total_mb,
            temperature_c,
            power_draw_w,
            power_limit_w,
            source: "nvidia-smi".into(),
            note: None,
        });
    }
    gpus
}

fn collect_nvidia_proc_fallback(smi_err: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    let Ok(entries) = fs::read_dir("/proc/driver/nvidia/gpus") else {
        return gpus;
    };
    for entry in entries.flatten() {
        let info_path = entry.path().join("information");
        if let Ok(content) = fs::read_to_string(&info_path) {
            let model = content
                .lines()
                .find_map(|l| l.strip_prefix("Model:"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "NVIDIA GPU".into());
            gpus.push(GpuInfo {
                name: model,
                utilization_percent: None,
                vram_used_mb: 0,
                vram_total_mb: 0,
                temperature_c: None,
                power_draw_w: None,
                power_limit_w: None,
                source: "proc-nvidia".into(),
                note: Some(smi_err.to_string()),
            });
        }
    }
    gpus
}

fn collect_amd_sysfs_gpus() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return gpus;
    };
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
            gpus.push(GpuInfo {
                name: format!("GPU ({name})"),
                utilization_percent: busy,
                vram_used_mb: vram_used,
                vram_total_mb: vram_total,
                temperature_c: None,
                power_draw_w: None,
                power_limit_w: None,
                source: "drm-sysfs".into(),
                note: None,
            });
        }
    }
    gpus
}

async fn collect_tailscale() -> TailscaleInfo {
    let mut info = TailscaleInfo::default();

    let status_out = match Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => out.stdout,
        Ok(out) => {
            info.error = Some(
                String::from_utf8_lossy(&out.stderr)
                    .trim()
                    .to_string(),
            );
            return info;
        }
        Err(err) => {
            info.error = Some(err.to_string());
            return info;
        }
    };

    if let Ok(json) = serde_json::from_slice::<Value>(&status_out) {
        info.available = true;
        info.version = json
            .get("Version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        info.backend_state = json
            .get("BackendState")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string();
        if let Some(ips) = json.get("TailscaleIPs").and_then(Value::as_array) {
            info.tailscale_ips = ips
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }
        if let Some(self_node) = json.get("Self") {
            info.hostname = self_node
                .get("HostName")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            info.dns_name = self_node
                .get("DNSName")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim_end_matches('.')
                .to_string();
            info.relay = self_node
                .get("Relay")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
        }
        if let Some(peers_obj) = json.get("Peer").and_then(Value::as_object) {
            let mut peers: Vec<TailscalePeerInfo> = peers_obj
                .values()
                .map(|p| TailscalePeerInfo {
                    hostname: p
                        .get("HostName")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    dns_name: p
                        .get("DNSName")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim_end_matches('.')
                        .to_string(),
                    os: p
                        .get("OS")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    tailscale_ip: p
                        .get("TailscaleIPs")
                        .and_then(Value::as_array)
                        .and_then(|a| a.first())
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    online: p.get("Online").and_then(Value::as_bool).unwrap_or(false),
                    active: p.get("Active").and_then(Value::as_bool).unwrap_or(false),
                    relay: p
                        .get("Relay")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    rx_bytes: p.get("RxBytes").and_then(Value::as_u64).unwrap_or(0),
                    tx_bytes: p.get("TxBytes").and_then(Value::as_u64).unwrap_or(0),
                })
                .collect();
            peers.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.hostname.cmp(&b.hostname)));
            info.peers = peers;
        }
    }

    if let Ok(serve_out) = Command::new("tailscale")
        .args(["serve", "status"])
        .output()
        .await
        && serve_out.status.success()
    {
        let text = String::from_utf8_lossy(&serve_out.stdout);
        info.serves = parse_tailscale_serve_status(&text);
    }

    info
}

pub async fn register_tailscale_serve(https_port: u16, local_port: u16) -> Result<String, String> {
    if https_port == 0 || local_port == 0 {
        return Err("Ports must be between 1 and 65535".into());
    }
    let https_arg = format!("--https={https_port}");
    let target = format!("http://127.0.0.1:{local_port}");
    let out = Command::new("tailscale")
        .args(["serve", "--bg", &https_arg, &target])
        .output()
        .await
        .map_err(|e| format!("Failed to invoke tailscale: {e}"))?;
    if out.status.success() {
        Ok(format!(
            "Exposed local :{local_port} on Tailscale HTTPS :{https_port}"
        ))
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Err(if !stderr.is_empty() { stderr } else { stdout })
    }
}

pub async fn unregister_tailscale_serve(https_port: u16) -> Result<String, String> {
    if https_port == 0 {
        return Err("HTTPS port must be between 1 and 65535".into());
    }
    let https_arg = format!("--https={https_port}");
    let out = Command::new("tailscale")
        .args(["serve", &https_arg, "off"])
        .output()
        .await
        .map_err(|e| format!("Failed to invoke tailscale: {e}"))?;
    if out.status.success() {
        Ok(format!("Unregistered Tailscale HTTPS :{https_port}"))
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Err(if !stderr.is_empty() { stderr } else { stdout })
    }
}

pub fn parse_tailscale_serve_status(text: &str) -> Vec<TailscaleServeEntry> {
    let mut entries = Vec::new();
    let mut current_url: Option<(String, bool)> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            let url = trimmed.split_whitespace().next().unwrap_or(trimmed);
            let funnel = trimmed.to_ascii_lowercase().contains("funnel on");
            current_url = Some((url.to_string(), funnel));
        } else if let Some(idx) = trimmed.find("proxy ") {
            let target = trimmed[idx + 6..].trim().to_string();
            let prefix = trimmed[..idx].trim();
            let mount_path = prefix
                .trim_start_matches("|--")
                .trim_start_matches("`--")
                .trim()
                .to_string();
            let mount_path = if mount_path.is_empty() {
                "/".to_string()
            } else {
                mount_path
            };
            if let Some((ref url, funnel)) = current_url {
                entries.push(TailscaleServeEntry {
                    https_url: url.clone(),
                    target,
                    mount_path,
                    funnel,
                });
            }
        }
    }
    entries
}

fn build_serve_port_map(serves: &[TailscaleServeEntry]) -> HashMap<u16, String> {
    let mut map = HashMap::new();
    for s in serves {
        if let Some(port_str) = s.target.rsplit(':').next()
            && let Ok(port) = port_str.trim_end_matches('/').parse::<u16>()
        {
            map.insert(port, s.https_url.clone());
        }
    }
    map
}

#[derive(Debug, Clone)]
struct RawPort {
    port: u16,
    bind_addr: String,
    process: String,
    pid: Option<u32>,
}

async fn collect_raw_listening_ports() -> Vec<RawPort> {
    let Ok(output) = Command::new("ss").args(["-tlnpH"]).output().await else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_ss_tlnp(&text)
}

fn parse_ss_tlnp(text: &str) -> Vec<RawPort> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            continue;
        }
        let local = cols[3];
        let Some((addr, port_str)) = local.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port_str.parse::<u16>() else {
            continue;
        };
        let proc_col = if cols.len() >= 6 {
            cols[5..].join(" ")
        } else {
            String::new()
        };
        let (process, pid) = parse_ss_process_col(&proc_col);
        out.push(RawPort {
            port,
            bind_addr: addr.to_string(),
            process,
            pid,
        });
    }
    out
}

fn parse_ss_process_col(col: &str) -> (String, Option<u32>) {
    // Format: users:(("kaaged",pid=294071,fd=11))
    let name = col
        .split('"')
        .nth(1)
        .unwrap_or("")
        .to_string();
    let pid = col
        .split("pid=")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse::<u32>().ok());
    (name, pid)
}

fn friendly_port_label(port: u16, process: &str) -> String {
    match (port, process) {
        (9090, _) => "kaage Webtop".into(),
        (8188, _) => "ComfyUI".into(),
        (8189, _) => "ComfyUI MCP".into(),
        (8888, _) => "Unsloth Studio".into(),
        (8765, _) => "aceDoP Studio".into(),
        (631, _) => "CUPS Printing".into(),
        (22, _) => "SSH".into(),
        (_, p) if !p.is_empty() => p.to_string(),
        _ => format!("TCP :{port}"),
    }
}

fn enrich_listening_ports(
    raw: Vec<RawPort>,
    serve_by_port: &HashMap<u16, String>,
) -> Vec<ListeningPortInfo> {
    // Deduplicate by port, preferring entries that have a process name & loopback/0.0.0.0 bind
    let mut by_port: BTreeMap<u16, ListeningPortInfo> = BTreeMap::new();

    for r in raw {
        // Skip ephemeral high ports (> 30000) unless exposed via Tailscale Serve
        if r.port >= 30000 && !serve_by_port.contains_key(&r.port) {
            continue;
        }
        let ts_url = serve_by_port.get(&r.port).cloned();
        let entry = by_port.entry(r.port).or_insert_with(|| ListeningPortInfo {
            port: r.port,
            bind_addr: r.bind_addr.clone(),
            process: r.process.clone(),
            pid: r.pid,
            label: friendly_port_label(r.port, &r.process),
            tailscale_url: ts_url.clone(),
        });

        // Upgrade empty process info if another socket on the same port has it
        if entry.process.is_empty() && !r.process.is_empty() {
            entry.process = r.process.clone();
            entry.pid = r.pid;
            entry.bind_addr = r.bind_addr;
            entry.label = friendly_port_label(r.port, &r.process);
        }
    }

    by_port.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi_csv_output() {
        let csv = "NVIDIA GeForce RTX 5090, 42, 12345, 32607, 61, 210.5, 575.0\n";
        let gpus = parse_nvidia_smi_csv(csv);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 5090");
        assert_eq!(gpus[0].utilization_percent, Some(42.0));
        assert_eq!(gpus[0].vram_used_mb, 12345);
        assert_eq!(gpus[0].vram_total_mb, 32607);
        assert_eq!(gpus[0].temperature_c, Some(61.0));
        assert_eq!(gpus[0].power_draw_w, Some(210.5));
    }

    #[test]
    fn parses_tailscale_serve_status_output() {
        let text = r#"
https://aihole-1.greyhound-goby.ts.net:8188 (tailnet only)
|-- / proxy http://127.0.0.1:8188

https://aihole-1.greyhound-goby.ts.net:9090 (tailnet only)
|-- / proxy http://127.0.0.1:9090
"#;
        let entries = parse_tailscale_serve_status(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].https_url,
            "https://aihole-1.greyhound-goby.ts.net:8188"
        );
        assert_eq!(entries[0].target, "http://127.0.0.1:8188");
        assert_eq!(entries[0].mount_path, "/");
        assert!(!entries[0].funnel);
        let port_map = build_serve_port_map(&entries);
        assert_eq!(
            port_map.get(&9090).map(String::as_str),
            Some("https://aihole-1.greyhound-goby.ts.net:9090")
        );
    }

    #[test]
    fn parses_comfy_queue_json() {
        let json: Value = serde_json::json!({
            "queue_running": [
                [12, "abcdef1234567890", {"1": {}, "2": {}, "3": {}}]
            ],
            "queue_pending": []
        });
        let running = parse_comfy_queue_items(json.get("queue_running"), "running");
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, "abcdef123456");
        assert_eq!(running[0].state, "running");
        assert_eq!(running[0].detail, "3 graph nodes");
    }

    #[test]
    fn parses_ss_tlnp_and_enriches_ports() {
        let ss = r#"LISTEN 0 128 127.0.0.1:8188 0.0.0.0:* users:(("python",pid=237156,fd=46))
LISTEN 0 4096 100.101.214.127%tailscale0:8188 0.0.0.0:*
LISTEN 0 128 127.0.0.1:9090 0.0.0.0:* users:(("kaaged",pid=294071,fd=11))
LISTEN 0 4096 127.0.0.1:39655 0.0.0.0:* users:(("agy",pid=32158,fd=12))
"#;
        let raw = parse_ss_tlnp(ss);
        let mut serves = HashMap::new();
        serves.insert(8188, "https://aihole.ts.net:8188".into());
        let enriched = enrich_listening_ports(raw, &serves);
        assert_eq!(enriched.len(), 2);
        assert_eq!(enriched[0].port, 8188);
        assert_eq!(enriched[0].label, "ComfyUI");
        assert_eq!(
            enriched[0].tailscale_url.as_deref(),
            Some("https://aihole.ts.net:8188")
        );
        assert_eq!(enriched[1].port, 9090);
        assert_eq!(enriched[1].label, "kaage Webtop");
    }
}
