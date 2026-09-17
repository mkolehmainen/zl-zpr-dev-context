//! The `build` command: build sets (spec-003). This stage (B1) implements the
//! build-set schema, default-manifest selection, ref resolution and the
//! `--dry-run` report; the gates, worktrees, builds and tiers it describes are
//! specified in `docs/specs/spec-003-build.md` and land in B2–B5.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::Manifest;

/// The only build-set version this tool understands (spec-003 §2.1).
const SUPPORTED_VERSION: u32 = 1;

/// A build set: the input manifest of spec-003 §2. Unknown keys are ignored on
/// purpose — no `deny_unknown_fields` — matching `workspace.yaml`'s tolerance;
/// it is also what makes an emitted manifest (§3) valid input, because its
/// `resolved:` block is an unknown key by design.
#[derive(Debug, Deserialize, Serialize)]
pub struct BuildSet {
    pub version: u32,
    /// Names the build and its dist directory.
    pub name: String,
    /// Repository name → tag, branch or sha. A `BTreeMap` so iteration and
    /// serialization order are deterministic.
    pub repositories: BTreeMap<String, String>,
    /// Known pin divergences, each with its reviewed reason (spec-003 §2.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_pin_drift: Vec<PinDrift>,
}

/// One tolerated pin disagreement. The reason is mandatory: it is the reviewed
/// record of why the divergence is acceptable.
#[derive(Debug, Deserialize, Serialize)]
pub struct PinDrift {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub reason: String,
}

/// Parses build-set text and applies the structural checks that need no
/// workspace manifest. Separate from file loading so it is testable without
/// touching the filesystem, like `config::parse`.
pub fn parse(_text: &str) -> Result<BuildSet> {
    bail!("unimplemented")
}

/// Validates the build set against `workspace.yaml`: every repository key must
/// name a repository the workspace manifest declares (spec-003 §2.1), because
/// `url` and `default_branch` come from there and nothing is duplicated.
pub fn validate_against(_set: &BuildSet, _manifest: &Manifest) -> Result<()> {
    bail!("unimplemented")
}

/// Picks the default build set: the newest file in `<context>/build-sets/` by
/// file name, descending — set names are dates, so the newest name is the
/// newest set and a rename cannot silently change which set builds
/// (spec-003 §2.2).
pub fn default_manifest_path(_context: &Path) -> Result<PathBuf> {
    bail!("unimplemented")
}

/// Everything `zpr-dev build` takes from the command line, threaded as one
/// struct so the dispatch arm in `main.rs` stays a single call.
#[derive(Debug)]
pub struct BuildArgs {
    pub manifest: Option<PathBuf>,
    pub tip: bool,
    pub test: Option<String>,
    pub repo: Option<String>,
    pub build_dir: Option<PathBuf>,
    pub keep: bool,
    pub allow_pin_drift: bool,
    pub no_tarball: bool,
}

/// The `build` command (spec-003 §7). In B1 only `--dry-run` does anything:
/// it resolves and reports without touching the workspace.
pub fn run(_ctx: &crate::Ctx, _args: &BuildArgs) -> Result<std::process::ExitCode> {
    bail!("unimplemented")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build set naming both fixture repositories, with one tolerated drift.
    const VALID: &str = "\
version: 1
name: 2026-09-17
repositories:
  zl-zpr-core: v0.3.1
  zl-zpr-common: main
allow_pin_drift:
  - crate: rcu
    reason: \"zl-zpr-common pins zpr-utils-v0.1.0, ph pins rcu-v0.1.2; zipline#18\"
";

    /// A workspace manifest declaring exactly the two fixture repositories.
    fn workspace_manifest() -> Manifest {
        crate::config::parse(
            "version: 1\nrepositories:\n  - name: zl-zpr-core\n    url: a\n  - name: zl-zpr-common\n    url: b\n",
        )
        .unwrap()
    }

    #[test]
    fn valid_build_set_parses() {
        let set = parse(VALID).unwrap();
        assert_eq!(set.version, 1);
        assert_eq!(set.name, "2026-09-17");
        assert_eq!(set.repositories["zl-zpr-core"], "v0.3.1");
        assert_eq!(set.repositories["zl-zpr-common"], "main");
        assert_eq!(set.allow_pin_drift.len(), 1);
        assert_eq!(set.allow_pin_drift[0].crate_name, "rcu");
    }

    /// Unknown keys are tolerated (spec-003 §2.1), which is also the property
    /// that lets an emitted manifest's `resolved:` block be ignored on re-read.
    #[test]
    fn unknown_top_level_key_is_tolerated() {
        let text = format!("{VALID}resolved:\n  built_at: sometime\n");
        assert!(parse(&text).is_ok());
    }

    #[test]
    fn version_two_is_rejected() {
        let text = VALID.replace("version: 1", "version: 2");
        let error = parse(&text).unwrap_err().to_string();
        assert!(error.contains("version"), "{error}");
    }

    #[test]
    fn empty_name_is_rejected() {
        let text = VALID.replace("name: 2026-09-17", "name: \"\"");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn empty_repositories_is_rejected() {
        assert!(parse("version: 1\nname: x\nrepositories: {}\n").is_err());
    }

    /// A key that is not a repository in `workspace.yaml` is rejected *naming
    /// it* (spec-003 §2.1), so the fix is obvious from the message.
    #[test]
    fn unknown_repository_is_rejected_naming_it() {
        let text = VALID.replace("zl-zpr-core:", "zl-zpr-nonesuch:");
        let set = parse(&text).unwrap();
        let error = validate_against(&set, &workspace_manifest())
            .unwrap_err()
            .to_string();
        assert!(error.contains("zl-zpr-nonesuch"), "{error}");
    }

    #[test]
    fn known_repositories_validate() {
        let set = parse(VALID).unwrap();
        assert!(validate_against(&set, &workspace_manifest()).is_ok());
    }

    /// An `allow_pin_drift` entry without a reason is rejected naming the
    /// crate: the reason is the reviewed record of the divergence.
    #[test]
    fn pin_drift_without_reason_is_rejected() {
        let text = VALID.replace(
            "    reason: \"zl-zpr-common pins zpr-utils-v0.1.0, ph pins rcu-v0.1.2; zipline#18\"",
            "    reason: \"\"",
        );
        let error = parse(&text).unwrap_err().to_string();
        assert!(error.contains("rcu"), "{error}");
        assert!(error.contains("reason"), "{error}");
    }

    /// Newest file by name wins: dates sort lexicographically, and only
    /// `.yaml`/`.yml` regular files count (spec-003 §2.2).
    #[test]
    fn default_manifest_is_newest_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("build-sets");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("2026-08-01.yaml"), "old").unwrap();
        std::fs::write(dir.join("2026-09-17.yaml"), "new").unwrap();
        std::fs::write(dir.join("zzz.txt"), "not a manifest").unwrap();

        let path = default_manifest_path(tmp.path()).unwrap();
        assert_eq!(path, dir.join("2026-09-17.yaml"));
    }

    #[test]
    fn absent_build_sets_directory_is_a_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let error = default_manifest_path(tmp.path()).unwrap_err().to_string();
        assert!(error.contains("build-sets"), "{error}");
        assert!(
            error.contains("--manifest") && error.contains("--tip"),
            "remedy not named: {error}"
        );
    }

    #[test]
    fn empty_build_sets_directory_is_a_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("build-sets")).unwrap();
        let error = default_manifest_path(tmp.path()).unwrap_err().to_string();
        assert!(error.contains("build-sets"), "{error}");
    }
}
