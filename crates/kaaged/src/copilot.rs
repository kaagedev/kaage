use kaage_proto::{
    CopilotCard, CopilotProvider, CopilotStatus, NixosInfo, ServerMsg, TrustMode, VitalsQuery,
    VitalsWindow,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

const MAX_TURNS: usize = 8;
const GEMINI_ENDPOINT: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions";
const OPENAI_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

pub struct CopilotConfig {
    pub provider: CopilotProvider,
    pub model: String,
    pub trust_mode: TrustMode,
    pub target_node: String,
    pub flake_path: String,
}

pub struct MemoryStore {
    conn: Mutex<Connection>,
}

impl MemoryStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let conn = Connection::open(path).map_err(|e| format!("SQLite open failed: {e}"))?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                category,
                content,
                created_at UNINDEXED
            );",
        )
        .map_err(|e| format!("FTS5 init failed: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn save(&self, category: &str, content: &str) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO memories_fts (category, content, created_at) VALUES (?1, ?2, ?3)",
            params![category.trim(), content.trim(), now.to_string()],
        )
        .map_err(|e| format!("Memory save failed: {e}"))?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<(String, String)> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tokens: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|t| t.len() >= 2)
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect();

        let mut results = Vec::new();
        if !tokens.is_empty() {
            let fts_query = tokens.join(" OR ");
            if let Ok(mut stmt) = conn.prepare(
                "SELECT category, content FROM memories_fts WHERE memories_fts MATCH ?1 ORDER BY rank LIMIT ?2",
            ) && let Ok(rows) = stmt.query_map(params![fts_query, limit as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }) {
                for row in rows.flatten() {
                    results.push(row);
                }
            }
        }

        if results.is_empty()
            && let Ok(mut stmt) = conn.prepare(
                "SELECT category, content FROM memories_fts ORDER BY rowid DESC LIMIT ?1",
            )
            && let Ok(rows) = stmt.query_map(params![limit as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
        {
            for row in rows.flatten() {
                results.push(row);
            }
        }
        results
    }
}

pub struct CopilotManager {
    config: RwLock<CopilotConfig>,
    local_hostname: String,
    memory: Option<Arc<MemoryStore>>,
    pending_confirms: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    active_tasks: Mutex<HashMap<u64, JoinHandle<()>>>,
    next_action_seq: AtomicU64,
}

impl CopilotManager {
    pub fn new(local_hostname: String) -> Arc<Self> {
        let provider = match std::env::var("KAAGE_COPILOT_PROVIDER")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "openai" => CopilotProvider::OpenAi,
            _ => CopilotProvider::Gemini,
        };
        let model = std::env::var("KAAGE_COPILOT_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| provider.default_model().to_string());
        let trust_mode = match std::env::var("KAAGE_COPILOT_TRUST_MODE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "monitored" | "readonly" | "read-only" => TrustMode::Monitored,
            _ => TrustMode::Managed,
        };
        let flake_path = std::env::var("KAAGE_COPILOT_FLAKE_PATH")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::appdata::expand_user_path("~/dotfiles").display().to_string());

        let db_path = crate::appdata::kaage_app_dir().join("memory.db");
        let memory = match MemoryStore::open(&db_path) {
            Ok(store) => {
                tracing::info!("Copilot FTS5 memory initialized at {}", db_path.display());
                Some(Arc::new(store))
            }
            Err(err) => {
                tracing::warn!("Copilot FTS5 memory disabled: {err}");
                None
            }
        };

