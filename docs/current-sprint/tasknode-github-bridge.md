# Task Node GitHub Bridge

Status: terminal bridge and parity pass implemented locally. PFTerminal now has
task-card rendering, evidence guidance, task request commands, request tracking,
read-only rewards/balance, and GitHub-linked Task Node session storage. Task
Node has matching terminal task rendering and task request endpoints pending
merge/deploy.

## User Story

I am a GitHub-authenticated user in PFTerminal and I want to natively interact
with the Task Node.

The first native surface is:

```text
/tasknode
```

`/tasknode` opens a terminal menu for linking the user's Task Node account,
viewing outstanding tasks, opening a terminal-native task card, accepting or
refusing a task, submitting evidence with the task requirement visible, and
requesting a personal task with text. The v0 bridge must not implement wallet
creation, seed import, private-key signing, payout flows, or crypto sends.
Read-only balance and recent reward views are allowed when the Task Node account
already has a linked wallet.

## Current Findings

### PFTerminal

PFTerminal already has the local TUI patterns needed for this feature:

- `codex-rs/tui/src/slash_command.rs` owns built-in slash command discovery,
  descriptions, inline-arg support, and command availability.
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs` maps slash commands to local
  UI flows or `AppEvent` messages.
- `codex-rs/tui/src/app_event.rs` and `codex-rs/tui/src/app/event_dispatch.rs`
  carry typed TUI events from chat input into the app runtime.
- `codex-rs/tui/src/spawn_orchestration.rs`,
  `codex-rs/tui/src/claude_panes/app_integration.rs`, and
  `codex-rs/tui/src/chatwidget/vault_menu.rs` are the closest menu examples.
- `SelectionViewParams` and `SelectionItem` already provide the highlighted-row
  terminal interaction that should be treated as "hover" for `/tasknode`.
- `CustomPromptView` can collect evidence text, URLs, PR links, and commit
  hashes without sending secrets through a model turn.
- `/vault` is the right storage boundary for Task Node terminal session tokens.

PFTerminal does not currently have a first-class GitHub login surface. It has
GitHub-adjacent code for repository metadata, but `/tasknode` should treat Task
Node account linking as a new host capability and avoid giving the model direct
access to GitHub OAuth tokens.

### Task Node

Task Node already has the account model and task read models needed for the
bridge:

- GitHub auth uses the existing `authStart`/`authCallback` provider contract.
- `server/auth-connected-accounts.js` implements GitHub OAuth and links the
  GitHub identity to the account cloud.
- `server/runtime-store.js::linkProviderToAccount` is the account-link boundary.
- `GET /api/tasks` is backed by `listTaskState` and returns the task tabs used
  by the web task surface.
- `GET /api/tasks/detail?taskId=...` returns the indexed task detail view.
- `POST /api/tasks/action` handles accept/refuse/cancel.
- `POST /api/tasks/submission` handles evidence submissions.
- `GET /api/wallet/balance` is read-only account-scoped balance data and does
  not require wallet unlock or seed material.
- `src/features/tasks/task-copy-format.js` defines the browser's `Copy task
  brief` payload. That contract is the best source of truth for terminal task
  cards because it already includes title, IDs, kind, status, reward, deadline,
  objective, steps, verification requirement, current verification request, and
  requested output guidance.
- `server/repositories/tasks.js::publicTask` already puts description, steps,
  verification text, due labels, reward, task class, metadata, timestamps, and
  PFTL provenance into task rows. The current terminal prototype discards much
  of that data in Rust; the API does not need a new authoritative task DB for
  basic card rendering.
- `POST /api/tasks/request` is the browser task request path. It builds a
  request bundle from the Task Node account's context document, memory, recent
  chat, and current task queue. The normal browser path signs a PFTL request
  pointer locally; the direct offchain path can record the request server-side
  when `TASKNODE_OFFCHAIN_TASK_LIFECYCLE=true` and dual-write is off.

The current mutation boundary is not GitHub-only yet. Task action and evidence
routes call helpers that require a signed-in account and a linked wallet before
they reach the offchain/direct-write path. A native `/tasknode` implementation
therefore needs a Task Node backend bridge for GitHub-authenticated terminal
sessions, not only a PFTerminal menu.

### Current Terminal Parity Gap

The first local `/tasknode` prototype proves auth, task listing, action menus,
and basic evidence POSTs, but it is not yet feature-parity with the existing web
Tasks surface:

- Task list rows show title/status/PFT/deadline only.
- Selecting a task jumps straight to action choices instead of showing the task
  card.
- The evidence prompt only asks for generic text, so the user cannot see the
  objective, steps, verification requirement, or current verification request
  while deciding what evidence to submit.
- There is no terminal task-request flow equivalent to the web `Request task`
  button or chat task-request mode.
- There is no terminal copy/export of the Codex-oriented task brief.

The next scope is therefore not "more buttons." It is terminal-native parity
with the task-card, submit, request, and copy-brief workflows.

## Product Boundary

### Goals

- Let a GitHub-authenticated PFTerminal user link to their Task Node account.
- Show account, GitHub, linked-wallet, balance, reward, and task status.
- List outstanding tasks inside PFTerminal.
- Open a selected task and show a terminal-native task card before actions.
- Render the web task brief contract in terminal form: objective, steps,
  verification requirements, current verification request, status, reward,
  deadline, relevant IDs, and requested output guidance.
- Accept, refuse, cancel, or submit evidence when the Task Node backend allows
  an account-scoped terminal action.
- Request a personal task from PFTerminal with free-form text, using the same
  Task Node context, memory, and queue inputs as the web task request path.
- Show active task request state until a generated offer becomes a task card or
  the request needs attention.
- Reuse existing Task Node task read models instead of creating a parallel task
  database.
- Reuse the existing PFTerminal vault boundary for terminal session secrets.

### Non-Goals

- No seed phrase entry or wallet import in PFTerminal.
- No private-key signing in PFTerminal.
- No PFT send, payout, swap, bridge, or custody features.
- No raw GitHub OAuth token exposure to the model, transcript, shell tools, or
  subagents.
- No fake completion if the backend still requires wallet signing for a task.
- No direct database access from PFTerminal.
- No terminal-native Network Task routing button in v0. Text task requests map
  to personal task requests unless Task Node later exposes an explicit server
  policy for terminal-routed Network Tasks.

## Two-Sided Integration

This feature must be implemented on both sides of the boundary.

Task Node is the authority for account identity and eligibility:

- It owns the GitHub OAuth callback.
- It owns the persisted linked-provider record.
- It decides whether an account is eligible for terminal bridge access.
- It mints, refreshes, and revokes Task Node terminal sessions.
- It accepts or rejects task actions.

PFTerminal is the host UI and secret boundary:

- It opens `/tasknode`.
- It starts and polls the Task Node terminal auth flow.
- It stores only Task Node terminal session tokens in `/vault`.
- It renders task, reward, and balance views from Task Node responses.
- It sends task actions and evidence to Task Node, then trusts only server
  receipts.

Local GitHub state in PFTerminal, `gh auth status`, or a future PFTerminal
GitHub provider login can be used as a convenience signal, but it is not
authorization. The feature works only when Task Node says the account has a
linked GitHub provider.

## GitHub Link Gate

The terminal bridge must be gated by Task Node's linked-provider state.

Rules:

- A Task Node terminal session must not be minted unless the resolved account
  has a persisted `github` provider link.
- The terminal GitHub auth flow may create or attach that provider link, but the
  session is returned only after the link is committed server-side.
- Terminal task mutations must re-check that the terminal session is bound to an
  account with a linked GitHub provider.
- If the account is authenticated by another method but lacks GitHub, Task Node
  returns `github_not_linked` and a GitHub-link URL.
- PFTerminal must show the GitHub-link action and must not attempt to bypass the
  gate with a local GitHub token.

Recommended structured error:

```json
{
  "error": "github_not_linked",
  "message": "Link GitHub in Task Node before using /tasknode.",
  "linkUrl": "https://tasknode.example/settings/accounts/github"
}
```

## Auth Model

PFTerminal should not use a GitHub token as the Task Node API token. GitHub
proves identity to Task Node; Task Node should then mint its own terminal
session token with explicit scopes.

### Recommended Flow

1. User runs `/tasknode` and selects `Link GitHub / Task Node`.
2. PFTerminal calls a new Task Node endpoint:

   ```http
   POST /api/auth/terminal/start/github
   ```

3. Task Node creates a pending terminal login request and returns:

   ```json
   {
     "requestId": "tnterm_...",
     "verificationUrl": "https://tasknode.example/auth/terminal/github?code=...",
     "userCode": "ABCD-EFGH",
     "expiresAt": "2026-06-29T12:30:00Z",
     "pollIntervalMs": 2000
   }
   ```

4. PFTerminal opens or prints the verification URL.
5. User completes GitHub OAuth in the browser through the existing Task Node
   GitHub provider path. Task Node creates or attaches the account cloud and
   persists the `github` provider link.
6. PFTerminal polls:

   ```http
   GET /api/auth/terminal/session?requestId=tnterm_...
   ```

7. When the GitHub provider link has been committed, Task Node returns a Task
   Node terminal session, not a GitHub token:

   ```json
   {
     "accountId": "acct_...",
     "githubUsername": "user",
     "terminalToken": "tns_...",
     "expiresAt": "2026-06-30T12:00:00Z",
     "refreshToken": "tnr_...",
     "scopes": [
       "tasknode:read",
       "tasknode:tasks:write",
       "tasknode:balance:read"
     ]
   }
   ```

8. PFTerminal stores the terminal session in the encrypted vault under a scoped
   label such as `tasknode/session/<origin>`.

If PFTerminal later adds a GitHub provider login using GitHub device auth or
`gh auth status`, that can shorten the browser step, but Task Node still must
mint the Task Node terminal session.

### Token Rules

- Terminal tokens are bearer tokens scoped to one Task Node origin.
- Tokens should expire and support refresh/revocation.
- Tokens are host-only secrets and must never enter the chat transcript.
- Logs must redact `Authorization`, refresh tokens, GitHub access tokens, and
  terminal session tokens.
- Logout removes the PFTerminal vault record and calls Task Node revocation when
  possible.

## Task Node Bridge API

The bridge should be a thin authenticated API over existing Task Node account
and task read models.

### Session Status

```http
GET /api/terminal/tasknode/status
Authorization: Bearer tns_...
```

Response:

```json
{
  "accountId": "acct_...",
  "github": {
    "linked": true,
    "username": "user",
    "terminalBridgeEligible": true
  },
  "wallet": {
    "linked": true,
    "address": "r...",
    "signingRequiredForActions": false
  },
  "counts": {
    "outstanding": 4,
    "verification": 1,
    "refused": 2,
    "rewarded": 7
  },
  "server": {
    "offchainTaskLifecycle": true,
    "terminalTaskActions": true
  }
}
```

If `github.linked` is false, the response should include `github_not_linked`
semantics and a link URL instead of task-action eligibility.

### Task List

```http
GET /api/terminal/tasknode/tasks?tab=outstanding
Authorization: Bearer tns_...
```

The response should be derived from `listTaskState` and shaped for terminal
rendering:

```json
{
  "tab": "outstanding",
  "tasks": [
    {
      "taskId": "task_...",
      "title": "Review release smoke output",
      "statusKey": "proposed",
      "statusLabel": "Proposed",
      "kind": "network",
      "rewardOfferPft": "25",
      "rewardActualPft": null,
      "acceptBy": "2026-06-30T00:00:00Z",
      "deadlineAt": "2026-07-01T00:00:00Z",
      "actions": ["accept", "refuse"]
    }
  ]
}
```

Tabs should match the web task surface:

- `outstanding`
- `verification`
- `refused`
- `rewarded`

### Task Detail

```http
GET /api/terminal/tasknode/tasks/:taskId
Authorization: Bearer tns_...
```

The response should be derived from `getTaskDetail` and include the task
projection, action history, evidence summary, reward fields, and links that are
safe to show in a terminal.

The terminal detail response must include either structured fields sufficient
for PFTerminal to render a card, or a server-built terminal brief generated from
the same contract as `src/features/tasks/task-copy-format.js`.

Minimum structured detail:

```json
{
  "task": {
    "taskId": "task_...",
    "title": "Finish NAVCoin Local Wallet Integration",
    "kind": "Network",
    "statusKey": "accepted",
    "status": "Accepted",
    "pft": 2.5,
    "dueLabel": "Deadline",
    "fullDue": "No deadline",
    "description": "Complete the remaining local wallet integration work.",
    "steps": [
      "Wire the wallet adapter into the local runtime.",
      "Add smoke coverage for the happy path.",
      "Document any unsupported wallet states."
    ],
    "verification": {
      "title": "Submit evidence",
      "body": "Submit a PR, commit link, terminal output, or concise proof that the integration works."
    },
    "metadata": {
      "requestId": "req_...",
      "networkProjectId": "..."
    }
  },
  "currentVerificationRequest": {
    "body": "Please provide the exact PR and smoke output.",
    "reason": "The previous evidence did not identify the changed files."
  },
  "rewardOutcome": {
    "decision": "accepted",
    "rewardPft": 2.5,
    "reason": "Evidence satisfied the request.",
    "userFeedback": "Clear proof."
  },
  "forensics": {
    "eventCount": 2,
    "lastEventTxHash": "...",
    "lastEventCid": "..."
  },
  "actions": {
    "canAccept": false,
    "canRefuse": false,
    "canCancel": true,
    "canSubmitInitialEvidence": true,
    "canSubmitVerificationEvidence": false
  },
  "terminal": {
    "briefText": "Task for Codex\n...",
    "evidencePrompt": {
      "mode": "initial_submission",
      "title": "Submit evidence for Finish NAVCoin Local Wallet Integration",
      "body": "Submit a PR, commit link, terminal output, or concise proof that the integration works.",
      "acceptedTypes": ["text", "url", "github_pr", "git_commit"],
      "maxArtifacts": 2
    }
  }
}
```

PFTerminal can render from `task`, `currentVerificationRequest`,
`rewardOutcome`, and `forensics` directly. The optional `terminal.briefText`
lets the server own exact brief parity with the web `Copy task brief` formatter
so Rust does not need to duplicate every presentation rule.

### Terminal Task Card

Selecting a task from a list must open a detail card first, not an action-only
menu. A terminal card should be optimized for scanning:

```text
Finish NAVCoin Local Wallet Integration
task_73926fa8b37cfcfb546e374b3744cf53
Accepted | Network | 2.5 PFT | Deadline: No deadline | 2 indexed events

