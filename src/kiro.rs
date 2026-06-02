use crate::config::Config;
use regex::Regex;
use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug)]
pub struct KiroResult {
    pub returncode: i32,
    pub output: String,
}

pub async fn list_sessions(config: &Config, cwd: &Path) -> BTreeSet<String> {
    let result = run_metadata_command(
        config,
        cwd,
        &["chat".to_string(), "--list-sessions".to_string()],
    )
    .await;
    let Ok((0, stdout, _stderr)) = result else {
        return BTreeSet::new();
    };
    session_ids(&stdout).into_iter().collect()
}

pub async fn delete_sessions(config: &Config, cwd: &Path, session_ids: &[String]) -> Vec<String> {
    let mut failed = Vec::new();
    for session_id in session_ids {
        let result = run_metadata_command(
            config,
            cwd,
            &[
                "chat".to_string(),
                "--delete-session".to_string(),
                session_id.to_string(),
            ],
        )
        .await;
        if !matches!(result, Ok((0, _, _))) {
            failed.push(session_id.clone());
        }
    }
    failed
}

pub async fn run_kiro(
    config: &Config,
    cwd: &Path,
    prompt: &str,
    run_dir: &Path,
    trust_tools: Option<&str>,
    agent: Option<&str>,
    cleanup_sessions: bool,
) -> KiroResult {
    if let Err(error) = std::fs::create_dir_all(run_dir) {
        return KiroResult {
            returncode: 1,
            output: format!("kiro-sidecar: could not create run dir: {error}\n"),
        };
    }
    let tmp_dir = run_dir.join("tmp");
    if let Err(error) = std::fs::create_dir_all(&tmp_dir) {
        return KiroResult {
            returncode: 1,
            output: format!("kiro-sidecar: could not create tmp dir: {error}\n"),
        };
    }
    let before = if cleanup_sessions {
        list_sessions(config, cwd).await
    } else {
        BTreeSet::new()
    };

    let mut args = vec![
        "chat".to_string(),
        "--no-interactive".to_string(),
        "--model".to_string(),
        config.model.clone(),
        "--wrap".to_string(),
        "never".to_string(),
    ];
    if let Some(agent) = agent {
        args.push("--agent".to_string());
        args.push(agent.to_string());
    }
    args.push(format!(
        "--trust-tools={}",
        trust_tools.unwrap_or(&config.read_tools)
    ));
    args.push(prompt.to_string());

    let (returncode, mut output) = match run_chat_command(config, cwd, &args, &tmp_dir).await {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            127,
            format!("kiro-sidecar: Kiro CLI not found: {}\n", config.kiro_cli),
        ),
        Err(error) => (
            1,
            format!("kiro-sidecar: could not run Kiro CLI: {error}\n"),
        ),
    };

    let after = if cleanup_sessions {
        list_sessions(config, cwd).await
    } else {
        BTreeSet::new()
    };
    let new_sessions: Vec<String> = after.difference(&before).cloned().collect();
    if cleanup_sessions && !new_sessions.is_empty() {
        let failed = delete_sessions(config, cwd, &new_sessions).await;
        for session_id in failed {
            output.push_str(&format!(
                "\nkiro-sidecar: could not delete active Kiro session {session_id}\n"
            ));
        }
    }
    cleanup_log(Some(&tmp_dir));
    KiroResult { returncode, output }
}

pub fn which_kiro(config: &Config) -> Option<PathBuf> {
    let command = Path::new(&config.kiro_cli);
    if command.components().count() > 1 {
        return command.exists().then(|| command.to_path_buf());
    }
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|entry| entry.join(&config.kiro_cli))
        .find(|candidate| candidate.is_file())
}

pub async fn kiro_version(config: &Config) -> String {
    match run_metadata_command(config, Path::new("."), &["--version".to_string()]).await {
        Ok((0, stdout, _)) => stdout.trim().to_string(),
        Ok(_) => "unknown".to_string(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "not found".to_string(),
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => "timeout".to_string(),
        Err(_) => "unknown".to_string(),
    }
}

pub async fn settings_json(config: &Config) -> Option<String> {
    let args = [
        "settings".to_string(),
        "list".to_string(),
        "--format".to_string(),
        "json-pretty".to_string(),
    ];
    match run_metadata_command(config, Path::new("."), &args).await {
        Ok((0, stdout, _)) => Some(stdout),
        _ => None,
    }
}

pub fn cleanup_log(tmp_dir: Option<&Path>) {
    let mut candidates = Vec::new();
    if let Some(tmp_dir) = tmp_dir {
        candidates.push(tmp_dir.join("kiro-log").join("kiro-chat.log"));
    }
    let env_tmp = env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    candidates.push(env_tmp.join("kiro-log").join("kiro-chat.log"));
    for candidate in candidates {
        let _ = std::fs::remove_file(candidate);
    }
}

async fn run_chat_command(
    config: &Config,
    cwd: &Path,
    args: &[String],
    tmp_dir: &Path,
) -> std::io::Result<(i32, String)> {
    let mut command = Command::new(&config.kiro_cli);
    command
        .args(args)
        .current_dir(cwd)
        .env("TMPDIR", tmp_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdout = tokio::spawn(read_pipe(child.stdout.take()));
    let stderr = tokio::spawn(read_pipe(child.stderr.take()));
    match timeout(Duration::from_secs(config.timeout_seconds), child.wait()).await {
        Ok(result) => {
            let status = result?;
            let stdout = stdout.await.unwrap_or_default();
            let stderr = stderr.await.unwrap_or_default();
            let mut text = String::from_utf8_lossy(&stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&stderr));
            Ok((status.code().unwrap_or(1), text))
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let stdout = stdout.await.unwrap_or_default();
            let stderr = stderr.await.unwrap_or_default();
            let mut text = String::from_utf8_lossy(&stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&stderr));
            text.push_str(&format!(
                "\nkiro-sidecar: Kiro CLI timed out after {}s\n",
                config.timeout_seconds
            ));
            Ok((124, text))
        }
    }
}

async fn read_pipe<R>(pipe: Option<R>) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let Some(mut pipe) = pipe else {
        return Vec::new();
    };
    let mut buffer = Vec::new();
    if pipe.read_to_end(&mut buffer).await.is_ok() {
        buffer
    } else {
        Vec::new()
    }
}

async fn run_metadata_command(
    config: &Config,
    cwd: &Path,
    args: &[String],
) -> std::io::Result<(i32, String, String)> {
    let mut command = Command::new(&config.kiro_cli);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn()?;
    let duration = Duration::from_secs(config.timeout_seconds.min(30));
    match timeout(duration, child.wait_with_output()).await {
        Ok(result) => {
            let output = result?;
            Ok((
                output.status.code().unwrap_or(1),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ))
        }
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "metadata command timed out",
        )),
    }
}

fn session_ids(output: &str) -> Vec<String> {
    let Ok(regex) = Regex::new(r"Chat SessionId:\s*([^\s]+)") else {
        return Vec::new();
    };
    regex
        .captures_iter(output)
        .filter_map(|captures| captures.get(1).map(|value| value.as_str().to_string()))
        .collect()
}
