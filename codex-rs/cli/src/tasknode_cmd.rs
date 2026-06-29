use anyhow::Context;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use codex_core::config::ConfigBuilder;
use codex_utils_cli::CliConfigOverrides;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const TASKNODE_SESSION_LABEL: &str = "tasknode/session";
const DEFAULT_TASKNODE_ORIGIN: &str = "https://tasknode.postfiat.org";

#[derive(Debug, Parser)]
pub(crate) struct TaskNodeCli {
    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Emit JSON. This helper always emits JSON; the flag is accepted for scripts.
    #[arg(long, global = true, default_value_t = false)]
    pub json: bool,

    /// Override Task Node origin. Defaults to PFT_TASKNODE_ORIGIN, TASKNODE_ORIGIN, saved session origin, or production.
    #[arg(long, global = true)]
    pub origin: Option<String>,

    #[command(subcommand)]
    pub command: TaskNodeCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TaskNodeCommand {
    /// Show linked account, wallet, server flags, and task counts.
    Status,

    /// Show linked-wallet PFT balance.
    Balance(BalanceArgs),

    /// Show recent rewarded tasks.
    Rewards(RewardsCli),

    /// Work with Task Node chat.
    Chat(ChatCli),

    /// Read or save the Task Node context document.
    Context(ContextCli),

    /// Create a new task request.
    Request(RequestCli),

    /// List or inspect active task-generation requests.
    Requests(RequestsCli),

    /// List Task Node tasks by tab.
    Tasks(TasksCli),

    /// Inspect or mutate one Task Node task.
    Task(TaskCli),

    /// Respond to verification requests.
    Verification(VerificationCli),
}

#[derive(Debug, Args)]
pub(crate) struct BalanceArgs {
    /// Force a fresh balance lookup.
    #[arg(long, default_value_t = false)]
    force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RewardsCli {
    #[command(subcommand)]
    action: RewardsCommand,
}

#[derive(Debug, Subcommand)]
enum RewardsCommand {
    /// List recent rewards.
    List(LimitArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ChatCli {
    #[command(subcommand)]
    action: ChatCommand,
}

#[derive(Debug, Subcommand)]
enum ChatCommand {
    /// List standard Task Node chat threads.
    #[clap(alias = "conversations")]
    List(LimitArgs),

    /// Read a chat thread.
    History(ChatHistoryArgs),

    /// Search chat threads.
    Search(ChatSearchArgs),

    /// Send a Private Thinking chat message.
    Send(ChatSendArgs),
}

#[derive(Debug, Args)]
struct ChatHistoryArgs {
    conversation_id: String,

    #[arg(long, default_value_t = 120)]
    limit: u16,
}

#[derive(Debug, Args)]
struct ChatSearchArgs {
    query: String,

    #[arg(long, default_value_t = 20)]
    limit: u8,
}

#[derive(Debug, Args)]
struct ChatSendArgs {
    /// Message text. Use --message-file for multiline prompts.
    #[arg(long)]
    message: Option<String>,

    /// Read message text from a file.
    #[arg(long, value_name = "PATH")]
    message_file: Option<PathBuf>,

    /// Existing conversation id. Omit to create a new terminal chat id.
    #[arg(long)]
    conversation_id: Option<String>,

    /// Chat mode. Defaults to Private Thinking.
    #[arg(long, default_value = "Private Thinking")]
    mode: String,

    /// Stream SSE events as JSON lines.
    #[arg(long, default_value_t = false)]
    stream: bool,

    /// Preflight through the backend without calling the model, when the server supports it.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ContextCli {
    #[command(subcommand)]
    action: ContextCommand,
}

#[derive(Debug, Subcommand)]
enum ContextCommand {
    /// Read the current context document.
    Get,

    /// Save a new context document body.
    Save(ContextSaveArgs),
}

#[derive(Debug, Args)]
struct ContextSaveArgs {
    /// Read context body from this file.
    #[arg(long, value_name = "PATH")]
    body_file: PathBuf,

    /// Current revision from `tasknode context get`.
    #[arg(long)]
    revision: u64,

    /// Optional document title.
    #[arg(long)]
    title: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct RequestCli {
    #[command(subcommand)]
    action: RequestCommand,
}

#[derive(Debug, Subcommand)]
enum RequestCommand {
    /// Create a new personal task request.
    Create(RequestCreateArgs),
}

#[derive(Debug, Args)]
struct RequestCreateArgs {
    /// Task request text. Use --body-file for multiline requests.
    #[arg(long)]
    text: Option<String>,

    /// Read task request text from a file.
    #[arg(long, value_name = "PATH")]
    body_file: Option<PathBuf>,

    /// Task request kind.
    #[arg(long, default_value = "personal")]
    kind: String,

    /// Source title recorded in Task Node.
    #[arg(long, default_value = "PFTerminal JSON helper")]
    source_title: String,
}

#[derive(Debug, Args)]
pub(crate) struct RequestsCli {
    #[command(subcommand)]
    action: RequestsCommand,
}

#[derive(Debug, Subcommand)]
enum RequestsCommand {
    /// List active task-generation requests.
    List(LimitArgs),

    /// Show one task request.
    Show(RequestShowArgs),
}

#[derive(Debug, Args)]
struct RequestShowArgs {
    request_id: String,
}

#[derive(Debug, Args)]
pub(crate) struct TasksCli {
    #[command(subcommand)]
    action: TasksCommand,
}

#[derive(Debug, Subcommand)]
enum TasksCommand {
    /// List tasks in a tab.
    List(TasksListArgs),
}

#[derive(Debug, Args)]
struct TasksListArgs {
    /// Task tab: outstanding, verification, refused, rewarded, etc.
    #[arg(long, default_value = "outstanding")]
    tab: String,
}

#[derive(Debug, Args)]
pub(crate) struct TaskCli {
    #[command(subcommand)]
    action: TaskCommand,
}

#[derive(Debug, Subcommand)]
enum TaskCommand {
    /// Show one task, including terminal-rendered brief text.
    Show(TaskIdArgs),

    /// Accept one task.
    Accept(TaskIdArgs),

    /// Refuse one task.
    Refuse(TaskRefuseArgs),

    /// Cancel one accepted task.
    Cancel(TaskIdArgs),

    /// Submit initial evidence or follow-up evidence for one task.
    Evidence(TaskEvidenceArgs),
}

#[derive(Debug, Args)]
struct TaskIdArgs {
    task_id: String,
}

#[derive(Debug, Args)]
struct TaskRefuseArgs {
    task_id: String,

    /// Refusal reason text.
    #[arg(long)]
    reason: Option<String>,

    /// Read refusal reason from a file.
    #[arg(long, value_name = "PATH")]
    reason_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct TaskEvidenceArgs {
    task_id: String,

    /// Evidence summary text.
    #[arg(long)]
    summary: Option<String>,

    /// Read evidence summary from a file.
    #[arg(long, value_name = "PATH")]
    body_file: Option<PathBuf>,

    /// Additional artifact. Accepts a URL or type=value, repeatable.
    #[arg(long = "artifact")]
    artifacts: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct VerificationCli {
    #[command(subcommand)]
    action: VerificationCommand,
}

#[derive(Debug, Subcommand)]
enum VerificationCommand {
    /// Submit a verification response for one task.
    Respond(TaskEvidenceArgs),
}

#[derive(Debug, Args)]
struct LimitArgs {
    #[arg(long, default_value_t = 20)]
    limit: u8,
}

pub(crate) async fn run(command: TaskNodeCli) -> anyhow::Result<()> {
    let result = run_inner(command).await;
    match result {
        Ok(exit_code) => {
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
            Ok(())
        }
        Err(err) => {
            print_json(&json!({
                "ok": false,
                "error": "tasknode_helper_error",
                "message": err.to_string(),
            }))?;
            std::process::exit(1);
        }
    }
}

async fn run_inner(command: TaskNodeCli) -> anyhow::Result<i32> {
    let _json_flag = command.json;
    let client = TaskNodeClient::from_cli(command.config_overrides, command.origin).await?;

    match command.command {
        TaskNodeCommand::Status => {
            emit_response(client.get("/api/terminal/tasknode/status").await?)
        }
        TaskNodeCommand::Balance(args) => {
            let path = if args.force {
                "/api/terminal/tasknode/balance?force=1"
            } else {
                "/api/terminal/tasknode/balance"
            };
            emit_response(client.get(path).await?)
        }
        TaskNodeCommand::Rewards(cli) => match cli.action {
            RewardsCommand::List(args) => emit_response(
                client
                    .get(&format!(
                        "/api/terminal/tasknode/rewards?limit={}",
                        limit(args.limit, 1, 50)
                    ))
                    .await?,
            ),
        },
        TaskNodeCommand::Chat(cli) => run_chat_command(&client, cli).await,
        TaskNodeCommand::Context(cli) => run_context_command(&client, cli).await,
        TaskNodeCommand::Request(cli) => run_request_command(&client, cli).await,
        TaskNodeCommand::Requests(cli) => run_requests_command(&client, cli).await,
        TaskNodeCommand::Tasks(cli) => run_tasks_command(&client, cli).await,
        TaskNodeCommand::Task(cli) => run_task_command(&client, cli).await,
        TaskNodeCommand::Verification(cli) => run_verification_command(&client, cli).await,
    }
}

async fn run_chat_command(client: &TaskNodeClient, cli: ChatCli) -> anyhow::Result<i32> {
    match cli.action {
        ChatCommand::List(args) => emit_response(
            client
                .get(&format!(
                    "/api/terminal/tasknode/chat/conversations?limit={}",
                    limit(args.limit, 1, 50)
                ))
                .await?,
        ),
        ChatCommand::History(args) => emit_response(
            client
                .get(&format!(
                    "/api/terminal/tasknode/chat/history?conversationId={}&limit={}",
                    urlencoding::encode(&args.conversation_id),
                    limit_u16(args.limit, 1, 200)
                ))
                .await?,
        ),
        ChatCommand::Search(args) => emit_response(
            client
                .get(&format!(
                    "/api/terminal/tasknode/chat/search?q={}&limit={}",
                    urlencoding::encode(&args.query),
                    limit(args.limit, 1, 50)
                ))
                .await?,
        ),
        ChatCommand::Send(args) => {
            let message = read_text_input(args.message, args.message_file, "chat message")?;
            let conversation_id = args
                .conversation_id
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(new_chat_id);
            let body = json!({
                "conversationId": conversation_id,
                "message": message,
                "mode": args.mode,
                "dryRun": args.dry_run,
            });
            if args.stream {
                client
                    .post_sse_jsonl("/api/terminal/tasknode/chat/stream", &body)
                    .await
            } else {
                emit_response(
                    client
                        .post("/api/terminal/tasknode/chat/send", &body)
                        .await?,
                )
            }
        }
    }
}

async fn run_context_command(client: &TaskNodeClient, cli: ContextCli) -> anyhow::Result<i32> {
    match cli.action {
        ContextCommand::Get => emit_response(client.get("/api/terminal/tasknode/context").await?),
        ContextCommand::Save(args) => {
            let body_text = read_file_required(&args.body_file, "context body")?;
            let mut body = Map::new();
            body.insert("body".to_string(), Value::String(body_text));
            body.insert("revision".to_string(), Value::from(args.revision));
            body.insert(
                "source".to_string(),
                Value::String("pfterminal-cli".to_string()),
            );
            if let Some(title) = args.title.filter(|value| !value.trim().is_empty()) {
                body.insert("title".to_string(), Value::String(title));
            }
            emit_response(
                client
                    .post("/api/terminal/tasknode/context", &Value::Object(body))
                    .await?,
            )
        }
    }
}

async fn run_request_command(client: &TaskNodeClient, cli: RequestCli) -> anyhow::Result<i32> {
    match cli.action {
        RequestCommand::Create(args) => {
            let detail = read_text_input(args.text, args.body_file, "task request")?;
            let body = json!({
                "userDetailText": detail,
                "requestedTaskKind": args.kind,
                "source": "pfterminal-cli",
                "sourceConversationTitle": args.source_title,
                "idempotencyKey": idempotency_key("request"),
            });
            emit_response(
                client
                    .post("/api/terminal/tasknode/requests", &body)
                    .await?,
            )
        }
    }
}

async fn run_requests_command(client: &TaskNodeClient, cli: RequestsCli) -> anyhow::Result<i32> {
    match cli.action {
        RequestsCommand::List(args) => emit_response(
            client
                .get(&format!(
                    "/api/terminal/tasknode/requests?limit={}",
                    limit(args.limit, 1, 50)
                ))
                .await?,
        ),
        RequestsCommand::Show(args) => emit_response(
            client
                .get(&format!(
                    "/api/terminal/tasknode/requests/{}",
                    urlencoding::encode(&args.request_id)
                ))
                .await?,
        ),
    }
}

async fn run_tasks_command(client: &TaskNodeClient, cli: TasksCli) -> anyhow::Result<i32> {
    match cli.action {
        TasksCommand::List(args) => emit_response(
            client
                .get(&format!(
                    "/api/terminal/tasknode/tasks?tab={}",
                    urlencoding::encode(&args.tab)
                ))
                .await?,
        ),
    }
}

async fn run_task_command(client: &TaskNodeClient, cli: TaskCli) -> anyhow::Result<i32> {
    match cli.action {
        TaskCommand::Show(args) => emit_response(task_detail(client, &args.task_id).await?),
        TaskCommand::Accept(args) => {
            emit_response(task_action(client, &args.task_id, "accept", None).await?)
        }
        TaskCommand::Refuse(args) => {
            let reason = read_optional_text_input(args.reason, args.reason_file, "refusal reason")?;
            emit_response(task_action(client, &args.task_id, "refuse", reason).await?)
        }
        TaskCommand::Cancel(args) => {
            emit_response(task_action(client, &args.task_id, "cancel", None).await?)
        }
        TaskCommand::Evidence(args) => emit_response(task_evidence(client, args).await?),
    }
}

async fn run_verification_command(
    client: &TaskNodeClient,
    cli: VerificationCli,
) -> anyhow::Result<i32> {
    match cli.action {
        VerificationCommand::Respond(args) => emit_response(task_evidence(client, args).await?),
    }
}

async fn task_detail(client: &TaskNodeClient, task_id: &str) -> anyhow::Result<TaskNodeResponse> {
    client
        .get(&format!(
            "/api/terminal/tasknode/tasks/{}",
            urlencoding::encode(task_id)
        ))
        .await
}

async fn task_action(
    client: &TaskNodeClient,
    task_id: &str,
    action: &str,
    reason: Option<String>,
) -> anyhow::Result<TaskNodeResponse> {
    let mut body = Map::new();
    body.insert("action".to_string(), Value::String(action.to_string()));
    body.insert(
        "source".to_string(),
        Value::String("pfterminal-cli".to_string()),
    );
    body.insert(
        "idempotencyKey".to_string(),
        Value::String(idempotency_key(action)),
    );
    if let Some(reason) = reason.filter(|value| !value.trim().is_empty()) {
        body.insert("reason".to_string(), Value::String(reason));
    }
    client
        .post(
            &format!(
                "/api/terminal/tasknode/tasks/{}/action",
                urlencoding::encode(task_id)
            ),
            &Value::Object(body),
        )
        .await
}

async fn task_evidence(
    client: &TaskNodeClient,
    args: TaskEvidenceArgs,
) -> anyhow::Result<TaskNodeResponse> {
    let summary = read_text_input(args.summary, args.body_file, "task evidence")?;
    let body = json!({
        "summary": summary,
        "evidence": evidence_items_from_summary_and_artifacts(&summary, &args.artifacts),
        "source": "pfterminal-cli",
        "idempotencyKey": idempotency_key("evidence"),
    });
    client
        .post(
            &format!(
                "/api/terminal/tasknode/tasks/{}/evidence",
                urlencoding::encode(&args.task_id)
            ),
            &body,
        )
        .await
}

#[derive(Debug, Clone)]
struct TaskNodeClient {
    origin: String,
    token: String,
}

impl TaskNodeClient {
    async fn from_cli(
        config_overrides: CliConfigOverrides,
        origin_override: Option<String>,
    ) -> anyhow::Result<Self> {
        let cli_kv_overrides = config_overrides
            .parse_overrides()
            .map_err(anyhow::Error::msg)?;
        let config = ConfigBuilder::default()
            .cli_overrides(cli_kv_overrides)
            .build()
            .await?;
        let session = load_tasknode_session(config.codex_home.as_path())?;
        let token = session.terminal_token.clone().ok_or_else(|| {
            anyhow::anyhow!("Task Node session is missing a terminal token. Run /tasknode link.")
        })?;
        Ok(Self {
            origin: resolve_origin(origin_override, session.origin.as_deref()),
            token,
        })
    }

    async fn get(&self, path: &str) -> anyhow::Result<TaskNodeResponse> {
        let url = self.url(path);
        let response = normal_http_client()?
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(reqwest_error)?;
        parse_response(response).await
    }

    async fn post(&self, path: &str, body: &Value) -> anyhow::Result<TaskNodeResponse> {
        let url = self.url(path);
        let response = normal_http_client()?
            .post(url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(reqwest_error)?;
        parse_response(response).await
    }

    async fn post_sse_jsonl(&self, path: &str, body: &Value) -> anyhow::Result<i32> {
        let url = self.url(path);
        let mut response = streaming_http_client()?
            .post(url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(reqwest_error)?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !(200..300).contains(&status) || !content_type.contains("text/event-stream") {
            return emit_response(parse_response(response).await?);
        }

        let mut stdout = std::io::stdout();
        let mut buffer = String::new();
        let mut saw_done = false;
        let mut exit_code = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .context("failed reading Task Node chat stream")?
        {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            for block in tasknode_sse_drain_blocks(&mut buffer) {
                if let Some((event, data)) = tasknode_parse_sse_block(&block)? {
                    if event == "done" {
                        saw_done = true;
                    } else if event == "error" {
                        exit_code = 1;
                    }
                    writeln!(
                        stdout,
                        "{}",
                        serde_json::to_string(&json!({ "event": event, "data": data }))?
                    )?;
                    stdout.flush()?;
                }
            }
        }
        for block in tasknode_sse_drain_remainder(&mut buffer) {
            if let Some((event, data)) = tasknode_parse_sse_block(&block)? {
                if event == "done" {
                    saw_done = true;
                } else if event == "error" {
                    exit_code = 1;
                }
                writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&json!({ "event": event, "data": data }))?
                )?;
            }
        }
        stdout.flush()?;
        if !saw_done && exit_code == 0 {
            print_json(&json!({
                "ok": false,
                "error": "tasknode_stream_incomplete",
                "message": "Task Node chat stream ended without a done event.",
            }))?;
            return Ok(1);
        }
        Ok(exit_code)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.origin.trim_end_matches('/'), path)
    }
}

#[derive(Debug)]
struct TaskNodeResponse {
    status: u16,
    body: Value,
}

#[derive(Debug, Deserialize)]
struct TaskNodeLocalSession {
    origin: Option<String>,
    terminal_token: Option<String>,
    pending_verification_url: Option<String>,
}

fn load_tasknode_session(codex_home: &std::path::Path) -> anyhow::Result<TaskNodeLocalSession> {
    let vault = codex_vault::Vault::new(codex_home.to_path_buf());
    let secret = vault
        .reveal(TASKNODE_SESSION_LABEL)
        .map_err(|err| anyhow::anyhow!("Task Node is not linked. Run /tasknode link. ({err})"))?;
    let session: TaskNodeLocalSession =
        serde_json::from_str(&secret).context("invalid local Task Node session")?;
    if session.terminal_token.is_none() {
        if let Some(url) = session
            .pending_verification_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            anyhow::bail!("Task Node link is pending. Finish GitHub auth: {url}");
        }
        anyhow::bail!("Task Node session is missing a terminal token. Run /tasknode link.");
    }
    Ok(session)
}

fn resolve_origin(origin_override: Option<String>, saved_origin: Option<&str>) -> String {
    origin_override
        .or_else(|| std::env::var("PFT_TASKNODE_ORIGIN").ok())
        .or_else(|| std::env::var("TASKNODE_ORIGIN").ok())
        .or_else(|| saved_origin.map(ToString::to_string))
        .unwrap_or_else(|| DEFAULT_TASKNODE_ORIGIN.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn normal_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(reqwest_error)
}

fn streaming_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(reqwest_error)
}

async fn parse_response(response: reqwest::Response) -> anyhow::Result<TaskNodeResponse> {
    let status = response.status().as_u16();
    let text = response.text().await.map_err(reqwest_error)?;
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| {
        json!({
            "ok": false,
            "error": "tasknode_non_json_response",
            "message": text,
            "httpStatus": status,
        })
    });
    Ok(TaskNodeResponse { status, body })
}

fn emit_response(response: TaskNodeResponse) -> anyhow::Result<i32> {
    print_json(&response.body)?;
    Ok(if response_is_ok(&response) { 0 } else { 1 })
}

fn response_is_ok(response: &TaskNodeResponse) -> bool {
    (200..300).contains(&response.status)
        && response.body.get("ok").and_then(Value::as_bool) != Some(false)
}

fn print_json(value: &Value) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{}", serde_json::to_string(value)?)?;
    stdout.flush()?;
    Ok(())
}

fn read_text_input(
    inline: Option<String>,
    file: Option<PathBuf>,
    label: &str,
) -> anyhow::Result<String> {
    match (inline, file) {
        (Some(_), Some(_)) => anyhow::bail!(
            "Provide either --message/--text/--summary or a file for {label}, not both."
        ),
        (Some(text), None) => require_nonempty_text(text, label),
        (None, Some(path)) => read_file_required(&path, label),
        (None, None) => anyhow::bail!("{label} is required."),
    }
}

fn read_optional_text_input(
    inline: Option<String>,
    file: Option<PathBuf>,
    label: &str,
) -> anyhow::Result<Option<String>> {
    match (inline, file) {
        (Some(_), Some(_)) => {
            anyhow::bail!("Provide either inline text or a file for {label}, not both.")
        }
        (Some(text), None) => Ok(Some(require_nonempty_text(text, label)?)),
        (None, Some(path)) => Ok(Some(read_file_required(&path, label)?)),
        (None, None) => Ok(None),
    }
}

fn read_file_required(path: &PathBuf, label: &str) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed reading {label} file {}", path.display()))?;
    require_nonempty_text(text, label)
}

fn require_nonempty_text(text: String, label: &str) -> anyhow::Result<String> {
    if text.trim().is_empty() {
        anyhow::bail!("{label} is empty.");
    }
    Ok(text)
}

fn idempotency_key(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("pfterminal-cli:{prefix}:{}:{nanos}", std::process::id())
}

fn new_chat_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("chat_cli_{}_{}", std::process::id(), nanos)
}

fn limit(value: u8, min: u8, max: u8) -> u8 {
    value.clamp(min, max)
}

fn limit_u16(value: u16, min: u16, max: u16) -> u16 {
    value.clamp(min, max)
}

fn evidence_items_from_summary_and_artifacts(summary: &str, artifacts: &[String]) -> Vec<Value> {
    let mut items = artifacts
        .iter()
        .filter_map(|artifact| evidence_item_from_artifact(artifact))
        .collect::<Vec<_>>();
    for url in summary
        .split_whitespace()
        .filter(|part| part.starts_with("http://") || part.starts_with("https://"))
        .take(5)
    {
        if !items
            .iter()
            .any(|item| evidence_item_value(item) == Some(url))
        {
            items.push(evidence_item_from_value(infer_artifact_type(url), url));
        }
    }
    items
}

fn evidence_item_from_artifact(artifact: &str) -> Option<Value> {
    let trimmed = artifact.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((kind, value)) = trimmed.split_once('=') {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        return Some(evidence_item_from_value(kind.trim(), value));
    }
    Some(evidence_item_from_value(
        infer_artifact_type(trimmed),
        trimmed,
    ))
}

fn evidence_item_from_value(kind: &str, value: &str) -> Value {
    if value.starts_with("http://") || value.starts_with("https://") {
        json!({ "type": kind, "url": value })
    } else {
        json!({ "type": kind, "value": value })
    }
}

fn evidence_item_value(item: &Value) -> Option<&str> {
    item.get("url")
        .or_else(|| item.get("value"))
        .or_else(|| item.get("text"))
        .and_then(Value::as_str)
}

fn infer_artifact_type(value: &str) -> &'static str {
    if value.contains("github.com/") && value.contains("/pull/") {
        "github_pr"
    } else if value.contains("github.com/") && value.contains("/commit/") {
        "git_commit"
    } else if value.starts_with("http://") || value.starts_with("https://") {
        "url"
    } else {
        "text"
    }
}

