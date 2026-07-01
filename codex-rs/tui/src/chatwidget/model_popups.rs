//! Model, collaboration, and reasoning popups for `ChatWidget`.
//!
//! These surfaces are tightly related because changing one often redirects
//! into another, especially while Plan mode is active.

use super::*;
use crate::bottom_pane::SelectionTab;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_4_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_5_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_PROVIDER_ID;
use codex_model_provider_info::AMBIENT_DEFAULT_MODEL;
use codex_model_provider_info::AMBIENT_KIMI_K2_7_CODE_MODEL;
use codex_model_provider_info::AMBIENT_PROVIDER_ID;
use codex_model_provider_info::ANTHROPIC_DEFAULT_MODEL;
use codex_model_provider_info::ANTHROPIC_PROVIDER_ID;
use codex_model_provider_info::BASETEN_DEFAULT_MODEL;
use codex_model_provider_info::BASETEN_PROVIDER_ID;
#[cfg(test)]
use codex_model_provider_info::CLAUDE_FABLE_PLAN_MODEL;
#[cfg(test)]
use codex_model_provider_info::CLAUDE_PLAN_MODEL;
use codex_model_provider_info::CLAUDE_PLAN_PROVIDER_ID;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::OPENROUTER_ANTHROPIC_PROVIDER_ID;
use codex_model_provider_info::OPENROUTER_DEFAULT_MODEL;
use codex_model_provider_info::OPENROUTER_PROVIDER_ID;
use codex_model_provider_info::VERCEL_ANTHROPIC_FAST_PROVIDER_ID;
use codex_model_provider_info::VERCEL_DEFAULT_MODEL;
use codex_model_provider_info::VERCEL_GLM_5_2_FAST_MODEL;
use codex_model_provider_info::VERCEL_PROVIDER_ID;
use codex_model_provider_info::ZAI_DEFAULT_MODEL;
use codex_model_provider_info::ZAI_PROVIDER_ID;
use codex_model_provider_info::is_claude_plan_model;
#[cfg(test)]
use codex_protocol::openai_models::ReasoningEffortPreset;
#[cfg(test)]
use codex_protocol::openai_models::default_input_modalities;

