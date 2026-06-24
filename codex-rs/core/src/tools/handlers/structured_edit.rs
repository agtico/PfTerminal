use std::collections::BTreeMap;
use std::path::Component;
use std::path::Path;

use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnContext;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::apply_patch::intercept_apply_patch;
use crate::tools::handlers::parse_arguments_for_tool;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::json;

pub(crate) const STRUCTURED_EDIT_TOOL_NAME: &str = "structured_edit";
pub(crate) const STRUCTURED_WRITE_TOOL_NAME: &str = "structured_write";

const MODEL_EDIT_COMPATIBILITY_METRIC: &str = "codex.model_edit_compatibility";
const MAX_STRUCTURED_WRITE_BYTES: usize = 256 * 1024;
const MAX_STRUCTURED_EDIT_FILE_BYTES: usize = 512 * 1024;

pub(crate) fn emit_model_edit_compat_metric(
    turn_context: &TurnContext,
    protocol: &'static str,
    outcome: &'static str,
    reason: &'static str,
) {
    let profile = model_edit_profile_tag(turn_context);
    turn_context.session_telemetry.counter(
        MODEL_EDIT_COMPATIBILITY_METRIC,
        /*inc*/ 1,
        &[
            ("profile", profile),
            ("protocol", protocol),
            ("outcome", outcome),
            ("reason", reason),
        ],
    );
    tracing::info!(
        target: "codex::model_edit_compatibility",
        profile,
        protocol,
        outcome,
        reason,
        "model edit compatibility event"
    );
}

fn model_edit_profile_tag(turn_context: &TurnContext) -> &'static str {
    let provider = turn_context.provider.info();
    if provider.is_zai() {
        return "zai";
    }
    if provider.is_ambient() {
        return "ambient";
    }

    let slug = turn_context.model_info.slug.to_ascii_lowercase();
    if slug.contains("glm") {
        return "glm";
    }
    if slug.contains("zai-org") {
        return "zai_org";
    }
    if turn_context.model_info.apply_patch_tool_type.is_some() {
        return "codex_patch";
    }
    "other"
}

pub(crate) fn structured_edit_protocol_enabled(turn_context: &TurnContext) -> bool {
    if turn_context.structured_edit_fallback_enabled() {
        return true;
    }

    let provider = turn_context.provider.info();
    if provider.is_zai() || provider.is_ambient() {
        return true;
    }

    let slug = turn_context.model_info.slug.to_ascii_lowercase();
    slug.contains("glm") || slug.contains("zai-org")
}