fn tasknode_sse_separator(buffer: &str) -> Option<(usize, usize)> {
    match (buffer.find("\n\n"), buffer.find("\r\n\r\n")) {
        (Some(lf), Some(crlf)) if crlf < lf => Some((crlf, 4)),
        (Some(lf), _) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

fn tasknode_sse_drain_blocks(buffer: &mut String) -> Vec<String> {
    let mut blocks = Vec::new();
    while let Some((index, separator_len)) = tasknode_sse_separator(buffer) {
        let drained: String = buffer.drain(..index + separator_len).collect();
        blocks.push(drained[..index].to_string());
    }
    blocks
}

fn tasknode_sse_drain_remainder(buffer: &mut String) -> Vec<String> {
    let remainder = std::mem::take(buffer);
    if remainder.trim().is_empty() {
        Vec::new()
    } else {
        vec![remainder]
    }
}

fn tasknode_parse_sse_block(block: &str) -> anyhow::Result<Option<(String, Value)>> {
    let normalized = block.replace("\r\n", "\n");
    let mut event = "message".to_string();
    let mut data = Vec::new();
    for line in normalized.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.trim_start().to_string());
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    let data = data.join("\n");
    if data.trim() == "[DONE]" {
        return Ok(None);
    }
    let value = serde_json::from_str(&data)
        .with_context(|| format!("invalid Task Node chat stream event: {data}"))?;
    Ok(Some((event, value)))
}

fn reqwest_error(err: reqwest::Error) -> anyhow::Error {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(err) = source {
        let part = err.to_string();
        if !part.is_empty() && !message.contains(&part) {
            message.push_str(": ");
            message.push_str(&part);
        }
        source = std::error::Error::source(err);
    }
    anyhow::anyhow!(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_evidence_items_from_summary_urls_and_artifacts() {
        let items = evidence_items_from_summary_and_artifacts(
            "Implemented in https://github.com/postfiatorg/tasknodeofficial/pull/192 and commit https://github.com/postfiatorg/tasknodeofficial/commit/abc",
            &["log=terminal smoke passed".to_string()],
        );

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].get("type").and_then(Value::as_str), Some("log"));
        assert_eq!(
            items[1].get("type").and_then(Value::as_str),
            Some("github_pr")
        );
        assert_eq!(
            items[2].get("type").and_then(Value::as_str),
            Some("git_commit")
        );
    }

    #[test]
    fn parses_sse_delta_and_done_blocks() {
        let mut buffer = String::new();
        buffer.push_str("event: delta\ndata: {\"delta\":\"hi\"}\n\n");
        buffer.push_str("event: done\ndata: {\"ok\":true}\n\n");

        let blocks = tasknode_sse_drain_blocks(&mut buffer);
        assert_eq!(blocks.len(), 2);

        let first = tasknode_parse_sse_block(&blocks[0])
            .expect("valid first block")
            .expect("first event");
        assert_eq!(first.0, "delta");
        assert_eq!(first.1.get("delta").and_then(Value::as_str), Some("hi"));

        let second = tasknode_parse_sse_block(&blocks[1])
            .expect("valid second block")
            .expect("second event");
        assert_eq!(second.0, "done");
        assert_eq!(second.1.get("ok").and_then(Value::as_bool), Some(true));
    }

    #[test]
    fn parses_crlf_sse_blocks() {
        let mut buffer = "event: error\r\ndata: {\"message\":\"failed\"}\r\n\r\n".to_string();
        let blocks = tasknode_sse_drain_blocks(&mut buffer);
        assert_eq!(blocks.len(), 1);
        let parsed = tasknode_parse_sse_block(&blocks[0])
            .expect("valid crlf block")
            .expect("event");
        assert_eq!(parsed.0, "error");
        assert_eq!(
            parsed.1.get("message").and_then(Value::as_str),
            Some("failed")
        );
    }
}
