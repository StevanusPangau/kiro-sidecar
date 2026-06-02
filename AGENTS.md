# AGENTS.md

## Build & Test

```bash
cargo build --release
cargo test                  # runs unit + integration tests
cargo clippy                # no custom config; uses defaults
cargo fmt -- --check        # no custom config; uses defaults
```

Integration tests (`tests/cli_fake.rs`) use `assert_cmd`, `predicates`, and `tempfile`. They create temp git repos and inject a fake Kiro CLI via `KIRO_CLI` env var, so tests run without a real Kiro installation.

## Architecture

Single-crate async CLI (`tokio` multi-threaded runtime, `clap` derive).

| Module | Responsibility |
|--------|---------------|
| `cli.rs` | Clap command definitions and dispatch (~1k lines, the main entry) |
| `parallel.rs` | Fan-out orchestration with concurrency, dependencies, resources |
| `writer.rs` | Bounded edit and worktree edit flows with write-guard |
| `kiro.rs` | Spawns and communicates with the real `kiro-cli` process |
| `profiles.rs` | Permission profile resolution (built-in + project TOML) |
| `task_schema.rs` | Task JSON validation and loading |
| `git_utils.rs` | Git operations (diff, status, worktree management) |
| `config.rs` | Constants, default deny globs, structured output templates |
| `events.rs` | JSONL event logging for parallel runs |
| `paths.rs` | Run ID generation, artifact paths, glob normalization |

## Conventions

- Conventional commits (type(scope): description)
- No workspace; single `[package]` in `Cargo.toml`
- Default rustfmt and clippy rules apply
- `bin/kiro-sidecar` shell wrapper resolves: release binary > debug binary > `cargo run`

## Runtime Artifacts

- `.kiro-sidecar/` - run history, results, verdicts (gitignored)
- `.kiro-sidecar/profiles.toml` - optional project-level permission profiles
- Temp worktrees created under `KIRO_TMP_ROOT` (default `/private/tmp`)

## Key Env Vars (for testing/development)

- `KIRO_CLI` - path to Kiro CLI executable (useful for faking in tests)
- `KIRO_TMP_ROOT` - temp directory root for worktrees
- `KIRO_MODEL` - model override (default `claude-opus-4.6`)
- `KIRO_TIMEOUT_SECONDS` - per-run timeout (default 1200)

## Gotchas

- The `__write-guard` subcommand is intentionally hidden from help output; it's invoked as a pre-write hook by the writer module.
- `KIRO_TRUST_TOOLS` and `KIRO_EDIT_TRUST_TOOLS` can only *narrow* built-in profiles, never add tools. Use explicit profile definitions to add capabilities.
- Parallel write tasks use git worktrees and return patches; they never modify the working tree directly.
- The test suite requires `git` on PATH and creates real git repos in temp directories.
