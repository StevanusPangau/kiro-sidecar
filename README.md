# kiro-sidecar

Rust CLI for using Kiro CLI as a bounded Codex sidecar.

The tool keeps Codex as the orchestrator and final reviewer while Kiro performs
read-heavy exploration, reviews, patch drafting, and bounded edits.

## Prerequisites

- Rust toolchain with Cargo.
- Git.
- Kiro CLI available as `kiro-cli` on `PATH`, or set `KIRO_CLI` to the
  executable path.

## Install From Source

```bash
git clone https://github.com/StevanusPangau/kiro-sidecar.git
cd kiro-sidecar
cargo build --release
mkdir -p ~/.local/bin
ln -sf "$(pwd)/bin/kiro-sidecar" ~/.local/bin/kiro-sidecar
```

Ensure `~/.local/bin` is on `PATH`, then verify the installation:

```bash
kiro-sidecar status
```

The `bin/kiro-sidecar` wrapper runs `target/release/kiro-sidecar` when
present, falls back to `target/debug/kiro-sidecar`, and finally falls back to
`cargo run`.

You can also run the wrapper directly without adding it to `PATH`:

```bash
./bin/kiro-sidecar status
```

## Codex Skill and AGENTS.md Setup

This repository includes an installable agent skill at
`skills/kiro-sidecar-codex/`. Use the skill when you want Codex to remember the
Kiro Sidecar workflow on demand. Use `AGENTS.md` when you want the policy active
before any skill trigger.

Install the skill from this checkout:

```bash
npx skills add . --skill kiro-sidecar-codex -a codex
```

Install from GitHub:

```bash
npx skills add https://github.com/StevanusPangau/kiro-sidecar --skill kiro-sidecar-codex -a codex
```

Project scope is the default. To install it globally for your Codex setup:

```bash
npx skills add https://github.com/StevanusPangau/kiro-sidecar --skill kiro-sidecar-codex -a codex -g
```

Update project or global installs:

```bash
npx skills update kiro-sidecar-codex -p -y
npx skills update kiro-sidecar-codex -g -y
```

Let the `skills` CLI choose the Codex skill destination for `-a codex`. The
`skills.sh` CLI currently documents Codex project installs under
`.agents/skills` and global installs under `~/.codex/skills`; current Codex docs
also document repo skills under `.agents/skills` and user skills under
`$HOME/.agents/skills`. Prefer the CLI or your active Codex docs over hardcoded
manual paths.

Recommended global Codex instructions, for `~/.codex/AGENTS.md`:

```md
## Kiro CLI Default External Agent

Use Kiro CLI as the default external sidecar agent when the user asks for a
review, when independent codebase exploration would help, or when bounded
sidecar analysis can save Codex context. If the user asks for a sub-agent
without specifying Codex, interpret the default as Kiro CLI.

Default Kiro usage:

- Prefer `kiro-sidecar` over raw `kiro-cli`; ensure it resolves on `PATH`.
- Keep Kiro read-only by default.
- For uncommitted-change reviews, run
  `kiro-sidecar review "optional focused review prompt"`.
- For codebase exploration, run
  `kiro-sidecar explore "concrete bounded question"`.
- For questions about Kiro CLI behavior, run
  `kiro-sidecar help "question"`.
- For focused review after a Kiro draft or edit, run
  `kiro-sidecar audit-diff "optional focused audit prompt"`.
- For hygiene checks, run `kiro-sidecar status`. If it reports sidecar traces,
  run `kiro-sidecar cleanup --all-sidecar` before finishing.

Write delegation:

- Use writer modes only after Codex has decided the scope and explicit path
  allowlists.
- Low risk: `kiro-sidecar edit --allow PATH_GLOB "concrete edit request"`.
- Medium risk: `kiro-sidecar edit-worktree --allow PATH_GLOB "concrete edit request"`
  or `kiro-sidecar patch --allow PATH_GLOB "concrete patch request"`.
- Parallel write work must use `kiro-sidecar parallel-worktree`, not direct
  `edit`.
- High-risk auth, secrets, payments, deployment config, database migrations,
  concurrency, data integrity, and broad architecture changes stay with Codex;
  Kiro may only `explore`, `review`, or `audit-diff`.
- After every Kiro writer run, inspect the diff yourself and run targeted
  tests/lint before finalizing.
- Do not pass `--trust-all-tools`, `execute_bash`, shell-capable tools, or
  unbounded write access to Kiro.
```

Recommended project instructions, for any repository `AGENTS.md`:

```md
## Kiro Sidecar Usage

- This repository may use `kiro-sidecar` for bounded Codex/Kiro collaboration.
- Use `kiro-sidecar review "focus"` for independent review of uncommitted
  changes.
- Use `kiro-sidecar explore "question"` for bounded codebase exploration before
  broad or ambiguous work.
- Keep Kiro read-only unless Codex has chosen an explicit writer mode and
  path allowlist.
- Writer output is draft work. Codex must inspect the diff and run this
  repository's relevant tests or lint before finalizing.
- Prefer `kiro-sidecar edit-worktree` or `kiro-sidecar patch` for medium-risk
  changes so the main working tree stays under Codex review.
- Keep auth, secrets, deployment config, migrations, concurrency, and data
  integrity changes with Codex; Kiro may only review or explore those areas.
- Run `kiro-sidecar cleanup --all-sidecar` if `kiro-sidecar status` reports
  temporary sidecar files, logs, or agents.
```

Only put this project's Rust build/test commands in a repository `AGENTS.md`
when that repository is the `kiro-sidecar` source itself. For normal consuming
projects, the `AGENTS.md` section should focus on how Codex uses
`kiro-sidecar`, not how to build this wrapper.

## Usage

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

## PATH Setup

If this repository is moved after installation, update the symlink:

```bash
ln -sf "$PROJECT_ROOT/bin/kiro-sidecar" ~/.local/bin/kiro-sidecar
```

Replace `$PROJECT_ROOT` with the repository path.

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

## License

MIT. See [LICENSE](LICENSE).
