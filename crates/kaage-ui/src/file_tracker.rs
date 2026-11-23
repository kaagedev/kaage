use crate::theme::{be_button, palette};
use egui::{RichText, ScrollArea, Ui};
use kaage_proto::{
    ClientMsg, DirListing, FileEntry, FileKind, FileOp, PreviewContent, WorkspaceDef, WorkspaceItem,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    GroupByKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RightPaneMode {
    DualFolder,
    Preview,
}

pub struct TrackerPaneState {
    pub pane_id: u64,
    pub path_input: String,
    pub filter: String,
    pub listing: Option<DirListing>,
    pub selected: Vec<String>,
    pub view_mode: ViewMode,
}

impl TrackerPaneState {
    pub fn new(pane_id: u64, initial_path: &str) -> Self {
        Self {
            pane_id,
            path_input: initial_path.into(),
            filter: String::new(),
            listing: None,
            selected: Vec::new(),
            view_mode: ViewMode::List,
        }
    }
}

pub struct TrackerWindow {
    pub left: TrackerPaneState,
    pub right: TrackerPaneState,
    pub active_pane: usize, // 0 = left, 1 = right
    pub right_mode: RightPaneMode,
    /// Temporary cross-directory staging shelf
    pub shelf: Vec<FileEntry>,
    pub workspaces: Vec<WorkspaceDef>,
    pub active_workspace: Option<usize>,
    pub preview_path: Option<String>,
    pub preview_content: Option<PreviewContent>,
    pub preview_texture: Option<egui::TextureHandle>,
    pub markdown_edit_buf: String,
    pub markdown_editing: bool,
    pub new_folder_name: String,
    pub show_mkdir_bar: bool,
}

pub enum TrackerSpawnRequest {
    OpenTerminalInDir { title: String, dir: String },
    OpenUnitLogs { unit: String },
}

impl TrackerWindow {
    pub fn new(left_id: u64, right_id: u64, send_msg: &mut dyn FnMut(ClientMsg)) -> Self {
        send_msg(ClientMsg::ListDir {
            pane_id: left_id,
            path: "~".into(),
        });
        send_msg(ClientMsg::ListDir {
            pane_id: right_id,
            path: "~/Downloads".into(),
        });
        send_msg(ClientMsg::ListWorkspaces);

        Self {
            left: TrackerPaneState::new(left_id, "~"),
            right: TrackerPaneState::new(right_id, "~/Downloads"),
            active_pane: 0,
            right_mode: RightPaneMode::DualFolder,
            shelf: Vec::new(),
            workspaces: Vec::new(),
            active_workspace: None,
            preview_path: None,
            preview_content: None,
            preview_texture: None,
            markdown_edit_buf: String::new(),
            markdown_editing: false,
            new_folder_name: String::new(),
            show_mkdir_bar: false,
        }
    }

    pub fn on_dir_snapshot(&mut self, listing: DirListing) {
        if listing.pane_id == self.left.pane_id {
            self.left.path_input = listing.path.clone();
            self.left.listing = Some(listing);
        } else if listing.pane_id == self.right.pane_id {
            self.right.path_input = listing.path.clone();
            self.right.listing = Some(listing);
        }
    }

    pub fn on_preview(&mut self, ctx: &egui::Context, path: String, preview: PreviewContent) {
        self.preview_texture = None;
        if let PreviewContent::Text { ref content, .. } = preview {
            self.markdown_edit_buf = content.clone();
        } else if let PreviewContent::ImageBytes { ref bytes, .. } = preview
            && let Ok(img) = image::load_from_memory(bytes)
        {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            self.preview_texture = Some(ctx.load_texture(
                format!("tracker_preview_{path}"),
                color_image,
                egui::TextureOptions::LINEAR,
            ));
        }
        self.preview_path = Some(path);
        self.preview_content = Some(preview);
        self.right_mode = RightPaneMode::Preview;
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        send_msg: &mut dyn FnMut(ClientMsg),
    ) -> Option<TrackerSpawnRequest> {
        let mut spawn_req = None;

        let (f5_pressed, f6_pressed, tab_pressed) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::F5),
                i.key_pressed(egui::Key::F6),
                i.key_pressed(egui::Key::Tab) && !i.modifiers.any(),
            )
        });
        if f5_pressed {
            self.trigger_transfer(false, send_msg);
        } else if f6_pressed {
            self.trigger_transfer(true, send_msg);
        } else if tab_pressed && !self.markdown_editing && !self.show_mkdir_bar {
            self.active_pane = 1 - self.active_pane;
        }

        // Top BeOS Action Toolbar
        ui.horizontal_wrapped(|ui| {
            if be_button(ui, "Home").clicked() {
                self.active_workspace = None;
                let pane = self.focused_pane_mut();
                send_msg(ClientMsg::ListDir {
                    pane_id: pane.pane_id,
                    path: "~".into(),
                });
            }
            if be_button(ui, "Refresh").clicked() {
                let pane = self.focused_pane();
                send_msg(ClientMsg::ListDir {
                    pane_id: pane.pane_id,
                    path: pane.path_input.clone(),
                });
            }
            if be_button(ui, "New Folder").clicked() {
                self.show_mkdir_bar = !self.show_mkdir_bar;
            }
            if be_button(ui, "Copy F5").clicked() {
                self.trigger_transfer(false, send_msg);
            }
            if be_button(ui, "Move F6").clicked() {
                self.trigger_transfer(true, send_msg);
            }
            if be_button(ui, "Trash").clicked() {
                let pane = self.focused_pane_mut();
                if !pane.selected.is_empty() {
                    let paths = std::mem::take(&mut pane.selected);
                    send_msg(ClientMsg::PerformFileOp {
                        pane_id: pane.pane_id,
                        op: FileOp::Trash { paths },
                    });
                }
            }
            if be_button(ui, "Undo").clicked() {
                let pid = self.focused_pane().pane_id;
                send_msg(ClientMsg::UndoLastFileOp { pane_id: pid });
            }

            ui.separator();

            let group_label = if self.focused_pane().view_mode == ViewMode::List {
                "Group by Kind"
            } else {
                "Flat List"
            };
            if be_button(ui, group_label).clicked() {
                let pane = self.focused_pane_mut();
                pane.view_mode = match pane.view_mode {
                    ViewMode::List => ViewMode::GroupByKind,
                    ViewMode::GroupByKind => ViewMode::List,
                };
            }

            let right_label = match self.right_mode {
                RightPaneMode::DualFolder => "Peek Pane",
                RightPaneMode::Preview => "Dual Pane",
            };
            if be_button(ui, right_label).clicked() {
                self.right_mode = match self.right_mode {
                    RightPaneMode::DualFolder => RightPaneMode::Preview,
                    RightPaneMode::Preview => RightPaneMode::DualFolder,
                };
            }

            if be_button(ui, ">_ Terminal").clicked() {
                let dir = self.focused_pane().path_input.clone();
                spawn_req = Some(TrackerSpawnRequest::OpenTerminalInDir {
                    title: format!("term: {dir}"),
                    dir,
                });
            }
        });

        if self.show_mkdir_bar {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Folder name:").color(crate::theme::text_color()));
                ui.text_edit_singleline(&mut self.new_folder_name);
                if be_button(ui, "Create").clicked() && !self.new_folder_name.trim().is_empty() {
                    let pane = self.focused_pane();
                    send_msg(ClientMsg::PerformFileOp {
                        pane_id: pane.pane_id,
                        op: FileOp::Mkdir {
                            parent: pane.path_input.clone(),
                            name: self.new_folder_name.trim().to_string(),
                        },
                    });
                    self.new_folder_name.clear();
                    self.show_mkdir_bar = false;
                }
            });
        }

        ui.separator();

        let shelf_reserve = if self.shelf.is_empty() { 0.0 } else { 32.0 };
        let body_height = (ui.available_height() - shelf_reserve).max(160.0);

        ui.allocate_ui(egui::Vec2::new(ui.available_width(), body_height), |ui| {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(148.0, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.label(RichText::new("LOCATIONS").strong().color(crate::theme::text_color()));
                        for (label, target) in [
                            ("Home (~)", "~"),
                            ("Projects", "~/projects"),
                            ("Downloads", "~/Downloads"),
                            ("Systemd", "/etc/systemd/system"),
                            ("Mounts (/mnt)", "/mnt"),
                        ] {
                            if ui
                                .selectable_label(false, RichText::new(label).color(crate::theme::text_color()))
                                .clicked()
                            {
                                self.active_workspace = None;
                                let pid = self.focused_pane().pane_id;
                                send_msg(ClientMsg::ListDir {
                                    pane_id: pid,
                                    path: target.into(),
                                });
                            }
                        }

                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("WORKSPACES").strong().color(crate::theme::text_color()));
                            if ui.small_button("+").clicked() {
                                let path = self.focused_pane().path_input.clone();
                                let ws = WorkspaceDef {
                                    name: format!("WS {}", self.workspaces.len() + 1),
                                    items: vec![WorkspaceItem {
                                        label: path.clone(),
                                        path,
                                        kind: FileKind::Directory,
                                    }],
                                };
                                send_msg(ClientMsg::SaveWorkspace(ws));
                            }
                        });
                        for (idx, ws) in self.workspaces.iter().enumerate() {
                            let selected = self.active_workspace == Some(idx);
                            if ui
                                .selectable_label(
                                    selected,
                                    RichText::new(&ws.name).color(crate::theme::text_color()),
                                )
                                .clicked()
                            {
                                self.active_workspace = Some(idx);
                            }
                        }
                    },
                );

                ui.separator();

                let content_w = ui.available_width().max(240.0);
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(content_w, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.columns(2, |panes| {
                            let left_focused = self.active_pane == 0;
                            if let Some(ws_idx) = self.active_workspace
                                && let Some(ws) = self.workspaces.get(ws_idx)
                            {
                                let mut open_path = None;
                                panes[0].label(
                                    RichText::new(format!("Workspace: {}", ws.name))
                                        .strong()
                                        .color(crate::theme::text_color()),
                                );
                                panes[0].separator();
                                for item in &ws.items {
                                    if panes[0]
                                        .selectable_label(
                                            false,
                                            RichText::new(format!(
                                                "[{:?}] {}",
                                                item.kind, item.label
                                            ))
                                            .color(crate::theme::text_color()),
                                        )
                                        .clicked()
                                    {
                                        open_path = Some((item.kind, item.path.clone()));
                                    }
                                }
                                if let Some((kind, path)) = open_path {
                                    self.active_workspace = None;
                                    if kind == FileKind::Directory {
                                        send_msg(ClientMsg::ListDir {
                                            pane_id: self.left.pane_id,
                                            path,
                                        });
                                    } else {
                                        send_msg(ClientMsg::PreviewFile {
                                            pane_id: self.left.pane_id,
                                            path,
                                        });
                                    }
                                }
                            } else if let Some(req) = render_folder_pane(
                                &mut panes[0],
                                &mut self.left,
                                left_focused,
                                &mut self.shelf,
                                send_msg,
                            ) {
                                spawn_req = Some(req);
                            }
                            if panes[0].ui_contains_pointer()
                                && panes[0].input(|i| i.pointer.any_click())
                            {
                                self.active_pane = 0;
                            }

                            match self.right_mode {
                                RightPaneMode::DualFolder => {
                                    let right_focused = self.active_pane == 1;
                                    if let Some(req) = render_folder_pane(
                                        &mut panes[1],
                                        &mut self.right,
                                        right_focused,
                                        &mut self.shelf,
                                        send_msg,
                                    ) {
                                        spawn_req = Some(req);
                                    }
                                    if panes[1].ui_contains_pointer()
                                        && panes[1].input(|i| i.pointer.any_click())
                                    {
                                        self.active_pane = 1;
                                    }
                                }
                                RightPaneMode::Preview => {
                                    render_preview_pane(
                                        &mut panes[1],
                                        &self.preview_path,
                                        &self.preview_content,
                                        self.preview_texture.as_ref(),
                                        &mut self.markdown_edit_buf,
                                        &mut self.markdown_editing,
                                        self.left.pane_id,
                                        send_msg,
                                    );
                                }
                            }
                        });
                    },
                );
            });
        });

        // Bottom Holding Shelf (Cross-directory staging bar)
        if !self.shelf.is_empty() {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Shelf ({}):", self.shelf.len()))
                        .strong()
                        .color(crate::theme::text_color()),
                );
                let pal = palette();
                for item in &self.shelf {
                    ui.label(
                        RichText::new(&item.name)
                            .background_color(pal.tab_active_bg)
                            .color(pal.tab_active_fg),
                    );
                }
                if be_button(ui, "Move Here").clicked() {
                    let sources: Vec<String> = self.shelf.drain(..).map(|e| e.path).collect();
                    let pane = self.focused_pane();
                    send_msg(ClientMsg::PerformFileOp {
                        pane_id: pane.pane_id,
                        op: FileOp::Move {
                            sources,
                            dest_dir: pane.path_input.clone(),
                        },
                    });
                }
                if be_button(ui, "Clear").clicked() {
                    self.shelf.clear();
                }
            });
        }

        spawn_req
    }

    fn focused_pane(&self) -> &TrackerPaneState {
        if self.active_pane == 0 {
            &self.left
        } else {
            &self.right
        }
    }

    fn focused_pane_mut(&mut self) -> &mut TrackerPaneState {
        if self.active_pane == 0 {
            &mut self.left
        } else {
            &mut self.right
        }
    }

    fn trigger_transfer(&mut self, is_move: bool, send_msg: &mut dyn FnMut(ClientMsg)) {
        let (src, dst) = if self.active_pane == 0 {
            (&mut self.left, &self.right)
        } else {
            (&mut self.right, &self.left)
        };
        if src.selected.is_empty() {
            return;
        }
        let sources = std::mem::take(&mut src.selected);
        let dest_dir = dst.path_input.clone();
        let op = if is_move {
            FileOp::Move { sources, dest_dir }
        } else {
            FileOp::Copy { sources, dest_dir }
        };
        send_msg(ClientMsg::PerformFileOp {
            pane_id: dst.pane_id,
            op,
        });
    }
}

