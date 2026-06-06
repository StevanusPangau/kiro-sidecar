use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use tempfile::TempDir;

fn cli(repo: &Path, fake_kiro: &Path) -> Result<Command, Box<dyn Error>> {
    let mut command = Command::cargo_bin("kiro-sidecar")?;
    command.current_dir(repo);
    command.env("KIRO_CLI", fake_kiro);
    command.env(
        "KIRO_TMP_ROOT",
        repo.parent().unwrap_or(repo).join("kiro-tmp"),
    );
    Ok(command)
}

fn init_repo(path: &Path) -> Result<(), Box<dyn Error>> {
    run_git(path, &["init", "-q"])?;
    run_git(path, &["config", "user.email", "test@example.com"])?;
    run_git(path, &["config", "user.name", "Test"])?;
    fs::write(path.join("allowed.txt"), "original\n")?;
    fs::write(path.join("outside.txt"), "original\n")?;
    run_git(path, &["add", "allowed.txt", "outside.txt"])?;
    run_git(path, &["commit", "-qm", "init"])?;
    Ok(())
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .status()?;
    assert!(status.success(), "git {:?} failed", args);
    Ok(())
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .output()?;
    assert!(output.status.success(), "git {:?} failed", args);
    Ok(String::from_utf8(output.stdout)?)
}

fn track_spaced_file(repo: &Path) -> Result<(), Box<dyn Error>> {
    fs::write(repo.join("foo bar.txt"), "original\n")?;
    run_git(repo, &["add", "foo bar.txt"])?;
    run_git(repo, &["commit", "-qm", "add spaced file"])?;
    Ok(())
}

fn track_rename_source_file(repo: &Path) -> Result<(), Box<dyn Error>> {
    fs::write(repo.join("old.txt"), "original\n")?;
    run_git(repo, &["add", "old.txt"])?;
    run_git(repo, &["commit", "-qm", "add rename source"])?;
    Ok(())
}

fn make_fake_kiro(tmp: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let fake = tmp.join("fake-kiro");
    fs::write(
        &fake,
        r#"#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -eq 1 && "$1" == "--version" ]]; then
  echo "fake-kiro 0.0.0"
  exit 0
fi

if [[ "$#" -ge 4 && "$1" == "settings" && "$2" == "list" && "$3" == "--format" ]]; then
  cat <<'JSON'
{"cleanup.periodDays":1,"chat.defaultModel":"claude-opus-4.6","chat.modelDefaults":{"claude-opus-4.6":{"output_config":{"effort":"max"}}}}
JSON
  exit 0
fi

if [[ "$#" -ge 1 && "$1" == "chat" ]]; then
  for arg in "$@"; do
    if [[ "$arg" == "--list-sessions" ]]; then
      if [[ -n "${FAKE_KIRO_SESSION_FILE:-}" && -f "$FAKE_KIRO_SESSION_FILE" ]]; then
        echo "Chat SessionId: ${FAKE_KIRO_SESSION_ID:-fake-session}"
      fi
      exit 0
    fi
    if [[ "$arg" == "--delete-session" ]]; then
      if [[ -n "${FAKE_KIRO_DELETE_FAIL:-}" ]]; then
        exit 99
      fi
      exit 0
    fi
  done
fi

if [[ -n "${FAKE_KIRO_SESSION_FILE:-}" ]]; then
  touch "$FAKE_KIRO_SESSION_FILE"
fi

case "${FAKE_KIRO_MODE:-}" in
  hang)
    exec sleep 30
    ;;
  edit_allowed)
    if [[ -n "${FAKE_KIRO_ATTEMPTS_FILE:-}" ]]; then
      printf 'attempt\n' >> "$FAKE_KIRO_ATTEMPTS_FILE"
    fi
    printf 'changed by fake kiro\n' > allowed.txt
    printf 'CHANGED_FILES:\n- allowed.txt\n\nSUMMARY:\n- changed allowed\n'
    ;;
  edit_space)
    if [[ -n "${FAKE_KIRO_ATTEMPTS_FILE:-}" ]]; then
      printf 'attempt\n' >> "$FAKE_KIRO_ATTEMPTS_FILE"
    fi
    printf 'changed by fake kiro\n' > 'foo bar.txt'
    printf 'CHANGED_FILES:\n- foo bar.txt\n\nSUMMARY:\n- changed spaced path\n'
    ;;
  edit_outside)
    if [[ -n "${FAKE_KIRO_ATTEMPTS_FILE:-}" ]]; then
      printf 'attempt\n' >> "$FAKE_KIRO_ATTEMPTS_FILE"
    fi
    printf 'changed outside\n' > outside.txt
    printf 'CHANGED_FILES:\n- outside.txt\n\nSUMMARY:\n- changed outside\n'
    ;;
  rename_allowed)
    if [[ -n "${FAKE_KIRO_ATTEMPTS_FILE:-}" ]]; then
      printf 'attempt\n' >> "$FAKE_KIRO_ATTEMPTS_FILE"
    fi
    mv old.txt new.txt
    printf 'CHANGED_FILES:\n- old.txt\n- new.txt\n\nSUMMARY:\n- renamed old to new\n'
    ;;
  patch)
    printf 'PATCH:\n```diff\n'
    printf 'diff --git a/allowed.txt b/allowed.txt\n'
    printf -- '--- a/allowed.txt\n'
    printf '+++ b/allowed.txt\n'
    printf '@@ -1 +1 @@\n'
    printf -- '-original\n'
    printf '+patched\n'
    printf '```\n\nNO_EXTRA_TEXT_AFTER_PATCH\n'
    ;;
  parallel)
    printf 'CHANGED_FILES:\n- none\n\nSUMMARY:\n- parallel ok\n'
    ;;
  print_trust)
    for arg in "$@"; do
      if [[ "$arg" == --trust-tools=* ]]; then
        printf 'TRUST:%s\n' "${arg#--trust-tools=}"
      fi
    done
    ;;
  print_effort)
    model=""
    effort=""
    previous=""
    for arg in "$@"; do
      if [[ "$previous" == "--model" ]]; then
        model="$arg"
      fi
      if [[ "$previous" == "--effort" ]]; then
        effort="$arg"
      fi
      previous="$arg"
    done
    printf 'MODEL:%s\n' "$model"
    if [[ -n "$effort" ]]; then
      printf 'EFFORT:%s\n' "$effort"
    else
      printf 'EFFORT:unset\n'
    fi
    ;;
  fail_on_fail)
    joined="$*"
    if [[ "$joined" == *"FAIL"* ]]; then
      printf 'failed as requested\n'
      exit 7
    fi
    printf 'ok\n'
    ;;
  help_agent)
    saw_agent=0
    previous=""
    for arg in "$@"; do
      if [[ "$previous" == "--agent" && "$arg" == "kiro_help" ]]; then
        saw_agent=1
      fi
      previous="$arg"
    done
    if [[ "$saw_agent" == "1" ]]; then
      echo "HELP_AGENT"
    else
      echo "missing kiro_help agent"
      exit 3
    fi
    ;;
  *)
    echo "OK"
    ;;
