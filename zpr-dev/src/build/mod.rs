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

/// The binary-producing repositories in build order (spec-003 §5). This is
/// also the repository list `--tip` resolves: a tip build has no build set to
/// name repositories, and these five are what a set may build. Repositories a
/// *named* set lists are validated against `workspace.yaml` instead.
const BUILD_ORDER: [&str; 5] = [
    "zl-zpr-compiler",
    "zl-zpr-visaservice",
    "zl-zpr-core",
    "zl-zpr-coredns",
    "zl-zpr-demo",
];

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
pub fn parse(text: &str) -> Result<BuildSet> {
    let set: BuildSet = serde_yaml_ng::from_str(text)?;

    if set.version != SUPPORTED_VERSION {
        bail!(
            "unsupported build-set version {} (expected {SUPPORTED_VERSION})",
            set.version
        );
    }
    if set.name.trim().is_empty() {
        bail!("build set has an empty name; the name labels the build and its dist directory");
    }
    if set.repositories.is_empty() {
        bail!("build set lists no repositories");
    }
    for (name, reference) in &set.repositories {
        if reference.trim().is_empty() {
            bail!("repository {name} has an empty ref");
        }
    }
    for drift in &set.allow_pin_drift {
        // The reason is the reviewed record of a known divergence; an entry
        // without one is a suppression with no audit trail (spec-003 §2.1).
        if drift.reason.trim().is_empty() {
            bail!(
                "allow_pin_drift entry for crate `{}` has no reason; every entry must say why",
                drift.crate_name
            );
        }
    }
    Ok(set)
}

/// Validates the build set against `workspace.yaml`: every repository key must
/// name a repository the workspace manifest declares (spec-003 §2.1), because
/// `url` and `default_branch` come from there and nothing is duplicated.
pub fn validate_against(set: &BuildSet, manifest: &Manifest) -> Result<()> {
    for name in set.repositories.keys() {
        if manifest.repo(name).is_none() {
            bail!("build set names `{name}`, which is not a repository in workspace.yaml");
        }
    }
    Ok(())
}

/// Picks the default build set: the newest file in `<context>/build-sets/` by
/// file name, descending — set names are dates, so the newest name is the
/// newest set and a rename cannot silently change which set builds
/// (spec-003 §2.2).
pub fn default_manifest_path(context: &Path) -> Result<PathBuf> {
    let dir = context.join("build-sets");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => bail!(
            "no build-sets/ directory in {}; pass --manifest <path> or --tip",
            context.display()
        ),
    };

    // Only regular files that look like manifests count, so a stray README or
    // subdirectory can never be selected as a build set.
    let mut manifests: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("yaml") | Some("yml")
                )
        })
        .collect();

    manifests.sort();
    match manifests.pop() {
        Some(path) => Ok(path),
        None => bail!(
            "build-sets/ in {} is empty; pass --manifest <path> or --tip",
            context.display()
        ),
    }
}

/// One repository's resolution: the ref asked for and the sha it names.
#[derive(Debug, PartialEq)]
pub struct Resolved {
    pub repo: String,
    /// What was asked for: the manifest value, or `origin/<default_branch>`
    /// under `--tip`.
    pub reference: String,
    /// The full 40-character commit sha.
    pub sha: String,
}

/// Resolves every entry of the build set to a 40-character sha against the
/// workspace checkouts (spec-003 §2.3). No fetch: resolution sees only what
/// the local repositories already have. A missing or non-Git directory, and an
/// unknown ref, are errors naming the repository (and the ref).
pub fn resolve_set(workspace: &Path, set: &BuildSet) -> Result<Vec<Resolved>> {
    set.repositories
        .iter()
        .map(|(name, reference)| resolve_one(workspace, name, reference))
        .collect()
}

/// Resolves `origin/<default_branch>` for every named repository — the `--tip`
/// path, which ignores manifest values entirely (spec-003 §2.3).
pub fn resolve_tip(workspace: &Path, repos: &[(&str, &str)]) -> Result<Vec<Resolved>> {
    repos
        .iter()
        .map(|(name, default_branch)| {
            resolve_one(workspace, name, &format!("origin/{default_branch}"))
        })
        .collect()
}