        Arc::new(Self {
            config: RwLock::new(CopilotConfig {
                provider,
                model,
                trust_mode,
                target_node: local_hostname.clone(),
                flake_path,
            }),
            local_hostname,
            memory,
            pending_confirms: Mutex::new(HashMap::new()),
            active_tasks: Mutex::new(HashMap::new()),
            next_action_seq: AtomicU64::new(1),
        })
    }

    pub fn status(&self) -> CopilotStatus {
        let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
        let has_gemini = resolve_api_key(CopilotProvider::Gemini).is_some();
        let has_openai = resolve_api_key(CopilotProvider::OpenAi).is_some();
        let ready = match cfg.provider {
            CopilotProvider::Gemini => has_gemini,
            CopilotProvider::OpenAi => has_openai,
        };
        CopilotStatus {
            provider: cfg.provider,
            model: cfg.model.clone(),
            trust_mode: cfg.trust_mode,
            target_node: cfg.target_node.clone(),
            ready,
            has_gemini_key: has_gemini,
            has_openai_key: has_openai,
            flake_path: cfg.flake_path.clone(),
        }
    }

    pub fn update_config(
        &self,
        provider: Option<CopilotProvider>,
        model: Option<String>,
        trust_mode: Option<TrustMode>,
        target_node: Option<String>,
    ) -> CopilotStatus {
        {
            let mut cfg = self.config.write().unwrap_or_else(|e| e.into_inner());
            if let Some(p) = provider {
                let provider_changed = cfg.provider != p;
                cfg.provider = p;
                if provider_changed && model.is_none() {
                    cfg.model = p.default_model().to_string();
                }
            }
            if let Some(m) = model
                && !m.trim().is_empty()
            {
                cfg.model = m.trim().to_string();
            }
            if let Some(t) = trust_mode {
                cfg.trust_mode = t;
            }
            if let Some(n) = target_node
                && !n.trim().is_empty()
            {
                cfg.target_node = n.trim().to_string();
            }
        }
        self.status()
    }

    pub fn resolve_confirmation(&self, action_id: &str, approved: bool) -> bool {
        let tx = self
            .pending_confirms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(action_id);
        if let Some(tx) = tx {
            let _ = tx.send(approved);
            true
        } else {
            false
        }
    }

    pub fn cancel_request(&self, request_id: u64) {
        if let Some(handle) = self
            .active_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&request_id)
        {
            handle.abort();
        }
    }

    pub fn spawn_ask(
        self: &Arc<Self>,
        request_id: u64,
        prompt: String,
        nixos: NixosInfo,
        vitals: Arc<crate::vitals::VitalsCollector>,
        out_tx: mpsc::UnboundedSender<ServerMsg>,
    ) {
        let mgr = Arc::clone(self);
        let mgr_cleanup = Arc::clone(self);
        let handle = tokio::spawn(async move {
            mgr.run_conversation(request_id, prompt, nixos, vitals, out_tx.clone())
                .await;
            mgr_cleanup
                .active_tasks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&request_id);
        });
        self.active_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request_id, handle);
    }

    async fn run_conversation(
        &self,
        request_id: u64,
        prompt: String,
        nixos: NixosInfo,
        vitals: Arc<crate::vitals::VitalsCollector>,
        out_tx: mpsc::UnboundedSender<ServerMsg>,
    ) {
        let status = self.status();
        let Some(api_key) = resolve_api_key(status.provider) else {
            let env_name = match status.provider {
                CopilotProvider::Gemini => "GEMINI_API_KEY",
                CopilotProvider::OpenAi => "OPENAI_API_KEY",
            };
            let _ = out_tx.send(ServerMsg::CopilotChunk {
                request_id,
                delta: format!(
                    "Copilot is not configured for `{}`: set `{env_name}` (or configure `services.kaage.copilot` key file) and restart `kaaged`.",
                    status.provider.as_str()
                ),
                done: true,
            });
            return;
        };

        let mut recalled_block = String::new();
        if let Some(ref mem) = self.memory {
            let hits = mem.search(&prompt, 5);
            if !hits.is_empty() {
                recalled_block.push_str("\nRecalled host memories:\n");
                for (cat, txt) in &hits {
                    recalled_block.push_str(&format!("- [{cat}] {txt}\n"));
                    let _ = out_tx.send(ServerMsg::CopilotCard {
                        request_id,
                        card: CopilotCard::MemoryNote {
                            saved: false,
                            category: cat.clone(),
                            summary: txt.clone(),
                        },
                    });
                }
            }
        }

        let system_prompt = format!(
            "You are Kaage Copilot, an expert Linux & NixOS operations assistant embedded inside kaage on host `{}`.\n\
             Active OS: {} (NixOS: {}, generation: {:?}, current-system: {:?}).\n\
             Configured flake path: {}.\n\
             Target node: {} · Trust mode: {}.\n\
             Use your diagnostic tools to inspect real host state before answering. Be concise, accurate, and cite exact unit names, metrics, and file paths.{recalled_block}",
            self.local_hostname,
            nixos.pretty_name,
            nixos.is_nixos,
            nixos.generation,
            nixos.current_system,
            status.flake_path,
            status.target_node,
            status.trust_mode.as_str(),
        );

        let mut messages = vec![
            json!({ "role": "system", "content": system_prompt }),
            json!({ "role": "user", "content": prompt }),
        ];

        let endpoint = match status.provider {
            CopilotProvider::Gemini => GEMINI_ENDPOINT,
            CopilotProvider::OpenAi => OPENAI_ENDPOINT,
        };
        let tools = tool_schemas();

        for _turn in 0..MAX_TURNS {
            match stream_openai_compat(
                endpoint,
                &api_key,
                &status.model,
                &messages,
                &tools,
                request_id,
                &out_tx,
            )
            .await
            {
                Ok(turn_res) => {
                    if turn_res.tool_calls.is_empty() {
                        let _ = out_tx.send(ServerMsg::CopilotChunk {
                            request_id,
                            delta: String::new(),
                            done: true,
                        });
                        return;
                    }

                    // Append assistant message with tool_calls
                    let tc_json: Vec<Value> = turn_res
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments
                                }
                            })
                        })
                        .collect();
                    messages.push(json!({
                        "role": "assistant",
                        "content": if turn_res.content.is_empty() { Value::Null } else { Value::String(turn_res.content) },
                        "tool_calls": tc_json
                    }));

                    for tc in turn_res.tool_calls {
                        let args: Value =
                            serde_json::from_str(&tc.arguments).unwrap_or_else(|_| json!({}));
                        let target_node = args
                            .get("node")
                            .and_then(Value::as_str)
                            .filter(|s| !s.trim().is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| status.target_node.clone());

                        let _ = out_tx.send(ServerMsg::CopilotToolStatus {
                            request_id,
                            tool_name: tc.name.clone(),
                            target_node: target_node.clone(),
                            state: "running".into(),
                        });

                        let result = self
                            .execute_tool(
                                request_id,
                                &tc.name,
                                &args,
                                &target_node,
                                status.trust_mode,
                                &status.flake_path,
                                &vitals,
                                &out_tx,
                            )
                            .await;

                        let _ = out_tx.send(ServerMsg::CopilotToolStatus {
                            request_id,
                            tool_name: tc.name.clone(),
                            target_node,
                            state: "done".into(),
                        });

                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tc.id,
                            "content": result
                        }));
                    }
                }
                Err(err) => {
                    let _ = out_tx.send(ServerMsg::CopilotChunk {
                        request_id,
                        delta: format!("\n[Copilot API error: {err}]"),
                        done: true,
                    });
                    return;
                }
            }
        }

        let _ = out_tx.send(ServerMsg::CopilotChunk {
            request_id,
            delta: "\n[Reached max tool turns (8)]".into(),
            done: true,
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_tool(
        &self,
        request_id: u64,
        name: &str,
        args: &Value,
        target_node: &str,
        trust_mode: TrustMode,
        flake_path: &str,
        vitals: &Arc<crate::vitals::VitalsCollector>,
        out_tx: &mpsc::UnboundedSender<ServerMsg>,
    ) -> String {
        let is_remote = !target_node.is_empty() && target_node != self.local_hostname;

        match name {
            // --- Tier 1: Read-Only Tools (Auto-Approved) ---
            "get_system_vitals" => {
                if is_remote {
                    return run_remote_shell(target_node, "uptime && free -m && cat /proc/pressure/cpu /proc/pressure/memory 2>/dev/null || true").await;
                }
                let snap = vitals.query_snapshot(&VitalsQuery {
                    window: VitalsWindow::Min5,
                    buckets: 36,
                    start_ts: None,
                    end_ts: None,
                });
                let points: Vec<f32> = snap.buckets.iter().map(|b| b.cpu_avg).collect();
                let start_ts = snap.buckets.first().map(|b| b.ts_start).unwrap_or(0);
                let end_ts = snap.buckets.last().map(|b| b.ts_end).unwrap_or(0);
                let mut procs = snap.processes.clone();
                procs.sort_by(|a, b| b.mem_mb.cmp(&a.mem_mb));
                let top_consumers: Vec<(String, String)> = procs
                    .into_iter()
                    .take(5)
                    .map(|p| {
                        let label = p.unit.unwrap_or(p.name);
                        (label, format!("{} MiB · {:.1}% CPU", p.mem_mb, p.cpu_percent))
                    })
                    .collect();

                let _ = out_tx.send(ServerMsg::CopilotCard {
                    request_id,
                    card: CopilotCard::VitalsSlice {
                        start_ts,
                        end_ts,
                        metric_label: format!(
                            "CPU {:.1}% · RAM {}/{} MiB",
                            snap.latest.cpu_percent, snap.latest.mem_used_mb, snap.latest.mem_total_mb
                        ),
                        points,
                        top_consumers: top_consumers.clone(),
                    },
                });

                json!({
                    "cpu_percent": snap.latest.cpu_percent,
                    "mem_used_mb": snap.latest.mem_used_mb,
                    "mem_total_mb": snap.latest.mem_total_mb,
                    "gpu_util": snap.latest.gpu_util_percent,
                    "gpu_vram_used_mb": snap.latest.gpu_vram_used_mb,
                    "psi_cpu_some10": snap.psi.cpu.some_avg10,
                    "psi_mem_some10": snap.psi.memory.some_avg10,
                    "psi_io_some10": snap.psi.io.some_avg10,
                    "top_consumers": top_consumers,
                })
                .to_string()
            }

            "inspect_systemd_unit" => {
                let unit = args.get("unit").and_then(Value::as_str).unwrap_or("");
                let scope = args.get("scope").and_then(Value::as_str).unwrap_or("system");
                let normalized = crate::nixos::normalize_service_name(unit);
                let user_flag = if scope == "user" { "--user " } else { "" };
                let cmd = format!(
                    "systemctl {user_flag}show '{normalized}' -p ActiveState -p SubState -p NRestarts -p MemoryCurrent --no-pager; \
                     echo '---LOGS---'; \
                     journalctl {user_flag}-u '{normalized}' -p err..warning -n 30 --no-pager --output=short-iso 2>/dev/null || true"
                );
                let raw = if is_remote {
                    run_remote_shell(target_node, &cmd).await
                } else {
                    run_local_shell(&cmd).await
                };
                let redacted = redact_secrets(&raw);
                let (meta_part, logs_part) = redacted.split_once("---LOGS---").unwrap_or((&redacted, ""));
                let mut active = "unknown".to_string();
                let mut sub = "unknown".to_string();
                let mut summary_bits = Vec::new();
                for line in meta_part.lines() {
                    if let Some(v) = line.strip_prefix("ActiveState=") {
                        active = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("SubState=") {
                        sub = v.trim().to_string();
                    } else if let Some(v) = line.strip_prefix("NRestarts=") {
                        summary_bits.push(format!("restarts: {}", v.trim()));
                    } else if let Some(v) = line.strip_prefix("MemoryCurrent=")
                        && let Ok(bytes) = v.trim().parse::<u64>()
                        && bytes < u64::MAX / 2
                    {
                        summary_bits.push(format!("RSS: {:.1} MiB", bytes as f64 / 1048576.0));
                    }
                }
                let error_lines: Vec<String> = logs_part
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with("-- "))
                    .take(8)
                    .map(str::to_string)
                    .collect();

                let _ = out_tx.send(ServerMsg::CopilotCard {
                    request_id,
                    card: CopilotCard::UnitCard {
                        unit: normalized.clone(),
                        active_state: active.clone(),
                        sub_state: sub.clone(),
                        summary: summary_bits.join(" · "),
                        error_lines: error_lines.clone(),
                    },
                });

                redacted
            }

            "list_containers" => {
                let cmd = "docker ps -a --format '{{.Names}} | {{.Image}} | {{.Status}} | {{.Ports}}' 2>/dev/null || podman ps -a --format '{{.Names}} | {{.Image}} | {{.Status}} | {{.Ports}}' 2>/dev/null || echo 'No container runtime available'";
                if is_remote {
                    run_remote_shell(target_node, cmd).await
                } else {
                    run_local_shell(cmd).await
                }
            }

            "eval_nix_option" => {
                let option_path = args.get("option_path").and_then(Value::as_str).unwrap_or("");
                let host = args
                    .get("host")
                    .and_then(Value::as_str)
                    .unwrap_or(target_node);
                if option_path.is_empty() || option_path.contains([';', '&', '|', '`', '$']) {
                    return "Invalid option_path".into();
                }
                let cmd = format!(
                    "nix eval --json '{flake_path}#nixosConfigurations.{host}.config.{option_path}' 2>&1"
                );
                let out = if is_remote {
                    run_remote_shell(target_node, &cmd).await
                } else {
                    run_local_shell(&cmd).await
                };
                redact_secrets(&out)
            }

            "diff_nix_generations" => {
                let cmd = "if command -v nvd >/dev/null 2>&1; then \
                    PREV=$(ls -1dv /nix/var/nix/profiles/system-*-link 2>/dev/null | tail -n 2 | head -n 1); \
                    if [ -n \"$PREV\" ]; then nvd diff \"$PREV\" /run/current-system; else echo 'Only one generation'; fi; \
                    else ls -1dv /nix/var/nix/profiles/system-*-link 2>/dev/null | tail -n 5; fi";
                if is_remote {
                    run_remote_shell(target_node, cmd).await
                } else {
                    run_local_shell(cmd).await
                }
            }

            "read_config_file" => {
                let path_str = args.get("path").and_then(Value::as_str).unwrap_or("");
                let expanded = crate::appdata::expand_user_path(path_str);
                if !is_path_in_allowed_roots(&expanded, flake_path) {
                    return format!(
                        "Access denied: {} is outside allowed config roots ({flake_path}, /etc/nixos, /etc/systemd/system)",
                        expanded.display()
                    );
                }
                match fs::read_to_string(&expanded) {
                    Ok(content) => {
                        let truncated: String = content.chars().take(12_000).collect();
                        redact_secrets(&truncated)
                    }
                    Err(e) => format!("Failed to read {}: {e}", expanded.display()),
                }
            }

            "save_host_memory" => {
                let category = args.get("category").and_then(Value::as_str).unwrap_or("system");
                let content = args.get("content").and_then(Value::as_str).unwrap_or("");
                if let Some(ref mem) = self.memory {
                    match mem.save(category, content) {
                        Ok(()) => {
                            let _ = out_tx.send(ServerMsg::CopilotCard {
                                request_id,
                                card: CopilotCard::MemoryNote {
                                    saved: true,
                                    category: category.to_string(),
                                    summary: content.to_string(),
                                },
                            });
                            "Saved to host memory.".into()
                        }
                        Err(e) => e,
                    }
                } else {
                    "Memory store unavailable.".into()
                }
            }

            "search_host_memory" => {
                let query = args.get("query").and_then(Value::as_str).unwrap_or("");
                if let Some(ref mem) = self.memory {
                    let hits = mem.search(query, 8);
                    json!(hits).to_string()
                } else {
                    "[]".into()
                }
            }

            // --- Tier 2: Guarded Mutation Tools (Requires Managed Mode + User Approval) ---
            "control_unit_or_container" => {
                if trust_mode == TrustMode::Monitored {
                    return "Blocked: Copilot is in Monitored (read-only) mode. Switch to Managed mode with `/mode manage` to perform mutations.".into();
                }
                let kind = args.get("kind").and_then(Value::as_str).unwrap_or("unit");
                let target = args.get("target").and_then(Value::as_str).unwrap_or("");
                let action = args.get("action").and_then(Value::as_str).unwrap_or("restart");
                let scope = args.get("scope").and_then(Value::as_str).unwrap_or("system");
                let user_flag = if scope == "user" { "--user " } else { "" };

                let cmd = if kind == "container" {
                    format!("docker {action} '{target}'")
                } else {
                    let norm = crate::nixos::normalize_service_name(target);
                    format!("systemctl {user_flag}{action} '{norm}'")
                };
                let rollback = if action == "stop" {
                    cmd.replacen("stop", "start", 1)
                } else {
                    format!("systemctl {user_flag}status '{target}'")
                };

                let approved = self
                    .request_approval(
                        request_id,
                        target_node,
                        format!("{action} {kind} `{target}`"),
                        cmd.clone(),
                        rollback,
                        out_tx,
                    )
                    .await;
                if !approved {
                    return "Action rejected by user.".into();
                }
                if is_remote {
                    run_remote_shell(target_node, &cmd).await
                } else {
                    run_local_shell(&cmd).await
                }
            }

            "propose_nix_edit" => {
                if trust_mode == TrustMode::Monitored {
                    return "Blocked: Copilot is in Monitored (read-only) mode.".into();
                }
                let path_str = args.get("file_path").and_then(Value::as_str).unwrap_or("");
                let new_content = args.get("new_content").and_then(Value::as_str).unwrap_or("");
                let summary = args
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("Update NixOS configuration");
                let expanded = crate::appdata::expand_user_path(path_str);
                if !is_path_in_allowed_roots(&expanded, flake_path) {
                    return format!("Blocked: {} is outside allowed config roots.", expanded.display());
                }
                let old_content = fs::read_to_string(&expanded).unwrap_or_default();
                let diff = simple_unified_diff(&old_content, new_content, path_str);

                let _ = out_tx.send(ServerMsg::CopilotCard {
                    request_id,
                    card: CopilotCard::NixDiff {
                        file_path: expanded.display().to_string(),
                        diff: diff.clone(),
                        dry_build_ok: None,
                        summary: summary.to_string(),
                    },
                });

                let approved = self
                    .request_approval(
                        request_id,
                        target_node,
                        format!("Write {}", expanded.display()),
                        diff,
                        format!("git -C '{flake_path}' checkout -- '{}'", expanded.display()),
                        out_tx,
                    )
                    .await;
                if !approved {
                    return "Edit rejected by user.".into();
                }
                match fs::write(&expanded, new_content) {
                    Ok(()) => format!("Successfully wrote {}. Run `nixos-rebuild switch` to activate.", expanded.display()),
                    Err(e) => format!("Failed to write {}: {e}", expanded.display()),
                }
            }

            "run_diagnostic_command" => {
                if trust_mode == TrustMode::Monitored {
                    return "Blocked: Copilot is in Monitored (read-only) mode.".into();
                }
                let cmd = args.get("command").and_then(Value::as_str).unwrap_or("");
                let summary = args
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("Run administrative command");
                let approved = self
                    .request_approval(
                        request_id,
                        target_node,
                        summary.to_string(),
                        cmd.to_string(),
                        "N/A (inspect output)".into(),
                        out_tx,
                    )
                    .await;
                if !approved {
                    return "Command rejected by user.".into();
                }
                let out = if is_remote {
                    run_remote_shell(target_node, cmd).await
                } else {
                    run_local_shell(cmd).await
                };
                redact_secrets(&out)
            }

            _ => format!("Unknown tool: {name}"),
        }
    }

    async fn request_approval(
        &self,
        request_id: u64,
        target_node: &str,
        summary: String,
        preview: String,
        rollback_cmd: String,
        out_tx: &mpsc::UnboundedSender<ServerMsg>,
    ) -> bool {
        let seq = self.next_action_seq.fetch_add(1, Ordering::Relaxed);
        let action_id = format!("act-{request_id}-{seq}");
        let (tx, rx) = oneshot::channel();
        self.pending_confirms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(action_id.clone(), tx);

        let _ = out_tx.send(ServerMsg::CopilotConfirmRequest {
            request_id,
            gate: CopilotCard::ConfirmGate {
                action_id: action_id.clone(),
                target_node: target_node.to_string(),
                summary,
                preview,
                rollback_cmd,
            },
        });

        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending_confirms
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&action_id);
                false
            }
        }
    }
}

