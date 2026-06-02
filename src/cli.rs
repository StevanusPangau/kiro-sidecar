use crate::config::{Config, PATCH_OUTPUT, STRUCTURED_OUTPUT};
use crate::events::utc_timestamp;
use crate::git_utils::{
    changed_files_from_patch, repo_root, run_git_owned, status_short, write_full_diff,
};
use crate::kiro::{kiro_version, list_sessions, run_kiro, settings_json, which_kiro};
use crate::parallel::{run_parallel, ParallelCommand, ParallelOptions, RunSummary};
use crate::paths::{new_run_id, normalize_repo_glob, safe_artifact_id};
use crate::profiles::{ProfileCatalog, ProfileMode};
use crate::task_schema::{load_tasks, TaskValidationMode};
use crate::writer::{bounded_edit, run_write_guard, worktree_edit, EditRequest};
use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "kiro-sidecar")]
#[command(about = "Parallel-safe Kiro CLI sidecar orchestration for Codex")]
#[command(disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Explore {
        #[arg(long = "profile")]
        profile: Option<String>,
        prompt: String,
    },
    Help {
        prompt: String,
    },
    Review {
        #[arg(long = "profile")]
        profile: Option<String>,
        #[arg(long = "format", value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        prompt: Option<String>,
    },
    #[command(name = "audit-diff")]
    AuditDiff {
        #[arg(long = "profile")]
        profile: Option<String>,
        #[arg(long = "format", value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        prompt: Option<String>,
    },
    Edit(ScopedPrompt),
    Patch(ScopedPrompt),
    #[command(name = "edit-worktree")]
    EditWorktree(ScopedPrompt),
    #[command(name = "parallel-explore")]
    ParallelExplore(ParallelArgs),
    #[command(name = "parallel-review")]
    ParallelReview(ParallelArgs),
    #[command(name = "parallel-worktree")]
    ParallelWorktree(ParallelArgs),
    Validate {
        tasks: PathBuf,
    },
    History {
        #[arg(long = "last", default_value_t = 10)]
        last: usize,
        #[arg(long = "format", value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    #[command(name = "diff-summary")]
    DiffSummary {
        run_id: String,
        #[arg(long = "task")]
        task: Option<String>,
        #[arg(long = "format", value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    Apply {
        run_id: String,
        #[arg(long = "task")]
        task: String,
    },
    Accept {
        run_id: String,
        #[arg(long = "task")]
        task: String,
    },
    Reject {
        run_id: String,
        #[arg(long = "task")]
        task: String,
    },
    Status,
    Cleanup {
        #[arg(long = "all-sidecar")]
        all_sidecar: bool,
    },
    #[command(name = "__write-guard", hide = true)]
    WriteGuard {
        #[arg(long)]
        policy: PathBuf,
    },
}

#[derive(Debug, clap::Args)]
struct ScopedPrompt {
    #[arg(long = "profile")]
    profile: Option<String>,
    #[arg(long = "allow")]
    allow: Vec<String>,
    #[arg(long = "deny")]
    deny: Vec<String>,
    prompt: String,
}

#[derive(Debug, clap::Args)]
struct ParallelArgs {
    tasks: PathBuf,
    #[arg(long = "max-concurrency", default_value_t = 6)]
    max_concurrency: usize,
    #[arg(long = "profile")]
    profile: Option<String>,
    #[arg(long = "fail-fast")]
    fail_fast: bool,
    #[arg(long = "format", value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

impl OutputFormat {
    fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }
}

