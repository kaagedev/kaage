use crate::inspectors::{ScopeFilter, StateFilter};
use crate::theme::palette;
use egui::{CornerRadius, Id, Key, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Vec2};
use kaage_proto::{ContainerAction, ContainerInfo, SystemdUnitInfo, UnitAction, UnitScope};

pub fn is_macos() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.navigator().user_agent().ok())
            .map(|ua| ua.contains("Mac"))
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        cfg!(target_os = "macos")
    }
}

pub fn palette_shortcut_badge() -> &'static str {
    if is_macos() { "⌘/" } else { "⌃`" }
}

pub enum PaletteAction {
    SwitchWorkspace(usize),
    OpenVitals,
    OpenFileTracker,
    OpenSystemd { scope: ScopeFilter, state: StateFilter },
    OpenSystemMonitor,
    OpenAiJobs,
    OpenHttpPorts,
    OpenTailscale,
    OpenTailscaleServices,
    OpenContainers,
    OpenHelp,
    OpenSettings,
    OpenAppDataSettings,
    OpenCopilot,
    NewTerminal,
    ToggleTheme,
    ToggleWallpaper,
    ToggleSidebar,
    TogglePinActive,
    ControlUnit { unit: String, scope: UnitScope, action: UnitAction },
    UnitLogs { unit: String, scope: UnitScope },
    ControlContainer { id: String, action: ContainerAction },
    ContainerShell { name: String, id: String },
}

struct PaletteItem {
    label: String,
    category: &'static str,
    shortcut: String,
    action: PaletteAction,
}

pub struct CommandPalette {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    just_opened: bool,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            just_opened: false,
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.query.clear();
            self.selected = 0;
            self.just_opened = true;
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        units: &[SystemdUnitInfo],
        containers: &[ContainerInfo],
    ) -> Option<PaletteAction> {
        if !self.open {
            return None;
        }

        let p = palette();
        let screen = ctx.content_rect();
        let modal_w = 560.0_f32.min(screen.width() - 32.0);
        let modal_h = 380.0_f32.min(screen.height() - 80.0);
        let modal_pos = Pos2::new(
            (screen.width() - modal_w) * 0.5,
            (screen.height() * 0.16).max(42.0),
        );

        let mut chosen = None;
        let mut close_now = false;

        let items = build_items(&self.query, units, containers);
        if self.selected >= items.len() {
            self.selected = items.len().saturating_sub(1);
        }

        ctx.input(|i| {
            if i.key_pressed(Key::Escape) {
                close_now = true;
            }
            if i.key_pressed(Key::ArrowDown) && !items.is_empty() {
                self.selected = (self.selected + 1) % items.len();
            }
            if i.key_pressed(Key::ArrowUp) && !items.is_empty() {
                self.selected = if self.selected == 0 {
                    items.len() - 1
                } else {
                    self.selected - 1
                };
            }
        });

        let enter_pressed = ctx.input(|i| i.key_pressed(Key::Enter));

        // Dim backdrop
        egui::Area::new(Id::unique("kaage_cmd_backdrop"))
            .fixed_pos(screen.min)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter().rect_filled(
                    screen,
                    CornerRadius::ZERO,
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 130),
                );
                if resp.clicked() {
                    close_now = true;
                }
            });

        egui::Area::new(Id::unique("kaage_cmd_palette"))
            .fixed_pos(modal_pos)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.5, p.accent))
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.set_min_size(Vec2::new(modal_w - 20.0, modal_h - 20.0));
                        ui.set_max_size(Vec2::new(modal_w - 20.0, modal_h - 20.0));

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(palette_shortcut_badge())
                                    .monospace()
                                    .strong()
                                    .color(p.accent),
                            );
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.query)
                                    .hint_text("Type a command: vitals, restart comfyui, tracker, shell...")
                                    .desired_width(ui.available_width() - 48.0),
                            );
                            if self.just_opened {
                                resp.request_focus();
                                self.just_opened = false;
                            }
                            ui.label(
                                RichText::new("ESC")
                                    .monospace()
                                    .size(10.0)
                                    .color(p.text_muted),
                            );
                        });
                        ui.separator();

                        ScrollArea::vertical()
                            .id_salt("cmd_palette_list")
                            .max_height(modal_h - 82.0)
                            .show(ui, |ui| {
                                if items.is_empty() {
                                    ui.label(
                                        RichText::new("No matching commands.")
                                            .italics()
                                            .color(p.text_muted),
                                    );
                                }
                                for (idx, item) in items.iter().enumerate() {
                                    let is_sel = idx == self.selected;
                                    let row_w = ui.available_width();
                                    let (rect, resp) = ui.allocate_exact_size(
                                        Vec2::new(row_w, 26.0),
                                        Sense::click(),
                                    );
                                    let painter = ui.painter();
                                    if is_sel || resp.hovered() {
                                        painter.rect_filled(rect, CornerRadius::ZERO, p.hover_row);
                                        if is_sel {
                                            painter.rect_filled(
                                                Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
                                                CornerRadius::ZERO,
                                                p.accent,
                                            );
                                        }
                                    }
                                    painter.text(
                                        Pos2::new(rect.min.x + 10.0, rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        &item.label,
                                        egui::FontId::proportional(12.5),
                                        if is_sel { p.accent } else { p.text },
                                    );
                                    painter.text(
                                        Pos2::new(rect.max.x - 8.0, rect.center().y),
                                        egui::Align2::RIGHT_CENTER,
                                        format!("{}  {}", item.category, item.shortcut),
                                        egui::FontId::monospace(10.5),
                                        p.text_muted,
                                    );
                                    if resp.hovered() {
                                        self.selected = idx;
                                    }
                                    if resp.clicked() {
                                        chosen = Some(idx);
                                    }
                                }
                            });
                    });
            });

        if enter_pressed && !items.is_empty() && chosen.is_none() {
            chosen = Some(self.selected);
        }

        if let Some(idx) = chosen {
            self.open = false;
            return items.into_iter().nth(idx).map(|i| i.action);
        }
        if close_now {
            self.open = false;
        }
        None
    }
}

