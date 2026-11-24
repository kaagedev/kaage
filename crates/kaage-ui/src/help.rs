use crate::theme::{be_toggle_button, palette};
use egui::{CornerRadius, RichText, ScrollArea, Stroke, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpPage {
    #[default]
    BrowserVimKeys,
    KaageShortcuts,
    AppsAndApis,
}

pub struct HelpWindow {
    pub page: HelpPage,
}

impl HelpWindow {
    pub fn new() -> Self {
        Self {
            page: HelpPage::BrowserVimKeys,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let p = palette();

        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("kaage-help")
                    .strong()
                    .size(13.0)
                    .color(p.accent),
            );
            ui.separator();
            if be_toggle_button(
                ui,
                "Browser & Vim Keys",
                self.page == HelpPage::BrowserVimKeys,
            )
            .clicked()
            {
                self.page = HelpPage::BrowserVimKeys;
            }
            if be_toggle_button(
                ui,
                "Kaage Shortcuts",
                self.page == HelpPage::KaageShortcuts,
            )
            .clicked()
            {
                self.page = HelpPage::KaageShortcuts;
            }
            if be_toggle_button(ui, "Apps & APIs", self.page == HelpPage::AppsAndApis).clicked() {
                self.page = HelpPage::AppsAndApis;
            }
        });
        ui.separator();

        ScrollArea::vertical()
            .id_salt("kaage_help_scroll")
            .show(ui, |ui| {
                match self.page {
                    HelpPage::BrowserVimKeys => render_browser_vim_keys(ui),
                    HelpPage::KaageShortcuts => render_kaage_shortcuts(ui),
                    HelpPage::AppsAndApis => render_apps_and_apis(ui),
                }
                ui.add_space(10.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("Copyright © 2026")
                            .size(11.0)
                            .color(p.text_muted),
                    );
                    ui.hyperlink_to(
                        RichText::new("Pradeep Gowda")
                            .strong()
                            .size(11.0)
                            .color(p.accent),
                        "https://linkedin.com/in/btbytes",
                    );
                    ui.label(
                        RichText::new("· MIT License")
                            .size(11.0)
                            .color(p.text_muted),
                    );
                });
            });
    }
}

