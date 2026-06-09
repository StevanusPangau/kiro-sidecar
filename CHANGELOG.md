# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-06-09

### Added

- Optional `review_loop` task JSON support for `parallel-worktree`, letting a
  read-only Kiro reviewer gate generated patches and request bounded worker
  revisions before Codex performs the final review.
- Review-loop artifact metadata for status, iteration count, final verdict, and
  reviewer output hashes.
- Validation coverage for invalid `review_loop` tokens, unsupported parallel
  commands, reviewer failures, unknown verdicts, exhausted loops, and retry
  behavior inside worker iterations.

### Changed

- Documented `review_loop` usage in the README and Codex skill guidance,
  including safe `parallel-worktree` defaults and the 12,000-character reviewer
  feedback cap for follow-up worker prompts.
- Hardened exhausted review loops so the final patch is promoted only when the
  expected `worktree.patch` artifact exists.

## [0.2.0] - 2026-06-07

### Added

- Optional `KIRO_EFFORT` forwarding to Kiro CLI 2.6.0+ for per-run effort
  overrides while preserving persisted Kiro model settings by default.
- Effort override visibility in `kiro-sidecar status`.
- Per-task `model` and `effort` overrides for parallel task JSON.
- Effective runtime settings and artifact SHA-256 hashes in task metadata.

### Changed

- `KIRO_EFFORT` is validated before launching Kiro, and unset task metadata now
  records effort as `null`.

## [0.1.1] - 2026-06-03

### Added

- Installable `kiro-sidecar-codex` agent skill for configuring Codex to use Kiro Sidecar for review, exploration, patch drafting, bounded edits, and cleanup.
- `skills.sh.json` repo page metadata for grouping the Codex skill on skills.sh.
- GitHub Actions draft release workflow that runs on `v*.*.*` tag pushes and creates or updates release notes from the matching `CHANGELOG.md` version section.

### Changed

- Updated README install commands to use `https://github.com/StevanusPangau/kiro-sidecar`.
- Added README guidance for global and project `AGENTS.md` setup focused on consuming `kiro-sidecar`, not building this source repository.
- Updated GitHub Actions checkout usage to `actions/checkout@v6`.
- Refreshed `AGENTS.md` table formatting while keeping the public project guidance scoped to build, test, architecture, and runtime notes.

## [0.1.0] - 2026-06-02

### Added

- Kiro sidecar CLI with clap and tokio async runtime
- Parallel orchestration with dependency scheduling, resource locks, fail-fast, and retry logic
- Permission profile system with built-in and project TOML profiles
- Task JSON schema validation and loading
- JSONL event logging for parallel runs
- CLI subcommands: `parallel-explore`, `parallel-review`, `parallel-worktree`, `validate`, `history`, `diff-summary`, `apply`, `accept`, `reject`, `status`, `cleanup`
- Git worktree management and patch operations
- Write guard with symlink boundary hardening
- Rust engineering skill for agent workflows
- GitHub Actions CI workflow with fmt, clippy, and test steps
- Comprehensive unit tests for profiles, task schema, and paths modules
- Integration tests using fake Kiro CLI via `KIRO_CLI` env var

### Changed

- Ported entire wrapper from Python to Rust, preserving existing command contract

### Removed

- Python kiro-sidecar package (replaced by Rust CLI)
- Python skills (replaced by Rust skill)

[Unreleased]: https://github.com/StevanusPangau/kiro-sidecar/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/StevanusPangau/kiro-sidecar/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/StevanusPangau/kiro-sidecar/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/StevanusPangau/kiro-sidecar/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/StevanusPangau/kiro-sidecar/releases/tag/v0.1.0
