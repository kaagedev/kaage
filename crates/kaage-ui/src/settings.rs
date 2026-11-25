use crate::theme::{
    FontPreset, ThemeMode, ThemePalette, apply_user_settings, be_button, be_toggle_button, palette,
    preset_palette, user_settings,
};
use egui::{Color32, CornerRadius, RichText, ScrollArea, Stroke, Ui};
use kaage_proto::{
    AppDataSnapshot, ClientMsg, CopilotOptimizations, CopilotProvider, CopilotStatus,
    HostMemoryItem, TrustMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Typography,
    ColorTheme,
    AppData,
    Copilot,
}

pub struct SettingsWindow {
    pub tab: SettingsTab,
    pub appdata_input: String,
    pub appdata_dirty: bool,
    pub new_app_name: String,
    pub copilot_model_input: String,
    pub copilot_fields_dirty: bool,
    pub local_name_input: String,
    pub local_url_input: String,
    pub local_key_input: String,
    pub local_ctx_window: u64,
    pub local_persist: bool,
    pub local_form_initialized: bool,
    pub local_testing: bool,
    pub local_test_result: Option<(bool, String)>,
    pub local_discovered_models: Vec<String>,
    pub gemini_key_input: String,
    pub openai_key_input: String,
    pub anthropic_key_input: String,
    pub persist_gemini_key: bool,
    pub persist_openai_key: bool,
    pub persist_anthropic_key: bool,
    pub memory_category: String,
    pub memory_input: String,
    pub memory_pin: bool,
    pub memory_search: String,
}

impl SettingsWindow {
    pub fn new() -> Self {
        Self {
            tab: SettingsTab::Typography,
            appdata_input: "~/appdata".into(),
            appdata_dirty: false,
            new_app_name: String::new(),
            copilot_model_input: String::new(),
            copilot_fields_dirty: false,
            local_name_input: String::new(),
            local_url_input: String::new(),
            local_key_input: String::new(),
            local_ctx_window: 32_768,
            local_persist: true,
            local_form_initialized: false,
            local_testing: false,
            local_test_result: None,
            local_discovered_models: Vec::new(),
            gemini_key_input: String::new(),
            openai_key_input: String::new(),
            anthropic_key_input: String::new(),
            persist_gemini_key: false,
            persist_openai_key: false,
            persist_anthropic_key: false,
            memory_category: "system".into(),
            memory_input: String::new(),
            memory_pin: false,
            memory_search: String::new(),
        }
    }

    pub fn on_local_llm_test_result(
        &mut self,
        ok: bool,
        message: String,
        models: Vec<String>,
        context_window: Option<u64>,
    ) {
        self.local_testing = false;
        self.local_test_result = Some((ok, message));
        if ok {
            self.local_discovered_models = models;
            if let Some(ctx) = context_window {
                self.local_ctx_window = ctx;
            }
        }
    }

    pub fn new_on_appdata() -> Self {
        let mut w = Self::new();
        w.tab = SettingsTab::AppData;
        w
    }

