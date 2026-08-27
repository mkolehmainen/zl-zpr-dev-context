//! The workspace manifest (`workspace.yaml`) and the path resolution rules for
//! locating the workspace and the context checkout. See spec §3.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Deserialize;

/// Directory name of the context checkout inside the workspace (spec §3.3).
pub const CONTEXT_DIR_NAME: &str = "zpr-dev-context";

/// The manifest's file name inside the context checkout (spec §3.1).
pub const MANIFEST_FILE: &str = "workspace.yaml";

/// The only manifest version this tool understands (spec §3.1).
const SUPPORTED_VERSION: u32 = 1;

/// `workspace.yaml` in full. Unknown keys are ignored on purpose — no
/// `deny_unknown_fields` anywhere — so the manifest can grow ahead of the tool.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: u32,
    // Parsed so the manifest round-trips and unknown-key tolerance is tested;
    // nothing in v0.1 acts on the workspace name (spec §3.1).
    #[serde(default)]
    #[allow(dead_code)]
    pub workspace: WorkspaceMeta,
    #[serde(default)]
    pub repositories: Vec<Repo>,
    #[serde(default)]
    pub documentation: Documentation,
    #[serde(default)]
    pub agent: AgentConfig,
}

/// Informational only; nothing in v0.1 acts on it.
#[derive(Debug, Default, Deserialize)]
pub struct WorkspaceMeta {
    #[serde(default)]
    #[allow(dead_code)]
    pub name: Option<String>,
}

/// One repository to clone into the workspace.
#[derive(Debug, Deserialize)]
pub struct Repo {
    pub name: String,
    pub url: String,
    #[serde(default = "default_branch")]
    pub default_branch: String,
    #[serde(default)]
    pub context: RepoContext,
}

/// Per-repository context file names.
#[derive(Debug, Deserialize)]
pub struct RepoContext {
    /// Hand-written, repository-specific context, merged in when present.
    #[serde(default = "default_local_context")]
    pub local: String,
    /// The file `zpr-dev` generates and owns.
    #[serde(default = "default_generated_context")]
    pub generated: String,
}

/// Where the shared documentation tree lives inside the context checkout.
#[derive(Debug, Deserialize)]
pub struct Documentation {
    #[serde(default = "default_documentation_root")]
    pub root: String,
}

/// Agent-specific settings. Parsed and validated for existence in v0.1 only.
#[derive(Debug, Default, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub hermes: Option<HermesConfig>,
}

#[derive(Debug, Deserialize)]
pub struct HermesConfig {
    #[serde(default)]
    pub shared_skills: Option<String>,
}

fn default_branch() -> String {
    "main".to_string()
}

fn default_local_context() -> String {
    "AGENTS.repo.md".to_string()
}

fn default_generated_context() -> String {
    "AGENTS.md".to_string()
}

fn default_documentation_root() -> String {
    "docs".to_string()
}

// `Default` impls exist so an omitted block still gets the field defaults above;
// serde's `#[serde(default)]` on the containing field needs them.
impl Default for RepoContext {
    fn default() -> Self {
        Self {
            local: default_local_context(),
            generated: default_generated_context(),
        }
    }
}

impl Default for Documentation {
    fn default() -> Self {
        Self {
            root: default_documentation_root(),
        }
    }
}

/// Reads and validates the manifest at `path`. The structural checks live here
/// so `validate` and every other command share one code path (spec §7).
pub fn load(path: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read manifest {}: {e}", path.display()))?;
    parse(&text)
}

/// Parses manifest text and applies the structural checks. Separate from
/// [`load`] so it is testable without touching the filesystem.
pub fn parse(text: &str) -> Result<Manifest> {
    let manifest: Manifest = serde_yaml_ng::from_str(text)?;

    if manifest.version != SUPPORTED_VERSION {
        bail!(
            "unsupported manifest version {} (expected {SUPPORTED_VERSION})",
            manifest.version
        );
    }
    if manifest.repositories.is_empty() {
        bail!("manifest lists no repositories");
    }

    let mut seen: Vec<&str> = Vec::new();
    for repo in &manifest.repositories {
        if repo.name.trim().is_empty() {
            bail!("manifest contains a repository with an empty name");
        }
        if seen.contains(&repo.name.as_str()) {
            bail!("duplicate repository name: {}", repo.name);
        }
        seen.push(&repo.name);
    }

    Ok(manifest)
}

