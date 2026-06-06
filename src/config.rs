use std::env;
use std::path::PathBuf;

use anyhow::{bail, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub kiro_cli: String,
    pub model: String,
    pub effort: Option<String>,
    pub read_tools: String,
    pub edit_tools: String,
    pub tmp_root: PathBuf,
    pub agent_dir: PathBuf,
    pub timeout_seconds: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let effort = env::var("KIRO_EFFORT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if let Some(value) = &effort {
            validate_effort("KIRO_EFFORT", value)?;
        }

        Ok(Self {
            kiro_cli: env::var("KIRO_CLI").unwrap_or_else(|_| "kiro-cli".to_string()),
            model: env::var("KIRO_MODEL").unwrap_or_else(|_| "claude-opus-4.6".to_string()),
            effort,
            read_tools: env::var("KIRO_TRUST_TOOLS")
                .unwrap_or_else(|_| "fs_read,grep,glob".to_string()),
            edit_tools: env::var("KIRO_EDIT_TRUST_TOOLS")
                .unwrap_or_else(|_| "fs_read,fs_write,grep,glob".to_string()),
            tmp_root: PathBuf::from(
                env::var("KIRO_TMP_ROOT").unwrap_or_else(|_| "/private/tmp".to_string()),
            ),
            agent_dir: PathBuf::from(
                env::var("KIRO_AGENT_DIR").unwrap_or_else(|_| ".kiro/agents".to_string()),
            ),
            timeout_seconds: env_u64("KIRO_TIMEOUT_SECONDS", 1200),
        })
    }
}

pub fn validate_effort(label: &str, value: &str) -> Result<()> {
    if matches!(value, "low" | "medium" | "high" | "xhigh" | "max") {
        return Ok(());
    }
    bail!("{label} must be one of low, medium, high, xhigh, max")
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| value.max(1))
        .unwrap_or(default)
}

pub const STRUCTURED_OUTPUT: &str = r#"Return a concise response in this exact structure unless the user explicitly requested
an exact literal output:

CHANGED_FILES:
- path

SUMMARY:
- ...

DECISIONS:
- ...

UNCERTAINTIES:
- ...

TESTS_NOT_RUN:
- ...

RISK_NOTES:
- ...
"#;

pub const PATCH_OUTPUT: &str = r#"Do not modify files. Return only this exact structure:

PATCH:
```diff
...
```

NO_EXTRA_TEXT_AFTER_PATCH
"#;

pub const DEFAULT_DENIES: &[&str] = &[
    "./.git/**",
    "./.env",
    "./.env.*",
    "./**/.env",
    "./**/.env.*",
    "./.codex/**",
    "./.kiro/**",
    "./node_modules/**",
    "./**/node_modules/**",
    "./.venv/**",
    "./venv/**",
    "./**/__pycache__/**",
    "./.next/**",
    "./dist/**",
    "./build/**",
    "./coverage/**",
];