    pub fn new_on_copilot() -> Self {
        let mut w = Self::new();
        w.tab = SettingsTab::Copilot;
        w
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        appdata: Option<&AppDataSnapshot>,
        copilot_status: Option<&CopilotStatus>,
        memories: &[HostMemoryItem],
        send_msg: &mut dyn FnMut(ClientMsg),
    ) {
        let p = palette();

        if !self.appdata_dirty
            && let Some(snap) = appdata
        {
            self.appdata_input = snap.appdata_dir.clone();
        }
        if !self.copilot_fields_dirty
            && let Some(st) = copilot_status
        {
            self.copilot_model_input = st.model.clone();
        }
        if !self.local_form_initialized
            && let Some(st) = copilot_status
        {
            self.local_form_initialized = true;
            if self.local_url_input.is_empty() {
                self.local_url_input = st.local_endpoint.clone();
            }
            if self.local_name_input.is_empty() {
                self.local_name_input = st
                    .active_local_name
                    .clone()
                    .unwrap_or_else(|| "Ollama Local".into());
            }
            self.persist_gemini_key = st.gemini_key_persisted;
            self.persist_openai_key = st.openai_key_persisted;
            self.persist_anthropic_key = st.anthropic_key_persisted;
        }

        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("kaage-settings")
                    .strong()
                    .size(13.0)
                    .color(p.accent),
            );
            ui.separator();
            if be_toggle_button(
                ui,
                "Typography & Sizing",
                self.tab == SettingsTab::Typography,
            )
            .clicked()
            {
                self.tab = SettingsTab::Typography;
            }
            if be_toggle_button(
                ui,
                "Color Theme Editor",
                self.tab == SettingsTab::ColorTheme,
            )
            .clicked()
            {
                self.tab = SettingsTab::ColorTheme;
            }
            if be_toggle_button(ui, "App Data", self.tab == SettingsTab::AppData).clicked() {
                self.tab = SettingsTab::AppData;
                send_msg(ClientMsg::GetAppDataState);
            }
            if be_toggle_button(ui, "Copilot (AI)", self.tab == SettingsTab::Copilot).clicked() {
                self.tab = SettingsTab::Copilot;
                send_msg(ClientMsg::GetCopilotStatus);
                send_msg(ClientMsg::ListMemories { query: None });
            }
        });
        ui.separator();

        ScrollArea::vertical()
            .id_salt("kaage_settings_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                SettingsTab::Typography => render_typography_tab(ui),
                SettingsTab::ColorTheme => render_color_theme_tab(ui),
                SettingsTab::AppData => self.render_appdata_tab(ui, appdata, send_msg),
                SettingsTab::Copilot => {
                    self.render_copilot_tab(ui, copilot_status, memories, send_msg)
                }
            });
    }

    fn render_copilot_tab(
        &mut self,
        ui: &mut Ui,
        status: Option<&CopilotStatus>,
        memories: &[HostMemoryItem],
        send_msg: &mut dyn FnMut(ClientMsg),
    ) {
        let p = palette();
        let active_provider = status.map(|s| s.provider).unwrap_or(CopilotProvider::Gemini);
        let active_trust = status.map(|s| s.trust_mode).unwrap_or(TrustMode::Managed);

        section_title(ui, "ACTIVE LLM BACKEND");
        ui.horizontal_wrapped(|ui| {
            for prov in [
                CopilotProvider::Gemini,
                CopilotProvider::OpenAi,
                CopilotProvider::Claude,
                CopilotProvider::Local,
            ] {
                if be_toggle_button(ui, prov.label(), active_provider == prov).clicked() {
                    self.copilot_fields_dirty = false;
                    send_msg(ClientMsg::SetCopilotConfig {
                        provider: Some(prov),
                        model: None,
                        trust_mode: None,
                        target_node: None,
                        local_endpoint: None,
                    });
                }
            }
        });

        ui.add_space(6.0);
        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Model ID:").color(p.text));
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.copilot_model_input)
                                .desired_width(220.0),
                        )
                        .changed()
                    {
                        self.copilot_fields_dirty = true;
                    }
                    if be_button(ui, "Apply Model").clicked() {
                        self.copilot_fields_dirty = false;
                        send_msg(ClientMsg::SetCopilotConfig {
                            provider: None,
                            model: Some(self.copilot_model_input.clone()),
                            trust_mode: None,
                            target_node: None,
                            local_endpoint: None,
                        });
                    }
                });

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Trust Mode:").color(p.text));
                    if be_toggle_button(
                        ui,
                        "Managed (Confirm Mutations)",
                        active_trust == TrustMode::Managed,
                    )
                    .clicked()
                    {
                        send_msg(ClientMsg::SetCopilotConfig {
                            provider: None,
                            model: None,
                            trust_mode: Some(TrustMode::Managed),
                            target_node: None,
                            local_endpoint: None,
                        });
                    }
                    if be_toggle_button(
                        ui,
                        "Monitored (Read-Only)",
                        active_trust == TrustMode::Monitored,
                    )
                    .clicked()
                    {
                        send_msg(ClientMsg::SetCopilotConfig {
                            provider: None,
                            model: None,
                            trust_mode: Some(TrustMode::Monitored),
                            target_node: None,
                            local_endpoint: None,
                        });
                    }
                });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                let default_provider = status.and_then(|s| s.default_provider);
                let default_model = status.and_then(|s| s.default_model.as_deref());
                let default_local_name = status.and_then(|s| s.default_local_name.as_deref());
                let default_local_ep = status.and_then(|s| s.default_local_endpoint.as_deref());
                let current_is_default = status.is_some_and(|st| {
                    st.default_provider == Some(st.provider)
                        && st.default_model.as_deref() == Some(st.model.as_str())
                        && (st.provider != CopilotProvider::Local
                            || (st.default_local_endpoint.as_deref()
                                == Some(st.local_endpoint.as_str())
                                && st.default_local_name == st.active_local_name))
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("Default Backend (Start & Resume):")
                            .strong()
                            .color(p.text),
                    );
                    if let Some(dp) = default_provider {
                        let desc = if dp == CopilotProvider::Local {
                            let ep_label = default_local_name
                                .or(default_local_ep)
                                .unwrap_or("Local");
                            format!(
                                "★ {} · {} · {}",
                                dp.label(),
                                ep_label,
                                default_model.unwrap_or(dp.default_model())
                            )
                        } else {
                            format!(
                                "★ {} · {}",
                                dp.label(),
                                default_model.unwrap_or(dp.default_model())
                            )
                        };
                        ui.label(
                            RichText::new(desc)
                                .monospace()
                                .size(10.5)
                                .color(p.status_green),
                        );
                    } else {
                        ui.label(
                            RichText::new("Auto (remembers last LLM used for chatting)")
                                .monospace()
                                .size(10.5)
                                .color(p.text_muted),
                        );
                    }
                });
                ui.add_space(3.0);
                ui.horizontal_wrapped(|ui| {
                    let set_label = if current_is_default {
                        "★ Current is Default"
                    } else {
                        "Set Current as Default"
                    };
                    if be_toggle_button(ui, set_label, current_is_default).clicked()
                        && let Some(st) = status
                    {
                        let model = if !self.copilot_model_input.trim().is_empty() {
                            self.copilot_model_input.trim().to_string()
                        } else {
                            st.model.clone()
                        };
                        send_msg(ClientMsg::SetCopilotDefaultBackend {
                            provider: Some(st.provider),
                            model: Some(model),
                            local_endpoint: if st.provider == CopilotProvider::Local {
                                Some(st.local_endpoint.clone())
                            } else {
                                None
                            },
                            local_name: if st.provider == CopilotProvider::Local {
                                st.active_local_name.clone()
                            } else {
                                None
                            },
                        });
                    }
                    if default_provider.is_some()
                        && be_button(ui, "Use Last Used Instead").clicked()
                    {
                        send_msg(ClientMsg::SetCopilotDefaultBackend {
                            provider: None,
                            model: None,
                            local_endpoint: None,
                            local_name: None,
                        });
                    }
                });
            });

        ui.add_space(10.0);
        section_title(ui, "LOCAL LLM CONNECTIONS (OLLAMA / VLLM / UNSLOTH / LM STUDIO)");
        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                egui::Grid::new("local_llm_form_grid")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Name:").strong().color(p.text));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.local_name_input)
                                .hint_text("e.g. Ollama Local, Unsloth RTX 5090")
                                .desired_width(280.0),
                        );
                        ui.end_row();

                        ui.label(RichText::new("URL:").strong().color(p.text));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.local_url_input)
                                .hint_text("http://127.0.0.1:11434/v1")
                                .desired_width(340.0),
                        );
                        ui.end_row();

                        ui.label(RichText::new("API Key:").strong().color(p.text));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.local_key_input)
                                .password(true)
                                .hint_text("Optional (leave blank if no auth required)")
                                .desired_width(340.0),
                        );
                        ui.end_row();

                        ui.label(RichText::new("Context:").strong().color(p.text));
                        ui.horizontal_wrapped(|ui| {
                            for (label, val) in [
                                ("16K", 16_384u64),
                                ("32K", 32_768),
                                ("64K", 65_536),
                                ("128K", 131_072),
                            ] {
                                if be_toggle_button(ui, label, self.local_ctx_window == val).clicked()
                                {
                                    self.local_ctx_window = val;
                                }
                            }
                            ui.label(
                                RichText::new(format!("({} tok)", self.local_ctx_window))
                                    .monospace()
                                    .size(10.0)
                                    .color(p.text_muted),
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.local_persist,
                    RichText::new("Save local connection & auth key to disk ($KAAGE_APPDATA/kaage/copilot_local_llms.json, 0600)")
                        .size(11.0)
                        .color(p.text_muted),
                );

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    let test_label = if self.local_testing {
                        "Testing..."
                    } else {
                        "Test Connection"
                    };
                    if be_button(ui, test_label).clicked()
                        && !self.local_testing
                        && !self.local_url_input.trim().is_empty()
                    {
                        self.local_testing = true;
                        self.local_test_result = None;
                        let key = if self.local_key_input.trim().is_empty() {
                            None
                        } else {
                            Some(self.local_key_input.trim().to_string())
                        };
                        send_msg(ClientMsg::TestLocalLlm {
                            url: self.local_url_input.trim().to_string(),
                            api_key: key,
                        });
                    }
                    if be_button(ui, "Save & Use").clicked()
                        && !self.local_name_input.trim().is_empty()
                        && !self.local_url_input.trim().is_empty()
                    {
                        let key = if self.local_key_input.trim().is_empty() {
                            None
                        } else {
                            Some(self.local_key_input.trim().to_string())
                        };
                        send_msg(ClientMsg::UpsertLocalLlm {
                            name: self.local_name_input.trim().to_string(),
                            url: self.local_url_input.trim().to_string(),
                            api_key: key,
                            context_window: Some(self.local_ctx_window),
                            persist: self.local_persist,
                        });
                        self.local_key_input.clear();
                    }
                    if be_button(ui, "+ New Connection").clicked() {
                        self.local_name_input.clear();
                        self.local_url_input.clear();
                        self.local_key_input.clear();
                        self.local_test_result = None;
                        self.local_discovered_models.clear();
                    }
                });

                if self.local_testing {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("● Probing endpoint & fetching available models...")
                            .monospace()
                            .size(10.5)
                            .color(p.accent),
                    );
                } else if let Some((ok, ref msg)) = self.local_test_result {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("● {msg}"))
                            .monospace()
                            .size(10.5)
                            .color(if ok { p.status_green } else { p.status_red }),
                    );
                }

                if !self.local_discovered_models.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Available Models (click to select):")
                            .size(11.0)
                            .color(p.text_muted),
                    );
                    ui.add_space(3.0);
                    let current_model = status.map(|s| s.model.as_str()).unwrap_or("");
                    let models_clone = self.local_discovered_models.clone();
                    let local_persist = self.local_persist;
                    ui.horizontal_wrapped(|ui| {
                        for model_id in models_clone {
                            let is_active =
                                active_provider == CopilotProvider::Local && current_model == model_id;
                            if be_toggle_button(ui, &model_id, is_active).clicked() {
                                self.copilot_model_input = model_id.clone();
                                self.copilot_fields_dirty = false;
                                if !self.local_key_input.trim().is_empty() {
                                    if !self.local_name_input.trim().is_empty()
                                        && !self.local_url_input.trim().is_empty()
                                    {
                                        send_msg(ClientMsg::UpsertLocalLlm {
                                            name: self.local_name_input.trim().to_string(),
                                            url: self.local_url_input.trim().to_string(),
                                            api_key: Some(self.local_key_input.trim().to_string()),
                                            context_window: Some(self.local_ctx_window),
                                            persist: local_persist,
                                        });
                                    } else {
                                        send_msg(ClientMsg::SetCopilotApiKey {
                                            provider: CopilotProvider::Local,
                                            api_key: self.local_key_input.trim().to_string(),
                                            persist: local_persist,
                                        });
                                    }
                                }
                                send_msg(ClientMsg::SetCopilotConfig {
                                    provider: Some(CopilotProvider::Local),
                                    model: Some(model_id),
                                    trust_mode: None,
                                    target_node: None,
                                    local_endpoint: Some(self.local_url_input.trim().to_string()),
                                });
                            }
                        }
                    });
                }

                if let Some(st) = status
                    && !st.local_endpoints.is_empty()
                {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "Saved Local Connections ({}):",
                            st.local_endpoints.len()
                        ))
                        .size(11.0)
                        .strong()
                        .color(p.text_muted),
                    );
                    ui.add_space(3.0);
                    for ep in &st.local_endpoints {
                        let is_active = active_provider == CopilotProvider::Local
                            && st.active_local_name.as_deref() == Some(ep.name.as_str());
                        let is_default = st.default_provider == Some(CopilotProvider::Local)
                            && st.default_local_name.as_deref() == Some(ep.name.as_str());
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(&ep.name)
                                    .monospace()
                                    .strong()
                                    .color(if is_active { p.accent } else { p.text }),
                            );
                            if is_default {
                                ui.label(
                                    RichText::new("★ Default")
                                        .monospace()
                                        .size(10.0)
                                        .color(p.status_green),
                                );
                            }
                            ui.label(
                                RichText::new(&ep.url)
                                    .monospace()
                                    .size(10.5)
                                    .color(p.text_muted),
                            );
                            ui.label(
                                RichText::new(format!("{}K ctx", ep.context_window / 1024))
                                    .monospace()
                                    .size(10.0)
                                    .color(p.status_sky),
                            );
                            if ep.has_key {
                                ui.label(
                                    RichText::new("● Key set")
                                        .monospace()
                                        .size(10.0)
                                        .color(p.status_green),
                                );
                            }
                            if be_toggle_button(ui, if is_active { "Active" } else { "Use" }, is_active)
                                .clicked()
                            {
                                self.local_name_input = ep.name.clone();
                                self.local_url_input = ep.url.clone();
                                self.local_ctx_window = ep.context_window;
                                self.local_key_input.clear();
                                send_msg(ClientMsg::SelectLocalLlm {
                                    name: ep.name.clone(),
                                });
                            }
                            if be_button(ui, "Edit").clicked() {
                                self.local_name_input = ep.name.clone();
                                self.local_url_input = ep.url.clone();
                                self.local_ctx_window = ep.context_window;
                                self.local_key_input.clear();
                            }
                            if be_button(ui, "Test").clicked() && !self.local_testing {
                                self.local_name_input = ep.name.clone();
                                self.local_url_input = ep.url.clone();
                                self.local_testing = true;
                                self.local_test_result = None;
                                send_msg(ClientMsg::TestLocalLlm {
                                    url: ep.url.clone(),
                                    api_key: None,
                                });
                            }
                            if be_button(ui, "Remove").clicked() {
                                send_msg(ClientMsg::DeleteLocalLlm {
                                    name: ep.name.clone(),
                                });
                            }
                        });
                    }
                }
            });

        ui.add_space(10.0);
        section_title(ui, "CLOUD PROVIDER API KEYS");
        ui.label(
            RichText::new(
                "Choose per provider whether to persist its key to disk (0600) or keep it in daemon RAM only. Forget removes the key from both RAM and disk.",
            )
            .size(11.0)
            .color(p.text_muted),
        );
        ui.add_space(4.0);

        let has_gemini = status.map(|s| s.has_gemini_key).unwrap_or(false);
        let has_openai = status.map(|s| s.has_openai_key).unwrap_or(false);
        let has_claude = status.map(|s| s.has_anthropic_key).unwrap_or(false);
        let disk_gemini = status.map(|s| s.gemini_key_persisted).unwrap_or(false);
        let disk_openai = status.map(|s| s.openai_key_persisted).unwrap_or(false);
        let disk_claude = status.map(|s| s.anthropic_key_persisted).unwrap_or(false);

        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                render_key_row(
                    ui,
                    "Google Gemini",
                    "GEMINI_API_KEY",
                    CopilotProvider::Gemini,
                    has_gemini,
                    disk_gemini,
                    &mut self.gemini_key_input,
                    &mut self.persist_gemini_key,
                    send_msg,
                );
                ui.separator();
                render_key_row(
                    ui,
                    "OpenAI",
                    "OPENAI_API_KEY",
                    CopilotProvider::OpenAi,
                    has_openai,
                    disk_openai,
                    &mut self.openai_key_input,
                    &mut self.persist_openai_key,
                    send_msg,
                );
                ui.separator();
                render_key_row(
                    ui,
                    "Anthropic Claude",
                    "ANTHROPIC_API_KEY",
                    CopilotProvider::Claude,
                    has_claude,
                    disk_claude,
                    &mut self.anthropic_key_input,
                    &mut self.persist_anthropic_key,
                    send_msg,
                );
            });

        ui.add_space(10.0);
        section_title(ui, "COPILOT OPTIMIZATIONS");
        let mut opts = status.map(|s| s.optimizations).unwrap_or_default();
        let mut opts_changed = false;
        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                if ui
                    .checkbox(
                        &mut opts.auto_detect_context,
                        RichText::new("Auto-detect local context window on Test Connection")
                            .color(p.text),
                    )
                    .changed()
                {
                    opts_changed = true;
                }
                if ui
                    .checkbox(
                        &mut opts.collapse_think_blocks,
                        RichText::new("Render <think>...</think> reasoning as collapsible sections")
                            .color(p.text),
                    )
                    .changed()
                {
                    opts_changed = true;
                }
                if ui
                    .checkbox(
                        &mut opts.adaptive_tool_truncation,
                        RichText::new(
                            "Adaptive tool output truncation (halve max tool output for <= 32K local models)",
                        )
                        .color(p.text),
                    )
                    .changed()
                {
                    opts_changed = true;
                }

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Max Tool Output:").color(p.text));
                    for (label, val) in [
                        ("4K chars", 4_000u32),
                        ("6K chars", 6_000),
                        ("12K chars", 12_000),
                        ("24K chars", 24_000),
                    ] {
                        if be_toggle_button(ui, label, opts.tool_output_max_chars == val).clicked() {
                            opts.tool_output_max_chars = val;
                            opts_changed = true;
                        }
                    }
                });

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Local Max Output Tokens:").color(p.text));
                    for (label, val) in [
                        ("Auto (ctx/8)", 0u64),
                        ("2K", 2_048),
                        ("4K", 4_096),
                        ("8K", 8_192),
                    ] {
                        if be_toggle_button(ui, label, opts.local_max_tokens == val).clicked() {
                            opts.local_max_tokens = val;
                            opts_changed = true;
                        }
                    }
                });

                ui.add_space(4.0);
                if be_button(ui, "Reset Defaults").clicked() {
                    opts = CopilotOptimizations::default();
                    opts_changed = true;
                }
            });
        if opts_changed {
            send_msg(ClientMsg::SetCopilotOptimizations(opts));
        }

        ui.add_space(10.0);
        section_title(ui, "HOST MEMORIES (SQLITE FTS5)");
        ui.label(
            RichText::new(
                "Pinned memories are always injected into every Copilot prompt; other memories are recalled automatically via FTS5 search. You can also use `/remember <fact>`, `/pin <fact>`, and `/memories` in Copilot chat.",
            )
            .size(11.0)
            .color(p.text_muted),
        );
        ui.add_space(4.0);
        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Category:").color(p.text));
                    for cat in ["system", "service", "preference", "incident"] {
                        if be_toggle_button(ui, cat, self.memory_category == cat).clicked() {
                            self.memory_category = cat.to_string();
                        }
                    }
                    ui.checkbox(
                        &mut self.memory_pin,
                        RichText::new("Pin (always active)").size(11.0).color(p.accent),
                    );
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.memory_input)
                            .hint_text("e.g. ComfyUI listens on port 8188 using RTX 5090")
                            .desired_width(360.0),
                    );
                    let enter =
                        resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (be_button(ui, "+ Remember").clicked() || enter)
                        && !self.memory_input.trim().is_empty()
                    {
                        send_msg(ClientMsg::SaveMemory {
                            category: self.memory_category.clone(),
                            content: self.memory_input.trim().to_string(),
                            pinned: self.memory_pin,
                        });
                        self.memory_input.clear();
                    }
                });

                ui.add_space(6.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("Saved ({}):", memories.len()))
                            .strong()
                            .size(11.0)
                            .color(p.text_muted),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.memory_search)
                            .hint_text("Filter memories...")
                            .desired_width(200.0),
                    );
                    if be_button(ui, "Refresh").clicked() {
                        send_msg(ClientMsg::ListMemories { query: None });
                    }
                });
                ui.add_space(4.0);

                let needle = self.memory_search.trim().to_ascii_lowercase();
                let mut shown = 0usize;
                for item in memories {
                    if !needle.is_empty()
                        && !item.content.to_ascii_lowercase().contains(&needle)
                        && !item.category.to_ascii_lowercase().contains(&needle)
                    {
                        continue;
                    }
                    shown += 1;
                    ui.horizontal_wrapped(|ui| {
                        if item.pinned {
                            ui.label(
                                RichText::new("📌")
                                    .size(11.0)
                                    .color(p.accent),
                            );
                        }
                        ui.label(
                            RichText::new(format!("[{}]", item.category))
                                .monospace()
                                .size(10.5)
                                .color(p.status_sky),
                        );
                        ui.label(
                            RichText::new(&item.content)
                                .size(11.5)
                                .color(p.text),
                        );
                        if be_toggle_button(
                            ui,
                            if item.pinned { "Pinned" } else { "Pin" },
                            item.pinned,
                        )
                        .clicked()
                        {
                            send_msg(ClientMsg::TogglePinMemory {
                                id: item.id,
                                pinned: !item.pinned,
                            });
                        }
                        if be_button(ui, "Delete").clicked() {
                            send_msg(ClientMsg::DeleteMemory { id: item.id });
                        }
                    });
                    ui.separator();
                }
                if shown == 0 {
                    ui.label(
                        RichText::new("No memories saved yet.")
                            .italics()
                            .size(11.0)
                            .color(p.text_muted),
                    );
                }
            });
    }

    fn render_appdata_tab(
        &mut self,
        ui: &mut Ui,
        appdata: Option<&AppDataSnapshot>,
        send_msg: &mut dyn FnMut(ClientMsg),
    ) {
        let p = palette();

        section_title(ui, "KAAGE_APPDATA BASE DIRECTORY");
        ui.label(
            RichText::new(
                "Apps running on kaage store their data under $KAAGE_APPDATA/<app_name>/ (default: ~/appdata/). \
                 New kaage terminals automatically export KAAGE_APPDATA.",
            )
            .size(11.5)
            .color(p.text_muted),
        );
        ui.add_space(6.0);

        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("KAAGE_APPDATA")
                            .monospace()
                            .strong()
                            .color(p.accent),
                    );
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.appdata_input)
                            .hint_text("~/appdata")
                            .desired_width(280.0),
                    );
                    if edit.changed() {
                        self.appdata_dirty = true;
                    }
                    if be_button(ui, "Apply").clicked() {
                        send_msg(ClientMsg::SetAppDataDir {
                            path: self.appdata_input.clone(),
                        });
                        self.appdata_dirty = false;
                    }
                    if be_button(ui, "Reset ~/appdata").clicked() {
                        self.appdata_input = "~/appdata".into();
                        self.appdata_dirty = false;
                        send_msg(ClientMsg::SetAppDataDir {
                            path: "~/appdata".into(),
                        });
                    }
                    if be_button(ui, "Copy").clicked() {
                        ui.ctx().copy_text(self.appdata_input.clone());
                    }
                    if be_button(ui, "Refresh").clicked() {
                        self.appdata_dirty = false;
                        send_msg(ClientMsg::GetAppDataState);
                    }
                });

                if let Some(snap) = appdata {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(if snap.exists {
                                "● Directory ready"
                            } else {
                                "● Not created yet"
                            })
                            .monospace()
                            .size(10.5)
                            .color(if snap.exists {
                                p.status_green
                            } else {
                                p.status_amber
                            }),
                        );
                        ui.label(
                            RichText::new(if snap.from_env {
                                "• source: KAAGE_APPDATA env"
                            } else {
                                "• source: default / saved config"
                            })
                            .monospace()
                            .size(10.5)
                            .color(p.text_muted),
                        );
                    });
                }
            });

        ui.add_space(10.0);
        section_title(ui, "APP DIRECTORIES");
        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("New App Dir:").color(p.text));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.new_app_name)
                            .hint_text("e.g. notes, comfyui, postgres")
                            .desired_width(200.0),
                    );
                    let enter =
                        resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (be_button(ui, "+ Create").clicked() || enter)
                        && !self.new_app_name.trim().is_empty()
                    {
                        send_msg(ClientMsg::CreateAppDataDir {
                            app_name: self.new_app_name.trim().to_string(),
                        });
                        self.new_app_name.clear();
                    }
                });
                ui.separator();

                let Some(snap) = appdata else {
                    ui.label(
                        RichText::new("Loading KAAGE_APPDATA state...")
                            .italics()
                            .color(p.text_muted),
                    );
                    return;
                };

                if snap.apps.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "No app directories inside {} yet. Create one above.",
                            snap.appdata_dir
                        ))
                        .italics()
                        .color(p.text_muted),
                    );
                } else {
                    for app in &snap.apps {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&app.app_name)
                                    .monospace()
                                    .strong()
                                    .color(p.accent),
                            );
                            ui.label(
                                RichText::new(&app.path)
                                    .monospace()
                                    .size(10.5)
                                    .color(p.text_muted),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if be_button(ui, "Copy Path").clicked() {
                                        ui.ctx().copy_text(app.path.clone());
                                    }
                                    ui.label(
                                        RichText::new(format!(
                                            "{} files · {}",
                                            app.file_count,
                                            fmt_appdata_bytes(app.size_bytes)
                                        ))
                                        .monospace()
                                        .size(10.5)
                                        .color(p.text),
                                    );
                                },
                            );
                        });
                        ui.separator();
                    }
                }
            });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_key_row(
    ui: &mut Ui,
    provider_label: &str,
    env_name: &str,
    provider: CopilotProvider,
    configured: bool,
    persisted_on_disk: bool,
    buf: &mut String,
    persist: &mut bool,
    send_msg: &mut dyn FnMut(ClientMsg),
) {
    let p = palette();
    let (badge_text, badge_color) = if persisted_on_disk {
        ("● Saved on disk", p.status_green)
    } else if configured {
        ("● In RAM / env", p.accent)
    } else {
        ("● Missing", p.status_red)
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(provider_label).strong().color(p.text));
        ui.label(RichText::new(env_name).monospace().size(10.5).color(p.text_muted));
        ui.label(
            RichText::new(badge_text)
                .monospace()
                .size(10.5)
                .color(badge_color),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(buf)
                .password(true)
                .hint_text("Paste API key...")
                .desired_width(240.0),
        );
        ui.checkbox(
            persist,
            RichText::new("Save to disk").size(11.0).color(p.text_muted),
        );
        if be_button(ui, "Set Key").clicked() && !buf.trim().is_empty() {
            send_msg(ClientMsg::SetCopilotApiKey {
                provider,
                api_key: buf.trim().to_string(),
                persist: *persist,
            });
            buf.clear();
        }
        if (configured || persisted_on_disk) && be_button(ui, "Forget Key").clicked() {
            *persist = false;
            send_msg(ClientMsg::SetCopilotApiKey {
                provider,
                api_key: String::new(),
                persist: false,
            });
        }
    });
}

