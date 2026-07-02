use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::InputModality;
use std::collections::HashSet;

use crate::util::error_or_panic;
use tracing::info;
use tracing::warn;

const IMAGE_CONTENT_OMITTED_PLACEHOLDER: &str =
    "image content omitted because you do not support image input";

pub(crate) fn ensure_call_outputs_present(items: &mut Vec<ResponseItem>) {
    let mut function_output_ids = HashSet::new();
    let mut tool_search_output_ids = HashSet::new();
    let mut custom_tool_output_ids = HashSet::new();
    for item in items.iter() {
        match item {
            ResponseItem::FunctionCallOutput { call_id, .. } => {
                function_output_ids.insert(call_id.as_str());
            }
            ResponseItem::ToolSearchOutput {
                call_id: Some(call_id),
                ..
            } => {
                tool_search_output_ids.insert(call_id.as_str());
            }
            ResponseItem::CustomToolCallOutput { call_id, .. } => {
                custom_tool_output_ids.insert(call_id.as_str());
            }
            _ => {}
        }
    }

    // Collect synthetic outputs to insert immediately after their calls.
    // Store the insertion position (index of call) alongside the item so
    // we can insert in reverse order and avoid index shifting.
    let mut missing_outputs_to_insert: Vec<(usize, ResponseItem)> = Vec::new();
    let mut missing_function_outputs = 0usize;
    let mut missing_function_sample: Option<String> = None;
    let mut missing_tool_search_outputs = 0usize;
    let mut missing_tool_search_sample: Option<String> = None;

    for (idx, item) in items.iter().enumerate() {
        match item {
            ResponseItem::FunctionCall { call_id, .. }
                if !function_output_ids.contains(call_id.as_str()) =>
            {
                missing_function_outputs += 1;
                if missing_function_sample.is_none() {
                    missing_function_sample = Some(call_id.clone());
                }
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItem::FunctionCallOutput {
                        id: None,
                        call_id: call_id.clone(),
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        metadata: None,
                    },
                ));
            }
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                ..
            } if !tool_search_output_ids.contains(call_id.as_str()) => {
                missing_tool_search_outputs += 1;
                if missing_tool_search_sample.is_none() {
                    missing_tool_search_sample = Some(call_id.clone());
                }
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItem::ToolSearchOutput {
                        id: None,
                        call_id: Some(call_id.clone()),
                        status: "completed".to_string(),
                        execution: "client".to_string(),
                        tools: Vec::new(),
                        metadata: None,
                    },
                ));
            }
            ResponseItem::CustomToolCall { call_id, .. }
                if !custom_tool_output_ids.contains(call_id.as_str()) =>
            {
                error_or_panic(format!(
                    "Custom tool call output is missing for call id: {call_id}"
                ));
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItem::CustomToolCallOutput {
                        id: None,
                        call_id: call_id.clone(),
                        name: None,
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        metadata: None,
                    },
                ));
            }
            // LocalShellCall is represented in upstream streams by a FunctionCallOutput
            ResponseItem::LocalShellCall {
                call_id: Some(call_id),
                ..
            } if !function_output_ids.contains(call_id.as_str()) => {
                error_or_panic(format!(
                    "Local shell call output is missing for call id: {call_id}"
                ));
                missing_outputs_to_insert.push((
                    idx,
                    ResponseItem::FunctionCallOutput {
                        id: None,
                        call_id: call_id.clone(),
                        output: FunctionCallOutputPayload::from_text("aborted".to_string()),
                        metadata: None,
                    },
                ));
            }
            _ => {}
        }
    }
    drop((
        function_output_ids,
        tool_search_output_ids,
        custom_tool_output_ids,
    ));

    // Insert synthetic outputs in reverse index order to avoid re-indexing.
    for (idx, output_item) in missing_outputs_to_insert.into_iter().rev() {
        items.insert(idx + 1, output_item);
    }

    if missing_function_outputs > 0 {
        info!(
            count = missing_function_outputs,
            sample_call_id = missing_function_sample.as_deref(),
            "function call outputs are missing; inserted aborted outputs"
        );
    }
    if missing_tool_search_outputs > 0 {
        info!(
            count = missing_tool_search_outputs,
            sample_call_id = missing_tool_search_sample.as_deref(),
            "tool search outputs are missing; inserted completed outputs"
        );
    }
}