Objective
Complete the remaining local wallet integration work.

Steps
1. Wire the wallet adapter into the local runtime.
2. Add smoke coverage for the happy path.
3. Document any unsupported wallet states.

Verification
Submit a PR, commit link, terminal output, or concise proof that the integration works.

Actions
> Submit evidence
  Cancel task
  Copy task brief
  Forensics
```

For `verification_requested`, the current verification request must be visible
above the evidence action:

```text
Current Verification Request
Please provide the exact PR and smoke output.
Reason: The previous evidence did not identify the changed files.
```

For rewarded tasks, show reward outcome decision, reward amount, reason, and
feedback before forensics/copy actions.

### Task Action

```http
POST /api/terminal/tasknode/tasks/:taskId/action
Authorization: Bearer tns_...
Idempotency-Key: ...
Content-Type: application/json

{
  "action": "accept",
  "source": "pfterminal"
}
```

Allowed v0 actions:

- `accept`
- `refuse`
- `cancel`

The server must decide whether this terminal session is allowed to create an
account-scoped direct-write transition. PFTerminal should not decide that by
itself.

If the server still requires a wallet-signed PFTL pointer, return:

```json
{
  "error": "wallet_action_required",
  "message": "Open Task Node web wallet to sign this task action.",
  "handoffUrl": "https://tasknode.example/tasks/task_..."
}
```

PFTerminal should show the handoff and leave the task unchanged.

### Evidence Submission

```http
POST /api/terminal/tasknode/tasks/:taskId/evidence
Authorization: Bearer tns_...
Idempotency-Key: ...
Content-Type: application/json