fn render_folder_pane(
    ui: &mut Ui,
    pane: &mut TrackerPaneState,
    focused: bool,
    shelf: &mut Vec<FileEntry>,
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<TrackerSpawnRequest> {
    let mut spawn_req = None;

    ui.horizontal(|ui| {
        if focused {
            ui.label(RichText::new("▶").color(crate::theme::text_color()));
        }
        if be_button(ui, "↑").clicked()
            && let Some(parent) = std::path::Path::new(&pane.path_input).parent()
        {
            send_msg(ClientMsg::ListDir {
                pane_id: pane.pane_id,
                path: parent.display().to_string(),
            });
        }
        let resp = ui.text_edit_singleline(&mut pane.path_input);
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            send_msg(ClientMsg::ListDir {
                pane_id: pane.pane_id,
                path: pane.path_input.clone(),
            });
        }
        if let Some(listing) = &pane.listing
            && let Some(git) = &listing.git
        {
            let label = if git.dirty_count > 0 {
                format!("git:{} *{}", git.branch, git.dirty_count)
            } else {
                format!("git:{}", git.branch)
            };
            let pal = palette();
            ui.label(
                RichText::new(label)
                    .background_color(pal.tab_active_bg)
                    .color(pal.tab_active_fg),
            );
        }
    });

    ui.horizontal(|ui| {
        ui.label(RichText::new("Filter:").color(crate::theme::text_color()));
        ui.text_edit_singleline(&mut pane.filter);
    });
    ui.separator();

    let (entries, disk_free_bytes, disk_total_bytes) = {
        let Some(listing) = &pane.listing else {
            ui.label(RichText::new("Loading...").color(crate::theme::text_color()));
            return None;
        };
        let needle = pane.filter.to_lowercase();
        let filtered: Vec<FileEntry> = listing
            .entries
            .iter()
            .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
            .cloned()
            .collect();
        (filtered, listing.disk_free_bytes, listing.disk_total_bytes)
    };

    let list_h = (ui.available_height() - 20.0).max(80.0);
    ScrollArea::vertical()
        .id_salt(("pane_scroll", pane.pane_id))
        .max_height(list_h)
        .show(ui, |ui| match pane.view_mode {
            ViewMode::List => {
                for entry in &entries {
                    if let Some(req) = render_entry_row(ui, pane, entry, shelf, send_msg) {
                        spawn_req = Some(req);
                    }
                }
            }
            ViewMode::GroupByKind => {
                let mut groups: BTreeMap<String, Vec<&FileEntry>> = BTreeMap::new();
                for e in &entries {
                    groups.entry(format!("{:?}", e.kind)).or_default().push(e);
                }
                for (kind_name, group_entries) in groups {
                    egui::CollapsingHeader::new(
                        RichText::new(format!("{kind_name} ({})", group_entries.len()))
                            .strong()
                            .color(crate::theme::text_color()),
                    )
                    .default_open(true)
                    .show(ui, |ui| {
                        for entry in group_entries {
                            if let Some(req) = render_entry_row(ui, pane, entry, shelf, send_msg) {
                                spawn_req = Some(req);
                            }
                        }
                    });
                }
            }
        });

    let free_gb = disk_free_bytes as f64 / 1_073_741_824.0;
    let total_gb = disk_total_bytes as f64 / 1_073_741_824.0;
    ui.label(
        RichText::new(format!(
            "{} items · {:.1}/{:.1} GB free",
            entries.len(),
            free_gb,
            total_gb
        ))
        .small()
        .color(palette().text_muted),
    );

    spawn_req
}

