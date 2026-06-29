# Task Node Submission Templates

Use these templates when drafting live Task Node submissions. Keep them compact; include only relevant sections.

## Task Request

```text
Objective:
<One concrete outcome.>

Context:
<Why this matters, current repo/product/user context, relevant constraints.>

Scope:
<What should be included and excluded.>

Expected Deliverable:
<PR, code change, spec, investigation note, terminal verification, etc.>

Acceptance Criteria:
- <Observable criterion 1>
- <Observable criterion 2>
- <Observable criterion 3>

Evidence Plan:
<What evidence should be submitted if the task is completed.>
```

## Initial Evidence

```text
Summary:
<What was completed.>

Artifacts:
- <PR URL, commit hash, file path, task id, screenshot, route probe, or generated text.>

Verification:
- <Command/check/probe and result.>

Requirement Mapping:
- <Task requirement> -> <proof it was satisfied>

Residual Risk:
<None, or honest limitations/follow-up needed.>
```

## Verification Response

```text
Verification Request:
<Restate the verifier's specific ask in one sentence.>

Response:
<Directly answer the ask. If they requested complete text, paste the complete text.>

Evidence:
- <Artifact or command output summary>
- <PR/commit/file reference if applicable>

Pass/Fail:
<Explicit pass/fail or partial status, with one or two sentences explaining why.>
```

## Context Document Edit Summary

Show this to the user after saving a context edit:

```diff
Context document saved.

- <old line or block removed>
+ <new line or block added>
```

If the change is an insertion, show enough unchanged surrounding heading context to make the insertion clear.

## Task Node Chat Prompt

```text
I need a Task Node context judgment.

Decision:
<Choice to make.>

Current context:
<Relevant facts from this work session.>

Options:
1. <Option A and tradeoff>
2. <Option B and tradeoff>

Constraints:
<Time, reward, user preference, product direction, risk.>

Please recommend the best next action and name any critical caveat.
```