/// Resolves one ref in one workspace checkout, with the directory checks that
/// make the failure modes diagnosable: a missing directory and a non-Git
/// directory each name the repository rather than surfacing a raw git error.
fn resolve_one(workspace: &Path, name: &str, reference: &str) -> Result<Resolved> {
    let dir = workspace.join(name);
    if !dir.is_dir() {
        bail!(
            "repository {name} is not checked out at {} (run: zpr-dev setup)",
            dir.display()
        );
    }
    if !crate::git::is_repo(&dir) {
        bail!(
            "repository {name} at {} is not a git repository",
            dir.display()
        );
    }
    let sha = crate::git::rev_parse(&dir, reference)
        .map_err(|e| anyhow::anyhow!("repository {name}: {e}"))?;
    Ok(Resolved {
        repo: name.to_string(),
        reference: reference.to_string(),
        sha,
    })
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
/// it resolves and reports without touching the workspace — no worktree, no
/// directory, no log file, and no `git fetch` (spec-003 §7.2).
pub fn run(ctx: &crate::Ctx, args: &BuildArgs) -> Result<std::process::ExitCode> {
    // Flags whose stages have not landed parse but are inert (spec-003 §7.1);
    // saying so beats silently ignoring them.
    for (flag, set) in [
        ("--test", args.test.is_some()),
        ("--repo", args.repo.is_some()),
        ("--keep", args.keep),
        ("--allow-pin-drift", args.allow_pin_drift),
        ("--no-tarball", args.no_tarball),
    ] {
        if set && !ctx.quiet {
            println!("note: {flag} parses but is inert until its stage (B2-B5) lands");
        }
    }

    if !ctx.dry_run {
        bail!(
            "only `build --dry-run` is implemented at this stage (B1); \
             gates, worktrees and builds land with B2-B3 (spec-003 §1.2)"
        );
    }

    let manifest = crate::config::load(&ctx.context.join(crate::config::MANIFEST_FILE))?;

    // `--tip` needs no build set at all: the repository list is the
    // binary-producing set of spec-003 §5 and the default branches come from
    // workspace.yaml (spec-003 §2.3). A named set supplies both instead.
    let (name, resolution, skipped) = if args.tip {
        let mut repos: Vec<(&str, &str)> = Vec::new();
        let mut skipped: Vec<&str> = Vec::new();
        for wanted in BUILD_ORDER {
            match manifest.repo(wanted) {
                Some(repo) => repos.push((&repo.name, &repo.default_branch)),
                // Not declared in this workspace: resolvable nowhere, so it is
                // reported rather than silently absent from the plan.
                None => skipped.push(wanted),
            }
        }
        if repos.is_empty() {
            bail!("--tip found none of the build-set repositories in workspace.yaml");
        }
        (
            "tip".to_string(),
            resolve_tip(&ctx.workspace, &repos),
            skipped,
        )
    } else {
        let path = match &args.manifest {
            Some(path) => path.clone(),
            None => default_manifest_path(&ctx.context)?,
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("cannot read build set {}: {e}", path.display()))?;
        let set = parse(&text)?;
        validate_against(&set, &manifest)?;
        (set.name.clone(), resolve_set(&ctx.workspace, &set), vec![])
    };

    // A resolution failure is a gate-style finding — the set is incoherent on
    // this machine — so it exits 1, not 2 (spec-003 §7.1).
    let resolved = match resolution {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("error: {error:#}");
            return Ok(std::process::ExitCode::from(1));
        }
    };

    report_dry_run(ctx, &name, &resolved, &skipped, args.build_dir.as_deref());
    Ok(std::process::ExitCode::SUCCESS)
}

