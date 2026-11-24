use crate::theme::palette;
#[cfg(not(target_arch = "wasm32"))]
use crate::theme::be_button;
use egui::{Rect, Ui};
#[cfg(not(target_arch = "wasm32"))]
use egui::{CornerRadius, RichText, ScrollArea, Stroke};
use kaage_proto::{ClientMsg, CopilotCard, CopilotProvider, CopilotStatus, TrustMode};

#[derive(Debug, Clone)]
pub enum CopilotCrossAction {
    ViewInVitals,
    OpenInTracker { path: String },
    TailUnitLogs { unit: String },
    OpenCopilotSettings,
    SendClientMsg(ClientMsg),
}

#[derive(Debug, Clone)]
pub enum CopilotItem {
    User(String),
    Assistant {
        request_id: u64,
        text: String,
        done: bool,
    },
    ToolStatus {
        tool_name: String,
        target_node: String,
        state: String,
    },
    Card(CopilotCard),
    SystemNotice(String),
}

pub struct CopilotWindow {
    pub items: Vec<CopilotItem>,
    #[cfg(not(target_arch = "wasm32"))]
    pub input: String,
    pub active_request_id: Option<u64>,
    pub next_request_id: u64,
    pub dom_dirty: bool,
}

impl CopilotWindow {
    pub fn new() -> Self {
        Self {
            items: vec![CopilotItem::SystemNotice(
                "Kaage Copilot ready. Try asking \"Why is memory high?\" or use /provider, /model, /mode, /node, /status."
                    .into(),
            )],
            #[cfg(not(target_arch = "wasm32"))]
            input: String::new(),
            active_request_id: None,
            next_request_id: 1,
            dom_dirty: true,
        }
    }

    pub fn on_chunk(&mut self, request_id: u64, delta: String, done: bool) {
        if let Some(CopilotItem::Assistant {
            request_id: rid,
            text,
            done: is_done,
        }) = self.items.last_mut()
            && *rid == request_id
        {
            text.push_str(&delta);
            if done {
                *is_done = true;
            }
        } else {
            self.items.push(CopilotItem::Assistant {
                request_id,
                text: delta,
                done,
            });
        }
        if done && self.active_request_id == Some(request_id) {
            self.active_request_id = None;
        }
        self.dom_dirty = true;
    }

    pub fn on_tool_status(&mut self, tool_name: String, target_node: String, state: String) {
        self.items.push(CopilotItem::ToolStatus {
            tool_name,
            target_node,
            state,
        });
        self.dom_dirty = true;
    }

    pub fn on_card(&mut self, card: CopilotCard) {
        self.items.push(CopilotItem::Card(card));
        self.dom_dirty = true;
    }

    pub fn handle_prompt(&mut self, raw: &str) -> Vec<CopilotCrossAction> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        self.dom_dirty = true;