{
  "mode": "initial_submission",
  "summary": "Implemented the task and attached the PR.",
  "evidence": [
    {
      "type": "github_pr",
      "url": "https://github.com/org/repo/pull/123"
    },
    {
      "type": "git_commit",
      "sha": "abc123..."
    }
  ],
  "source": "pfterminal"
}
```

Allowed v0 evidence types:

- `text`
- `url`
- `github_pr`
- `git_commit`
- `artifact_reference`

The first implementation can accept plain text and URLs only. GitHub PR and
commit normalization can reuse the existing evidence packet tooling from Task
Node after the basic bridge works.

Evidence prompting must be task-aware. The terminal prompt must include:

- task title and ID;
- whether this is initial evidence or verification-response evidence;
- objective summary;
- verification requirement;
- current verification request and reason when present;
- accepted evidence types;
- concrete examples such as PR URL, commit URL, terminal output summary, test
  command output, screenshot description, or concise proof text.

The prompt copy must not be a blank "Summary, PR URL, commit, or evidence text"
field. The user should know what they are satisfying before entering evidence.

### Task Request

```http
POST /api/terminal/tasknode/requests
Authorization: Bearer tns_...
Idempotency-Key: ...
Content-Type: application/json

{
  "userDetailText": "Give me a 2-4 hour engineering task that advances the terminal Task Node workflow.",
  "requestedTaskKind": "personal",
  "source": "pfterminal"
}
```

Terminal task requests should reuse the same server bundle builder as the web
task request path:

- Task Node context document;
- deep and recent memory;
- recent chats when available;
- current outstanding, verification, refused, and rewarded task queues;
- linked account and linked wallet identity;
- task policy, reward policy, and deadline defaults.

Because PFTerminal intentionally has no wallet signing, Task Node must decide
whether a terminal request can be recorded:

- If direct offchain task lifecycle is enabled and dual-write is disabled, the
  server may record the request using the existing direct-write path.
- If the deployment requires a wallet-signed PFTL pointer, return
  `wallet_action_required` with a handoff URL instead of asking the terminal for
  a seed phrase.
- If the account has no linked wallet, return `wallet_not_linked`.
- If GitHub is unlinked or the terminal session is invalid, return the normal
  terminal auth errors.

Response:

```json
{
  "ok": true,
  "requestId": "req_...",
  "bundleId": "bundle_...",
  "message": "Task request recorded in Task Node.",
  "generationScheduled": {
    "scheduled": true,
    "reason": "terminal_task_request_direct_write"
  },
  "request": {
    "status": "published",
    "userDetailText": "Give me a 2-4 hour engineering task..."
  }
}
```

Request status endpoints:

```http
GET /api/terminal/tasknode/requests
GET /api/terminal/tasknode/requests/:requestId
```

These should mirror `GET /api/tasks/requests` and return active request rows so
PFTerminal can show queued/generating/proposed/failed states until the generated
offer becomes a normal task card.

### Balance

```http
GET /api/terminal/tasknode/balance
Authorization: Bearer tns_...
```

This should wrap the existing read-only balance behavior. If no wallet is
linked, return a structured `wallet_not_linked` response. PFTerminal must not
prompt for a seed phrase or wallet password.

### Recent Rewards

```http
GET /api/terminal/tasknode/rewards?limit=10
Authorization: Bearer tns_...
```

This can be derived from the `rewarded` task bucket first. A richer reward
ledger can be added later if Task Node exposes one.

## PFTerminal UI

### Slash Command

Add `SlashCommand::Tasknode`:

- command string: `/tasknode`
- description: `interact with Task Node tasks and rewards`
- inline args:
  - `/tasknode link`
  - `/tasknode status`
  - `/tasknode tasks`
  - `/tasknode task <task-id>`
  - `/tasknode request [text]`
  - `/tasknode requests`
  - `/tasknode balance`
  - `/tasknode rewards`
  - `/tasknode logout`
- available during task: yes for read-only views and local menus
- side conversation availability: yes for read-only views; write actions should
  dispatch through the main app event loop

Dispatch should follow the existing `/spawn`, `/panes`, and `/vault` pattern:

- bare `/tasknode` sends `AppEvent::OpenTaskNodeMenu`
- inline `/tasknode status` sends `AppEvent::OpenTaskNodeStatus`
- inline `/tasknode tasks` sends `AppEvent::OpenTaskNodeTaskList`
- inline `/tasknode task <task-id>` opens a task detail card
- inline `/tasknode request` opens the task request prompt
- inline `/tasknode request <text>` submits or previews a terminal task request
- inline `/tasknode link` starts the auth flow

### Menu Structure

Bare `/tasknode` opens:

```text
Task Node
  GitHub-linked work and rewards for this account.

