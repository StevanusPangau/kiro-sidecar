use anyhow::{anyhow, Result};
use regex::Regex;
use std::path::{Component, Path};
use time::macros::format_description;
use time::OffsetDateTime;
use uuid::Uuid;

const MAX_ARTIFACT_ID_LEN: usize = 200;

pub fn new_run_id(prefix: &str) -> String {
    let format = format_description!("[year][month][day]T[hour][minute][second]Z");
    let stamp = OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| "19700101T000000Z".to_string());
    let uuid = Uuid::new_v4().simple().to_string();
    format!("{prefix}-{stamp}-{}", &uuid[..8])
}

pub fn safe_artifact_id(value: &str) -> Option<String> {
    let id = stable_safe_id(value)?;
    (id.len() <= MAX_ARTIFACT_ID_LEN && is_normal_component(&id)).then_some(id)
}

fn stable_safe_id(value: &str) -> Option<String> {
    let mut output = String::with_capacity(value.len());
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
            output.push(ch);
        } else {
            output.push('-');
        }
    }
    let trimmed = output.trim_matches('-').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn is_normal_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

pub fn normalize_repo_glob(pattern: &str) -> Result<String> {
    if pattern.is_empty() {
        return Err(anyhow!("empty path glob is not allowed"));
    }
    if pattern.starts_with('/')
        || pattern.starts_with('~')
        || Path::new(pattern).components().any(is_parent_dir)
    {
        return Err(anyhow!(
            "path glob must be repo-relative and cannot contain \"..\": {pattern}"
        ));
    }
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    Ok(format!("./{pattern}"))
}

fn is_parent_dir(component: std::path::Component<'_>) -> bool {
    matches!(component, std::path::Component::ParentDir)
}

pub fn path_candidates(path: &str) -> [String; 2] {
    let normalized = path.trim().replace('\\', "/");
    let normalized = normalized
        .strip_prefix("./")
        .unwrap_or(&normalized)
        .to_string();
    [normalized.clone(), format!("./{normalized}")]
}

pub fn matches_any(path: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| matches_pattern(path, pattern))
}

pub fn matches_pattern(path: &str, pattern: &str) -> bool {
    path_candidates(path)
        .iter()
        .any(|candidate| fnmatch(candidate, pattern))
}

fn fnmatch(candidate: &str, pattern: &str) -> bool {
    let Ok(regex) = Regex::new(&format!("^{}$", translate_fnmatch(pattern))) else {
        return false;
    };
    regex.is_match(candidate)
}

fn translate_fnmatch(pattern: &str) -> String {
    let mut output = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '*' => output.push_str(".*"),
            '?' => output.push('.'),
            '[' => {
                if let Some((class, next_index)) = translate_class(&chars, index) {
                    output.push_str(&class);
                    index = next_index;
                } else {
                    output.push_str(r"\[");
                }
            }
            ch => output.push_str(&regex::escape(&ch.to_string())),
        }
        index += 1;
    }
    output
}