        // Built-in slash commands handled locally without LLM round-trip
        if let Some(rest) = trimmed.strip_prefix('/') {
            let mut parts = rest.split_whitespace();
            let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
            let arg = parts.next().unwrap_or("");
            match cmd.as_str() {
                "provider" => {
                    let provider = match arg.to_ascii_lowercase().as_str() {
                        "openai" => Some(CopilotProvider::OpenAi),
                        "gemini" => Some(CopilotProvider::Gemini),
                        "claude" | "anthropic" => Some(CopilotProvider::Claude),
                        "local" | "ollama" => Some(CopilotProvider::Local),
                        _ => None,
                    };
                    if let Some(p) = provider {
                        self.items.push(CopilotItem::SystemNotice(format!(
                            "Switching provider to `{}`...",
                            p.label()
                        )));
                        return vec![CopilotCrossAction::SendClientMsg(
                            ClientMsg::SetCopilotConfig {
                                provider: Some(p),
                                model: None,
                                trust_mode: None,
                                target_node: None,
                                local_endpoint: None,
                            },
                        )];
                    }
                    self.items.push(CopilotItem::SystemNotice(
                        "Usage: /provider <gemini|openai|claude|local>".into(),
                    ));
                    return Vec::new();
                }
                "model" => {
                    if !arg.is_empty() {
                        self.items.push(CopilotItem::SystemNotice(format!(
                            "Setting model to `{arg}`..."
                        )));
                        return vec![CopilotCrossAction::SendClientMsg(
                            ClientMsg::SetCopilotConfig {
                                provider: None,
                                model: Some(arg.to_string()),
                                trust_mode: None,
                                target_node: None,
                                local_endpoint: None,
                            },
                        )];
                    }
                    self.items
                        .push(CopilotItem::SystemNotice("Usage: /model <model-id>".into()));
                    return Vec::new();
                }
                "mode" => {
                    let mode = match arg.to_ascii_lowercase().as_str() {
                        "monitored" | "readonly" | "read-only" => Some(TrustMode::Monitored),
                        "manage" | "managed" => Some(TrustMode::Managed),
                        _ => None,
                    };
                    if let Some(m) = mode {
                        self.items.push(CopilotItem::SystemNotice(format!(
                            "Switching trust mode to `{}`...",
                            m.as_str()
                        )));
                        return vec![CopilotCrossAction::SendClientMsg(
                            ClientMsg::SetCopilotConfig {
                                provider: None,
                                model: None,
                                trust_mode: Some(m),
                                target_node: None,
                                local_endpoint: None,
                            },
                        )];
                    }
                    self.items.push(CopilotItem::SystemNotice(
                        "Usage: /mode <monitored|manage>".into(),
                    ));
                    return Vec::new();
                }
                "node" => {
                    if !arg.is_empty() {
                        self.items.push(CopilotItem::SystemNotice(format!(
                            "Targeting node `{arg}`..."
                        )));
                        return vec![CopilotCrossAction::SendClientMsg(
                            ClientMsg::SetCopilotConfig {
                                provider: None,
                                model: None,
                                trust_mode: None,
                                target_node: Some(arg.to_string()),
                                local_endpoint: None,
                            },
                        )];
                    }
                    self.items
                        .push(CopilotItem::SystemNotice("Usage: /node <hostname>".into()));
                    return Vec::new();
                }
                "status" => {
                    self.items
                        .push(CopilotItem::SystemNotice("Refreshing Copilot status...".into()));
                    return vec![CopilotCrossAction::SendClientMsg(
                        ClientMsg::GetCopilotStatus,
                    )];
                }
                "clear" => {
                    self.items.clear();
                    return Vec::new();
                }
                _ => {
                    self.items.push(CopilotItem::SystemNotice(
                        "Commands: /provider <gemini|openai>, /model <id>, /mode <monitored|manage>, /node <host>, /status, /clear"
                            .into(),
                    ));
                    return Vec::new();
                }
            }
        }