> Outstanding tasks
  Verification requests
  Request personal task
  Active task requests
  Recent rewards
  Balance
  Link or refresh GitHub session
  Logout Task Node
```

If no session exists:

```text
Task Node
  Link your GitHub-backed Task Node account.

> Link GitHub / Task Node
  Configure Task Node origin
```

### Task List

Outstanding tasks should render as stable rows:

```text
Outstanding Tasks

> Review release smoke output      Proposed   25 PFT   accept by Jun 30
  Add bridge route smoke coverage   Accepted   40 PFT   due Jul 1
  Submit CLI evidence packet        Verify     15 PFT   response requested
```

The highlighted row is the terminal equivalent of hover. Pressing Enter opens
task detail and actions.

### Task Detail Card and Actions

Proposed task:

```text
Review release smoke output
Proposed - 25 PFT - accept by Jun 30

Objective
Review the release smoke output and identify any blocking deployment issues.

Verification
Submit the smoke command, result, and any linked issue or PR.

> Accept task
  Refuse task
  Copy task brief
  Copy task link
```

Accepted task:

```text
Add bridge route smoke coverage
Accepted - 40 PFT - due Jul 1

Objective
Add smoke coverage for terminal bridge route behavior.

Steps
1. Cover GitHub-linked terminal session issuance.
2. Cover task list/detail reads.
3. Cover wallet-required mutation fallback.

