use crate::config::{Config, DEFAULT_DENIES, STRUCTURED_OUTPUT};
use crate::git_utils::{
    binary_diff, changed_files, diff_check, diff_stat, repo_root, require_success, run_git,
    run_git_owned, status_short,
};
use crate::kiro::run_kiro;
use crate::paths::{matches_any, normalize_repo_glob};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub struct EditRequest<'a> {
    pub prompt: &'a str,
    pub allow: &'a [String],
    pub deny: &'a [String],
    pub run_dir: &'a Path,
    pub trust_tools: &'a str,
    pub emit_output: bool,
}

#[derive(Debug)]
pub struct EditOutcome {
    pub status: i32,
    pub output: String,
}

pub fn normalize_allow_deny(
    allow: &[String],
    deny: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    let allow = allow
        .iter()
        .map(|item| normalize_repo_glob(item))
        .collect::<Result<Vec<_>>>()?;
    let deny = deny
        .iter()
        .map(|item| normalize_repo_glob(item))
        .collect::<Result<Vec<_>>>()?;
    Ok((allow, deny))
}

pub fn validate_introduced(
    before: &[String],
    after: &[String],
    allow: &[String],
) -> (bool, Vec<String>, Vec<String>) {
    let before: BTreeSet<_> = before.iter().cloned().collect();
    let after: BTreeSet<_> = after.iter().cloned().collect();
    let introduced: Vec<String> = after.difference(&before).cloned().collect();
    let outside = introduced
        .iter()
        .filter(|path| !matches_any(path, allow))
        .cloned()
        .collect::<Vec<_>>();
    (outside.is_empty(), introduced, outside)
}

pub fn dirty_outside_fingerprints(
    root: &Path,
    dirty_paths: &[String],
    allow: &[String],
) -> BTreeMap<String, String> {
    dirty_paths
        .iter()
        .filter(|path| !matches_any(path, allow))
        .map(|path| (path.clone(), path_fingerprint(&root.join(path))))
        .collect()
}

pub fn changed_fingerprints(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    before
        .iter()
        .filter_map(|(path, fingerprint)| {
            if after.get(path) != Some(fingerprint) {
                Some(path.clone())
            } else {
                None
            }
        })
        .collect()
}

pub fn path_fingerprint(path: &Path) -> String {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return "missing".to_string();
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return fs::read_link(path)
            .map(|target| format!("symlink:{}", target.display()))
            .unwrap_or_else(|_| "symlink:<unreadable>".to_string());
    }
    if file_type.is_file() {
        let Ok(mut file) = fs::File::open(path) else {
            return format!("file:{}:<unreadable>", metadata.len());
        };
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 1024 * 1024];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => digest.update(&buffer[..n]),
                Err(_) => return format!("file:{}:<read-error>", metadata.len()),
            }
        }
        return format!("file:{}:{:x}", metadata.len(), digest.finalize());
    }
    format!(
        "other:{:?}:{}:{:?}",
        file_type,
        metadata.len(),
        metadata.modified().ok()
    )
}

pub fn validate_allowed_realpaths(root: &Path, allow: &[String]) -> Vec<String> {
    let real_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut outside = Vec::new();
    for pattern in allow {
        let raw_pattern = pattern.strip_prefix("./").unwrap_or(pattern);
        let matches = existing_matches(root, raw_pattern);
        for path in matches {
            if !resolves_inside_repo(&real_root, &path) {
                outside.push(pattern.clone());
                break;
            }
        }
    }
    outside
}