esac
"#,
    )?;
    make_executable(&fake)?;
    Ok(fake)
}

fn make_executable(path: &Path) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[test]
fn explore_only_passes_effort_when_configured() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "print_effort")
        .env_remove("KIRO_EFFORT")
        .args(["explore", "inspect"])
        .assert()
        .success()
        .stdout(predicate::str::contains("EFFORT:unset"));

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "print_effort")
        .env("KIRO_EFFORT", "high")
        .args(["explore", "inspect"])
        .assert()
        .success()
        .stdout(predicate::str::contains("EFFORT:high"));
    Ok(())
}

#[test]
fn explore_rejects_unknown_env_effort() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("KIRO_EFFORT", "extreme")
        .args(["explore", "inspect"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "KIRO_EFFORT must be one of low, medium, high, xhigh, max",
        ));
    Ok(())
}

#[test]
fn edit_allows_scoped_file() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["edit", "--allow", "allowed.txt", "change allowed"])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(repo.join("allowed.txt"))?,
        "changed by fake kiro\n"
    );
    let agents = repo.join(".kiro/agents");
    assert!(!agents.exists() || fs::read_dir(agents)?.next().is_none());
    Ok(())
}

#[test]
fn edit_blocks_outside_allow() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_outside")
        .args(["edit", "--allow", "allowed.txt", "change outside"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("outside --allow"));
    Ok(())
}

#[test]
fn edit_blocks_changes_to_existing_dirty_file_outside_allow() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    fs::write(repo.join("outside.txt"), "dirty before sidecar\n")?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_outside")
        .args(["edit", "--allow", "allowed.txt", "change outside"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "pre-existing dirty files outside --allow",
        ));
    assert_eq!(
        fs::read_to_string(repo.join("outside.txt"))?,
        "changed outside\n"
    );
    Ok(())
}

#[test]
fn patch_does_not_change_worktree() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "patch")
        .args(["patch", "--allow", "allowed.txt", "draft patch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PATCH:"));

    let status = StdCommand::new("git")
        .args(["status", "--short"])
        .current_dir(&repo)
        .output()?;
    assert_eq!(String::from_utf8_lossy(&status.stdout), "");
    Ok(())
}