fn fmt_appdata_bytes(b: u64) -> String {
    if b >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", b as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024 * 1024 {
        format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0))
    } else if b >= 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{b} B")
    }
}

fn render_typography_tab(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let mut s = user_settings();
    let mut changed = false;
    let mut reload_fonts = false;
    let p = s.palette;

    section_title(ui, "FONT FAMILY");
    ui.label(
        RichText::new(
            "IBM Plex Sans is used for application UI typography and IBM Plex Mono for terminals, code, and tabular metrics.",
        )
        .size(11.5)
        .color(p.text_muted),
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for preset in [
            FontPreset::IbmPlex,
            FontPreset::IbmPlexMonoAll,
            FontPreset::EguiDefault,
        ] {
            if be_toggle_button(ui, preset.label(), s.font_preset == preset).clicked() {
                s.font_preset = preset;
                changed = true;
                reload_fonts = true;
            }
        }
    });

    ui.add_space(10.0);
    section_title(ui, "FONT SIZE & INTERFACE SCALE");
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());

            ui.horizontal(|ui| {
                ui.label(RichText::new("UI Font Size (pt):").color(p.text));
                if ui
                    .add(egui::Slider::new(&mut s.base_font_size, 10.0..=18.0).step_by(0.5))
                    .changed()
                {
                    changed = true;
                }
                if be_button(ui, "13").clicked() {
                    s.base_font_size = 13.0;
                    changed = true;
                }
                if be_button(ui, "15").clicked() {
                    s.base_font_size = 15.0;
                    changed = true;
                }
                if be_button(ui, "17").clicked() {
                    s.base_font_size = 17.0;
                    changed = true;
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Terminal Font Size (pt):").color(p.text));
                if ui
                    .add(egui::Slider::new(&mut s.terminal_font_size, 10.0..=22.0).step_by(0.5))
                    .changed()
                {
                    changed = true;
                }
                if be_button(ui, "14").clicked() {
                    s.terminal_font_size = 14.0;
                    changed = true;
                }
                if be_button(ui, "16").clicked() {
                    s.terminal_font_size = 16.0;
                    changed = true;
                }
                if be_button(ui, "18").clicked() {
                    s.terminal_font_size = 18.0;
                    changed = true;
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Global UI Zoom:").color(p.text));
                if ui
                    .add(
                        egui::Slider::new(&mut s.ui_scale, 0.85..=1.40)
                            .step_by(0.05)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    )
                    .changed()
                {
                    changed = true;
                }
                if be_button(ui, "100%").clicked() {
                    s.ui_scale = 1.0;
                    changed = true;
                }
            });
        });

    ui.add_space(10.0);
    section_title(ui, "LIVE TYPOGRAPHY PREVIEW");
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(
                RichText::new("Application Heading — IBM Plex Sans")
                    .strong()
                    .size(s.base_font_size + 3.0)
                    .color(p.accent),
            );
            ui.label(
                RichText::new(
                    "Operational cockpit telemetry, service inspectors, and file previews use clean proportional sans-serif typography.",
                )
                .size(s.base_font_size)
                .color(p.text),
            );
            ui.add_space(4.0);
            egui::Frame::NONE
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius::ZERO)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        RichText::new(
                            "$ systemctl --user status kaaged.service\n● kaaged.service - Kaage Mission-Control Daemon (active: running, PID 4821)",
                        )
                        .monospace()
                        .size(s.terminal_font_size)
                        .color(p.status_green),
                    );
                });
        });

    if changed {
        apply_user_settings(&ctx, s, reload_fonts);
    }
}

