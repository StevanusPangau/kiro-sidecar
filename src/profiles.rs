use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileMode {
    ReadOnly,
    ScopedEdit,
    WorktreeEdit,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Profile {
    pub name: String,
    pub tools: Vec<String>,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub worktree_only: bool,
}

impl Profile {
    pub fn trust_tools(&self) -> String {
        self.tools.join(",")
    }
}

#[derive(Clone, Debug)]
pub struct ProfileCatalog {
    profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Deserialize)]
struct ProfilesFile {
    #[serde(default)]
    profiles: BTreeMap<String, RawProfile>,
}

#[derive(Debug, Deserialize)]
struct RawProfile {
    tools: Vec<String>,
    #[serde(default)]
    write: bool,
    #[serde(default)]
    worktree_only: bool,
}

impl ProfileCatalog {
    pub fn load(repo_root: &Path) -> Result<Self> {
        let mut profiles = builtin_profiles()?;
        let path = repo_root.join(".kiro-sidecar").join("profiles.toml");
        if path.exists() {
            let data = std::fs::read_to_string(&path)
                .with_context(|| format!("could not read {}", path.display()))?;
            let parsed: ProfilesFile = toml::from_str(&data)
                .with_context(|| format!("could not parse {}", path.display()))?;
            for (name, raw) in parsed.profiles {
                if profiles.contains_key(&name) {
                    bail!("project profile `{name}` cannot replace a built-in profile");
                }
                let tools = validate_tools(&name, raw.tools)?;
                validate_profile_flags(&name, &tools, raw.write, raw.worktree_only)?;
                let profile = Profile {
                    name: name.clone(),
                    tools,
                    write: raw.write,
                    worktree_only: raw.worktree_only,
                };
                profiles.insert(name, profile);
            }
        }
        Ok(Self { profiles })
    }

    pub fn names(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    pub fn resolve(
        &self,
        requested: Option<&str>,
        default_name: &str,
        mode: ProfileMode,
    ) -> Result<Profile> {
        let name = requested.unwrap_or(default_name);
        let profile = self
            .profiles
            .get(name)
            .ok_or_else(|| anyhow!("unknown Kiro permission profile `{name}`"))?;
        let mut profile = profile.clone();
        apply_legacy_env_override(&mut profile)?;
        match mode {
            ProfileMode::ReadOnly => {
                if profile.write {
                    bail!("profile `{name}` grants write tools and cannot be used for read-only commands");
                }
            }
            ProfileMode::ScopedEdit => {
                if !profile.write {
                    bail!("profile `{name}` does not grant scoped write tools");
                }
                if profile.worktree_only {
                    bail!("profile `{name}` is restricted to worktree edit modes");
                }
            }
            ProfileMode::WorktreeEdit => {
                if !profile.write {
                    bail!("profile `{name}` does not grant worktree write tools");
                }
            }
        }
        Ok(profile)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.profiles.contains_key(name)
    }
}

fn builtin_profiles() -> Result<BTreeMap<String, Profile>> {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        "read-only".to_string(),
        Profile {
            name: "read-only".to_string(),
            tools: validate_tools("read-only", default_tools(&["fs_read", "grep", "glob"]))?,
            write: false,
            worktree_only: false,
        },
    );
    profiles.insert(
        "web-research".to_string(),
        Profile {
            name: "web-research".to_string(),
            tools: validate_tools(
                "web-research",
                ["fs_read", "grep", "glob", "web_search", "web_fetch"]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            )?,
            write: false,
            worktree_only: false,
        },
    );
    profiles.insert(
        "scoped-edit".to_string(),
        Profile {
            name: "scoped-edit".to_string(),
            tools: validate_tools(
                "scoped-edit",
                default_tools(&["fs_read", "fs_write", "grep", "glob"]),
            )?,
            write: true,
            worktree_only: false,
        },
    );
    profiles.insert(
        "worktree-edit".to_string(),
        Profile {
            name: "worktree-edit".to_string(),
            tools: validate_tools(
                "worktree-edit",
                default_tools(&["fs_read", "fs_write", "grep", "glob"]),
            )?,
            write: true,
            worktree_only: true,
        },
    );
    Ok(profiles)
}

fn default_tools(defaults: &[&str]) -> Vec<String> {
    defaults.iter().map(|value| value.to_string()).collect()
}

fn apply_legacy_env_override(profile: &mut Profile) -> Result<()> {
    let (env_name, defaults) = match profile.name.as_str() {
        "read-only" => (
            "KIRO_TRUST_TOOLS",
            default_tools(&["fs_read", "grep", "glob"]),
        ),
        "scoped-edit" | "worktree-edit" => (
            "KIRO_EDIT_TRUST_TOOLS",
            default_tools(&["fs_read", "fs_write", "grep", "glob"]),
        ),
        _ => return Ok(()),
    };
    let Ok(value) = env::var(env_name) else {
        return Ok(());
    };
    let tools = parse_tool_csv(&value);
    let defaults = defaults.into_iter().collect::<BTreeSet<_>>();
    let extras = tools
        .iter()
        .filter(|tool| !defaults.contains(*tool))
        .cloned()
        .collect::<Vec<_>>();
    if !extras.is_empty() {
        bail!(
            "{env_name} cannot widen built-in profile `{}` with {}; define an explicit profile in .kiro-sidecar/profiles.toml instead",
            profile.name,
            extras.join(",")
        );
    }
    profile.tools = validate_tools(&profile.name, tools)?;
    validate_profile_flags(
        &profile.name,
        &profile.tools,
        profile.write,
        profile.worktree_only,
    )?;
    Ok(())
}

