use crate::theme::{BEOS_STATUS_GREEN, BEOS_STATUS_RED, be_button, be_toggle_button};
use egui::{Color32, RichText, ScrollArea, Ui};
use kaage_proto::{
    ClientMsg, ContainerAction, ContainerInfo, NixosInfo, SystemdUnitInfo, SystemdUnitStatus,
    UnitAction, UnitScope,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeFilter {
    #[default]
    All,
    System,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateFilter {
    #[default]
    All,
    Running,
    Enabled,
    Stopped,
    Failed,
}

pub enum InspectorSpawnRequest {
    OpenCommandTab { title: String, cmd: Vec<String> },
}

#[allow(clippy::too_many_arguments)]
pub fn render_systemd_inspector(
    ui: &mut Ui,
    filter: &mut String,
    scope_filter: &mut ScopeFilter,
    state_filter: &mut StateFilter,
    nixos: Option<&NixosInfo>,
    units: &[SystemdUnitInfo],
    selected_status: &mut Option<SystemdUnitStatus>,
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<InspectorSpawnRequest> {
    let mut spawn_req = None;

    let total_count = units.len();
    let running_count = units.iter().filter(|u| u.running).count();
    let enabled_count = units.iter().filter(|u| u.enabled).count();

    if let Some(info) = nixos
        && info.is_nixos
    {
        ui.horizontal(|ui| {
            let gen_str = info
                .generation
                .map(|g| format!(" • gen #{g}"))
                .unwrap_or_default();
            ui.label(
                RichText::new(format!("{}{gen_str}", info.pretty_name))
                    .strong()
                    .size(11.5)
                    .color(Color32::from_rgb(30, 70, 120)),
            );
            ui.label(
                RichText::new(format!(
                    "• {running_count} running • {enabled_count} enabled • {total_count} services"
                ))
                .size(11.0)
                .color(Color32::from_rgb(60, 60, 60)),
            );
        });
        ui.separator();
    }

    ui.horizontal(|ui| {
        ui.label(RichText::new("Filter:").color(Color32::BLACK));
        ui.add(egui::TextEdit::singleline(filter).desired_width(140.0));
        if be_toggle_button(ui, "All", *scope_filter == ScopeFilter::All).clicked() {
            *scope_filter = ScopeFilter::All;
        }
        if be_toggle_button(ui, "Sys", *scope_filter == ScopeFilter::System).clicked() {
            *scope_filter = ScopeFilter::System;
        }
        if be_toggle_button(ui, "User", *scope_filter == ScopeFilter::User).clicked() {
            *scope_filter = ScopeFilter::User;
        }
        if be_button(ui, "Refresh").clicked() {
            send_msg(ClientMsg::ListUnits);
            if let Some(st) = selected_status.as_ref() {
                send_msg(ClientMsg::GetUnitStatus {
                    unit: st.name.clone(),
                    scope: st.scope,
                });
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(RichText::new("State:").color(Color32::BLACK));
        if be_toggle_button(ui, "All", *state_filter == StateFilter::All).clicked() {
            *state_filter = StateFilter::All;
        }
        if be_toggle_button(ui, "Running", *state_filter == StateFilter::Running).clicked() {
            *state_filter = StateFilter::Running;
        }
        if be_toggle_button(ui, "Enabled", *state_filter == StateFilter::Enabled).clicked() {
            *state_filter = StateFilter::Enabled;
        }
        if be_toggle_button(ui, "Stopped", *state_filter == StateFilter::Stopped).clicked() {
            *state_filter = StateFilter::Stopped;
        }
        if be_toggle_button(ui, "Failed", *state_filter == StateFilter::Failed).clicked() {
            *state_filter = StateFilter::Failed;
        }
    });
    ui.separator();

    let needle = filter.to_lowercase();
    let list_max_height = if selected_status.is_some() {
        (ui.available_height() - 185.0).max(90.0)
    } else {
        ui.available_height()
    };

    ScrollArea::vertical()
        .id_salt("systemd_units_list")
        .max_height(list_max_height)
        .show(ui, |ui| {
            let mut shown = 0usize;
            for unit in units {
                let scope_matches = match *scope_filter {
                    ScopeFilter::All => true,
                    ScopeFilter::System => unit.scope == UnitScope::System,
                    ScopeFilter::User => unit.scope == UnitScope::User,
                };
                if !scope_matches {
                    continue;
                }
                let state_matches = match *state_filter {
                    StateFilter::All => true,
                    StateFilter::Running => unit.running,
                    StateFilter::Enabled => unit.enabled,
                    StateFilter::Stopped => !unit.running && unit.active_state != "failed",
                    StateFilter::Failed => unit.active_state == "failed",
                };
                if !state_matches {
                    continue;
                }
                if !needle.is_empty()
                    && !unit.name.to_lowercase().contains(&needle)
                    && !unit.description.to_lowercase().contains(&needle)
                    && !unit.unit_file_state.to_lowercase().contains(&needle)
                {
                    continue;
                }
                shown += 1;

                ui.horizontal(|ui| {
                    let state_color = if unit.running {
                        BEOS_STATUS_GREEN
                    } else if unit.active_state == "active" {
                        Color32::from_rgb(184, 134, 11)
                    } else if unit.active_state == "failed" {
                        BEOS_STATUS_RED
                    } else {
                        Color32::from_rgb(115, 115, 115)
                    };
                    ui.label(RichText::new("●").color(state_color));

                    let scope_tag = match unit.scope {
                        UnitScope::System => "sys",
                        UnitScope::User => "usr",
                    };
                    ui.label(
                        RichText::new(format!("[{scope_tag}]"))
                            .monospace()
                            .size(10.5)
                            .color(Color32::from_rgb(70, 70, 70)),
                    );

                    if unit.nixos_managed {
                        ui.label(
                            RichText::new("[nix]")
                                .monospace()
                                .size(10.0)
                                .color(Color32::from_rgb(51, 102, 152)),
                        );
                    }

                    ui.label(RichText::new(&unit.name).strong().color(Color32::BLACK));
                    ui.label(
                        RichText::new(format!("({})", unit.sub_state))
                            .size(11.0)
                            .color(Color32::from_rgb(70, 70, 70)),
                    );

                    if !unit.unit_file_state.is_empty() {
                        let file_state_color = if unit.enabled {
                            BEOS_STATUS_GREEN
                        } else if unit.unit_file_state == "masked" {
                            BEOS_STATUS_RED
                        } else {
                            Color32::from_rgb(95, 95, 95)
                        };
                        ui.label(
                            RichText::new(&unit.unit_file_state)
                                .monospace()
                                .size(10.5)
                                .color(file_state_color),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if be_button(ui, "Logs").clicked() {
                            let mut cmd = vec!["journalctl".into()];
                            if unit.scope == UnitScope::User {
                                cmd.push("--user".into());
                            }
                            cmd.extend([
                                "-u".into(),
                                unit.name.clone(),
                                "-n".into(),
                                "100".into(),
                                "-f".into(),
                            ]);
                            spawn_req = Some(InspectorSpawnRequest::OpenCommandTab {
                                title: format!("logs: {}", unit.name),
                                cmd,
                            });
                        }
                        if be_button(ui, "Status").clicked() {
                            send_msg(ClientMsg::GetUnitStatus {
                                unit: unit.name.clone(),
                                scope: unit.scope,
                            });
                        }
                        if be_button(ui, "Restart").clicked() {
                            send_msg(ClientMsg::ControlUnit {
                                unit: unit.name.clone(),
                                scope: unit.scope,
                                action: UnitAction::Restart,
                            });
                        }
                        if unit.active_state == "active" {
                            if be_button(ui, "Stop").clicked() {
                                send_msg(ClientMsg::ControlUnit {
                                    unit: unit.name.clone(),
                                    scope: unit.scope,
                                    action: UnitAction::Stop,
                                });
                            }
                        } else if be_button(ui, "Start").clicked() {
                            send_msg(ClientMsg::ControlUnit {
                                unit: unit.name.clone(),
                                scope: unit.scope,
                                action: UnitAction::Start,
                            });
                        }
                    });
                });
                ui.separator();
            }

            if shown == 0 {
                ui.label(
                    RichText::new("No matching systemd services.")
                        .italics()
                        .color(Color32::from_rgb(80, 80, 80)),
                );
            }
        });

    let mut close_status = false;
    if let Some(st) = selected_status.as_ref() {
        ui.separator();
        egui::Frame::NONE
            .fill(Color32::from_rgb(232, 232, 232))
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let scope_tag = match st.scope {
                        UnitScope::System => "system",
                        UnitScope::User => "user",
                    };
                    ui.label(
                        RichText::new(format!("Status: {} [{scope_tag}]", st.name))
                            .strong()
                            .color(Color32::BLACK),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if be_button(ui, "Close").clicked() {
                            close_status = true;
                        }
                        if be_button(ui, "Refresh").clicked() {
                            send_msg(ClientMsg::GetUnitStatus {
                                unit: st.name.clone(),
                                scope: st.scope,
                            });
                        }
                        if be_button(ui, "Restart").clicked() {
                            send_msg(ClientMsg::ControlUnit {
                                unit: st.name.clone(),
                                scope: st.scope,
                                action: UnitAction::Restart,
                            });
                        }
                        if st.active_state == "active" {
                            if be_button(ui, "Stop").clicked() {
                                send_msg(ClientMsg::ControlUnit {
                                    unit: st.name.clone(),
                                    scope: st.scope,
                                    action: UnitAction::Stop,
                                });
                            }
                        } else if be_button(ui, "Start").clicked() {
                            send_msg(ClientMsg::ControlUnit {
                                unit: st.name.clone(),
                                scope: st.scope,
                                action: UnitAction::Start,
                            });
                        }
                    });
                });

                let mem_str = st
                    .memory_current
                    .map(|b| format!(" • Mem: {:.1} MiB", b as f64 / (1024.0 * 1024.0)))
                    .unwrap_or_default();
                let pid_str = if st.main_pid > 0 {
                    format!(" • PID: {}", st.main_pid)
                } else {
                    String::new()
                };
                ui.label(
                    RichText::new(format!(
                        "Enabled: {} ({}) • Running: {} ({}/{}) • Load: {}{pid_str}{mem_str}",
                        if st.enabled { "yes" } else { "no" },
                        if st.unit_file_state.is_empty() {
                            "n/a"
                        } else {
                            &st.unit_file_state
                        },
                        if st.running { "yes" } else { "no" },
                        st.active_state,
                        st.sub_state,
                        st.load_state,
                    ))
                    .size(11.0)
                    .color(Color32::BLACK),
                );

                if let Some(ref store) = st.nix_store_path {
                    ui.label(
                        RichText::new(format!("NixOS unit: {store}"))
                            .monospace()
                            .size(10.0)
                            .color(Color32::from_rgb(51, 102, 152)),
                    );
                } else if !st.fragment_path.is_empty() {
                    ui.label(
                        RichText::new(format!("Unit file: {}", st.fragment_path))
                            .monospace()
                            .size(10.0)
                            .color(Color32::from_rgb(80, 80, 80)),
                    );
                }

                ScrollArea::vertical()
                    .id_salt("systemd_unit_status_text")
                    .max_height(105.0)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&st.status_text)
                                .monospace()
                                .size(10.5)
                                .color(Color32::from_rgb(25, 25, 25)),
                        );
                    });
            });
    }
    if close_status {
        *selected_status = None;
    }

    spawn_req
}

