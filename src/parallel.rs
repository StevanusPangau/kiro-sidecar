use crate::config::{Config, STRUCTURED_OUTPUT};
use crate::events::{append_event, utc_timestamp, EventRecord};
use crate::git_utils::{changed_files_from_patch, repo_root, write_full_diff};
use crate::kiro::{delete_sessions, list_sessions, run_kiro};
use crate::paths::{new_run_id, path_candidates, safe_artifact_id};
use crate::profiles::{ProfileCatalog, ProfileMode};
use crate::task_schema::{dependency_batches, Task};
use crate::writer::{worktree_edit, EditOutcome, EditRequest};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};

const PATCH_LIMIT_VIOLATION: &str = "kiro-sidecar: patch limit violation:";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskRecord {
    pub id: String,
    pub status: String,
    pub returncode: i32,
    pub output: String,
    pub task_dir: String,
    pub profile: String,
    pub attempts: u32,
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskSummary {
    pub id: String,
    pub status: String,
    pub returncode: i32,
    pub task_dir: String,
    pub profile: String,
    pub attempts: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RunSummary {
    pub run_id: String,
    pub command: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub task_count: usize,
    pub failures: usize,
    pub run_dir: String,
    pub tasks: Vec<TaskSummary>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParallelCommand {
    Explore,
    Review,
    Worktree,
}

impl ParallelCommand {
    pub fn run_id_prefix(self) -> &'static str {
        match self {
            Self::Explore => "parallel-explore",
            Self::Review => "parallel-review",
            Self::Worktree => "parallel-worktree",
        }
    }

    pub fn default_profile(self) -> &'static str {
        match self {
            Self::Explore | Self::Review => "read-only",
            Self::Worktree => "worktree-edit",
        }
    }

    pub fn profile_mode(self) -> ProfileMode {
        match self {
            Self::Explore | Self::Review => ProfileMode::ReadOnly,
            Self::Worktree => ProfileMode::WorktreeEdit,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ParallelOptions {
    pub max_concurrency: usize,
    pub fail_fast: bool,
    pub profile: Option<String>,
    pub format_json: bool,
}

pub async fn run_parallel(
    config: &Config,
    cwd: &Path,
    tasks: Vec<Task>,
    command: ParallelCommand,
    options: ParallelOptions,
    profiles: &ProfileCatalog,
) -> Result<i32> {
    let root = repo_root(cwd).await;
    let run_id = new_run_id(command.run_id_prefix());
    let run_root = root.join(".kiro-sidecar").join("runs").join(&run_id);
    let tasks_root = run_root.join("tasks");
    std::fs::create_dir_all(&tasks_root)?;
    let results_file = run_root.join("results.jsonl");
    let events_file = run_root.join("events.jsonl");
    let run_summary_file = run_root.join("run_summary.json");
    let verdict_file = run_root.join("verdict.json");
    std::fs::write(&verdict_file, initial_verdict_json(&run_id)?)?;

    let started_at = utc_timestamp();
    let mut summary = RunSummary {
        run_id: run_id.clone(),
        command: command.run_id_prefix().to_string(),
        status: "running".to_string(),
        started_at,
        finished_at: None,
        task_count: tasks.len(),
        failures: 0,
        run_dir: run_root.display().to_string(),
        tasks: Vec::new(),
    };
    write_run_summary(&run_summary_file, &summary)?;

    let events_lock = Arc::new(Mutex::new(()));
    let results_lock = Arc::new(Mutex::new(()));
    let event_context = EventContext {
        run_id: run_id.clone(),
        events_file: events_file.clone(),
        lock: events_lock.clone(),
    };
    event_context
        .emit(None, "run_started", "parallel run started")
        .await?;

    let before_sessions = list_sessions(config, &root).await;
    if !options.format_json {
        println!(
            "kiro-sidecar: starting {} run {} with {} task(s), max concurrency {}",
            command.run_id_prefix(),
            run_id,
            tasks.len(),
            options.max_concurrency
        );
    }

    let batches = dependency_batches(&tasks)?;
    let max_concurrency = options.max_concurrency.max(1);
    let config = Arc::new(config.clone());
    let root = Arc::new(root);
    let tasks_root = Arc::new(tasks_root);
    let results_file = Arc::new(results_file);
    let profiles = Arc::new(profiles.clone());
    let cli_profile = Arc::new(options.profile.clone());
    let resource_locks = Arc::new(resource_locks(&tasks));
    let mut ok_tasks = BTreeSet::new();
    let mut failed_tasks = BTreeSet::new();
    let mut recorded_indexes = BTreeSet::new();
    let mut records = Vec::new();
    let mut failures = 0_usize;

    for batch in batches {
        if options.fail_fast && failures > 0 {
            for index in batch {
                let record = skipped_record(
                    &tasks[index],
                    &tasks_root,
                    "fail-fast stopped scheduling remaining tasks",
                );
                write_record(&results_file, &results_lock, &record).await?;
                records.push(record);
                recorded_indexes.insert(index);
            }
            continue;
        }

        let mut pending = batch.into_iter().collect::<VecDeque<_>>();
        let mut handles = JoinSet::new();
        loop {
            while handles.len() < max_concurrency && !(options.fail_fast && failures > 0) {
                let Some(index) = pending.pop_front() else {
                    break;
                };
                if tasks[index]
                    .depends_on
                    .iter()
                    .any(|dependency| failed_tasks.contains(dependency))
                {
                    let record = skipped_record(
                        &tasks[index],
                        &tasks_root,
                        "dependency failed or was skipped",
                    );
                    write_record(&results_file, &results_lock, &record).await?;
                    failed_tasks.insert(tasks[index].id.clone());
                    failures += 1;
                    records.push(record);
                    recorded_indexes.insert(index);
                    continue;
                }
                let config = config.clone();
                let root = root.clone();
                let task_root = tasks_root.clone();
                let results_file = results_file.clone();
                let results_lock = results_lock.clone();
                let profiles = profiles.clone();
                let cli_profile = cli_profile.clone();
                let task = tasks[index].clone();
                let event_context = event_context.clone();
                let resource_locks = resource_locks.clone();
                handles.spawn(async move {
                    let record = run_task(
                        TaskRunContext {
                            config: &config,
                            root: &root,
                            tasks_root: &task_root,
                            command,
                            profiles: &profiles,
                            cli_profile: cli_profile.as_ref().as_ref(),
                            events: &event_context,
                            resource_locks: &resource_locks,
                        },
                        task,
                    )
                    .await;
                    write_record(&results_file, &results_lock, &record).await?;
                    Ok::<TaskRecord, anyhow::Error>(record)
                });
                recorded_indexes.insert(index);
            }

            if options.fail_fast && failures > 0 {
                for index in pending.drain(..) {
                    let record = skipped_record(
                        &tasks[index],
                        &tasks_root,
                        "fail-fast stopped scheduling remaining tasks",
                    );
                    write_record(&results_file, &results_lock, &record).await?;
                    records.push(record);
                    recorded_indexes.insert(index);
                }
            }

            let Some(result) = handles.join_next().await else {
                break;
            };
            let record = match result {
                Ok(Ok(record)) => record,
                Ok(Err(error)) => {
                    failures += 1;
                    eprintln!("kiro-sidecar: could not record parallel task: {error:#}");
                    continue;
                }
                Err(error) => {
                    failures += 1;
                    eprintln!("kiro-sidecar: parallel task join failed: {error}");
                    continue;
                }
            };
            if record.status == "ok" {
                ok_tasks.insert(record.id.clone());
            } else {
                failed_tasks.insert(record.id.clone());
                failures += 1;
            }
            if !options.format_json {
                println!("- {}: {} ({})", record.id, record.status, record.task_dir);
            }
            records.push(record);
        }
    }

    for (index, task) in tasks.iter().enumerate() {
        if !recorded_indexes.contains(&index) {
            let record = skipped_record(task, &tasks_root, "task was not scheduled");
            write_record(&results_file, &results_lock, &record).await?;
            failed_tasks.insert(task.id.clone());
            failures += 1;
            records.push(record);
        }
    }

    let new_sessions = list_sessions(&config, &root)
        .await
        .difference(&before_sessions)
        .cloned()
        .collect::<Vec<_>>();
    let failed_cleanup = delete_sessions(&config, &root, &new_sessions).await;
    for session_id in failed_cleanup {
        eprintln!("kiro-sidecar: could not delete active Kiro session {session_id}");
    }

    summary.status = if failures == 0 { "ok" } else { "failed" }.to_string();
    summary.finished_at = Some(utc_timestamp());
    summary.failures = failures;
    summary.tasks = records.iter().map(TaskSummary::from).collect();
    write_run_summary(&run_summary_file, &summary)?;
    event_context
        .emit(None, "run_finished", &format!("status={}", summary.status))
        .await?;

    if options.format_json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "kiro-sidecar: results written to {}",
            results_file.display()
        );
    }
    Ok(if failures == 0 { 0 } else { 1 })
}

struct TaskRunContext<'a> {
    config: &'a Config,
    root: &'a Path,
    tasks_root: &'a Path,
    command: ParallelCommand,
    profiles: &'a ProfileCatalog,
    cli_profile: Option<&'a String>,
    events: &'a EventContext,
    resource_locks: &'a BTreeMap<String, Arc<Mutex<()>>>,
}

async fn run_task(context: TaskRunContext<'_>, task: Task) -> TaskRecord {
    let profile_name = task.profile.as_ref().or(context.cli_profile);
    let profile = match context.profiles.resolve(
        profile_name.map(String::as_str),
        context.command.default_profile(),
        context.command.profile_mode(),
    ) {
        Ok(profile) => profile,
        Err(error) => {
            return failed_record(
                &task,
                context.tasks_root,
                "unresolved".to_string(),
                1,
                format!("kiro-sidecar: {error:#}\n"),
                0,
            );
        }
    };
    if context.command == ParallelCommand::Worktree {
        if let Some(resource) = &task.resource {
            if let Some(lock) = context.resource_locks.get(resource) {
                let _guard = lock.lock().await;
                return run_task_with_retries(
                    context.config,
                    context.root,
                    context.tasks_root,
                    task,
                    context.command,
                    profile,
                    context.events,
                )
                .await;
            }
        }
    }
    run_task_with_retries(
        context.config,
        context.root,
        context.tasks_root,
        task,
        context.command,
        profile,
        context.events,
    )
    .await
}

async fn run_task_with_retries(
    config: &Config,
    root: &Path,
    tasks_root: &Path,
    task: Task,
    command: ParallelCommand,
    profile: crate::profiles::Profile,
    events: &EventContext,
) -> TaskRecord {
    let Some(task_component) = safe_artifact_id(&task.id) else {
        return failed_record(
            &task,
            tasks_root,
            profile.name,
            1,
            "kiro-sidecar: task id is not artifact-safe\n".to_string(),
            0,
        );
    };
    let task_dir = tasks_root.join(task_component);
    if let Err(error) = std::fs::create_dir_all(&task_dir) {
        return failed_record(
            &task,
            tasks_root,
            profile.name,
            1,
            format!("kiro-sidecar: could not create task dir: {error}\n"),
            0,
        );
    }
    let _ = events
        .emit(Some(&task.id), "task_started", "task started")
        .await;
    let heartbeat_stop = Arc::new(AtomicBool::new(false));
    let heartbeat = heartbeat_task(events.clone(), task.id.clone(), heartbeat_stop.clone());

    let mut attempts = 0_u32;
    let mut record;
    loop {
        attempts += 1;
        if attempts > 1 {
            let _ = events
                .emit(
                    Some(&task.id),
                    "task_retry",
                    &format!("retry attempt {attempts}"),
                )
                .await;
        }
        record = worker_attempt(config, root, &task_dir, &task, command, &profile, attempts).await;
        if record.returncode == 0 || attempts > task.retry || !retryable(&record) {
            break;
        }
    }

    heartbeat_stop.store(true, Ordering::Relaxed);
    heartbeat.abort();
    let _ = std::fs::write(task_dir.join("output.txt"), &record.output);
    let metadata = serde_json::json!({
        "task": &task,
        "record": &record,
        "finished_at": utc_timestamp()
    });
    let _ = std::fs::write(
        task_dir.join("metadata.json"),
        serde_json::to_string_pretty(&metadata).unwrap_or_else(|_| "{}".to_string()) + "\n",
    );
    let _ = events
        .emit(
            Some(&record.id),
            "task_finished",
            &format!("status={} returncode={}", record.status, record.returncode),
        )
        .await;
    record
}

async fn worker_attempt(
    config: &Config,
    root: &Path,
    task_dir: &Path,
    task: &Task,
    command: ParallelCommand,
    profile: &crate::profiles::Profile,
    attempts: u32,
) -> TaskRecord {
    let mut task_config = config.clone();
    if let Some(timeout_seconds) = task.timeout_seconds {
        task_config.timeout_seconds = timeout_seconds;
    }
    let trust_tools = profile.trust_tools();
    let (returncode, output) = match command {
        ParallelCommand::Explore => {
            let result = run_kiro(
                &task_config,
                root,
                &format!("{STRUCTURED_OUTPUT} Request: {}", task.prompt),
                task_dir,
                Some(&trust_tools),
                None,
                false,
            )
            .await;
            (result.returncode, result.output)
        }
        ParallelCommand::Review => {
            let diff_file = task_dir.join("diff.patch");
            if let Err(error) = write_full_diff(root, &diff_file).await {
                return failed_record(
                    task,
                    task_dir.parent().unwrap_or(task_dir),
                    profile.name.clone(),
                    1,
                    format!("kiro-sidecar: could not write diff: {error:#}\n"),
                    attempts,
                );
            }
            let result = run_kiro(
                &task_config,
                root,
                &format!(
                    "Review the repository diff in @{}. {STRUCTURED_OUTPUT} {}",
                    diff_file.display(),
                    task.prompt
                ),
                task_dir,
                Some(&trust_tools),
                None,
                false,
            )
            .await;
            (result.returncode, result.output)
        }
        ParallelCommand::Worktree => {
            let outcome = worktree_edit(
                &task_config,
                root,
                EditRequest {
                    prompt: &task.prompt,
                    allow: &task.allow,
                    deny: &task.deny,
                    run_dir: task_dir,
                    trust_tools: &trust_tools,
                    emit_output: false,
                },
            )
            .await
            .unwrap_or_else(|error| {
                let message = format!("kiro-sidecar: worktree task failed: {error:#}\n");
                eprintln!("{}", message.trim_end());
                EditOutcome {
                    status: 1,
                    output: message,
                }
            });
            let patch_file = task_dir.join("worktree.patch");
            let patch = std::fs::read_to_string(&patch_file).unwrap_or_default();
            if outcome.status == 0 {
                match validate_patch_limits(root, &patch_file, &patch, task).await {
                    Ok(Some(message)) => (1, output_message(&patch, &message)),
                    Ok(None) => (outcome.status, patch),
                    Err(error) => (
                        1,
                        output_message(
                            &patch,
                            &format!("could not inspect patch changed files: {error:#}"),
                        ),
                    ),
                }
            } else {
                (
                    outcome.status,
                    output_with_diagnostic(&patch, &outcome.output),
                )
            }
        }
    };
    let status = if returncode == 0 { "ok" } else { "failed" }.to_string();
    TaskRecord {
        id: task.id.clone(),
        status,
        returncode,
        output,
        task_dir: task_dir.display().to_string(),
        profile: profile.name.clone(),
        attempts,
        skipped_reason: None,
    }
}

async fn validate_patch_limits(
    root: &Path,
    patch_file: &Path,
    patch: &str,
    task: &Task,
) -> Result<Option<String>> {
    if let Some(max_lines) = task.max_diff_lines {
        let line_count = patch.lines().count();
        if line_count > max_lines {
            return Ok(Some(format!(
                "patch has {line_count} lines, exceeding max_diff_lines={max_lines}"
            )));
        }
    }
    if !task.expected_files.is_empty() {
        let expected = task
            .expected_files
            .iter()
            .map(|path| path.trim().replace('\\', "/"))
            .collect::<BTreeSet<_>>();
        let changed = changed_files_from_patch(root, patch_file)
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let unexpected = changed
            .iter()
            .filter(|path| {
                !path_candidates(path)
                    .iter()
                    .any(|candidate| expected.contains(candidate))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !unexpected.is_empty() {
            return Ok(Some(format!(
                "patch changed unexpected file(s): {}",
                unexpected.join(", ")
            )));
        }
    }
    Ok(None)
}

fn output_message(patch: &str, message: &str) -> String {
    let mut output = patch.to_string();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(PATCH_LIMIT_VIOLATION);
    output.push(' ');
    output.push_str(message);
    output.push('\n');
    output
}

fn output_with_diagnostic(patch: &str, diagnostic: &str) -> String {
    let mut output = patch.to_string();
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    let diagnostic = diagnostic.trim_end();
    if !diagnostic.is_empty() {
        output.push_str(diagnostic);
        output.push('\n');
    }
    output
}

fn retryable(record: &TaskRecord) -> bool {
    record.returncode != 0
        && !record.output.contains(PATCH_LIMIT_VIOLATION)
        && !record.output.contains("write guard blocked")
        && !record.output.contains("outside --allow")
        && !record.output.contains("outside repo boundary")
        && !record.output.contains("path is not in allowed edit scope")
        && !record.output.contains("resolves outside repo boundary")
}

fn failed_record(
    task: &Task,
    tasks_root: &Path,
    profile: String,
    returncode: i32,
    output: String,
    attempts: u32,
) -> TaskRecord {
    TaskRecord {
        id: task.id.clone(),
        status: "failed".to_string(),
        returncode,
        output,
        task_dir: task_record_dir(tasks_root, &task.id),
        profile,
        attempts,
        skipped_reason: None,
    }
}

fn skipped_record(task: &Task, tasks_root: &Path, reason: &str) -> TaskRecord {
    TaskRecord {
        id: task.id.clone(),
        status: "skipped".to_string(),
        returncode: 1,
        output: format!("kiro-sidecar: {reason}\n"),
        task_dir: task_record_dir(tasks_root, &task.id),
        profile: task
            .profile
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        attempts: 0,
        skipped_reason: Some(reason.to_string()),
    }
}

fn task_record_dir(tasks_root: &Path, task_id: &str) -> String {
    tasks_root
        .join(safe_artifact_id(task_id).unwrap_or_else(|| "invalid-task-id".to_string()))
        .display()
        .to_string()
}

async fn write_record(path: &Path, lock: &Mutex<()>, record: &TaskRecord) -> Result<()> {
    let _guard = lock.lock().await;
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?
        .write_all(line.as_bytes())
        .await?;
    Ok(())
}

fn write_run_summary(path: &Path, summary: &RunSummary) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(summary)? + "\n")
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

fn initial_verdict_json(run_id: &str) -> Result<String> {
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "run_id": run_id,
        "decisions": {}
    }))? + "\n")
}

