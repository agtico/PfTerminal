# PFTerminal upstream Codex 0.147.0 upgrade

## Release and integration strategy

- Selected upstream release: `rust-v0.147.0` (released 2026-08-06).
- Upstream commit: `be6e8eac029b183056b7e4402879f15d2c85f61b`.
- PFTerminal upgrade branch: `upgrade/codex-0.147.0`.
- Strategy: a dedicated non-fast-forward merge of the signed upstream tag into the PFTerminal branch, preserving both histories. The pre-upgrade merge base is `413492cd6c3a4d4f8dff6f406247ccda5a9d88aa`; the selected release contributes 192 upstream commits after that base.
- PFTerminal remains version `0.1.27`. Product identity, `pfterminal` command names, install behavior, and `~/.pfterminal` / `PFTERMINAL_HOME` state isolation remain fork-owned.

## Conflict hotspots and retained behavior

The merge initially reported 71 conflicted paths. The most consequential resolutions were:

- **Session/runtime configuration:** integrated upstream permission profiles, environment-aware session settings, provider-auth ownership, service tiers, thread pagination, and resume behavior with PFTerminal's provider/model persistence.
- **Provider and model plumbing:** retained the PFTerminal provider catalog and `model_specialty` routing while integrating upstream `RemoteCompactionSupport`, capability discovery, and the revised `ModelClient` construction. Runtime providers now use the session auth manager rather than a disconnected auth snapshot.
- **Agent orchestration:** integrated upstream spawn/request types with PFTerminal's durable graph, mailbox, agent roles, plaintext adapters, pane restoration, and provider capability filtering. Namespace-wrapped tools remain suppressed only where a provider cannot represent them; plain collaboration tools remain available.
- **App server and protocol:** integrated thread sections, plugin search, client MCP extensions, history pagination, permission-profile resume, and new generated schemas while preserving PFTerminal extensions.
- **TUI:** integrated upstream resize/history, lifecycle, telemetry, and shortcut changes with `/vault`, `/tasknode`, `/spawn`, `/usage`, GPU workflows, provider selectors, and retained pane layouts.
- **Local services:** retained encrypted vault storage, Task Node sessions, wallet daemon, GPU market, Telegram, provider bridge services, and the dedicated `pfterminal_queue_1.sqlite` queue (with legacy queue migration support).
- **Release/build tooling:** kept PFTerminal packaging and branding while adopting upstream Rusty V8 150.4 build/release changes and refreshed app-server/config schemas.

No upstream subsystem is disabled wholesale. Upstream OpenAI-only defaults are intentionally superseded where they conflict with PFTerminal's multi-provider catalog, PFTerminal product naming, or isolated state directory. The upstream model catalog is not allowed to replace the PFTerminal catalog; all other compatible protocol and runtime features are integrated.

## Build and test results

Passed:

- `cargo check -p codex-core -p codex-app-server -p codex-tui -p codex-cli`
- test-target compilation for `codex-core`, `codex-app-server`, `codex-tui`, and `codex-cli`
- `just write-app-server-schema` (schema fixture check and regeneration)
- `just write-config-schema`
- `just fix -p codex-core -p codex-app-server -p codex-tui -p codex-cli`
- `just fmt`
- full workspace build with the official `rusty-v8-v150.4.0` release archive and binding source supplied through `RUSTY_V8_ARCHIVE` and `RUSTY_V8_SRC_BINDING_PATH`
- focused service run: 244/247 tests passed on the first run; the remaining three encrypted Task Node session tests exceeded the generic 60-second ceiling. The encrypted-vault test group is now serialized with a three-minute ceiling, and all three reruns passed in 66.786s, 84.951s, and 65.761s.
- focused core integration: 2/2 passed (`spawn_agent_uses_explorer_role_and_preserves_approval_policy`, provider namespace flattening).
- focused app-server integration: 3/3 passed (provider capability profiles, Ambient default catalog, persisted permission-profile resume).
- focused TUI integration: 6/6 passed (`/vault`, `/spawn`, `/usage`, GPU control events, Task Node stream parsing, persisted-model resume).
- provider/service packages: the initial combined run covered 247 tests across Task Node session, vault, wallet, wallet daemon, GPU market, model provider, and provider-info; after the timeout-budget correction, every observed failure case passed.

The repository requires separate explicit approval before running the complete `just test` suite. That approval was requested during this upgrade and was not yet received, so the complete suite was not represented as run. The targeted suites above cover the conflict-heavy runtime and every retained custom service.

The default Rusty V8 archive URL returned HTTP 404 during the first workspace build. The build passed with the two official assets published on the OpenAI Codex `rusty-v8-v150.4.0` release and verified against the release checksum manifest:

```text
librusty_v8_ptrcomp_sandbox_release_x86_64-unknown-linux-gnu.a.gz
src_binding_ptrcomp_sandbox_release_x86_64-unknown-linux-gnu.rs
```

## Fresh TUI and service smoke (2026-08-08 UTC)

The freshly rebuilt `target/debug/pfterminal` reported `pfterminal 0.1.27` and started in a 160x48 tmux pane against the isolated PFTerminal debug home.

| Surface | Result |
| --- | --- |
| Ambient default provider | PASS — status line showed `zai-org/GLM-5.2-FP8 xhigh` with PFTerminal branding; no model inference was sent. |
| `/vault` | PASS — encrypted Vault action menu rendered and loaded credential metadata without revealing secrets. |
| `/tasknode` | PASS — Task Node menu rendered against `https://tasknode.postfiat.org` and recognized the linked `goodalexander` session. |
| `/spawn` | PASS — role picker rendered the standard crew plus Nazgul, Troll, Orc, and status actions. No agents were spawned. |
| `/usage` | PASS — usage menu rendered account usage and reset availability. |
| GPU rental | PASS (safe pre-charge smoke) — `/gpu` rendered qualified and experimental rental recipes plus masked credential setup. The smoke stopped before provider search/charge confirmation, so it created no rental and spent no funds. The focused TUI test also passed durable GPU control-event dispatch. |
| Wallet daemon | PASS — `pfterminal-walletd` created `wallet/run/walletd.sock` with mode `0600`; a second daemon was rejected with `another wallet daemon owns this PfTerminal home`, and the live socket remained intact. The isolated smoke home was then removed. |

## Residual risks

- The full `just test` suite remains pending explicit operator approval; conflict-focused and custom-service suites are green.
- Rusty V8's default archive resolution was unavailable at test time. Release automation already carries upstream V8 asset support, but local builders may need the documented archive/binding environment variables until the default URL is restored.
- This is a large upstream merge (1,000+ touched paths). Compile gates, schema gates, and focused tests cover the conflict hotspots, but platform-specific Windows/macOS release paths were not exercised on this Linux host.
- GPU rental was deliberately stopped before any paid mutation. Recipe rendering and control dispatch are verified; live provider capacity and billing were not changed.
- Existing compiler warnings remain in upstream/PFTerminal compatibility code; no new warning was treated as a release blocker in this merge.