        let req_id = self.next_request_id;
        self.next_request_id += 1;
        self.active_request_id = Some(req_id);
        self.items.push(CopilotItem::User(trimmed.to_string()));
        vec![CopilotCrossAction::SendClientMsg(ClientMsg::AskCopilot {
            request_id: req_id,
            prompt: trimmed.to_string(),
        })]
    }

    /// Render the pane. On WASM, syncs the Hybrid DOM Overlay to `pane_rect` and drains any
    /// dispatched DOM actions; on native desktop, renders an equivalent egui UI.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        pane_rect: Rect,
        status: Option<&CopilotStatus>,
        dragging_window: bool,
        z_index: i32,
    ) -> Vec<CopilotCrossAction> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = ui;
            wasm_overlay::sync_theme_css();
            wasm_overlay::sync_pane_geometry(pane_rect, true, dragging_window, z_index);
            if self.dom_dirty {
                wasm_overlay::render_copilot_dom(self, status);
                self.dom_dirty = false;
            }
            return wasm_overlay::drain_dom_actions(self, status);
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (pane_rect, dragging_window, z_index);
            self.render_native_egui(ui, status)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn render_native_egui(
        &mut self,
        ui: &mut Ui,
        status: Option<&CopilotStatus>,
    ) -> Vec<CopilotCrossAction> {
        let p = palette();
        let mut actions = Vec::new();

        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("✦ Kaage Copilot").strong().color(p.accent));
            if let Some(st) = status {
                ui.label(
                    RichText::new(format!(
                        "[{} · {}] [{}] [@{}]",
                        st.provider.as_str(),
                        st.model,
                        st.trust_mode.as_str(),
                        st.target_node
                    ))
                    .monospace()
                    .size(10.5)
                    .color(if st.ready { p.status_green } else { p.status_amber }),
                );
                if be_button(ui, "⚙ Keys / Backends").clicked() {
                    actions.push(CopilotCrossAction::OpenCopilotSettings);
                }
            }
        });
        if let Some(st) = status
            && !st.ready
        {
            egui::Frame::NONE
                .fill(p.well)
                .stroke(Stroke::new(1.0, p.status_red))
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "⚠ Missing {} for {}.",
                                st.provider.env_var_name(),
                                st.provider.label()
                            ))
                            .color(p.status_red),
                        );
                        if be_button(ui, "Configure API Key & Backend").clicked() {
                            actions.push(CopilotCrossAction::OpenCopilotSettings);
                        }
                    });
                });
        }
        ui.separator();

        let scroll_h = (ui.available_height() - 42.0).max(80.0);
        ScrollArea::vertical()
            .id_salt("copilot_native_scroll")
            .max_height(scroll_h)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for item in &self.items {
                    match item {
                        CopilotItem::User(txt) => {
                            egui::Frame::NONE
                                .fill(p.surface_alt)
                                .stroke(Stroke::new(1.0, p.accent))
                                .inner_margin(6.0)
                                .show(ui, |ui| {
                                    ui.label(RichText::new(format!("You: {txt}")).color(p.text));
                                });
                        }
                        CopilotItem::Assistant { text, done, .. } => {
                            egui::Frame::NONE
                                .fill(p.well)
                                .stroke(Stroke::new(1.0, p.border))
                                .inner_margin(6.0)
                                .show(ui, |ui| {
                                    let suffix = if *done { "" } else { " ▌" };
                                    ui.label(RichText::new(format!("{text}{suffix}")).color(p.text));
                                });
                        }
                        CopilotItem::ToolStatus {
                            tool_name,
                            target_node,
                            state,
                        } => {
                            ui.label(
                                RichText::new(format!("⚙ {tool_name} @{target_node} ({state})"))
                                    .monospace()
                                    .size(10.0)
                                    .color(p.text_muted),
                            );
                        }
                        CopilotItem::SystemNotice(msg) => {
                            ui.label(RichText::new(msg).italics().size(11.0).color(p.text_muted));
                        }
                        CopilotItem::Card(card) => {
                            render_native_card(ui, card, &mut actions);
                        }
                    }
                    ui.add_space(4.0);
                }
            });

        ui.separator();
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .hint_text("Ask Kaage Copilot or type /provider, /model, /mode, /node...")
                    .desired_width((ui.available_width() - 90.0).max(120.0)),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (be_button(ui, "Send").clicked() || enter) && !self.input.trim().is_empty() {
                let prompt = std::mem::take(&mut self.input);
                actions.extend(self.handle_prompt(&prompt));
            }
            if let Some(rid) = self.active_request_id
                && be_button(ui, "Stop").clicked()
            {
                actions.push(CopilotCrossAction::SendClientMsg(ClientMsg::CancelCopilot {
                    request_id: rid,
                }));
            }
        });

        actions
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn render_native_card(ui: &mut Ui, card: &CopilotCard, actions: &mut Vec<CopilotCrossAction>) {
    let p = palette();
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.accent))
        .corner_radius(CornerRadius::ZERO)
        .inner_margin(6.0)
        .show(ui, |ui| match card {
            CopilotCard::UnitCard {
                unit,
                active_state,
                sub_state,
                summary,
                error_lines,
            } => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(unit).monospace().strong().color(p.accent));
                    ui.label(
                        RichText::new(format!("{active_state}/{sub_state} · {summary}"))
                            .monospace()
                            .size(10.5)
                            .color(p.text_muted),
                    );
                    if be_button(ui, "Tail Logs").clicked() {
                        actions.push(CopilotCrossAction::TailUnitLogs { unit: unit.clone() });
                    }
                });
                for line in error_lines.iter().take(4) {
                    ui.label(RichText::new(line).monospace().size(10.0).color(p.status_red));
                }
            }
            CopilotCard::VitalsSlice {
                metric_label,
                top_consumers,
                ..
            } => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(metric_label).strong().color(p.status_sky));
                    if be_button(ui, "View in Vitals").clicked() {
                        actions.push(CopilotCrossAction::ViewInVitals);
                    }
                });
                for (name, val) in top_consumers {
                    ui.label(
                        RichText::new(format!("• {name}: {val}"))
                            .monospace()
                            .size(10.5)
                            .color(p.text),
                    );
                }
            }
            CopilotCard::NixDiff {
                file_path,
                diff,
                summary,
                ..
            } => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(file_path).monospace().strong().color(p.accent));
                    if be_button(ui, "Open in Tracker").clicked() {
                        actions.push(CopilotCrossAction::OpenInTracker {
                            path: file_path.clone(),
                        });
                    }
                });
                ui.label(RichText::new(summary).color(p.text));
                ui.label(RichText::new(diff).monospace().size(10.0).color(p.text_muted));
            }
            CopilotCard::ConfirmGate {
                action_id,
                target_node,
                summary,
                preview,
                rollback_cmd,
            } => {
                ui.label(
                    RichText::new(format!("⚠ Confirm on @{target_node}: {summary}"))
                        .strong()
                        .color(p.status_amber),
                );
                ui.label(RichText::new(preview).monospace().size(10.5).color(p.text));
                ui.label(
                    RichText::new(format!("Rollback: {rollback_cmd}"))
                        .monospace()
                        .size(10.0)
                        .color(p.text_muted),
                );
                ui.horizontal(|ui| {
                    if be_button(ui, "Approve").clicked() {
                        actions.push(CopilotCrossAction::SendClientMsg(
                            ClientMsg::CopilotConfirmResponse {
                                action_id: action_id.clone(),
                                approved: true,
                            },
                        ));
                    }
                    if be_button(ui, "Reject").clicked() {
                        actions.push(CopilotCrossAction::SendClientMsg(
                            ClientMsg::CopilotConfirmResponse {
                                action_id: action_id.clone(),
                                approved: false,
                            },
                        ));
                    }
                });
            }
            CopilotCard::MemoryNote {
                saved,
                category,
                summary,
            } => {
                let tag = if *saved { "Saved Memory" } else { "Recalled Memory" };
                ui.label(
                    RichText::new(format!("[{tag} · {category}] {summary}"))
                        .size(10.5)
                        .color(p.status_sky),
                );
            }
        });
}