Verification
Submit the smoke command output and changed files.

> Submit evidence
  Cancel task
  Copy task brief
  Copy task link
```

Verification requested:

```text
Submit CLI evidence packet
Verification requested - 15 PFT

Original task
Submit evidence packet support in the terminal.

Current verification request
Please provide the PR URL and terminal output from the route smoke.

> Submit verification evidence
  Copy task brief
  Copy task link
```

Actions should come from the server response where possible. PFTerminal can use
status defaults only as a display fallback:

- `proposed`: `accept`, `refuse`
- `accepted`: `submit_evidence`, `cancel`
- `verification_requested`: `submit_verification_evidence`
- terminal states: view/copy only

The card should appear before action selection. It can be implemented as a
selection view with a rich header and action rows, or as a history-rendered card
followed by an action picker. Either way, the current implementation that shows
only:

```text
Finish NAVCoin Local Wallet Integration
accepted - 2.5 PFT

> Cancel task
  Submit evidence
```

is insufficient because it hides the task contract and leaves evidence
submission ambiguous.

### Evidence Prompt

Evidence entry should use a bottom-pane prompt, not a model chat message.

The evidence prompt must be seeded from the opened task detail and must show the
task requirement above the input. Required visible context:

- task title;
- task ID;
- objective summary;
- steps or a one-line step summary;
- verification requirement;
- current verification request when this is verification-response evidence;
- accepted evidence types;
- examples relevant to the requirement.

Editable fields:

- summary text
- evidence URL
- optional GitHub PR URL
- optional commit SHA
- optional local artifact path for future upload/reference

PFTerminal should create a local draft before submission under:

```text
$PFT_HOME/tasknode/drafts/<task-id>/<timestamp>.json
```

If submission succeeds, record the receipt next to the draft. If submission
fails, keep the draft and show the failure reason.

### Task Request Prompt

`/tasknode request` should open a text prompt:

```text
Request personal task

