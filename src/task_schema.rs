use crate::config::validate_effort;
use crate::paths::{normalize_repo_glob, safe_artifact_id};
use crate::profiles::ProfileCatalog;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub profile: Option<String>,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub timeout_seconds: Option<u64>,
    pub depends_on: Vec<String>,
    pub expected_files: Vec<String>,
    pub tags: Vec<String>,
    pub resource: Option<String>,
    pub retry: u32,
    pub max_diff_lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RawTask {
    id: Option<String>,
    prompt: String,
    model: Option<String>,
    effort: Option<String>,
    profile: Option<String>,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    timeout_seconds: Option<u64>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    expected_files: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    resource: Option<String>,
    #[serde(default)]
    retry: u32,
    max_diff_lines: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskValidationMode {
    Generic,
    Write,
}

pub fn load_tasks(
    path: &Path,
    profiles: &ProfileCatalog,
    mode: TaskValidationMode,
) -> Result<Vec<Task>> {
    let raw: Vec<RawTask> = serde_json::from_str(
        &std::fs::read_to_string(path)
            .with_context(|| format!("could not read task file {}", path.display()))?,
    )
    .with_context(|| format!("could not parse task file {}", path.display()))?;
    let mut tasks = Vec::with_capacity(raw.len());
    let mut ids = BTreeSet::new();
    let mut artifact_ids = BTreeSet::new();
    for (index, item) in raw.into_iter().enumerate() {
        let id = item.id.unwrap_or_else(|| format!("task-{}", index + 1));
        if id.trim().is_empty() {
            bail!("task {} requires a non-empty id", index + 1);
        }
        let artifact_id = safe_artifact_id(&id).ok_or_else(|| {
            anyhow!("task `{id}` id must sanitize to a normal artifact directory")
        })?;
        if !ids.insert(id.clone()) {
            bail!("duplicate task id `{id}`");
        }
        if !artifact_ids.insert(artifact_id.clone()) {
            bail!("task `{id}` id conflicts after sanitizing to `{artifact_id}`");
        }
        let prompt = item.prompt.trim().to_string();
        if prompt.is_empty() {
            bail!("task `{id}` requires prompt");
        }
        let model = optional_non_empty_string(&id, "model", item.model)?;
        let effort = optional_effort(&id, item.effort)?;
        if let Some(profile) = &item.profile {
            if !profiles.contains(profile) {
                bail!("task `{id}` references unknown profile `{profile}`");
            }
        }
        validate_globs(&id, "allow", &item.allow)?;
        validate_globs(&id, "deny", &item.deny)?;
        if mode == TaskValidationMode::Write && item.allow.is_empty() {
            bail!("write task `{id}` requires at least one allow path");
        }
        if matches!(item.timeout_seconds, Some(0)) {
            bail!("task `{id}` timeout_seconds must be greater than zero");
        }
        if matches!(item.max_diff_lines, Some(0)) {
            bail!("task `{id}` max_diff_lines must be greater than zero");
        }
        tasks.push(Task {
            id,
            prompt,
            model,
            effort,
            profile: item.profile,
            allow: item.allow,
            deny: item.deny,
            timeout_seconds: item.timeout_seconds,
            depends_on: item.depends_on,
            expected_files: item.expected_files,
            tags: item.tags,
            resource: item.resource,
            retry: item.retry,
            max_diff_lines: item.max_diff_lines,
        });
    }
    validate_dependencies(&tasks)?;
    Ok(tasks)
}

pub fn dependency_batches(tasks: &[Task]) -> Result<Vec<Vec<usize>>> {
    let index_by_id = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| (task.id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut indegree = vec![0_usize; tasks.len()];
    let mut dependents = vec![Vec::new(); tasks.len()];
    for (index, task) in tasks.iter().enumerate() {
        indegree[index] = task.depends_on.len();
        for dependency in &task.depends_on {
            let Some(dependency_index) = index_by_id.get(dependency) else {
                bail!("task `{}` depends on unknown task `{dependency}`", task.id);
            };
            dependents[*dependency_index].push(index);
        }
    }

    let mut ready = indegree
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect::<VecDeque<_>>();
    let mut seen = 0;
    let mut batches = Vec::new();
    while !ready.is_empty() {
        let mut batch = Vec::new();
        for _ in 0..ready.len() {
            let Some(index) = ready.pop_front() else {
                continue;
            };
            batch.push(index);
            seen += 1;
            for dependent in &dependents[index] {
                indegree[*dependent] -= 1;
                if indegree[*dependent] == 0 {
                    ready.push_back(*dependent);
                }
            }
        }
        batches.push(batch);
    }
    if seen != tasks.len() {
        bail!("task dependency graph contains a cycle");
    }
    Ok(batches)
}

fn validate_globs(id: &str, field: &str, values: &[String]) -> Result<()> {
    for value in values {
        normalize_repo_glob(value)
            .map_err(|error| anyhow!("task `{id}` has invalid {field} path `{value}`: {error}"))?;
    }
    Ok(())
}

fn optional_non_empty_string(
    id: &str,
    field: &str,
    value: Option<String>,
) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_string();
    if value.is_empty() {
        bail!("task `{id}` {field} must be non-empty when set");
    }
    Ok(Some(value))
}

fn optional_effort(id: &str, value: Option<String>) -> Result<Option<String>> {
    let Some(value) = optional_non_empty_string(id, "effort", value)? else {
        return Ok(None);
    };
    validate_effort(&format!("task `{id}` effort"), &value)?;
    Ok(Some(value))
}

fn validate_dependencies(tasks: &[Task]) -> Result<()> {
    let ids = tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    for task in tasks {
        for dependency in &task.depends_on {
            if !ids.contains(dependency.as_str()) {
                bail!("task `{}` depends on unknown task `{dependency}`", task.id);
            }
        }
    }
    dependency_batches(tasks)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn task(id: &str, depends_on: &[&str]) -> Task {
        Task {
            id: id.to_string(),
            prompt: "prompt".to_string(),
            model: None,
            effort: None,
            profile: None,
            allow: Vec::new(),
            deny: Vec::new(),
            timeout_seconds: None,
            depends_on: depends_on.iter().map(|value| value.to_string()).collect(),
            expected_files: Vec::new(),
            tags: Vec::new(),
            resource: None,
            retry: 0,
            max_diff_lines: None,
        }
    }

    fn catalog() -> ProfileCatalog {
        let tmp = TempDir::new().unwrap();
        ProfileCatalog::load(tmp.path()).unwrap()
    }

    fn write_task_file(dir: &Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("tasks.json");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn dependency_batches_order_tasks() -> Result<()> {
        let tasks = vec![task("a", &[]), task("b", &["a"]), task("c", &["a"])];
        let batches = dependency_batches(&tasks)?;
        assert_eq!(batches, vec![vec![0], vec![1, 2]]);
        Ok(())
    }

    #[test]
    fn dependency_batches_reject_cycles() {
        let tasks = vec![task("a", &["b"]), task("b", &["a"])];
        let error = dependency_batches(&tasks).unwrap_err();
        assert!(error.to_string().contains("cycle"));
    }

    #[test]
    fn dependency_batches_diamond() -> Result<()> {
        let tasks = vec![
            task("a", &[]),
            task("b", &["a"]),
            task("c", &["a"]),
            task("d", &["b", "c"]),
        ];
        let batches = dependency_batches(&tasks)?;
        assert_eq!(batches, vec![vec![0], vec![1, 2], vec![3]]);
        Ok(())
    }

    #[test]
    fn dependency_batches_all_independent() -> Result<()> {
        let tasks = vec![task("a", &[]), task("b", &[]), task("c", &[])];
        let batches = dependency_batches(&tasks)?;
        assert_eq!(batches, vec![vec![0, 1, 2]]);
        Ok(())
    }

    #[test]
    fn load_tasks_minimal_valid() -> Result<()> {
        let tmp = TempDir::new()?;
        let path = write_task_file(tmp.path(), r#"[{"id": "t1", "prompt": "do something"}]"#);
        let profiles = catalog();
        let tasks = load_tasks(&path, &profiles, TaskValidationMode::Generic)?;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "t1");
        assert_eq!(tasks[0].prompt, "do something");
        assert_eq!(tasks[0].model, None);
        assert_eq!(tasks[0].effort, None);
        Ok(())
    }

    #[test]
    fn load_tasks_auto_generates_id() -> Result<()> {
        let tmp = TempDir::new()?;
        let path = write_task_file(tmp.path(), r#"[{"prompt": "first"}, {"prompt": "second"}]"#);
        let profiles = catalog();
        let tasks = load_tasks(&path, &profiles, TaskValidationMode::Generic)?;
        assert_eq!(tasks[0].id, "task-1");
        assert_eq!(tasks[1].id, "task-2");
        Ok(())
    }

    #[test]
    fn load_tasks_rejects_duplicate_ids() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "dup", "prompt": "a"}, {"id": "dup", "prompt": "b"}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("duplicate task id"));
    }

    #[test]
    fn load_tasks_rejects_empty_prompt() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(tmp.path(), r#"[{"id": "t1", "prompt": "   "}]"#);
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("requires prompt"));
    }

    #[test]
    fn load_tasks_rejects_empty_model() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "model": "   "}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("model must be non-empty"));
    }

    #[test]
    fn load_tasks_rejects_unknown_effort() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "effort": "extreme"}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("effort must be one of"));
    }

    #[test]
    fn load_tasks_rejects_unknown_profile() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "profile": "nonexistent"}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("unknown profile"));
    }

    #[test]
    fn load_tasks_rejects_zero_timeout() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "timeout_seconds": 0}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error
            .to_string()
            .contains("timeout_seconds must be greater than zero"));
    }

    #[test]
    fn load_tasks_rejects_zero_max_diff_lines() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "max_diff_lines": 0}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error
            .to_string()
            .contains("max_diff_lines must be greater than zero"));
    }

    #[test]
    fn load_tasks_write_mode_requires_allow() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(tmp.path(), r#"[{"id": "t1", "prompt": "x"}]"#);
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Write).unwrap_err();
        assert!(error
            .to_string()
            .contains("requires at least one allow path"));
    }

    #[test]
    fn load_tasks_write_mode_accepts_with_allow() -> Result<()> {
        let tmp = TempDir::new()?;
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "allow": ["src/**"]}]"#,
        );
        let profiles = catalog();
        let tasks = load_tasks(&path, &profiles, TaskValidationMode::Write)?;
        assert_eq!(tasks[0].allow, vec!["src/**"]);
        Ok(())
    }

    #[test]
    fn load_tasks_rejects_invalid_glob_in_allow() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "allow": ["/etc/passwd"]}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("invalid allow path"));
    }

    #[test]
    fn load_tasks_rejects_parent_dir_in_deny() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "deny": ["../secret"]}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("invalid deny path"));
    }

    #[test]
    fn load_tasks_rejects_unknown_dependency() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "t1", "prompt": "x", "depends_on": ["missing"]}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("unknown task"));
    }

    #[test]
    fn load_tasks_rejects_cyclic_dependencies() {
        let tmp = TempDir::new().unwrap();
        let path = write_task_file(
            tmp.path(),
            r#"[{"id": "a", "prompt": "x", "depends_on": ["b"]}, {"id": "b", "prompt": "y", "depends_on": ["a"]}]"#,
        );
        let profiles = catalog();
        let error = load_tasks(&path, &profiles, TaskValidationMode::Generic).unwrap_err();
        assert!(error.to_string().contains("cycle"));
    }

    #[test]
    fn load_tasks_with_all_optional_fields() -> Result<()> {
        let tmp = TempDir::new()?;
        let path = write_task_file(
            tmp.path(),
            r#"[{
                "id": "full",
                "prompt": "do work",
                "model": "claude-sonnet-4.6",
                "effort": "medium",
                "profile": "read-only",
                "allow": ["src/**"],
                "deny": ["src/secret/**"],
                "timeout_seconds": 600,
                "depends_on": [],
                "expected_files": ["src/main.rs"],
                "tags": ["security"],
                "resource": "shared-db",
                "retry": 2,
                "max_diff_lines": 500
            }]"#,
        );
        let profiles = catalog();
        let tasks = load_tasks(&path, &profiles, TaskValidationMode::Generic)?;
        let t = &tasks[0];
        assert_eq!(t.id, "full");
        assert_eq!(t.model.as_deref(), Some("claude-sonnet-4.6"));
        assert_eq!(t.effort.as_deref(), Some("medium"));
        assert_eq!(t.profile.as_deref(), Some("read-only"));
        assert_eq!(t.timeout_seconds, Some(600));
        assert_eq!(t.tags, vec!["security"]);
        assert_eq!(t.resource.as_deref(), Some("shared-db"));
        assert_eq!(t.retry, 2);
        assert_eq!(t.max_diff_lines, Some(500));
        Ok(())
    }
}