pub async fn run() -> Result<i32> {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        Cli::command().print_help()?;
        println!();
        return Ok(0);
    };
    let config = Config::from_env();
    let cwd = std::env::current_dir()?;
    let root = repo_root(&cwd).await;
    let load_profiles = || ProfileCatalog::load(&root);
    let code = match command {
        Command::Explore { profile, prompt } => {
            let profiles = load_profiles()?;
            let profile =
                profiles.resolve(profile.as_deref(), "read-only", ProfileMode::ReadOnly)?;
            let trust_tools = profile.trust_tools();
            let run_dir = config
                .tmp_root
                .join(format!("kiro-sidecar-{}", new_run_id("explore")));
            let _cleanup = RunDirCleanup::new(&run_dir);
            let result = run_kiro(
                &config,
                &cwd,
                &format!("{STRUCTURED_OUTPUT} Request: {prompt}"),
                &run_dir,
                Some(&trust_tools),
                None,
                true,
            )
            .await;
            print!("{}", result.output);
            result.returncode
        }
        Command::Help { prompt } => {
            let profiles = load_profiles()?;
            let profile = profiles.resolve(None, "read-only", ProfileMode::ReadOnly)?;
            let trust_tools = profile.trust_tools();
            let run_dir = config
                .tmp_root
                .join(format!("kiro-sidecar-{}", new_run_id("help")));
            let _cleanup = RunDirCleanup::new(&run_dir);
            let result = run_kiro(
                &config,
                &cwd,
                &format!("{STRUCTURED_OUTPUT} Request: {prompt}"),
                &run_dir,
                Some(&trust_tools),
                Some("kiro_help"),
                true,
            )
            .await;
            print!("{}", result.output);
            result.returncode
        }
        Command::Review {
            profile,
            format,
            prompt,
        } => {
            let profiles = load_profiles()?;
            let profile =
                profiles.resolve(profile.as_deref(), "read-only", ProfileMode::ReadOnly)?;
            let trust_tools = profile.trust_tools();
            let run_dir = config
                .tmp_root
                .join(format!("kiro-sidecar-{}", new_run_id("review")));
            let _cleanup = RunDirCleanup::new(&run_dir);
            review(
                &config,
                &cwd,
                prompt.unwrap_or_default(),
                &run_dir,
                &trust_tools,
                format,
            )
            .await?
        }
        Command::AuditDiff {
            profile,
            format,
            prompt,
        } => {
            let profiles = load_profiles()?;
            let profile =
                profiles.resolve(profile.as_deref(), "read-only", ProfileMode::ReadOnly)?;
            let trust_tools = profile.trust_tools();
            let run_dir = config
                .tmp_root
                .join(format!("kiro-sidecar-{}", new_run_id("audit-diff")));
            let _cleanup = RunDirCleanup::new(&run_dir);
            audit(
                &config,
                &cwd,
                prompt.unwrap_or_default(),
                &run_dir,
                &trust_tools,
                format,
            )
            .await?
        }
        Command::Edit(args) => {
            let profiles = load_profiles()?;
            require_allow("edit", &args.allow)?;
            let profile = profiles.resolve(
                args.profile.as_deref(),
                "scoped-edit",
                ProfileMode::ScopedEdit,
            )?;
            let trust_tools = profile.trust_tools();
            let run_dir = config
                .tmp_root
                .join(format!("kiro-sidecar-{}", new_run_id("edit")));
            bounded_edit(
                &config,
                &cwd,
                EditRequest {
                    prompt: &args.prompt,
                    allow: &args.allow,
                    deny: &args.deny,
                    run_dir: &run_dir,
                    trust_tools: &trust_tools,
                    emit_output: true,
                },
            )
            .await?
            .status
        }
        Command::Patch(args) => {
            let profiles = load_profiles()?;
            require_allow("patch", &args.allow)?;
            let profile =
                profiles.resolve(args.profile.as_deref(), "read-only", ProfileMode::ReadOnly)?;
            let trust_tools = profile.trust_tools();
            let run_dir = config
                .tmp_root
                .join(format!("kiro-sidecar-{}", new_run_id("patch")));
            let _cleanup = RunDirCleanup::new(&run_dir);
            patch(
                &config,
                &cwd,
                &args.prompt,
                &args.allow,
                &args.deny,
                &run_dir,
                &trust_tools,
            )
            .await?
        }
        Command::EditWorktree(args) => {
            let profiles = load_profiles()?;
            require_allow("edit-worktree", &args.allow)?;
            let profile = profiles.resolve(
                args.profile.as_deref(),
                "worktree-edit",
                ProfileMode::WorktreeEdit,
            )?;
            let trust_tools = profile.trust_tools();
            let run_dir = config
                .tmp_root
                .join(format!("kiro-sidecar-{}", new_run_id("edit-worktree")));
            worktree_edit(
                &config,
                &cwd,
                EditRequest {
                    prompt: &args.prompt,
                    allow: &args.allow,
                    deny: &args.deny,
                    run_dir: &run_dir,
                    trust_tools: &trust_tools,
                    emit_output: true,
                },
            )
            .await?
            .status
        }
        Command::ParallelExplore(args) => {
            let profiles = load_profiles()?;
            let tasks = load_tasks(&args.tasks, &profiles, TaskValidationMode::Generic)?;
            run_parallel(
                &config,
                &root,
                tasks,
                ParallelCommand::Explore,
                parallel_options(&args),
                &profiles,
            )
            .await?
        }
        Command::ParallelReview(args) => {
            let profiles = load_profiles()?;
            let tasks = load_tasks(&args.tasks, &profiles, TaskValidationMode::Generic)?;
            run_parallel(
                &config,
                &root,
                tasks,
                ParallelCommand::Review,
                parallel_options(&args),
                &profiles,
            )
            .await?
        }
        Command::ParallelWorktree(args) => {
            let profiles = load_profiles()?;
            let tasks = load_tasks(&args.tasks, &profiles, TaskValidationMode::Write)?;
            run_parallel(
                &config,
                &root,
                tasks,
                ParallelCommand::Worktree,
                parallel_options(&args),
                &profiles,
            )
            .await?
        }
        Command::Validate { tasks } => {
            let profiles = load_profiles()?;
            let tasks = load_tasks(&tasks, &profiles, TaskValidationMode::Generic)?;
            println!("kiro-sidecar: task file is valid ({} task(s))", tasks.len());
            0
        }
        Command::History { last, format } => history(&root, last, format)?,
        Command::DiffSummary {
            run_id,
            task,
            format,
        } => diff_summary(&root, &run_id, task.as_deref(), format).await?,
        Command::Apply { run_id, task } => apply_run_patch(&root, &run_id, &task).await?,
        Command::Accept { run_id, task } => record_verdict(&root, &run_id, &task, "accepted")?,
        Command::Reject { run_id, task } => record_verdict(&root, &run_id, &task, "rejected")?,
        Command::Status => {
            let profiles = load_profiles()?;
            status(&config, &cwd, &profiles).await?
        }
        Command::Cleanup { all_sidecar } => cleanup(&config, &cwd, all_sidecar).await?,
        Command::WriteGuard { policy } => {
            let mut stdin = String::new();
            std::io::stdin().read_to_string(&mut stdin)?;
            run_write_guard(&policy, &stdin)?
        }
    };
    Ok(code)
}