pub(crate) fn remove_orphan_outputs(items: &mut Vec<ResponseItem>) {
    let function_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            ResponseItem::FunctionCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let tool_search_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                ..
            } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let local_shell_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            ResponseItem::LocalShellCall {
                call_id: Some(call_id),
                ..
            } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let custom_tool_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match i {
            ResponseItem::CustomToolCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let mut orphan_function_outputs = 0usize;
    let mut orphan_function_empty_call_ids = 0usize;
    let mut orphan_function_sample: Option<String> = None;
    let mut orphan_custom_tool_outputs = 0usize;
    let mut orphan_custom_tool_sample: Option<String> = None;
    let mut orphan_tool_search_outputs = 0usize;
    let mut orphan_tool_search_sample: Option<String> = None;

    items.retain(|item| match item {
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            let has_match =
                function_call_ids.contains(call_id) || local_shell_call_ids.contains(call_id);
            if !has_match {
                orphan_function_outputs += 1;
                if call_id.trim().is_empty() {
                    orphan_function_empty_call_ids += 1;
                } else if orphan_function_sample.is_none() {
                    orphan_function_sample = Some(call_id.clone());
                }
            }
            has_match
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            let has_match = custom_tool_call_ids.contains(call_id);
            if !has_match {
                orphan_custom_tool_outputs += 1;
                if orphan_custom_tool_sample.is_none() {
                    orphan_custom_tool_sample = Some(call_id.clone());
                }
            }
            has_match
        }
        ResponseItem::ToolSearchOutput { execution, .. } if execution == "server" => true,
        ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            ..
        } => {
            let has_match = tool_search_call_ids.contains(call_id);
            if !has_match {
                orphan_tool_search_outputs += 1;
                if orphan_tool_search_sample.is_none() {
                    orphan_tool_search_sample = Some(call_id.clone());
                }
            }
            has_match
        }
        ResponseItem::ToolSearchOutput { call_id: None, .. } => true,
        _ => true,
    });

    if orphan_function_outputs > 0 {
        warn!(
            count = orphan_function_outputs,
            empty_call_ids = orphan_function_empty_call_ids,
            sample_call_id = orphan_function_sample.as_deref(),
            "orphan function call outputs removed"
        );
    }
    if orphan_custom_tool_outputs > 0 {
        warn!(
            count = orphan_custom_tool_outputs,
            sample_call_id = orphan_custom_tool_sample.as_deref(),
            "orphan custom tool call outputs removed"
        );
    }
    if orphan_tool_search_outputs > 0 {
        warn!(
            count = orphan_tool_search_outputs,
            sample_call_id = orphan_tool_search_sample.as_deref(),
            "orphan tool search outputs removed"
        );
    }
}

pub(crate) fn remove_corresponding_for(items: &mut Vec<ResponseItem>, item: &ResponseItem) {
    match item {
        ResponseItem::FunctionCall { call_id, .. } => {
            remove_first_matching(items, |i| {
                matches!(
                    i,
                    ResponseItem::FunctionCallOutput {
                        call_id: existing, ..
                    } if existing == call_id
                )
            });
        }
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            if let Some(pos) = items.iter().position(|i| {
                matches!(i, ResponseItem::FunctionCall { call_id: existing, .. } if existing == call_id)
            }) {
                items.remove(pos);
            } else if let Some(pos) = items.iter().position(|i| {
                matches!(i, ResponseItem::LocalShellCall { call_id: Some(existing), .. } if existing == call_id)
            }) {
                items.remove(pos);
            }
        }
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            ..
        } => {
            remove_first_matching(items, |i| {
                matches!(
                    i,
                    ResponseItem::ToolSearchOutput {
                        call_id: Some(existing),
                        ..
                    } if existing == call_id
                )
            });
        }
        ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            ..
        } => {
            remove_first_matching(
                items,
                |i| {
                    matches!(
                        i,
                        ResponseItem::ToolSearchCall {
                            call_id: Some(existing),
                            ..
                        } if existing == call_id
                    )
                },
            );
        }
        ResponseItem::CustomToolCall { call_id, .. } => {
            remove_first_matching(items, |i| {
                matches!(
                    i,
                    ResponseItem::CustomToolCallOutput {
                        call_id: existing, ..
                    } if existing == call_id
                )
            });
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            remove_first_matching(
                items,
                |i| matches!(i, ResponseItem::CustomToolCall { call_id: existing, .. } if existing == call_id),
            );
        }
        ResponseItem::LocalShellCall {
            call_id: Some(call_id),
            ..
        } => {
            remove_first_matching(items, |i| {
                matches!(
                    i,
                    ResponseItem::FunctionCallOutput {
                        call_id: existing, ..
                    } if existing == call_id
                )
            });
        }
        _ => {}
    }
}

fn remove_first_matching<F>(items: &mut Vec<ResponseItem>, predicate: F)
where
    F: Fn(&ResponseItem) -> bool,
{
    if let Some(pos) = items.iter().position(predicate) {
        items.remove(pos);
    }
}

/// Strip image content from messages and tool outputs when the model does not support images.
/// When `input_modalities` contains `InputModality::Image`, no stripping is performed.
pub(crate) fn strip_images_when_unsupported(
    input_modalities: &[InputModality],
    items: &mut [ResponseItem],
) {
    let supports_images = input_modalities.contains(&InputModality::Image);
    if supports_images {
        return;
    }

    for item in items.iter_mut() {
        match item {
            ResponseItem::Message { content, .. } => {
                let mut normalized_content = Vec::with_capacity(content.len());
                for content_item in content.iter() {
                    match content_item {
                        ContentItem::InputImage { .. } => {
                            normalized_content.push(ContentItem::InputText {
                                text: IMAGE_CONTENT_OMITTED_PLACEHOLDER.to_string(),
                            });
                        }
                        _ => normalized_content.push(content_item.clone()),
                    }
                }
                *content = normalized_content;
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let Some(content_items) = output.content_items_mut() {
                    let mut normalized_content_items = Vec::with_capacity(content_items.len());
                    for content_item in content_items.iter() {
                        match content_item {
                            FunctionCallOutputContentItem::InputImage { .. } => {
                                normalized_content_items.push(
                                    FunctionCallOutputContentItem::InputText {
                                        text: IMAGE_CONTENT_OMITTED_PLACEHOLDER.to_string(),
                                    },
                                );
                            }
                            _ => normalized_content_items.push(content_item.clone()),
                        }
                    }
                    *content_items = normalized_content_items;
                }
            }
            ResponseItem::ImageGenerationCall { result, .. } => {
                result.clear();
            }
            _ => {}
        }
    }
}
