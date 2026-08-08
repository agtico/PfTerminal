# Task Node Tooling

Use this reference before making live Task Node calls or operating Task Node through PFTerminal.

## Current Terminal Surface

PFTerminal exposes Task Node through slash commands:

- `/tasknode` - open the Task Node menu.
- `/tasknode link` - link GitHub / Task Node.
- `/tasknode status` - show linked account and counts.
- `/tasknode tasks` or `/tasknode outstanding` - list outstanding accepted tasks.
- `/tasknode tasks <tab>` - open a server-backed task tab when supported.
- `/tasknode task <task-id>` - open a task action/detail view.
- `/tasknode verification` - list verification requests.
- `/tasknode refused` - list refused tasks.
- `/tasknode rewarded` - list rewarded tasks.
- `/tasknode request` - open a task-request prompt.
- `/tasknode request <text>` - submit a task request directly.
- `/tasknode context` - view or edit the context document.
- `/tasknode chat` - open Task Node chat threads.
- `/tasknode chat <message>` - start a new Private Thinking chat with a message.
- `/tasknode requests` - list active task-generation requests.
- `/tasknode balance` - show linked-wallet PFT balance.
- `/tasknode rewards` - show recent rewards.
- `/tasknode logout` - remove the local Task Node terminal session.

Follow the TUI footer for exact keybindings. Multiline prompts may use a submit key such as `Ctrl-D` so Enter can insert a newline.

## JSON Helper

Prefer the JSON helper for agent work.

**Resolve the helper binary first.** PFTerminal entrypoints pin isolated state
homes: `pfterminal` always uses `~/.pfterminal`, and `pfterminal-debug` always
uses `~/.pfterminal-debug`, regardless of the shell's `CODEX_HOME`. Calling the
wrong entrypoint queries a different vault than the running session and reports
"not linked" even when the session is linked. Pick the binary that matches the
session's home before any Task Node call:

```bash
PF_BIN=pfterminal
case "${CODEX_HOME:-}" in
  *.pfterminal-debug) PF_BIN=pfterminal-debug ;;
esac
"$PF_BIN" tasknode status --json
```

Every `pfterminal tasknode` command below means `"$PF_BIN" tasknode`.

```bash
pfterminal tasknode status --json
pfterminal tasknode chat list --json
pfterminal tasknode chat history <conversation-id> --json
pfterminal tasknode chat search "<query>" --json
pfterminal tasknode chat send --message "<text>" --json
pfterminal tasknode chat send --stream --message "<text>" --json
pfterminal tasknode context get --json
pfterminal tasknode context save --body-file <path> --revision <n> --json
pfterminal tasknode request create --body-file <path> --json
pfterminal tasknode requests list --json
pfterminal tasknode requests show <request-id> --json
pfterminal tasknode tasks list --tab outstanding --json
pfterminal tasknode task show <task-id> --json
pfterminal tasknode task accept <task-id> --json
pfterminal tasknode task refuse <task-id> --reason-file <path> --json
pfterminal tasknode task evidence <task-id> --body-file <path> --json
pfterminal tasknode verification respond <task-id> --body-file <path> --json
pfterminal tasknode rewards list --json
pfterminal tasknode balance --json
```

The helper reuses the same PFTerminal vault session as the TUI and never prints the bearer token. Non-streaming commands emit one JSON object. Streaming chat emits JSON lines for SSE events when the backend streams; dry-run or preflight responses may return one normal JSON object.

## Evidence Lifecycle Gate

Initial evidence and verification response are different state transitions and use different commands:

```bash
pfterminal tasknode task show <task-id> --json
pfterminal tasknode task evidence <task-id> --body-file <path> --json
pfterminal tasknode task show <task-id> --json
pfterminal tasknode verification respond <task-id> --body-file <path> --json
pfterminal tasknode task show <task-id> --json
```

The evidence commands preflight the server-reported task actions and reject a mode mismatch. Successful receipts include `pfterminalLifecycle` with `completionConfirmed`, the current phase, and `nextCommand`. Always follow that command. After initial evidence, respond when `actions.canSubmitVerificationEvidence` becomes true. After verification, confirm `rewardOutcome` or an explicit `rewarded` state before reporting completion.

If verification has not materialized yet, query `pfterminal tasknode tasks list --tab verification --json`. Keep the task pending rather than treating the initial evidence receipt as completion.

Use `--origin <url>` only for explicit local/dev testing. Production defaults to `https://tasknode.postfiat.org` unless the environment or saved session overrides it.

## Agent Operation Pattern

Use a real PFTerminal tmux session for UI-only flows, visual verification, or interactions not yet exposed by the JSON helper.

Recommended tmux pattern:

```bash
tmux new-session -d -s tasknode-work -x 160 -y 48 'cd /home/pfrpc/repos && pfterminal --yolo'
tmux send-keys -t tasknode-work '/tasknode chat' Enter
tmux capture-pane -t tasknode-work -p -S -120
tmux kill-session -t tasknode-work
```

For long or multiline text, avoid typing through shell history. Use tmux paste buffers or the TUI prompt safely, then capture the screen to verify the result.

Do not print or persist the terminal bearer token. Do not manually copy secrets from the vault into chat history or command output.

## Direct HTTP Calls

Direct HTTP calls are acceptable only when the token is retrieved by approved local tooling and is not printed. The default production origin is:

```text
https://tasknode.postfiat.org
```

The terminal bridge requires GitHub-linked terminal auth. If a route returns `401`, link or relink Task Node from PFTerminal.

## Helper Behavior Expectations

The helper should keep returning bounded JSON errors, redact tokens by construction, and include server receipt fields such as request IDs, task IDs, or receipt IDs whenever the backend returns them.