async fn review(
    config: &Config,
    cwd: &Path,
    prompt: String,
    run_dir: &Path,
    trust_tools: &str,
    format: OutputFormat,
) -> Result<i32> {
    let diff_file = run_dir.join("review.diff");
    write_full_diff(cwd, &diff_file).await?;
    if std::fs::read_to_string(&diff_file)?.is_empty() {
        print_command_output(
            format,
            0,
            "kiro-sidecar: no uncommitted diff found against HEAD\n",
        )?;
        return Ok(0);
    }
    let focus = if prompt.is_empty() {
        "Review the uncommitted changes in this repository. Prioritize concrete bugs, regressions, security risks, \
API behavior changes, data integrity risks, and missing tests. Output findings first."
            .to_string()
    } else {
        prompt
    };
    let result = run_kiro(
        config,
        cwd,
        &format!(
            "Review the uncommitted repository changes in @{}. {STRUCTURED_OUTPUT} {focus}",
            diff_file.display()
        ),
        run_dir,
        Some(trust_tools),
        None,
        true,
    )
    .await;
    print_command_output(format, result.returncode, &result.output)?;
    Ok(result.returncode)
}

async fn audit(
    config: &Config,
    cwd: &Path,
    prompt: String,
    run_dir: &Path,
    trust_tools: &str,
    format: OutputFormat,
) -> Result<i32> {
    let diff_file = run_dir.join("audit.diff");
    write_full_diff(cwd, &diff_file).await?;
    if std::fs::read_to_string(&diff_file)?.is_empty() {
        print_command_output(
            format,
            0,
            "kiro-sidecar: no uncommitted diff found against HEAD\n",
        )?;
        return Ok(0);
    }
    let focus = if prompt.is_empty() {
        "Audit this diff after a Kiro sidecar run. Focus on correctness, unrelated changes, missing tests, \
security risk, data integrity risk, and whether Codex should accept or reject the patch. Do not modify files."
            .to_string()
    } else {
        prompt
    };
    let result = run_kiro(
        config,
        cwd,
        &format!(
            "Audit the uncommitted repository diff in @{}. {STRUCTURED_OUTPUT} {focus}",
            diff_file.display()
        ),
        run_dir,
        Some(trust_tools),
        None,
        true,
    )
    .await;
    print_command_output(format, result.returncode, &result.output)?;
    Ok(result.returncode)
}