#[test]
fn edit_worktree_does_not_change_main_tree() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args([
            "edit-worktree",
            "--allow",
            "allowed.txt",
            "change in worktree",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PATCH:"))
        .stdout(predicate::str::contains("changed by fake kiro"));

    assert_eq!(fs::read_to_string(repo.join("allowed.txt"))?, "original\n");
    Ok(())
}

#[test]
fn parallel_explore_writes_results() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "a", "prompt": "A"},
            {"id": "b", "prompt": "B"}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "parallel")
        .args(["parallel-explore", "tasks.json", "--max-concurrency", "2"])
        .assert()
        .success();

    let runs = repo.join(".kiro-sidecar/runs");
    let result_files = fs::read_dir(runs)?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("results.jsonl"))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    assert_eq!(result_files.len(), 1);
    let lines = fs::read_to_string(&result_files[0])?;
    assert_eq!(lines.lines().count(), 2);
    assert!(lines.contains("\"id\":\"a\""));
    assert!(lines.contains("\"id\":\"b\""));
    Ok(())
}

#[test]
fn parallel_tasks_can_override_model_and_effort() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "fast-scan", "prompt": "A", "model": "claude-haiku-4.5", "effort": "low"},
            {"id": "deep-review", "prompt": "B", "model": "claude-opus-4.6", "effort": "max"}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "print_effort")
        .env("KIRO_MODEL", "claude-sonnet-4.6")
        .env("KIRO_EFFORT", "medium")
        .args(["parallel-explore", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let run_id = latest_run_id(&repo)?;
    let fast_dir = repo.join(format!(".kiro-sidecar/runs/{run_id}/tasks/fast-scan"));
    let deep_dir = repo.join(format!(".kiro-sidecar/runs/{run_id}/tasks/deep-review"));
    assert!(fs::read_to_string(fast_dir.join("output.txt"))?.contains("MODEL:claude-haiku-4.5"));
    assert!(fs::read_to_string(fast_dir.join("output.txt"))?.contains("EFFORT:low"));
    assert!(fs::read_to_string(deep_dir.join("output.txt"))?.contains("MODEL:claude-opus-4.6"));
    assert!(fs::read_to_string(deep_dir.join("output.txt"))?.contains("EFFORT:max"));

    let metadata: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(deep_dir.join("metadata.json"))?)?;
    assert_eq!(metadata["execution"]["model"], "claude-opus-4.6");
    assert_eq!(metadata["execution"]["effort"], "max");
    assert!(metadata["artifacts"]["output_sha256"]
        .as_str()
        .is_some_and(|value| value.len() == 64));
    Ok(())
}

#[test]
fn parallel_metadata_uses_null_for_unset_effort() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([{"id": "scan", "prompt": "A"}]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "print_effort")
        .env_remove("KIRO_EFFORT")
        .args(["parallel-explore", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let run_id = latest_run_id(&repo)?;
    let task_dir = repo.join(format!(".kiro-sidecar/runs/{run_id}/tasks/scan"));
    assert!(fs::read_to_string(task_dir.join("output.txt"))?.contains("EFFORT:unset"));
    let metadata: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(task_dir.join("metadata.json"))?)?;
    assert!(metadata["execution"]["effort"].is_null());
    Ok(())
}

#[test]
fn parallel_review_writes_results() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    fs::write(repo.join("allowed.txt"), "dirty\n")?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([{"id": "review", "prompt": "Review diff"}]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "parallel")
        .args(["parallel-review", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let result_file = single_results_file(&repo)?;
    let lines = fs::read_to_string(result_file)?;
    assert!(lines.contains("\"id\":\"review\""));
    assert!(lines.contains("\"status\":\"ok\""));
    Ok(())
}

#[test]
fn parallel_worktree_writes_patch_without_changing_main_tree() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "worktree", "prompt": "Change allowed", "allow": ["allowed.txt"]}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    assert_eq!(fs::read_to_string(repo.join("allowed.txt"))?, "original\n");
    let result_file = single_results_file(&repo)?;
    let lines = fs::read_to_string(result_file)?;
    assert!(lines.contains("\"id\":\"worktree\""));
    assert!(lines.contains("changed by fake kiro"));
    Ok(())
}

#[test]
fn apply_refuses_patch_when_spaced_path_is_dirty() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    track_spaced_file(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "space", "prompt": "Change spaced file", "allow": ["foo bar.txt"]}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_space")
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();
    fs::write(repo.join("foo bar.txt"), "dirty in main tree\n")?;

    let run_id = latest_run_id(&repo)?;
    cli(&repo, &fake)?
        .args(["apply", &run_id, "--task", "space"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("foo bar.txt is already dirty"));
    assert_eq!(
        fs::read_to_string(repo.join("foo bar.txt"))?,
        "dirty in main tree\n"
    );
    Ok(())
}

