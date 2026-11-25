use kaage_proto::{
    CopilotCard, CopilotHistoryMessage, CopilotProvider, CopilotStatus, LocalLlmEndpoint,
    NixosInfo, ServerMsg, TrustMode, VitalsQuery, VitalsWindow,
};
use pi_durable::ai::event_stream::AssistantMessageEventStream;
use pi_durable::ai::http::{Request as PiHttpRequest, send as pi_http_send};
use pi_durable::ai::models::{Models, ModelsError};
use pi_durable::ai::registry::{Auth, Providers};
use pi_durable::ai::types::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent, DeferredHandle, Message, Model,
    SimpleStreamOptions, TranscriptContext, UserBlock, UserContent,
};
use pi_durable::cancel::CancelToken;
use pi_durable::harness::agent::{AgentChange, Change};
use pi_durable::harness::harness::Harness;
use pi_durable::harness::registry::Registry;
use pi_durable::harness::submissions::{SubmissionDraft, WhenBusy};
use pi_durable::harness::types::{
    Extension, HarnessOptions, ModelRef, RootOptions, ToolExecutionResult, ToolRegistration,
};
use pi_durable::storage::memory::MemoryStorage;
use pi_durable::storage::sqlite::SqliteStorage;
use pi_durable::types::Storage;
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
    process::Command,
    sync::{mpsc, oneshot},
};

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_LOCAL_ENDPOINT: &str = "http://127.0.0.1:11434/v1/chat/completions";