async fn patch(
    config: &Config,
    cwd: &Path,
    prompt: &str,
    allow: &[String],
    deny: &[String],
    run_dir: &Path,
    trust_tools: &str,
) -> Result<i32> {
    let allow = allow
        .iter()
        .map(|item| normalize_repo_glob(item))
        .collect::<Result<Vec<_>>>()?;
    let deny = deny
        .iter()
        .map(|item| normalize_repo_glob(item))
        .collect::<Result<Vec<_>>>()?;
    let before = status_short(cwd).await?;
    let result = run_kiro(
        config,
        cwd,
        &format!(
            "Create a unified diff patch for this bounded request without modifying the working tree. \
Allowed paths: {}. Denied paths: {}. \
If any required change is outside the allowed paths, return an empty PATCH block and explain the \
missing scope inside diff comments only. \
{PATCH_OUTPUT} Request: {prompt}",
            allow.join(" "),
            deny.join(" ")
        ),
        run_dir,
        Some(trust_tools),
        None,
        true,
    )
    .await;
    let after = status_short(cwd).await?;
    print!("{}", result.output);
    if before != after {
        println!("kiro-sidecar: patch mode changed the working tree; refusing result");
        return Ok(1);
    }
    Ok(result.returncode)
}

fn require_allow(command: &str, allow: &[String]) -> Result<()> {
    if allow.is_empty() {
        anyhow::bail!("kiro-sidecar: {command} requires at least one --allow path");
    }
    Ok(())
}

fn parallel_options(args: &ParallelArgs) -> ParallelOptions {
    ParallelOptions {
        max_concurrency: args.max_concurrency,
        fail_fast: args.fail_fast,
        profile: args.profile.clone(),
        format_json: args.format.is_json(),
    }
}

#[derive(Serialize)]
struct CommandJsonOutput<'a> {
    status: &'a str,
    returncode: i32,
    output: &'a str,
}

fn print_command_output(format: OutputFormat, returncode: i32, output: &str) -> Result<()> {
    if format.is_json() {
        println!(
            "{}",
            serde_json::to_string_pretty(&CommandJsonOutput {
                status: if returncode == 0 { "ok" } else { "failed" },
                returncode,
                output,
            })?
        );
    } else {
        print!("{output}");
    }
    Ok(())
}

async fn status(config: &Config, cwd: &Path, profiles: &ProfileCatalog) -> Result<i32> {
    println!("KIRO_SIDECAR_STATUS:");
    println!("WRAPPER:\n- {}", std::env::current_exe()?.display());
    println!(
        "KIRO_CLI:\n- command: {}\n- resolved: {}\n- version: {}",
        config.kiro_cli,
        which_kiro(config)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not found".to_string()),
        kiro_version(config).await
    );
    println!("MODEL:\n- {}", config.model);
    println!(
        "TRUST_TOOLS:\n- read_only: {}\n- edit: {}",
        config.read_tools, config.edit_tools
    );
    println!("PROFILES:");
    for name in profiles.names() {
        let read_profile = profiles
            .resolve(Some(&name), "read-only", ProfileMode::ReadOnly)
            .ok()
            .map(|profile| profile.trust_tools());
        let edit_profile = profiles
            .resolve(Some(&name), "scoped-edit", ProfileMode::ScopedEdit)
            .ok()
            .or_else(|| {
                profiles
                    .resolve(Some(&name), "worktree-edit", ProfileMode::WorktreeEdit)
                    .ok()
            })
            .map(|profile| profile.trust_tools());
        let tools = read_profile
            .or(edit_profile)
            .unwrap_or_else(|| "invalid".to_string());
        println!("- {name}: {tools}");
    }
    println!("GLOBAL_CONFIG:");
    if let Some(settings) = settings_json(config).await {
        if let Ok(data) = serde_json::from_str::<Value>(&settings) {
            println!(
                "- chat.defaultModel: {}",
                display_json_value(data.get("chat.defaultModel"))
            );
            println!(
                "- chat.modelDefaults: {}",
                data.get("chat.modelDefaults")
                    .map(|value| serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()))
                    .unwrap_or_else(|| "{}".to_string())
            );
            println!(
                "- cleanup.periodDays: {}",
                display_json_value(data.get("cleanup.periodDays"))
            );
        } else {
            println!("- unavailable");
        }
    } else {
        println!("- unavailable");
    }
    let sessions = list_sessions(config, cwd).await;
    println!("SAVED_SESSIONS:");
    if sessions.is_empty() {
        println!("- none detected");
    } else {
        for session in sessions {
            println!("- {session}");
        }
    }
    println!("TEMP_FILES:");
    let tmp_files = sidecar_tmp_files(&config.tmp_root);
    if tmp_files.is_empty() {
        println!("- none detected");
    } else {
        for path in tmp_files {
            println!("- {}", path.display());
        }
    }
    println!("TEMP_AGENTS:");
    let agents = temp_agents(&repo_root(cwd).await.join(&config.agent_dir));
    if agents.is_empty() {
        println!("- none detected");
    } else {
        for path in agents {
            println!("- {}", path.display());
        }
    }
    Ok(0)
}