struct ToolCallAcc {
    id: String,
    name: String,
    arguments: String,
}

struct TurnResult {
    content: String,
    tool_calls: Vec<ToolCallAcc>,
}

async fn stream_openai_compat(
    endpoint: &str,
    api_key: &str,
    model: &str,
    messages: &[Value],
    tools: &Value,
    request_id: u64,
    out_tx: &mpsc::UnboundedSender<ServerMsg>,
) -> Result<TurnResult, String> {
    let payload = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "stream": true
    });
    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;

    let mut child = Command::new("curl")
        .args([
            "-sSN",
            "--no-buffer",
            "-X",
            "POST",
            endpoint,
            "-H",
            "Content-Type: application/json",
            "-H",
            &format!("Authorization: Bearer {api_key}"),
            "--data-binary",
            "@-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn curl: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| format!("Write request body failed: {e}"))?;
    }

    let stdout = child.stdout.take().ok_or("Missing curl stdout")?;
    let mut lines = BufReader::new(stdout).lines();

    let mut full_content = String::new();
    let mut tool_map: HashMap<usize, ToolCallAcc> = HashMap::new();
    let mut raw_error = String::new();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(data) = trimmed.strip_prefix("data:") {
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            if let Ok(val) = serde_json::from_str::<Value>(data) {
                if let Some(err) = val.get("error") {
                    return Err(err.to_string());
                }
                if let Some(delta) = val
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                {
                    if let Some(text) = delta.get("content").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        full_content.push_str(text);
                        let _ = out_tx.send(ServerMsg::CopilotChunk {
                            request_id,
                            delta: text.to_string(),
                            done: false,
                        });
                    }
                    if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                        for tc in tcs {
                            let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                            let entry = tool_map.entry(idx).or_insert_with(|| ToolCallAcc {
                                id: String::new(),
                                name: String::new(),
                                arguments: String::new(),
                            });
                            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                                entry.id.push_str(id);
                            }
                            if let Some(func) = tc.get("function") {
                                if let Some(n) = func.get("name").and_then(Value::as_str) {
                                    entry.name.push_str(n);
                                }
                                if let Some(a) = func.get("arguments").and_then(Value::as_str) {
                                    entry.arguments.push_str(a);
                                }
                            }
                        }
                    }
                }
            }
        } else if raw_error.len() < 1024 {
            raw_error.push_str(trimmed);
            raw_error.push(' ');
        }
    }

    let _ = child.wait().await;

    if full_content.is_empty() && tool_map.is_empty() && !raw_error.is_empty() {
        return Err(raw_error.trim().to_string());
    }

    let mut ordered: Vec<(usize, ToolCallAcc)> = tool_map.into_iter().collect();
    ordered.sort_by_key(|(idx, _)| *idx);
    let tool_calls = ordered.into_iter().map(|(_, acc)| acc).collect();

    Ok(TurnResult {
        content: full_content,
        tool_calls,
    })
}