pub struct CopilotConfig {
    pub provider: CopilotProvider,
    pub model: String,
    pub trust_mode: TrustMode,
    pub target_node: String,
    pub flake_path: String,
    pub local_endpoint: String,
    pub active_local_name: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct StoredLocalLlm {
    name: String,
    url: String,
    #[serde(default)]
    api_key: Option<String>,
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
struct SavedApiKeys {
    #[serde(default)]
    gemini: Option<String>,
    #[serde(default)]
    openai: Option<String>,
    #[serde(default)]
    claude: Option<String>,
    #[serde(default)]
    local: Option<String>,
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

#[derive(Clone)]
struct ActiveTurn {
    request_id: u64,
    out_tx: mpsc::UnboundedSender<ServerMsg>,
    vitals: Arc<crate::vitals::VitalsCollector>,
    tokio_handle: tokio::runtime::Handle,
}

/// Shared state accessible by both `CopilotManager`, `KaageModels`, and `pi-durable` tool handlers.
struct CopilotCore {
    config: RwLock<CopilotConfig>,
    runtime_keys: RwLock<SavedApiKeys>,
    disk_keys: RwLock<SavedApiKeys>,
    local_endpoints: RwLock<Vec<StoredLocalLlm>>,
    keys_file: PathBuf,
    local_llms_file: PathBuf,
    local_hostname: String,
    memory: Option<Arc<MemoryStore>>,
    pending_confirms: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    next_action_seq: AtomicU64,
    active_turn: Mutex<Option<ActiveTurn>>,
}

impl CopilotCore {
    fn effective_api_key(&self, provider: CopilotProvider) -> Option<String> {
        let rt = self.runtime_keys.read().unwrap_or_else(|e| e.into_inner());
        let from_rt = match provider {
            CopilotProvider::Gemini => rt.gemini.as_deref(),
            CopilotProvider::OpenAi => rt.openai.as_deref(),
            CopilotProvider::Claude => rt.claude.as_deref(),
            CopilotProvider::Local => rt.local.as_deref(),
        }
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
        if from_rt.is_some() {
            return from_rt;
        }
        if provider == CopilotProvider::Local {
            let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
            let target_base = normalize_openai_base_url(&cfg.local_endpoint);
            let list = self.local_endpoints.read().unwrap_or_else(|e| e.into_inner());
            if let Some(found) = list.iter().find(|e| {
                cfg.active_local_name
                    .as_deref()
                    .is_some_and(|n| e.name.eq_ignore_ascii_case(n))
                    || normalize_openai_base_url(&e.url) == target_base
            }) && let Some(k) = found.api_key.as_deref().map(str::trim).filter(|s| !s.is_empty())
            {
                return Some(k.to_string());
            }
        }
        resolve_api_key_from_env(provider)
    }

    fn status(&self) -> CopilotStatus {
        let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
        let has_gemini = self.effective_api_key(CopilotProvider::Gemini).is_some();
        let has_openai = self.effective_api_key(CopilotProvider::OpenAi).is_some();
        let has_anthropic = self.effective_api_key(CopilotProvider::Claude).is_some();
        let ready = match cfg.provider {
            CopilotProvider::Gemini => has_gemini,
            CopilotProvider::OpenAi => has_openai,
            CopilotProvider::Claude => has_anthropic,
            CopilotProvider::Local => !cfg.local_endpoint.trim().is_empty(),
        };
        let local_endpoints: Vec<LocalLlmEndpoint> = self
            .local_endpoints
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|e| LocalLlmEndpoint {
                name: e.name.clone(),
                url: e.url.clone(),
                has_key: e
                    .api_key
                    .as_deref()
                    .is_some_and(|k| !k.trim().is_empty()),
            })
            .collect();
        let dk = self.disk_keys.read().unwrap_or_else(|e| e.into_inner());
        let gemini_key_persisted = dk.gemini.as_deref().is_some_and(|s| !s.trim().is_empty());
        let openai_key_persisted = dk.openai.as_deref().is_some_and(|s| !s.trim().is_empty());
        let anthropic_key_persisted = dk.claude.as_deref().is_some_and(|s| !s.trim().is_empty());
        CopilotStatus {
            provider: cfg.provider,
            model: cfg.model.clone(),
            trust_mode: cfg.trust_mode,
            target_node: cfg.target_node.clone(),
            ready,
            has_gemini_key: has_gemini,
            has_openai_key: has_openai,
            has_anthropic_key: has_anthropic,
            gemini_key_persisted,
            openai_key_persisted,
            anthropic_key_persisted,
            local_endpoint: cfg.local_endpoint.clone(),
            local_endpoints,
            active_local_name: cfg.active_local_name.clone(),
            flake_path: cfg.flake_path.clone(),
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
                    return run_remote_shell(
                        target_node,
                        "uptime && free -m && cat /proc/pressure/cpu /proc/pressure/memory 2>/dev/null || true",
                    )
                    .await;
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
                            snap.latest.cpu_percent,
                            snap.latest.mem_used_mb,
                            snap.latest.mem_total_mb
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
                let (meta_part, logs_part) =
                    redacted.split_once("---LOGS---").unwrap_or((&redacted, ""));
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
                let category = args
                    .get("category")
                    .and_then(Value::as_str)
                    .unwrap_or("system");
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
                    return "Blocked: Kaage Copilot is in Monitored (read-only) mode. Switch to Managed mode with `/mode manage` to perform mutations.".into();
                }
                let kind = args.get("kind").and_then(Value::as_str).unwrap_or("unit");
                let target = args.get("target").and_then(Value::as_str).unwrap_or("");
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("restart");
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
                    return "Blocked: Kaage Copilot is in Monitored (read-only) mode.".into();
                }
                let path_str = args.get("file_path").and_then(Value::as_str).unwrap_or("");
                let new_content = args.get("new_content").and_then(Value::as_str).unwrap_or("");
                let summary = args
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("Update NixOS configuration");
                let expanded = crate::appdata::expand_user_path(path_str);
                if !is_path_in_allowed_roots(&expanded, flake_path) {
                    return format!(
                        "Blocked: {} is outside allowed config roots.",
                        expanded.display()
                    );
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
                    Ok(()) => format!(
                        "Successfully wrote {}. Run `nixos-rebuild switch` to activate.",
                        expanded.display()
                    ),
                    Err(e) => format!("Failed to write {}: {e}", expanded.display()),
                }
            }

            "run_diagnostic_command" => {
                if trust_mode == TrustMode::Monitored {
                    return "Blocked: Kaage Copilot is in Monitored (read-only) mode.".into();
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
}

fn provider_slug(provider: CopilotProvider) -> &'static str {
    match provider {
        CopilotProvider::Gemini => "kaage-gemini",
        CopilotProvider::OpenAi => "kaage-openai",
        CopilotProvider::Claude => "kaage-claude",
        CopilotProvider::Local => "kaage-local",
    }
}

fn provider_from_slug(slug: &str) -> Option<CopilotProvider> {
    match slug {
        "kaage-gemini" => Some(CopilotProvider::Gemini),
        "kaage-openai" => Some(CopilotProvider::OpenAi),
        "kaage-claude" => Some(CopilotProvider::Claude),
        "kaage-local" => Some(CopilotProvider::Local),
        _ => None,
    }
}

fn normalize_openai_base_url(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    for suffix in ["/chat/completions", "/completions", "/models", "/api/tags"] {
        if let Some(base) = trimmed.strip_suffix(suffix) {
            return base.trim_end_matches('/').to_string();
        }
    }
    trimmed.to_string()
}

/// Bridges `pi-durable`'s `Providers` with Kaage's dynamic runtime keys, custom model IDs,
/// and live WebSocket `ServerMsg::CopilotChunk` streaming.
struct KaageModels {
    providers: Providers,
    core: Arc<CopilotCore>,
}

impl KaageModels {
    fn new(core: Arc<CopilotCore>) -> Self {
        let mut providers = Providers::builtin();
        let auth_core = Arc::clone(&core);
        providers.set_auth(Arc::new(move |model: &Model| {
            let mut auth = Auth::default();
            if let Some(p) = provider_from_slug(&model.provider) {
                auth.api_key = auth_core.effective_api_key(p);
                if p == CopilotProvider::Local {
                    if auth.api_key.is_none() {
                        auth.api_key = Some("kaage-local".to_string());
                    }
                    let ep = auth_core
                        .config
                        .read()
                        .unwrap_or_else(|e| e.into_inner())
                        .local_endpoint
                        .clone();
                    auth.base_url = Some(normalize_openai_base_url(&ep));
                }
            }
            Ok(auth)
        }));
        Self { providers, core }
    }
}

impl Models for KaageModels {
    fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        match provider {
            "kaage-gemini" => {
                let mut m = Model::new(provider, id, "openai-completions", 1_000_000, 16_384, false);
                m.base_url = GEMINI_BASE_URL.to_string();
                Some(m)
            }
            "kaage-openai" => {
                let mut m = Model::new(provider, id, "openai-completions", 200_000, 16_384, false);
                m.base_url = OPENAI_BASE_URL.to_string();
                Some(m)
            }
            "kaage-claude" => {
                let mut m = Model::new(provider, id, "anthropic-messages", 200_000, 16_384, false);
                m.base_url = ANTHROPIC_BASE_URL.to_string();
                Some(m)
            }
            "kaage-local" => {
                let ep = self
                    .core
                    .config
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .local_endpoint
                    .clone();
                let mut m = Model::new(provider, id, "openai-completions", 128_000, 8_192, false);
                m.base_url = normalize_openai_base_url(&ep);
                Some(m)
            }
            _ => self.providers.get_model(provider, id),
        }
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: TranscriptContext,
        options: SimpleStreamOptions,
    ) -> AssistantMessageEventStream {
        let endpoint = format!(
            "{}{}",
            options
                .base_url
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(&model.base_url)
                .trim_end_matches('/'),
            if model.api == "anthropic-messages" {
                "/v1/messages"
            } else {
                "/chat/completions"
            }
        );
        let req_bytes = serde_json::to_vec(&context.messages)
            .map(|v| v.len())
            .unwrap_or(0);
        let model_id = model.id.clone();
        tracing::info!("copilot req -> {endpoint} (model={model_id}, req_bytes={req_bytes})");

        let started = std::time::Instant::now();
        let inner = self.providers.stream_simple(model, context, options);
        let outer = AssistantMessageEventStream::new();
        let outer_tx = outer.clone();
        let core = Arc::clone(&self.core);

        std::thread::spawn(move || {
            let mut streamed_bytes: usize = 0;
            let mut had_error = false;
            while let Some(event) = inner.next_event() {
                match &event {
                    AssistantMessageEvent::TextDelta { delta, .. } if !delta.is_empty() => {
                        streamed_bytes += delta.len();
                        if let Some(turn) = core
                            .active_turn
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clone()
                        {
                            let _ = turn.out_tx.send(ServerMsg::CopilotChunk {
                                request_id: turn.request_id,
                                delta: delta.clone(),
                                done: false,
                            });
                        }
                    }
                    AssistantMessageEvent::Error { error, .. } => {
                        had_error = true;
                        if let Some(ref msg) = error.error_message
                            && !msg.is_empty()
                        {
                            if let Some(turn) = core
                                .active_turn
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .clone()
                            {
                                let _ = turn.out_tx.send(ServerMsg::CopilotChunk {
                                    request_id: turn.request_id,
                                    delta: format!("\n[Kaage Copilot error: {msg}]"),
                                    done: false,
                                });
                            }
                        }
                    }
                    _ => {}
                }
                outer_tx.push(event);
            }
            let final_res = inner.result();
            let res_bytes = final_res
                .as_ref()
                .and_then(|m| serde_json::to_vec(&m.content).ok())
                .map(|v| v.len())
                .unwrap_or(streamed_bytes);
            let elapsed_ms = started.elapsed().as_millis();
            if had_error {
                tracing::warn!(
                    "copilot res <- {endpoint} (model={model_id}, req_bytes={req_bytes}, res_bytes={res_bytes}, elapsed_ms={elapsed_ms}, status=error)"
                );
            } else {
                tracing::info!(
                    "copilot res <- {endpoint} (model={model_id}, req_bytes={req_bytes}, res_bytes={res_bytes}, elapsed_ms={elapsed_ms})"
                );
            }
            outer_tx.end(final_res);
        });

        outer
    }

    fn fetch_deferred(
        &self,
        model: &Model,
        handle: &DeferredHandle,
        signal: &CancelToken,
    ) -> Result<AssistantMessage, ModelsError> {
        self.providers.fetch_deferred(model, handle, signal)
    }

    fn cancel_deferred(
        &self,
        model: &Model,
        handle: &DeferredHandle,
        signal: &CancelToken,
    ) -> Result<(), ModelsError> {
        self.providers.cancel_deferred(model, handle, signal)
    }
}

pub struct CopilotManager {
    core: Arc<CopilotCore>,
    harness: Arc<Harness>,
    turn_lock: Arc<tokio::sync::Mutex<()>>,
    active_cancels: Mutex<HashMap<u64, CancelToken>>,
}

impl CopilotManager {
    pub fn new(local_hostname: String) -> Arc<Self> {
        let provider = match std::env::var("KAAGE_COPILOT_PROVIDER")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "openai" => CopilotProvider::OpenAi,
            "claude" | "anthropic" => CopilotProvider::Claude,
            "local" | "ollama" => CopilotProvider::Local,
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
            .unwrap_or_else(|| {
                crate::appdata::expand_user_path("~/dotfiles")
                    .display()
                    .to_string()
            });
        let local_endpoint = std::env::var("KAAGE_COPILOT_LOCAL_ENDPOINT")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_LOCAL_ENDPOINT.to_string());