/// Hide the Hybrid DOM Overlay when no Copilot pane is visible/active this frame.
pub fn hide_copilot_overlay() {
    #[cfg(target_arch = "wasm32")]
    wasm_overlay::sync_pane_geometry(Rect::NOTHING, false, false, 0);
}

/// Returns true if an HTML `<textarea>` / `<input>` inside the DOM overlay currently holds focus.
pub fn is_dom_overlay_focused() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window()
            && let Some(doc) = window.document()
            && let Some(active) = doc.active_element()
        {
            let tag = active.tag_name().to_ascii_uppercase();
            return tag == "TEXTAREA" || tag == "INPUT";
        }
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_overlay {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    #[derive(Debug, Clone)]
    enum RawDomEvent {
        Submit(String),
        Cancel,
        Approve(String),
        Reject(String),
        ViewVitals,
        OpenTracker(String),
        TailLogs(String),
        OpenSettings,
        SetKey(String),
    }

    static DOM_QUEUE: OnceLock<Mutex<Vec<RawDomEvent>>> = OnceLock::new();
    static BRIDGE_INSTALLED: OnceLock<()> = OnceLock::new();

    fn queue() -> &'static Mutex<Vec<RawDomEvent>> {
        DOM_QUEUE.get_or_init(|| Mutex::new(Vec::new()))
    }

    fn ensure_bridge_installed() {
        BRIDGE_INSTALLED.get_or_init(|| {
            let Some(window) = web_sys::window() else {
                return;
            };
            let cb = Closure::<dyn FnMut(String, String)>::new(move |kind: String, arg: String| {
                let ev = match kind.as_str() {
                    "submit" => RawDomEvent::Submit(arg),
                    "cancel" => RawDomEvent::Cancel,
                    "approve" => RawDomEvent::Approve(arg),
                    "reject" => RawDomEvent::Reject(arg),
                    "vitals" => RawDomEvent::ViewVitals,
                    "tracker" => RawDomEvent::OpenTracker(arg),
                    "logs" => RawDomEvent::TailLogs(arg),
                    "settings" => RawDomEvent::OpenSettings,
                    "setkey" => RawDomEvent::SetKey(arg),
                    _ => return,
                };
                queue().lock().unwrap_or_else(|e| e.into_inner()).push(ev);
            });
            let _ = js_sys::Reflect::set(
                &window,
                &wasm_bindgen::JsValue::from_str("__kaageDispatch"),
                cb.as_ref(),
            );
            cb.forget();
        });
    }

    fn ensure_pane_element() -> Option<web_sys::Element> {
        ensure_bridge_installed();
        let doc = web_sys::window()?.document()?;
        if let Some(el) = doc.get_element_by_id("kaage_copilot_pane") {
            return Some(el);
        }
        let root = doc.get_element_by_id("kaage_dom_overlay")?;
        let pane = doc.create_element("div").ok()?;
        pane.set_id("kaage_copilot_pane");
        pane.set_attribute("class", "kaage-copilot-pane").ok()?;
        pane.set_inner_html(
            r#"
            <div class="kc-header" id="kc_header"></div>
            <div class="kc-messages" id="kc_messages"></div>
            <div class="kc-composer">
              <textarea id="kc_input" rows="2" placeholder="Ask Kaage Copilot or type /provider, /model, /mode, /node, /status..."></textarea>
              <button class="kc-btn kc-btn-primary" id="kc_send" onclick="(function(){var i=document.getElementById('kc_input');if(i&&i.value.trim()){window.__kaageDispatch('submit',i.value);i.value='';}})()">Send</button>
              <button class="kc-btn" id="kc_stop" style="display:none" onclick="window.__kaageDispatch('cancel','')">Stop</button>
            </div>
            "#,
        );
        root.append_child(&pane).ok()?;

        // Wire Enter to submit (Shift+Enter inserts newline)
        if let Some(ta) = doc.get_element_by_id("kc_input") {
            let on_kd = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                move |e: web_sys::KeyboardEvent| {
                    if e.key() == "Enter" && !e.shift_key() {
                        e.prevent_default();
                        if let Some(d) = web_sys::window().and_then(|w| w.document())
                            && let Some(el) = d.get_element_by_id("kc_input")
                            && let Ok(val) = js_sys::Reflect::get(&el, &"value".into())
                            && let Some(txt) = val.as_string()
                            && !txt.trim().is_empty()
                        {
                            let _ = js_sys::Reflect::set(&el, &"value".into(), &"".into());
                            queue()
                                .lock()
                                .unwrap_or_else(|err| err.into_inner())
                                .push(RawDomEvent::Submit(txt));
                        }
                    }
                },
            );
            let _ = ta.add_event_listener_with_callback("keydown", on_kd.as_ref().unchecked_ref());
            on_kd.forget();
        }
        Some(pane)
    }

    pub fn sync_theme_css() {
        let p = palette();
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let Some(root) = doc.get_element_by_id("kaage_dom_overlay") else {
            return;
        };
        let css = format!(
            "--kaage-bg:{};--kaage-panel:{};--kaage-well:{};--kaage-alt:{};--kaage-accent:{};--kaage-border:{};--kaage-text:{};--kaage-muted:{};--kaage-green:{};--kaage-red:{};--kaage-sky:{};",
            hex_c(p.canvas_bg),
            hex_c(p.surface),
            hex_c(p.well),
            hex_c(p.surface_alt),
            hex_c(p.accent),
            hex_c(p.border),
            hex_c(p.text),
            hex_c(p.text_muted),
            hex_c(p.status_green),
            hex_c(p.status_red),
            hex_c(p.status_sky),
        );
        let _ = root.set_attribute("style", &css);
    }

    pub fn sync_pane_geometry(rect: Rect, visible: bool, dragging: bool, z_index: i32) {
        let Some(pane) = ensure_pane_element() else {
            return;
        };
        if !visible || rect.width() < 20.0 || rect.height() < 20.0 {
            let _ = pane.set_attribute("style", "display:none;");
            return;
        }
        let pe = if dragging { "none" } else { "auto" };
        let style = format!(
            "display:flex;transform:translate3d({:.1}px,{:.1}px,0);width:{:.1}px;height:{:.1}px;z-index:{z_index};pointer-events:{pe};",
            rect.min.x,
            rect.min.y,
            rect.width(),
            rect.height(),
        );
        let _ = pane.set_attribute("style", &style);
    }

    pub fn render_copilot_dom(win: &CopilotWindow, status: Option<&CopilotStatus>) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if let Some(hdr) = doc.get_element_by_id("kc_header") {
            let html = if let Some(st) = status {
                let dot = if st.ready { "var(--kaage-green)" } else { "var(--kaage-red)" };
                format!(
                    "<span class='kc-title'>✦ Kaage Copilot</span>\
                     <span class='kc-pill' style='border-color:{dot};color:{dot}'>{} · {}</span>\
                     <span class='kc-pill'>{}</span>\
                     <span class='kc-pill'>@{}</span>\
                     <button class='kc-btn' style='margin-left:auto' onclick=\"window.__kaageDispatch('settings','')\">⚙ Backends &amp; Keys</button>",
                    esc(st.provider.as_str()),
                    esc(&st.model),
                    esc(st.trust_mode.as_str()),
                    esc(&st.target_node),
                )
            } else {
                "<span class='kc-title'>✦ Kaage Copilot</span><span class='kc-pill'>connecting…</span>".into()
            };
            hdr.set_inner_html(&html);
        }

        if let Some(stop_btn) = doc.get_element_by_id("kc_stop") {
            let _ = stop_btn.set_attribute(
                "style",
                if win.active_request_id.is_some() {
                    "display:inline-flex;"
                } else {
                    "display:none;"
                },
            );
        }

        if let Some(box_el) = doc.get_element_by_id("kc_messages") {
            let mut html = String::new();
            if let Some(st) = status
                && !st.ready
            {
                let env_name = st.provider.env_var_name();
                let prov_name = st.provider.label();
                html.push_str(&format!(
                    "<div class='kc-card kc-confirm'>\
                      <div class='kc-card-hdr'><span>⚠ Missing {env_name} ({prov_name})</span>\
                      <button class='kc-btn' onclick=\"window.__kaageDispatch('settings','')\">All Backends</button></div>\
                      <div class='kc-sub'>Paste your key below (kept in daemon RAM) or open Backends &amp; Keys to switch to OpenAI, Claude, or Local Ollama:</div>\
                      <div class='kc-actions'>\
                        <input id='kc_quick_key' type='password' placeholder='Paste {env_name}...' style='flex:1;background:var(--kaage-bg);color:var(--kaage-text);border:1px solid var(--kaage-border);padding:4px 6px;font-family:monospace;font-size:11px;' />\
                        <button class='kc-btn kc-btn-primary' onclick=\"(function(){{var k=document.getElementById('kc_quick_key');if(k&&k.value.trim()){{window.__kaageDispatch('setkey',k.value.trim());k.value='';}}}})()\">Set Key</button>\
                      </div>\
                    </div>"
                ));
            }
            for item in &win.items {
                match item {
                    CopilotItem::User(txt) => {
                        html.push_str(&format!(
                            "<div class='kc-msg kc-user'><div class='kc-role'>YOU</div><div class='kc-body'>{}</div></div>",
                            esc(txt)
                        ));
                    }
                    CopilotItem::Assistant { text, done, .. } => {
                        let cursor = if *done { "" } else { "<span class='kc-cursor'>▌</span>" };
                        html.push_str(&format!(
                            "<div class='kc-msg kc-assistant'><div class='kc-role'>COPILOT</div><div class='kc-body'>{}{cursor}</div></div>",
                            esc(text)
                        ));
                    }
                    CopilotItem::ToolStatus {
                        tool_name,
                        target_node,
                        state,
                    } => {
                        html.push_str(&format!(
                            "<div class='kc-tool'>⚙ {} @{} · {}</div>",
                            esc(tool_name),
                            esc(target_node),
                            esc(state)
                        ));
                    }
                    CopilotItem::SystemNotice(msg) => {
                        html.push_str(&format!("<div class='kc-notice'>{}</div>", esc(msg)));
                    }
                    CopilotItem::Card(card) => {
                        html.push_str(&render_card_html(card));
                    }
                }
            }
            box_el.set_inner_html(&html);
            let _ = js_sys::Reflect::set(
                &box_el,
                &"scrollTop".into(),
                &js_sys::Reflect::get(&box_el, &"scrollHeight".into()).unwrap_or_default(),
            );
        }
    }

    pub fn drain_dom_actions(
        win: &mut CopilotWindow,
        status: Option<&CopilotStatus>,
    ) -> Vec<CopilotCrossAction> {
        let events: Vec<RawDomEvent> = {
            let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *q)
        };
        let mut out = Vec::new();
        for ev in events {
            match ev {
                RawDomEvent::Submit(prompt) => out.extend(win.handle_prompt(&prompt)),
                RawDomEvent::Cancel => {
                    if let Some(rid) = win.active_request_id {
                        out.push(CopilotCrossAction::SendClientMsg(ClientMsg::CancelCopilot {
                            request_id: rid,
                        }));
                    }
                }
                RawDomEvent::Approve(action_id) => {
                    out.push(CopilotCrossAction::SendClientMsg(
                        ClientMsg::CopilotConfirmResponse {
                            action_id,
                            approved: true,
                        },
                    ));
                }
                RawDomEvent::Reject(action_id) => {
                    out.push(CopilotCrossAction::SendClientMsg(
                        ClientMsg::CopilotConfirmResponse {
                            action_id,
                            approved: false,
                        },
                    ));
                }
                RawDomEvent::ViewVitals => out.push(CopilotCrossAction::ViewInVitals),
                RawDomEvent::OpenTracker(path) => {
                    out.push(CopilotCrossAction::OpenInTracker { path })
                }
                RawDomEvent::TailLogs(unit) => out.push(CopilotCrossAction::TailUnitLogs { unit }),
                RawDomEvent::OpenSettings => out.push(CopilotCrossAction::OpenCopilotSettings),
                RawDomEvent::SetKey(api_key) => {
                    let provider = status.map(|s| s.provider).unwrap_or(CopilotProvider::Gemini);
                    out.push(CopilotCrossAction::SendClientMsg(
                        ClientMsg::SetCopilotApiKey {
                            provider,
                            api_key,
                            persist: false,
                        },
                    ));
                }
            }
        }
        out
    }

    fn render_card_html(card: &CopilotCard) -> String {
        match card {
            CopilotCard::UnitCard {
                unit,
                active_state,
                sub_state,
                summary,
                error_lines,
            } => {
                let mut errs = String::new();
                for l in error_lines.iter().take(6) {
                    errs.push_str(&format!("<div class='kc-err'>{}</div>", esc(l)));
                }
                let unit_js = js_str(unit);
                format!(
                    "<div class='kc-card'><div class='kc-card-hdr'><span>● {} ({}/{}) · {}</span>\
                     <button class='kc-btn' onclick=\"window.__kaageDispatch('logs','{unit_js}')\">Tail Logs</button></div>{errs}</div>",
                    esc(unit),
                    esc(active_state),
                    esc(sub_state),
                    esc(summary),
                )
            }
            CopilotCard::VitalsSlice {
                metric_label,
                points,
                top_consumers,
                ..
            } => {
                let svg = build_sparkline_svg(points);
                let mut rows = String::new();
                for (k, v) in top_consumers {
                    rows.push_str(&format!(
                        "<div class='kc-kv'><span>{}</span><span>{}</span></div>",
                        esc(k),
                        esc(v)
                    ));
                }
                format!(
                    "<div class='kc-card'><div class='kc-card-hdr'><span>{}</span>\
                     <button class='kc-btn' onclick=\"window.__kaageDispatch('vitals','')\">View in Vitals</button></div>{svg}{rows}</div>",
                    esc(metric_label)
                )
            }
            CopilotCard::NixDiff {
                file_path,
                diff,
                summary,
                ..
            } => {
                let path_js = js_str(file_path);
                format!(
                    "<div class='kc-card'><div class='kc-card-hdr'><span>{}</span>\
                     <button class='kc-btn' onclick=\"window.__kaageDispatch('tracker','{path_js}')\">Open in Tracker</button></div>\
                     <div class='kc-sub'>{}</div><pre class='kc-pre'>{}</pre></div>",
                    esc(file_path),
                    esc(summary),
                    esc(diff)
                )
            }
            CopilotCard::ConfirmGate {
                action_id,
                target_node,
                summary,
                preview,
                rollback_cmd,
            } => {
                let aid = js_str(action_id);
                format!(
                    "<div class='kc-card kc-confirm'><div class='kc-card-hdr'><span>⚠ Confirm @{}: {}</span></div>\
                     <pre class='kc-pre'>{}</pre><div class='kc-sub'>Rollback: {}</div>\
                     <div class='kc-actions'>\
                       <button class='kc-btn kc-btn-primary' onclick=\"window.__kaageDispatch('approve','{aid}')\">Approve</button>\
                       <button class='kc-btn kc-btn-danger' onclick=\"window.__kaageDispatch('reject','{aid}')\">Reject</button>\
                     </div></div>",
                    esc(target_node),
                    esc(summary),
                    esc(preview),
                    esc(rollback_cmd)
                )
            }
            CopilotCard::MemoryNote {
                saved,
                category,
                summary,
            } => {
                let tag = if *saved { "Saved Memory" } else { "Recalled Memory" };
                format!(
                    "<div class='kc-memory'>[{} · {}] {}</div>",
                    tag,
                    esc(category),
                    esc(summary)
                )
            }
        }
    }

    fn build_sparkline_svg(points: &[f32]) -> String {
        if points.len() < 2 {
            return String::new();
        }
        let max_v = points.iter().copied().fold(10.0_f32, f32::max);
        let step = 280.0 / (points.len() - 1) as f32;
        let mut pts = String::new();
        for (i, &v) in points.iter().enumerate() {
            let x = i as f32 * step;
            let y = 34.0 - ((v / max_v).clamp(0.0, 1.0) * 30.0);
            pts.push_str(&format!("{x:.1},{y:.1} "));
        }
        format!(
            "<svg class='kc-spark' viewBox='0 0 280 36' preserveAspectRatio='none'>\
             <polyline fill='none' stroke='var(--kaage-accent)' stroke-width='1.5' points='{pts}'/></svg>"
        )
    }

    fn hex_c(c: egui::Color32) -> String {
        format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
    }

    fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn js_str(s: &str) -> String {
        s.replace('\\', "\\\\").replace('\'', "\\'")
    }
}
