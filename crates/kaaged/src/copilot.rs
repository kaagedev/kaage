use kaage_proto::{
    CopilotCard, CopilotHistoryMessage, CopilotOptimizations, CopilotProvider, CopilotStatus,
    HostMemoryItem, LocalLlmEndpoint, NixosInfo, ServerMsg, TrustMode, VitalsQuery, VitalsWindow,
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
const DEFAULT_LOCAL_CTX_WINDOW: u64 = 32_768;

fn default_local_ctx() -> u64 {
    DEFAULT_LOCAL_CTX_WINDOW
}

pub struct CopilotConfig {
    pub provider: CopilotProvider,
    pub model: String,
    pub trust_mode: TrustMode,
    pub target_node: String,
    pub flake_path: String,
    pub local_endpoint: String,
    pub local_context_window: u64,
    pub active_local_name: Option<String>,
    pub optimizations: CopilotOptimizations,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct StoredLocalLlm {
    name: String,
    url: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default = "default_local_ctx")]
    context_window: u64,
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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DefaultLlmConfig {
    provider: CopilotProvider,
    model: String,
    #[serde(default)]
    local_endpoint: Option<String>,
    #[serde(default)]
    local_name: Option<String>,
}

#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedCopilotConfig {
    #[serde(default)]
    default_backend: Option<DefaultLlmConfig>,
    #[serde(default)]
    last_provider: Option<CopilotProvider>,
    #[serde(default)]
    last_model: Option<String>,
    #[serde(default)]
    last_local_endpoint: Option<String>,
    #[serde(default)]
    last_local_name: Option<String>,
    #[serde(default)]
    trust_mode: Option<TrustMode>,
    #[serde(default)]
    optimizations: CopilotOptimizations,
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
            );
            CREATE TABLE IF NOT EXISTS pinned_memories (
                memory_rowid INTEGER PRIMARY KEY
            );",
        )
        .map_err(|e| format!("FTS5 init failed: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn save(&self, category: &str, content: &str) -> Result<(), String> {
        self.save_with_pin(category, content, false).map(|_| ())
    }

    pub fn save_with_pin(&self, category: &str, content: &str, pinned: bool) -> Result<i64, String> {
        let content = content.trim();
        if content.is_empty() {
            return Err("Memory content cannot be empty".into());
        }
        let cat = match category.trim().to_ascii_lowercase().as_str() {
            "service" => "service",
            "preference" => "preference",
            "incident" => "incident",
            _ => "system",
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO memories_fts (category, content, created_at) VALUES (?1, ?2, ?3)",
            params![cat, content, now.to_string()],
        )
        .map_err(|e| format!("Memory save failed: {e}"))?;
        let id = conn.last_insert_rowid();
        if pinned {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO pinned_memories (memory_rowid) VALUES (?1)",
                params![id],
            );
        }
        Ok(id)
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<(), String> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if pinned {
            conn.execute(
                "INSERT OR IGNORE INTO pinned_memories (memory_rowid) VALUES (?1)",
                params![id],
            )
            .map_err(|e| format!("Pin memory failed: {e}"))?;
        } else {
            conn.execute(
                "DELETE FROM pinned_memories WHERE memory_rowid = ?1",
                params![id],
            )
            .map_err(|e| format!("Unpin memory failed: {e}"))?;
        }
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let _ = conn.execute(
            "DELETE FROM pinned_memories WHERE memory_rowid = ?1",
            params![id],
        );
        conn.execute("DELETE FROM memories_fts WHERE rowid = ?1", params![id])
            .map_err(|e| format!("Delete memory failed: {e}"))?;
        Ok(())
    }

    pub fn pinned(&self, limit: usize) -> Vec<HostMemoryItem> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT m.rowid, m.category, m.content, m.created_at \
             FROM memories_fts m \
             INNER JOIN pinned_memories p ON p.memory_rowid = m.rowid \
             ORDER BY m.rowid DESC LIMIT ?1",
        ) && let Ok(rows) = stmt.query_map(params![limit as i64], |r| {
            let created_str: String = r.get(3).unwrap_or_default();
            Ok(HostMemoryItem {
                id: r.get(0)?,
                category: r.get(1)?,
                content: r.get(2)?,
                pinned: true,
                created_at: created_str.parse::<i64>().unwrap_or(0),
            })
        }) {
            for row in rows.flatten() {
                out.push(row);
            }
        }
        out
    }

    pub fn list(&self, query: Option<&str>, limit: usize) -> Vec<HostMemoryItem> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let q = query.map(str::trim).filter(|s| !s.is_empty());
        let mut out = Vec::new();

        if let Some(q) = q {
            let tokens: Vec<String> = q
                .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
                .filter(|t| t.len() >= 2)
                .map(|t| format!("\"{}\"", t.replace('"', "")))
                .collect();
            if !tokens.is_empty() {
                let fts_query = tokens.join(" OR ");
                if let Ok(mut stmt) = conn.prepare(
                    "SELECT m.rowid, m.category, m.content, m.created_at, (p.memory_rowid IS NOT NULL) AS is_pinned \
                     FROM memories_fts m \
                     LEFT JOIN pinned_memories p ON p.memory_rowid = m.rowid \
                     WHERE memories_fts MATCH ?1 \
                     ORDER BY is_pinned DESC, rank LIMIT ?2",
                ) && let Ok(rows) = stmt.query_map(params![fts_query, limit as i64], |r| {
                    let created_str: String = r.get(3).unwrap_or_default();
                    let pinned_int: i64 = r.get(4).unwrap_or(0);
                    Ok(HostMemoryItem {
                        id: r.get(0)?,
                        category: r.get(1)?,
                        content: r.get(2)?,
                        pinned: pinned_int != 0,
                        created_at: created_str.parse::<i64>().unwrap_or(0),
                    })
                }) {
                    for row in rows.flatten() {
                        out.push(row);
                    }
                }
            }
            return out;
        }

        if let Ok(mut stmt) = conn.prepare(
            "SELECT m.rowid, m.category, m.content, m.created_at, (p.memory_rowid IS NOT NULL) AS is_pinned \
             FROM memories_fts m \
             LEFT JOIN pinned_memories p ON p.memory_rowid = m.rowid \
             ORDER BY is_pinned DESC, m.rowid DESC LIMIT ?1",
        ) && let Ok(rows) = stmt.query_map(params![limit as i64], |r| {
            let created_str: String = r.get(3).unwrap_or_default();
            let pinned_int: i64 = r.get(4).unwrap_or(0);
            Ok(HostMemoryItem {
                id: r.get(0)?,
                category: r.get(1)?,
                content: r.get(2)?,
                pinned: pinned_int != 0,
                created_at: created_str.parse::<i64>().unwrap_or(0),
            })
        }) {
            for row in rows.flatten() {
                out.push(row);
            }
        }
        out
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
    default_backend: RwLock<Option<DefaultLlmConfig>>,
    runtime_keys: RwLock<SavedApiKeys>,
    disk_keys: RwLock<SavedApiKeys>,
    local_endpoints: RwLock<Vec<StoredLocalLlm>>,
    config_file: PathBuf,
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
                context_window: e.context_window,
            })
            .collect();
        let dk = self.disk_keys.read().unwrap_or_else(|e| e.into_inner());
        let gemini_key_persisted = dk.gemini.as_deref().is_some_and(|s| !s.trim().is_empty());
        let openai_key_persisted = dk.openai.as_deref().is_some_and(|s| !s.trim().is_empty());
        let anthropic_key_persisted = dk.claude.as_deref().is_some_and(|s| !s.trim().is_empty());
        let def = self
            .default_backend
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
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
            default_provider: def.as_ref().map(|d| d.provider),
            default_model: def.as_ref().map(|d| d.model.clone()),
            default_local_name: def.as_ref().and_then(|d| d.local_name.clone()),
            default_local_endpoint: def.as_ref().and_then(|d| d.local_endpoint.clone()),
            optimizations: cfg.optimizations,
        }
    }

    fn persist_config(&self) -> Result<(), String> {
        let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
        let def = self
            .default_backend
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let persisted = PersistedCopilotConfig {
            default_backend: def,
            last_provider: Some(cfg.provider),
            last_model: Some(cfg.model.clone()),
            last_local_endpoint: Some(cfg.local_endpoint.clone()),
            last_local_name: cfg.active_local_name.clone(),
            trust_mode: Some(cfg.trust_mode),
            optimizations: cfg.optimizations,
        };
        drop(cfg);
        if let Some(parent) = self.config_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(&persisted).map_err(|e| e.to_string())?;
        fs::write(&self.config_file, json)
            .map_err(|e| format!("Failed to write {}: {e}", self.config_file.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.config_file, fs::Permissions::from_mode(0o600));
        }
        Ok(())
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
        let max_chars = {
            let cfg = self.config.read().unwrap_or_else(|e| e.into_inner());
            let base = (cfg.optimizations.tool_output_max_chars as usize).clamp(1_000, 64_000);
            if cfg.optimizations.adaptive_tool_truncation
                && cfg.provider == CopilotProvider::Local
                && cfg.local_context_window <= 32_768
            {
                (base / 2).max(2_000)
            } else {
                base
            }
        };

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
                        let truncated: String = content.chars().take(max_chars).collect();
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
                    return "Blocked: Cawpilot is in Monitored (read-only) mode. Switch to Managed mode with `/mode manage` to perform mutations.".into();
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
                    return "Blocked: Cawpilot is in Monitored (read-only) mode.".into();
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
                    return "Blocked: Cawpilot is in Monitored (read-only) mode.".into();
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
                let (ep, ctx_window, override_max) = {
                    let cfg = self.core.config.read().unwrap_or_else(|e| e.into_inner());
                    (
                        cfg.local_endpoint.clone(),
                        cfg.local_context_window.clamp(4_096, 1_000_000),
                        cfg.optimizations.local_max_tokens,
                    )
                };
                let max_tokens = if override_max > 0 {
                    override_max.clamp(512, ctx_window / 2)
                } else {
                    (ctx_window / 8).clamp(1_024, 8_192)
                };
                let mut m = Model::new(
                    provider,
                    id,
                    "openai-completions",
                    ctx_window,
                    max_tokens,
                    false,
                );
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
                    AssistantMessageEvent::ThinkingStart { .. } => {
                        if let Some(turn) = core
                            .active_turn
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clone()
                        {
                            let _ = turn.out_tx.send(ServerMsg::CopilotChunk {
                                request_id: turn.request_id,
                                delta: "<think>".to_string(),
                                done: false,
                            });
                        }
                    }
                    AssistantMessageEvent::ThinkingDelta { delta, .. } if !delta.is_empty() => {
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
                    AssistantMessageEvent::ThinkingEnd { .. } => {
                        if let Some(turn) = core
                            .active_turn
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clone()
                        {
                            let _ = turn.out_tx.send(ServerMsg::CopilotChunk {
                                request_id: turn.request_id,
                                delta: "</think>\n\n".to_string(),
                                done: false,
                            });
                        }
                    }
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
                                    delta: format!("\n[Cawpilot error: {msg}]"),
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
        Self::new_in_dir(local_hostname, crate::appdata::kaage_app_dir())
    }

    pub(crate) fn new_in_dir(local_hostname: String, kaage_dir: PathBuf) -> Arc<Self> {
        let env_provider = match std::env::var("KAAGE_COPILOT_PROVIDER")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "openai" => CopilotProvider::OpenAi,
            "claude" | "anthropic" => CopilotProvider::Claude,
            "local" | "ollama" => CopilotProvider::Local,
            _ => CopilotProvider::Gemini,
        };
        let env_model = std::env::var("KAAGE_COPILOT_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let env_trust_mode = match std::env::var("KAAGE_COPILOT_TRUST_MODE")
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
        let env_local_endpoint = std::env::var("KAAGE_COPILOT_LOCAL_ENDPOINT")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_LOCAL_ENDPOINT.to_string());

        let db_path = kaage_dir.join("memory.db");
        let durable_db_path = kaage_dir.join("copilot_durable.db");
        let config_file = kaage_dir.join("copilot_config.json");
        let keys_file = kaage_dir.join("copilot_keys.json");
        let local_llms_file = kaage_dir.join("copilot_local_llms.json");
        let saved_config: PersistedCopilotConfig = fs::read_to_string(&config_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let mut saved_keys: SavedApiKeys = fs::read_to_string(&keys_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let disk_keys = saved_keys.clone();
        let saved_local_llms: Vec<StoredLocalLlm> = fs::read_to_string(&local_llms_file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

        // Startup / resume backend precedence:
        // 1. Explicit Default Backend set in Copilot Settings (used on start & resume)
        // 2. Last LLM used for chatting / configured in Copilot
        // 3. Environment variables / built-in defaults
        let (provider, model, local_endpoint, mut active_local_name) =
            if let Some(ref def) = saved_config.default_backend {
                let ep = def
                    .local_endpoint
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| {
                        saved_config
                            .last_local_endpoint
                            .clone()
                            .filter(|s| !s.trim().is_empty())
                    })
                    .unwrap_or_else(|| env_local_endpoint.clone());
                let m = if def.model.trim().is_empty() {
                    def.provider.default_model().to_string()
                } else {
                    def.model.clone()
                };
                (def.provider, m, ep, def.local_name.clone())
            } else if let Some(last_prov) = saved_config.last_provider {
                let ep = saved_config
                    .last_local_endpoint
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| env_local_endpoint.clone());
                let m = saved_config
                    .last_model
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| last_prov.default_model().to_string());
                (last_prov, m, ep, saved_config.last_local_name.clone())
            } else {
                let m = env_model.unwrap_or_else(|| env_provider.default_model().to_string());
                (env_provider, m, env_local_endpoint, None)
            };

        let trust_mode = saved_config.trust_mode.unwrap_or(env_trust_mode);

        let memory = match MemoryStore::open(&db_path) {
            Ok(store) => {
                tracing::info!(
                    "Cawpilot FTS5 memory initialized at {}",
                    db_path.display()
                );
                Some(Arc::new(store))
            }
            Err(err) => {
                tracing::warn!("Cawpilot FTS5 memory disabled: {err}");
                None
            }
        };

        let mut local_context_window = DEFAULT_LOCAL_CTX_WINDOW;
        if let Some(matched) = saved_local_llms.iter().find(|e| {
            active_local_name
                .as_deref()
                .is_some_and(|n| e.name.eq_ignore_ascii_case(n))
                || normalize_openai_base_url(&e.url) == normalize_openai_base_url(&local_endpoint)
        }) {
            if active_local_name.is_none() {
                active_local_name = Some(matched.name.clone());
            }
            local_context_window = matched.context_window;
            if provider == CopilotProvider::Local && saved_keys.local.is_none() {
                saved_keys.local = matched.api_key.clone();
            }
        }

        let core = Arc::new(CopilotCore {
            config: RwLock::new(CopilotConfig {
                provider,
                model: model.clone(),
                trust_mode,
                target_node: local_hostname.clone(),
                flake_path,
                local_endpoint,
                local_context_window,
                active_local_name,
                optimizations: saved_config.optimizations,
            }),
            default_backend: RwLock::new(saved_config.default_backend),
            runtime_keys: RwLock::new(saved_keys),
            disk_keys: RwLock::new(disk_keys),
            local_endpoints: RwLock::new(saved_local_llms),
            config_file,
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
                    "Cawpilot durable SQLite store initialized at {}",
                    durable_db_path.display()
                );
                Arc::new(sql)
            }
            Err(err) => {
                tracing::warn!(
                    "Cawpilot durable SQLite store failed ({err}), falling back to in-memory storage"
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
                    tracing::warn!("Cawpilot durable harness report: {err}");
                })),
            },
        )
        .expect("open pi-durable harness for Cawpilot");

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

    pub fn list_memories(&self, query: Option<&str>) -> Vec<HostMemoryItem> {
        self.core
            .memory
            .as_ref()
            .map(|m| m.list(query, 100))
            .unwrap_or_default()
    }

    pub fn save_memory(
        &self,
        category: &str,
        content: &str,
        pinned: bool,
    ) -> Result<Vec<HostMemoryItem>, String> {
        let mem = self
            .core
            .memory
            .as_ref()
            .ok_or_else(|| "Memory store unavailable".to_string())?;
        mem.save_with_pin(category, content, pinned)?;
        Ok(mem.list(None, 100))
    }

    pub fn toggle_pin_memory(&self, id: i64, pinned: bool) -> Result<Vec<HostMemoryItem>, String> {
        let mem = self
            .core
            .memory
            .as_ref()
            .ok_or_else(|| "Memory store unavailable".to_string())?;
        mem.set_pinned(id, pinned)?;
        Ok(mem.list(None, 100))
    }

    pub fn delete_memory(&self, id: i64) -> Result<Vec<HostMemoryItem>, String> {
        let mem = self
            .core
            .memory
            .as_ref()
            .ok_or_else(|| "Memory store unavailable".to_string())?;
        mem.delete(id)?;
        Ok(mem.list(None, 100))
    }

    pub fn set_optimizations(&self, mut opts: CopilotOptimizations) -> Result<CopilotStatus, String> {
        opts.tool_output_max_chars = opts.tool_output_max_chars.clamp(1_000, 64_000);
        if opts.local_max_tokens > 0 {
            opts.local_max_tokens = opts.local_max_tokens.clamp(512, 32_768);
        }
        {
            let mut cfg = self.core.config.write().unwrap_or_else(|e| e.into_inner());
            cfg.optimizations = opts;
        }
        self.core.persist_config()?;
        Ok(self.status())
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
    /// If a default LLM backend is configured in Copilot Settings, activates it for the fresh session.
    pub fn clear_history(&self) -> Result<(), String> {
        let cancel = CancelToken::new();
        let root = self
            .harness
            .root(RootOptions::default(), &cancel)
            .map_err(|e| format!("Failed to open durable conversation: {e}"))?;
        root.reset(None, &cancel)
            .map_err(|e| format!("Failed to reset conversation: {e}"))?;
        let _ = root.wait_for_idle(&cancel);

        if let Some(def) = self
            .core
            .default_backend
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            let matched_key = if def.provider == CopilotProvider::Local {
                let target_base = def
                    .local_endpoint
                    .as_deref()
                    .map(normalize_openai_base_url);
                self.core
                    .local_endpoints
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .find(|e| {
                        def.local_name
                            .as_deref()
                            .is_some_and(|n| e.name.eq_ignore_ascii_case(n))
                            || target_base
                                .as_deref()
                                .is_some_and(|b| normalize_openai_base_url(&e.url) == b)
                    })
                    .and_then(|e| e.api_key.clone())
            } else {
                None
            };
            {
                let mut cfg = self.core.config.write().unwrap_or_else(|e| e.into_inner());
                cfg.provider = def.provider;
                cfg.model = def.model.clone();
                if let Some(ep) = def.local_endpoint.filter(|s| !s.trim().is_empty()) {
                    cfg.local_endpoint = ep;
                }
                if def.provider == CopilotProvider::Local {
                    cfg.active_local_name = def.local_name;
                }
            }
            if let Some(k) = matched_key {
                self.core
                    .runtime_keys
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .local = Some(k);
            }
            let _ = root.configure(
                &AgentChange {
                    model: Change::Set(ModelRef {
                        provider: provider_slug(def.provider).to_string(),
                        model_id: def.model,
                    }),
                    ..Default::default()
                },
                &cancel,
            );
            let _ = self.core.persist_config();
        }
        Ok(())
    }

    /// Pin or clear the default LLM backend used when starting or resuming Copilot sessions.
    pub fn set_default_backend(
        &self,
        provider: Option<CopilotProvider>,
        model: Option<String>,
        local_endpoint: Option<String>,
        local_name: Option<String>,
    ) -> Result<CopilotStatus, String> {
        if let Some(p) = provider {
            let clean_model = model
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| p.default_model().to_string());
            let clean_ep = local_endpoint
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let clean_name = local_name
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            let def = DefaultLlmConfig {
                provider: p,
                model: clean_model.clone(),
                local_endpoint: if p == CopilotProvider::Local {
                    clean_ep.clone()
                } else {
                    None
                },
                local_name: if p == CopilotProvider::Local {
                    clean_name.clone()
                } else {
                    None
                },
            };
            *self
                .core
                .default_backend
                .write()
                .unwrap_or_else(|e| e.into_inner()) = Some(def);

            {
                let mut cfg = self.core.config.write().unwrap_or_else(|e| e.into_inner());
                cfg.provider = p;
                cfg.model = clean_model;
                if let Some(ep) = clean_ep {
                    cfg.local_endpoint = ep;
                }
                if p == CopilotProvider::Local && clean_name.is_some() {
                    cfg.active_local_name = clean_name;
                }
            }
        } else {
            *self
                .core
                .default_backend
                .write()
                .unwrap_or_else(|e| e.into_inner()) = None;
        }
        self.core.persist_config()?;
        Ok(self.status())
    }

    pub fn upsert_local_llm(
        &self,
        name: String,
        url: String,
        api_key: Option<String>,
        context_window: Option<u64>,
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
        let ctx_win = context_window
            .unwrap_or(DEFAULT_LOCAL_CTX_WINDOW)
            .clamp(4_096, 1_000_000);

        let effective_key = {
            let mut list = self
                .core
                .local_endpoints
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = list.iter_mut().find(|e| e.name.eq_ignore_ascii_case(&name)) {
                existing.name = name.clone();
                existing.url = url.clone();
                existing.context_window = ctx_win;
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
                    context_window: ctx_win,
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
            cfg.local_context_window = ctx_win;
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
        let _ = self.core.persist_config();
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
            cfg.local_context_window = found.context_window;
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
        let _ = self.core.persist_config();
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
        {
            let mut def = self
                .core
                .default_backend
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(d) = def.as_mut()
                && d.local_name
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(name.trim()))
            {
                d.local_name = None;
            }
        }
        let _ = self.core.persist_config();
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
                context_window: None,
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
                    let (models, raw_ctx) = parse_models_response_with_ctx(&body);
                    let auto_detect = self
                        .core
                        .config
                        .read()
                        .unwrap_or_else(|e| e.into_inner())
                        .optimizations
                        .auto_detect_context;
                    let detected_ctx = if auto_detect { raw_ctx } else { None };
                    tracing::info!(
                        "copilot probe <- {probe_url} (req_bytes=0, res_bytes={res_bytes}, models={}, ctx={:?})",
                        models.len(),
                        detected_ctx
                    );
                    let msg = if models.is_empty() {
                        format!("Connected to {probe_url} (0 models returned)")
                    } else if let Some(ctx) = detected_ctx {
                        format!(
                            "Connected — {} model(s) available (detected {}K ctx)",
                            models.len(),
                            ctx / 1024
                        )
                    } else {
                        format!("Connected — {} model(s) available", models.len())
                    };
                    return ServerMsg::LocalLlmTestResult {
                        ok: true,
                        url: trimmed,
                        message: msg,
                        models,
                        context_window: detected_ctx,
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
            context_window: None,
        }
    }

    /// Test the currently active Copilot provider/model connection and return a formatted
    /// Markdown summary with model metadata, endpoint, latency, and capabilities.
    pub fn test_active_connection(&self) -> (bool, String) {
        let started = std::time::Instant::now();
        let status = self.status();
        let (local_ctx, opts) = {
            let cfg = self.core.config.read().unwrap_or_else(|e| e.into_inner());
            (cfg.local_context_window, cfg.optimizations)
        };
        let api_key = self.core.effective_api_key(status.provider);

        if status.provider != CopilotProvider::Local && api_key.is_none() {
            let env_name = status.provider.env_var_name();
            return (
                false,
                format!(
                    "### Connection Test — Failed\n\
                     - **Provider:** {}\n\
                     - **Model:** `{}`\n\
                     - **Error:** Missing `{env_name}`. Configure it in `⚙ Backends & Keys`.",
                    status.provider.label(),
                    status.model,
                ),
            );
        }

        if status.provider == CopilotProvider::Local {
            let res = self.test_local_llm(status.local_endpoint.clone(), api_key);
            let elapsed_ms = started.elapsed().as_millis();
            return match res {
                ServerMsg::LocalLlmTestResult {
                    ok,
                    url,
                    message,
                    models,
                    context_window,
                } => {
                    let max_tok = if opts.local_max_tokens > 0 {
                        opts.local_max_tokens
                    } else {
                        (local_ctx / 8).clamp(1_024, 8_192)
                    };
                    let model_found = models.iter().any(|m| m == &status.model);
                    let models_preview = if models.is_empty() {
                        "_none reported_".to_string()
                    } else {
                        models
                            .iter()
                            .take(10)
                            .map(|m| format!("`{m}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    let detected_str = context_window
                        .map(|c| format!("{}K (detected)", c / 1024))
                        .unwrap_or_else(|| "not reported".into());
                    let summary = format!(
                        "### Connection Test — {}\n\
                         - **Provider:** {} {}\n\
                         - **Endpoint:** `{}`\n\
                         - **Active Model:** `{}`{}\n\
                         - **Context Window:** {}K configured · {} · max output {} tokens\n\
                         - **Latency:** {} ms\n\
                         - **Status:** {}\n\
                         - **Available Models ({}):** {}",
                        if ok { "OK" } else { "Failed" },
                        status.provider.label(),
                        status
                            .active_local_name
                            .as_deref()
                            .map(|n| format!("(`{n}`)"))
                            .unwrap_or_default(),
                        normalize_openai_base_url(&url),
                        status.model,
                        if ok && !models.is_empty() && !model_found {
                            " *(warning: not in server's model list)*"
                        } else {
                            ""
                        },
                        local_ctx / 1024,
                        detected_str,
                        max_tok,
                        elapsed_ms,
                        message,
                        models.len(),
                        models_preview,
                    );
                    (ok, summary)
                }
                _ => (false, "Unexpected test result".into()),
            };
        }

        let key = api_key.unwrap_or_default();
        let (probe_url, is_anthropic) = match status.provider {
            CopilotProvider::Gemini => (
                format!("{GEMINI_BASE_URL}/models/{}", status.model),
                false,
            ),
            CopilotProvider::OpenAi => (
                format!("{OPENAI_BASE_URL}/models/{}", status.model),
                false,
            ),
            CopilotProvider::Claude => (
                format!("{ANTHROPIC_BASE_URL}/v1/models/{}", status.model),
                true,
            ),
            CopilotProvider::Local => unreachable!(),
        };

        let cancel = CancelToken::new();
        let mut req = PiHttpRequest::new("GET", probe_url.clone())
            .timeout(Some(std::time::Duration::from_secs(10)));
        if is_anthropic {
            req = req
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01");
        } else {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        match pi_http_send(&req, Some(&cancel)) {
            Ok(resp) if resp.ok() => {
                let body = resp.text().unwrap_or_default();
                let elapsed_ms = started.elapsed().as_millis();
                tracing::info!(
                    "copilot test <- {probe_url} (req_bytes=0, res_bytes={}, elapsed_ms={elapsed_ms})",
                    body.len()
                );
                let meta: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let mut extra = Vec::new();
                if let Some(dn) = meta.get("display_name").and_then(Value::as_str) {
                    extra.push(format!("- **Display Name:** {dn}"));
                }
                if let Some(owner) = meta.get("owned_by").and_then(Value::as_str) {
                    extra.push(format!("- **Owned By:** `{owner}`"));
                }
                if let Some(created) = meta.get("created_at").and_then(Value::as_str) {
                    extra.push(format!("- **Created:** `{created}`"));
                }
                let extra_block = if extra.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", extra.join("\n"))
                };
                (
                    true,
                    format!(
                        "### Connection Test — OK\n\
                         - **Provider:** {}\n\
                         - **Model:** `{}`\n\
                         - **Endpoint:** `{probe_url}`\n\
                         - **Latency:** {elapsed_ms} ms\n\
                         - **Trust Mode:** `{}`{extra_block}",
                        status.provider.label(),
                        status.model,
                        status.trust_mode.as_str(),
                    ),
                )
            }
            Ok(resp) => {
                let code = resp.status;
                let snippet: String = resp.text().unwrap_or_default().chars().take(240).collect();
                let elapsed_ms = started.elapsed().as_millis();
                tracing::warn!(
                    "copilot test <- {probe_url} (status={code}, elapsed_ms={elapsed_ms})"
                );
                (
                    false,
                    format!(
                        "### Connection Test — Failed (HTTP {code})\n\
                         - **Provider:** {}\n\
                         - **Model:** `{}`\n\
                         - **Endpoint:** `{probe_url}`\n\
                         - **Latency:** {elapsed_ms} ms\n\
                         - **Response:** `{}`",
                        status.provider.label(),
                        status.model,
                        snippet.trim(),
                    ),
                )
            }
            Err(err) => {
                let elapsed_ms = started.elapsed().as_millis();
                tracing::warn!("copilot test <- {probe_url} (error={err}, elapsed_ms={elapsed_ms})");
                (
                    false,
                    format!(
                        "### Connection Test — Failed\n\
                         - **Provider:** {}\n\
                         - **Model:** `{}`\n\
                         - **Endpoint:** `{probe_url}`\n\
                         - **Error:** `{err}`",
                        status.provider.label(),
                        status.model,
                    ),
                )
            }
        }
    }

    /// List available model IDs on the currently active Copilot LLM connection
    /// (for `/model` autocomplete).
    pub fn list_active_models(&self) -> Vec<String> {
        let status = self.status();
        let api_key = self.core.effective_api_key(status.provider);
        let mut models: Vec<String> = match status.provider {
            CopilotProvider::Local => {
                match self.test_local_llm(status.local_endpoint.clone(), api_key) {
                    ServerMsg::LocalLlmTestResult { models, .. } => models,
                    _ => Vec::new(),
                }
            }
            CopilotProvider::Gemini => {
                let mut found = Vec::new();
                if let Some(key) = api_key {
                    let req = PiHttpRequest::new("GET", format!("{GEMINI_BASE_URL}/models"))
                        .header("Authorization", format!("Bearer {key}"))
                        .timeout(Some(std::time::Duration::from_secs(8)));
                    if let Ok(resp) = pi_http_send(&req, Some(&CancelToken::new()))
                        && resp.ok()
                    {
                        let body = resp.text().unwrap_or_default();
                        found = parse_models_response_with_ctx(&body)
                            .0
                            .into_iter()
                            .map(|m| m.strip_prefix("models/").unwrap_or(&m).to_string())
                            .collect();
                    }
                }
                if found.is_empty() {
                    found = vec![
                        "gemini-2.5-flash".into(),
                        "gemini-2.5-pro".into(),
                        "gemini-2.0-flash".into(),
                    ];
                }
                found
            }
            CopilotProvider::OpenAi => {
                let mut found = Vec::new();
                if let Some(key) = api_key {
                    let req = PiHttpRequest::new("GET", format!("{OPENAI_BASE_URL}/models"))
                        .header("Authorization", format!("Bearer {key}"))
                        .timeout(Some(std::time::Duration::from_secs(8)));
                    if let Ok(resp) = pi_http_send(&req, Some(&CancelToken::new()))
                        && resp.ok()
                    {
                        let body = resp.text().unwrap_or_default();
                        found = parse_models_response_with_ctx(&body)
                            .0
                            .into_iter()
                            .filter(|m| {
                                m.starts_with("gpt-")
                                    || m.starts_with("o1")
                                    || m.starts_with("o3")
                                    || m.starts_with("o4")
                                    || m.starts_with("chatgpt-")
                            })
                            .collect();
                    }
                }
                if found.is_empty() {
                    found = vec![
                        "gpt-4o".into(),
                        "gpt-4o-mini".into(),
                        "gpt-4.1".into(),
                        "o3-mini".into(),
                    ];
                }
                found
            }
            CopilotProvider::Claude => {
                let mut found = Vec::new();
                if let Some(key) = api_key {
                    let req = PiHttpRequest::new("GET", format!("{ANTHROPIC_BASE_URL}/v1/models"))
                        .header("x-api-key", key)
                        .header("anthropic-version", "2023-06-01")
                        .timeout(Some(std::time::Duration::from_secs(8)));
                    if let Ok(resp) = pi_http_send(&req, Some(&CancelToken::new()))
                        && resp.ok()
                    {
                        let body = resp.text().unwrap_or_default();
                        found = parse_models_response_with_ctx(&body).0;
                    }
                }
                if found.is_empty() {
                    found = vec![
                        "claude-sonnet-4-20250514".into(),
                        "claude-3-7-sonnet-20250219".into(),
                        "claude-3-5-haiku-20241022".into(),
                    ];
                }
                found
            }
        };

        if !status.model.trim().is_empty() && !models.iter().any(|m| m == &status.model) {
            models.insert(0, status.model);
        }
        models
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
        let _ = self.core.persist_config();
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
            // Persist the active LLM so Copilot remembers the last LLM used for chatting across restarts.
            let _ = mgr.core.persist_config();
            let status = mgr.status();
            let api_key = mgr.core.effective_api_key(status.provider);
            if status.provider != CopilotProvider::Local && api_key.is_none() {
                let env_name = status.provider.env_var_name();
                let _ = out_tx.send(ServerMsg::CopilotChunk {
                    request_id,
                    delta: format!(
                        "Cawpilot is missing `{env_name}` for `{}`. Add the key in the setup banner above or in `kaage-settings → Cawpilot (AI)`.",
                        status.provider.label()
                    ),
                    done: true,
                });
                return;
            }

            let mut recalled_block = String::new();
            if let Some(ref mem) = mgr.core.memory {
                let pinned = mem.pinned(10);
                let mut seen_contents: Vec<String> = Vec::new();
                if !pinned.is_empty() {
                    recalled_block.push_str("\nPinned host memories (always active):\n");
                    for item in &pinned {
                        recalled_block.push_str(&format!("- [{}] {}\n", item.category, item.content));
                        seen_contents.push(item.content.clone());
                    }
                }
                let hits = mem.search(&prompt, 5);
                let extra: Vec<(String, String)> = hits
                    .into_iter()
                    .filter(|(_, txt)| !seen_contents.contains(txt))
                    .collect();
                if !extra.is_empty() {
                    recalled_block.push_str("\nRecalled host memories:\n");
                    for (cat, txt) in &extra {
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
                "You are Cawpilot, an expert Linux & NixOS operations assistant embedded inside kaage on host `{}`.\n\
                 Active OS: {} (NixOS: {}, generation: {:?}, current-system: {:?}).\n\
                 Configured flake path: {}.\n\
                 Target node: {} · Trust mode: {}.\n\
                 Use your diagnostic tools to inspect real host state before answering. Be concise, accurate, and cite exact unit names, metrics, and file paths. \
                 When you learn durable host facts (port mappings, GPU/VRAM splits, user preferences, or incident fixes), save them with `save_host_memory`.{recalled_block}",
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
                        // Do not reuse client-local sequential `request_id`s (1, 2, 3...) as
                        // pi-durable idempotency keys, or prompts after a browser refresh would
                        // be silently deduplicated against prior turns in SQLite.
                        request_id: None,
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
                        format!("\n[Cawpilot error: {err}]")
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
                        delta: format!("\n[Cawpilot worker error: {join_err}]"),
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
                                "text": "No active Cawpilot session."
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

/// Parse model IDs and optional context window length from OpenAI `/v1/models`
/// (`{ data: [{ id, max_model_len / context_length / meta.n_ctx_train }] }`) or Ollama `/api/tags`.
pub fn parse_models_response_with_ctx(body: &str) -> (Vec<String>, Option<u64>) {
    let Ok(val) = serde_json::from_str::<Value>(body) else {
        return (Vec::new(), None);
    };
    let mut out = Vec::new();
    let mut detected_ctx: Option<u64> = None;
    if let Some(arr) = val.get("data").and_then(Value::as_array) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(Value::as_str)
                && !id.trim().is_empty()
            {
                out.push(id.trim().to_string());
            }
            if detected_ctx.is_none() {
                detected_ctx = item
                    .get("max_model_len")
                    .or_else(|| item.get("context_length"))
                    .or_else(|| item.get("n_ctx"))
                    .or_else(|| item.get("meta").and_then(|m| m.get("n_ctx_train")))
                    .and_then(Value::as_u64)
                    .filter(|&n| n >= 2_048);
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
    (out, detected_ctx)
}

#[cfg(test)]
pub fn parse_models_response(body: &str) -> Vec<String> {
    parse_models_response_with_ctx(body).0
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

        let pinned_id = store
            .save_with_pin("preference", "Always prefer nixos-rebuild dry-build first", true)
            .unwrap();
        let pinned_list = store.pinned(10);
        assert_eq!(pinned_list.len(), 1);
        assert_eq!(pinned_list[0].id, pinned_id);
        assert!(pinned_list[0].pinned);

        let all = store.list(None, 20);
        assert_eq!(all.len(), 3);
        assert!(all[0].pinned, "pinned memories should sort first");

        store.set_pinned(pinned_id, false).unwrap();
        assert!(store.pinned(10).is_empty());
        store.delete(pinned_id).unwrap();
        assert_eq!(store.list(None, 20).len(), 2);

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
                "content": [{"type": "text", "text": "Cawpilot durable reply."}],
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
                    AssistantBlock::Text(t) => t.text.contains("Cawpilot durable reply."),
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
        let openai_json = r#"{"object":"list","data":[{"id":"qwen3:30b","max_model_len":65536},{"id":"llama3.3:70b"},{"id":"qwen3:30b"}]}"#;
        let (models, ctx) = parse_models_response_with_ctx(openai_json);
        assert_eq!(
            models,
            vec!["llama3.3:70b".to_string(), "qwen3:30b".to_string()]
        );
        assert_eq!(ctx, Some(65_536));

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

    #[test]
    fn remembers_last_used_llm_and_default_backend_across_restart() {
        let dir = std::env::temp_dir().join(format!(
            "kaage_copilot_cfg_persist_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        // 1. Without a pinned default, switching LLM persists as last-used and resumes on restart.
        {
            let mgr = CopilotManager::new_in_dir("test-host".into(), dir.clone());
            mgr.upsert_local_llm(
                "Unsloth".into(),
                "http://127.0.0.1:8888/v1".into(),
                Some("secret-key".into()),
                Some(65_536),
                true,
            )
            .unwrap();
            let st = mgr.update_config(
                Some(CopilotProvider::Local),
                Some("gemma-4-test".into()),
                Some(TrustMode::Monitored),
                None,
                None,
            );
            assert_eq!(st.provider, CopilotProvider::Local);
            assert_eq!(st.model, "gemma-4-test");
            assert_eq!(st.active_local_name.as_deref(), Some("Unsloth"));
        }

        {
            let mgr2 = CopilotManager::new_in_dir("test-host".into(), dir.clone());
            let st2 = mgr2.status();
            assert_eq!(st2.provider, CopilotProvider::Local);
            assert_eq!(st2.model, "gemma-4-test");
            assert_eq!(st2.local_endpoint, "http://127.0.0.1:8888/v1");
            assert_eq!(st2.active_local_name.as_deref(), Some("Unsloth"));
            assert_eq!(st2.trust_mode, TrustMode::Monitored);
            assert_eq!(st2.default_provider, None);

            // 2. Pinning a default backend uses it on start/resume and after clear_history even if
            //    the user temporarily switched to another LLM.
            let st_def = mgr2
                .set_default_backend(
                    Some(CopilotProvider::Local),
                    Some("gemma-4-test".into()),
                    Some("http://127.0.0.1:8888/v1".into()),
                    Some("Unsloth".into()),
                )
                .unwrap();
            assert_eq!(st_def.default_provider, Some(CopilotProvider::Local));
            assert_eq!(st_def.default_model.as_deref(), Some("gemma-4-test"));

            // Temporarily switch to OpenAI
            let st_tmp = mgr2.update_config(
                Some(CopilotProvider::OpenAi),
                Some("gpt-4o".into()),
                None,
                None,
                None,
            );
            assert_eq!(st_tmp.provider, CopilotProvider::OpenAi);

            // Clearing history restores the configured default backend
            mgr2.clear_history().unwrap();
            let st_cleared = mgr2.status();
            assert_eq!(st_cleared.provider, CopilotProvider::Local);
            assert_eq!(st_cleared.model, "gemma-4-test");

            // Switch to Claude right before restart; pinned default should still win on start/resume
            mgr2.update_config(
                Some(CopilotProvider::Claude),
                Some("claude-sonnet-4-20250514".into()),
                None,
                None,
                None,
            );
        }

        {
            let mgr3 = CopilotManager::new_in_dir("test-host".into(), dir.clone());
            let st3 = mgr3.status();
            assert_eq!(st3.provider, CopilotProvider::Local);
            assert_eq!(st3.model, "gemma-4-test");
            assert_eq!(st3.default_provider, Some(CopilotProvider::Local));

            // 3. Clearing the default falls back to last-used LLM on next restart.
            mgr3.set_default_backend(None, None, None, None).unwrap();
            mgr3.update_config(
                Some(CopilotProvider::OpenAi),
                Some("gpt-4o-mini".into()),
                None,
                None,
                None,
            );
        }

        {
            let mgr4 = CopilotManager::new_in_dir("test-host".into(), dir.clone());
            let st4 = mgr4.status();
            assert_eq!(st4.provider, CopilotProvider::OpenAi);
            assert_eq!(st4.model, "gpt-4o-mini");
            assert_eq!(st4.default_provider, None);
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