fn render_browser_vim_keys(ui: &mut Ui) {
    let p = palette();
    section_title(ui, "VIM & TERMINAL KEYS IN THE BROWSER");
    ui.label(
        RichText::new(
            "Browsers reserve certain global shortcuts (especially in fullscreen or macOS Safari/Edge). \
             kaage locks Escape, Tab, and Arrow keys inside focused terminals, and maps safe alternatives:",
        )
        .size(11.5)
        .color(p.text_muted),
    );
    ui.add_space(6.0);

    key_table(
        ui,
        &[
            (
                "Esc",
                "Sends 0x1B (focus-locked in kaage terminals). Works in normal browser windows; browser fullscreen may intercept Esc to exit fullscreen.",
            ),
            (
                "Ctrl+[",
                "Sends 0x1B (identical to Escape in Vim/Neovim). Never intercepted by Safari, Edge, or Firefox — even in fullscreen.",
            ),
            (
                "Ctrl+C",
                "Sends SIGINT (0x03) to the active PTY when focused. Also exits Insert mode in Vim.",
            ),
            (
                "Tab / Shift+Tab",
                "Focus-locked to the terminal (shell completion, Vim, TUI navigation) instead of cycling browser focus.",
            ),
            (
                "Alt+Arrows",
                "Sends xterm Alt+Arrow sequences (\\x1b[1;3A..D) for word movement and TUI pane navigation.",
            ),
            (
                "Ctrl+W / Ctrl+T / Ctrl+N",
                "Reserved by browsers (close tab / new tab / new window). In Vim, use :wincmd (or remap <Leader>w) instead of Ctrl+W; in shell, use Alt+Backspace to delete a word.",
            ),
            (
                "Ctrl+R",
                "In some browsers may reload the page if canvas isn't focused; click inside the terminal first, or use history search via Up/Down or fzf.",
            ),
            (
                "Ctrl+L / Ctrl+D / Ctrl+Z",
                "Forwarded directly to PTY (clear screen / EOF / suspend).",
            ),
            (
                "Ctrl+\\ / Ctrl+]",
                "Sends 0x1C (SIGQUIT) / 0x1D (telnet/Vim tag jump).",
            ),
            (
                "Cmd+V / Ctrl+Shift+V",
                "Pastes clipboard text directly into the focused terminal PTY.",
            ),
        ],
    );

    ui.add_space(8.0);
    section_title(ui, "VIM TIPS FOR BROWSER TERMINALS");
    bullet(
        ui,
        "Click once inside any terminal pane to focus it (cursor turns bright amber).",
    );
    bullet(
        ui,
        "In Safari or Edge fullscreen, use Ctrl+[ (or map 'inoremap jk <Esc>' in ~/.vimrc) so you never accidentally exit browser fullscreen.",
    );
    bullet(
        ui,
        "Set 'set ttimeoutlen=10' in Vim for instant Escape response over Tailscale.",
    );

    ui.add_space(10.0);
    section_title(ui, "TERMINAL ENGINE: LIBGHOSTTY-VT & KITTY KEYBOARD PROTOCOL");
    ui.label(
        RichText::new(
            "kaage's terminal architecture targets libghostty-vt (optional native 'ghostty' feature, ghostty-vt 0.8) \
             with a pure-Rust vt100 fallback for zero-toolchain offline Nix WASM builds.",
        )
        .size(11.5)
        .color(p.text_muted),
    );
    ui.add_space(4.0);
    key_table(
        ui,
        &[
            (
                "libghostty-vt",
                "Battle-tested VT emulator extracted from Ghostty: full 24-bit truecolor, Unicode grapheme clusters, synchronized output (DEC 2026), and fast scrollback without xterm.js DOM overhead.",
            ),
            (
                "Kitty Keyboard Protocol",
                "Supported by libghostty-vt (CSI u encoding). Unlike legacy VT100 where Ctrl+I == Tab, Ctrl+M == Enter, and Ctrl+[ == Esc, Kitty protocol encodes exact key + modifier + press/release state.",
            ),
            (
                "Why it matters",
                "Unlocks unambiguous shortcuts in Neovim, Helix, tmux, and Zellij (e.g. Shift+Enter, Ctrl+Enter, Ctrl+Shift+letter, and clean Esc vs Ctrl+[ handling) even over a browser/Tailscale session.",
            ),
        ],
    );
}

fn render_kaage_shortcuts(ui: &mut Ui) {
    section_title(ui, "MISSION-CONTROL WORKSPACE & WINDOW SHORTCUTS");
    key_table(
        ui,
        &[
            (
                "Shift × 2",
                "Double-tap Shift to open the global Command HUD (zero browser/terminal conflicts; launch apps, control services, switch workspaces, toggle theme).",
            ),
            ("Alt+1 .. Alt+4", "Switch virtual workspaces: 1: Ops, 2: AI, 3: Files, 4: Logs."),
            ("Alt+H / J / K / L", "Cycle focus across tiled panes (or Alt+Left/Right)."),
            ("FLT / PIP button", "Pin any pane as a draggable floating PiP overlay that stays visible across all workspaces."),
            ("Drag Tab → Header", "Stack two panes into a shared tab bar; drag a tab off to detach into a new BSP tile."),
            ("◐ Obsidian / BeOS", "Toggle live between Obsidian & Amber (Dark) and Retro BeOS (Light) themes."),
            ("Click Clock", "Cycle live topbar clock between Date+Time, HH:MM, and HH:MM:SS."),
        ],
    );

    ui.add_space(8.0);
    section_title(ui, "TRACKER (FILE EXPLORER) SHORTCUTS");
    key_table(
        ui,
        &[
            ("F5 / F6", "Copy or Move selected items between Left and Right panes."),
            ("Tab", "Switch active focus between Left and Right folder panes."),
            ("+Shelf / Move Here", "Stage files across deep directory trees in the bottom Holding Shelf."),
            ("Trash + Undo", "Safe delete to ~/.local/share/kaage/trash with 1-click Undo in the toolbar or toast."),
            ("● unit.service", "Click a green systemd badge on any open file to tail its journalctl logs."),
        ],
    );

    ui.add_space(8.0);
    section_title(ui, "KAAGE VITALS (TELEMETRY) INTERACTIONS");
    key_table(
        ui,
        &[
            ("Hover any chart", "Synchronizes the yellow vertical crosshair scrubber across CPU, GPU, RAM, Disk, and Network."),
            ("Drag across chart", "Zoom into any time range across all subsystems; click 'Reset Zoom' to return to live scrolling."),
            ("1m .. 24h", "Switch between 1s live ring buffer (<=1h) and persisted 10s postcard rollups (up to 24h)."),
        ],
    );
}

