# AGENTS.md — Instructions for AI coding agents

## Agent Notes

- Use `uv` to run Python commands in this workspace.
- Examples:
  - `uv run scripts/dev.py --clear`
  - `uv run --with rich --with iterm2 python -m unittest scripts/tests/test_dev.py`

## Repository Overview

This is the **Rusty Timer Remote Forwarding Suite**, a multi-service Rust workspace with two SvelteKit frontend apps.

### Components
- `services/streamer/` — Connects to IPICO readers, fans out TCP to local clients
- `services/emulator/` — Simulates IPICO reads for development/testing
- `services/forwarder/` — Reads from IPICO hardware, journals to SQLite, forwards over WebSocket
- `services/server/` — Axum/Postgres: ingest, dedup, fanout, dashboard API
- `services/receiver/` — Windows app: subscribes to server, proxies streams to local TCP ports
- `apps/server-ui/` — SvelteKit static web dashboard (served by the server)
- `apps/receiver-ui/` — SvelteKit static frontend for the receiver (embedded in binary via `--features embed-ui`)
- `crates/rt-protocol/` — Frozen WebSocket message types (WsMessage enum)
- `crates/ipico-core/` — Frozen IPICO chip read parser
- `crates/emulator/` — Emulator library: read generation, scenarios, fault injection
- `crates/rt-test-utils/` — MockWsServer + MockWsClient test helpers

### Key Decisions
- Rust MSRV: 1.85.0; pinned toolchain: 1.93.1 (see `rust-toolchain.toml`)
- Node 24.x / npm 11.x (see root `package.json` + `.nvmrc`)
- Server config: env vars only (`DATABASE_URL`, `BIND_ADDR`, `LOG_LEVEL`)
- Forwarder config: TOML only (no env var overrides)
- sqlx 0.8 offline cache at `services/server/.sqlx/`
- Event delivery: at-least-once; deduplicated by `(forwarder_id, reader_ip, stream_epoch, seq)`

## Git Hooks Setup (run once per clone)

```bash
git config core.hooksPath .githooks
```

The pre-commit hook automatically:
1. Strips registry URL `"resolved"` fields from all `package-lock.json` files (root and `apps/*/`), while keeping local workspace `"resolved"` paths
2. Checks Rust formatting: `cargo fmt --all -- --check`
3. Runs Clippy: `cargo clippy --workspace --all-targets`
4. For touched frontend apps, runs `npm run lint` and `npm run check` (blocking)

To run the pre-commit hook manually before committing:
```bash
bash .githooks/pre-commit
```

## Running Tests

```bash
# All Rust unit tests (no Docker needed)
cargo test --workspace --lib

# All tests including integration (Docker required)
cargo test --workspace -- --test-threads=4

# Dashboard unit tests
cd apps/server-ui && npm test

# Packaging validation
bash scripts/validate-packaging.sh
```

## Code Quality

```bash
# Format Rust
cargo fmt --all

# Lint Rust
cargo clippy --workspace --all-targets

# Format JS/TS
cd apps/server-ui && npm run format
cd apps/forwarder-ui && npm run format
cd apps/receiver-ui && npm run format
```

## Important Notes

- Integration tests require Docker (for Postgres via testcontainers-rs)
- Never commit without running `bash .githooks/pre-commit` first
- The `.sqlx/` offline cache is at `services/server/.sqlx/` — regenerate with `cargo sqlx prepare` if schema changes
- `docs/plans/` is gitignored; all other docs (runbooks, specs, guides) are tracked
- Clippy is configured with `pedantic = warn` at the workspace level (see `Cargo.toml` `[workspace.lints.clippy]`)
- **Never commit `package-lock.json` files with registry URL `"resolved"` fields** — they leak internal registry URLs and bloat diffs. Keep local workspace path `"resolved"` fields (for workspace links). The pre-commit hook handles this automatically, but if you bypass hooks, clean manually with: `jq 'walk(if type == "object" then with_entries(select(.key != "resolved" or (.value | type) != "string" or (.value | test("^https?://") | not))) else . end)' package-lock.json > /tmp/clean.json && mv /tmp/clean.json package-lock.json`