fn parse_tool_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|tool| !tool.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn validate_tools(profile_name: &str, tools: Vec<String>) -> Result<Vec<String>> {
    if tools.is_empty() {
        bail!("profile `{profile_name}` must list at least one trusted tool");
    }
    let mut seen = BTreeSet::new();
    let mut output = Vec::with_capacity(tools.len());
    for tool in tools {
        if tool == "--trust-all-tools" || tool == "trust-all-tools" || tool == "*" {
            bail!("profile `{profile_name}` cannot use --trust-all-tools");
        }
        if tool.contains(',') {
            bail!(
                "profile `{profile_name}` has invalid comma-delimited tool entry `{tool}`; TOML profile tools must be separate array items, for example [\"fs_read\", \"fs_write\"]"
            );
        }
        if is_shell_capable_tool(&tool) {
            bail!("profile `{profile_name}` cannot grant shell-capable tool `{tool}`");
        }
        if tool.contains(char::is_whitespace) {
            bail!("profile `{profile_name}` has invalid whitespace in tool `{tool}`");
        }
        if seen.insert(tool.clone()) {
            output.push(tool);
        }
    }
    Ok(output)
}

fn validate_profile_flags(
    profile_name: &str,
    tools: &[String],
    write: bool,
    worktree_only: bool,
) -> Result<()> {
    let grants_write = tools
        .iter()
        .any(|tool| matches!(tool.as_str(), "fs_write" | "write"));
    if grants_write && !write {
        bail!("profile `{profile_name}` grants write tools but is missing write = true");
    }
    if write && !grants_write {
        bail!("profile `{profile_name}` has write = true but does not grant fs_write");
    }
    if worktree_only && !write {
        bail!("profile `{profile_name}` has worktree_only = true without write = true");
    }
    Ok(())
}

