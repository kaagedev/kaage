use crate::theme::{BEOS_STATUS_GREEN, BEOS_STATUS_RED, be_button, be_toggle_button};
use egui::{Color32, RichText, ScrollArea, Ui};
use kaage_proto::{
    ClientMsg, ContainerAction, ContainerInfo, SystemdUnitInfo, UnitAction, UnitScope,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeFilter {
    #[default]
    All,
    System,
    User,
}

pub enum InspectorSpawnRequest {
    OpenCommandTab { title: String, cmd: Vec<String> },
}

pub fn render_systemd_inspector(
    ui: &mut Ui,
    filter: &mut String,
    scope_filter: &mut ScopeFilter,
    units: &[SystemdUnitInfo],
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<InspectorSpawnRequest> {
    let mut spawn_req = None;

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
        }
    });
    ui.separator();

    let needle = filter.to_lowercase();

    ScrollArea::vertical().show(ui, |ui| {
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
            if !needle.is_empty()
                && !unit.name.to_lowercase().contains(&needle)
                && !unit.description.to_lowercase().contains(&needle)
            {
                continue;
            }
            shown += 1;

            ui.horizontal(|ui| {
                let state_color = if unit.active_state == "active" {
                    BEOS_STATUS_GREEN
                } else {
                    BEOS_STATUS_RED
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
                ui.label(RichText::new(&unit.name).strong().color(Color32::BLACK));
                ui.label(
                    RichText::new(format!("({})", unit.sub_state))
                        .color(Color32::from_rgb(70, 70, 70)),
                );

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