fn render_entry_row(
    ui: &mut Ui,
    pane: &mut TrackerPaneState,
    entry: &FileEntry,
    shelf: &mut Vec<FileEntry>,
    send_msg: &mut dyn FnMut(ClientMsg),
) -> Option<TrackerSpawnRequest> {
    let mut spawn_req = None;
    let is_selected = pane.selected.contains(&entry.path);

    ui.horizontal(|ui| {
        let icon = match entry.kind {
            FileKind::Directory => "[DIR]",
            FileKind::Markdown => "[MD]",
            FileKind::ModelWeights => "[ML]",
            FileKind::Archive => "[ZIP]",
            FileKind::Image => "[IMG]",
            FileKind::Video => "[VID]",
            FileKind::Audio => "[AUD]",
            FileKind::Code => "[SRC]",
            FileKind::Installer => "[PKG]",
            _ => "[FILE]",
        };

        let pal = palette();
        let name_color = if entry.symlink_broken {
            pal.status_red
        } else {
            pal.text
        };

        let display_name = if let Some(ref target) = entry.symlink_target {
            format!("{icon} {} -> {target}", entry.name)
        } else {
            format!("{icon} {}", entry.name)
        };

        let resp = ui.selectable_label(is_selected, RichText::new(display_name).color(name_color));

        if resp.clicked() {
            if is_selected {
                pane.selected.retain(|p| p != &entry.path);
            } else {
                pane.selected.push(entry.path.clone());
            }
            if entry.kind != FileKind::Directory {
                send_msg(ClientMsg::PreviewFile {
                    pane_id: pane.pane_id,
                    path: entry.path.clone(),
                });
            }
        }
        if resp.double_clicked() && entry.kind == FileKind::Directory {
            pane.selected.clear();
            send_msg(ClientMsg::ListDir {
                pane_id: pane.pane_id,
                path: entry.path.clone(),
            });
        }

        if let Some(unit) = &entry.held_by_unit
            && ui
                .small_button(RichText::new(format!("● {unit}")).color(pal.status_green))
                .clicked()
        {
            spawn_req = Some(TrackerSpawnRequest::OpenUnitLogs { unit: unit.clone() });
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("+Shelf").clicked() && !shelf.iter().any(|s| s.path == entry.path) {
                shelf.push(entry.clone());
            }
            ui.label(
                RichText::new(format!("{:04o}", entry.mode & 0o7777))
                    .monospace()
                    .small()
                    .color(pal.text_muted),
            );
            if entry.kind != FileKind::Directory {
                ui.label(
                    RichText::new(format_bytes(entry.size))
                        .small()
                        .color(pal.text_muted),
                );
            }
        });
    });

    spawn_req
}

