# Current Security Sprint

The only active PFTerminal sprint material in this folder is security work for
provider API-key containment.

The vault already exists and is documented in
[Authentication And Vault](../authentication.md). The current security work is
narrower: agent/pane processes must be able to use provider credentials without
inheriting or exposing raw long-lived secrets.

## Active Security Scope

| Area | Current State | Where To Read |
| ---- | ------------- | ------------- |
| Agent vault access | Active security design for letting agents, subagents, and Claude panes use provider credentials without reading raw vault records or inheriting long-lived API-key environment variables. | [Agent Vault Access](agent-vault-access.md) |

## Reading Path

1. Read [Authentication And Vault](../authentication.md) for the already-shipped
   `/vault` command surface and local credential-store behavior.
2. Read [Agent Vault Access](agent-vault-access.md) for the provider-secret
   containment model for agent and pane execution.

## Boundary

This folder is not a general backlog. Shipped feature documentation belongs in
the [Features](../features/index.md) section, and deferred/non-feature planning
notes belong in [TBD](../TBD/index.md).