        let kaage_dir = crate::appdata::kaage_app_dir();
        let db_path = kaage_dir.join("memory.db");
        let durable_db_path = kaage_dir.join("copilot_durable.db");
        let keys_file = kaage_dir.join("copilot_keys.json");
        let local_llms_file = kaage_dir.join("copilot_local_llms.json");
        let saved_keys: SavedApiKeys = fs::read_to_string(&keys_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let saved_local_llms: Vec<StoredLocalLlm> = fs::read_to_string(&local_llms_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

        let memory = match MemoryStore::open(&db_path) {
            Ok(store) => {
                tracing::info!(
                    "Kaage Copilot FTS5 memory initialized at {}",
                    db_path.display()
                );
                Some(Arc::new(store))
            }
            Err(err) => {
                tracing::warn!("Kaage Copilot FTS5 memory disabled: {err}");
                None
            }
        };

        let active_local_name = saved_local_llms
            .iter()
            .find(|e| normalize_openai_base_url(&e.url) == normalize_openai_base_url(&local_endpoint))
            .map(|e| e.name.clone());

        let core = Arc::new(CopilotCore {
            config: RwLock::new(CopilotConfig {
                provider,
                model: model.clone(),
                trust_mode,
                target_node: local_hostname.clone(),
                flake_path,
                local_endpoint,
                active_local_name,
            }),
            runtime_keys: RwLock::new(saved_keys.clone()),
            disk_keys: RwLock::new(saved_keys),
            local_endpoints: RwLock::new(saved_local_llms),
            keys_file,
            local_llms_file,
            local_hostname,
            memory,
            pending_confirms: Mutex::new(HashMap::new()),
            next_action_seq: AtomicU64::new(1),
            active_turn: Mutex::new(None),
        });

        let storage: Arc<dyn Storage> = match SqliteStorage::open(&durable_db_path, &Default::default())
        {
            Ok(sql) => {
                tracing::info!(
                    "Kaage Copilot durable SQLite store initialized at {}",
                    durable_db_path.display()
                );
                Arc::new(sql)
            }
            Err(err) => {
                tracing::warn!(
                    "Kaage Copilot durable SQLite store failed ({err}), falling back to in-memory storage"
                );
                Arc::new(MemoryStorage::new())
            }
        };

        let registry = Arc::new(Registry::default());
        registry
            .install(build_kaage_extension(Arc::clone(&core)))
            .expect("install kaage-copilot pi-durable extension");

        let harness = Harness::open(
            storage,
            HarnessOptions {
                models: Arc::new(KaageModels::new(Arc::clone(&core))),
                registry,
                settings: None,
                conversation_created: None,
                now: None,
                env: None,
                on_report: Some(Arc::new(|err| {
                    tracing::warn!("Kaage Copilot durable harness report: {err}");
                })),
            },
        )
        .expect("open pi-durable harness for Kaage Copilot");

        let init_cancel = CancelToken::new();
        let _ = harness.root(
            RootOptions {
                agent: Some(AgentChange {
                    model: Change::Set(ModelRef {
                        provider: provider_slug(provider).to_string(),
                        model_id: model,
                    }),
                    ..Default::default()
                }),
                init: None,
            },
            &init_cancel,
        );

        Arc::new(Self {
            core,
            harness: Arc::new(harness),
            turn_lock: Arc::new(tokio::sync::Mutex::new(())),
            active_cancels: Mutex::new(HashMap::new()),
        })
    }

    pub fn set_api_key(
        &self,
        provider: CopilotProvider,
        api_key: String,
        persist: bool,
    ) -> Result<CopilotStatus, String> {
        let trimmed = api_key.trim().to_string();
        let val = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        {
            let mut rt = self
                .core
                .runtime_keys
                .write()
                .unwrap_or_else(|e| e.into_inner());
            match provider {
                CopilotProvider::Gemini => rt.gemini = val.clone(),
                CopilotProvider::OpenAi => rt.openai = val.clone(),
                CopilotProvider::Claude => rt.claude = val.clone(),
                CopilotProvider::Local => rt.local = val.clone(),
            }
        }
        {
            // Per-provider disk persistence:
            // - If forgetting (`val.is_none()`): always erase this provider's key from disk too.
            // - If saving with `persist == true`: store this provider's key on disk (`0600`).
            // - If saving with `persist == false`: keep in RAM only and clear any old disk entry for this provider.
            let mut dk = self
                .core
                .disk_keys
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let disk_val = if persist { val } else { None };
            let mut changed = false;
            match provider {
                CopilotProvider::Gemini => {
                    if dk.gemini != disk_val {
                        dk.gemini = disk_val;
                        changed = true;
                    }
                }
                CopilotProvider::OpenAi => {
                    if dk.openai != disk_val {
                        dk.openai = disk_val;
                        changed = true;
                    }
                }
                CopilotProvider::Claude => {
                    if dk.claude != disk_val {
                        dk.claude = disk_val;
                        changed = true;
                    }
                }
                CopilotProvider::Local => {
                    if dk.local != disk_val {
                        dk.local = disk_val;
                        changed = true;
                    }
                }
            }
            if changed {
                let any_disk = dk.gemini.is_some()
                    || dk.openai.is_some()
                    || dk.claude.is_some()
                    || dk.local.is_some();
                if any_disk {
                    if let Some(parent) = self.core.keys_file.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let json = serde_json::to_string_pretty(&*dk).map_err(|e| e.to_string())?;
                    fs::write(&self.core.keys_file, json).map_err(|e| {
                        format!("Failed to write {}: {e}", self.core.keys_file.display())
                    })?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = fs::set_permissions(
                            &self.core.keys_file,
                            fs::Permissions::from_mode(0o600),
                        );
                    }
                } else if self.core.keys_file.exists() {
                    let _ = fs::remove_file(&self.core.keys_file);
                }
            }
        }
        Ok(self.status())
    }

