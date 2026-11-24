use crate::dom_overlay::CopilotWindow;
use crate::file_tracker::TrackerWindow;
use crate::help::HelpWindow;
use crate::inspectors::{ScopeFilter, StateFilter};
use crate::settings::SettingsWindow;
use crate::terminal::TerminalPane;
use crate::theme::{FontPreset, ThemeMode, UserSettings, preset_palette};
use crate::vitals::VitalsWindow;
use crate::wm::{BeTab, BeWindowGroup, BeWindowManager, PaneKind, TilingLayout};
use egui::{Pos2, Vec2};
use kaage_proto::ClientMsg;
use serde::{Deserialize, Serialize};

const STORAGE_KEY: &str = "kaage_ui_session_v1";
const LAYOUTS_STORAGE_KEY: &str = "kaage_ui_layouts_v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SavedPaneKind {
    Terminal,
    SystemdInspector,
    DockerInspector,
    SystemMonitor,
    AiJobs,
    HttpPorts,
    TailscalePanel,
    TailscaleServices,
    FileTracker,
    Vitals,
    Help,
    Settings,
    Copilot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedTab {
    pub title: String,
    pub kind: SavedPaneKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedGroup {
    pub workspace: usize,
    pub floating: bool,
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub active_tab: usize,
    pub shaded: bool,
    pub tabs: Vec<SavedTab>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedSession {
    pub active_workspace: usize,
    pub tracker_visible: bool,
    pub tracker_wide: bool,
    pub show_wallpaper: bool,
    pub theme_mode_id: u8,
    pub font_preset_id: u8,
    pub ui_scale: f32,
    pub base_font_size: f32,
    pub terminal_font_size: f32,
    pub groups: Vec<SavedGroup>,
}

impl SavedSession {
    pub fn capture(
        wm: &BeWindowManager,
        tracker_visible: bool,
        tracker_wide: bool,
        show_wallpaper: bool,
        settings: UserSettings,
    ) -> Self {
        let groups = wm
            .groups
            .iter()
            .filter(|g| !g.tabs.is_empty())
            .map(|g| SavedGroup {
                workspace: g.workspace,
                floating: g.floating,
                pos: [g.pos.x.round(), g.pos.y.round()],
                size: [g.size.x.round(), g.size.y.round()],
                active_tab: g.active_tab.min(g.tabs.len().saturating_sub(1)),
                shaded: g.shaded,
                tabs: g
                    .tabs
                    .iter()
                    .map(|t| SavedTab {
                        title: t.title.clone(),
                        kind: match t.kind {
                            PaneKind::Terminal(_) => SavedPaneKind::Terminal,
                            PaneKind::SystemdInspector { .. } => SavedPaneKind::SystemdInspector,
                            PaneKind::DockerInspector { .. } => SavedPaneKind::DockerInspector,
                            PaneKind::SystemMonitor => SavedPaneKind::SystemMonitor,
                            PaneKind::AiJobs => SavedPaneKind::AiJobs,
                            PaneKind::HttpPorts { .. } => SavedPaneKind::HttpPorts,
                            PaneKind::TailscalePanel => SavedPaneKind::TailscalePanel,
                            PaneKind::TailscaleServices { .. } => SavedPaneKind::TailscaleServices,
                            PaneKind::FileTracker(_) => SavedPaneKind::FileTracker,
                            PaneKind::Vitals(_) => SavedPaneKind::Vitals,
                            PaneKind::Help(_) => SavedPaneKind::Help,
                            PaneKind::Settings(_) => SavedPaneKind::Settings,
                            PaneKind::Copilot(_) => SavedPaneKind::Copilot,
                        },
                    })
                    .collect(),
            })
            .collect();

        Self {
            active_workspace: wm.active_workspace,
            tracker_visible,
            tracker_wide,
            show_wallpaper,
            theme_mode_id: settings.theme_mode as u8,
            font_preset_id: match settings.font_preset {
                FontPreset::IbmPlex => 0,
                FontPreset::IbmPlexMonoAll => 1,
                FontPreset::EguiDefault => 2,
            },
            ui_scale: settings.ui_scale,
            base_font_size: settings.base_font_size,
            terminal_font_size: settings.terminal_font_size,
            groups,
        }
    }

    pub fn to_user_settings(&self) -> UserSettings {
        let theme_mode = if self.theme_mode_id == ThemeMode::RetroBeos as u8 {
            ThemeMode::RetroBeos
        } else {
            ThemeMode::ObsidianAmber
        };
        let font_preset = match self.font_preset_id {
            1 => FontPreset::IbmPlexMonoAll,
            2 => FontPreset::EguiDefault,
            _ => FontPreset::IbmPlex,
        };
        UserSettings {
            theme_mode,
            palette: preset_palette(theme_mode),
            font_preset,
            ui_scale: self.ui_scale.clamp(0.85, 1.40),
            base_font_size: self.base_font_size.clamp(10.0, 18.0),
            terminal_font_size: self.terminal_font_size.clamp(10.0, 22.0),
        }
    }

    /// Reconstruct `BeWindowManager` groups from a saved session and return initial `ClientMsg`
    /// requests (PTY spawns, Tracker directory listings, Vitals snapshots) to send once connected.
    pub fn restore_wm(
        &self,
        wm: &mut BeWindowManager,
        next_pty_id: &mut u32,
        next_tracker_pane_id: &mut u64,
    ) -> Vec<ClientMsg> {
        let mut startup_msgs = Vec::new();
        wm.groups.clear();
        wm.active_workspace = self.active_workspace.min(3);

        for sg in &self.groups {
            if sg.tabs.is_empty() {
                continue;
            }
            let group_id = wm.alloc_id();
            let mut tabs = Vec::with_capacity(sg.tabs.len());
            for st in &sg.tabs {
                let tab_id = wm.alloc_id();
                let kind = match st.kind {
                    SavedPaneKind::Terminal => {
                        let sid = *next_pty_id;
                        *next_pty_id += 1;
                        startup_msgs.push(ClientMsg::SpawnPty {
                            session_id: sid,
                            cols: 80,
                            rows: 24,
                            cmd: None,
                        });
                        PaneKind::Terminal(TerminalPane::new(sid, 80, 24))
                    }
                    SavedPaneKind::SystemdInspector => PaneKind::SystemdInspector {
                        filter: String::new(),
                        scope_filter: ScopeFilter::All,
                        state_filter: StateFilter::All,
                    },
                    SavedPaneKind::DockerInspector => PaneKind::DockerInspector {
                        filter: String::new(),
                    },
                    SavedPaneKind::SystemMonitor => PaneKind::SystemMonitor,
                    SavedPaneKind::AiJobs => PaneKind::AiJobs,
                    SavedPaneKind::HttpPorts => PaneKind::HttpPorts {
                        filter: String::new(),
                    },
                    SavedPaneKind::TailscalePanel => PaneKind::TailscalePanel,
                    SavedPaneKind::TailscaleServices => PaneKind::TailscaleServices {
                        filter: String::new(),
                        local_port_input: String::new(),
                        https_port_input: String::new(),
                    },
                    SavedPaneKind::FileTracker => {
                        let left_id = *next_tracker_pane_id;
                        let right_id = left_id + 1;
                        *next_tracker_pane_id += 2;
                        let tracker = TrackerWindow::new(left_id, right_id, &mut |msg| {
                            startup_msgs.push(msg);
                        });
                        PaneKind::FileTracker(Box::new(tracker))
                    }
                    SavedPaneKind::Vitals => {
                        let vitals = VitalsWindow::new(&mut |msg| {
                            startup_msgs.push(msg);
                        });
                        PaneKind::Vitals(Box::new(vitals))
                    }
                    SavedPaneKind::Help => PaneKind::Help(HelpWindow::new()),
                    SavedPaneKind::Settings => PaneKind::Settings(SettingsWindow::new()),
                    SavedPaneKind::Copilot => PaneKind::Copilot(Box::new(CopilotWindow::new())),
                };
                tabs.push(BeTab {
                    id: tab_id,
                    title: st.title.clone(),
                    kind,
                });
            }
            let active_tab = sg.active_tab.min(tabs.len().saturating_sub(1));
            wm.groups.push(BeWindowGroup {
                id: group_id,
                workspace: sg.workspace.min(3),
                floating: sg.floating,
                pos: Pos2::new(sg.pos[0], sg.pos[1]),
                size: Vec2::new(sg.size[0].max(280.0), sg.size[1].max(160.0)),
                tab_offset: 0.0,
                active_tab,
                tabs,
                shaded: sg.shaded,
            });
        }

        startup_msgs
    }
}

pub fn load_session() -> Option<SavedSession> {
    let raw = read_storage_string(STORAGE_KEY)?;
    let bytes = hex_decode(&raw)?;
    postcard::from_bytes::<SavedSession>(&bytes).ok()
}

pub fn save_session_bytes(bytes: &[u8]) {
    write_storage_string(STORAGE_KEY, &hex_encode(bytes));
}

pub fn load_workspace_layouts() -> [TilingLayout; 4] {
    let Some(raw) = read_storage_string(LAYOUTS_STORAGE_KEY) else {
        return [TilingLayout::Bsp; 4];
    };
    let Some(bytes) = hex_decode(&raw) else {
        return [TilingLayout::Bsp; 4];
    };
    if bytes.len() != 4 {
        return [TilingLayout::Bsp; 4];
    }
    [
        TilingLayout::from_id(bytes[0]),
        TilingLayout::from_id(bytes[1]),
        TilingLayout::from_id(bytes[2]),
        TilingLayout::from_id(bytes[3]),
    ]
}

pub fn save_workspace_layouts(layouts: [TilingLayout; 4]) {
    let bytes = [
        layouts[0].to_id(),
        layouts[1].to_id(),
        layouts[2].to_id(),
        layouts[3].to_id(),
    ];
    write_storage_string(LAYOUTS_STORAGE_KEY, &hex_encode(&bytes));
}

#[cfg(target_arch = "wasm32")]
fn read_storage_string(key: &str) -> Option<String> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok()??;
    storage.get_item(key).ok()?
}

#[cfg(target_arch = "wasm32")]
fn write_storage_string(key: &str, val: &str) {
    if let Some(window) = web_sys::window()
        && let Ok(Some(storage)) = window.local_storage()
    {
        let _ = storage.set_item(key, val);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_storage_string(_key: &str) -> Option<String> {
    let _ = (STORAGE_KEY, LAYOUTS_STORAGE_KEY);
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn write_storage_string(_key: &str, _val: &str) {}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