fn existing_matches(root: &Path, raw_pattern: &str) -> Vec<PathBuf> {
    if raw_pattern.contains(['*', '?', '[']) {
        let pattern = root.join(raw_pattern).to_string_lossy().into_owned();
        glob::glob(&pattern)
            .map(|paths| {
                paths
                    .filter_map(std::result::Result::ok)
                    .filter(|path| path.exists() || is_symlink(path))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        let path = root.join(raw_pattern);
        if path.exists() || is_symlink(&path) {
            vec![path]
        } else {
            Vec::new()
        }
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn resolves_inside_repo(real_root: &Path, path: &Path) -> bool {
    path.canonicalize()
        .map(|real_path| is_inside(real_root, &real_path))
        .unwrap_or(false)
}

fn is_inside(root: &Path, path: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn join_output(output: Vec<String>) -> String {
    output
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn edit_outcome(status: i32, output: String, emit_output: bool) -> EditOutcome {
    if emit_output && !output.is_empty() {
        println!("{output}");
    }
    EditOutcome { status, output }
}

pub async fn bounded_edit(
    config: &Config,
    cwd: &Path,
    request: EditRequest<'_>,
) -> Result<EditOutcome> {
    let root = repo_root(cwd).await;
    let (allow, explicit_deny) = normalize_allow_deny(request.allow, request.deny)?;
    let outside_allowed = validate_allowed_realpaths(&root, &allow);
    if !outside_allowed.is_empty() {
        let mut output = vec![
            "kiro-sidecar: refusing allowed paths that resolve outside repo boundary:".to_string(),
        ];
        output.extend(outside_allowed.into_iter().map(|path| format!("  {path}")));
        return Ok(edit_outcome(1, join_output(output), request.emit_output));
    }

    let combined_deny = DEFAULT_DENIES
        .iter()
        .map(|value| value.to_string())
        .chain(explicit_deny)
        .collect::<Vec<_>>();
    let agent_name = format!(
        "codex_kiro_writer_{}_{}",
        std::process::id(),
        request
            .run_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("run")
            .replace('-', "_")
    );
    let agent_dir = root.join(&config.agent_dir);
    let created_agent_dir = !agent_dir.exists();
    let created_kiro_dir = !root.join(".kiro").exists();
    fs::create_dir_all(&agent_dir)?;
    fs::create_dir_all(request.run_dir)?;

    let agent_file = agent_dir.join(format!("{agent_name}.json"));
    let policy_file = request.run_dir.join("write_guard_policy.json");
    let hook_file = request.run_dir.join("write_guard.sh");
    let snapshot_dir = request.run_dir.join("snapshot");
    fs::create_dir_all(&snapshot_dir)?;

    let _cleanup = AgentCleanup {
        agent_file: agent_file.clone(),
        agent_dir: agent_dir.clone(),
        kiro_dir: root.join(".kiro"),
        remove_agent_dir: created_agent_dir,
        remove_kiro_dir: created_kiro_dir,
    };
    write_guard_policy(
        &policy_file,
        &allow,
        &combined_deny,
        &root,
        &hook_file.with_extension("blocked"),
    )?;
    write_guard_shim(&hook_file, &policy_file)?;
    write_agent(
        &agent_file,
        &agent_name,
        &allow,
        &combined_deny,
        &hook_file,
        &config.model,
    )?;

    let before_status = status_short(&root).await?;
    let before_files = changed_files(&root).await?;
    let before_dirty_outside = dirty_outside_fingerprints(&root, &before_files, &allow);
    binary_diff(&root, &snapshot_dir.join("before.diff")).await?;
    fs::write(
        snapshot_dir.join("before-files.txt"),
        with_trailing_newline(&before_files),
    )?;

    let mut output = vec![
        "kiro-sidecar: starting bounded edit with allowed paths:".to_string(),
        allow
            .iter()
            .map(|item| format!("  {item}"))
            .collect::<Vec<_>>()
            .join("\n"),
        format!(
            "kiro-sidecar: pre-run snapshot saved in {}",
            snapshot_dir.display()
        ),
    ];

    let kiro_prompt = format!(
        "Apply this bounded edit request. Allowed paths: {}. Stay strictly inside those paths. \
If the request requires any other path, stop without editing and explain the missing scope. \
{STRUCTURED_OUTPUT} Request: {}",
        allow.join(" "),
        request.prompt
    );
    let result = run_kiro(
        config,
        &root,
        &kiro_prompt,
        request.run_dir,
        Some(request.trust_tools),
        Some(&agent_name),
        true,
    )
    .await;
    output.push(result.output);

    let mut hook_block_status = 0;
    let block_log = hook_file.with_extension("blocked");
    if block_log.exists() {
        output
            .push("\nkiro-sidecar: write guard blocked at least one attempted write:".to_string());
        let lines = fs::read_to_string(&block_log).unwrap_or_default();
        output.extend(lines.lines().map(|line| format!("  {line}")));
        hook_block_status = 1;
    }

    let after_status = status_short(&root).await?;
    let after_files = changed_files(&root).await?;
    fs::write(
        snapshot_dir.join("after-files.txt"),
        with_trailing_newline(&after_files),
    )?;
    let (ok, introduced, outside) = validate_introduced(&before_files, &after_files, &allow);
    let dirty_outside_changed = changed_fingerprints(
        &before_dirty_outside,
        &dirty_outside_fingerprints(
            &root,
            &before_dirty_outside.keys().cloned().collect::<Vec<_>>(),
            &allow,
        ),
    );

    output.push("\nkiro-sidecar: status changes introduced during sidecar run:".to_string());
    output.push(diff_text(&before_status, &after_status));
    output.push("\nkiro-sidecar: changed-file validator:".to_string());
    if introduced.is_empty() {
        output.push("kiro-sidecar: no newly changed files detected during sidecar run".to_string());
    } else {
        output.push("kiro-sidecar: files newly changed during sidecar run:".to_string());
        output.extend(introduced.iter().map(|path| format!("  {path}")));
    }
    if !outside.is_empty() {
        output.push(
            "kiro-sidecar: blocked because sidecar changed files outside --allow:".to_string(),
        );
        output.extend(outside.iter().map(|path| format!("  {path}")));
    }
    if !dirty_outside_changed.is_empty() {
        output.push("kiro-sidecar: blocked because sidecar modified pre-existing dirty files outside --allow:".to_string());
        output.extend(dirty_outside_changed.iter().map(|path| format!("  {path}")));
    }
    output.push("\nkiro-sidecar: repository diff stat after sidecar run:".to_string());
    output.push(diff_stat(&root).await?);
    let check = diff_check(&root).await?;
    output.push("\nkiro-sidecar: whitespace/conflict marker check:".to_string());
    output.push(format!("{}{}", check.stdout, check.stderr));
    output.push(
        "\nkiro-sidecar: Codex must review git diff before finalizing this work.".to_string(),
    );

    let status = if result.returncode != 0 {
        result.returncode
    } else if hook_block_status != 0 {
        hook_block_status
    } else if !ok || !dirty_outside_changed.is_empty() {
        1
    } else {
        check.returncode
    };
    Ok(edit_outcome(
        status,
        join_output(output),
        request.emit_output,
    ))
}

pub async fn worktree_edit(
    config: &Config,
    cwd: &Path,
    request: EditRequest<'_>,
) -> Result<EditOutcome> {
    let root = repo_root(cwd).await;
    let verify_head = run_git(&["rev-parse", "--verify", "HEAD"], &root).await?;
    if verify_head.returncode != 0 {
        return Ok(edit_outcome(
            2,
            "kiro-sidecar: edit-worktree requires a repository with at least one commit"
                .to_string(),
            request.emit_output,
        ));
    }

    fs::create_dir_all(request.run_dir)?;
    let worktree = request.run_dir.join("worktree");
    let patch_file = request.run_dir.join("worktree.patch");
    require_success(
        run_git_owned(
            vec![
                "-C".to_string(),
                root.display().to_string(),
                "worktree".to_string(),
                "add".to_string(),
                "--detach".to_string(),
                worktree.display().to_string(),
                "HEAD".to_string(),
            ],
            &root,
        )
        .await?,
        "git worktree add failed",
    )?;

    let edit_run_dir = request.run_dir.join("edit");
    let outcome = bounded_edit(
        config,
        &worktree,
        EditRequest {
            run_dir: &edit_run_dir,
            ..request
        },
    )
    .await;
    let diff = run_git_owned(
        vec![
            "-C".to_string(),
            worktree.display().to_string(),
            "diff".to_string(),
            "--binary".to_string(),
            "HEAD".to_string(),
            "--no-ext-diff".to_string(),
        ],
        &worktree,
    )
    .await;
    let remove = run_git_owned(
        vec![
            "-C".to_string(),
            root.display().to_string(),
            "worktree".to_string(),
            "remove".to_string(),
            "--force".to_string(),
            worktree.display().to_string(),
        ],
        &root,
    )
    .await;

    let diff = diff?;
    fs::write(&patch_file, &diff.stdout)?;
    if request.emit_output {
        println!(
            "\nkiro-sidecar: main working tree was not modified. Codex must review and apply this patch manually if accepted."
        );
        println!("\nkiro-sidecar: worktree patch for Codex review:");
        println!("PATCH:\n```diff");
        print!("{}", diff.stdout);
        println!("```\n\nNO_EXTRA_TEXT_AFTER_PATCH");
    }
    if let Err(error) = remove {
        eprintln!("kiro-sidecar: could not remove worktree: {error}");
    }
    outcome
}

#[derive(Debug, Deserialize, Serialize)]
struct GuardPolicy {
    allow: Vec<String>,
    deny: Vec<String>,
    repo_root: PathBuf,
    block_log: PathBuf,
}

pub fn run_write_guard(policy_path: &Path, stdin: &str) -> Result<i32> {
    let policy: GuardPolicy = serde_json::from_str(&fs::read_to_string(policy_path)?)?;
    let payload: Value = match serde_json::from_str(stdin) {
        Ok(value) => value,
        Err(error) => {
            return block(
                &policy.block_log,
                &format!("could not parse hook payload: {error}"),
            )
        }
    };
    for path in collect_paths(&payload, None) {
        let normalized = normalize_hook_path(&policy.repo_root, &path);
        if normalized.is_empty() {
            continue;
        }
        let parts = normalized
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if Path::new(&normalized).is_absolute()
            || normalized.starts_with('~')
            || parts.contains(&"..")
        {
            return block(
                &policy.block_log,
                &format!("path outside repo boundary: {path}"),
            );
        }
        if !real_path_inside_repo(&policy.repo_root, &normalized) {
            return block(
                &policy.block_log,
                &format!("path resolves outside repo boundary: {path}"),
            );
        }
        if matches_any(&normalized, &policy.deny) {
            return block(
                &policy.block_log,
                &format!("path matches denied pattern: {path}"),
            );
        }
        if !policy.allow.is_empty() && !matches_any(&normalized, &policy.allow) {
            return block(
                &policy.block_log,
                &format!("path is not in allowed edit scope: {path}"),
            );
        }
    }
    Ok(0)
}

fn write_guard_policy(
    policy_file: &Path,
    allow: &[String],
    deny: &[String],
    root: &Path,
    block_log: &Path,
) -> Result<()> {
    let policy = GuardPolicy {
        allow: allow.to_vec(),
        deny: deny.to_vec(),
        repo_root: root.to_path_buf(),
        block_log: block_log.to_path_buf(),
    };
    fs::write(policy_file, serde_json::to_string_pretty(&policy)? + "\n")?;
    Ok(())
}

fn write_guard_shim(hook_file: &Path, policy_file: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("could not resolve current kiro-sidecar binary")?;
    let source = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nexec {} __write-guard --policy {}\n",
        shell_quote(&exe),
        shell_quote(policy_file)
    );
    fs::write(hook_file, source)?;
    make_executable(hook_file)?;
    Ok(())
}

fn shell_quote(path: &Path) -> String {
    let value = path.as_os_str().to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_agent(
    agent_file: &Path,
    agent_name: &str,
    allow: &[String],
    deny: &[String],
    hook_file: &Path,
    model: &str,
) -> Result<()> {
    let prompt = format!(
        "You are a bounded editor sidecar controlled by Codex.\n\n\
Follow the user's requested edit exactly and keep changes minimal.\n\
Only read or write files inside the explicitly allowed paths.\n\
Do not use shell commands, package managers, network tools, MCP tools, delegation, or broad project rewrites.\n\
Do not quote full files in the answer.\n\
If the request cannot be completed inside the allowed paths, stop and explain the missing scope instead of\n\
editing outside the boundary.\n\
{STRUCTURED_OUTPUT}\n"
    );
    let config = json!({
        "name": agent_name,
        "description": "Codex-owned bounded write sidecar. No shell. Temporary per invocation.",
        "prompt": prompt,
        "mcpServers": {},
        "tools": ["read", "write", "grep", "glob"],
        "toolAliases": {},
        "allowedTools": ["read", "write", "grep", "glob"],
        "resources": [],
        "hooks": {
            "preToolUse": [
                {
                    "matcher": "write",
                    "command": hook_file.display().to_string(),
                    "description": "Block writes outside Codex-approved sidecar edit scope"
                }
            ]
        },
        "toolsSettings": {
            "fs_write": {"allowedPaths": allow, "deniedPaths": deny, "fallbackAction": "deny"},
            "write": {"allowedPaths": allow, "deniedPaths": deny, "fallbackAction": "deny"}
        },
        "useLegacyMcpJson": true,
        "model": model
    });
    fs::write(agent_file, serde_json::to_string_pretty(&config)? + "\n")?;
    Ok(())
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o100);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn with_trailing_newline(lines: &[String]) -> String {
    if lines.is_empty() {
        "\n".to_string()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn diff_text(before: &str, after: &str) -> String {
    if before == after {
        "  no status changes".to_string()
    } else {
        format!("--- before\n{before}--- after\n{after}")
    }
}

fn block(block_log: &Path, message: &str) -> Result<i32> {
    if let Some(parent) = block_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(block_log)?;
    writeln!(file, "{message}")?;
    eprintln!("kiro-sidecar write blocked: {message}");
    Ok(2)
}

fn collect_paths(value: &Value, key: Option<&str>) -> Vec<String> {
    let mut found = Vec::new();
    match value {
        Value::String(text) => {
            let normalized_key = key.unwrap_or_default().to_ascii_lowercase();
            if is_path_key(&normalized_key) {
                found.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                found.extend(collect_paths(item, key));
            }
        }
        Value::Object(map) => {
            for (child_key, item) in map {
                found.extend(collect_paths(item, Some(child_key)));
            }
        }
        _ => {}
    }
    found
}

fn is_path_key(key: &str) -> bool {
    matches!(
        key,
        "path"
            | "paths"
            | "file"
            | "files"
            | "file_path"
            | "filepath"
            | "filename"
            | "target"
            | "target_path"
            | "absolute_path"
            | "relative_path"
    ) || key.ends_with("_path")
        || key.ends_with("path")
}

fn normalize_hook_path(root: &Path, value: &str) -> String {
    let mut value = value
        .trim()
        .trim_matches(['`', '\'', '"'])
        .trim_end_matches([')', '.', ',', ';', ':'])
        .replace('\\', "/");
    if let Some(stripped) = value.strip_prefix("file://") {
        value = stripped.to_string();
    }
    let path = Path::new(&value);
    if path.is_absolute() {
        let absolute = path.to_path_buf();
        if absolute == root {
            return String::new();
        }
        if let Ok(relative) = absolute.strip_prefix(root) {
            value = relative.to_string_lossy().replace('\\', "/");
        }
    }
    value.strip_prefix("./").unwrap_or(&value).to_string()
}

fn real_path_inside_repo(root: &Path, normalized: &str) -> bool {
    let real_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let candidate = root.join(normalized);
    let mut current = root.to_path_buf();
    for component in Path::new(normalized).components() {
        current.push(component.as_os_str());
        if current.exists() || is_symlink(&current) {
            let Ok(real_current) = current.canonicalize() else {
                return false;
            };
            if !is_inside(&real_root, &real_current) {
                return false;
            }
        } else {
            break;
        }
    }
    candidate
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .map(|real_parent| is_inside(&real_root, &real_parent))
        .unwrap_or(true)
}

struct AgentCleanup {
    agent_file: PathBuf,
    agent_dir: PathBuf,
    kiro_dir: PathBuf,
    remove_agent_dir: bool,
    remove_kiro_dir: bool,
}

impl Drop for AgentCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.agent_file);
        if self.remove_agent_dir {
            let _ = fs::remove_dir_all(&self.agent_dir);
        }
        if self.remove_kiro_dir {
            let _ = fs::remove_dir(&self.kiro_dir);
        }
    }
}