async fn cleanup(config: &Config, cwd: &Path, all_sidecar: bool) -> Result<i32> {
    for path in sidecar_tmp_files(&config.tmp_root) {
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    if all_sidecar {
        let root = repo_root(cwd).await;
        cleanup_run_artifacts(&root.join(".kiro-sidecar"));
        for agent in temp_agents(&root.join(&config.agent_dir)) {
            let _ = std::fs::remove_file(agent);
        }
    }
    println!("kiro-sidecar: cleanup completed");
    Ok(0)
}

fn cleanup_run_artifacts(sidecar_dir: &Path) {
    let _ = std::fs::remove_dir_all(sidecar_dir.join("runs"));
    if std::fs::read_dir(sidecar_dir)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(sidecar_dir);
    }
}

fn history(root: &Path, last: usize, format: OutputFormat) -> Result<i32> {
    let runs_dir = root.join(".kiro-sidecar").join("runs");
    let mut summaries = std::fs::read_dir(&runs_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(std::result::Result::ok))
        .filter_map(|entry| {
            let path = entry.path().join("run_summary.json");
            let text = std::fs::read_to_string(path).ok()?;
            serde_json::from_str::<RunSummary>(&text).ok()
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    summaries.truncate(last);
    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
    } else if summaries.is_empty() {
        println!("kiro-sidecar: no run history found");
    } else {
        for summary in summaries {
            println!(
                "{} {} tasks={} failures={} dir={}",
                summary.run_id,
                summary.status,
                summary.task_count,
                summary.failures,
                summary.run_dir
            );
        }
    }
    Ok(0)
}

#[derive(Debug, Serialize)]
struct PatchSummary {
    run_id: String,
    task_id: String,
    patch_file: String,
    changed_files: Vec<String>,
    insertions: usize,
    deletions: usize,
    lines: usize,
    bytes: usize,
}

async fn diff_summary(
    root: &Path,
    run_id: &str,
    task_id: Option<&str>,
    format: OutputFormat,
) -> Result<i32> {
    let summaries = collect_patch_summaries(root, run_id, task_id).await?;
    if summaries.is_empty() {
        if format.is_json() {
            println!("[]");
        } else {
            println!("kiro-sidecar: no worktree patch found for run {run_id}");
        }
        return Ok(1);
    }
    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
    } else {
        for summary in summaries {
            println!(
                "{} task={} files={} +{} -{} lines={} bytes={}",
                summary.run_id,
                summary.task_id,
                summary.changed_files.join(","),
                summary.insertions,
                summary.deletions,
                summary.lines,
                summary.bytes
            );
            println!("- patch: {}", summary.patch_file);
        }
    }
    Ok(0)
}