fn is_shell_capable_tool(tool: &str) -> bool {
    matches!(
        tool,
        "execute_bash" | "bash" | "shell" | "terminal" | "run_shell" | "execute_shell"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolves_builtin_read_only_profile() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let profile = catalog.resolve(None, "read-only", ProfileMode::ReadOnly)?;
        assert_eq!(profile.trust_tools(), "fs_read,grep,glob");
        Ok(())
    }

    #[test]
    fn resolves_builtin_scoped_edit_profile() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let profile = catalog.resolve(None, "scoped-edit", ProfileMode::ScopedEdit)?;
        assert_eq!(profile.trust_tools(), "fs_read,fs_write,grep,glob");
        assert!(profile.write);
        assert!(!profile.worktree_only);
        Ok(())
    }

    #[test]
    fn resolves_builtin_worktree_edit_profile() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let profile = catalog.resolve(None, "worktree-edit", ProfileMode::WorktreeEdit)?;
        assert_eq!(profile.trust_tools(), "fs_read,fs_write,grep,glob");
        assert!(profile.write);
        assert!(profile.worktree_only);
        Ok(())
    }

    #[test]
    fn resolves_builtin_web_research_profile() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let profile = catalog.resolve(Some("web-research"), "read-only", ProfileMode::ReadOnly)?;
        assert_eq!(
            profile.trust_tools(),
            "fs_read,grep,glob,web_search,web_fetch"
        );
        assert!(!profile.write);
        Ok(())
    }

    #[test]
    fn rejects_write_profile_for_read_only_command() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let error = catalog
            .resolve(Some("scoped-edit"), "read-only", ProfileMode::ReadOnly)
            .unwrap_err();
        assert!(error.to_string().contains("grants write tools"));
        Ok(())
    }

    #[test]
    fn rejects_read_only_profile_for_scoped_edit_command() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let error = catalog
            .resolve(Some("read-only"), "scoped-edit", ProfileMode::ScopedEdit)
            .unwrap_err();
        assert!(error.to_string().contains("does not grant scoped write"));
        Ok(())
    }

    #[test]
    fn rejects_worktree_only_profile_for_scoped_edit() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let error = catalog
            .resolve(
                Some("worktree-edit"),
                "scoped-edit",
                ProfileMode::ScopedEdit,
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("restricted to worktree edit modes"));
        Ok(())
    }

    #[test]
    fn rejects_unknown_profile() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let error = catalog
            .resolve(Some("nonexistent"), "read-only", ProfileMode::ReadOnly)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("unknown Kiro permission profile"));
        Ok(())
    }

    #[test]
    fn loads_valid_project_profile() -> Result<()> {
        let tmp = TempDir::new()?;
        let sidecar = tmp.path().join(".kiro-sidecar");
        std::fs::create_dir_all(&sidecar)?;
        std::fs::write(
            sidecar.join("profiles.toml"),
            "[profiles.custom-read]\ntools = [\"fs_read\", \"grep\"]\n",
        )?;

        let catalog = ProfileCatalog::load(tmp.path())?;
        assert!(catalog.contains("custom-read"));
        let profile = catalog.resolve(Some("custom-read"), "read-only", ProfileMode::ReadOnly)?;
        assert_eq!(profile.trust_tools(), "fs_read,grep");
        Ok(())
    }

    #[test]
    fn rejects_comma_delimited_toml_tool_entry() -> Result<()> {
        let tmp = TempDir::new()?;
        let sidecar = tmp.path().join(".kiro-sidecar");
        std::fs::create_dir_all(&sidecar)?;
        std::fs::write(
            sidecar.join("profiles.toml"),
            "[profiles.bad]\ntools = [\"fs_read,fs_write\"]\nwrite = false\n",
        )?;

        let error = ProfileCatalog::load(tmp.path()).unwrap_err();

        assert!(error.to_string().contains("separate array items"));
        assert!(error.to_string().contains("[\"fs_read\", \"fs_write\"]"));
        Ok(())
    }

    #[test]
    fn rejects_project_profile_that_replaces_builtin() -> Result<()> {
        let tmp = TempDir::new()?;
        let sidecar = tmp.path().join(".kiro-sidecar");
        std::fs::create_dir_all(&sidecar)?;
        std::fs::write(
            sidecar.join("profiles.toml"),
            "[profiles.read-only]\ntools = [\"fs_read\", \"grep\", \"glob\", \"web_search\"]\n",
        )?;

        let error = ProfileCatalog::load(tmp.path()).unwrap_err();

        assert!(error
            .to_string()
            .contains("project profile `read-only` cannot replace a built-in profile"));
        Ok(())
    }

    #[test]
    fn rejects_shell_capable_tools() {
        let error = validate_tools("bad", vec!["bash".to_string()]).unwrap_err();
        assert!(error.to_string().contains("shell-capable tool"));

        let error = validate_tools("bad", vec!["execute_bash".to_string()]).unwrap_err();
        assert!(error.to_string().contains("shell-capable tool"));
    }

    #[test]
    fn rejects_trust_all_tools() {
        let error = validate_tools("bad", vec!["--trust-all-tools".to_string()]).unwrap_err();
        assert!(error.to_string().contains("cannot use --trust-all-tools"));

        let error = validate_tools("bad", vec!["*".to_string()]).unwrap_err();
        assert!(error.to_string().contains("cannot use --trust-all-tools"));
    }

    #[test]
    fn rejects_empty_tools_list() {
        let error = validate_tools("bad", vec![]).unwrap_err();
        assert!(error.to_string().contains("at least one trusted tool"));
    }

    #[test]
    fn rejects_whitespace_in_tool_name() {
        let error = validate_tools("bad", vec!["fs read".to_string()]).unwrap_err();
        assert!(error.to_string().contains("invalid whitespace"));
    }

    #[test]
    fn deduplicates_tools() -> Result<()> {
        let tools = validate_tools("ok", vec!["fs_read".to_string(), "fs_read".to_string()])?;
        assert_eq!(tools, vec!["fs_read"]);
        Ok(())
    }

    #[test]
    fn parse_tool_csv_handles_edge_cases() {
        assert_eq!(parse_tool_csv("fs_read,grep"), vec!["fs_read", "grep"]);
        assert_eq!(parse_tool_csv(" fs_read , grep "), vec!["fs_read", "grep"]);
        assert!(parse_tool_csv("").is_empty());
        assert!(parse_tool_csv(",,,").is_empty());
    }

    #[test]
    fn catalog_names_returns_all_profiles() -> Result<()> {
        let tmp = TempDir::new()?;
        let catalog = ProfileCatalog::load(tmp.path())?;
        let names = catalog.names();
        assert!(names.contains(&"read-only".to_string()));
        assert!(names.contains(&"web-research".to_string()));
        assert!(names.contains(&"scoped-edit".to_string()));
        assert!(names.contains(&"worktree-edit".to_string()));
        assert_eq!(names.len(), 4);
        Ok(())
    }

    #[test]
    fn validate_profile_flags_rejects_write_tool_without_write_flag() {
        let error =
            validate_profile_flags("bad", &["fs_write".to_string()], false, false).unwrap_err();
        assert!(error.to_string().contains("missing write = true"));
    }

    #[test]
    fn validate_profile_flags_rejects_write_flag_without_write_tool() {
        let error =
            validate_profile_flags("bad", &["fs_read".to_string()], true, false).unwrap_err();
        assert!(error.to_string().contains("does not grant fs_write"));
    }

    #[test]
    fn validate_profile_flags_rejects_worktree_only_without_write() {
        let error =
            validate_profile_flags("bad", &["fs_read".to_string()], false, true).unwrap_err();
        assert!(error
            .to_string()
            .contains("worktree_only = true without write = true"));
    }
}