Describe the work you want Task Node to generate.
Task Node will use your saved context document, account memory, recent chats,
and current task queue.

Examples
- Give me a 2-4 hour engineering task that advances PFTerminal Task Node parity.
- Create a documentation task for the GitHub bridge deployment handoff.
```

The prompt submits `userDetailText` through the terminal task request endpoint.
After submit, PFTerminal shows the request ID, status, and generation state:

```text
Task request recorded
req_...
Queued for generation. Run /tasknode requests to track it.
```

If Task Node returns `wallet_action_required`, PFTerminal must show the handoff
URL and must not request wallet secrets.

### Active Request List

`/tasknode requests` should show active task request rows:

```text
Active Task Requests

> req_abc123  published   terminal Task Node parity   generation scheduled
  req_def456  failed      docs handoff task            needs attention
```

Rows should collapse once a generated offer is visible in the normal task list.

## Local Modules

Recommended PFTerminal module split:

- `codex-rs/tui/src/tasknode.rs`
  - session state
  - client request/response structs
  - menu builders
  - task row formatting
- `codex-rs/tui/src/chatwidget/tasknode_menu.rs`
  - if keeping menu code next to `/vault` and other chat widget popups is a
    better local fit
- `codex-rs/tui/src/tasknode_client.rs`
  - async HTTP client, redaction, retries, idempotency keys
- `codex-rs/tui/src/tasknode_auth.rs`
  - terminal auth start/poll/refresh/logout

The exact filenames should follow the implementation shape, but the boundaries
should stay clear: menu code should not own token storage, and client code
should not print secrets.

## Data Model

### Session

```rust
struct TaskNodeSession {
    origin: String,
    account_id: String,
    github_username: Option<String>,
    access_token_ref: VaultSecretRef,
    refresh_token_ref: Option<VaultSecretRef>,
    expires_at: Option<DateTime<Utc>>,
    scopes: Vec<String>,
}
```

Only secret references belong in normal app state. Raw token values should live
inside the vault or the immediate outbound HTTP request path.

### Task Row

```rust
struct TaskNodeTaskRow {
    task_id: String,
    title: String,
    status_key: String,
    status_label: String,
    kind: Option<String>,
    reward_offer_pft: Option<String>,
    reward_actual_pft: Option<String>,
    accept_by: Option<DateTime<Utc>>,
    deadline_at: Option<DateTime<Utc>>,
    actions: Vec<TaskNodeAction>,
}
```

### Task Detail

```rust
struct TaskNodeTaskDetail {
    task: TaskNodeTaskCard,
    current_verification_request: Option<TaskNodeVerificationRequest>,
    reward_outcome: Option<TaskNodeRewardOutcome>,
    forensics: TaskNodeForensicsSummary,
    actions: TaskNodeActions,
    terminal: Option<TaskNodeTerminalRendering>,
}

struct TaskNodeTaskCard {
    task_id: String,
    title: String,
    kind: String,
    status_key: String,
    status_label: String,
    reward_pft: String,
    due_label: String,
    due_value: String,
    description: String,
    steps: Vec<String>,
    verification_title: String,
    verification_body: String,
    request_id: Option<String>,
    network_project_id: Option<String>,
}
```

### Task Brief

```rust
struct TaskNodeTaskBrief {
    task_id: String,
    display_text: String,
    copy_text: String,
    evidence_prompt: TaskNodeEvidencePrompt,
}
```

`copy_text` should match the web `Copy task brief` contract closely enough that
a user can paste it into Codex or another worker and get the same task context.

### Evidence Draft

```rust
struct TaskNodeEvidenceDraft {
    task_id: String,
    mode: EvidenceMode,
    summary: String,
    evidence: Vec<TaskNodeEvidenceItem>,
    created_at: DateTime<Utc>,
    source: String,
}
```

### Task Request

```rust
struct TaskNodeTaskRequestDraft {
    request_id: String,
    bundle_id: String,
    user_detail_text: String,
    requested_task_kind: String,
    created_at: DateTime<Utc>,
}