    pub fn status(&self) -> CopilotStatus {
        self.core.status()
    }

    /// Read the persisted durable chat history from `pi-durable`'s SQLite ledger.
    pub fn history(&self) -> Vec<CopilotHistoryMessage> {
        let cancel = CancelToken::new();
        let Ok(root) = self.harness.root(RootOptions::default(), &cancel) else {
            return Vec::new();
        };
        let Ok(ctx) = root.context(None, &cancel) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for msg in ctx.messages {
            match msg {
                Message::User(u) => {
                    let text = match &u.content {
                        UserContent::Text(s) => s.clone(),
                        UserContent::Blocks(blocks) => blocks
                            .iter()
                            .filter_map(|b| match b {
                                UserBlock::Text(t) => Some(t.text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    };
                    if !text.trim().is_empty() {
                        out.push(CopilotHistoryMessage {
                            role: "user".into(),
                            text,
                            ts_ms: u.timestamp,
                        });
                    }
                }
                Message::Assistant(a) => {
                    let text = a
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            AssistantBlock::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !text.trim().is_empty() {
                        out.push(CopilotHistoryMessage {
                            role: "assistant".into(),
                            text,
                            ts_ms: a.timestamp,
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Reset the durable root conversation so the user starts a fresh chat session.
    pub fn clear_history(&self) -> Result<(), String> {
        let cancel = CancelToken::new();
        let root = self
            .harness
            .root(RootOptions::default(), &cancel)
            .map_err(|e| format!("Failed to open durable conversation: {e}"))?;
        root.reset(None, &cancel)
            .map_err(|e| format!("Failed to reset conversation: {e}"))?;
        let _ = root.wait_for_idle(&cancel);
        Ok(())
    }

    pub fn upsert_local_llm(
        &self,
        name: String,
        url: String,
        api_key: Option<String>,
        persist: bool,
    ) -> Result<CopilotStatus, String> {
        let name = name.trim().to_string();
        let url = url.trim().to_string();
        if name.is_empty() {
            return Err("Endpoint name cannot be empty".into());
        }
        if url.is_empty() || (!url.starts_with("http://") && !url.starts_with("https://")) {
            return Err("Endpoint URL must start with http:// or https://".into());
        }
        let clean_key = api_key
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());

        let effective_key = {
            let mut list = self
                .core
                .local_endpoints
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = list.iter_mut().find(|e| e.name.eq_ignore_ascii_case(&name)) {
                existing.name = name.clone();
                existing.url = url.clone();
                if clean_key.is_some() {
                    existing.api_key = clean_key.clone();
                }
                let k = existing.api_key.clone();
                if persist {
                    persist_local_llms(&self.core.local_llms_file, &list)?;
                }
                k
            } else {
                list.push(StoredLocalLlm {
                    name: name.clone(),
                    url: url.clone(),
                    api_key: clean_key.clone(),
                });
                if persist {
                    persist_local_llms(&self.core.local_llms_file, &list)?;
                }
                clean_key
            }
        };

        {
            let mut cfg = self.core.config.write().unwrap_or_else(|e| e.into_inner());
            cfg.provider = CopilotProvider::Local;
            cfg.local_endpoint = url;
            cfg.active_local_name = Some(name);
        }
        {
            let mut rt = self
                .core
                .runtime_keys
                .write()
                .unwrap_or_else(|e| e.into_inner());
            rt.local = effective_key;
        }
        Ok(self.status())
    }

    pub fn select_local_llm(&self, name: &str) -> Result<CopilotStatus, String> {
        let found = self
            .core
            .local_endpoints
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name.trim()))
            .cloned()
            .ok_or_else(|| format!("Local endpoint `{name}` not found"))?;
        {
            let mut cfg = self.core.config.write().unwrap_or_else(|e| e.into_inner());
            cfg.provider = CopilotProvider::Local;
            cfg.local_endpoint = found.url;
            cfg.active_local_name = Some(found.name);
        }
        {
            let mut rt = self
                .core
                .runtime_keys
                .write()
                .unwrap_or_else(|e| e.into_inner());
            rt.local = found.api_key;
        }
        Ok(self.status())
    }

    pub fn delete_local_llm(&self, name: &str) -> Result<CopilotStatus, String> {
        {
            let mut list = self
                .core
                .local_endpoints
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let before = list.len();
            list.retain(|e| !e.name.eq_ignore_ascii_case(name.trim()));
            if list.len() != before {
                if list.is_empty() && self.core.local_llms_file.exists() {
                    let _ = fs::remove_file(&self.core.local_llms_file);
                } else if self.core.local_llms_file.exists() {
                    persist_local_llms(&self.core.local_llms_file, &list)?;
                }
            }
        }
        {
            let mut cfg = self.core.config.write().unwrap_or_else(|e| e.into_inner());
            if cfg
                .active_local_name
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(name.trim()))
            {
                cfg.active_local_name = None;
            }
        }
        Ok(self.status())
    }

    /// Test connectivity to a local OpenAI-compatible / Ollama endpoint and pull its available models.
    pub fn test_local_llm(&self, url: String, api_key: Option<String>) -> ServerMsg {
        let trimmed = url.trim().to_string();
        if trimmed.is_empty() || (!trimmed.starts_with("http://") && !trimmed.starts_with("https://"))
        {
            return ServerMsg::LocalLlmTestResult {
                ok: false,
                url: trimmed,
                message: "URL must start with http:// or https://".into(),
                models: Vec::new(),
            };
        }
        let key = api_key
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .or_else(|| self.core.effective_api_key(CopilotProvider::Local));

        let base = normalize_openai_base_url(&trimmed);
        let mut candidate_urls = Vec::new();
        if base.ends_with("/models") {
            candidate_urls.push(base.clone());
        } else if base.ends_with("/v1") {
            candidate_urls.push(format!("{base}/models"));
            if let Some(root) = base.strip_suffix("/v1") {
                candidate_urls.push(format!("{root}/api/tags"));
            }
        } else {
            candidate_urls.push(format!("{base}/v1/models"));
            candidate_urls.push(format!("{base}/models"));
            candidate_urls.push(format!("{base}/api/tags"));
        }

        let cancel = CancelToken::new();
        let mut last_err = String::from("Could not reach endpoint");
        for probe_url in &candidate_urls {
            tracing::debug!("Testing local LLM endpoint: {probe_url} (key_present={})", key.is_some());
            let mut req = PiHttpRequest::new("GET", probe_url.clone())
                .timeout(Some(std::time::Duration::from_secs(8)));
            if let Some(ref k) = key {
                req = req.header("Authorization", format!("Bearer {k}"));
            }
            match pi_http_send(&req, Some(&cancel)) {
                Ok(resp) if resp.ok() => {
                    let body = resp.text().unwrap_or_default();
                    let res_bytes = body.len();
                    let models = parse_models_response(&body);
                    tracing::info!(
                        "copilot probe <- {probe_url} (req_bytes=0, res_bytes={res_bytes}, models={})",
                        models.len()
                    );
                    let msg = if models.is_empty() {
                        format!("Connected to {probe_url} (0 models returned)")
                    } else {
                        format!("Connected — {} model(s) available", models.len())
                    };
                    return ServerMsg::LocalLlmTestResult {
                        ok: true,
                        url: trimmed,
                        message: msg,
                        models,
                    };
                }
                Ok(resp) => {
                    let status = resp.status;
                    let snippet: String = resp
                        .text()
                        .unwrap_or_default()
                        .chars()
                        .take(180)
                        .collect();
                    last_err = format!("HTTP {status} from {probe_url}: {}", snippet.trim());
                }
                Err(err) => {
                    last_err = format!("{probe_url}: {err}");
                }
            }
        }

        ServerMsg::LocalLlmTestResult {
            ok: false,
            url: trimmed,
            message: last_err,
            models: Vec::new(),
        }
    }

    pub fn update_config(
        &self,
        provider: Option<CopilotProvider>,
        model: Option<String>,
        trust_mode: Option<TrustMode>,
        target_node: Option<String>,
        local_endpoint: Option<String>,
    ) -> CopilotStatus {
        {
            let mut cfg = self.core.config.write().unwrap_or_else(|e| e.into_inner());
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
            if let Some(ep) = local_endpoint
                && !ep.trim().is_empty()
            {
                cfg.local_endpoint = ep.trim().to_string();
            }
        }
        self.status()
    }

    pub fn resolve_confirmation(&self, action_id: &str, approved: bool) -> bool {
        let tx = self
            .core
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
        if let Some(token) = self
            .active_cancels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&request_id)
        {
            token.cancel();
        }
        let harness = Arc::clone(&self.harness);
        std::thread::spawn(move || {
            let c = CancelToken::new();
            if let Ok(root) = harness.root(RootOptions::default(), &c) {
                let _ = root.abort(false, &c);
            }
        });
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
        let handle = tokio::runtime::Handle::current();
        tokio::spawn(async move {
            let _guard = mgr.turn_lock.lock().await;
            let status = mgr.status();
            let api_key = mgr.core.effective_api_key(status.provider);
            if status.provider != CopilotProvider::Local && api_key.is_none() {
                let env_name = status.provider.env_var_name();
                let _ = out_tx.send(ServerMsg::CopilotChunk {
                    request_id,
                    delta: format!(
                        "Kaage Copilot is missing `{env_name}` for `{}`. Add the key in the setup banner above or in `kaage-settings → Copilot (AI)`.",
                        status.provider.label()
                    ),
                    done: true,
                });
                return;
            }

            let mut recalled_block = String::new();
            if let Some(ref mem) = mgr.core.memory {
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
                mgr.core.local_hostname,
                nixos.pretty_name,
                nixos.is_nixos,
                nixos.generation,
                nixos.current_system,
                status.flake_path,
                status.target_node,
                status.trust_mode.as_str(),
            );

            let cancel = CancelToken::new();
            mgr.active_cancels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(request_id, cancel.clone());

            *mgr.core
                .active_turn
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(ActiveTurn {
                request_id,
                out_tx: out_tx.clone(),
                vitals,
                tokio_handle: handle,
            });

            let harness = Arc::clone(&mgr.harness);
            let provider = status.provider;
            let model_id = status.model.clone();
            let cancel_for_blocking = cancel.clone();

            let res = tokio::task::spawn_blocking(move || -> Result<(), String> {
                let root = harness
                    .root(RootOptions::default(), &cancel_for_blocking)
                    .map_err(|e| format!("Durable root conversation failed: {e}"))?;

                root.configure(
                    &AgentChange {
                        model: Change::Set(ModelRef {
                            provider: provider_slug(provider).to_string(),
                            model_id,
                        }),
                        instructions: Change::Set(system_prompt),
                        ..Default::default()
                    },
                    &cancel_for_blocking,
                )
                .map_err(|e| format!("Durable configure failed: {e}"))?;

                root.submit(
                    SubmissionDraft::Input {
                        request_id: Some(format!("kaage-{request_id}")),
                        content: UserContent::Text(prompt),
                        when_busy: WhenBusy::default(),
                    },
                    &cancel_for_blocking,
                )
                .map_err(|e| format!("Durable submit failed: {e}"))?;

                root.wait_for_idle(&cancel_for_blocking)
                    .map_err(|e| format!("{e}"))?;
                Ok(())
            })
            .await;

            *mgr.core
                .active_turn
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            mgr.active_cancels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&request_id);

            match res {
                Ok(Ok(())) => {
                    let _ = out_tx.send(ServerMsg::CopilotChunk {
                        request_id,
                        delta: String::new(),
                        done: true,
                    });
                }
                Ok(Err(err)) => {
                    let msg = if cancel.is_cancelled() {
                        "\n[Cancelled]".to_string()
                    } else {
                        format!("\n[Kaage Copilot error: {err}]")
                    };
                    let _ = out_tx.send(ServerMsg::CopilotChunk {
                        request_id,
                        delta: msg,
                        done: true,
                    });
                }
                Err(join_err) => {
                    let _ = out_tx.send(ServerMsg::CopilotChunk {
                        request_id,
                        delta: format!("\n[Kaage Copilot worker error: {join_err}]"),
                        done: true,
                    });
                }
            }
        });
    }
}

fn build_kaage_extension(core: Arc<CopilotCore>) -> Extension {
    let defs: Vec<(&'static str, &'static str, Value)> = vec![
        (
            "get_system_vitals",
            "Read live CPU/RAM/GPU/PSI telemetry and top memory/CPU consumers.",
            json!({
                "type": "object",
                "properties": {
                    "node": { "type": "string", "description": "Optional Tailnet peer node name" }
                }
            }),
        ),
        (
            "inspect_systemd_unit",
            "Inspect a systemd service state, restarts, RSS memory, and recent error/warning journalctl logs.",
            json!({
                "type": "object",
                "properties": {
                    "unit": { "type": "string" },
                    "scope": { "type": "string", "enum": ["system", "user"] },
                    "node": { "type": "string" }
                },
                "required": ["unit"]
            }),
        ),
        (
            "list_containers",
            "List Docker/Podman containers and their status/ports.",
            json!({
                "type": "object",
                "properties": { "node": { "type": "string" } }
            }),
        ),
        (
            "eval_nix_option",
            "Evaluate a read-only NixOS flake configuration option.",
            json!({
                "type": "object",
                "properties": {
                    "option_path": { "type": "string" },
                    "host": { "type": "string" },
                    "node": { "type": "string" }
                },
                "required": ["option_path"]
            }),
        ),
        (
            "diff_nix_generations",
            "Diff /run/current-system against recent NixOS profile generations.",
            json!({
                "type": "object",
                "properties": { "node": { "type": "string" } }
            }),
        ),
        (
            "read_config_file",
            "Read a .nix or systemd unit file inside allowed config roots.",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        ),
        (
            "save_host_memory",
            "Persist an important host fact, port mapping, preference, or incident fix into SQLite FTS5 memory.",
            json!({
                "type": "object",
                "properties": {
                    "category": { "type": "string", "enum": ["system", "service", "preference", "incident"] },
                    "content": { "type": "string" }
                },
                "required": ["category", "content"]
            }),
        ),
        (
            "search_host_memory",
            "Search persistent SQLite FTS5 host memories.",
            json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
        ),
        (
            "control_unit_or_container",
            "Start, stop, or restart a systemd unit or container (requires user confirmation).",
            json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["unit", "container"] },
                    "target": { "type": "string" },
                    "action": { "type": "string", "enum": ["start", "stop", "restart"] },
                    "scope": { "type": "string", "enum": ["system", "user"] },
                    "node": { "type": "string" }
                },
                "required": ["kind", "target", "action"]
            }),
        ),
        (
            "propose_nix_edit",
            "Propose an edit to a NixOS configuration file with a diff preview (requires user confirmation).",
            json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" },
                    "new_content": { "type": "string" },
                    "summary": { "type": "string" },
                    "node": { "type": "string" }
                },
                "required": ["file_path", "new_content", "summary"]
            }),
        ),
        (
            "run_diagnostic_command",
            "Run a bounded administrative shell command on the target node (requires user confirmation).",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "summary": { "type": "string" },
                    "node": { "type": "string" }
                },
                "required": ["command", "summary"]
            }),
        ),
    ];

    let tools = defs
        .into_iter()
        .map(|(name, desc, params)| {
            let core_ref = Arc::clone(&core);
            ToolRegistration {
                tool: serde_json::from_value(json!({
                    "name": name,
                    "description": desc,
                    "parameters": params
                }))
                .expect("valid kaage tool schema"),
                replay: None,
                execution_mode: None,
                output_limits: None,
                prepare_arguments: None,
                execute: Arc::new(move |arguments, _api, _cancel| {
                    let Some(turn) = core_ref
                        .active_turn
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                    else {
                        return Ok(ToolExecutionResult {
                            content: Some(vec![serde_json::from_value(json!({
                                "type": "text",
                                "text": "No active Kaage Copilot session."
                            }))?]),
                            is_error: Some(true),
                            ..Default::default()
                        });
                    };

                    let status = core_ref.status();
                    let target_node = arguments
                        .get("node")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| status.target_node.clone());

                    let _ = turn.out_tx.send(ServerMsg::CopilotToolStatus {
                        request_id: turn.request_id,
                        tool_name: name.to_string(),
                        target_node: target_node.clone(),
                        state: "running".into(),
                    });

                    let result = turn.tokio_handle.block_on(core_ref.execute_tool(
                        turn.request_id,
                        name,
                        &arguments,
                        &target_node,
                        status.trust_mode,
                        &status.flake_path,
                        &turn.vitals,
                        &turn.out_tx,
                    ));

                    let _ = turn.out_tx.send(ServerMsg::CopilotToolStatus {
                        request_id: turn.request_id,
                        tool_name: name.to_string(),
                        target_node,
                        state: "done".into(),
                    });

                    Ok(ToolExecutionResult {
                        content: Some(vec![serde_json::from_value(json!({
                            "type": "text",
                            "text": result
                        }))?]),
                        ..Default::default()
                    })
                }),
            }
        })
        .collect();

    Extension {
        name: "kaage-copilot".to_string(),
        tools,
        sections: vec![],
        wraps: vec![],
        hooks: vec![],
        tasks: vec![],
    }
}

