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
pub struct SystemdUnitInfo {
    pub name: String,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub scope: UnitScope,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    ListUnits,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Metrics(HostMetrics),
    UnitsSnapshot(Vec<SystemdUnitInfo>),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_msg_roundtrip() {
        let messages = vec![
            ClientMsg::ListUnits,
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
                scope: UnitScope::User,
            }]),
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