pub(crate) fn reject_source_write_heredoc_when_structured_edit_enabled(
    turn_context: &TurnContext,
    command: &str,
) -> Result<(), FunctionCallError> {
    if structured_edit_protocol_enabled(turn_context) && is_python_heredoc_source_write(command) {
        emit_model_edit_compat_metric(
            turn_context,
            "shell_heredoc",
            "rejected",
            "python_source_write",
        );
        return Err(FunctionCallError::RespondToModel(
            "source file edits via Python heredoc are disabled for this model profile; use structured_edit for existing files or structured_write for new/full-file writes"
                .to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredEditArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    environment_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StructuredWriteMode {
    CreateOnly,
    Overwrite,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredWriteArgs {
    path: String,
    content: String,
    mode: StructuredWriteMode,
    #[serde(default)]
    environment_id: Option<String>,
}

pub(crate) struct StructuredEditHandler {
    multi_environment: bool,
}

impl StructuredEditHandler {
    pub(crate) fn new(multi_environment: bool) -> Self {
        Self { multi_environment }
    }
}

pub(crate) struct StructuredWriteHandler {
    multi_environment: bool,
}

impl StructuredWriteHandler {
    pub(crate) fn new(multi_environment: bool) -> Self {
        Self { multi_environment }
    }
}

pub(crate) fn validate_structured_write_arguments(
    arguments: &str,
) -> Result<(), FunctionCallError> {
    let _: StructuredWriteArgs = parse_arguments_for_tool(STRUCTURED_WRITE_TOOL_NAME, arguments)?;
    Ok(())
}

impl ToolExecutor<ToolInvocation> for StructuredEditHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(STRUCTURED_EDIT_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_structured_edit_tool(self.multi_environment)
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(handle_structured_edit(invocation, self.multi_environment))
    }
}

impl CoreToolRuntime for StructuredEditHandler {}

impl ToolExecutor<ToolInvocation> for StructuredWriteHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(STRUCTURED_WRITE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_structured_write_tool(self.multi_environment)
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(handle_structured_write(invocation, self.multi_environment))
    }
}

impl CoreToolRuntime for StructuredWriteHandler {}

fn create_structured_edit_tool(multi_environment: bool) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Relative path of the text file to edit. Absolute paths and `..` segments are rejected."
                    .to_string(),
            )),
        ),
        (
            "old_string".to_string(),
            JsonSchema::string(Some(
                "Non-empty exact current text to replace. Must match the file byte-for-byte as UTF-8 text. This is not a read or inspection field; during read-only review, inspect files with shell commands such as `rg` and `sed -n` instead."
                    .to_string(),
            )),
        ),
        (
            "new_string".to_string(),
            JsonSchema::string(Some("Replacement text.".to_string())),
        ),
        (
            "replace_all".to_string(),
            JsonSchema::boolean(Some(
                "When true, replace every exact occurrence. When false, exactly one occurrence is required."
                    .to_string(),
            )),
        ),
    ]);
    let mut required = vec![
        "path".to_string(),
        "old_string".to_string(),
        "new_string".to_string(),
    ];
    if multi_environment {
        properties.insert(
            "environment_id".to_string(),
            JsonSchema::string(Some(
                "Target turn environment id when multiple environments are available.".to_string(),
            )),
        );
        required.push("environment_id".to_string());
    }

    ToolSpec::Function(ResponsesApiTool {
        name: STRUCTURED_EDIT_TOOL_NAME.to_string(),
        description: "Modify an existing UTF-8 text file by replacing exact text. This is an edit tool only, not a read or inspection tool; during read-only review, use shell commands such as `rg` and `sed -n` and do not call this tool. Use this instead of apply_patch when the model is not reliable at Codex patch grammar. The edit is converted to the same internal apply_patch runtime for diff preview, approvals, sandboxing, and tool events."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(required), Some(false.into())),
        output_schema: None,
    })
}

fn create_structured_write_tool(multi_environment: bool) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Relative path of the UTF-8 text file to write. Absolute paths and `..` segments are rejected."
                    .to_string(),
            )),
        ),
        (
            "content".to_string(),
            JsonSchema::string(Some(
                "Complete intended file contents. This is not a read or inspection field."
                    .to_string(),
            )),
        ),
        (
            "mode".to_string(),
            JsonSchema::string_enum(
                vec![json!("create_only"), json!("overwrite")],
                Some("Use create_only for new files and overwrite for intentional full-file replacement.".to_string()),
            ),
        ),
    ]);
    let mut required = vec![
        "path".to_string(),
        "content".to_string(),
        "mode".to_string(),
    ];
    if multi_environment {
        properties.insert(
            "environment_id".to_string(),
            JsonSchema::string(Some(
                "Target turn environment id when multiple environments are available.".to_string(),
            )),
        );
        required.push("environment_id".to_string());
    }

    ToolSpec::Function(ResponsesApiTool {
        name: STRUCTURED_WRITE_TOOL_NAME.to_string(),
        description: "Create or intentionally overwrite a UTF-8 text file. This is a write tool only, not a read or inspection tool; do not call it during read-only review. Prefer structured_edit for changes to existing files. Writes are converted to the same internal apply_patch runtime for diff preview, approvals, sandboxing, and tool events."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(required), Some(false.into())),
        output_schema: None,
    })
}