struct TaskNodeTaskRequestRow {
    request_id: String,
    status: String,
    user_detail_text: String,
    generated_task_id: Option<String>,
    error: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}
```

## Caching And Background Refresh

Task Node remains the authoritative task store. PFTerminal should not create a
parallel task database or infer task state from local actions.

Recommended terminal cache:

- Store non-secret task list/detail/request snapshots under the PFTerminal data
  directory, keyed by Task Node origin, account ID, and task ID or request ID.
- Do not store terminal bearer tokens in that cache; keep them in `/vault`.
- Cache invalidation key should include `taskId`, `updatedAt`, `lastEventAt`,
  and `forensics.eventCount` when available.
- Opening a task should render immediately from the selected list row, then
  enrich from `GET /api/terminal/tasknode/tasks/:taskId`.
- After loading a task list, PFTerminal may background-prefetch detail for the
  highlighted task and the next few visible rows. It should not prefetch the
  entire account history.
- After any successful action, evidence submission, or task request, refresh
  from Task Node before claiming the state changed.
- If cache and server disagree, show the server state and drop the stale cache
  entry.

This is a stale-while-revalidate UX optimization, not a new local database. The
first parity implementation can render live responses only if latency is
acceptable; background prefetch should be added if the card feels slow.

## Server-Side Requirements

Task Node needs a backend bridge before PFTerminal can complete write actions
without wallet functions.

Required server changes:

1. Add terminal auth start/poll/revoke endpoints that reuse the existing GitHub
   provider identity and account-linking logic.
2. Add bearer-token authentication for terminal sessions or a token-to-session
   adapter before route handlers.
3. Enforce the `github` linked-provider gate before terminal session issuance
   and before terminal task mutations.
4. Add terminal task read endpoints backed by `listTaskState` and
   `getTaskDetail`.
5. Add terminal-shaped task detail fields or a shared task brief formatter so
   PFTerminal can render the same task contract as the web task card without
   reverse-engineering browser presentation code.
6. Add terminal action/evidence endpoints that either:
   - perform account-scoped direct-write transitions when policy allows; or
   - return `wallet_action_required` with a handoff URL.
7. Add terminal task request endpoints that either:
   - reuse the existing direct offchain task request path when policy allows;
   - return `wallet_action_required` when a wallet-signed PFTL request pointer
     is still required; or
   - return `wallet_not_linked` when the account has no linked wallet.
8. Add idempotency handling for task mutations and task requests.
9. Add audit events with:
   - `source: "pfterminal"`
   - account id
   - GitHub provider id
   - task id
   - action/evidence mode
   - idempotency key
   - result
10. Add revocation and expiry for terminal sessions.

The existing `TASKNODE_OFFCHAIN_TASK_LIFECYCLE` flag is relevant but not
sufficient by itself, because the current action helpers still require linked
wallet state before direct-write transitions.

## Security Requirements

- No GitHub access token in model context.
- No Task Node terminal token in model context.
- No authorization header in logs, transcript, debug UI, or tool output.
- No wallet seed, private key, or signing secret prompt in PFTerminal.
- Task mutation receipts must come from Task Node, not local optimistic state.
- PFTerminal can show optimistic loading states, but it must refresh from the
  server after every write.
- Evidence drafts may contain user-authored task content, so they should stay
  under the user-owned PFTerminal data directory and not be attached to model
  prompts unless the user explicitly does so.
- Agent/subagent automation can request a host task action, but the host must
  mediate the token and redact request metadata.

## Implementation Phases

### Phase 0: Bridge Hardening

- Keep the Task Node backend terminal bridge deployed and reviewed.
- Keep the PFTerminal local client from panicking inside the async TUI runtime.
- Always print the terminal GitHub verification URL on headless Linux.
- Preserve terminal session tokens in `/vault` only.
- Confirm production origin defaults to `https://tasknode.postfiat.org`.

### Phase 1: Terminal Task Card Parity

- Expand the Rust task structs to retain description, steps, verification,
  metadata, timestamps, event count, reward outcome, and current verification
  request.
- Make task selection open a terminal card before any action picker.
- Add `Copy task brief` and `Copy task ID` actions.
- Add optional background detail prefetch for highlighted/list-visible tasks if
  live detail fetches are noticeably slow.
- Keep the list row dense but never make it the only task detail surface.

### Phase 2: Evidence Parity

- Seed evidence prompts from the opened task detail.
- Show objective, steps summary, verification requirement, and current
  verification request while the user types evidence.
- Support text and URL evidence first.
- Add GitHub PR/commit normalization after the task-aware prompt works.
- Store local drafts and receipts under the PFTerminal data directory.
- Refresh task detail and list after successful submission.