/// Prints the §7.2 dry-run report: resolved shas, planned build order, planned
/// tiers with probe results, and the dist/ target. Read-only by construction —
/// the only processes it may spawn are the read-only tier probes.
fn report_dry_run(
    ctx: &crate::Ctx,
    name: &str,
    resolved: &[Resolved],
    skipped: &[&str],
    build_dir: Option<&Path>,
) {
    if ctx.quiet {
        return;
    }
    println!("build set {name} (dry-run)");
    println!();
    println!("resolved refs:");
    let width = resolved.iter().map(|r| r.repo.len()).max().unwrap_or(0);
    for entry in resolved {
        println!(
            "  {:width$}  {} -> {}",
            entry.repo, entry.reference, entry.sha
        );
    }
    for name in skipped {
        println!("  {name}: not in workspace.yaml, skipped");
    }
    println!();

    // Build order: the resolved repositories that have a recipe, in the fixed
    // §5 order. A resolved repository without a recipe is stated, not hidden.
    let ordered: Vec<&str> = BUILD_ORDER
        .iter()
        .filter(|name| resolved.iter().any(|r| r.repo == **name))
        .copied()
        .collect();
    println!("build order: {}", ordered.join(", "));
    let recipeless: Vec<&str> = resolved
        .iter()
        .map(|r| r.repo.as_str())
        .filter(|name| !BUILD_ORDER.contains(name))
        .collect();
    if !recipeless.is_empty() {
        println!(
            "  (resolved but no build recipe: {})",
            recipeless.join(", ")
        );
    }
    println!();

    println!("tiers (planned):");
    println!("  unit    would run");
    println!("  netns   not requested (--test netns); {}", netns_probe());
    println!(
        "  docker  not requested (--test docker); {}",
        docker_probe()
    );
    println!();

    let build_dir = build_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ctx.workspace.join(".zpr-build").join(name));
    println!("dist: {}", build_dir.join("dist").display());
    println!("dry-run: nothing was created, and nothing was fetched");
}

/// True when `program` is on `PATH` — the read-only half of a prerequisite
/// probe (spec-003 §7.2).
fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

