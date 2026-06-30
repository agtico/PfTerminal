use codex_protocol::ThreadId;
#[cfg(test)]
use codex_protocol::config_types::EnvironmentVariablePattern;
use codex_protocol::config_types::ShellEnvironmentPolicy;
use codex_protocol::shell_environment;
use std::collections::HashMap;

pub use codex_protocol::shell_environment::CODEX_THREAD_ID_ENV_VAR;

const BUILT_IN_PROVIDER_AUTH_ENV_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "AMBIENT_API_KEY",
    "ZAI_API_KEY",
    "OPENROUTER_API_KEY",
    "BASETEN_API_KEY",
    "AI_GATEWAY_API_KEY",
];

/// Construct an environment map based on the rules in the specified policy. The
/// resulting map can be passed directly to `Command::envs()` after calling
/// `env_clear()` to ensure no unintended variables are leaked to the spawned
/// process.
///
/// The derivation follows the algorithm documented in the struct-level comment
/// for [`ShellEnvironmentPolicy`].
///
/// `CODEX_THREAD_ID` is injected when a thread id is provided, even when
/// `include_only` is set.
pub fn create_env(
    policy: &ShellEnvironmentPolicy,
    thread_id: Option<ThreadId>,
) -> HashMap<String, String> {
    let thread_id = thread_id.map(|thread_id| thread_id.to_string());
    shell_environment::create_env(policy, thread_id.as_deref())
}

pub fn create_shell_tool_env<'a, I>(
    policy: &ShellEnvironmentPolicy,
    thread_id: Option<ThreadId>,
    provider_env_keys: I,
) -> HashMap<String, String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut env = create_env(policy, thread_id);
    remove_provider_auth_env_vars(&mut env, provider_env_keys);
    env
}

pub fn remove_provider_auth_env_vars<'a, I>(env: &mut HashMap<String, String>, provider_env_keys: I)
where
    I: IntoIterator<Item = &'a str>,
{
    let provider_env_keys = provider_env_keys.into_iter().collect::<Vec<_>>();
    env.retain(|key, _| {
        !BUILT_IN_PROVIDER_AUTH_ENV_VARS
            .iter()
            .any(|blocked| key.eq_ignore_ascii_case(blocked))
            && !provider_env_keys
                .iter()
                .any(|blocked| key.eq_ignore_ascii_case(blocked))
    });
}

#[cfg(all(test, target_os = "windows"))]
fn create_env_from_vars<I>(
    vars: I,
    policy: &ShellEnvironmentPolicy,
    thread_id: Option<ThreadId>,
) -> HashMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    let thread_id = thread_id.map(|thread_id| thread_id.to_string());
    shell_environment::create_env_from_vars(vars, policy, thread_id.as_deref())
}

#[cfg(test)]
fn populate_env<I>(
    vars: I,
    policy: &ShellEnvironmentPolicy,
    thread_id: Option<ThreadId>,
) -> HashMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    let thread_id = thread_id.map(|thread_id| thread_id.to_string());
    shell_environment::populate_env(vars, policy, thread_id.as_deref())
}

#[cfg(test)]
#[path = "exec_env_tests.rs"]
mod tests;