fn render_apps_and_apis(ui: &mut Ui) {
    section_title(ui, "BUILT-IN APPS");
    key_table(
        ui,
        &[
            ("Systemd Services", "NixOS-aware system + user service list (enabled/running/linked/masked), live Status panel, Start/Stop/Restart/Logs."),
            ("Kaage Vitals", "Time-traveling CPU/GPU/RAM/Disk/Network telemetry, BeOS Pulse core bars, PSI pressure, and process controls."),
            ("Tracker (Files)", "Dual-pane file commander with Markdown/Image/ZIP/CSV/Code/SafeTensors/GGUF previews & Workspaces."),
            ("AI Jobs", "Live ComfyUI queue (/queue) and Unsloth Studio health & worker processes."),
            ("HTTP Ports & Tailscale", "Listening TCP ports mapped to Tailscale Serve HTTPS URLs, plus Tailnet peer status."),
        ],
    );

    ui.add_space(8.0);
    section_title(ui, "KAAGED HTTP REST ENDPOINTS");
    key_table(
        ui,
        &[
            ("GET /api/v1/health", "Daemon health, D-Bus connectivity, Docker status, and NixOS generation info."),
            ("GET /api/v1/nixos", "NixOS OS version, current-system store path, and active generation number."),
            ("GET /api/v1/units", "Filterable systemd services (?scope=system|user&enabled=true&running=true&q=...)."),
            ("GET /api/v1/units/{u}/status", "Detailed unit status, PID, memory, resolved /nix/store path, and systemctl status output."),
            ("POST /api/v1/units/{u}/{act}", "Control a service (start, stop, restart, reload)."),
            ("GET /api/v1/vitals", "Historical & live VitalsSnapshot (?window=5m&buckets=120)."),
            ("GET /api/v1/dashboard", "Full snapshot of GPU, CPU, RAM, ComfyUI, Unsloth, HTTP ports, and Tailscale."),
            ("GET /api/v1/files/download", "Stream any host file over HTTP (?path=/path/to/file)."),
        ],
    );
}

fn section_title(ui: &mut Ui, title: &str) {
    let p = palette();
    ui.label(
        RichText::new(title)
            .monospace()
            .strong()
            .size(11.0)
            .color(p.accent),
    );
    ui.add_space(3.0);
}

fn bullet(ui: &mut Ui, text: &str) {
    let p = palette();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("•").strong().color(p.accent));
        ui.label(RichText::new(text).size(11.5).color(p.text));
    });
}

fn key_table(ui: &mut Ui, rows: &[(&str, &str)]) {
    let p = palette();
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            for (i, (key, desc)) in rows.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    egui::Frame::NONE
                        .fill(p.surface_alt)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::ZERO)
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(*key)
                                    .monospace()
                                    .strong()
                                    .size(10.5)
                                    .color(p.accent),
                            );
                        });
                    ui.label(RichText::new(*desc).size(11.5).color(p.text));
                });
                if i + 1 < rows.len() {
                    ui.add_space(3.0);
                }
            }
        });
}
