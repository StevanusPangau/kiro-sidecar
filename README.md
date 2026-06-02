# kiro-sidecar

Rust CLI for using Kiro CLI as a bounded Codex sidecar.

The tool keeps Codex as the orchestrator and final reviewer while Kiro performs
read-heavy exploration, reviews, patch drafting, and bounded edits.

## Build

```bash
cargo build --release
```

The `bin/kiro-sidecar` wrapper runs `target/release/kiro-sidecar` when present,
falls back to `target/debug/kiro-sidecar`, and finally falls back to `cargo run`.

## Commands

```bash
kiro-sidecar explore "question"
kiro-sidecar help "question"
kiro-sidecar review "optional focus"
kiro-sidecar audit-diff "optional focus"
kiro-sidecar review --format json "optional focus"
kiro-sidecar explore --profile web-research "question that needs web tools"
kiro-sidecar patch --allow docs/file.md "draft patch"
kiro-sidecar edit --allow docs/file.md "bounded edit"
kiro-sidecar edit-worktree --allow app/foo.py "bounded edit in temp worktree"
kiro-sidecar parallel-explore tasks.json --max-concurrency 6 --fail-fast
kiro-sidecar parallel-review tasks.json --format json
kiro-sidecar parallel-worktree tasks.json --max-concurrency 6
kiro-sidecar validate tasks.json
kiro-sidecar history --last 10
kiro-sidecar diff-summary RUN_ID --task TASK_ID
kiro-sidecar apply RUN_ID --task TASK_ID
kiro-sidecar accept RUN_ID --task TASK_ID
kiro-sidecar reject RUN_ID --task TASK_ID
kiro-sidecar status
kiro-sidecar cleanup --all-sidecar
```

`apply` only applies the selected patch from a successful `parallel-worktree`
task to the current working tree. It does not commit, push, or mark the patch
accepted.

## Permission Profiles

Built-in profiles are used when `.kiro-sidecar/profiles.toml` is absent:

- `read-only`: `fs_read,grep,glob`; default for explore, review, audit-diff,
  parallel-explore, and parallel-review.
- `web-research`: `fs_read,grep,glob,web_search,web_fetch`; explicit only.
- `scoped-edit`: `fs_read,fs_write,grep,glob`; default for `edit`.
- `worktree-edit`: `fs_read,fs_write,grep,glob`; default for worktree modes.

Example project profile file:

```toml
[profiles.docs-review]
tools = ["fs_read", "grep", "glob"]

[profiles.docs-edit]
tools = ["fs_read", "fs_write", "grep", "glob"]
write = true
```

Profiles are selected by task JSON first, then CLI `--profile`, then the
command default. Legacy `KIRO_TRUST_TOOLS` and `KIRO_EDIT_TRUST_TOOLS` may
narrow built-in defaults, but they cannot add extra tools to those defaults.
Define an explicit profile instead when a run needs additional tools.

## Local PATH Setup

Source of truth:

```bash
$PROJECT_ROOT
```

Preferred PATH entry:

```bash
$PROJECT_ROOT/bin
```

If you prefer keeping `~/.local/bin` as the only PATH entry, symlink
`~/.local/bin/kiro-sidecar` to `bin/kiro-sidecar` in this project.

## Task JSON

```json
[
  {
    "id": "security",
    "prompt": "Review security risks only.",
    "profile": "read-only",
    "allow": [],
    "deny": [],
    "timeout_seconds": 900,
    "depends_on": [],
    "expected_files": [],
    "tags": ["security"],
    "resource": "security-review",
    "retry": 0,
    "max_diff_lines": 400
  }
]
```

Parallel run artifacts are written to `.kiro-sidecar/runs/<run_id>/`:

- `results.jsonl`: one task record per line.
- `events.jsonl`: run, task, retry, and heartbeat events.
- `run_summary.json`: final run summary used by `history`.
- `verdict.json`: Codex `accept` or `reject` decisions.
- `tasks/<task>/metadata.json`: per-task metadata.

Default parallel fan-out is 6 tasks. Raise or lower it with `--max-concurrency`.
`depends_on` controls scheduling order. `resource` serializes write tasks that
target the same resource. `retry` retries timeout or non-zero Kiro exits, but
write-guard and path-boundary failures are not retried. `--fail-fast` stops
scheduling new tasks after a failure is observed; tasks already running are
allowed to finish. `accept` and `reject` only record decisions for generated
task artifacts in an existing parallel run.

## Safety Model

- Kiro is read-only by default.
- Direct `edit` is single-task only.
- Parallel writes use temporary git worktrees and return patches.
- `web_search` and `web_fetch` are available only through an explicit
  `web-research` profile or an explicit project profile.
- Writer mode creates a temporary Kiro agent with a pre-write hook and post-run
  changed-file validation.
- The pre-write hook is handled by the hidden Rust subcommand
  `kiro-sidecar __write-guard`.
- Codex must review Kiro output and patches before finalizing.
- Do not use `--trust-all-tools` for Codex sidecar work. If a specific workflow
  needs additional read-only capability such as web search, grant only that tool
  explicitly for that run and keep shell-capable tools out of sidecar defaults.

## Optional Environment Overrides

- `KIRO_CLI`: Kiro CLI executable, default `kiro-cli`.
- `KIRO_MODEL`: Kiro model, default `claude-opus-4.6`.
- `KIRO_TRUST_TOOLS`: read-only tools, default `fs_read,grep,glob`; can only
  narrow the built-in profile.
- `KIRO_EDIT_TRUST_TOOLS`: writer tools, default `fs_read,fs_write,grep,glob`;
  can only narrow the built-in writer profiles.
- `KIRO_TIMEOUT_SECONDS`: max seconds for one Kiro chat run, default `1200`.
- `KIRO_TMP_ROOT`: temp directory root, default `/private/tmp`.
- `KIRO_AGENT_DIR`: Kiro agent directory, default `.kiro/agents`.