<!-- BEGIN TBD INTEGRATION -->
---
title: tbd Workflow
description: Full tbd workflow guide for agents
---
**`tbd` helps humans and agents ship code with greater speed, quality, and discipline.**

1. **Beads**: Git-native issue tracking (tasks, bugs, features).
   Never lose work across sessions.
   Drop-in replacement for `bd`.
2. **Spec-Driven Workflows**: Plan features → break into beads → implement
   systematically.
3. **Knowledge Injection**: 17+ engineering guidelines (TypeScript, Python, TDD,
   testing, Convex, monorepos) available on demand.
4. **Shortcuts**: Reusable instruction templates for common workflows (code review,
   commits, PRs, cleanup, handoffs).

## Installation

```bash
npm install -g get-tbd@latest
tbd setup --auto --prefix=<name>   # Fresh project (--prefix is REQUIRED: 2-8 alphabetic chars recommended. ALWAYS ASK THE USER FOR THE PREFIX; do not guess it)
tbd setup --auto                   # Existing tbd project (prefix already set)
tbd setup --from-beads             # Migration from .beads/ if `bd` has been used
```

## Routine Commands

```bash
tbd --help    # Command reference
tbd status    # Status
tbd doctor    # If there are problems

tbd setup --auto   # Run any time to refresh setup
tbd prime      # Restore full context on tbd after compaction
```

## CRITICAL: You Operate tbd — The User Doesn’t

**You are the tbd operator:** Users talk naturally; you translate their requests to tbd
actions. DO NOT tell users to run tbd commands.
That’s your job.

- **WRONG**: "Run `tbd create` to track this bug"

- **RIGHT**: *(you run `tbd create` yourself and tell the user it’s tracked)*

**Welcoming a user:** When users ask “what is tbd?”
or want help → run `tbd shortcut welcome-user`

## User Request → Agent Action

| User Says | You (the Agent) Run |
| --- | --- |
| **Issues/Beads** |  |
| "There's a bug where ..." | `tbd create "..." --type=bug` |
| "Create a task/feature for ..." | `tbd create "..." --type=task` or `--type=feature` |
| "Let's work on issues/beads" | `tbd ready` |
| "Show me issue X" | `tbd show <id>` |
| "Close this issue" | `tbd close <id>` |
| "Search issues for X" | `tbd search "X"` |
| "Add label X to issue" | `tbd label add <id> <label>` |
| "What issues are stale?" | `tbd stale` |
| **Planning & Specs** |  |
| "Plan a new feature" / "Create a spec" | `tbd shortcut new-plan-spec` |
| "Break spec into beads" | `tbd shortcut plan-implementation-with-beads` |
| "Implement these beads" | `tbd shortcut implement-beads` |
| **Code Review & Commits** |  |
| "Review this code" / "Code review" | `tbd shortcut review-code` |
| "Review this PR" | `tbd shortcut review-github-pr` |
| "Commit this" / "Use the commit shortcut" | `tbd shortcut code-review-and-commit` |
| "Create a PR" / "File a PR" | `tbd shortcut create-or-update-pr-simple` |
| "Merge main into my branch" | `tbd shortcut merge-upstream` |
| **Guidelines & Knowledge** |  |
| "Use TypeScript best practices" | `tbd guidelines typescript-rules` |
| "Use Python best practices" | `tbd guidelines python-rules` |
| "Build a TypeScript CLI" | `tbd guidelines typescript-cli-tool-rules` |
| "Improve monorepo setup" | `tbd guidelines pnpm-monorepo-patterns` or `bun-monorepo-patterns` |
| "Add golden/e2e testing" | `tbd guidelines golden-testing-guidelines` |
| "Use TDD" / "Test-driven development" | `tbd guidelines general-tdd-guidelines` |
| "Convex best practices" | `tbd guidelines convex-rules` |
| **Documentation** |  |
| "Research this topic" | `tbd shortcut new-research-brief` |
| "Document architecture" | `tbd shortcut new-architecture-doc` |
| **Cleanup & Maintenance** |  |
| "Clean up this code" / "Remove dead code" | `tbd shortcut code-cleanup-all` |
| "Fix repository problems" | `tbd doctor --fix` |
| **Sessions & Handoffs** |  |
| "Hand off to another agent" | `tbd shortcut agent-handoff` |
| "Check out this library's source" | `tbd shortcut checkout-third-party-repo` |
| *(your choice whenever appropriate)* | `tbd list`, `tbd dep add`, `tbd close`, `tbd sync`, etc. |

