//! Model, collaboration, and reasoning popups for `ChatWidget`.
//!
//! These surfaces are tightly related because changing one often redirects
//! into another, especially while Plan mode is active.

use super::*;
use crate::bottom_pane::SelectionTab;
use crate::spawn_orchestration::SpawnRole;
use crate::spawn_orchestration::spawn_reasoning_effort_for_role;
#[cfg(test)]
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_5_MODEL_ID;
#[cfg(test)]
use codex_model_provider_info::AMAZON_BEDROCK_PROVIDER_ID;
use codex_model_provider_info::AMBIENT_DEFAULT_MODEL;
use codex_model_provider_info::AMBIENT_KIMI_K2_7_CODE_MODEL;
use codex_model_provider_info::AMBIENT_PROVIDER_ID;
#[cfg(test)]
use codex_model_provider_info::ANTHROPIC_DEFAULT_MODEL;
use codex_model_provider_info::ANTHROPIC_PROVIDER_ID;
#[cfg(test)]
use codex_model_provider_info::BASETEN_DEFAULT_MODEL;
use codex_model_provider_info::BASETEN_PROVIDER_ID;
#[cfg(test)]
use codex_model_provider_info::CLAUDE_FABLE_5_PLAN_MODEL;
#[cfg(test)]
use codex_model_provider_info::CLAUDE_PLAN_LEGACY_OPUS_4_8_MODEL;
#[cfg(test)]
use codex_model_provider_info::CLAUDE_PLAN_MODEL;
use codex_model_provider_info::CLAUDE_PLAN_PROVIDER_ID;
#[cfg(test)]
use codex_model_provider_info::DEEPSEEK_DEFAULT_MODEL;
use codex_model_provider_info::DEEPSEEK_PROVIDER_ID;
use codex_model_provider_info::KIMI_CODE_PROVIDER_ID;
#[cfg(test)]
use codex_model_provider_info::META_DEFAULT_MODEL;
use codex_model_provider_info::META_PROVIDER_ID;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::OPENROUTER_ANTHROPIC_PROVIDER_ID;
use codex_model_provider_info::OPENROUTER_PROVIDER_ID;
use codex_model_provider_info::PFTERMINAL_PLAN_API_KEY_ENV_VAR;
use codex_model_provider_info::PFTERMINAL_PLAN_PROVIDER_ID;
use codex_model_provider_info::VERCEL_ANTHROPIC_FAST_PROVIDER_ID;
use codex_model_provider_info::VERCEL_DEFAULT_MODEL;
use codex_model_provider_info::VERCEL_GLM_5_2_FAST_MODEL;
use codex_model_provider_info::VERCEL_PROVIDER_ID;
use codex_model_provider_info::ZAI_DEFAULT_MODEL;
use codex_model_provider_info::ZAI_PROVIDER_ID;
#[cfg(test)]
use codex_protocol::openai_models::ReasoningEffortPreset;
#[cfg(test)]
use codex_protocol::openai_models::default_input_modalities;

#[cfg(test)]
const OPENROUTER_OWL_ALPHA_MODEL: &str = "openrouter/owl-alpha";
#[cfg(test)]
const OPENROUTER_GROK_4_5_MODEL: &str = "x-ai/grok-4.5";
#[cfg(test)]
const OPENROUTER_DEEPSEEK_V4_PRO_MODEL: &str = "deepseek/deepseek-v4-pro";
#[cfg(test)]
const OPENROUTER_DEEPSEEK_V4_FLASH_0731_MODEL: &str = "deepseek/deepseek-v4-flash-0731";
#[cfg(test)]
const OPENROUTER_TENCENT_HY3_FREE_MODEL: &str = "tencent/hy3:free";
#[cfg(test)]
const OPENROUTER_KIMI_K3_MODEL: &str = "moonshotai/kimi-k3";
const OPENAI_GPT_5_5_MODEL: &str = "gpt-5.5";
const OPENAI_GPT_5_6_SOL_MODEL: &str = "gpt-5.6-sol";
const OPENAI_GPT_5_6_TERRA_MODEL: &str = "gpt-5.6-terra";
const OPENAI_GPT_5_6_LUNA_MODEL: &str = "gpt-5.6-luna";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelSelectionPurpose {
    Session,
    CodexPane {
        default_model: String,
    },
    SpawnAgent {
        role: SpawnRole,
        parent_node_id: Option<String>,
        default_model: String,
    },
}

impl ModelSelectionPurpose {
    fn selected_model<'a>(&'a self, session_model: &'a str) -> &'a str {
        match self {
            Self::Session => session_model,
            Self::CodexPane { default_model } => default_model,
            Self::SpawnAgent { default_model, .. } => default_model,
        }
    }