#[allow(clippy::too_many_arguments)]
fn render_preview_pane(
    ui: &mut Ui,
    preview_path: &Option<String>,
    preview: &Option<PreviewContent>,
    preview_texture: Option<&egui::TextureHandle>,
    edit_buf: &mut String,
    editing: &mut bool,
    pane_id: u64,
    send_msg: &mut dyn FnMut(ClientMsg),
) {
    let Some(path) = preview_path else {
        ui.label(RichText::new("Click any file to Peek").color(crate::theme::text_color()));
        return;
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new(path).strong().color(crate::theme::text_color()));
        if let Some(PreviewContent::Text {
            is_markdown: true, ..
        }) = preview
        {
            if be_button(ui, if *editing { "Preview" } else { "Edit" }).clicked() {
                *editing = !*editing;
            }
            if *editing && be_button(ui, "Save").clicked() {
                send_msg(ClientMsg::PerformFileOp {
                    pane_id,
                    op: FileOp::WriteText {
                        path: path.clone(),
                        content: edit_buf.clone(),
                    },
                });
            }
        }
    });
    ui.separator();

    ScrollArea::vertical()
        .id_salt("tracker_preview_scroll")
        .show(ui, |ui| match preview {
            Some(PreviewContent::Text {
                content,
                is_markdown,
                truncated,
            }) => {
                if *truncated {
                    ui.label(
                        RichText::new("(Truncated to first 256 KB)")
                            .small()
                            .italics()
                            .color(crate::theme::palette().accent),
                    );
                }
                if *editing {
                    ui.add(
                        egui::TextEdit::multiline(edit_buf)
                            .code_editor()
                            .desired_width(f32::INFINITY),
                    );
                } else if *is_markdown {
                    render_markdown_preview(ui, content);
                } else if path.to_ascii_lowercase().ends_with(".csv") {
                    render_csv_preview(ui, content);
                } else {
                    render_code_preview(ui, content);
                }
            }
            Some(PreviewContent::ImageBytes { mime, bytes }) => {
                if let Some(tex) = preview_texture {
                    let tex_size = tex.size_vec2();
                    ui.label(
                        RichText::new(format!(
                            "{} × {} px  ·  {}  ·  {}",
                            tex_size.x as u32,
                            tex_size.y as u32,
                            mime,
                            format_bytes(bytes.len() as u64)
                        ))
                        .monospace()
                        .size(10.5)
                        .color(crate::theme::muted_text_color()),
                    );
                    ui.add_space(4.0);
                    let max_w = ui.available_width().max(120.0);
                    let scale = (max_w / tex_size.x.max(1.0)).min(1.0);
                    ui.image((tex.id(), tex_size * scale));
                } else {
                    ui.label(
                        RichText::new(format!("Could not decode {mime} image preview"))
                            .color(crate::theme::text_color()),
                    );
                }
            }
            Some(PreviewContent::ModelHeader { format, metadata }) => {
                let pal = crate::theme::palette();
                ui.label(
                    RichText::new(format!("Model Checkpoint: {format}"))
                        .strong()
                        .size(13.0)
                        .color(pal.accent),
                );
                ui.add_space(4.0);
                egui::Frame::NONE
                    .fill(pal.well)
                    .stroke(egui::Stroke::new(1.0, pal.border))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        for (k, v) in metadata {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("{k}:"))
                                        .monospace()
                                        .strong()
                                        .color(pal.status_sky),
                                );
                                ui.label(RichText::new(v).monospace().color(pal.text));
                            });
                        }
                    });
            }
            Some(PreviewContent::ArchiveEntries { entries }) => {
                let pal = crate::theme::palette();
                let total_uncomp: u64 = entries.iter().map(|(_, s)| *s).sum();
                ui.label(
                    RichText::new(format!(
                        "Archive Contents: {} entries · {} uncompressed",
                        entries.len(),
                        format_bytes(total_uncomp)
                    ))
                    .strong()
                    .size(11.5)
                    .color(pal.accent),
                );
                ui.add_space(4.0);
                if entries.is_empty() {
                    ui.label(
                        RichText::new("(Empty archive)")
                            .italics()
                            .color(pal.text_muted),
                    );
                } else {
                    egui::Frame::NONE
                        .fill(pal.well)
                        .stroke(egui::Stroke::new(1.0, pal.border))
                        .inner_margin(6.0)
                        .show(ui, |ui| {
                            for (rel, sz) in entries {
                                let is_dir = rel.ends_with('/');
                                ui.horizontal(|ui| {
                                    let tag = if is_dir { "[DIR]" } else { "     " };
                                    ui.label(
                                        RichText::new(format!("{tag} {rel}"))
                                            .monospace()
                                            .size(11.0)
                                            .color(if is_dir { pal.accent } else { pal.text }),
                                    );
                                    if !is_dir {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(format_bytes(*sz))
                                                        .monospace()
                                                        .size(10.5)
                                                        .color(pal.text_muted),
                                                );
                                            },
                                        );
                                    }
                                });
                            }
                        });
                }
            }
            Some(PreviewContent::Unsupported { reason }) => {
                ui.label(RichText::new(reason).color(crate::theme::text_color()));
            }
            None => {}
        });
}