pub fn render_docker_inspector(
    ui: &mut Ui,
    filter: &mut String,
    containers: &[ContainerInfo],
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<InspectorSpawnRequest> {
    let mut spawn_req = None;

    ui.horizontal(|ui| {
        ui.label(RichText::new("Filter:").color(Color32::BLACK));
        ui.text_edit_singleline(filter);
        if be_button(ui, "Refresh").clicked() {
            send_msg(ClientMsg::ListContainers);
        }
    });
    ui.separator();

    let needle = filter.to_lowercase();

    ScrollArea::vertical().show(ui, |ui| {
        let mut shown = 0usize;
        for ctr in containers {
            if !needle.is_empty()
                && !ctr.name.to_lowercase().contains(&needle)
                && !ctr.image.to_lowercase().contains(&needle)
            {
                continue;
            }
            shown += 1;

            ui.horizontal(|ui| {
                let state_color = if ctr.state == "running" {
                    BEOS_STATUS_GREEN
                } else {
                    BEOS_STATUS_RED
                };
                ui.label(RichText::new("■").color(state_color));
                ui.label(RichText::new(&ctr.name).strong().color(Color32::BLACK));
                ui.label(RichText::new(&ctr.image).color(Color32::from_rgb(75, 75, 75)));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ctr.state == "running" {
                        if be_button(ui, "Shell").clicked() {
                            spawn_req = Some(InspectorSpawnRequest::OpenCommandTab {
                                title: format!("sh: {}", ctr.name),
                                cmd: vec![
                                    "docker".into(),
                                    "exec".into(),
                                    "-it".into(),
                                    ctr.id.clone(),
                                    "/bin/sh".into(),
                                ],
                            });
                        }
                        if be_button(ui, "Restart").clicked() {
                            send_msg(ClientMsg::ControlContainer {
                                id: ctr.id.clone(),
                                action: ContainerAction::Restart,
                            });
                        }
                        if be_button(ui, "Stop").clicked() {
                            send_msg(ClientMsg::ControlContainer {
                                id: ctr.id.clone(),
                                action: ContainerAction::Stop,
                            });
                        }
                    } else {
                        if be_button(ui, "Remove").clicked() {
                            send_msg(ClientMsg::ControlContainer {
                                id: ctr.id.clone(),
                                action: ContainerAction::Remove,
                            });
                        }
                        if be_button(ui, "Start").clicked() {
                            send_msg(ClientMsg::ControlContainer {
                                id: ctr.id.clone(),
                                action: ContainerAction::Start,
                            });
                        }
                    }
                });
            });
            ui.separator();
        }

        if shown == 0 {
            ui.label(
                RichText::new("No containers detected.")
                    .italics()
                    .color(Color32::from_rgb(80, 80, 80)),
            );
        }
    });

    spawn_req
}
