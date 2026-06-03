---
name: kiro-sidecar-codex
description: Use Kiro Sidecar from Codex for bounded Kiro CLI review, codebase exploration, patch drafting, edit-worktree delegation, parallel sidecar work, audit-diff, cleanup, and AGENTS.md setup. Trigger when the user mentions Kiro, Kiro CLI, kiro-sidecar, sidecar agents, external agent review or exploration, or configuring Codex AGENTS.md for Kiro sidecar.
license: MIT
compatibility: Designed for Codex with kiro-sidecar on PATH and Kiro CLI available as kiro-cli or KIRO_CLI.
---

# Kiro Sidecar for Codex

Use `kiro-sidecar` as Codex's bounded external Kiro agent. Codex stays the planner, editor of record, and final reviewer.

## Preconditions

- Run from the target project root.
- Prefer the `kiro-sidecar` wrapper on PATH.
- If the wrapper is unavailable, ask the user to install it or use raw `kiro-cli` read-only only for exploration/debugging.
- Keep Kiro read-only unless the user asked for an edit and Codex has chosen explicit path allowlists.

## Default Routing

- Review current changes: `kiro-sidecar review "focused prompt"`.
- Explore a codebase question: `kiro-sidecar explore "bounded question"`.
- Ask about Kiro CLI behavior: `kiro-sidecar help "question"`.
- Audit the current diff after a draft/edit: `kiro-sidecar audit-diff "focused prompt"`.
- Check/clean wrapper traces before finishing: `kiro-sidecar status`, then `kiro-sidecar cleanup --all-sidecar` if status reports sidecar artifacts.

## Write Delegation

- Low-risk bounded edits: `kiro-sidecar edit --allow PATH_GLOB "specific request"`.
- Medium-risk bounded edits: `kiro-sidecar edit-worktree --allow PATH_GLOB "specific request"`; review the returned patch before applying it.
- Patch-only drafts: `kiro-sidecar patch --allow PATH_GLOB "specific request"`.
- Parallel read work: use JSON tasks with `parallel-explore` or `parallel-review`.
- Parallel write work: use `parallel-worktree`, not direct `edit`.
- Never give Kiro `--trust-all-tools`, shell-capable tools, or unbounded write access.
- After any writer or patch run, inspect Kiro's structured output and the diff, then run targeted tests or lint.

Expected Kiro writer sections are `CHANGED_FILES`, `SUMMARY`, `DECISIONS`, `UNCERTAINTIES`, `TESTS_NOT_RUN`, and `RISK_NOTES`.

## Risk Routing

- High-risk auth, secrets, payments, deployment config, database migrations, concurrency, data integrity, and broad architecture changes stay with Codex. Kiro may only explore, review, or audit.
- Unknown-risk tasks start with `kiro-sidecar explore`.

## AGENTS.md Setup

When the user asks to configure Codex instructions for Kiro Sidecar:

- Update only the requested scope: global `~/.codex/AGENTS.md`, project `AGENTS.md`, or a nested `AGENTS.override.md`.
- Preserve existing instructions and append a small Kiro Sidecar section instead of replacing the whole file.
- Include the default routing, read-only default, explicit write allowlists, Codex final-review requirement, and cleanup command.
- For consuming projects, focus the AGENTS.md section on how Codex should use `kiro-sidecar`; do not add this wrapper source repository's Rust build/test commands unless the target repo is the `kiro-sidecar` source itself.
- If the target repo contains a Kiro Sidecar README section, prefer its current snippet over recreating one from memory.