fn resource_locks(tasks: &[Task]) -> BTreeMap<String, Arc<Mutex<()>>> {
    tasks
        .iter()
        .filter_map(|task| task.resource.as_ref())
        .map(|resource| (resource.clone(), Arc::new(Mutex::new(()))))
        .collect()
}

fn heartbeat_task(
    events: EventContext,
    task_id: String,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let _ = events
                .emit(Some(&task_id), "heartbeat", "task still running")
                .await;
        }
    })
}

#[derive(Clone)]
struct EventContext {
    run_id: String,
    events_file: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl EventContext {
    async fn emit(&self, task_id: Option<&str>, kind: &str, message: &str) -> Result<()> {
        append_event(
            &self.events_file,
            &self.lock,
            &EventRecord {
                timestamp: utc_timestamp(),
                run_id: self.run_id.clone(),
                task_id: task_id.map(ToOwned::to_owned),
                kind: kind.to_string(),
                message: message.to_string(),
            },
        )
        .await
    }
}

impl From<&TaskRecord> for TaskSummary {
    fn from(record: &TaskRecord) -> Self {
        Self {
            id: record.id.clone(),
            status: record.status.clone(),
            returncode: record.returncode,
            task_dir: record.task_dir.clone(),
            profile: record.profile.clone(),
            attempts: record.attempts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(output: &str) -> TaskRecord {
        TaskRecord {
            id: "task".to_string(),
            status: "failed".to_string(),
            returncode: 1,
            output: output.to_string(),
            task_dir: "task-dir".to_string(),
            profile: "worktree-edit".to_string(),
            attempts: 1,
            skipped_reason: None,
        }
    }

    #[test]
    fn retryable_rejects_patch_limit_violations() {
        assert!(!retryable(&record(
            "kiro-sidecar: patch limit violation: patch has 5 lines, exceeding max_diff_lines=1\n"
        )));
    }

    #[test]
    fn retryable_allows_unclassified_failures() {
        assert!(retryable(&record("temporary Kiro failure\n")));
    }
}