/// The `netns` tier's prerequisite probe results, as one report fragment. Every
/// check here is read-only: `sudo -n true` never prompts and changes nothing.
fn netns_probe() -> String {
    let mut missing: Vec<&str> = Vec::new();
    if !cfg!(target_os = "linux") {
        missing.push("linux");
    }
    let sudo = std::process::Command::new("sudo")
        .args(["-n", "true"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !sudo {
        missing.push("passwordless sudo");
    }
    if !on_path("valkey-server") {
        missing.push("valkey-server");
    }
    if !on_path("python3") {
        missing.push("python3");
    }
    if missing.is_empty() {
        "prerequisites present".to_string()
    } else {
        format!("missing: {}", missing.join(", "))
    }
}

/// The `docker` tier's prerequisite probe results.
fn docker_probe() -> String {
    if on_path("docker") {
        "prerequisites present".to_string()
    } else {
        "missing: docker".to_string()
    }
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

    // -- ref resolution (spec-003 §2.3) --------------------------------------

    /// Runs git in `dir` for fixture setup, panicking on failure.
    fn setup_git(dir: &Path, args: &[&str]) -> String {
        crate::git::git(dir, args).unwrap()
    }

    /// A workspace holding one repository, `zl-zpr-core`, cloned from a local
    /// bare origin so `origin/main` exists — what `--tip` resolves.
    fn workspace_with_repo() -> (tempfile::TempDir, PathBuf, String) {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        setup_git(&origin, &["init", "--bare", "-b", "main"]);

        let scratch = tmp.path().join("scratch");
        setup_git(
            tmp.path(),
            &[
                "clone",
                &origin.to_string_lossy(),
                &scratch.to_string_lossy(),
            ],
        );
        setup_git(&scratch, &["config", "user.name", "zpr-dev tests"]);
        setup_git(&scratch, &["config", "user.email", "tests@example.invalid"]);
        setup_git(&scratch, &["config", "commit.gpgsign", "false"]);
        std::fs::write(scratch.join("README.md"), "fixture\n").unwrap();
        setup_git(&scratch, &["add", "-A"]);
        setup_git(&scratch, &["commit", "-m", "seed"]);
        setup_git(&scratch, &["tag", "v0.3.1"]);
        setup_git(&scratch, &["push", "origin", "HEAD:main", "--tags"]);
        let sha = setup_git(&scratch, &["rev-parse", "HEAD"]);
        std::fs::remove_dir_all(&scratch).unwrap();

        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        setup_git(
            &workspace,
            &["clone", &origin.to_string_lossy(), "zl-zpr-core"],
        );
        (tmp, workspace, sha)
    }

    /// A build set naming only `zl-zpr-core` at `reference`.
    fn one_repo_set(reference: &str) -> BuildSet {
        parse(&format!(
            "version: 1\nname: t\nrepositories:\n  zl-zpr-core: {reference}\n"
        ))
        .unwrap()
    }

    /// A tag, a branch, a short sha and the full sha all resolve to the same
    /// 40-character sha.
    #[test]
    fn resolve_set_handles_tag_branch_short_and_full_sha() {
        let (_tmp, workspace, sha) = workspace_with_repo();
        for reference in ["v0.3.1", "main", &sha[..7], sha.as_str()] {
            let resolved = resolve_set(&workspace, &one_repo_set(reference)).unwrap();
            assert_eq!(resolved.len(), 1, "ref {reference}");
            assert_eq!(resolved[0].repo, "zl-zpr-core");
            assert_eq!(resolved[0].reference, reference);
            assert_eq!(resolved[0].sha, sha, "ref {reference}");
        }
    }

    /// An unknown ref errors naming **both** the repository and the ref.
    #[test]
    fn resolve_set_unknown_ref_names_repository_and_ref() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let error = resolve_set(&workspace, &one_repo_set("v9.9.9"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("zl-zpr-core"), "{error}");
        assert!(error.contains("v9.9.9"), "{error}");
    }

    #[test]
    fn resolve_set_missing_repository_directory_errors_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        let error = resolve_set(tmp.path(), &one_repo_set("main"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("zl-zpr-core"), "{error}");
    }

    #[test]
    fn resolve_set_non_git_directory_errors_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("zl-zpr-core")).unwrap();
        let error = resolve_set(tmp.path(), &one_repo_set("main"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("zl-zpr-core"), "{error}");
        assert!(error.contains("not a git repository"), "{error}");
    }

    /// `--tip` resolves `origin/<default_branch>` and ignores whatever the
    /// build set says: after the origin advances (fetched, not merged), tip is
    /// the origin's sha, not the checkout's.
    #[test]
    fn resolve_tip_uses_origin_default_branch_not_local_head() {
        let (tmp, workspace, seed_sha) = workspace_with_repo();
        // Advance the origin by one commit through a second clone, then fetch
        // in the workspace checkout without merging.
        let scratch = tmp.path().join("scratch2");
        let origin = tmp.path().join("origin.git");
        setup_git(
            tmp.path(),
            &[
                "clone",
                &origin.to_string_lossy(),
                &scratch.to_string_lossy(),
            ],
        );
        setup_git(&scratch, &["config", "user.name", "zpr-dev tests"]);
        setup_git(&scratch, &["config", "user.email", "tests@example.invalid"]);
        setup_git(&scratch, &["config", "commit.gpgsign", "false"]);
        std::fs::write(scratch.join("later.md"), "later\n").unwrap();
        setup_git(&scratch, &["add", "-A"]);
        setup_git(&scratch, &["commit", "-m", "later"]);
        setup_git(&scratch, &["push", "origin", "HEAD:main"]);
        let new_sha = setup_git(&scratch, &["rev-parse", "HEAD"]);
        let repo_dir = workspace.join("zl-zpr-core");
        setup_git(&repo_dir, &["fetch"]);

        let resolved = resolve_tip(&workspace, &[("zl-zpr-core", "main")]).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].reference, "origin/main");
        assert_eq!(resolved[0].sha, new_sha);
        assert_ne!(resolved[0].sha, seed_sha);
        // The checkout itself was not moved: resolution reads, never merges.
        assert_eq!(setup_git(&repo_dir, &["rev-parse", "HEAD"]), seed_sha);
    }
}
