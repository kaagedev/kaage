use crate::theme::{be_button, be_toggle_button, palette};
use egui::{CornerRadius, RichText, ScrollArea, Stroke, Ui};
use kaage_proto::{ClientMsg, DebugLogEntry, DebugLogLevel};
use std::collections::{BTreeSet, VecDeque};

pub struct DebugPanelWindow {
    pub search: String,
    pub selected_module: Option<String>,
    pub show_trace: bool,
    pub show_debug: bool,
    pub show_info: bool,
    pub show_warn: bool,
    pub show_error: bool,
    pub follow: bool,
}

impl DebugPanelWindow {
    pub fn new() -> Self {
        Self {
            search: String::new(),
            selected_module: None,
            show_trace: true,
            show_debug: true,
            show_info: true,
            show_warn: true,
            show_error: true,
            follow: true,
        }
    }

    fn level_enabled(&self, lvl: DebugLogLevel) -> bool {
        match lvl {
            DebugLogLevel::Trace => self.show_trace,
            DebugLogLevel::Debug => self.show_debug,
            DebugLogLevel::Info => self.show_info,
            DebugLogLevel::Warn => self.show_warn,
            DebugLogLevel::Error => self.show_error,
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        logs: &mut VecDeque<DebugLogEntry>,
        send_msg: &mut dyn FnMut(ClientMsg),
    ) {
        let p = palette();

        // Collect all seen modules dynamically so any app/module running under kaage appears.
        let mut modules = BTreeSet::new();
        let (mut c_err, mut c_wrn, mut c_inf, mut c_dbg, mut c_trc) = (0usize, 0, 0, 0, 0);
        for entry in logs.iter() {
            modules.insert(entry.module.clone());
            match entry.level {
                DebugLogLevel::Error => c_err += 1,
                DebugLogLevel::Warn => c_wrn += 1,
                DebugLogLevel::Info => c_inf += 1,
                DebugLogLevel::Debug => c_dbg += 1,
                DebugLogLevel::Trace => c_trc += 1,
            }
        }

        // Top toolbar: Search + Controls
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Debug Console")
                    .strong()
                    .size(13.0)
                    .color(p.accent),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Filter message, module, or target...")
                    .desired_width(220.0),
            );
            if be_toggle_button(
                ui,
                if self.follow { "● Live" } else { "⏸ Paused" },
                self.follow,
            )
            .clicked()
            {
                self.follow = !self.follow;
            }
            if be_button(ui, "Refresh").clicked() {
                send_msg(ClientMsg::GetDebugLogs);
            }
            if be_button(ui, "Clear").clicked() {
                logs.clear();
                send_msg(ClientMsg::ClearDebugLogs);
            }
        });

        ui.add_space(4.0);

        // Level filter row
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Level:").size(11.0).color(p.text_muted));
            if be_toggle_button(ui, &format!("ERR ({c_err})"), self.show_error).clicked() {
                self.show_error = !self.show_error;
            }
            if be_toggle_button(ui, &format!("WRN ({c_wrn})"), self.show_warn).clicked() {
                self.show_warn = !self.show_warn;
            }
            if be_toggle_button(ui, &format!("INF ({c_inf})"), self.show_info).clicked() {
                self.show_info = !self.show_info;
            }
            if be_toggle_button(ui, &format!("DBG ({c_dbg})"), self.show_debug).clicked() {
                self.show_debug = !self.show_debug;
            }
            if be_toggle_button(ui, &format!("TRC ({c_trc})"), self.show_trace).clicked() {
                self.show_trace = !self.show_trace;
            }
            if be_button(ui, "All").clicked() {
                self.show_trace = true;
                self.show_debug = true;
                self.show_info = true;
                self.show_warn = true;
                self.show_error = true;
            }
            if be_button(ui, "Warn+").clicked() {
                self.show_trace = false;
                self.show_debug = false;
                self.show_info = false;
                self.show_warn = true;
                self.show_error = true;
            }
        });

        ui.add_space(4.0);

        // Module / App filter chips
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Module:").size(11.0).color(p.text_muted));
            if be_toggle_button(ui, "All", self.selected_module.is_none()).clicked() {
                self.selected_module = None;
            }
            for m in &modules {
                let active = self.selected_module.as_deref() == Some(m.as_str());
                if be_toggle_button(ui, m, active).clicked() {
                    self.selected_module = if active { None } else { Some(m.clone()) };
                }
            }
        });

        let needle = self.search.trim().to_ascii_lowercase();
        let filtered: Vec<&DebugLogEntry> = logs
            .iter()
            .filter(|e| {
                if !self.level_enabled(e.level) {
                    return false;
                }
                if let Some(ref sel) = self.selected_module
                    && &e.module != sel
                {
                    return false;
                }
                if !needle.is_empty() {
                    let hit = e.message.to_ascii_lowercase().contains(&needle)
                        || e.module.to_ascii_lowercase().contains(&needle)
                        || e.target.to_ascii_lowercase().contains(&needle);
                    if !hit {
                        return false;
                    }
                }
                true
            })
            .collect();

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Showing {} / {} events", filtered.len(), logs.len()))
                    .monospace()
                    .size(10.5)
                    .color(p.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if be_button(ui, "Copy Filtered").clicked() {
                    let dump: String = filtered
                        .iter()
                        .map(|e| {
                            format!(
                                "{} [{}] [{}] {}",
                                format_ts_ms(e.ts_ms),
                                e.level.label(),
                                e.module,
                                e.message
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(dump);
                }
            });
        });
        ui.separator();

        // Log stream well
        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::ZERO)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ScrollArea::vertical()
                    .id_salt("kaage_debug_log_scroll")
                    .auto_shrink([false, false])
                    .stick_to_bottom(self.follow)
                    .show(ui, |ui| {
                        if filtered.is_empty() {
                            ui.label(
                                RichText::new("No matching log events.")
                                    .italics()
                                    .color(p.text_muted),
                            );
                            return;
                        }
                        for entry in &filtered {
                            let lvl_color = match entry.level {
                                DebugLogLevel::Error => p.status_red,
                                DebugLogLevel::Warn => p.status_amber,
                                DebugLogLevel::Info => p.status_green,
                                DebugLogLevel::Debug => p.status_sky,
                                DebugLogLevel::Trace => p.text_muted,
                            };
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.label(
                                    RichText::new(format_ts_ms(entry.ts_ms))
                                        .monospace()
                                        .size(10.5)
                                        .color(p.text_muted),
                                );
                                ui.label(
                                    RichText::new(entry.level.short())
                                        .monospace()
                                        .strong()
                                        .size(10.5)
                                        .color(lvl_color),
                                );
                                ui.label(
                                    RichText::new(format!("[{}]", entry.module))
                                        .monospace()
                                        .size(10.5)
                                        .color(p.accent),
                                );
                                ui.label(
                                    RichText::new(&entry.message)
                                        .monospace()
                                        .size(11.0)
                                        .color(p.text),
                                );
                            });
                        }
                    });
            });
    }
}

fn format_ts_ms(ts_ms: i64) -> String {
    let secs = (ts_ms / 1000).rem_euclid(86_400);
    let ms = ts_ms.rem_euclid(1000);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}