fn render_markdown_preview(ui: &mut Ui, content: &str) {
    let pal = crate::theme::palette();
    let mut in_code_block = false;
    let mut code_buf = String::new();

    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            if in_code_block {
                egui::Frame::NONE
                    .fill(pal.well)
                    .stroke(egui::Stroke::new(1.0, pal.border))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            RichText::new(code_buf.trim_end())
                                .monospace()
                                .size(11.0)
                                .color(pal.text),
                        );
                    });
                code_buf.clear();
                in_code_block = false;
            } else {
                in_code_block = true;
            }
            continue;
        }
        if in_code_block {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            ui.add_space(4.0);
        } else if trimmed == "---" || trimmed == "***" {
            ui.separator();
        } else if let Some(h) = trimmed.strip_prefix("### ") {
            ui.label(RichText::new(h).strong().size(13.5).color(pal.accent));
        } else if let Some(h) = trimmed.strip_prefix("## ") {
            ui.add_space(3.0);
            ui.label(RichText::new(h).strong().size(15.0).color(pal.accent));
        } else if let Some(h) = trimmed.strip_prefix("# ") {
            ui.add_space(4.0);
            ui.label(RichText::new(h).strong().size(17.5).color(pal.text));
            ui.separator();
        } else if let Some(q) = trimmed.strip_prefix("> ") {
            egui::Frame::NONE
                .fill(pal.section_bg)
                .stroke(egui::Stroke::new(1.0, pal.accent))
                .inner_margin(5.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(q).italics().color(pal.text_muted));
                });
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* "))
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("•").strong().color(pal.accent));
                ui.label(RichText::new(item.replace("**", "")).color(pal.text));
            });
        } else {
            ui.label(RichText::new(trimmed.replace("**", "")).color(pal.text));
        }
    }
}