### Phase 3: Text Task Requests

- Add `/tasknode request` prompt and `/tasknode request <text>` inline flow.
- Add `/tasknode requests` active request list.
- Add terminal Task Node request endpoints or wrappers around
  `taskRequestAction`.
- Use direct offchain request recording only when Task Node policy allows it.
- Return wallet-required handoff for deployments that still require signed PFTL
  task request pointers.
- Show request state until a generated offer appears as a task card.

### Phase 4: Agent Integration

- Allow managed agents to propose Task Node actions through host-mediated
  events.
- Keep terminal session tokens host-only.
- Record receipts and evidence drafts as artifacts the supervising pane can
  inspect.
- Allow agents to draft task request text, but require the terminal host to
  submit through the same `/tasknode request` path.

## Acceptance Criteria

1. Running `/tasknode` opens a Task Node menu.
2. A user can link GitHub through Task Node and PFTerminal stores only a Task
   Node terminal session token.
3. Task Node refuses terminal session issuance and terminal task mutations for
   accounts without a persisted linked GitHub provider.
4. `/tasknode status` shows account, GitHub username, linked-wallet status, and
   task counts.
5. `/tasknode tasks` shows outstanding tasks with stable row layout.
6. Selecting a task opens a terminal-native task card with title, full task ID,
   status, kind, reward, due value, objective, steps, verification requirement,
   current verification request when present, and forensics summary.
7. The task card exposes `Copy task brief` with enough context to paste into
   Codex or another worker.
8. Accept/refuse/cancel only reports success when the Task Node server returns a
   successful receipt.
9. Evidence submission shows the task's verification requirement before the
   input, stores a local draft, and sends evidence only through the Task Node
   bridge.
10. `/tasknode request` accepts free-form text and records a personal task
    request when Task Node policy allows terminal direct-write requests.
11. `/tasknode requests` shows active request status until the generated offer
    appears as a normal task card or the request enters a needs-attention state.
12. Wallet-required tasks and task requests produce a clear web handoff and no
    terminal wallet prompt.
13. Balance and recent rewards are read-only and work without wallet unlock when
    an account-linked wallet exists.
14. Seed phrases, private keys, GitHub tokens, terminal tokens, and
    authorization headers do not appear in transcripts, logs, or model-visible
    tool output.

## Tests

### PFTerminal

- Slash command enum/description/inline-arg coverage for `/tasknode`.
- Dispatch tests for bare and inline `/tasknode` commands.
- Menu snapshot or unit coverage for no-session, linked, no-wallet, and
  wallet-required states.
- Task card rendering coverage for proposed, accepted, verification-requested,
  rewarded, and no-action tasks.
- Evidence prompt coverage that asserts objective, verification requirement,
  and current verification request are visible before submission.
- Task request prompt and active request list coverage.
- Client tests for auth polling, token refresh, redaction, idempotency keys, and
  structured error handling.
- Evidence draft tests for local write, retry after failure, and receipt write.

### Task Node

- Terminal auth start/poll/revoke route smoke tests.
- GitHub callback links to the same account cloud used by the web app.
- Terminal session issuance fails with `github_not_linked` when the resolved
  account lacks a linked GitHub provider.
- Terminal task mutations fail with `github_not_linked` if the provider link is
  removed or revoked after session issuance.
- Terminal bearer auth maps to `session.accountId` without exposing GitHub
  tokens.
- Task list/detail endpoints match the existing web read model.
- Terminal task detail includes either structured card fields or
  `terminal.briefText` matching the web `Copy task brief` contract.
- Action/evidence endpoints return either a direct-write receipt or
  `wallet_action_required`.
- Terminal task request endpoints return recorded request state when direct
  offchain request recording is allowed, and return `wallet_action_required` or
  `wallet_not_linked` when it is not.
- Active request endpoints mirror visible rows from `GET /api/tasks/requests`.
- Audit events include source, account, provider, task, action, idempotency key,
  and result.

## Open Questions

- Should GitHub-authenticated direct-write actions apply to every task the
  account can see, or only network/offchain tasks?
- How should Task Node expose tasks to GitHub-only accounts when current task
  projections are wallet-subject scoped?
- Should PFTerminal support multiple Task Node origins at once?
- What is the terminal session TTL and refresh-token rotation policy?
- What is the revocation UI in the Task Node web app?
- Should evidence artifacts be uploaded to Task Node in v0, or should
  PFTerminal submit only text and URLs until artifact storage is finalized?
- Should the task brief formatter move from frontend-only code into a shared
  module or should the terminal API return a server-built `terminal.briefText`?
- Should terminal task requests be limited to personal tasks forever, or can
  Task Node safely expose an explicit terminal-routed Network Task request
  policy later?