#[test]
fn apply_refuses_patch_when_rename_source_is_dirty() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    track_rename_source_file(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    fs::rename(repo.join("old.txt"), repo.join("new.txt"))?;
    run_git(&repo, &["add", "-A"])?;
    let patch = git_stdout(&repo, &["diff", "--binary", "--find-renames", "HEAD"])?;
    assert!(patch.contains("rename from old.txt"));
    run_git(&repo, &["reset", "-q", "HEAD"])?;
    fs::rename(repo.join("new.txt"), repo.join("old.txt"))?;

    let run_id = "parallel-worktree-rename";
    let run_dir = repo.join(format!(".kiro-sidecar/runs/{run_id}"));
    let task_dir = run_dir.join("tasks/rename");
    fs::create_dir_all(&task_dir)?;
    fs::write(
        run_dir.join("run_summary.json"),
        serde_json::to_string_pretty(&json!({
            "run_id": run_id,
            "command": "parallel-worktree",
            "status": "ok",
            "started_at": "2026-01-01T00:00:00Z",
            "finished_at": "2026-01-01T00:00:00Z",
            "task_count": 1,
            "failures": 0,
            "run_dir": run_dir.display().to_string(),
            "tasks": [{
                "id": "rename",
                "status": "ok",
                "returncode": 0,
                "task_dir": task_dir.display().to_string(),
                "profile": "worktree-edit",
                "attempts": 1
            }]
        }))?,
    )?;
    fs::write(
        task_dir.join("metadata.json"),
        serde_json::to_string_pretty(&json!({
            "task": {"id": "rename"},
            "record": {"id": "rename", "status": "ok"}
        }))?,
    )?;
    fs::write(task_dir.join("worktree.patch"), patch)?;
    fs::write(repo.join("old.txt"), "dirty in main tree\n")?;

    cli(&repo, &fake)?
        .args(["apply", run_id, "--task", "rename"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("old.txt is already dirty"));
    assert_eq!(
        fs::read_to_string(repo.join("old.txt"))?,
        "dirty in main tree\n"
    );
    assert!(!repo.join("new.txt").exists());
    Ok(())
}

#[test]
fn apply_rejects_task_ids_that_escape_task_artifact_dir() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    let run_id = "parallel-worktree-traversal";
    let run_dir = repo.join(format!(".kiro-sidecar/runs/{run_id}"));
    fs::create_dir_all(run_dir.join("tasks"))?;
    fs::write(
        run_dir.join("run_summary.json"),
        serde_json::to_string_pretty(&json!({
            "run_id": run_id,
            "command": "parallel-worktree",
            "status": "ok",
            "started_at": "2026-01-01T00:00:00Z",
            "finished_at": "2026-01-01T00:00:00Z",
            "task_count": 1,
            "failures": 0,
            "run_dir": run_dir.display().to_string(),
            "tasks": [{
                "id": "..",
                "status": "ok",
                "returncode": 0,
                "task_dir": run_dir.display().to_string(),
                "profile": "worktree-edit",
                "attempts": 1
            }]
        }))?,
    )?;
    fs::write(
        run_dir.join("metadata.json"),
        serde_json::to_string_pretty(&json!({
            "task": {"id": ".."},
            "record": {"id": "..", "status": "ok"}
        }))?,
    )?;
    fs::write(
        run_dir.join("worktree.patch"),
        "diff --git a/allowed.txt b/allowed.txt\n\
         --- a/allowed.txt\n\
         +++ b/allowed.txt\n\
         @@ -1 +1 @@\n\
         -original\n\
         +escaped patch\n",
    )?;

    cli(&repo, &fake)?
        .args(["apply", run_id, "--task", ".."])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no task artifact found"));
    assert_eq!(fs::read_to_string(repo.join("allowed.txt"))?, "original\n");
    Ok(())
}

