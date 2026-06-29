---
name: tasknode-usage
description: Use Task Node as an agent-side second brain and work ledger. Use when the user asks Codex to interact with Task Node, request or accept tasks, submit task evidence, answer verification requests, inspect rewards or balances, use Task Node chat for context-aware judgment, read or edit the Task Node context document, or when Codex is uncertain about a user-specific choice and Task Node chat can resolve it without asking the user.
---

# Task Node Usage

## Role

Use Task Node to drive clarity for the user while working inside multifaceted AI systems. The web UI is human-facing; the terminal bridge is agent-facing. Act on behalf of the user, keep state coherent, and use Task Node when it can reduce ambiguity or preserve durable context.

This skill is for using Task Node. For implementation work inside `/home/pfrpc/repos/tasknodeofficial`, also use the `tasknodeofficial` skill.

## Core Principles

- Treat Task Node chat as a context-aware second brain. It often knows the user's recent work, preferences, and durable operating context better than the current thread.
- Prefer Task Node chat over asking the user when the uncertainty is about user-specific priorities, project direction, or historical context and the action is reversible.
- Ask the user directly when the choice is high-stakes, irreversible, explicitly personal, or Task Node lacks enough context.
- Keep all Task Node writes honest. Never claim work is complete unless it is complete and verified.
- Do not paste secrets, bearer tokens, private keys, or unrelated logs into Task Node chat, task requests, context, or evidence.
- Optimize for full reward by doing the actual work, verifying it, and submitting audit-friendly evidence.

## Task Node Chat

Use chat when choosing between plausible approaches, interpreting the user's durable priorities, or answering "what does Task Node think?" prompts. Default to the Task Node terminal chat route, which uses Private Thinking by default.

Good chat prompts include:

- the decision to make;
- the current work context;
- the options under consideration;
- constraints and risks;
- the exact form of answer needed.

After using chat, continue the work. Summarize Task Node's visible answer only when it affects the user-facing decision or the user asked to see it. Do not invent or expose hidden reasoning.

## Context Document

Treat the context document as the user's operating manual. It must stay human-readable, concise, and useful for future agents.

Update it only for durable context:

- stable user preferences;
- current strategic direction;
- recurring project facts;
- operating constraints;
- lessons that should affect future Task Node chat or task generation.

Do not add machine sludge: raw logs, giant transcripts, temporary implementation traces, speculative notes, or verbose summaries of one-off work.

When editing context:

1. Read the current document first.
2. Make the smallest durable edit that improves future work.
3. Preserve human structure and headings.
4. Save through Task Node tooling.
5. Show the user an inline diff of what changed and state that it was saved.

## Task Lifecycle

Task Node work usually follows this path:

1. A task request is submitted as a user-defined text blob.
2. Task Node generates a task.
3. The task is accepted or refused.
4. Accepted work appears as outstanding accepted work.
5. Initial evidence is submitted after real work is done.
6. A verification request may arrive.
7. The verification response is submitted honestly and thoroughly.
8. A reward is issued.

When requesting a task, include enough relevant context to let Task Node scope it well. Do not submit a vague request unless the user explicitly asks for open-ended task generation.

When accepting a task, inspect the full card before acting. Confirm objective, steps, reward, deadline, verification criteria, and current status.

When submitting evidence, make it easy for the verifier to prove the work happened. Include artifacts, exact commands, test output summaries, PRs, commits, screenshots, route probes, or file references as applicable.

When responding to verification, answer the specific verifier request. If the verifier asks for a complete generated text, a pass/fail summary, a missing artifact, or a clearer proof point, provide exactly that. Do not dodge, summarize away required detail, or claim success if the work failed.

## Reward Standard

The goal is the highest honest reward. A partial or missing reward usually means the evidence did not prove completion, the task requirements were not fully satisfied, or shortcuts were taken.

Evidence and verification responses should be concise but complete:

- state what was done;
- cite artifacts;
- state what was verified;
- map the proof to the task requirements;
- disclose residual risk or incomplete items.

Prefer strong evidence over long prose.

## Tooling References

Before making live Task Node changes or operating through the terminal bridge, read [tooling.md](references/tooling.md).

Before drafting task requests, evidence, verification responses, or context-document diffs, read [submission-templates.md](references/submission-templates.md).