fn persist_local_llms(path: &Path, list: &[StoredLocalLlm]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Parse model IDs from OpenAI `/v1/models` (`{ data: [{ id }] }`) or Ollama `/api/tags` (`{ models: [{ name }] }`).
pub fn parse_models_response(body: &str) -> Vec<String> {
    let Ok(val) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(arr) = val.get("data").and_then(Value::as_array) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(Value::as_str)
                && !id.trim().is_empty()
            {
                out.push(id.trim().to_string());
            }
        }
    }
    if out.is_empty()
        && let Some(arr) = val.get("models").and_then(Value::as_array)
    {
        for item in arr {
            if let Some(name) = item
                .get("name")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                && !name.trim().is_empty()
            {
                out.push(name.trim().to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn resolve_api_key_from_env(provider: CopilotProvider) -> Option<String> {
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
        CopilotProvider::Claude => (
            "ANTHROPIC_API_KEY",
            "KAAGE_COPILOT_ANTHROPIC_KEY_FILE",
            "anthropic_api_key",
        ),
        CopilotProvider::Local => (
            "KAAGE_COPILOT_LOCAL_API_KEY",
            "KAAGE_COPILOT_LOCAL_KEY_FILE",
            "local_api_key",
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

#[cfg(test)]
mod tests {
    use super::*;
    use pi_durable::ai::providers::faux::{Faux, FauxOptions, FauxResponse};
    use pi_durable::ai::types::{AssistantBlock, Message};

    #[test]
    fn fts5_memory_save_and_search_roundtrip() {
        let db = std::env::temp_dir().join(format!("kaage_mem_test_{}.db", std::process::id()));
        let _ = fs::remove_file(&db);
        let store = MemoryStore::open(&db).expect("open fts5");
        store
            .save("service", "ComfyUI listens on port 8188 using RTX 5090")
            .unwrap();
        store
            .save(
                "incident",
                "Fixed tailscaled DNS timeout by restarting systemd-resolved",
            )
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

    #[test]
    fn pi_durable_sqlite_conversation_persists_across_reopen() {
        let db = std::env::temp_dir().join(format!(
            "kaage_copilot_durable_test_{}.db",
            std::process::id()
        ));
        let _ = fs::remove_file(&db);
        let cancel = CancelToken::new();

        let faux = Arc::new(Faux::new(FauxOptions::default()));
        faux.set_responses(vec![FauxResponse::Message(Box::new(
            serde_json::from_value(json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "Kaage Copilot durable reply."}],
                "api": "faux",
                "provider": "faux",
                "model": "faux-1",
                "stopReason": "stop",
                "timestamp": 0,
                "usage": {
                    "input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 15,
                    "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}
                }
            }))
            .unwrap(),
        ))]);

        let registry = Arc::new(Registry::default());
        registry
            .install(Extension {
                name: "kaage-copilot".into(),
                tools: vec![],
                sections: vec![],
                wraps: vec![],
                hooks: vec![],
                tasks: vec![],
            })
            .unwrap();

        let harness = Harness::open(
            Arc::new(SqliteStorage::open(&db, &Default::default()).unwrap()),
            HarnessOptions {
                models: faux.clone(),
                registry: registry.clone(),
                settings: None,
                conversation_created: None,
                now: None,
                env: None,
                on_report: None,
            },
        )
        .unwrap();

        let root = harness
            .root(
                RootOptions {
                    agent: Some(AgentChange {
                        model: Change::Set(ModelRef {
                            provider: "faux".into(),
                            model_id: "faux-1".into(),
                        }),
                        ..Default::default()
                    }),
                    init: None,
                },
                &cancel,
            )
            .unwrap();

        root.submit(
            SubmissionDraft::Input {
                request_id: Some("kaage-test-1".into()),
                content: UserContent::Text("check host status".into()),
                when_busy: WhenBusy::default(),
            },
            &cancel,
        )
        .unwrap();
        root.wait_for_idle(&cancel).unwrap();
        harness.close(&cancel).unwrap();

        // Reopen the same SQLite ledger and verify the conversation survived restart
        let harness2 = Harness::open(
            Arc::new(SqliteStorage::open(&db, &Default::default()).unwrap()),
            HarnessOptions {
                models: faux,
                registry,
                settings: None,
                conversation_created: None,
                now: None,
                env: None,
                on_report: None,
            },
        )
        .unwrap();
        let root2 = harness2.root(RootOptions::default(), &cancel).unwrap();
        let ctx = root2.context(None, &cancel).unwrap();
        let found = ctx.messages.iter().any(|m| {
            if let Message::Assistant(a) = m {
                a.content.iter().any(|b| match b {
                    AssistantBlock::Text(t) => t.text.contains("Kaage Copilot durable reply."),
                    _ => false,
                })
            } else {
                false
            }
        });
        assert!(found, "durable reply should persist across Harness reopen");
        harness2.close(&cancel).unwrap();
        let _ = fs::remove_file(&db);
    }

    #[test]
    fn parses_openai_and_ollama_model_listings() {
        let openai_json = r#"{"object":"list","data":[{"id":"qwen3:30b"},{"id":"llama3.3:70b"},{"id":"qwen3:30b"}]}"#;
        assert_eq!(
            parse_models_response(openai_json),
            vec!["llama3.3:70b".to_string(), "qwen3:30b".to_string()]
        );

        let ollama_json = r#"{"models":[{"name":"deepseek-r1:32b"},{"name":"gemma3:27b"}]}"#;
        assert_eq!(
            parse_models_response(ollama_json),
            vec!["deepseek-r1:32b".to_string(), "gemma3:27b".to_string()]
        );

        assert_eq!(
            normalize_openai_base_url("http://127.0.0.1:8888/v1/completions"),
            "http://127.0.0.1:8888/v1"
        );
        assert_eq!(
            normalize_openai_base_url("http://127.0.0.1:11434/v1/chat/completions/"),
            "http://127.0.0.1:11434/v1"
        );
    }
}