fn render_csv_preview(ui: &mut Ui, content: &str) {
    let pal = crate::theme::palette();
    egui::Frame::NONE
        .fill(pal.well)
        .stroke(egui::Stroke::new(1.0, pal.border))
        .inner_margin(6.0)
        .show(ui, |ui| {
            for (row_idx, line) in content.lines().take(60).enumerate() {
                let cols: Vec<&str> = line.split(',').collect();
                let formatted = cols
                    .iter()
                    .map(|c| format!("{:<16}", c.trim().chars().take(15).collect::<String>()))
                    .collect::<Vec<_>>()
                    .join(" │ ");
                ui.label(
                    RichText::new(formatted)
                        .monospace()
                        .size(10.5)
                        .color(if row_idx == 0 { pal.accent } else { pal.text }),
                );
                if row_idx == 0 {
                    ui.separator();
                }
            }
        });
}

fn render_code_preview(ui: &mut Ui, content: &str) {
    let pal = crate::theme::palette();
    egui::Frame::NONE
        .fill(pal.well)
        .stroke(egui::Stroke::new(1.0, pal.border))
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            for (idx, line) in content.lines().take(400).enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{:>4}", idx + 1))
                            .monospace()
                            .size(10.5)
                            .color(pal.text_muted),
                    );
                    ui.label(
                        RichText::new(line)
                            .monospace()
                            .size(11.0)
                            .color(pal.text),
                    );
                });
            }
        });
}

fn format_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = b as f64;
    let mut idx = 0;
    while size >= 1024.0 && idx < UNITS.len() - 1 {
        size /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{b} B")
    } else {
        format!("{size:.1} {}", UNITS[idx])
    }
}