async fn collect_patch_summaries(
    root: &Path,
    run_id: &str,
    task_id: Option<&str>,
) -> Result<Vec<PatchSummary>> {
    let Some((_summary, run_dir)) = run_artifact(root, run_id)? else {
        return Ok(Vec::new());
    };
    let task_dirs = if let Some(task_id) = task_id {
        match task_dir(&run_dir, task_id) {
            Some(task_dir) => vec![(Some(task_id.to_string()), task_dir)],
            None => Vec::new(),
        }
    } else {
        std::fs::read_dir(run_dir.join("tasks"))
            .ok()
            .into_iter()
            .flat_map(|entries| entries.filter_map(std::result::Result::ok))
            .filter(|entry| entry.path().is_dir())
            .map(|entry| (None, entry.path()))
            .collect::<Vec<_>>()
    };
    let mut summaries = Vec::new();
    for (task_id, task_dir) in task_dirs {
        let metadata_task_id = metadata_task_id(&task_dir)?;
        let task_id = match task_id {
            Some(task_id) => {
                if metadata_task_id.as_deref() != Some(task_id.as_str()) {
                    continue;
                }
                task_id
            }
            None => metadata_task_id.unwrap_or_else(|| task_dir_name(&task_dir)),
        };
        let patch_file = task_dir.join("worktree.patch");
        if !patch_file.exists() {
            continue;
        }
        let patch = std::fs::read_to_string(&patch_file)?;
        let stats = patch_stats(&patch);
        summaries.push(PatchSummary {
            run_id: run_id.to_string(),
            task_id,
            patch_file: patch_file.display().to_string(),
            changed_files: changed_files_from_patch(root, &patch_file).await?,
            insertions: stats.0,
            deletions: stats.1,
            lines: patch.lines().count(),
            bytes: patch.len(),
        });
    }
    Ok(summaries)
}

fn metadata_task_id(task_dir: &Path) -> Result<Option<String>> {
    let path = task_dir.join("metadata.json");
    if !path.is_file() {
        return Ok(None);
    }
    let metadata = serde_json::from_str::<Value>(&std::fs::read_to_string(path)?)?;
    let task_id = metadata.pointer("/task/id").and_then(Value::as_str);
    let record_id = metadata.pointer("/record/id").and_then(Value::as_str);
    Ok(match (task_id, record_id) {
        (Some(task_id), Some(record_id)) if task_id == record_id => Some(task_id.to_string()),
        _ => None,
    })
}