#[test]
fn apply_refuses_patch_from_non_worktree_run() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "explore", "prompt": "Explore"}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "parallel")
        .args(["parallel-explore", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let run_id = latest_run_id(&repo)?;
    let task_dir = repo.join(format!(".kiro-sidecar/runs/{run_id}/tasks/explore"));
    fs::write(
        task_dir.join("worktree.patch"),
        "diff --git a/allowed.txt b/allowed.txt\n\
         --- a/allowed.txt\n\
         +++ b/allowed.txt\n\
         @@ -1 +1 @@\n\
         -original\n\
         +changed\n",
    )?;

    cli(&repo, &fake)?
        .args(["apply", &run_id, "--task", "explore"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no worktree patch found"));
    assert_eq!(fs::read_to_string(repo.join("allowed.txt"))?, "original\n");
    Ok(())
}

#[test]
fn parallel_worktree_expected_files_handles_spaced_patch_paths() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    track_spaced_file(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {
                "id": "space",
                "prompt": "Change spaced file",
                "allow": ["foo bar.txt"],
                "expected_files": ["allowed.txt"]
            }
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_space")
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .failure();

    let lines = fs::read_to_string(single_results_file(&repo)?)?;
    assert!(lines.contains("\"status\":\"failed\""));
    assert!(lines.contains("patch changed unexpected file(s): foo bar.txt"));
    Ok(())
}

#[test]
fn validate_accepts_valid_tasks_and_rejects_unknown_profiles() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("valid.json"),
        serde_json::to_string(&json!([
            {"id": "docs", "prompt": "Review docs", "profile": "read-only"}
        ]))?,
    )?;
    fs::write(
        repo.join("invalid.json"),
        serde_json::to_string(&json!([
            {"id": "docs", "prompt": "Review docs", "profile": "unknown"}
        ]))?,
    )?;
    fs::write(
        repo.join("unsafe-id.json"),
        serde_json::to_string(&json!([
            {"id": "..", "prompt": "Review docs"}
        ]))?,
    )?;
    fs::write(
        repo.join("colliding-id.json"),
        serde_json::to_string(&json!([
            {"id": "worktree", "prompt": "Review docs"},
            {"id": "worktree!", "prompt": "Review more docs"}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .args(["validate", "valid.json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("task file is valid"));

    cli(&repo, &fake)?
        .args(["validate", "invalid.json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown profile"));
    cli(&repo, &fake)?
        .args(["validate", "unsafe-id.json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("normal artifact directory"));
    cli(&repo, &fake)?
        .args(["validate", "colliding-id.json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("conflicts after sanitizing"));
    Ok(())
}

#[test]
fn parallel_explore_profile_web_research_passes_explicit_web_tools() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([{"id": "research", "prompt": "Research"}]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "print_trust")
        .args([
            "parallel-explore",
            "tasks.json",
            "--profile",
            "web-research",
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"run_id\""));

    let lines = fs::read_to_string(single_results_file(&repo)?)?;
    assert!(lines.contains("TRUST:fs_read,grep,glob,web_search,web_fetch"));
    Ok(())
}

#[test]
fn explicit_builtin_profiles_honor_legacy_env_narrowing() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "task-profile", "prompt": "Trust", "profile": "read-only"}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "print_trust")
        .env("KIRO_TRUST_TOOLS", "fs_read")
        .args(["parallel-explore", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let first_run = latest_run_id(&repo)?;
    let first_results =
        fs::read_to_string(repo.join(format!(".kiro-sidecar/runs/{first_run}/results.jsonl")))?;
    assert!(first_results.contains("TRUST:fs_read"));
    assert!(!first_results.contains("TRUST:fs_read,grep,glob"));

    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "cli-profile", "prompt": "Trust"}
        ]))?,
    )?;
    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "print_trust")
        .env("KIRO_TRUST_TOOLS", "fs_read")
        .args([
            "parallel-explore",
            "tasks.json",
            "--profile",
            "read-only",
            "--max-concurrency",
            "1",
        ])
        .assert()
        .success();

    let second_run = fs::read_dir(repo.join(".kiro-sidecar/runs"))?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|id| id != &first_run)
        .ok_or("missing second run")?;
    let second_results =
        fs::read_to_string(repo.join(format!(".kiro-sidecar/runs/{second_run}/results.jsonl")))?;
    assert!(second_results.contains("TRUST:fs_read"));
    assert!(!second_results.contains("TRUST:fs_read,grep,glob"));
    Ok(())
}

#[test]
fn parallel_worktree_enforces_expected_files_and_max_diff_lines() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {
                "id": "too-large",
                "prompt": "Change allowed",
                "allow": ["allowed.txt"],
                "expected_files": ["allowed.txt"],
                "max_diff_lines": 1
            }
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .failure();

    let lines = fs::read_to_string(single_results_file(&repo)?)?;
    assert!(lines.contains("\"status\":\"failed\""));
    assert!(lines.contains("max_diff_lines"));
    let run_id = latest_run_id(&repo)?;
    cli(&repo, &fake)?
        .args(["apply", &run_id, "--task", "too-large"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "refusing to apply patch because task too-large status is failed",
        ));
    assert_eq!(fs::read_to_string(repo.join("allowed.txt"))?, "original\n");
    Ok(())
}

#[test]
fn parallel_worktree_expected_files_accepts_dot_prefix() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {
                "id": "dot-prefix",
                "prompt": "Change allowed",
                "allow": ["allowed.txt"],
                "expected_files": ["./allowed.txt"]
            }
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let lines = fs::read_to_string(single_results_file(&repo)?)?;
    assert!(lines.contains("\"status\":\"ok\""));
    assert!(!lines.contains("unexpected file"));
    Ok(())
}