    fn provider_subtitle(&self, provider_subtitle: &str) -> String {
        match self {
            Self::Session => provider_subtitle.to_string(),
            Self::CodexPane { .. } => format!("New PFTerminal pane - {provider_subtitle}"),
            Self::SpawnAgent { role, .. } => {
                format!("PFTerminal {} pane - {provider_subtitle}", role.label())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelPickerProviderGroup {
    id: &'static str,
    label: &'static str,
    subtitle: &'static str,
}

const MODEL_PICKER_PROVIDER_GROUPS: [ModelPickerProviderGroup; 13] = [
    ModelPickerProviderGroup {
        id: "openai",
        label: "OpenAI",
        subtitle: "OpenAI Codex plan",
    },
    ModelPickerProviderGroup {
        id: "ambient",
        label: "Ambient",
        subtitle: "Ambient coding plan",
    },
    ModelPickerProviderGroup {
        id: "pfterminal-plan",
        label: "PfTerminal Plan",
        subtitle: "USDC-funded PfTerminal plan",
    },
    ModelPickerProviderGroup {
        id: "kimi-code",
        label: "Kimi Code",
        subtitle: "Kimi Code plan",
    },
    ModelPickerProviderGroup {
        id: "zai",
        label: "Z.AI",
        subtitle: "Z.AI coding plan",
    },
    ModelPickerProviderGroup {
        id: "deepseek",
        label: "DeepSeek",
        subtitle: "DeepSeek API key",
    },
    ModelPickerProviderGroup {
        id: "claude-plan",
        label: "Claude Plan",
        subtitle: "Claude Code plan",
    },
    ModelPickerProviderGroup {
        id: "anthropic",
        label: "Anthropic",
        subtitle: "Anthropic API key",
    },
    ModelPickerProviderGroup {
        id: "meta",
        label: "Meta",
        subtitle: "Meta Model API key",
    },
    ModelPickerProviderGroup {
        id: "vercel",
        label: "Vercel",
        subtitle: "Vercel AI Gateway API key",
    },
    ModelPickerProviderGroup {
        id: "baseten",
        label: "Baseten",
        subtitle: "Baseten API key",
    },
    ModelPickerProviderGroup {
        id: "openrouter",
        label: "OpenRouter",
        subtitle: "OpenRouter API key",
    },
    ModelPickerProviderGroup {
        id: "gpu",
        label: "Rented GPU",
        subtitle: "Authenticated PFTerminal rental",
    },
];

const ULTRA_REASONING_CONCURRENCY_WARNING_THRESHOLD: usize = 8;

impl ChatWidget {
    /// Open a popup to choose a quick auto model. Selecting "All models"
    /// opens the full picker with every available preset.
    pub(crate) fn open_model_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Model selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let presets: Vec<ModelPreset> = match self.model_catalog.try_list_models() {
            Ok(models) => models,
            Err(_) => {
                self.add_info_message(
                    "Models are being updated; please try /model again in a moment.".to_string(),
                    /*hint*/ None,
                );
                return;
            }
        };
        self.open_model_popup_with_presets(presets);
    }

    fn model_menu_header(&self, title: &str, subtitle: &str) -> Box<dyn Renderable> {
        let title = title.to_string();
        let subtitle = subtitle.to_string();
        let mut header = ColumnRenderable::new();
        header.push(Line::from(title.bold()));
        header.push(Line::from(subtitle.dim()));
        if let Some(warning) = self.model_menu_warning_line() {
            header.push(warning);
        }
        Box::new(header)
    }

    fn model_menu_warning_line(&self) -> Option<Line<'static>> {
        let base_url = self.custom_openai_base_url()?;
        let warning = format!(
            "Warning: OpenAI base URL is overridden to {base_url}. Selecting models may not be supported or work properly."
        );
        Some(Line::from(warning.red()))
    }

    fn custom_openai_base_url(&self) -> Option<String> {
        if !self.config.model_provider.is_openai() {
            return None;
        }

        let base_url = self.config.model_provider.base_url.as_ref()?;
        let trimmed = base_url.trim();
        if trimmed.is_empty() {
            return None;
        }

        let normalized = trimmed.trim_end_matches('/');
        if normalized == DEFAULT_OPENAI_BASE_URL {
            return None;
        }

        Some(trimmed.to_string())
    }

    pub(crate) fn open_model_popup_with_presets(&mut self, presets: Vec<ModelPreset>) {
        let presets: Vec<ModelPreset> = presets
            .into_iter()
            .filter(Self::show_in_pfterminal_model_picker)
            .collect();

        let current_model = self.current_model();
        let current_label = presets
            .iter()
            .find(|preset| preset.model.as_str() == current_model)
            .map(Self::model_display_label_for_preset)
            .unwrap_or_else(|| self.model_display_name());

        let (mut auto_presets, other_presets): (Vec<ModelPreset>, Vec<ModelPreset>) = presets
            .into_iter()
            .partition(|preset| Self::is_auto_model(&preset.model));

        if auto_presets.is_empty() {
            self.open_all_models_popup(other_presets);
            return;
        }

        auto_presets.sort_by_key(|preset| Self::auto_model_order(&preset.model));
        let mut items: Vec<SelectionItem> = auto_presets
            .into_iter()
            .map(|preset| {
                let description = Self::model_description_for_preset(&preset);
                let model = preset.model.clone();
                let display_name = Self::model_display_label_for_preset(&preset);
                let provider = preset
                    .provider_id
                    .clone()
                    .or_else(|| Self::model_provider_for_selection(&model));
                let requires_advanced_selection =
                    Self::is_advanced_reasoning_effort(&preset.default_reasoning_effort)
                        || preset
                            .supported_reasoning_efforts
                            .iter()
                            .any(|option| Self::is_advanced_reasoning_effort(&option.effort));
                let actions: Vec<SelectionAction> = if requires_advanced_selection {
                    let preset_for_action = preset.clone();
                    vec![Box::new(move |tx| {
                        tx.send(AppEvent::OpenReasoningPopup {
                            model: preset_for_action.clone(),
                            purpose: ModelSelectionPurpose::Session,
                        });
                    })]
                } else {
                    let should_prompt_plan_mode_scope = self
                        .should_prompt_plan_mode_reasoning_scope(
                            model.as_str(),
                            Some(preset.default_reasoning_effort.clone()),
                        );
                    self.model_selection_actions(
                        model.clone(),
                        provider,
                        Some(preset.default_reasoning_effort.clone()),
                        should_prompt_plan_mode_scope,
                    )
                };
                SelectionItem {
                    name: display_name.clone(),
                    description,
                    is_current: model.as_str() == current_model,
                    is_default: preset.is_default,
                    search_value: Some(format!("{display_name} {model}")),
                    actions,
                    dismiss_on_select: !requires_advanced_selection,
                    dismiss_parent_on_child_accept: requires_advanced_selection,
                    ..Default::default()
                }
            })
            .collect();

        if !other_presets.is_empty() {
            let all_models = other_presets;
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenAllModelsPopup {
                    models: all_models.clone(),
                });
            })];

            let is_current = !items.iter().any(|item| item.is_current);
            let description = Some(format!(
                "Choose a specific model and reasoning level (current: {current_label})"
            ));