fn build_items(
    query: &str,
    units: &[SystemdUnitInfo],
    containers: &[ContainerInfo],
) -> Vec<PaletteItem> {
    let q = query.trim().to_ascii_lowercase();
    let mut all = vec![
        PaletteItem {
            label: "Open Kaage Copilot (AI Assistant)".into(),
            category: "AI",
            shortcut: "✦".into(),
            action: PaletteAction::OpenCopilot,
        },
        PaletteItem {
            label: "Open Kaage Vitals (Telemetry Cockpit)".into(),
            category: "APP",
            shortcut: "VIT".into(),
            action: PaletteAction::OpenVitals,
        },
        PaletteItem {
            label: "Open Tracker (Dual-Pane File Explorer)".into(),
            category: "APP",
            shortcut: "TRK".into(),
            action: PaletteAction::OpenFileTracker,
        },
        PaletteItem {
            label: "Open Systemd Services".into(),
            category: "APP",
            shortcut: "SYS".into(),
            action: PaletteAction::OpenSystemd {
                scope: ScopeFilter::All,
                state: StateFilter::All,
            },
        },
        PaletteItem {
            label: "New Interactive Shell Terminal".into(),
            category: "APP",
            shortcut: ">_".into(),
            action: PaletteAction::NewTerminal,
        },
        PaletteItem {
            label: "Open AI Jobs (ComfyUI & Unsloth)".into(),
            category: "APP",
            shortcut: "AI".into(),
            action: PaletteAction::OpenAiJobs,
        },
        PaletteItem {
            label: "Open GPU / CPU / Memory Monitor".into(),
            category: "APP",
            shortcut: "MON".into(),
            action: PaletteAction::OpenSystemMonitor,
        },
        PaletteItem {
            label: "Open HTTP Ports & Services".into(),
            category: "APP",
            shortcut: "NET".into(),
            action: PaletteAction::OpenHttpPorts,
        },
        PaletteItem {
            label: "Open Tailscale Services (Exposed Node Endpoints)".into(),
            category: "NET",
            shortcut: "SVC".into(),
            action: PaletteAction::OpenTailscaleServices,
        },
        PaletteItem {
            label: "Open Tailscale Mesh & Serve".into(),
            category: "APP",
            shortcut: "TS".into(),
            action: PaletteAction::OpenTailscale,
        },
        PaletteItem {
            label: "Open Containers".into(),
            category: "APP",
            shortcut: "CTR".into(),
            action: PaletteAction::OpenContainers,
        },
        PaletteItem {
            label: "Open kaage-help (Vim Keys, Shortcuts & APIs)".into(),
            category: "HELP",
            shortcut: "?".into(),
            action: PaletteAction::OpenHelp,
        },
        PaletteItem {
            label: "Open kaage-settings (Typography, Font Size & Color Editor)".into(),
            category: "CFG",
            shortcut: "⚙".into(),
            action: PaletteAction::OpenSettings,
        },
        PaletteItem {
            label: "Open kaage-settings → App Data (KAAGE_APPDATA)".into(),
            category: "CFG",
            shortcut: "DIR".into(),
            action: PaletteAction::OpenAppDataSettings,
        },
        PaletteItem {
            label: "Workspace 1: Ops".into(),
            category: "WS",
            shortcut: "Alt+1".into(),
            action: PaletteAction::SwitchWorkspace(0),
        },
        PaletteItem {
            label: "Workspace 2: AI".into(),
            category: "WS",
            shortcut: "Alt+2".into(),
            action: PaletteAction::SwitchWorkspace(1),
        },
        PaletteItem {
            label: "Workspace 3: Files".into(),
            category: "WS",
            shortcut: "Alt+3".into(),
            action: PaletteAction::SwitchWorkspace(2),
        },
        PaletteItem {
            label: "Workspace 4: Logs".into(),
            category: "WS",
            shortcut: "Alt+4".into(),
            action: PaletteAction::SwitchWorkspace(3),
        },
        PaletteItem {
            label: "Toggle Theme (Obsidian Amber / Retro BeOS)".into(),
            category: "UI",
            shortcut: "◐".into(),
            action: PaletteAction::ToggleTheme,
        },
        PaletteItem {
            label: "Toggle Wallpaper / Obsidian Dot-Grid".into(),
            category: "UI",
            shortcut: "BG".into(),
            action: PaletteAction::ToggleWallpaper,
        },
        PaletteItem {
            label: "Toggle Left Sidebar".into(),
            category: "UI",
            shortcut: "NAV".into(),
            action: PaletteAction::ToggleSidebar,
        },
        PaletteItem {
            label: "Pin / Float Active Pane (PiP Overlay)".into(),
            category: "WM",
            shortcut: "PiP".into(),
            action: PaletteAction::TogglePinActive,
        },
    ];

    // Dynamic service actions when user types a query
    if !q.is_empty() {
        for u in units {
            let short = u.name.trim_end_matches(".service");
            if q.contains(short) || u.name.to_ascii_lowercase().contains(&q) || matches_tokens(&format!("restart {short}"), &q) {
                all.push(PaletteItem {
                    label: format!("restart {}", u.name),
                    category: "UNIT",
                    shortcut: u.sub_state.clone(),
                    action: PaletteAction::ControlUnit {
                        unit: u.name.clone(),
                        scope: u.scope,
                        action: UnitAction::Restart,
                    },
                });
                all.push(PaletteItem {
                    label: format!(
                        "{} {}",
                        if u.active_state == "active" { "stop" } else { "start" },
                        u.name
                    ),
                    category: "UNIT",
                    shortcut: u.sub_state.clone(),
                    action: PaletteAction::ControlUnit {
                        unit: u.name.clone(),
                        scope: u.scope,
                        action: if u.active_state == "active" {
                            UnitAction::Stop
                        } else {
                            UnitAction::Start
                        },
                    },
                });
                all.push(PaletteItem {
                    label: format!("logs {}", u.name),
                    category: "LOGS",
                    shortcut: "journalctl".into(),
                    action: PaletteAction::UnitLogs {
                        unit: u.name.clone(),
                        scope: u.scope,
                    },
                });
            }
        }
        for c in containers {
            if c.name.to_ascii_lowercase().contains(&q) || matches_tokens(&format!("shell {}", c.name), &q) {
                all.push(PaletteItem {
                    label: format!("shell {}", c.name),
                    category: "CTR",
                    shortcut: c.state.clone(),
                    action: PaletteAction::ContainerShell {
                        name: c.name.clone(),
                        id: c.id.clone(),
                    },
                });
                all.push(PaletteItem {
                    label: format!("restart container {}", c.name),
                    category: "CTR",
                    shortcut: c.state.clone(),
                    action: PaletteAction::ControlContainer {
                        id: c.id.clone(),
                        action: ContainerAction::Restart,
                    },
                });
            }
        }
    }

    if q.is_empty() {
        all.truncate(14);
        return all;
    }

    let mut filtered: Vec<PaletteItem> = all
        .into_iter()
        .filter(|item| matches_tokens(&format!("{} {}", item.label, item.category), &q))
        .collect();
    filtered.truncate(16);
    filtered
}

fn matches_tokens(haystack: &str, query: &str) -> bool {
    let lower = haystack.to_ascii_lowercase();
    query
        .split_whitespace()
        .all(|tok| lower.contains(tok))
}