#[test]
fn parallel_worktree_patch_limit_violation_does_not_retry() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    let attempts = tmp.path().join("attempts.txt");
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {
                "id": "too-large",
                "prompt": "Change allowed",
                "allow": ["allowed.txt"],
                "expected_files": ["allowed.txt"],
                "max_diff_lines": 1,
                "retry": 3
            }
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .env("FAKE_KIRO_ATTEMPTS_FILE", &attempts)
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .failure();

    assert_eq!(fs::read_to_string(attempts)?.lines().count(), 1);
    let lines = fs::read_to_string(single_results_file(&repo)?)?;
    assert!(lines.contains("\"attempts\":1"));
    assert!(lines.contains("patch limit violation"));
    Ok(())
}

#[test]
fn parallel_worktree_guard_failure_does_not_retry() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    let attempts = tmp.path().join("attempts.txt");
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {
                "id": "outside",
                "prompt": "Change outside",
                "allow": ["allowed.txt"],
                "retry": 3
            }
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_outside")
        .env("FAKE_KIRO_ATTEMPTS_FILE", &attempts)
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .failure();

    assert_eq!(fs::read_to_string(attempts)?.lines().count(), 1);
    let lines = fs::read_to_string(single_results_file(&repo)?)?;
    assert!(lines.contains("\"attempts\":1"));
    assert!(lines.contains("outside --allow"));
    Ok(())
}

#[test]
fn parallel_fail_fast_skips_remaining_independent_tasks() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "first", "prompt": "FAIL"},
            {"id": "second", "prompt": "SHOULD_SKIP"}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "fail_on_fail")
        .args([
            "parallel-explore",
            "tasks.json",
            "--fail-fast",
            "--max-concurrency",
            "1",
        ])
        .assert()
        .failure();

    let lines = fs::read_to_string(single_results_file(&repo)?)?;
    assert!(lines.contains("\"id\":\"first\""));
    assert!(lines.contains("\"id\":\"second\""));
    assert!(lines.contains("\"status\":\"skipped\""));
    Ok(())
}

#[test]
fn parallel_json_keeps_cleanup_warnings_off_stdout() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    let session_file = tmp.path().join("session-created");
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "json", "prompt": "JSON"}
        ]))?,
    )?;

    let output = cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "parallel")
        .env("FAKE_KIRO_SESSION_FILE", &session_file)
        .env("FAKE_KIRO_SESSION_ID", "session-new")
        .env("FAKE_KIRO_DELETE_FAIL", "1")
        .args([
            "parallel-explore",
            "tasks.json",
            "--format",
            "json",
            "--max-concurrency",
            "1",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "could not delete active Kiro session session-new",
        ))
        .get_output()
        .stdout
        .clone();

    let value: serde_json::Value = serde_json::from_slice(&output)?;
    assert_eq!(value["command"], "parallel-explore");
    assert_eq!(value["status"], "ok");
    Ok(())
}