            items.push(SelectionItem {
                name: "All models".to_string(),
                description,
                is_current,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let header = self.model_menu_header(
            "Select Model",
            "Pick a quick auto mode or browse all models.",
        );
        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header,
            ..Default::default()
        });
    }

    fn is_auto_model(model: &str) -> bool {
        model.starts_with("codex-auto-")
    }

    pub(crate) fn model_provider_for_selection(model: &str) -> Option<String> {
        codex_model_provider_info::canonical_catalog_provider(model).map(str::to_string)
    }

    fn resolved_model_provider(&self, model: &str) -> Option<String> {
        if model == self.current_model()
            && self.config.model_provider_id == PFTERMINAL_PLAN_PROVIDER_ID
        {
            return Some(PFTERMINAL_PLAN_PROVIDER_ID.to_string());
        }
        self.model_catalog
            .provider_for_model(model)
            .or_else(|| Self::model_provider_for_selection(model))
    }

    fn auto_model_order(model: &str) -> usize {
        match model {
            "codex-auto-fast" => 0,
            "codex-auto-balanced" => 1,
            "codex-auto-thorough" => 2,
            _ => 3,
        }
    }

    pub(crate) fn open_all_models_popup(&mut self, presets: Vec<ModelPreset>) {
        self.open_all_models_popup_for_purpose(presets, ModelSelectionPurpose::Session);
    }

    pub(crate) fn open_all_models_popup_for_purpose(
        &mut self,
        presets: Vec<ModelPreset>,
        purpose: ModelSelectionPurpose,
    ) {
        let mut presets: Vec<ModelPreset> = presets
            .into_iter()
            .filter(Self::show_in_pfterminal_model_picker)
            .collect();

        if self.pfterminal_plan_key_is_linked() {
            let paid_models = presets
                .iter()
                .filter(|preset| {
                    matches!(
                        preset.model.as_str(),
                        AMBIENT_DEFAULT_MODEL | AMBIENT_KIMI_K2_7_CODE_MODEL
                    )
                })
                .cloned()
                .map(|mut preset| {
                    preset.provider_id = Some(PFTERMINAL_PLAN_PROVIDER_ID.to_string());
                    preset.is_default = preset.model == AMBIENT_DEFAULT_MODEL;
                    preset
                })
                .collect::<Vec<_>>();
            presets.extend(paid_models);
        }

        if presets.is_empty() {
            self.add_info_message(
                "No additional models are available right now.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let mut provider_items = MODEL_PICKER_PROVIDER_GROUPS
            .into_iter()
            .map(|group| (group, Vec::new()))
            .collect::<Vec<(ModelPickerProviderGroup, Vec<SelectionItem>)>>();
        for preset in presets.into_iter() {
            let provider = preset
                .provider_id
                .clone()
                .or_else(|| Self::model_provider_for_selection(&preset.model));
            let Some(group) = Self::model_picker_provider_group(provider.as_deref()) else {
                continue;
            };
            let Some((_, items)) = provider_items
                .iter_mut()
                .find(|(candidate, _)| candidate.id == group.id)
            else {
                continue;
            };
            items.push(self.model_picker_item(preset, purpose.clone()));
        }
        for (_, items) in &mut provider_items {
            items.sort_by_key(|item| !item.is_default);
        }
        provider_items.retain(|(_, items)| !items.is_empty());

        let (items, tabs, initial_tab_id, footer_hint) = if provider_items.len() > 1 {
            let selected_model = purpose.selected_model(self.current_model());
            let current_provider = self.resolved_model_provider(selected_model);
            let current_group = Self::model_picker_provider_group(current_provider.as_deref());
            let initial_tab_id = current_group
                .filter(|group| {
                    provider_items
                        .iter()
                        .any(|(candidate, _)| candidate.id == group.id)
                })
                .map(|group| group.id.to_string());
            let tabs = provider_items
                .into_iter()
                .map(|(group, items)| SelectionTab {
                    id: group.id.to_string(),
                    label: group.label.to_string(),
                    header: self.model_menu_header(
                        "Select Model and Effort",
                        &purpose.provider_subtitle(group.subtitle),
                    ),
                    items,
                })
                .collect();
            (
                Vec::new(),
                tabs,
                initial_tab_id,
                Some(Self::model_picker_tabbed_footer_hint_line()),
            )
        } else {
            let items = provider_items
                .pop()
                .map(|(_, items)| items)
                .unwrap_or_default();
            (items, Vec::new(), None, Some(standard_popup_hint_line()))
        };

        let header = if tabs.is_empty() {
            self.model_menu_header(
                "Select Model and Effort",
                "Access hidden models by running pfterminal -m <model_name> or in your config.toml",
            )
        } else {
            Box::new(())
        };
        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint,
            items,
            tabs,
            initial_tab_id,
            header,
            ..Default::default()
        });
    }

    fn model_picker_item(
        &self,
        preset: ModelPreset,
        purpose: ModelSelectionPurpose,
    ) -> SelectionItem {
        let description = Self::model_description_for_preset(&preset);
        let is_current = preset.model.as_str() == purpose.selected_model(self.current_model());
        let direct_select = preset.supported_reasoning_efforts.len() <= 1;
        let preset_for_action = preset.clone();
        let display_name = Self::model_display_label_for_preset(&preset);
        let model = preset.model.clone();
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            let preset_for_event = preset_for_action.clone();
            tx.send(AppEvent::OpenReasoningPopup {
                model: preset_for_event,
                purpose: purpose.clone(),
            });
        })];
        SelectionItem {
            name: display_name.clone(),
            description,
            is_current,
            is_default: preset.is_default,
            search_value: Some(format!("{display_name} {model}")),
            actions,
            dismiss_on_select: direct_select,
            dismiss_parent_on_child_accept: !direct_select,
            ..Default::default()
        }
    }

    fn model_display_label_for_preset(preset: &ModelPreset) -> String {
        let display_name = preset.display_name.trim();
        if display_name.is_empty() {
            preset.model.clone()
        } else {
            display_name.to_string()
        }
    }

    fn model_description_for_preset(preset: &ModelPreset) -> Option<String> {
        let description = preset.description.trim();
        let display_name = Self::model_display_label_for_preset(preset);
        let slug = preset.model.as_str();
        let slug_prefix = (display_name != slug).then(|| format!("Model: {slug}"));
        match (slug_prefix, description.is_empty()) {
            (Some(prefix), false) => Some(format!("{prefix}. {description}")),
            (Some(prefix), true) => Some(prefix),
            (None, false) => Some(description.to_string()),
            (None, true) => None,
        }
    }

    fn model_picker_tabbed_footer_hint_line() -> Line<'static> {
        Line::from("Use Left/Right to switch providers. Press Enter to confirm or Esc to go back")
    }

    pub(crate) fn show_in_pfterminal_model_picker(preset: &ModelPreset) -> bool {
        if !preset.show_in_picker {
            return false;
        }

        let provider = preset
            .provider_id
            .clone()
            .or_else(|| Self::model_provider_for_selection(&preset.model));
        match provider.as_deref() {
            Some(OPENAI_PROVIDER_ID) => Self::is_openai_coding_plan_model(&preset.model),
            Some(
                AMBIENT_PROVIDER_ID
                | PFTERMINAL_PLAN_PROVIDER_ID
                | KIMI_CODE_PROVIDER_ID
                | CLAUDE_PLAN_PROVIDER_ID
                | ANTHROPIC_PROVIDER_ID
                | ZAI_PROVIDER_ID
                | DEEPSEEK_PROVIDER_ID
                | BASETEN_PROVIDER_ID
                | OPENROUTER_PROVIDER_ID
                | OPENROUTER_ANTHROPIC_PROVIDER_ID
                | META_PROVIDER_ID
                | VERCEL_PROVIDER_ID
                | VERCEL_ANTHROPIC_FAST_PROVIDER_ID,
            ) => true,
            Some(provider) if provider.starts_with("gpu-") => true,
            _ => false,
        }
    }

    fn is_openai_coding_plan_model(model: &str) -> bool {
        matches!(
            model.trim(),
            OPENAI_GPT_5_5_MODEL
                | OPENAI_GPT_5_6_SOL_MODEL
                | OPENAI_GPT_5_6_TERRA_MODEL
                | OPENAI_GPT_5_6_LUNA_MODEL
        )
    }

    fn model_picker_provider_group(provider: Option<&str>) -> Option<ModelPickerProviderGroup> {
        let group_id = match provider {
            Some(OPENAI_PROVIDER_ID) => "openai",
            Some(AMBIENT_PROVIDER_ID) => "ambient",
            Some(PFTERMINAL_PLAN_PROVIDER_ID) => "pfterminal-plan",
            Some(KIMI_CODE_PROVIDER_ID) => "kimi-code",
            Some(ZAI_PROVIDER_ID) => "zai",
            Some(DEEPSEEK_PROVIDER_ID) => "deepseek",
            Some(CLAUDE_PLAN_PROVIDER_ID) => "claude-plan",
            Some(ANTHROPIC_PROVIDER_ID) => "anthropic",
            Some(META_PROVIDER_ID) => "meta",
            Some(VERCEL_PROVIDER_ID | VERCEL_ANTHROPIC_FAST_PROVIDER_ID) => "vercel",
            Some(BASETEN_PROVIDER_ID) => "baseten",
            Some(OPENROUTER_PROVIDER_ID | OPENROUTER_ANTHROPIC_PROVIDER_ID) => "openrouter",
            Some(provider) if provider.starts_with("gpu-") => "gpu",
            _ => return None,
        };
        MODEL_PICKER_PROVIDER_GROUPS
            .into_iter()
            .find(|group| group.id == group_id)
    }

    pub(crate) fn pfterminal_plan_key_is_linked(&self) -> bool {
        codex_login::provider_api_key_from_auth_storage(
            &self.config.codex_home,
            PFTERMINAL_PLAN_API_KEY_ENV_VAR,
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        )
        .is_ok_and(|key| key.is_some_and(|value| !value.trim().is_empty()))
    }

    fn model_selection_actions(
        &self,
        model_for_action: String,
        provider_for_action: Option<String>,
        effort_for_action: Option<ReasoningEffortConfig>,
        should_prompt_plan_mode_scope: bool,
    ) -> Vec<SelectionAction> {
        let warning = effort_for_action
            .as_ref()
            .and_then(|effort| self.ultra_reasoning_concurrency_warning(effort));
        vec![Box::new(move |tx| {
            if effort_for_action == Some(ReasoningEffortConfig::Ultra) {
                tx.send(AppEvent::ApplyAdvancedReasoning {
                    model: model_for_action.clone(),
                    effort: ReasoningEffortConfig::Ultra,
                });
            } else if should_prompt_plan_mode_scope {
                tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                    model: model_for_action.clone(),
                    provider: provider_for_action.clone(),
                    effort: effort_for_action.clone(),
                });
            } else {
                tx.send(AppEvent::UpdateModelSelection {
                    model: model_for_action.clone(),
                    provider: provider_for_action.clone(),
                });
                tx.send(AppEvent::UpdateReasoningEffort(effort_for_action.clone()));
                if Self::should_persist_model_provider(provider_for_action.as_deref()) {
                    tx.send(AppEvent::PersistModelSelection {
                        model: model_for_action.clone(),
                        provider: provider_for_action.clone(),
                        effort: effort_for_action.clone(),
                    });
                }
            }
            if let Some(warning) = warning.clone() {
                tx.send(AppEvent::InsertHistoryCell(Box::new(
                    history_cell::new_warning_event(warning),
                )));
            }
        })]
    }

    fn should_persist_model_provider(provider: Option<&str>) -> bool {
        !provider.is_some_and(|provider| provider.starts_with("gpu-"))
    }

    fn should_prompt_plan_mode_reasoning_scope(
        &self,
        selected_model: &str,
        selected_effort: Option<ReasoningEffortConfig>,
    ) -> bool {
        if !self.collaboration_modes_enabled()
            || self.active_mode_kind() != ModeKind::Plan
            || selected_model != self.current_model()
        {
            return false;
        }

        // Prompt whenever the selection is not a true no-op for both:
        // 1) the active Plan-mode effective reasoning, and
        // 2) the stored global defaults that would be updated by the fallback path.
        selected_effort != self.effective_reasoning_effort()
            || selected_model != self.current_collaboration_mode.model()
            || selected_effort != self.current_collaboration_mode.reasoning_effort()
    }

    pub(crate) fn open_plan_reasoning_scope_prompt(
        &mut self,
        model: String,
        provider: Option<String>,
        effort: Option<ReasoningEffortConfig>,
    ) {
        let reasoning_phrase = match effort.as_ref() {
            Some(ReasoningEffortConfig::None) => "no reasoning".to_string(),
            Some(selected_effort) => {
                format!(
                    "{} reasoning",
                    Self::reasoning_effort_sentence_label(selected_effort)
                )
            }
            None => "the selected reasoning".to_string(),
        };
        let plan_only_description = format!("Always use {reasoning_phrase} in Plan mode.");
        let plan_reasoning_source = if let Some(plan_override) =
            self.config.plan_mode_reasoning_effort.as_ref()
        {
            format!(
                "user-chosen Plan override ({})",
                Self::reasoning_effort_sentence_label(plan_override)
            )
        } else if let Some(plan_mask) = collaboration_modes::plan_mask(self.model_catalog.as_ref())
        {
            match plan_mask
                .reasoning_effort
                .as_ref()
                .and_then(|effort| effort.as_ref())
            {
                Some(plan_effort) => format!(
                    "built-in Plan default ({})",
                    Self::reasoning_effort_sentence_label(plan_effort)
                ),
                None => "built-in Plan default (no reasoning)".to_string(),
            }
        } else {
            "built-in Plan default".to_string()
        };
        let all_modes_description = format!(
            "Set the global default reasoning level and the Plan mode override. This replaces the current {plan_reasoning_source}."
        );
        let subtitle = format!("Choose where to apply {reasoning_phrase}.");
        let warning = effort
            .as_ref()
            .and_then(|effort| self.ultra_reasoning_concurrency_warning(effort));

        let plan_only_actions: Vec<SelectionAction> = vec![Box::new({
            let model = model.clone();
            let effort = effort.clone();
            let provider = provider.clone();
            let warning = warning.clone();
            move |tx| {
                tx.send(AppEvent::UpdateModelSelection {
                    model: model.clone(),
                    provider: provider.clone(),
                });
                tx.send(AppEvent::UpdatePlanModeReasoningEffort(effort.clone()));
                tx.send(AppEvent::PersistPlanModeReasoningEffort(effort.clone()));
                if let Some(warning) = warning.clone() {
                    tx.send(AppEvent::InsertHistoryCell(Box::new(
                        history_cell::new_warning_event(warning),
                    )));
                }
            }
        })];
        let all_modes_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::UpdateModelSelection {
                model: model.clone(),
                provider: provider.clone(),
            });
            tx.send(AppEvent::UpdateReasoningEffort(effort.clone()));
            tx.send(AppEvent::UpdatePlanModeReasoningEffort(effort.clone()));
            tx.send(AppEvent::PersistPlanModeReasoningEffort(effort.clone()));
            tx.send(AppEvent::PersistModelSelection {
                model: model.clone(),
                provider: provider.clone(),
                effort: effort.clone(),
            });
            if let Some(warning) = warning.clone() {
                tx.send(AppEvent::InsertHistoryCell(Box::new(
                    history_cell::new_warning_event(warning),
                )));
            }
        })];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some(PLAN_MODE_REASONING_SCOPE_TITLE.to_string()),
            subtitle: Some(subtitle),
            footer_hint: Some(standard_popup_hint_line()),
            items: vec![
                SelectionItem {
                    name: PLAN_MODE_REASONING_SCOPE_PLAN_ONLY.to_string(),
                    description: Some(plan_only_description),
                    actions: plan_only_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: PLAN_MODE_REASONING_SCOPE_ALL_MODES.to_string(),
                    description: Some(all_modes_description),
                    actions: all_modes_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        self.notify(Notification::PlanModePrompt {
            title: PLAN_MODE_REASONING_SCOPE_TITLE.to_string(),
        });
    }

    /// Open a popup to choose the standard reasoning effort for the given model.
    ///
    /// Max and Ultra require an explicit second step so expensive efforts cannot
    /// be selected accidentally while moving through the normal effort scale.
    #[cfg(test)]
    pub(crate) fn open_reasoning_popup(&mut self, preset: ModelPreset) {
        self.open_reasoning_popup_for_purpose(preset, ModelSelectionPurpose::Session);
    }

    pub(crate) fn open_reasoning_popup_for_purpose(
        &mut self,
        preset: ModelPreset,
        purpose: ModelSelectionPurpose,
    ) {
        let spawn_default_effort = match &purpose {
            ModelSelectionPurpose::SpawnAgent { role, .. } => {
                Some(spawn_reasoning_effort_for_role(*role, &preset))
            }
            ModelSelectionPurpose::Session | ModelSelectionPurpose::CodexPane { .. } => None,
        };
        let model_label = Self::model_display_label_for_preset(&preset);
        let provider = preset
            .provider_id
            .clone()
            .or_else(|| Self::model_provider_for_selection(&preset.model));
        let default_effort = preset.default_reasoning_effort.clone();
        let supported = &preset.supported_reasoning_efforts;
        let in_plan_mode =
            self.collaboration_modes_enabled() && self.active_mode_kind() == ModeKind::Plan;
        let uses_ambient_reasoning_modes = Self::uses_glm_reasoning_modes(&preset.model);

        let warn_effort = if supported
            .iter()
            .any(|option| option.effort == ReasoningEffortConfig::XHigh)
        {
            Some(ReasoningEffortConfig::XHigh)
        } else if supported
            .iter()
            .any(|option| option.effort == ReasoningEffortConfig::High)
        {
            Some(ReasoningEffortConfig::High)
        } else {
            None
        };
        let warning_text = warn_effort.as_ref().map(|effort| {
            let effort_label = Self::reasoning_effort_label_for_model(&preset.model, effort);
            format!("⚠ {effort_label} reasoning effort can quickly consume Plus plan rate limits.")
        });
        let warn_for_model = preset.model.starts_with("gpt-5.1-codex")
            || preset.model.starts_with("gpt-5.1-codex-max")
            || preset.model.starts_with("gpt-5.2");

        let mut all_choices: Vec<ReasoningEffortConfig> = supported
            .iter()
            .map(|option| option.effort.clone())
            .collect();
        if all_choices.is_empty() {
            all_choices.push(default_effort.clone());
        }
        let (choices, advanced_choices): (Vec<_>, Vec<_>) = all_choices
            .into_iter()
            .partition(|effort| !Self::is_advanced_reasoning_effort(effort));

        if choices.len() == 1 && advanced_choices.is_empty() {
            let selected_effort = choices.first().cloned();
            let selected_model = preset.model;
            match purpose {
                ModelSelectionPurpose::Session => {
                    if self.should_prompt_plan_mode_reasoning_scope(
                        &selected_model,
                        selected_effort.clone(),
                    ) {
                        self.app_event_tx
                            .send(AppEvent::OpenPlanReasoningScopePrompt {
                                model: selected_model,
                                provider,
                                effort: selected_effort,
                            });
                    } else {
                        self.apply_model_and_effort(selected_model, selected_effort);
                    }
                }
                ModelSelectionPurpose::CodexPane { .. } => {
                    self.app_event_tx.send(AppEvent::OpenCodexPaneNamePrompt {
                        provider,
                        model: selected_model,
                        effort: selected_effort,
                    });
                }
                ModelSelectionPurpose::SpawnAgent {
                    role,
                    parent_node_id,
                    ..
                } => self.app_event_tx.send(AppEvent::CreateSpawnAgent {
                    role,
                    parent_node_id,
                    agent_nickname: None,
                    provider,
                    model: selected_model,
                    effort: selected_effort,
                }),
            }
            return;
        }

        let default_choice = choices
            .contains(&default_effort)
            .then(|| default_effort.clone());

        let model_slug = preset.model.to_string();
        let is_current_model = self.current_model() == preset.model.as_str();
        let highlight_choice = match &purpose {
            ModelSelectionPurpose::SpawnAgent { .. } => spawn_default_effort,
            ModelSelectionPurpose::CodexPane { .. } => default_choice.clone(),
            ModelSelectionPurpose::Session if is_current_model => {
                if in_plan_mode {
                    self.config
                        .plan_mode_reasoning_effort
                        .clone()
                        .or_else(|| self.effective_reasoning_effort())
                } else {
                    self.effective_reasoning_effort()
                }
            }
            ModelSelectionPurpose::Session => {
                default_choice.clone().or_else(|| choices.first().cloned())
            }
        };
        let selection_choice = highlight_choice.clone().or_else(|| default_choice.clone());
        let initial_selected_idx = choices
            .iter()
            .position(|choice| Some(choice) == selection_choice.as_ref());
        let mut items: Vec<SelectionItem> = Vec::new();
        for choice in choices.iter() {
            let effort = choice.clone();
            let mut effort_label = Self::reasoning_effort_label_for_model(&model_slug, &effort);
            if Some(choice) == default_choice.as_ref() {
                effort_label.push_str(" (default)");
            }

            let description = supported
                .iter()
                .find(|option| option.effort == effort)
                .map(|option| option.description.to_string())
                .filter(|text| !text.is_empty());

            let show_warning = warn_for_model && warn_effort.as_ref() == Some(&effort);
            let selected_description = if show_warning {
                warning_text.as_ref().map(|warning_message| {
                    description.as_ref().map_or_else(
                        || warning_message.clone(),
                        |d| format!("{d}\n{warning_message}"),
                    )
                })
            } else {
                None
            };

            let choice_effort = Some(effort);
            let should_prompt_plan_mode_scope = matches!(purpose, ModelSelectionPurpose::Session)
                && self.should_prompt_plan_mode_reasoning_scope(
                    model_slug.as_str(),
                    choice_effort.clone(),
                );
            let purpose_for_action = purpose.clone();
            let provider_for_action = provider.clone();
            let model_for_action = model_slug.clone();
            let actions: Vec<SelectionAction> =
                vec![Box::new(move |tx| match &purpose_for_action {
                    ModelSelectionPurpose::Session => {
                        if should_prompt_plan_mode_scope {
                            tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                                model: model_for_action.clone(),
                                provider: provider_for_action.clone(),
                                effort: choice_effort.clone(),
                            });
                        } else {
                            tx.send(AppEvent::UpdateModelSelection {
                                model: model_for_action.clone(),
                                provider: provider_for_action.clone(),
                            });
                            tx.send(AppEvent::UpdateReasoningEffort(choice_effort.clone()));
                            if Self::should_persist_model_provider(provider_for_action.as_deref()) {
                                tx.send(AppEvent::PersistModelSelection {
                                    model: model_for_action.clone(),
                                    provider: provider_for_action.clone(),
                                    effort: choice_effort.clone(),
                                });
                            }
                        }
                    }
                    ModelSelectionPurpose::CodexPane { .. } => {
                        tx.send(AppEvent::OpenCodexPaneNamePrompt {
                            provider: provider_for_action.clone(),
                            model: model_for_action.clone(),
                            effort: choice_effort.clone(),
                        });
                    }
                    ModelSelectionPurpose::SpawnAgent {
                        role,
                        parent_node_id,
                        ..
                    } => tx.send(AppEvent::CreateSpawnAgent {
                        role: *role,
                        parent_node_id: parent_node_id.clone(),
                        agent_nickname: None,
                        provider: provider_for_action.clone(),
                        model: model_for_action.clone(),
                        effort: choice_effort.clone(),
                    }),
                })];

            items.push(SelectionItem {
                name: effort_label,
                description,
                selected_description,
                is_current: is_current_model && Some(choice) == highlight_choice.as_ref(),
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        if !advanced_choices.is_empty() {
            let advanced_label = advanced_choices
                .iter()
                .map(Self::reasoning_effort_label)
                .collect::<Vec<_>>()
                .join(" and ");
            let verb = if advanced_choices.len() == 1 {
                "consumes"
            } else {
                "consume"
            };
            let preset_for_action = preset;
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenAdvancedReasoningPopup {
                    model: preset_for_action.clone(),
                });
            })];
            items.push(SelectionItem {
                name: "More reasoning…".to_string(),
                description: Some(format!("{advanced_label} {verb} usage limits faster")),
                is_current: is_current_model
                    && highlight_choice
                        .as_ref()
                        .is_some_and(Self::is_advanced_reasoning_effort),
                actions,
                dismiss_parent_on_child_accept: true,
                ..Default::default()
            });
        }

        let mut header = ColumnRenderable::new();
        let header_title = if uses_ambient_reasoning_modes {
            "Select Reasoning Mode"
        } else {
            "Select Reasoning Level"
        };
        header.push(Line::from(
            format!("{header_title} for {model_label}").bold(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            initial_selected_idx,
            ..Default::default()
        });
    }

    fn reasoning_effort_label_for_model(model: &str, effort: &ReasoningEffortConfig) -> String {
        if Self::uses_glm_reasoning_modes(model) {
            return match effort {
                ReasoningEffortConfig::High | ReasoningEffortConfig::XHigh => "Deep".to_string(),
                ReasoningEffortConfig::Custom(value)
                    if matches!(
                        value.as_str(),
                        "deep" | "max" | "xhigh" | "extra_high" | "extra-high"
                    ) =>
                {
                    "Deep".to_string()
                }
                _ => "Standard".to_string(),
            };
        }

        Self::reasoning_effort_label(effort)
    }

    fn uses_glm_reasoning_modes(model: &str) -> bool {
        matches!(
            model,
            AMBIENT_DEFAULT_MODEL
                | AMBIENT_KIMI_K2_7_CODE_MODEL
                | ZAI_DEFAULT_MODEL
                | VERCEL_DEFAULT_MODEL
                | VERCEL_GLM_5_2_FAST_MODEL
        )
    }

    /// Open the explicit Max/Ultra effort picker for the given model.
    pub(crate) fn open_advanced_reasoning_popup(&mut self, preset: ModelPreset) {
        let provider = preset
            .provider_id
            .clone()
            .or_else(|| Self::model_provider_for_selection(&preset.model));
        let mut choices = preset
            .supported_reasoning_efforts
            .iter()
            .map(|option| option.effort.clone())
            .filter(Self::is_advanced_reasoning_effort)
            .collect::<Vec<_>>();
        if choices.is_empty()
            && Self::is_advanced_reasoning_effort(&preset.default_reasoning_effort)
        {
            choices.push(preset.default_reasoning_effort.clone());
        }
        choices.sort_by_key(|effort| matches!(effort, ReasoningEffortConfig::Ultra));
        if choices.is_empty() {
            return;
        }

        let model_slug = preset.model.to_string();
        let is_current_model = self.current_model() == preset.model.as_str();
        let highlight_choice = is_current_model
            .then(|| self.effective_reasoning_effort())
            .flatten();
        let mut items = Vec::new();
        for effort in choices {
            let description = match &effort {
                ReasoningEffortConfig::Max => {
                    "For difficult problems when quality matters more than speed · higher usage"
                }
                ReasoningEffortConfig::Ultra => {
                    "For demanding work using multiple agents · highest usage"
                }
                _ => unreachable!("advanced choices are limited to Max and Ultra"),
            };
            let should_prompt_plan_mode_scope = self
                .should_prompt_plan_mode_reasoning_scope(model_slug.as_str(), Some(effort.clone()));
            let actions = self.model_selection_actions(
                model_slug.clone(),
                provider.clone(),
                Some(effort.clone()),
                should_prompt_plan_mode_scope,
            );

            items.push(SelectionItem {
                name: Self::reasoning_effort_label(&effort),
                description: Some(description.to_string()),
                is_current: is_current_model && Some(&effort) == highlight_choice.as_ref(),
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Advanced Reasoning".bold()));
        header.push(Line::from("⚠ Consumes usage limits faster".cyan()));
        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(super) fn is_advanced_reasoning_effort(effort: &ReasoningEffortConfig) -> bool {
        matches!(
            effort,
            ReasoningEffortConfig::Max | ReasoningEffortConfig::Ultra
        )
    }

    pub(super) fn reasoning_effort_label(effort: &ReasoningEffortConfig) -> String {
        match effort {
            ReasoningEffortConfig::None => "None".to_string(),
            ReasoningEffortConfig::Minimal => "Minimal".to_string(),
            ReasoningEffortConfig::Low => "Low".to_string(),
            ReasoningEffortConfig::Medium => "Medium".to_string(),
            ReasoningEffortConfig::High => "High".to_string(),
            ReasoningEffortConfig::XHigh => "Extra high".to_string(),
            ReasoningEffortConfig::Max => "Max".to_string(),
            ReasoningEffortConfig::Ultra => "Ultra".to_string(),
            ReasoningEffortConfig::Custom(value) => value.clone(),
        }
    }

    pub(super) fn reasoning_effort_sentence_label(effort: &ReasoningEffortConfig) -> String {
        match effort {
            ReasoningEffortConfig::Custom(value) => value.clone(),
            effort => Self::reasoning_effort_label(effort).to_lowercase(),
        }
    }

    pub(super) fn ultra_reasoning_concurrency_warning(
        &self,
        effort: &ReasoningEffortConfig,
    ) -> Option<String> {
        if effort != &ReasoningEffortConfig::Ultra {
            return None;
        }

        let max_threads = self
            .config
            .multi_agent_v2
            .max_concurrent_threads_per_session;
        if max_threads < ULTRA_REASONING_CONCURRENCY_WARNING_THRESHOLD {
            return None;
        }

        let max_subagents = max_threads.saturating_sub(1);
        Some(format!(
            "Ultra reasoning may proactively use multiple agents. This session is configured for \
             {max_threads} concurrent threads with up to {max_subagents} subagents which can \
             increase usage quickly. Consider setting \
             features.multi_agent_v2.max_concurrent_threads_per_session below 8."
        ))
    }

    pub(super) fn apply_model_and_effort_without_persist(
        &self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) {
        let warning = effort
            .as_ref()
            .and_then(|effort| self.ultra_reasoning_concurrency_warning(effort));
        self.app_event_tx.send(AppEvent::UpdateModel(model));
        self.app_event_tx
            .send(AppEvent::UpdateReasoningEffort(effort));
        if let Some(warning) = warning {
            self.app_event_tx.send(AppEvent::InsertHistoryCell(Box::new(
                history_cell::new_warning_event(warning),
            )));
        }
    }

    fn apply_model_and_effort(&self, model: String, effort: Option<ReasoningEffortConfig>) {
        self.apply_model_and_effort_without_persist(model.clone(), effort.clone());
        let provider = self.resolved_model_provider(&model);
        if Self::should_persist_model_provider(provider.as_deref()) {
            self.app_event_tx.send(AppEvent::PersistModelSelection {
                model,
                provider,
                effort,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runtime_gpu_model_selection_is_session_only() {
        let (chat, mut rx, _op_rx) =
            crate::chatwidget::tests::helpers::make_chatwidget_manual(None).await;
        let actions = chat.model_selection_actions(
            "pinned-model".to_string(),
            Some("gpu-rental-123".to_string()),
            None,
            false,
        );

        actions[0](&chat.app_event_tx);
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();

        assert!(events.iter().any(|event| matches!(
            event,
            AppEvent::UpdateModelSelection { model, provider }
                if model == "pinned-model" && provider.as_deref() == Some("gpu-rental-123")
        )));
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, AppEvent::PersistModelSelection { .. })),
            "runtime GPU selection must not mutate the static model configuration: {events:?}"
        );
    }

    #[test]
    fn model_provider_for_selection_maps_cross_provider_models() {
        assert_eq!(
            ChatWidget::model_provider_for_selection(DEEPSEEK_DEFAULT_MODEL).as_deref(),
            Some(DEEPSEEK_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(META_DEFAULT_MODEL).as_deref(),
            Some(META_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(AMBIENT_DEFAULT_MODEL).as_deref(),
            Some(AMBIENT_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(AMBIENT_KIMI_K2_7_CODE_MODEL).as_deref(),
            Some(AMBIENT_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(ZAI_DEFAULT_MODEL).as_deref(),
            Some(ZAI_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(CLAUDE_PLAN_MODEL).as_deref(),
            Some(CLAUDE_PLAN_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(CLAUDE_PLAN_LEGACY_OPUS_4_8_MODEL).as_deref(),
            Some(CLAUDE_PLAN_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(CLAUDE_FABLE_5_PLAN_MODEL).as_deref(),
            Some(CLAUDE_PLAN_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(ANTHROPIC_DEFAULT_MODEL).as_deref(),
            Some(ANTHROPIC_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(BASETEN_DEFAULT_MODEL).as_deref(),
            Some(BASETEN_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(OPENROUTER_OWL_ALPHA_MODEL).as_deref(),
            Some(OPENROUTER_PROVIDER_ID)
        );
        for model in [
            OPENROUTER_GROK_4_5_MODEL,
            OPENROUTER_DEEPSEEK_V4_PRO_MODEL,
            OPENROUTER_DEEPSEEK_V4_FLASH_0731_MODEL,
            OPENROUTER_TENCENT_HY3_FREE_MODEL,
            OPENROUTER_KIMI_K3_MODEL,
        ] {
            assert_eq!(
                ChatWidget::model_provider_for_selection(model).as_deref(),
                Some(OPENROUTER_PROVIDER_ID),
                "expected {model} to route through OpenRouter"
            );
        }
        assert_eq!(
            ChatWidget::model_provider_for_selection(VERCEL_DEFAULT_MODEL).as_deref(),
            Some(VERCEL_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(VERCEL_GLM_5_2_FAST_MODEL).as_deref(),
            Some(VERCEL_ANTHROPIC_FAST_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection("minimax/minimax-m3").as_deref(),
            Some(OPENROUTER_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection("gpt-5.5").as_deref(),
            Some(OPENAI_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(AMAZON_BEDROCK_GPT_5_5_MODEL_ID).as_deref(),
            Some(AMAZON_BEDROCK_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::reasoning_effort_label_for_model(
                AMBIENT_KIMI_K2_7_CODE_MODEL,
                &ReasoningEffortConfig::Medium,
            ),
            "Standard"
        );
        assert_eq!(
            ChatWidget::reasoning_effort_label_for_model(
                AMBIENT_KIMI_K2_7_CODE_MODEL,
                &ReasoningEffortConfig::XHigh,
            ),
            "Deep"
        );
        assert_eq!(
            ChatWidget::reasoning_effort_label_for_model(
                ZAI_DEFAULT_MODEL,
                &ReasoningEffortConfig::Medium,
            ),
            "Standard"
        );
    }

    #[test]
    fn model_picker_groups_models_by_user_facing_provider() {
        let group_label = |provider| {
            ChatWidget::model_picker_provider_group(Some(provider)).map(|group| group.label)
        };

        assert_eq!(group_label(OPENAI_PROVIDER_ID), Some("OpenAI"));
        assert_eq!(group_label(AMBIENT_PROVIDER_ID), Some("Ambient"));
        assert_eq!(group_label(ZAI_PROVIDER_ID), Some("Z.AI"));
        assert_eq!(group_label(DEEPSEEK_PROVIDER_ID), Some("DeepSeek"));
        assert_eq!(group_label(CLAUDE_PLAN_PROVIDER_ID), Some("Claude Plan"));
        assert_eq!(group_label(ANTHROPIC_PROVIDER_ID), Some("Anthropic"));
        assert_eq!(group_label(META_PROVIDER_ID), Some("Meta"));
        assert_eq!(group_label(BASETEN_PROVIDER_ID), Some("Baseten"));
        assert_eq!(group_label(VERCEL_PROVIDER_ID), Some("Vercel"));
        assert_eq!(
            group_label(VERCEL_ANTHROPIC_FAST_PROVIDER_ID),
            Some("Vercel")
        );
        assert_eq!(group_label(OPENROUTER_PROVIDER_ID), Some("OpenRouter"));
        assert_eq!(
            group_label(OPENROUTER_ANTHROPIC_PROVIDER_ID),
            Some("OpenRouter")
        );
        assert_eq!(group_label(AMAZON_BEDROCK_PROVIDER_ID), None);
    }

    #[test]
    fn model_picker_display_label_uses_catalog_name_with_slug_in_description() {
        let mut ambient = preset(AMBIENT_DEFAULT_MODEL, true);
        ambient.display_name = "Ambient GLM 5.2".to_string();
        ambient.description = "Ambient-backed GLM.".to_string();

        assert_eq!(
            ChatWidget::model_display_label_for_preset(&ambient),
            "Ambient GLM 5.2"
        );
        assert_eq!(
            ChatWidget::model_description_for_preset(&ambient),
            Some(format!(
                "Model: {AMBIENT_DEFAULT_MODEL}. Ambient-backed GLM."
            ))
        );

        let fallback = preset("custom-model", true);
        assert_eq!(
            ChatWidget::model_display_label_for_preset(&fallback),
            "custom-model"
        );
        assert_eq!(
            ChatWidget::model_description_for_preset(&fallback),
            Some("custom-model description".to_string())
        );
    }

    #[test]
    fn pfterminal_picker_allows_curated_openai_plan_models() {
        assert!(ChatWidget::show_in_pfterminal_model_picker(&preset(
            AMBIENT_KIMI_K2_7_CODE_MODEL,
            true
        )));
        assert!(ChatWidget::show_in_pfterminal_model_picker(&preset(
            ANTHROPIC_DEFAULT_MODEL,
            true
        )));
        assert!(ChatWidget::show_in_pfterminal_model_picker(&preset(
            CLAUDE_PLAN_MODEL,
            true
        )));
        assert!(ChatWidget::show_in_pfterminal_model_picker(&preset(
            CLAUDE_FABLE_5_PLAN_MODEL,
            true
        )));
        assert!(ChatWidget::show_in_pfterminal_model_picker(&preset(
            DEEPSEEK_DEFAULT_MODEL,
            true
        )));
        assert!(ChatWidget::show_in_pfterminal_model_picker(&preset(
            "gpt-5.5", true
        )));
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            assert!(ChatWidget::show_in_pfterminal_model_picker(&preset(
                model, true
            )));
        }
        assert!(!ChatWidget::show_in_pfterminal_model_picker(&preset(
            "gpt-5.4", true
        )));
        assert!(!ChatWidget::show_in_pfterminal_model_picker(&preset(
            "codex-auto-review",
            true
        )));
        assert!(!ChatWidget::show_in_pfterminal_model_picker(&preset(
            "gpt-5.5", false
        )));
    }

    fn preset(model: &str, show_in_picker: bool) -> ModelPreset {
        ModelPreset {
            id: model.to_string(),
            model: model.to_string(),
            provider_id: None,
            model_specialty: None,
            orchestration: None,
            display_name: model.to_string(),
            description: format!("{model} description"),
            default_reasoning_effort: ReasoningEffortConfig::Medium,
            supported_reasoning_efforts: vec![ReasoningEffortPreset {
                effort: ReasoningEffortConfig::Medium,
                description: "medium".to_string(),
            }],
            supports_personality: false,
            additional_speed_tiers: Vec::new(),
            service_tiers: Vec::new(),
            default_service_tier: None,
            is_default: false,
            upgrade: None,
            show_in_picker,
            multi_agent_version: None,
            availability_nux: None,
            supported_in_api: true,
            input_modalities: default_input_modalities(),
        }
    }
}
