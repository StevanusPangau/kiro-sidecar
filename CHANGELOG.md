# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