**Note:** Never gitignore `.tbd/workspaces/` — the outbox must be committed to your
working branch. See `tbd guidelines tbd-sync-troubleshooting` for details.

## CRITICAL: Session Closing Protocol

**Before saying “done”, you MUST complete this checklist:**

```
[ ] 1. git add + git commit
[ ] 2. git push
[ ] 3. gh pr checks <PR> --watch 2>&1 (IMPORTANT: WAIT for final summary, do NOT tell user it is done until you confirm it passes CI!)
[ ] 4. tbd close/update <id> for all beads worked on
[ ] 5. tbd sync
[ ] 6. CONFIRM CI passed (if failed: fix, run tests, re-push, restart from step 3)
```

**Work is not done until pushed, CI passes, and tbd is synced.**

## Bead Tracking Rules

- Track all task work not done immediately as beads (discovered work, TODOs,
  multi-session work)
- When in doubt, create a bead
- Check `tbd ready` when not given specific directions
- Always close/update beads and run `tbd sync` at session end

## Commands

### Finding Work

| Command | Purpose |
| --- | --- |
| `tbd ready` | Beads ready to work (no blockers) |
| `tbd list --status open` | All open beads |
| `tbd list --status in_progress` | Your active work |
| `tbd show <id>` | Bead details with dependencies |

### Creating & Updating

| Command | Purpose |
| --- | --- |
| `tbd create "title" --type task\|bug\|feature --priority=P2` | New bead (P0-P4, not "high/medium/low") |
| `tbd update <id> --status in_progress` | Claim work |
| `tbd close <id> [--reason "..."]` | Mark complete |

### Dependencies & Sync

| Command | Purpose |
| --- | --- |
| `tbd dep add <bead> <depends-on>` | Add dependency |
| `tbd blocked` | Show blocked beads |
| `tbd sync` | Sync with git remote (run at session end) |
| `tbd stats` | Project statistics |
| `tbd doctor` | Check for problems |
| `tbd doctor --fix` | Auto-fix repository problems |

### Labels & Search

| Command | Purpose |
| --- | --- |
| `tbd search <query>` | Search issues by text |
| `tbd label add <id> <label>` | Add label to issue |
| `tbd label remove <id> <label>` | Remove label from issue |
| `tbd label list` | List all labels in use |
| `tbd stale` | List issues not updated recently |

### Documentation

| Command | Purpose |
| --- | --- |
| `tbd shortcut <name>` | Run a shortcut |
| `tbd shortcut --list` | List shortcuts |
| `tbd guidelines <name>` | Load coding guidelines |
| `tbd guidelines --list` | List guidelines |
| `tbd template <name>` | Output a template |

## Quick Reference

- **Priority**: P0=critical, P1=high, P2=medium (default), P3=low, P4=backlog
- **Types**: task, bug, feature, epic
- **Status**: open, in_progress, closed
- **JSON output**: Add `--json` to any command

<!-- BEGIN SHORTCUT DIRECTORY -->
## Available Shortcuts

Run `tbd shortcut <name>` to use any of these shortcuts:

| Name | Description |
| --- | --- |
| agent-handoff | Generate a concise handoff prompt for another coding agent to continue work |
| checkout-third-party-repo | Get source code for libraries and third-party repos using git. Essential for reliable source code review. Prefer this to web searches or fetching of web pages from github.com as it is far more effective (github.com blocks web scraping from main website). |
| code-cleanup-all | Full cleanup cycle including duplicate removal, dead code, and code quality improvements |
| code-cleanup-docstrings | Review and add concise docstrings to major functions and types |
| code-cleanup-tests | Review and remove tests that do not add meaningful coverage |
| code-review-and-commit | Run pre-commit checks, review changes, and commit code |
| coding-spike | Prototype to validate a spec through hands-on implementation |
| create-or-update-pr-simple | Create or update a pull request with a concise summary |
| create-or-update-pr-with-validation-plan | Create or update a pull request with a detailed test/validation plan |
| implement-beads | Implement beads from a spec, following TDD and project rules |
| merge-upstream | Merge origin/main into current branch with conflict resolution |
| new-architecture-doc | Create an architecture document for a system or component design |
| new-guideline | Create a new coding guideline document for tbd |
| new-plan-spec | Create a new feature planning specification document |
| new-qa-playbook | Create a QA test playbook for manual validation workflows |
| new-research-brief | Create a research document for investigating a topic or technology |
| new-shortcut | Create a new shortcut (reusable instruction template) for tbd |
| new-validation-plan | Create a validation/test plan showing what's tested and what remains |
| plan-implementation-with-beads | Create implementation beads from a feature planning spec |
| precommit-process | Full pre-commit checklist including spec sync, code review, and testing |
| review-code | Comprehensive code review for uncommitted changes, branch work, or GitHub PRs |
| review-code-python | Python-focused code review (language-specific rules only) |
| review-code-typescript | TypeScript-focused code review (language-specific rules only) |
| review-github-pr | Review a GitHub pull request with follow-up actions (comment, fix, CI check) |
| revise-all-architecture-docs | Comprehensive revision of all current architecture documents |
| revise-architecture-doc | Update an architecture document to reflect current codebase state |
| setup-github-cli | Ensure GitHub CLI (gh) is installed and working |
| sync-failure-recovery | Handle tbd sync failures by saving to workspace and recovering later |
| update-specs-status | Review active specs and sync their status with tbd issues |
| welcome-user | Welcome message for users after tbd installation or setup |

## Available Guidelines

Run `tbd guidelines <name>` to apply any of these guidelines:

| Name | Description |
| --- | --- |
| backward-compatibility-rules | Guidelines for maintaining backward compatibility across code, APIs, file formats, and database schemas |
| bun-monorepo-patterns | Modern patterns for Bun-based TypeScript monorepo architecture |
| cli-agent-skill-patterns | Best practices for building TypeScript CLIs that function as agent skills in Claude Code and other AI coding agents |
| commit-conventions | Conventional Commits format with extensions for agentic workflows |
| convex-limits-best-practices | Comprehensive reference for Convex platform limits, workarounds, and performance best practices |
| convex-rules | Guidelines and best practices for building Convex projects, including database schema design, queries, mutations, and real-world examples |
| electron-app-development-patterns | Guidelines for Electron development ecosystems including npm, pnpm, and Bun, with security baselines and framework comparisons |
| error-handling-rules | Rules for handling errors, failures, and exceptional conditions |
| general-coding-rules | Rules for constants, magic numbers, and general coding practices |
| general-comment-rules | Language-agnostic rules for writing clean, maintainable comments |
| general-eng-assistant-rules | Rules for AI assistants acting as senior engineers, including objectivity and communication guidelines |
| general-style-rules | Style guidelines for auto-formatting, emoji usage, and output formatting |
| general-tdd-guidelines | Test-Driven Development methodology and best practices |
| general-testing-rules | Rules for writing minimal, effective tests with maximum coverage |
| golden-testing-guidelines | Guidelines for implementing golden/snapshot testing for complex systems |
| pnpm-monorepo-patterns | Modern patterns for pnpm-based TypeScript monorepo architecture |
| python-cli-patterns | Modern patterns for Python CLI application architecture |
| python-modern-guidelines | Guidelines for modern Python projects using uv, with a few more opinionated practices |
| python-rules | General Python coding rules and best practices |
| release-notes-guidelines | Guidelines for writing clear, accurate release notes |
| tbd-sync-troubleshooting | Common issues and solutions for tbd sync and workspace operations |
| typescript-cli-tool-rules | Rules for building CLI tools with Commander.js, picocolors, and TypeScript |
| typescript-code-coverage | Best practices for code coverage in TypeScript with Vitest and v8 provider |
| typescript-rules | TypeScript coding rules and best practices |
| typescript-sorting-patterns | Deterministic sorting patterns and comparison chains for TypeScript |
| typescript-yaml-handling-rules | Best practices for parsing and serializing YAML in TypeScript |
| writing-style-guidelines | Guidelines for clear, concise, and reader-friendly writing in documentation and code |

<!-- END SHORTCUT DIRECTORY -->
<!-- END TBD INTEGRATION -->
