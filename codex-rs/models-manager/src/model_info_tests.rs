use super::*;
use crate::ModelsManagerConfig;
use codex_protocol::config_types::Personality;
use pretty_assertions::assert_eq;

#[test]
fn reasoning_summaries_override_true_enables_support() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(true),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.supports_reasoning_summaries = true;

    assert_eq!(updated, expected);
}

#[test]
fn glm_model_slug_gets_local_personality_messages() {
    let model = model_info_from_slug("glm-5.2");

    assert!(model.supports_personality());
    assert!(
        model
            .get_model_instructions(Some(Personality::Pragmatic))
            .contains(LOCAL_PRAGMATIC_TEMPLATE)
    );
    assert!(
        !model
            .get_model_instructions(Some(Personality::Pragmatic))
            .contains("based on GPT-5")
    );
}

#[test]
fn config_overrides_fill_missing_personality_for_remote_glm_models() {
    let mut model = model_info_from_slug("remote-placeholder");
    model.slug = "z-ai/glm-5.2".to_string();
    model.model_messages = None;

    let updated = with_config_overrides(
        model,
        &ModelsManagerConfig {
            personality_enabled: true,
            ..Default::default()
        },
    );

    assert!(updated.supports_personality());
    assert!(
        updated
            .get_model_instructions(Some(Personality::Pragmatic))
            .contains(LOCAL_PRAGMATIC_TEMPLATE)
    );
}

#[test]
fn reasoning_summaries_override_false_does_not_disable_support() {
    let mut model = model_info_from_slug("unknown-model");
    model.supports_reasoning_summaries = true;
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn reasoning_summaries_override_false_is_noop_when_model_is_false() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn model_context_window_override_clamps_to_max_context_window() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig {
        model_context_window: Some(500_000),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.context_window = Some(400_000);

    assert_eq!(updated, expected);
}

#[test]
fn model_context_window_uses_model_value_without_override() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig::default();

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}