fn render_color_theme_tab(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let mut s = user_settings();
    let mut changed = false;

    section_title(ui, "THEME PRESETS");
    ui.horizontal_wrapped(|ui| {
        if be_button(ui, "Obsidian & Amber").clicked() {
            s.theme_mode = ThemeMode::ObsidianAmber;
            s.palette = preset_palette(ThemeMode::ObsidianAmber);
            changed = true;
        }
        if be_button(ui, "Midnight Slate").clicked() {
            s.theme_mode = ThemeMode::ObsidianAmber;
            s.palette = midnight_slate_palette();
            changed = true;
        }
        if be_button(ui, "Gruvbox Dark").clicked() {
            s.theme_mode = ThemeMode::ObsidianAmber;
            s.palette = gruvbox_dark_palette();
            changed = true;
        }
        if be_button(ui, "Retro BeOS (Light)").clicked() {
            s.theme_mode = ThemeMode::RetroBeos;
            s.palette = preset_palette(ThemeMode::RetroBeos);
            changed = true;
        }
        if be_button(ui, "Reset Current").clicked() {
            s.palette = preset_palette(s.theme_mode);
            changed = true;
        }
    });

    ui.add_space(8.0);
    section_title(ui, "LIVE COLOR THEME EDITOR");
    let pal_preview = s.palette;
    egui::Frame::NONE
        .fill(pal_preview.well)
        .stroke(Stroke::new(1.0, pal_preview.border))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.columns(2, |cols| {
                cols[0].label(
                    RichText::new("SURFACES & CHROME")
                        .monospace()
                        .strong()
                        .size(10.0)
                        .color(pal_preview.accent),
                );
                changed |= color_row(&mut cols[0], "Canvas BG", &mut s.palette.canvas_bg);
                changed |= color_row(&mut cols[0], "Panel Surface", &mut s.palette.surface);
                changed |= color_row(&mut cols[0], "Alt Surface", &mut s.palette.surface_alt);
                changed |= color_row(&mut cols[0], "Recessed Well", &mut s.palette.well);
                changed |= color_row(&mut cols[0], "Widget Face", &mut s.palette.widget);
                changed |= color_row(&mut cols[0], "Widget Hover", &mut s.palette.widget_hover);
                changed |= color_row(&mut cols[0], "Structural Border", &mut s.palette.border);
                changed |= color_row(&mut cols[0], "Section Header", &mut s.palette.section_bg);
                changed |= color_row(&mut cols[0], "Row Hover", &mut s.palette.hover_row);

                cols[1].label(
                    RichText::new("ACCENTS, TABS & TEXT")
                        .monospace()
                        .strong()
                        .size(10.0)
                        .color(pal_preview.accent),
                );
                changed |= color_row(&mut cols[1], "Primary Text", &mut s.palette.text);
                changed |= color_row(&mut cols[1], "Muted Text", &mut s.palette.text_muted);
                changed |= color_row(&mut cols[1], "Accent", &mut s.palette.accent);
                changed |= color_row(&mut cols[1], "Active Tab BG", &mut s.palette.tab_active_bg);
                changed |= color_row(&mut cols[1], "Active Tab Text", &mut s.palette.tab_active_fg);
                changed |= color_row(&mut cols[1], "Status Green", &mut s.palette.status_green);
                changed |= color_row(&mut cols[1], "Status Red", &mut s.palette.status_red);
                changed |= color_row(&mut cols[1], "Status Amber", &mut s.palette.status_amber);
                changed |= color_row(&mut cols[1], "Status Sky", &mut s.palette.status_sky);
            });
        });

    ui.add_space(8.0);
    section_title(ui, "CONTRAST CHECK PREVIEW");
    egui::Frame::NONE
        .fill(s.palette.surface)
        .stroke(Stroke::new(1.0, s.palette.border))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Primary Text").strong().color(s.palette.text));
                ui.label(RichText::new("Muted Label").color(s.palette.text_muted));
                ui.label(RichText::new("Accent Highlight").strong().color(s.palette.accent));
                ui.label(RichText::new("● active").monospace().color(s.palette.status_green));
                ui.label(RichText::new("● failed").monospace().color(s.palette.status_red));
                ui.label(RichText::new("● tailnet").monospace().color(s.palette.status_sky));
            });
        });

    if changed {
        apply_user_settings(&ctx, s, false);
    }
}