fn resolve_api_key(provider: CopilotProvider) -> Option<String> {
    let (env_var, file_var, cred_name) = match provider {
        CopilotProvider::Gemini => (
            "GEMINI_API_KEY",
            "KAAGE_COPILOT_GEMINI_KEY_FILE",
            "gemini_api_key",
        ),
        CopilotProvider::OpenAi => (
            "OPENAI_API_KEY",
            "KAAGE_COPILOT_OPENAI_KEY_FILE",
            "openai_api_key",
        ),
    };
    if let Ok(val) = std::env::var(env_var)
        && !val.trim().is_empty()
    {
        return Some(val.trim().to_string());
    }
    if let Ok(path) = std::env::var(file_var)
        && let Ok(val) = fs::read_to_string(path.trim())
        && !val.trim().is_empty()
    {
        return Some(val.trim().to_string());
    }
    if let Ok(cred_dir) = std::env::var("CREDENTIALS_DIRECTORY")
        && let Ok(val) = fs::read_to_string(Path::new(&cred_dir).join(cred_name))
        && !val.trim().is_empty()
    {
        return Some(val.trim().to_string());
    }
    None
}

async fn run_local_shell(cmd: &str) -> String {
    match Command::new("sh").args(["-c", cmd]).output().await {
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).to_string();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(&err);
            }
            s.chars().take(12_000).collect()
        }
        Err(e) => format!("Command failed: {e}"),
    }
}