async fn handle_structured_edit(
    invocation: ToolInvocation,
    multi_environment: bool,
) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
    let ToolInvocation {
        session,
        turn,
        tracker,
        call_id,
        payload,
        ..
    } = invocation;
    let ToolPayload::Function { arguments } = payload else {
        return Err(FunctionCallError::RespondToModel(format!(
            "{STRUCTURED_EDIT_TOOL_NAME} handler received unsupported payload"
        )));
    };
    let args: StructuredEditArgs = parse_arguments_for_tool(STRUCTURED_EDIT_TOOL_NAME, &arguments)?;
    validate_relative_path(&args.path)?;
    validate_environment_id(args.environment_id.as_deref(), multi_environment)?;
    if args.old_string.is_empty() {
        emit_model_edit_compat_metric(
            &turn,
            STRUCTURED_EDIT_TOOL_NAME,
            "failure",
            "empty_old_string",
        );
        return Err(FunctionCallError::RespondToModel(
            "old_string cannot be empty; structured_edit is not a read tool. Use shell commands such as `rg` and `sed -n` for inspection, or use structured_write for intentional full-file writes"
                .to_string(),
        ));
    }
    if args.old_string == args.new_string {
        emit_model_edit_compat_metric(
            &turn,
            STRUCTURED_EDIT_TOOL_NAME,
            "failure",
            "identical_edit",
        );
        return Err(FunctionCallError::RespondToModel(
            "old_string and new_string are identical; no edit was applied".to_string(),
        ));
    }

    let Some(turn_environment) =
        resolve_tool_environment(turn.as_ref(), args.environment_id.as_deref())?.cloned()
    else {
        return Err(FunctionCallError::RespondToModel(
            "structured_edit is unavailable in this session".to_string(),
        ));
    };
    let fs = turn_environment.environment.get_filesystem();
    let sandbox = turn
        .file_system_sandbox_context(/*additional_permissions*/ None, turn_environment.cwd());
    let path_uri = turn_environment
        .cwd()
        .join(&args.path)
        .map_err(|err| FunctionCallError::RespondToModel(format!("invalid path: {err}")))?;
    let current = fs
        .read_file_text(&path_uri, Some(&sandbox))
        .await
        .map_err(|err| {
            emit_model_edit_compat_metric(
                &turn,
                STRUCTURED_EDIT_TOOL_NAME,
                "failure",
                "read_failed",
            );
            FunctionCallError::RespondToModel(format!(
                "failed to read {} as UTF-8 text: {err}",
                args.path
            ))
        })?;
    if current.len() > MAX_STRUCTURED_EDIT_FILE_BYTES {
        emit_model_edit_compat_metric(
            &turn,
            STRUCTURED_EDIT_TOOL_NAME,
            "failure",
            "file_too_large",
        );
        return Err(FunctionCallError::RespondToModel(format!(
            "{} is {} bytes; structured_edit limit is {MAX_STRUCTURED_EDIT_FILE_BYTES} bytes",
            args.path,
            current.len()
        )));
    }
    let next = match replace_structured(
        &current,
        &args.old_string,
        &args.new_string,
        args.replace_all,
    ) {
        Ok(next) => next,
        Err(err) => {
            emit_model_edit_compat_metric(
                &turn,
                STRUCTURED_EDIT_TOOL_NAME,
                "failure",
                "replace_failed",
            );
            return Err(err);
        }
    };
    if next.len() > MAX_STRUCTURED_EDIT_FILE_BYTES {
        emit_model_edit_compat_metric(
            &turn,
            STRUCTURED_EDIT_TOOL_NAME,
            "failure",
            "result_too_large",
        );
        return Err(FunctionCallError::RespondToModel(format!(
            "{} would become {} bytes; structured_edit limit is {MAX_STRUCTURED_EDIT_FILE_BYTES} bytes",
            args.path,
            next.len()
        )));
    }
    let patch = build_update_patch(&args.path, &current, &next);
    let result = run_generated_patch(GeneratedPatchInvocation {
        session,
        turn: turn.clone(),
        tracker,
        call_id,
        tool_name: STRUCTURED_EDIT_TOOL_NAME,
        patch,
        turn_environment,
    })
    .await;
    emit_model_edit_compat_metric(
        &turn,
        STRUCTURED_EDIT_TOOL_NAME,
        if result.is_ok() { "success" } else { "failure" },
        if result.is_ok() {
            "applied"
        } else {
            "generated_patch_failed"
        },
    );
    result
}