fn color_row(ui: &mut Ui, label: &str, color: &mut Color32) -> bool {
    let p = palette();
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.color_edit_button_srgba(color).changed() {
            changed = true;
        }
        ui.label(RichText::new(label).size(11.5).color(p.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b()))
                    .monospace()
                    .size(10.0)
                    .color(p.text_muted),
            );
        });
    });
    changed
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

fn midnight_slate_palette() -> ThemePalette {
    let mut p = preset_palette(ThemeMode::ObsidianAmber);
    p.canvas_bg = Color32::from_rgb(10, 14, 23);
    p.surface = Color32::from_rgb(17, 24, 39);
    p.surface_alt = Color32::from_rgb(26, 36, 56);
    p.well = Color32::from_rgb(12, 17, 29);
    p.widget = Color32::from_rgb(28, 39, 60);
    p.widget_hover = Color32::from_rgb(38, 53, 80);
    p.tab_active_bg = Color32::from_rgb(24, 42, 68);
    p.tab_active_fg = Color32::from_rgb(56, 189, 248);
    p.accent = Color32::from_rgb(56, 189, 248);
    p.border = Color32::from_rgb(56, 74, 102);
    p.section_bg = Color32::from_rgb(22, 31, 49);
    p.hover_row = Color32::from_rgb(33, 46, 72);
    p
}

fn gruvbox_dark_palette() -> ThemePalette {
    let mut p = preset_palette(ThemeMode::ObsidianAmber);
    p.canvas_bg = Color32::from_rgb(29, 32, 33);
    p.surface = Color32::from_rgb(40, 40, 40);
    p.surface_alt = Color32::from_rgb(50, 48, 47);
    p.well = Color32::from_rgb(29, 32, 33);
    p.widget = Color32::from_rgb(60, 56, 54);
    p.widget_hover = Color32::from_rgb(80, 73, 69);
    p.tab_active_bg = Color32::from_rgb(60, 56, 54);
    p.tab_active_fg = Color32::from_rgb(250, 189, 47);
    p.accent = Color32::from_rgb(250, 189, 47);
    p.border = Color32::from_rgb(102, 92, 84);
    p.text = Color32::from_rgb(235, 219, 178);
    p.text_muted = Color32::from_rgb(189, 174, 147);
    p.status_green = Color32::from_rgb(184, 187, 38);
    p.status_red = Color32::from_rgb(251, 73, 52);
    p.status_sky = Color32::from_rgb(131, 165, 152);
    p.section_bg = Color32::from_rgb(50, 48, 47);
    p.hover_row = Color32::from_rgb(68, 62, 58);
    p
}
