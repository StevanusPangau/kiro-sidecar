use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug)]
pub struct CommandResult {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
}

pub async fn run_git(args: &[&str], cwd: &Path) -> Result<CommandResult> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await?;
    Ok(CommandResult {
        returncode: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub async fn run_git_owned(args: Vec<String>, cwd: &Path) -> Result<CommandResult> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await?;
    Ok(CommandResult {
        returncode: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub async fn repo_root(cwd: &Path) -> PathBuf {
    match run_git(&["rev-parse", "--show-toplevel"], cwd).await {
        Ok(result) if result.returncode == 0 => {
            PathBuf::from(result.stdout.trim()).resolve_like_cwd(cwd)
        }
        _ => cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()),
    }
}

pub async fn has_head(cwd: &Path) -> bool {
    run_git(&["rev-parse", "--verify", "HEAD"], cwd)
        .await
        .map(|result| result.returncode == 0)
        .unwrap_or(false)
}

pub async fn status_short(cwd: &Path) -> Result<String> {
    Ok(run_git(&["status", "--short"], cwd).await?.stdout)
}

pub async fn changed_files(cwd: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if has_head(cwd).await {
        let result = run_git(&["diff", "--name-only", "HEAD", "--no-ext-diff"], cwd).await?;
        extend_lines(&mut names, &result.stdout);
    } else {
        for args in [
            ["diff", "--name-only", "--cached", "--no-ext-diff"].as_slice(),
            ["diff", "--name-only", "--no-ext-diff"].as_slice(),
        ] {
            let result = run_git(args, cwd).await?;
            extend_lines(&mut names, &result.stdout);
        }
    }
    let untracked = run_git(&["ls-files", "--others", "--exclude-standard"], cwd).await?;
    extend_lines(&mut names, &untracked.stdout);
    names.sort();
    names.dedup();
    Ok(names)
}

pub async fn write_full_diff(cwd: &Path, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let result = if has_head(cwd).await {
        run_git(
            &[
                "diff",
                "HEAD",
                "--no-ext-diff",
                "--find-renames",
                "--find-copies",
            ],
            cwd,
        )
        .await?
    } else {
        run_git(&["diff", "--cached", "--no-ext-diff"], cwd).await?
    };
    std::fs::write(output, result.stdout)?;

    let untracked = run_git(&["ls-files", "--others", "--exclude-standard", "-z"], cwd).await?;
    for raw_path in untracked.stdout.split('\0').filter(|path| !path.is_empty()) {
        let no_index = run_git_owned(
            vec![
                "diff".to_string(),
                "--no-index".to_string(),
                "--".to_string(),
                "/dev/null".to_string(),
                raw_path.to_string(),
            ],
            cwd,
        )
        .await?;
        append_text(output, &no_index.stdout)?;
    }
    Ok(())
}

pub async fn binary_diff(cwd: &Path, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let result = if has_head(cwd).await {
        run_git(&["diff", "--binary", "HEAD", "--no-ext-diff"], cwd).await?
    } else {
        run_git(&["diff", "--binary", "--cached", "--no-ext-diff"], cwd).await?
    };
    std::fs::write(output, result.stdout)?;
    Ok(())
}

pub async fn diff_check(cwd: &Path) -> Result<CommandResult> {
    run_git(&["diff", "--check"], cwd).await
}

pub async fn diff_stat(cwd: &Path) -> Result<String> {
    Ok(run_git(&["diff", "--stat"], cwd).await?.stdout)
}

pub async fn changed_files_from_patch(root: &Path, patch_file: &Path) -> Result<Vec<String>> {
    let result = run_git_owned(
        vec![
            "apply".to_string(),
            "--numstat".to_string(),
            "-z".to_string(),
            "--".to_string(),
            patch_file.to_string_lossy().into_owned(),
        ],
        root,
    )
    .await?;
    if result.returncode != 0 {
        let message = format!("{}{}", result.stdout, result.stderr);
        anyhow::bail!(
            "git apply --numstat -z failed for {}: {}",
            patch_file.display(),
            message.trim()
        );
    }
    let mut files = parse_numstat_z(&result.stdout)?;
    let patch = std::fs::read(patch_file)?;
    let patch = String::from_utf8_lossy(&patch);
    files.extend(parse_rename_paths_from_patch(&patch)?);
    files.sort();
    files.dedup();
    Ok(files)
}

pub fn require_success(result: CommandResult, context: &str) -> Result<()> {
    if result.returncode == 0 {
        Ok(())
    } else {
        Err(anyhow!("{context}: {}", result.stderr.trim()))
    }
}

fn extend_lines(names: &mut Vec<String>, text: &str) {
    names.extend(
        text.lines()
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned),
    );
}

fn parse_numstat_z(output: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(record) = fields.next() {
        let mut parts = record.splitn(3, '\t');
        let Some(_insertions) = parts.next() else {
            continue;
        };
        let Some(_deletions) = parts.next() else {
            anyhow::bail!("could not parse git numstat record `{record}`");
        };
        let Some(path) = parts.next() else {
            anyhow::bail!("could not parse git numstat record `{record}`");
        };
        if path.is_empty() {
            let Some(source) = fields.next() else {
                anyhow::bail!("could not parse git numstat rename source");
            };
            let Some(destination) = fields.next() else {
                anyhow::bail!("could not parse git numstat rename destination");
            };
            files.push(source.to_string());
            files.push(destination.to_string());
        } else {
            files.push(path.to_string());
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn parse_rename_paths_from_patch(patch: &str) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("rename from ") {
            files.push(parse_git_quoted_path(path)?);
        } else if let Some(path) = line.strip_prefix("rename to ") {
            files.push(parse_git_quoted_path(path)?);
        }
    }
    Ok(files)
}

fn parse_git_quoted_path(path: &str) -> Result<String> {
    if !path.starts_with('"') {
        return Ok(path.to_string());
    }
    if !path.ends_with('"') || path.len() < 2 {
        anyhow::bail!("could not parse quoted git path `{path}`");
    }

    let mut parsed = Vec::new();
    let mut chars = path[1..path.len() - 1].chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            push_char_bytes(&mut parsed, ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            anyhow::bail!("could not parse quoted git path `{path}`");
        };
        match escaped {
            '"' => parsed.push(b'"'),
            '\\' => parsed.push(b'\\'),
            'n' => parsed.push(b'\n'),
            'r' => parsed.push(b'\r'),
            't' => parsed.push(b'\t'),
            'b' => parsed.push(0x08),
            'f' => parsed.push(0x0c),
            '0'..='7' => {
                let mut value = escaped.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().and_then(|next| next.to_digit(8)) {
                        Some(digit) => {
                            value = value * 8 + digit;
                            chars.next();
                        }
                        None => break,
                    }
                }
                let byte = u8::try_from(value)
                    .with_context(|| format!("could not parse quoted git path `{path}`"))?;
                parsed.push(byte);
            }
            other => push_char_bytes(&mut parsed, other),
        }
    }
    String::from_utf8(parsed).with_context(|| format!("could not parse quoted git path `{path}`"))
}

fn push_char_bytes(output: &mut Vec<u8>, ch: char) {
    let mut buffer = [0; 4];
    output.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
}

fn append_text(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

trait ResolveLikeCwd {
    fn resolve_like_cwd(self, cwd: &Path) -> PathBuf;
}

impl ResolveLikeCwd for PathBuf {
    fn resolve_like_cwd(self, cwd: &Path) -> PathBuf {
        if self.is_absolute() {
            self
        } else {
            cwd.join(self)
        }
        .canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn changed_files_from_patch_handles_paths_with_spaces() -> Result<()> {
        let tmp = TempDir::new()?;
        require_success(run_git(&["init", "-q"], tmp.path()).await?, "git init")?;
        require_success(
            run_git(&["config", "user.email", "test@example.com"], tmp.path()).await?,
            "git config user.email",
        )?;
        require_success(
            run_git(&["config", "user.name", "Test"], tmp.path()).await?,
            "git config user.name",
        )?;
        std::fs::write(tmp.path().join("foo bar.txt"), "original\n")?;
        require_success(
            run_git(&["add", "foo bar.txt"], tmp.path()).await?,
            "git add",
        )?;
        require_success(
            run_git(&["commit", "-qm", "init"], tmp.path()).await?,
            "git commit",
        )?;
        std::fs::write(tmp.path().join("foo bar.txt"), "changed\n")?;
        let diff = run_git(&["diff", "--no-ext-diff"], tmp.path()).await?;
        require_success(
            CommandResult {
                returncode: diff.returncode,
                stdout: String::new(),
                stderr: diff.stderr,
            },
            "git diff",
        )?;
        let patch_file = tmp.path().join("space.patch");
        std::fs::write(&patch_file, diff.stdout)?;

        let files = changed_files_from_patch(tmp.path(), &patch_file).await?;

        assert_eq!(files, vec!["foo bar.txt"]);
        Ok(())
    }

    #[test]
    fn parse_numstat_z_includes_rename_sources_and_destinations() -> Result<()> {
        let files = parse_numstat_z("0\t0\t\0old.rs\0new.rs\0")?;

        assert_eq!(files, vec!["new.rs", "old.rs"]);
        Ok(())
    }

    #[test]
    fn parse_rename_paths_from_patch_decodes_quoted_paths() -> Result<()> {
        let patch = "diff --git \"a/old\\tname.txt\" \"b/new\\tname.txt\"\n\
                     similarity index 100%\n\
                     rename from \"old\\tname.txt\"\n\
                     rename to \"new\\tname.txt\"\n";

        let files = parse_rename_paths_from_patch(patch)?;

        assert_eq!(files, vec!["old\tname.txt", "new\tname.txt"]);
        Ok(())
    }

    #[test]
    fn parse_rename_paths_from_patch_decodes_octal_utf8_paths() -> Result<()> {
        let patch = "diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251-renamed.txt\"\n\
                     similarity index 100%\n\
                     rename from \"caf\\303\\251.txt\"\n\
                     rename to \"caf\\303\\251-renamed.txt\"\n";

        let files = parse_rename_paths_from_patch(patch)?;

        assert_eq!(files, vec!["caf\u{00e9}.txt", "caf\u{00e9}-renamed.txt"]);
        Ok(())
    }

    #[tokio::test]
    async fn changed_files_from_patch_handles_octal_utf8_rename_paths() -> Result<()> {
        let tmp = TempDir::new()?;
        require_success(run_git(&["init", "-q"], tmp.path()).await?, "git init")?;
        require_success(
            run_git(&["config", "user.email", "test@example.com"], tmp.path()).await?,
            "git config user.email",
        )?;
        require_success(
            run_git(&["config", "user.name", "Test"], tmp.path()).await?,
            "git config user.name",
        )?;
        let source = "caf\u{00e9}.txt";
        let destination = "caf\u{00e9}-renamed.txt";
        std::fs::write(tmp.path().join(source), "original\n")?;
        require_success(
            run_git_owned(vec!["add".to_string(), source.to_string()], tmp.path()).await?,
            "git add",
        )?;
        require_success(
            run_git(&["commit", "-qm", "init"], tmp.path()).await?,
            "git commit",
        )?;
        require_success(
            run_git_owned(
                vec![
                    "mv".to_string(),
                    source.to_string(),
                    destination.to_string(),
                ],
                tmp.path(),
            )
            .await?,
            "git mv",
        )?;
        let diff = run_git(
            &["diff", "--cached", "--find-renames", "--no-ext-diff"],
            tmp.path(),
        )
        .await?;
        require_success(
            CommandResult {
                returncode: diff.returncode,
                stdout: String::new(),
                stderr: diff.stderr,
            },
            "git diff",
        )?;
        let patch_file = tmp.path().join("rename.patch");
        std::fs::write(&patch_file, diff.stdout)?;

        let files = changed_files_from_patch(tmp.path(), &patch_file).await?;

        assert_eq!(files, vec![destination, source]);
        Ok(())
    }
}