async fn run_remote_shell(node: &str, cmd: &str) -> String {
    match Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=6", node, cmd])
        .output()
        .await
    {
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).to_string();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(&err);
            }
            s.chars().take(12_000).collect()
        }
        Err(e) => format!("SSH to {node} failed: {e}"),
    }
}

pub fn is_path_in_allowed_roots(path: &Path, flake_path: &str) -> bool {
    let allowed = [
        crate::appdata::expand_user_path(flake_path),
        PathBuf::from("/etc/nixos"),
        PathBuf::from("/etc/systemd/system"),
    ];
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if target
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    allowed.iter().any(|root| {
        let canon_root = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        target.starts_with(&canon_root)
    })
}

/// Redact Bearer tokens, API keys (`sk-`, `AIza`), and inline `password=` / `token=` / `secret=` values.
pub fn redact_secrets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let mut redacted_words = Vec::new();
        let mut prev_bearer = false;
        for word in line.split_whitespace() {
            if prev_bearer {
                redacted_words.push("[REDACTED]".to_string());
                prev_bearer = false;
                continue;
            }
            if word.eq_ignore_ascii_case("bearer") {
                redacted_words.push(word.to_string());
                prev_bearer = true;
                continue;
            }
            if word.starts_with("sk-") || word.starts_with("AIza") || word.starts_with("ghp_") {
                redacted_words.push("[REDACTED_KEY]".to_string());
                continue;
            }
            if let Some((k, v)) = word.split_once('=') {
                let kl = k.to_ascii_lowercase();
                if kl.contains("password")
                    || kl.contains("secret")
                    || kl.contains("token")
                    || kl.contains("api_key")
                    || v.starts_with("sk-")
                    || v.starts_with("AIza")
                    || v.starts_with("ghp_")
                {
                    redacted_words.push(format!("{k}=[REDACTED]"));
                    continue;
                }
            }
            redacted_words.push(word.to_string());
        }
        out.push_str(&redacted_words.join(" "));
        out.push('\n');
    }
    out
}

