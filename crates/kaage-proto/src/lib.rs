use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum UnitScope {
    #[default]
    System,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitAction {
    Start,
    Stop,
    Restart,
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NixosInfo {
    pub is_nixos: bool,
    pub pretty_name: String,
    pub version: String,
    pub current_system: Option<String>,
    pub generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemdUnitInfo {
    pub name: String,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: String,
    pub enabled: bool,
    pub running: bool,
    pub nixos_managed: bool,
    pub scope: UnitScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemdUnitStatus {
    pub name: String,
    pub scope: UnitScope,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: String,
    pub enabled: bool,
    pub running: bool,
    pub fragment_path: String,
    pub nix_store_path: Option<String>,
    pub nixos_managed: bool,
    pub main_pid: u32,
    pub memory_current: Option<u64>,
    pub active_enter_timestamp_usec: u64,
    pub result: String,
    pub status_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostMetrics {
    pub hostname: String,
    pub cpu_percent: f32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessSummary {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub mem_mb: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub utilization_percent: Option<f32>,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub temperature_c: Option<f32>,
    pub power_draw_w: Option<f32>,
    pub power_limit_w: Option<f32>,
    pub source: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiJobInfo {
    pub id: String,
    pub title: String,
    pub state: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ComfyUiStatus {
    pub reachable: bool,
    pub url: String,
    pub version: String,
    pub running_jobs: Vec<AiJobInfo>,
    pub pending_jobs: Vec<AiJobInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UnslothStatus {
    pub reachable: bool,
    pub url: String,
    pub service_name: String,
    pub jobs: Vec<AiJobInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListeningPortInfo {
    pub port: u16,
    pub bind_addr: String,
    pub process: String,
    pub pid: Option<u32>,
    pub label: String,
    pub tailscale_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailscaleServeEntry {
    pub https_url: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailscalePeerInfo {
    pub hostname: String,
    pub dns_name: String,
    pub os: String,
    pub tailscale_ip: String,
    pub online: bool,
    pub active: bool,
    pub relay: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TailscaleInfo {
    pub available: bool,
    pub backend_state: String,
    pub version: String,
    pub hostname: String,
    pub dns_name: String,
    pub tailscale_ips: Vec<String>,
    pub relay: String,
    pub serves: Vec<TailscaleServeEntry>,
    pub peers: Vec<TailscalePeerInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub cpu_percent: f32,
    pub cpu_cores: Vec<f32>,
    pub cpu_brand: String,
    pub load_avg: [f64; 3],
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub swap_used_mb: u64,
    pub swap_total_mb: u64,
    pub top_processes: Vec<ProcessSummary>,
    pub gpus: Vec<GpuInfo>,
    pub comfyui: ComfyUiStatus,
    pub unsloth: UnslothStatus,
    pub listening_ports: Vec<ListeningPortInfo>,
    pub tailscale: TailscaleInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    Directory,
    Document,
    Markdown,
    Image,
    Video,
    Audio,
    Archive,
    ModelWeights, // .safetensors, .gguf, .ckpt, .onnx
    Code,
    Installer, // .deb, .rpm, .AppImage, .nix
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    pub modified_ts: i64,
    pub mode: u32,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub symlink_broken: bool,
    /// e.g. Some("comfyui.service") if a systemd unit or process holds an open fd
    pub held_by_unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDirStatus {
    pub branch: String,
    pub dirty_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirListing {
    pub pane_id: u64,
    pub path: String,
    pub git: Option<GitDirStatus>,
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDef {
    pub name: String,
    pub items: Vec<WorkspaceItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceItem {
    pub label: String,
    pub path: String,
    pub kind: FileKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewContent {
    Text {
        content: String,
        is_markdown: bool,
        truncated: bool,
    },
    ArchiveEntries {
        entries: Vec<(String, u64)>,
    },
    ModelHeader {
        format: String,
        metadata: Vec<(String, String)>,
    },
    ImageBytes {
        mime: String,
        bytes: Vec<u8>,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOp {
    Copy {
        sources: Vec<String>,
        dest_dir: String,
    },
    Move {
        sources: Vec<String>,
        dest_dir: String,
    },
    Rename {
        path: String,
        new_name: String,
    },
    Trash {
        paths: Vec<String>,
    },
    Mkdir {
        parent: String,
        name: String,
    },
    WriteText {
        path: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    ListUnits,
    RefreshDashboard,
    GetUnitStatus {
        unit: String,
        scope: UnitScope,
    },
    ControlUnit {
        unit: String,
        scope: UnitScope,
        action: UnitAction,
    },
    ListContainers,
    ControlContainer {
        id: String,
        action: ContainerAction,
    },
    SpawnPty {
        session_id: u32,
        cols: u16,
        rows: u16,
        cmd: Option<Vec<String>>,
    },
    PtyInput {
        session_id: u32,
        data: Vec<u8>,
    },
    ResizePty {
        session_id: u32,
        cols: u16,
        rows: u16,
    },
    ClosePty {
        session_id: u32,
    },
    ListDir {
        pane_id: u64,
        path: String,
    },
    PerformFileOp {
        pane_id: u64,
        op: FileOp,
    },
    UndoLastFileOp {
        pane_id: u64,
    },
    PreviewFile {
        pane_id: u64,
        path: String,
    },
    SearchFiles {
        query_id: u64,
        root: String,
        pattern: String,
        search_contents: bool,
    },
    ListWorkspaces,
    SaveWorkspace(WorkspaceDef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    NixosInfo(NixosInfo),
    Metrics(HostMetrics),
    Dashboard(DashboardSnapshot),
    UnitsSnapshot(Vec<SystemdUnitInfo>),
    UnitStatus(SystemdUnitStatus),
    ContainersSnapshot(Vec<ContainerInfo>),
    ActionFeedback {
        target: String,
        ok: bool,
        message: String,
    },
    PtyOutput {
        session_id: u32,
        data: Vec<u8>,
    },
    PtyExited {
        session_id: u32,
    },
    DirSnapshot(DirListing),
    FilePreview {
        pane_id: u64,
        path: String,
        preview: PreviewContent,
    },
    SearchResults {
        query_id: u64,
        matches: Vec<FileEntry>,
        done: bool,
    },
    WorkspacesSnapshot(Vec<WorkspaceDef>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_msg_roundtrip() {
        let messages = vec![
            ClientMsg::ListUnits,
            ClientMsg::GetUnitStatus {
                unit: "kaaged.service".into(),
                scope: UnitScope::System,
            },
            ClientMsg::ControlUnit {
                unit: "tailscaled.service".into(),
                scope: UnitScope::System,
                action: UnitAction::Restart,
            },
            ClientMsg::ControlUnit {
                unit: "acedop.service".into(),
                scope: UnitScope::User,
                action: UnitAction::Start,
            },
            ClientMsg::ListContainers,
            ClientMsg::ControlContainer {
                id: "abc123def456".into(),
                action: ContainerAction::Stop,
            },
            ClientMsg::SpawnPty {
                session_id: 7,
                cols: 120,
                rows: 40,
                cmd: Some(vec!["journalctl".into(), "-u".into(), "kaaged.service".into()]),
            },
            ClientMsg::PtyInput {
                session_id: 7,
                data: b"ls -la\r".to_vec(),
            },
            ClientMsg::ResizePty {
                session_id: 7,
                cols: 80,
                rows: 24,
            },
            ClientMsg::ClosePty { session_id: 7 },
        ];

        for msg in messages {
            let bytes = postcard::to_allocvec(&msg).expect("serialize ClientMsg");
            let decoded: ClientMsg = postcard::from_bytes(&bytes).expect("deserialize ClientMsg");
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn server_msg_roundtrip() {
        let messages = vec![
            ServerMsg::NixosInfo(NixosInfo {
                is_nixos: true,
                pretty_name: "NixOS 26.11 (Zokor)".into(),
                version: "26.11 (Zokor)".into(),
                current_system: Some("/nix/store/abc-nixos-system-aihole".into()),
                generation: Some(42),
            }),
            ServerMsg::Metrics(HostMetrics {
                hostname: "aihole".into(),
                cpu_percent: 42.5,
                mem_used_mb: 8192,
                mem_total_mb: 65536,
                uptime_secs: 123_456,
            }),
            ServerMsg::UnitsSnapshot(vec![SystemdUnitInfo {
                name: "acedop.service".into(),
                description: "aceDoP Studio".into(),
                load_state: "loaded".into(),
                active_state: "active".into(),
                sub_state: "running".into(),
                unit_file_state: "enabled".into(),
                enabled: true,
                running: true,
                nixos_managed: true,
                scope: UnitScope::User,
            }]),
            ServerMsg::UnitStatus(SystemdUnitStatus {
                name: "kaaged.service".into(),
                scope: UnitScope::System,
                description: "kaage daemon".into(),
                load_state: "loaded".into(),
                active_state: "active".into(),
                sub_state: "running".into(),
                unit_file_state: "enabled".into(),
                enabled: true,
                running: true,
                fragment_path: "/etc/systemd/system/kaaged.service".into(),
                nix_store_path: Some("/nix/store/xyz-unit-kaaged.service/kaaged.service".into()),
                nixos_managed: true,
                main_pid: 1234,
                memory_current: Some(4_194_304),
                active_enter_timestamp_usec: 1_700_000_000_000_000,
                result: "success".into(),
                status_text: "● kaaged.service - kaage daemon".into(),
            }),
            ServerMsg::ContainersSnapshot(vec![ContainerInfo {
                id: "1234567890ab".into(),
                name: "postgres".into(),
                image: "postgres:16".into(),
                state: "running".into(),
                status: "Up 3 hours".into(),
            }]),
            ServerMsg::ActionFeedback {
                target: "acedop.service".into(),
                ok: true,
                message: "OK".into(),
            },
            ServerMsg::PtyOutput {
                session_id: 1,
                data: b"\x1b[32mhello\x1b[0m\r\n".to_vec(),
            },
            ServerMsg::PtyExited { session_id: 1 },
        ];

        for msg in messages {
            let bytes = postcard::to_allocvec(&msg).expect("serialize ServerMsg");
            let decoded: ServerMsg = postcard::from_bytes(&bytes).expect("deserialize ServerMsg");
            assert_eq!(msg, decoded);
        }
    }
}