#[test]
fn diff_summary_json_missing_patch_returns_json() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "explore", "prompt": "Explore"}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "parallel")
        .args(["parallel-explore", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let run_id = latest_run_id(&repo)?;
    let output = cli(&repo, &fake)?
        .args([
            "diff-summary",
            &run_id,
            "--task",
            "explore",
            "--format",
            "json",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let summaries: Vec<serde_json::Value> = serde_json::from_slice(&output)?;
    assert!(summaries.is_empty());
    assert!(!String::from_utf8(output)?.contains("no worktree patch"));
    Ok(())
}

#[test]
fn diff_summary_apply_accept_reject_and_history_use_run_artifacts() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "worktree", "prompt": "Change allowed", "allow": ["allowed.txt"]}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let run_id = latest_run_id(&repo)?;
    cli(&repo, &fake)?
        .args(["diff-summary", &run_id, "--task", "worktree"])
        .assert()
        .success()
        .stdout(predicate::str::contains("allowed.txt"));
    cli(&repo, &fake)?
        .args(["apply", &run_id, "--task", "worktree"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no commit was created"));
    assert_eq!(
        fs::read_to_string(repo.join("allowed.txt"))?,
        "changed by fake kiro\n"
    );
    cli(&repo, &fake)?
        .args(["accept", "missing-run", "--task", "worktree"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no parallel run found"));
    assert!(!repo.join(".kiro-sidecar/runs/missing-run").exists());
    cli(&repo, &fake)?
        .args(["reject", &run_id, "--task", "missing-task"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no task artifact found"));
    cli(&repo, &fake)?
        .args(["accept", &format!("{run_id}!"), "--task", "worktree"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no parallel run found"));
    cli(&repo, &fake)?
        .args(["reject", &run_id, "--task", "worktree!"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no task artifact found"));
    cli(&repo, &fake)?
        .args(["accept", &run_id, "--task", "worktree"])
        .assert()
        .success();
    cli(&repo, &fake)?
        .args(["reject", &run_id, "--task", "worktree"])
        .assert()
        .success();
    let verdict =
        fs::read_to_string(repo.join(format!(".kiro-sidecar/runs/{run_id}/verdict.json")))?;
    assert!(verdict.contains("\"verdict\": \"rejected\""));
    cli(&repo, &fake)?
        .args(["history", "--last", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&run_id));
    Ok(())
}

#[test]
fn diff_summary_without_task_preserves_original_task_id() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "work tree", "prompt": "Change allowed", "allow": ["allowed.txt"]}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let run_id = latest_run_id(&repo)?;
    cli(&repo, &fake)?
        .args(["diff-summary", &run_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("task=work tree"))
        .stdout(predicate::str::contains("allowed.txt"));
    cli(&repo, &fake)?
        .args(["accept", &run_id, "--task", "work tree"])
        .assert()
        .success();

    let verdict =
        fs::read_to_string(repo.join(format!(".kiro-sidecar/runs/{run_id}/verdict.json")))?;
    assert!(verdict.contains("\"work tree\""));
    assert!(!verdict.contains("\"work-tree\""));
    Ok(())
}

#[test]
fn diff_summary_with_task_rejects_metadata_mismatch() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    fs::write(
        repo.join("tasks.json"),
        serde_json::to_string(&json!([
            {"id": "worktree", "prompt": "Change allowed", "allow": ["allowed.txt"]}
        ]))?,
    )?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["parallel-worktree", "tasks.json", "--max-concurrency", "1"])
        .assert()
        .success();

    let run_id = latest_run_id(&repo)?;
    cli(&repo, &fake)?
        .args(["diff-summary", &run_id, "--task", "worktree!"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("no worktree patch found"))
        .stdout(predicate::str::contains("allowed.txt").not());
    Ok(())
}

#[test]
fn help_routes_to_kiro_help_agent() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "help_agent")
        .args(["help", "explain behavior"])
        .assert()
        .success()
        .stdout(predicate::str::contains("HELP_AGENT"));
    Ok(())
}

#[test]
fn symlink_target_outside_repo_is_blocked() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "outside-original\n")?;
    run_git(&repo, &["init", "-q"])?;
    run_git(&repo, &["config", "user.email", "test@example.com"])?;
    run_git(&repo, &["config", "user.name", "Test"])?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, repo.join("allowed.txt"))?;
    run_git(&repo, &["add", "allowed.txt"])?;
    run_git(&repo, &["commit", "-qm", "init"])?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["edit", "--allow", "allowed.txt", "change allowed"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "refusing allowed paths that resolve outside repo boundary",
        ));
    assert_eq!(fs::read_to_string(outside)?, "outside-original\n");
    Ok(())
}

#[test]
fn dangling_symlink_target_outside_repo_is_blocked() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    let outside = tmp.path().join("outside-created.txt");
    run_git(&repo, &["init", "-q"])?;
    run_git(&repo, &["config", "user.email", "test@example.com"])?;
    run_git(&repo, &["config", "user.name", "Test"])?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, repo.join("allowed.txt"))?;
    run_git(&repo, &["add", "allowed.txt"])?;
    run_git(&repo, &["commit", "-qm", "init"])?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "edit_allowed")
        .args(["edit", "--allow", "allowed.txt", "change allowed"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "refusing allowed paths that resolve outside repo boundary",
        ));
    assert!(!outside.exists());
    Ok(())
}

#[test]
fn status_handles_missing_kiro_cli() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;

    let mut command = Command::cargo_bin("kiro-sidecar")?;
    command
        .current_dir(&repo)
        .env("KIRO_CLI", tmp.path().join("missing-kiro-cli"))
        .env_remove("KIRO_EFFORT")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("- resolved: not found"))
        .stdout(predicate::str::contains("- version: not found"))
        .stdout(predicate::str::contains("EFFORT:\n- from Kiro settings"))
        .stderr(predicate::str::is_empty());
    Ok(())
}

fn single_results_file(repo: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let runs = repo.join(".kiro-sidecar/runs");
    let result_files = fs::read_dir(runs)?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("results.jsonl"))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    assert_eq!(result_files.len(), 1);
    Ok(result_files[0].clone())
}