const OPENROUTER_MINIMAX_M3_MODEL: &str = "minimax/minimax-m3";
const OPENROUTER_OWL_ALPHA_MODEL: &str = "openrouter/owl-alpha";
const OPENROUTER_GEMINI_3_5_FLASH_MODEL: &str = "google/gemini-3.5-flash";
const OPENAI_GPT_5_5_MODEL: &str = "gpt-5.5";
const MODEL_PICKER_CODING_TAB_ID: &str = "coding-plans";
const MODEL_PICKER_API_KEY_TAB_ID: &str = "api-key-models";

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
            .map(|preset| preset.model.to_string())
            .unwrap_or_else(|| self.model_display_name().to_string());

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
                let description =
                    (!preset.description.is_empty()).then_some(preset.description.clone());
                let model = preset.model.clone();
                let should_prompt_plan_mode_scope = self.should_prompt_plan_mode_reasoning_scope(
                    model.as_str(),
                    Some(preset.default_reasoning_effort.clone()),
                );
                let actions = Self::model_selection_actions(
                    model.clone(),
                    Some(preset.default_reasoning_effort.clone()),
                    should_prompt_plan_mode_scope,
                );
                SelectionItem {
                    name: model.clone(),
                    description,
                    is_current: model.as_str() == current_model,
                    is_default: preset.is_default,
                    actions,
                    dismiss_on_select: true,
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
        let trimmed = model.trim();
        if trimmed == BASETEN_DEFAULT_MODEL {
            return Some(BASETEN_PROVIDER_ID.to_string());
        }
        if trimmed == AMBIENT_DEFAULT_MODEL
            || trimmed == AMBIENT_KIMI_K2_7_CODE_MODEL
            || trimmed.starts_with("ambient/")
            || trimmed.starts_with("zai-org/")
        {
            return Some(AMBIENT_PROVIDER_ID.to_string());
        }
        if trimmed == ZAI_DEFAULT_MODEL || trimmed.starts_with("glm-") {
            return Some(ZAI_PROVIDER_ID.to_string());
        }
        if is_claude_plan_model(trimmed) {
            return Some(CLAUDE_PLAN_PROVIDER_ID.to_string());
        }
        if trimmed == ANTHROPIC_DEFAULT_MODEL || trimmed.starts_with("claude-") {
            return Some(ANTHROPIC_PROVIDER_ID.to_string());
        }
        if trimmed == OPENROUTER_DEFAULT_MODEL {
            return Some(OPENROUTER_ANTHROPIC_PROVIDER_ID.to_string());
        }
        if Self::is_openrouter_model(trimmed) {
            return Some(OPENROUTER_PROVIDER_ID.to_string());
        }
        if trimmed == VERCEL_GLM_5_2_FAST_MODEL {
            return Some(VERCEL_ANTHROPIC_FAST_PROVIDER_ID.to_string());
        }
        if Self::is_vercel_model(trimmed) {
            return Some(VERCEL_PROVIDER_ID.to_string());
        }
        if matches!(
            trimmed,
            AMAZON_BEDROCK_GPT_5_5_MODEL_ID | AMAZON_BEDROCK_GPT_5_4_MODEL_ID
        ) {
            return Some(AMAZON_BEDROCK_PROVIDER_ID.to_string());
        }
        if trimmed.starts_with("gpt-") || trimmed.starts_with("codex-auto-") {
            return Some(OPENAI_PROVIDER_ID.to_string());
        }
        None
    }

    fn is_openrouter_model(model: &str) -> bool {
        matches!(
            model,
            OPENROUTER_DEFAULT_MODEL
                | OPENROUTER_MINIMAX_M3_MODEL
                | OPENROUTER_OWL_ALPHA_MODEL
                | OPENROUTER_GEMINI_3_5_FLASH_MODEL
        )
    }

    fn is_vercel_model(model: &str) -> bool {
        matches!(model, VERCEL_DEFAULT_MODEL | VERCEL_GLM_5_2_FAST_MODEL)
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
        let presets: Vec<ModelPreset> = presets
            .into_iter()
            .filter(Self::show_in_pfterminal_model_picker)
            .collect();

        if presets.is_empty() {
            self.add_info_message(
                "No additional models are available right now.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let mut coding_plan_items: Vec<SelectionItem> = Vec::new();
        let mut pay_per_api_call_items: Vec<SelectionItem> = Vec::new();
        for preset in presets.into_iter() {
            let provider = Self::model_provider_for_selection(&preset.model);
            let item = self.model_picker_item(preset);
            match provider.as_deref() {
                Some(AMBIENT_PROVIDER_ID | CLAUDE_PLAN_PROVIDER_ID | ZAI_PROVIDER_ID) => {
                    coding_plan_items.push(item)
                }
                Some(OPENAI_PROVIDER_ID) => coding_plan_items.push(item),
                Some(
                    ANTHROPIC_PROVIDER_ID
                    | BASETEN_PROVIDER_ID
                    | OPENROUTER_PROVIDER_ID
                    | OPENROUTER_ANTHROPIC_PROVIDER_ID
                    | VERCEL_PROVIDER_ID
                    | VERCEL_ANTHROPIC_FAST_PROVIDER_ID,
                ) => pay_per_api_call_items.push(item),
                _ => {}
            }
        }
        let (items, tabs, initial_tab_id, footer_hint) = if !coding_plan_items.is_empty()
            && !pay_per_api_call_items.is_empty()
        {
            let current_provider = Self::model_provider_for_selection(self.current_model());
            let initial_tab_id = if Self::is_api_key_model_provider(current_provider.as_deref()) {
                MODEL_PICKER_API_KEY_TAB_ID
            } else {
                MODEL_PICKER_CODING_TAB_ID
            };
            (
                Vec::new(),
                vec![
                    SelectionTab {
                        id: MODEL_PICKER_CODING_TAB_ID.to_string(),
                        label: "Coding Plans".to_string(),
                        header: self.model_menu_header(
                            "Select Model and Effort",
                            "OpenAI Codex, Ambient, and Z.AI plan-backed models",
                        ),
                        items: coding_plan_items,
                    },
                    SelectionTab {
                        id: MODEL_PICKER_API_KEY_TAB_ID.to_string(),
                        label: "API Key Models".to_string(),
                        header: self.model_menu_header(
                            "Select Model and Effort",
                            "Anthropic, OpenRouter, Baseten, and Vercel API-key models",
                        ),
                        items: pay_per_api_call_items,
                    },
                ],
                Some(initial_tab_id.to_string()),
                Some(Self::model_picker_tabbed_footer_hint_line()),
            )
        } else {
            let mut items = Vec::new();
            items.append(&mut coding_plan_items);
            items.append(&mut pay_per_api_call_items);
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

    fn model_picker_item(&self, preset: ModelPreset) -> SelectionItem {
        let description =
            (!preset.description.is_empty()).then_some(preset.description.to_string());
        let is_current = preset.model.as_str() == self.current_model();
        let direct_select = preset.supported_reasoning_efforts.len() <= 1;
        let preset_for_action = preset.clone();
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            let preset_for_event = preset_for_action.clone();
            tx.send(AppEvent::OpenReasoningPopup {
                model: preset_for_event,
            });
        })];
        SelectionItem {
            name: preset.model.clone(),
            description,
            is_current,
            is_default: preset.is_default,
            actions,
            dismiss_on_select: direct_select,
            dismiss_parent_on_child_accept: !direct_select,
            ..Default::default()
        }
    }

    fn model_picker_tabbed_footer_hint_line() -> Line<'static> {
        Line::from("Use Left/Right to switch groups. Press Enter to confirm or Esc to go back")
    }

    pub(crate) fn show_in_pfterminal_model_picker(preset: &ModelPreset) -> bool {
        if !preset.show_in_picker {
            return false;
        }

        let provider = Self::model_provider_for_selection(&preset.model);
        match provider.as_deref() {
            Some(OPENAI_PROVIDER_ID) => Self::is_openai_coding_plan_model(&preset.model),
            Some(
                AMBIENT_PROVIDER_ID
                | CLAUDE_PLAN_PROVIDER_ID
                | ANTHROPIC_PROVIDER_ID
                | ZAI_PROVIDER_ID
                | BASETEN_PROVIDER_ID
                | OPENROUTER_PROVIDER_ID
                | OPENROUTER_ANTHROPIC_PROVIDER_ID
                | VERCEL_PROVIDER_ID
                | VERCEL_ANTHROPIC_FAST_PROVIDER_ID,
            ) => true,
            _ => false,
        }
    }

    fn is_openai_coding_plan_model(model: &str) -> bool {
        model.trim() == OPENAI_GPT_5_5_MODEL
    }

    fn is_api_key_model_provider(provider: Option<&str>) -> bool {
        matches!(
            provider,
            Some(
                ANTHROPIC_PROVIDER_ID
                    | BASETEN_PROVIDER_ID
                    | OPENROUTER_PROVIDER_ID
                    | OPENROUTER_ANTHROPIC_PROVIDER_ID
                    | VERCEL_PROVIDER_ID
                    | VERCEL_ANTHROPIC_FAST_PROVIDER_ID
            )
        )
    }

    fn model_selection_actions(
        model_for_action: String,
        effort_for_action: Option<ReasoningEffortConfig>,
        should_prompt_plan_mode_scope: bool,
    ) -> Vec<SelectionAction> {
        let provider_for_action = Self::model_provider_for_selection(&model_for_action);
        vec![Box::new(move |tx| {
            if should_prompt_plan_mode_scope {
                tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                    model: model_for_action.clone(),
                    effort: effort_for_action.clone(),
                });
                return;
            }

            tx.send(AppEvent::UpdateModelSelection {
                model: model_for_action.clone(),
                provider: provider_for_action.clone(),
            });
            tx.send(AppEvent::UpdateReasoningEffort(effort_for_action.clone()));
            tx.send(AppEvent::PersistModelSelection {
                model: model_for_action.clone(),
                provider: provider_for_action.clone(),
                effort: effort_for_action.clone(),
            });
        })]
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

        let plan_only_actions: Vec<SelectionAction> = vec![Box::new({
            let model = model.clone();
            let effort = effort.clone();
            let provider = Self::model_provider_for_selection(&model);
            move |tx| {
                tx.send(AppEvent::UpdateModelSelection {
                    model: model.clone(),
                    provider: provider.clone(),
                });
                tx.send(AppEvent::UpdatePlanModeReasoningEffort(effort.clone()));
                tx.send(AppEvent::PersistPlanModeReasoningEffort(effort.clone()));
            }
        })];
        let provider = Self::model_provider_for_selection(&model);
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

    /// Open a popup to choose the reasoning effort (stage 2) for the given model.
    pub(crate) fn open_reasoning_popup(&mut self, preset: ModelPreset) {
        let default_effort = preset.default_reasoning_effort;
        let supported = preset.supported_reasoning_efforts;
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

        let mut choices: Vec<ReasoningEffortConfig> = supported
            .iter()
            .map(|option| option.effort.clone())
            .collect();
        if choices.is_empty() {
            choices.push(default_effort.clone());
        }

        if choices.len() == 1 {
            let selected_effort = choices.first().cloned();
            let selected_model = preset.model;
            if self
                .should_prompt_plan_mode_reasoning_scope(&selected_model, selected_effort.clone())
            {
                self.app_event_tx
                    .send(AppEvent::OpenPlanReasoningScopePrompt {
                        model: selected_model,
                        effort: selected_effort,
                    });
            } else {
                self.apply_model_and_effort(selected_model, selected_effort);
            }
            return;
        }

        let default_choice = choices
            .contains(&default_effort)
            .then(|| default_effort.clone());

        let model_slug = preset.model.to_string();
        let is_current_model = self.current_model() == preset.model.as_str();
        let highlight_choice = if is_current_model {
            if in_plan_mode {
                self.config
                    .plan_mode_reasoning_effort
                    .clone()
                    .or_else(|| self.effective_reasoning_effort())
            } else {
                self.effective_reasoning_effort()
            }
        } else {
            default_choice.clone()
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

            let model_for_action = model_slug.clone();
            let choice_effort = Some(effort);
            let provider_for_action = Self::model_provider_for_selection(&model_for_action);
            let should_prompt_plan_mode_scope = self.should_prompt_plan_mode_reasoning_scope(
                model_slug.as_str(),
                choice_effort.clone(),
            );
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                if should_prompt_plan_mode_scope {
                    tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                        model: model_for_action.clone(),
                        effort: choice_effort.clone(),
                    });
                } else {
                    tx.send(AppEvent::UpdateModelSelection {
                        model: model_for_action.clone(),
                        provider: provider_for_action.clone(),
                    });
                    tx.send(AppEvent::UpdateReasoningEffort(choice_effort.clone()));
                    tx.send(AppEvent::PersistModelSelection {
                        model: model_for_action.clone(),
                        provider: provider_for_action.clone(),
                        effort: choice_effort.clone(),
                    });
                }
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

        let mut header = ColumnRenderable::new();
        let header_title = if uses_ambient_reasoning_modes {
            "Select Reasoning Mode"
        } else {
            "Select Reasoning Level"
        };
        header.push(Line::from(
            format!("{header_title} for {model_slug}").bold(),
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

    pub(super) fn reasoning_effort_label(effort: &ReasoningEffortConfig) -> String {
        match effort {
            ReasoningEffortConfig::None => "None".to_string(),
            ReasoningEffortConfig::Minimal => "Minimal".to_string(),
            ReasoningEffortConfig::Low => "Low".to_string(),
            ReasoningEffortConfig::Medium => "Medium".to_string(),
            ReasoningEffortConfig::High => "High".to_string(),
            ReasoningEffortConfig::XHigh => "Extra high".to_string(),
            ReasoningEffortConfig::Custom(value) => value.clone(),
        }
    }

    pub(super) fn reasoning_effort_sentence_label(effort: &ReasoningEffortConfig) -> String {
        match effort {
            ReasoningEffortConfig::Custom(value) => value.clone(),
            effort => Self::reasoning_effort_label(effort).to_lowercase(),
        }
    }

    pub(super) fn apply_model_and_effort_without_persist(
        &self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) {
        let provider = Self::model_provider_for_selection(&model);
        self.app_event_tx
            .send(AppEvent::UpdateModelSelection { model, provider });
        self.app_event_tx
            .send(AppEvent::UpdateReasoningEffort(effort));
    }

    fn apply_model_and_effort(&self, model: String, effort: Option<ReasoningEffortConfig>) {
        self.apply_model_and_effort_without_persist(model.clone(), effort.clone());
        let provider = Self::model_provider_for_selection(&model);
        self.app_event_tx.send(AppEvent::PersistModelSelection {
            model,
            provider,
            effort,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_provider_for_selection_maps_cross_provider_models() {
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
            ChatWidget::model_provider_for_selection(ANTHROPIC_DEFAULT_MODEL).as_deref(),
            Some(ANTHROPIC_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(BASETEN_DEFAULT_MODEL).as_deref(),
            Some(BASETEN_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(OPENROUTER_DEFAULT_MODEL).as_deref(),
            Some(OPENROUTER_ANTHROPIC_PROVIDER_ID)
        );
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
    }

    #[test]
    fn model_provider_for_selection_maps_claude_plan_models() {
        assert_eq!(
            ChatWidget::model_provider_for_selection(CLAUDE_PLAN_MODEL).as_deref(),
            Some(CLAUDE_PLAN_PROVIDER_ID)
        );
        assert_eq!(
            ChatWidget::model_provider_for_selection(CLAUDE_FABLE_PLAN_MODEL).as_deref(),
            Some(CLAUDE_PLAN_PROVIDER_ID)
        );
    }

    #[test]
    fn pfterminal_picker_allows_only_gpt_5_5_for_openai() {
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
            CLAUDE_FABLE_PLAN_MODEL,
            true
        )));
        assert!(ChatWidget::show_in_pfterminal_model_picker(&preset(
            "gpt-5.5", true
        )));
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
            availability_nux: None,
            supported_in_api: true,
            input_modalities: default_input_modalities(),
        }
    }
}