async fn handle_structured_write(
    invocation: ToolInvocation,
    multi_environment: bool,
) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
    let ToolInvocation {
        session,
        turn,
        tracker,
        call_id,
        payload,
        ..
    } = invocation;
    let ToolPayload::Function { arguments } = payload else {
        return Err(FunctionCallError::RespondToModel(format!(
            "{STRUCTURED_WRITE_TOOL_NAME} handler received unsupported payload"
        )));
    };
    let args: StructuredWriteArgs =
        parse_arguments_for_tool(STRUCTURED_WRITE_TOOL_NAME, &arguments)?;
    validate_relative_path(&args.path)?;
    validate_environment_id(args.environment_id.as_deref(), multi_environment)?;
    if args.content.len() > MAX_STRUCTURED_WRITE_BYTES {
        emit_model_edit_compat_metric(
            &turn,
            STRUCTURED_WRITE_TOOL_NAME,
            "failure",
            "content_too_large",
        );
        return Err(FunctionCallError::RespondToModel(format!(
            "structured_write content is {} bytes; limit is {MAX_STRUCTURED_WRITE_BYTES} bytes",
            args.content.len()
        )));
    }

    let Some(turn_environment) =
        resolve_tool_environment(turn.as_ref(), args.environment_id.as_deref())?.cloned()
    else {
        return Err(FunctionCallError::RespondToModel(
            "structured_write is unavailable in this session".to_string(),
        ));
    };
    let fs = turn_environment.environment.get_filesystem();
    let sandbox = turn
        .file_system_sandbox_context(/*additional_permissions*/ None, turn_environment.cwd());
    let path_uri = turn_environment
        .cwd()
        .join(&args.path)
        .map_err(|err| FunctionCallError::RespondToModel(format!("invalid path: {err}")))?;
    let current = fs.read_file_text(&path_uri, Some(&sandbox)).await;
    let patch = match (args.mode, current) {
        (StructuredWriteMode::CreateOnly, Ok(_)) => {
            emit_model_edit_compat_metric(
                &turn,
                STRUCTURED_WRITE_TOOL_NAME,
                "failure",
                "create_exists",
            );
            return Err(FunctionCallError::RespondToModel(format!(
                "{} already exists; use structured_edit or mode=overwrite for intentional replacement",
                args.path
            )));
        }
        (StructuredWriteMode::CreateOnly, Err(err))
            if err.kind() == std::io::ErrorKind::NotFound =>
        {
            build_add_patch(&args.path, &args.content)
        }
        (StructuredWriteMode::CreateOnly, Err(err)) => {
            emit_model_edit_compat_metric(
                &turn,
                STRUCTURED_WRITE_TOOL_NAME,
                "failure",
                "check_failed",
            );
            return Err(FunctionCallError::RespondToModel(format!(
                "failed to check {}: {err}",
                args.path
            )));
        }
        (StructuredWriteMode::Overwrite, Ok(current)) => {
            if current == args.content {
                emit_model_edit_compat_metric(
                    &turn,
                    STRUCTURED_WRITE_TOOL_NAME,
                    "failure",
                    "content_unchanged",
                );
                return Err(FunctionCallError::RespondToModel(
                    "content matches the existing file; no write was applied".to_string(),
                ));
            }
            build_update_patch(&args.path, &current, &args.content)
        }
        (StructuredWriteMode::Overwrite, Err(err))
            if err.kind() == std::io::ErrorKind::NotFound =>
        {
            build_add_patch(&args.path, &args.content)
        }
        (StructuredWriteMode::Overwrite, Err(err)) => {
            emit_model_edit_compat_metric(
                &turn,
                STRUCTURED_WRITE_TOOL_NAME,
                "failure",
                "read_failed",
            );
            return Err(FunctionCallError::RespondToModel(format!(
                "failed to read {} as UTF-8 text: {err}",
                args.path
            )));
        }
    };
    let result = run_generated_patch(GeneratedPatchInvocation {
        session,
        turn: turn.clone(),
        tracker,
        call_id,
        tool_name: STRUCTURED_WRITE_TOOL_NAME,
        patch,
        turn_environment,
    })
    .await;
    emit_model_edit_compat_metric(
        &turn,
        STRUCTURED_WRITE_TOOL_NAME,
        if result.is_ok() { "success" } else { "failure" },
        if result.is_ok() {
            "applied"
        } else {
            "generated_patch_failed"
        },
    );
    result
}