fn translate_class(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut index = start + 1;
    if index >= chars.len() {
        return None;
    }
    let mut class = String::from("[");
    if matches!(chars[index], '!' | '^') {
        class.push('^');
        index += 1;
    }
    if index < chars.len() && chars[index] == ']' {
        class.push(']');
        index += 1;
    }
    while index < chars.len() {
        let ch = chars[index];
        if ch == ']' {
            class.push(']');
            return Some((class, index));
        }
        if ch == '\\' {
            class.push_str(r"\\");
        } else {
            class.push(ch);
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- safe_artifact_id ---

    #[test]
    fn safe_artifact_id_rejects_non_child_components() {
        assert_eq!(safe_artifact_id("."), None);
        assert_eq!(safe_artifact_id(".."), None);
        assert_eq!(safe_artifact_id("!!!"), None);
        assert_eq!(safe_artifact_id("---"), None);
        assert_eq!(safe_artifact_id(&"a".repeat(201)), None);
    }

    #[test]
    fn safe_artifact_id_preserves_stable_child_components() {
        assert_eq!(safe_artifact_id("work tree").as_deref(), Some("work-tree"));
        assert_eq!(safe_artifact_id("worktree").as_deref(), Some("worktree"));
    }

    #[test]
    fn safe_artifact_id_handles_special_characters() {
        assert_eq!(safe_artifact_id("a/b").as_deref(), Some("a-b"));
        assert_eq!(
            safe_artifact_id("hello world!").as_deref(),
            Some("hello-world")
        );
        assert_eq!(
            safe_artifact_id("my_task-1.0").as_deref(),
            Some("my_task-1.0")
        );
    }

    #[test]
    fn safe_artifact_id_trims_leading_trailing_dashes() {
        assert_eq!(safe_artifact_id("  hello  ").as_deref(), Some("hello"));
        assert_eq!(safe_artifact_id("--abc--").as_deref(), Some("abc"));
    }

    #[test]
    fn safe_artifact_id_empty_and_whitespace() {
        assert_eq!(safe_artifact_id(""), None);
        assert_eq!(safe_artifact_id("   "), None);
    }

    #[test]
    fn safe_artifact_id_max_length_boundary() {
        let at_limit = "a".repeat(200);
        assert!(safe_artifact_id(&at_limit).is_some());
        let over_limit = "a".repeat(201);
        assert_eq!(safe_artifact_id(&over_limit), None);
    }

    // --- normalize_repo_glob ---

    #[test]
    fn normalize_repo_glob_valid_patterns() {
        assert_eq!(normalize_repo_glob("src/**").unwrap(), "./src/**");
        assert_eq!(normalize_repo_glob("./src/**").unwrap(), "./src/**");
        assert_eq!(normalize_repo_glob("*.rs").unwrap(), "./*.rs");
        assert_eq!(
            normalize_repo_glob("docs/file.md").unwrap(),
            "./docs/file.md"
        );
    }

    #[test]
    fn normalize_repo_glob_rejects_empty() {
        let error = normalize_repo_glob("").unwrap_err();
        assert!(error.to_string().contains("empty path glob"));
    }

    #[test]
    fn normalize_repo_glob_rejects_absolute_paths() {
        let error = normalize_repo_glob("/etc/passwd").unwrap_err();
        assert!(error.to_string().contains("repo-relative"));
    }

    #[test]
    fn normalize_repo_glob_rejects_home_expansion() {
        let error = normalize_repo_glob("~/something").unwrap_err();
        assert!(error.to_string().contains("repo-relative"));
    }

    #[test]
    fn normalize_repo_glob_rejects_parent_traversal() {
        let error = normalize_repo_glob("../secret").unwrap_err();
        assert!(error.to_string().contains("cannot contain \"..\""));

        let error = normalize_repo_glob("src/../../etc").unwrap_err();
        assert!(error.to_string().contains("cannot contain \"..\""));
    }

    // --- path_candidates ---

    #[test]
    fn path_candidates_normalizes_dotslash() {
        let candidates = path_candidates("./src/main.rs");
        assert_eq!(
            candidates,
            ["src/main.rs".to_string(), "./src/main.rs".to_string()]
        );
    }

    #[test]
    fn path_candidates_normalizes_backslashes() {
        let candidates = path_candidates("src\\main.rs");
        assert_eq!(
            candidates,
            ["src/main.rs".to_string(), "./src/main.rs".to_string()]
        );
    }

    #[test]
    fn path_candidates_trims_whitespace() {
        let candidates = path_candidates("  src/main.rs  ");
        assert_eq!(
            candidates,
            ["src/main.rs".to_string(), "./src/main.rs".to_string()]
        );
    }

    // --- matches_pattern / matches_any ---

    #[test]
    fn matches_pattern_basic_glob() {
        assert!(matches_pattern("src/main.rs", "./src/**"));
        assert!(matches_pattern("./src/main.rs", "./src/**"));
        assert!(!matches_pattern("tests/cli.rs", "./src/**"));
    }

    #[test]
    fn matches_pattern_wildcard() {
        assert!(matches_pattern("src/foo.rs", "./*.rs"));
        assert!(matches_pattern("hello.rs", "./*.rs"));
    }

    #[test]
    fn matches_pattern_exact() {
        assert!(matches_pattern("src/main.rs", "./src/main.rs"));
        assert!(!matches_pattern("src/lib.rs", "./src/main.rs"));
    }

    #[test]
    fn matches_any_checks_all_patterns() {
        let patterns = vec!["./src/**".to_string(), "./tests/**".to_string()];
        assert!(matches_any("src/main.rs", &patterns));
        assert!(matches_any("tests/cli.rs", &patterns));
        assert!(!matches_any("docs/readme.md", &patterns));
    }

    #[test]
    fn matches_any_empty_patterns() {
        assert!(!matches_any("src/main.rs", &[]));
    }

    // --- new_run_id ---

    #[test]
    fn new_run_id_contains_prefix() {
        let id = new_run_id("explore");
        assert!(id.starts_with("explore-"));
    }

    #[test]
    fn new_run_id_is_unique() {
        let id1 = new_run_id("run");
        let id2 = new_run_id("run");
        assert_ne!(id1, id2);
    }
}