fn simple_unified_diff(old: &str, new: &str, path: &str) -> String {
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n");
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let max_len = old_lines.len().max(new_lines.len());
    for i in 0..max_len {
        let o = old_lines.get(i).copied();
        let n = new_lines.get(i).copied();
        if o != n {
            if let Some(ol) = o {
                diff.push_str(&format!("-{ol}\n"));
            }
            if let Some(nl) = n {
                diff.push_str(&format!("+{nl}\n"));
            }
        }
    }
    diff
}

fn tool_schemas() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "get_system_vitals",
                "description": "Read live CPU/RAM/GPU/PSI telemetry and top memory/CPU consumers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "node": { "type": "string", "description": "Optional Tailnet peer node name" }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "inspect_systemd_unit",
                "description": "Inspect a systemd service state, restarts, RSS memory, and recent error/warning journalctl logs.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "unit": { "type": "string" },
                        "scope": { "type": "string", "enum": ["system", "user"] },
                        "node": { "type": "string" }
                    },
                    "required": ["unit"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_containers",
                "description": "List Docker/Podman containers and their status/ports.",
                "parameters": {
                    "type": "object",
                    "properties": { "node": { "type": "string" } }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "eval_nix_option",
                "description": "Evaluate a read-only NixOS flake configuration option.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "option_path": { "type": "string" },
                        "host": { "type": "string" },
                        "node": { "type": "string" }
                    },
                    "required": ["option_path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "diff_nix_generations",
                "description": "Diff /run/current-system against recent NixOS profile generations.",
                "parameters": {
                    "type": "object",
                    "properties": { "node": { "type": "string" } }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_config_file",
                "description": "Read a .nix or systemd unit file inside allowed config roots.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "save_host_memory",
                "description": "Persist an important host fact, port mapping, preference, or incident fix into SQLite FTS5 memory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "category": { "type": "string", "enum": ["system", "service", "preference", "incident"] },
                        "content": { "type": "string" }
                    },
                    "required": ["category", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_host_memory",
                "description": "Search persistent SQLite FTS5 host memories.",
                "parameters": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "control_unit_or_container",
                "description": "Start, stop, or restart a systemd unit or container (requires user confirmation).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "enum": ["unit", "container"] },
                        "target": { "type": "string" },
                        "action": { "type": "string", "enum": ["start", "stop", "restart"] },
                        "scope": { "type": "string", "enum": ["system", "user"] },
                        "node": { "type": "string" }
                    },
                    "required": ["kind", "target", "action"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "propose_nix_edit",
                "description": "Propose an edit to a NixOS configuration file with a diff preview (requires user confirmation).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "new_content": { "type": "string" },
                        "summary": { "type": "string" },
                        "node": { "type": "string" }
                    },
                    "required": ["file_path", "new_content", "summary"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_diagnostic_command",
                "description": "Run a bounded administrative shell command on the target node (requires user confirmation).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "summary": { "type": "string" },
                        "node": { "type": "string" }
                    },
                    "required": ["command", "summary"]
                }
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts5_memory_save_and_search_roundtrip() {
        let db = std::env::temp_dir().join(format!("kaage_mem_test_{}.db", std::process::id()));
        let _ = fs::remove_file(&db);
        let store = MemoryStore::open(&db).expect("open fts5");
        store
            .save("service", "ComfyUI listens on port 8188 using RTX 5090")
            .unwrap();
        store
            .save("incident", "Fixed tailscaled DNS timeout by restarting systemd-resolved")
            .unwrap();

        let hits = store.search("ComfyUI 8188", 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0, "service");
        assert!(hits[0].1.contains("8188"));

        let _ = fs::remove_file(&db);
    }

    #[test]
    fn redacts_api_keys_and_bearer_tokens() {
        let raw = "Authorization: Bearer super_secret_tok\nGEMINI_API_KEY=AIzaSyTest12345\nOPENAI=sk-proj-abcdef\npassword=hunter2";
        let clean = redact_secrets(raw);
        assert!(!clean.contains("super_secret_tok"));
        assert!(!clean.contains("AIzaSyTest12345"));
        assert!(!clean.contains("sk-proj-abcdef"));
        assert!(!clean.contains("hunter2"));
        assert!(clean.contains("[REDACTED]"));
    }

    #[test]
    fn enforces_allowed_config_roots() {
        assert!(is_path_in_allowed_roots(
            Path::new("/etc/nixos/configuration.nix"),
            "/home/pradeep/dotfiles"
        ));
        assert!(!is_path_in_allowed_roots(
            Path::new("/etc/shadow"),
            "/home/pradeep/dotfiles"
        ));
        assert!(!is_path_in_allowed_roots(
            Path::new("/etc/nixos/../../etc/shadow"),
            "/home/pradeep/dotfiles"
        ));
    }
}