struct GeneratedPatchInvocation<'a> {
    session: std::sync::Arc<crate::session::session::Session>,
    turn: std::sync::Arc<crate::session::turn_context::TurnContext>,
    tracker: crate::tools::context::SharedTurnDiffTracker,
    call_id: String,
    tool_name: &'a str,
    patch: String,
    turn_environment: crate::session::turn_context::TurnEnvironment,
}

async fn run_generated_patch(
    invocation: GeneratedPatchInvocation<'_>,
) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
    let command = vec!["apply_patch".to_string(), invocation.patch];
    let cwd = invocation.turn_environment.cwd().clone();
    let fs = invocation.turn_environment.environment.get_filesystem();
    let output = Box::pin(intercept_apply_patch(
        &command,
        &cwd,
        fs.as_ref(),
        invocation.turn_environment,
        invocation.session,
        invocation.turn,
        Some(&invocation.tracker),
        &invocation.call_id,
        invocation.tool_name,
    ))
    .await?;
    match output {
        Some(output) => Ok(boxed_tool_output(output)),
        None => Err(FunctionCallError::RespondToModel(
            "generated structured edit patch was not recognized".to_string(),
        )),
    }
}

fn validate_environment_id(
    environment_id: Option<&str>,
    allow_environment_id: bool,
) -> Result<(), FunctionCallError> {
    if environment_id.is_some() && !allow_environment_id {
        return Err(FunctionCallError::RespondToModel(
            "environment_id is unavailable for this turn".to_string(),
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), FunctionCallError> {
    if path.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "path cannot be empty".to_string(),
        ));
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(FunctionCallError::RespondToModel(
            "path must be relative, not absolute".to_string(),
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(FunctionCallError::RespondToModel(
            "path cannot contain `..`, root, or drive-prefix components".to_string(),
        ));
    }
    Ok(())
}

fn replace_exact(
    current: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, FunctionCallError> {
    let matches = current.matches(old_string).count();
    match (matches, replace_all) {
        (0, _) => Err(FunctionCallError::RespondToModel(
            "old_string was not found in the file; read the current file and retry with exact text"
                .to_string(),
        )),
        (1, false) => Ok(current.replacen(old_string, new_string, 1)),
        (_, true) => Ok(current.replace(old_string, new_string)),
        (count, false) => Err(FunctionCallError::RespondToModel(format!(
            "old_string matched {count} locations; provide more surrounding context or set replace_all=true"
        ))),
    }
}

fn replace_structured(
    current: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, FunctionCallError> {
    if current.matches(old_string).next().is_some() {
        return replace_exact(current, old_string, new_string, replace_all);
    }

    let line_ending = detect_line_ending(current);
    let normalized_old = convert_to_line_ending(normalize_line_endings(old_string), line_ending);
    let normalized_new = convert_to_line_ending(normalize_line_endings(new_string), line_ending);

    if normalized_old != old_string || normalized_new != new_string {
        return replace_exact(current, &normalized_old, &normalized_new, replace_all);
    }

    replace_exact(current, old_string, new_string, replace_all)
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn convert_to_line_ending(text: String, line_ending: &str) -> String {
    if line_ending == "\n" {
        text
    } else {
        text.replace('\n', line_ending)
    }
}

fn detect_line_ending(text: &str) -> &str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

fn is_python_heredoc_source_write(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    if !(lower.contains("python") && lower.contains("<<")) {
        return false;
    }
    if !looks_like_python_file_write(&lower) {
        return false;
    }
    contains_source_like_path(&lower)
}

fn looks_like_python_file_write(lower: &str) -> bool {
    lower.contains(".write_text(")
        || lower.contains(".write_bytes(")
        || lower.contains(".write(")
        || (lower.contains("open(")
            && (lower.contains(", 'w'")
                || lower.contains(", \"w\"")
                || lower.contains(",'w'")
                || lower.contains(",\"w\"")
                || lower.contains(", 'a'")
                || lower.contains(", \"a\"")
                || lower.contains(",'a'")
                || lower.contains(",\"a\"")))
}

fn contains_source_like_path(lower: &str) -> bool {
    const SOURCE_EXTENSIONS: &[&str] = &[
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".go", ".java", ".kt",
        ".swift", ".c", ".h", ".cc", ".cpp", ".hpp", ".cs", ".rb", ".php", ".sh", ".bash", ".zsh",
        ".fish", ".toml", ".json", ".yaml", ".yml", ".md",
    ];
    SOURCE_EXTENSIONS
        .iter()
        .any(|extension| lower.contains(extension))
}

fn build_add_patch(path: &str, content: &str) -> String {
    let mut patch = String::from("*** Begin Patch\n");
    patch.push_str("*** Add File: ");
    patch.push_str(path);
    patch.push('\n');
    append_prefixed_lines(&mut patch, '+', content);
    patch.push_str("*** End Patch");
    patch
}

fn build_update_patch(path: &str, current: &str, next: &str) -> String {
    let mut patch = String::from("*** Begin Patch\n");
    patch.push_str("*** Update File: ");
    patch.push_str(path);
    patch.push_str("\n@@\n");
    append_prefixed_lines(&mut patch, '-', current);
    append_prefixed_lines(&mut patch, '+', next);
    patch.push_str("*** End Patch");
    patch
}

fn append_prefixed_lines(output: &mut String, prefix: char, text: &str) {
    for line in patch_lines(text) {
        output.push(prefix);
        output.push_str(line);
        output.push('\n');
    }
}

fn patch_lines(text: &str) -> Vec<&str> {
    let mut lines = text.split('\n').collect::<Vec<_>>();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn assert_model_error<T>(result: Result<T, FunctionCallError>, expected: &str) {
        match result {
            Err(FunctionCallError::RespondToModel(message)) => {
                assert!(
                    message.contains(expected),
                    "expected error containing {expected:?}, got {message:?}"
                );
            }
            Ok(_) => panic!("expected model error containing {expected:?}"),
            Err(error) => panic!("expected RespondToModel, got {error:?}"),
        }
    }

    #[test]
    fn replace_exact_requires_unique_match_by_default() {
        assert_eq!(
            replace_exact("a b a", "b", "B", false).expect("unique replacement"),
            "a B a"
        );
        assert_model_error(
            replace_exact("a b a", "a", "A", false),
            "matched 2 locations",
        );
        assert_model_error(replace_exact("a b a", "missing", "x", false), "not found");
    }

    #[test]
    fn replace_exact_allows_explicit_replace_all() {
        assert_eq!(
            replace_exact("a b a", "a", "A", true).expect("replace all"),
            "A b A"
        );
    }

    #[test]
    fn replace_structured_normalizes_line_endings_to_current_file() {
        assert_eq!(
            replace_structured("alpha\r\nbeta\r\ngamma\r\n", "beta\n", "BETA\n", false)
                .expect("line-ending normalized replacement"),
            "alpha\r\nBETA\r\ngamma\r\n"
        );
    }

    #[test]
    fn detects_python_heredoc_source_writes() {
        let command =
            "python3 - <<'PY'\nfrom pathlib import Path\nPath('src/lib.rs').write_text('x')\nPY";
        assert!(is_python_heredoc_source_write(command));
        assert!(!is_python_heredoc_source_write(
            "python3 - <<'PY'\nprint('src/lib.rs')\nPY"
        ));
        assert!(!is_python_heredoc_source_write("python3 scripts/check.py"));
    }

    #[tokio::test]
    async fn heredoc_rewrite_guard_applies_only_to_structured_edit_profiles() {
        let command =
            "python3 - <<'PY'\nfrom pathlib import Path\nPath('src/lib.rs').write_text('x')\nPY";
        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        let openai_provider =
            codex_model_provider_info::ModelProviderInfo::create_openai_provider(None);
        turn.provider =
            codex_model_provider::create_model_provider(openai_provider, turn.auth_manager.clone());
        turn.model_info.slug = "gpt-5.2".to_string();
        assert!(
            reject_source_write_heredoc_when_structured_edit_enabled(&turn, command).is_ok(),
            "default Codex-native profile should not block Python heredocs"
        );

        turn.model_info.slug = "glm-5.2".to_string();
        assert_model_error(
            reject_source_write_heredoc_when_structured_edit_enabled(&turn, command),
            "structured_edit",
        );
    }

    #[tokio::test]
    async fn repeated_strict_patch_failures_enable_structured_edit_fallback() {
        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        let openai_provider =
            codex_model_provider_info::ModelProviderInfo::create_openai_provider(None);
        turn.provider =
            codex_model_provider::create_model_provider(openai_provider, turn.auth_manager.clone());
        turn.model_info.slug = "gpt-5.2".to_string();

        assert!(!structured_edit_protocol_enabled(&turn));
        assert_eq!(turn.record_strict_apply_patch_failure(), 1);
        assert!(!structured_edit_protocol_enabled(&turn));
        assert_eq!(turn.record_strict_apply_patch_failure(), 2);
        assert!(structured_edit_protocol_enabled(&turn));
    }

    #[tokio::test]
    async fn model_edit_profile_tag_classifies_glm_and_codex_patch_profiles() {
        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        let openai_provider =
            codex_model_provider_info::ModelProviderInfo::create_openai_provider(None);
        turn.provider =
            codex_model_provider::create_model_provider(openai_provider, turn.auth_manager.clone());

        turn.model_info.slug = "glm-5.2".to_string();
        assert_eq!(model_edit_profile_tag(&turn), "glm");

        turn.model_info.slug = "zai-org/glm-5.2".to_string();
        assert_eq!(model_edit_profile_tag(&turn), "glm");

        turn.model_info.slug = "gpt-5.2".to_string();
        if turn.model_info.apply_patch_tool_type.is_some() {
            assert_eq!(model_edit_profile_tag(&turn), "codex_patch");
        }
    }

    #[test]
    fn validate_relative_path_rejects_unsafe_paths() {
        assert!(validate_relative_path("src/lib.rs").is_ok());
        assert_model_error(validate_relative_path("/tmp/file"), "relative");
        assert_model_error(validate_relative_path("../file"), "cannot contain");
    }

    #[test]
    fn generated_update_patch_is_valid_apply_patch() {
        let patch = build_update_patch("src/lib.rs", "one\n\ntwo\n", "one\n\nthree\n");
        let parsed = codex_apply_patch::parse_patch(&patch).expect("valid generated patch");
        assert_eq!(parsed.hunks.len(), 1);
    }

    #[test]
    fn generated_add_patch_is_valid_apply_patch() {
        let patch = build_add_patch("src/lib.rs", "one\n\ntwo\n");
        let parsed = codex_apply_patch::parse_patch(&patch).expect("valid generated patch");
        assert_eq!(parsed.hunks.len(), 1);
    }

    #[test]
    fn multi_environment_schema_requires_environment_id() {
        let spec = create_structured_edit_tool(/*multi_environment*/ true);
        let value = serde_json::to_value(spec).expect("tool spec serializes");
        assert!(
            value
                .pointer("/parameters/required")
                .and_then(Value::as_array)
                .expect("required array")
                .iter()
                .any(|value| value == "environment_id")
        );
    }

    #[test]
    fn single_environment_schema_omits_environment_id() {
        let spec = create_structured_edit_tool(/*multi_environment*/ false);
        let value = serde_json::to_value(spec).expect("tool spec serializes");
        assert!(
            value
                .pointer("/parameters/properties/environment_id")
                .is_none()
        );
    }

    #[test]
    fn structured_tool_descriptions_discourage_read_only_review_misuse() {
        let edit = serde_json::to_value(create_structured_edit_tool(
            /*multi_environment*/ false,
        ))
        .expect("edit tool spec serializes");
        let edit_description = edit
            .pointer("/description")
            .and_then(Value::as_str)
            .expect("edit description");
        assert!(edit_description.contains("not a read or inspection tool"));
        assert!(edit_description.contains("read-only review"));

        let old_string_description = edit
            .pointer("/parameters/properties/old_string/description")
            .and_then(Value::as_str)
            .expect("old_string description");
        assert!(old_string_description.contains("Non-empty"));
        assert!(old_string_description.contains("not a read or inspection field"));

        let write = serde_json::to_value(create_structured_write_tool(
            /*multi_environment*/ false,
        ))
        .expect("write tool spec serializes");
        let write_description = write
            .pointer("/description")
            .and_then(Value::as_str)
            .expect("write description");
        assert!(write_description.contains("not a read or inspection tool"));
        assert!(write_description.contains("read-only review"));
    }
}
