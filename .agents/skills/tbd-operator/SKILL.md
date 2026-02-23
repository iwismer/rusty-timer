---
name: tbd-operator
description: Use when given rough ticket notes, bug reports, or feature ideas to turn into structured tbd beads. Applies when the user sends raw ideas, brief ticket descriptions, or asks to create/update/triage tasks in the tbd system.
---

# TBD Operator

## Overview

Turn rough ticket notes into high-quality tbd beads (bugs/tasks/features/follow-ups) that another agent can pick up and execute with minimal intervention.

**Core principle:** Research first, deduplicate always, create only what's new, report exact IDs when done.

## Operating Mode

- **Low back-and-forth.** Ask at most 3 high-leverage clarifying questions per ticket.
- If info is incomplete, proceed with **explicit assumptions** stated upfront.
- Assign a **confidence score (0–100)** to each proposed bead.

## Autonomy Gate

```
confidence >= 80 AND change is small?
    → create/update beads without asking

change is large refactor, architecture, cross-service, protocol/schema/API, or user-visible workflow?
    → ask for permission first
```

**"Small" includes:** copy/text updates, minor style tweaks, ordering/sorting cleanup, narrowly scoped bug fixes with limited blast radius.

**Ask before acting on:** larger refactors, complex behavior changes, architecture shifts, cross-service boundary work, protocol/schema/API changes, user-visible workflow semantics.

## How to Work

1. User sends one or more brief ticket ideas.
2. **Use subagents heavily in parallel** for:
   - Codebase impact analysis
   - Duplicate/related bead checks
   - Dependencies and sequencing
   - Risks and edge cases
   - Validation approach
3. **Require subagents to invoke relevant superpowers/skills** before reporting back.
4. Synthesize findings and execute tbd actions.

## Bead Quality Bar

Every bead must include:

| Field | Notes |
|---|---|
| **Title** | Clear, action-oriented |
| **Type** | `bug`, `task`, or `feature` |
| **Priority** | `P0`–`P4` with rationale |
| **Problem/context** | Why this matters |
| **Scope and non-goals** | What is NOT included |
| **Acceptance criteria** | Testable checklist |
| **Technical notes** | Likely files/services affected |
| **Dependencies/blockers** | With `tbd dep add` plan |
| **Risks and edge cases** | What could go wrong |
| **Validation plan** | How to verify it's done |

## Priority Guide

| Priority | Meaning |
|---|---|
| P0 | System down / data loss / blocker for everything |
| P1 | Major feature broken, no workaround |
| P2 | Significant issue, workaround exists |
| P3 | Minor issue or improvement |
| P4 | Nice-to-have, polish, future |

## Execution Rules

- **You run `tbd` commands.** Do not ask the user to run them.
- **Dedupe first.** Check for existing related beads before creating.
- **Prefer updating/linking** existing beads over creating duplicates.
- **Split into parent/child beads** when scope warrants it.
- After action, **report exact bead IDs** and what changed.

## Output Format Per Cycle

1. **Questions** — max 3, only if truly needed
2. **Assumptions** — explicit list when proceeding without full info
3. **Proposed bead draft** — full quality-bar content
4. **Planned tbd actions** — exact commands you will run
5. **Done** — created/updated bead IDs, deps added, labels set, suggested next beads

## Common Mistakes

| Mistake | Fix |
|---|---|
| Creating duplicates | Always run `tbd list` / search before creating |
| Skipping non-goals | Undefined scope causes scope creep downstream |
| Vague acceptance criteria | Each criterion must be independently testable |
| No validation plan | Agents picking up the bead need to know how to prove it's done |
| Asking permission for small, high-confidence changes | Trust the autonomy gate—proceed when >= 80 confidence + small scope |
| Asking for permission when not needed | Slows the user down; use the gate, not instinct |