fn latest_run_id(repo: &Path) -> Result<String, Box<dyn Error>> {
    let runs = repo.join(".kiro-sidecar/runs");
    let mut ids = fs::read_dir(runs)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("run_summary.json").exists())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    ids.sort();
    ids.pop().ok_or_else(|| "missing run id".into())
}

#[test]
fn cleanup_all_sidecar_keeps_unrelated_kiro_tmp() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    let tmp_root = tmp.path().join("tmp");
    let sidecar_tmp = tmp_root.join("kiro-sidecar-old");
    let unrelated_tmp = tmp_root.join("kiro-unrelated");
    fs::create_dir_all(&sidecar_tmp)?;
    fs::create_dir_all(&unrelated_tmp)?;

    cli(&repo, &fake)?
        .env("KIRO_TMP_ROOT", &tmp_root)
        .args(["cleanup", "--all-sidecar"])
        .assert()
        .success();
    assert!(!sidecar_tmp.exists());
    assert!(unrelated_tmp.exists());
    Ok(())
}

#[test]
fn cleanup_all_sidecar_preserves_profiles_toml() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    let sidecar = repo.join(".kiro-sidecar");
    fs::create_dir_all(sidecar.join("runs/old"))?;
    fs::write(
        sidecar.join("profiles.toml"),
        "[profiles.docs]\ntools = [\"fs_read\", \"grep\", \"glob\"]\n",
    )?;

    cli(&repo, &fake)?
        .args(["cleanup", "--all-sidecar"])
        .assert()
        .success();

    assert!(sidecar.join("profiles.toml").exists());
    assert!(!sidecar.join("runs").exists());
    Ok(())
}

#[test]
fn cleanup_ignores_malformed_profiles_toml() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;
    let sidecar = repo.join(".kiro-sidecar");
    fs::create_dir_all(sidecar.join("runs/stale-run"))?;
    fs::write(sidecar.join("runs/stale-run/output.txt"), "old\n")?;
    fs::write(sidecar.join("profiles.toml"), "profiles = [\n")?;

    cli(&repo, &fake)?
        .args(["cleanup", "--all-sidecar"])
        .assert()
        .success();

    assert!(sidecar.join("profiles.toml").exists());
    assert!(!sidecar.join("runs").exists());
    Ok(())
}

#[test]
fn run_kiro_times_out() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    init_repo(&repo)?;
    let fake = make_fake_kiro(tmp.path())?;

    cli(&repo, &fake)?
        .env("FAKE_KIRO_MODE", "hang")
        .env("KIRO_TIMEOUT_SECONDS", "1")
        .args(["explore", "hang"])
        .assert()
        .code(124)
        .stdout(predicate::str::contains("timed out after 1s"));
    Ok(())
}

#[test]
fn write_guard_allows_and_blocks_paths() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    fs::write(repo.join("allowed.txt"), "ok\n")?;
    let policy = tmp.path().join("policy.json");
    let block_log = tmp.path().join("blocked.log");
    fs::write(
        &policy,
        serde_json::to_string(&json!({
            "allow": ["./allowed.txt"],
            "deny": [],
            "repo_root": repo,
            "block_log": block_log
        }))?,
    )?;

    Command::cargo_bin("kiro-sidecar")?
        .args(["__write-guard", "--policy"])
        .arg(&policy)
        .write_stdin(r#"{"path":"allowed.txt"}"#)
        .assert()
        .success();

    Command::cargo_bin("kiro-sidecar")?
        .args(["__write-guard", "--policy"])
        .arg(&policy)
        .write_stdin(r#"{"path":"outside.txt"}"#)
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "path is not in allowed edit scope",
        ));
    Ok(())
}

#[test]
fn write_guard_blocks_symlink_target_outside_repo() -> Result<(), Box<dyn Error>> {
    let tmp = TempDir::new()?;
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo)?;
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "outside-original\n")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, repo.join("allowed.txt"))?;
    let policy = tmp.path().join("policy.json");
    let block_log = tmp.path().join("blocked.log");
    fs::write(
        &policy,
        serde_json::to_string(&json!({
            "allow": ["./allowed.txt"],
            "deny": [],
            "repo_root": repo,
            "block_log": block_log
        }))?,
    )?;

    Command::cargo_bin("kiro-sidecar")?
        .args(["__write-guard", "--policy"])
        .arg(&policy)
        .write_stdin(r#"{"path":"allowed.txt"}"#)
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "path resolves outside repo boundary",
        ));
    assert_eq!(fs::read_to_string(outside)?, "outside-original\n");
    Ok(())
}