fn task_dir_name(task_dir: &Path) -> String {
    task_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

async fn apply_run_patch(root: &Path, run_id: &str, task_id: &str) -> Result<i32> {
    let Some((summary, metadata)) = task_artifact(root, run_id, task_id)? else {
        return Ok(1);
    };
    if summary.command != "parallel-worktree" {
        println!("kiro-sidecar: no worktree patch found for run {run_id} task {task_id}");
        return Ok(1);
    }
    let status = metadata
        .pointer("/record/status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if status != "ok" {
        println!("kiro-sidecar: refusing to apply patch because task {task_id} status is {status}");
        return Ok(1);
    }

    let Some(patch_file) = task_patch(root, run_id, task_id) else {
        println!("kiro-sidecar: no worktree patch found for run {run_id} task {task_id}");
        return Ok(1);
    };
    if !patch_file.exists() {
        println!("kiro-sidecar: no worktree patch found for run {run_id} task {task_id}");
        return Ok(1);
    }
    let patch = std::fs::read_to_string(&patch_file)?;
    if patch.trim().is_empty() {
        println!("kiro-sidecar: refusing to apply empty patch");
        return Ok(1);
    }
    for file in changed_files_from_patch(root, &patch_file).await? {
        let result = run_git_owned(
            vec![
                "status".to_string(),
                "--short".to_string(),
                "--".to_string(),
                file.clone(),
            ],
            root,
        )
        .await?;
        if !result.stdout.trim().is_empty() {
            println!("kiro-sidecar: refusing to apply patch because {file} is already dirty");
            return Ok(1);
        }
    }
    let check = run_git_owned(
        vec![
            "apply".to_string(),
            "--check".to_string(),
            patch_file.display().to_string(),
        ],
        root,
    )
    .await?;
    if check.returncode != 0 {
        print!("{}", check.stdout);
        print!("{}", check.stderr);
        return Ok(check.returncode);
    }
    let apply = run_git_owned(
        vec!["apply".to_string(), patch_file.display().to_string()],
        root,
    )
    .await?;
    print!("{}", apply.stdout);
    print!("{}", apply.stderr);
    if apply.returncode == 0 {
        println!("kiro-sidecar: applied run {run_id} task {task_id}; no commit was created");
    }
    Ok(apply.returncode)
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct VerdictFile {
    run_id: String,
    #[serde(default)]
    decisions: BTreeMap<String, VerdictDecision>,
}

#[derive(Debug, Deserialize, Serialize)]
struct VerdictDecision {
    verdict: String,
    decided_at: String,
}

fn record_verdict(root: &Path, run_id: &str, task_id: &str, verdict: &str) -> Result<i32> {
    if task_artifact(root, run_id, task_id)?.is_none() {
        return Ok(1);
    }
    let Some(run_dir) = run_dir(root, run_id) else {
        println!("kiro-sidecar: no parallel run found for run {run_id}");
        return Ok(1);
    };
    let path = run_dir.join("verdict.json");
    if !path.is_file() {
        println!("kiro-sidecar: no verdict artifact found for run {run_id}");
        return Ok(1);
    }
    let mut file = serde_json::from_str::<VerdictFile>(&std::fs::read_to_string(&path)?)?;
    if file.run_id != run_id {
        println!("kiro-sidecar: verdict artifact does not match run {run_id}");
        return Ok(1);
    }
    file.decisions.insert(
        task_id.to_string(),
        VerdictDecision {
            verdict: verdict.to_string(),
            decided_at: utc_timestamp(),
        },
    );
    std::fs::write(&path, serde_json::to_string_pretty(&file)? + "\n")?;
    println!("kiro-sidecar: recorded {verdict} for run {run_id} task {task_id}");
    Ok(0)
}

fn task_artifact(root: &Path, run_id: &str, task_id: &str) -> Result<Option<(RunSummary, Value)>> {
    let Some((summary, run_dir)) = run_artifact(root, run_id)? else {
        println!("kiro-sidecar: no parallel run found for run {run_id}");
        return Ok(None);
    };

    let Some(task_dir) = task_dir(&run_dir, task_id) else {
        println!("kiro-sidecar: no task artifact found for run {run_id} task {task_id}");
        return Ok(None);
    };
    let metadata_path = task_dir.join("metadata.json");
    if !metadata_path.is_file() {
        println!("kiro-sidecar: no task artifact found for run {run_id} task {task_id}");
        return Ok(None);
    }
    let metadata = serde_json::from_str::<Value>(&std::fs::read_to_string(metadata_path)?)?;
    if metadata.pointer("/task/id").and_then(Value::as_str) != Some(task_id)
        || metadata.pointer("/record/id").and_then(Value::as_str) != Some(task_id)
    {
        println!("kiro-sidecar: no task artifact found for run {run_id} task {task_id}");
        return Ok(None);
    }

    Ok(Some((summary, metadata)))
}

fn run_artifact(root: &Path, run_id: &str) -> Result<Option<(RunSummary, PathBuf)>> {
    let Some(run_dir) = run_dir(root, run_id) else {
        return Ok(None);
    };
    let summary_path = run_dir.join("run_summary.json");
    if !summary_path.is_file() {
        return Ok(None);
    }
    let summary = serde_json::from_str::<RunSummary>(&std::fs::read_to_string(summary_path)?)?;
    if summary.run_id != run_id {
        return Ok(None);
    }
    Ok(Some((summary, run_dir)))
}

fn run_dir(root: &Path, run_id: &str) -> Option<PathBuf> {
    Some(
        root.join(".kiro-sidecar")
            .join("runs")
            .join(safe_artifact_id(run_id)?),
    )
}

fn task_dir(run_dir: &Path, task_id: &str) -> Option<PathBuf> {
    Some(run_dir.join("tasks").join(safe_artifact_id(task_id)?))
}

fn task_patch(root: &Path, run_id: &str, task_id: &str) -> Option<PathBuf> {
    Some(task_dir(&run_dir(root, run_id)?, task_id)?.join("worktree.patch"))
}

fn patch_stats(patch: &str) -> (usize, usize) {
    let insertions = patch
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    let deletions = patch
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();
    (insertions, deletions)
}

fn display_json_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}

fn sidecar_tmp_files(tmp_root: &Path) -> Vec<PathBuf> {
    let mut paths = std::fs::read_dir(tmp_root)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(std::result::Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("kiro-sidecar-"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn temp_agents(agent_dir: &Path) -> Vec<PathBuf> {
    let mut paths = std::fs::read_dir(agent_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(std::result::Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("codex_kiro_writer_") && name.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

struct RunDirCleanup {
    path: PathBuf,
}

impl RunDirCleanup {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for RunDirCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