/// Picks the workspace directory: explicit flag, then `$ZPR_WORKSPACE`, then
/// `<home>/src/zpr`. Pure so it is testable without touching the environment.
pub fn resolve_workspace(flag: Option<&Path>, env: Option<&str>, home: &Path) -> PathBuf {
    match (flag, env) {
        (Some(path), _) => path.to_path_buf(),
        (None, Some(value)) => PathBuf::from(value),
        (None, None) => home.join("src").join("zpr"),
    }
}

/// Picks the context checkout: explicit flag, else the conventional child of
/// the workspace.
pub fn resolve_context(flag: Option<&Path>, workspace: &Path) -> PathBuf {
    match flag {
        Some(path) => path.to_path_buf(),
        None => workspace.join(CONTEXT_DIR_NAME),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest with a single three-line repository entry and nothing else.
    const MINIMAL: &str = "
version: 1
repositories:
  - name: zpr-core
    url: git@github.com:org-zpr/zpr-core.git
";

    #[test]
    fn minimal_repo_entry_gets_defaults() {
        let manifest = parse(MINIMAL).unwrap();
        let repo = &manifest.repositories[0];
        assert_eq!(repo.default_branch, "main");
        assert_eq!(repo.context.local, "AGENTS.repo.md");
        assert_eq!(repo.context.generated, "AGENTS.md");
        assert_eq!(manifest.documentation.root, "docs");
    }

    #[test]
    fn unknown_top_level_key_is_ignored() {
        let text = format!("{MINIMAL}future_feature:\n  enabled: true\n");
        assert!(parse(&text).is_ok());
    }

    #[test]
    fn version_other_than_one_is_rejected() {
        let text = MINIMAL.replace("version: 1", "version: 2");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn empty_repositories_is_rejected() {
        assert!(parse("version: 1\nrepositories: []\n").is_err());
    }

    #[test]
    fn duplicate_repository_names_are_rejected() {
        let text = format!("{MINIMAL}  - name: zpr-core\n    url: other\n");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn empty_repository_name_is_rejected() {
        let text = MINIMAL.replace("name: zpr-core", "name: \"\"");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn workspace_resolution_prefers_flag_then_env_then_home_default() {
        let home = Path::new("/home/dev");
        let flag = Path::new("/flag/ws");
        assert_eq!(
            resolve_workspace(Some(flag), Some("/env/ws"), home),
            PathBuf::from("/flag/ws")
        );
        assert_eq!(
            resolve_workspace(None, Some("/env/ws"), home),
            PathBuf::from("/env/ws")
        );
        assert_eq!(
            resolve_workspace(None, None, home),
            PathBuf::from("/home/dev/src/zpr")
        );
    }

    #[test]
    fn context_resolution_prefers_flag_then_workspace_child() {
        let workspace = Path::new("/home/dev/src/zpr");
        assert_eq!(
            resolve_context(Some(Path::new("/elsewhere/ctx")), workspace),
            PathBuf::from("/elsewhere/ctx")
        );
        assert_eq!(
            resolve_context(None, workspace),
            PathBuf::from("/home/dev/src/zpr/zpr-dev-context")
        );
    }

    /// The real manifest at the repository root must parse and match spec §3.2:
    /// ten repositories, all on `main` by way of the serde default.
    #[test]
    fn real_workspace_yaml_parses_with_ten_repositories() {
        let manifest = parse(include_str!("../../workspace.yaml")).expect("real manifest parses");
        assert_eq!(manifest.repositories.len(), 10);
        for repo in &manifest.repositories {
            assert_eq!(repo.default_branch, "main", "{} is not on main", repo.name);
        }
    }
}
