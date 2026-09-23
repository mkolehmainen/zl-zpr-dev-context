//! The `build` command: build sets (spec-003). This stage (B1) implements the
//! build-set schema, default-manifest selection, ref resolution and the
//! `--dry-run` report; the gates, worktrees, builds and tiers it describes are
//! specified in `docs/specs/spec-003-build.md` and land in B2–B5.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::Manifest;

pub mod gates;
pub mod recipes;
pub mod tiers;

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
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    // The name becomes a filesystem path (`.zpr-build/<name>`, the emitted
    // manifest and tarball names), and `--force` removes that directory
    // wholesale — so a name like `..`, `a/b` or `/abs` could escape the build
    // root and put arbitrary directories in `remove_dir_all`'s path. Require
    // one plain path component: no separators, no `.`/`..`, not absolute.
    if set.name.contains(['/', '\\'])
        || set.name == "."
        || set.name == ".."
        || Path::new(&set.name).is_absolute()
    {
        bail!(
            "build set name {:?} is not a single safe path component; \
             it names the build directory, so it must contain no path \
             separators and must not be `.` or `..`",
            set.name
        );
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
    pub gates_only: bool,
    pub allow_pin_drift: bool,
    pub no_tarball: bool,
    /// `--prompt-for-sudo`: authorize one sudo password prompt before the
    /// run, so the netns tier can run without a NOPASSWD sudoers entry
    /// (zipline#70). Opt-in, and refused when stdin is not a terminal.
    pub prompt_for_sudo: bool,
    /// `--note <text>`, repeatable: operator sentences appended verbatim to
    /// the emitted manifest's `notes` after the generated ones (zipline#87).
    pub note: Vec<String>,
    /// `--clean`: remove the build directory and clear its worktree
    /// registrations, then exit — a mode, not a modifier (zipline#71). It
    /// resolves nothing and fetches nothing, so it works on a workspace too
    /// broken to resolve a build set. Conflicts with every build-shaping
    /// flag; `--build-dir` is allowed because it scopes the clean.
    pub clean: bool,
}

/// Creates the build directory `<build_dir>` with `logs/` and `dist/` inside.
/// A directory left over from a previous run is refused naming the path and
/// `--force` (approved decision on zipline#60: reuse of half-built state is
/// how silent staleness gets shipped); `force` removes it entirely and
/// recreates it fresh. `workspace` and `manifest_repos` locate the source
/// repositories, so a worktree retained by a failed run is unregistered, not
/// just deleted.
fn prepare_build_dir(
    dir: &Path,
    force: bool,
    workspace: &Path,
    manifest_repos: &[&str],
) -> Result<()> {
    if dir.exists() {
        if !force {
            bail!(
                "build directory {} already exists from a previous run; \
                 rerun with --force to remove and rebuild it",
                dir.display()
            );
        }
        unregister_worktrees(dir, workspace, manifest_repos);
        std::fs::remove_dir_all(dir)
            .map_err(|e| anyhow::anyhow!("cannot remove {}: {e}", dir.display()))?;
    }
    for sub in ["logs", "dist"] {
        std::fs::create_dir_all(dir.join(sub))
            .map_err(|e| anyhow::anyhow!("cannot create {}/{sub}: {e}", dir.display()))?;
    }
    Ok(())
}

/// Unregisters every build worktree under `<build_dir>/src` from its source
/// repository, then prunes every manifest repository. Used by `--force`
/// (via [`prepare_build_dir`]); the `--clean` path prunes *after* its
/// deletions instead — see [`run_clean`].
///
/// A failed run retains its worktrees under `src/` for debugging (see
/// `execute_build`). They must be *unregistered* from their source
/// repositories, not just deleted: a raw removal leaves each registration
/// behind, and the next `git worktree add` fails with git's "missing but
/// already registered worktree" — which made the documented --force recovery
/// unusable (Codex review on PR #8).
///
/// The `src/` walk is best-effort by construction — it derives names from the
/// filesystem, and the failure mode this function exists for is exactly the
/// filesystem being gone (zipline#71: `rm -rf` of the build directory). The
/// prune pass over the manifest list is what guarantees no registration
/// survives: prune drops exactly the registrations whose directory is
/// missing, and never touches a live worktree, so it is safe to run against
/// every repository unconditionally.
fn unregister_worktrees(build_dir: &Path, workspace: &Path, manifest_repos: &[&str]) {
    if let Ok(entries) = std::fs::read_dir(build_dir.join("src")) {
        for entry in entries.flatten() {
            let repo = workspace.join(entry.file_name());
            if !crate::git::is_repo(&repo) {
                continue;
            }
            // Best effort: a worktree that cannot be removed cleanly is
            // deleted with the build directory by the caller; the prune
            // below drops whatever registration is left pointing at a
            // missing path.
            let _ = crate::git::worktree_remove(&repo, &entry.path());
        }
    }
    // The walk above cannot see a worktree whose directory was already
    // deleted by hand, so the registrations are cleared from the repository
    // side: prune every manifest repository rather than trusting the
    // enumeration.
    prune_worktree_registrations(workspace, manifest_repos);
}

/// Runs `git worktree prune` in every manifest repository that exists in the
/// workspace. Prune drops exactly the registrations whose worktree directory
/// is missing and never touches a live worktree, so it is safe to run
/// unconditionally — but for the same reason it only helps *after* the
/// directories are gone. Callers that delete a build tree must call this
/// after the deletion, not before (Codex review on PR #16).
fn prune_worktree_registrations(workspace: &Path, manifest_repos: &[&str]) {
    for name in manifest_repos {
        let repo = workspace.join(name);
        if crate::git::is_repo(&repo) {
            let _ = crate::git::git(&repo, &["worktree", "prune"]);
        }
    }
}

/// Verifies that every expected binary exists in `dist/` and is executable
/// (task B3 step 4). All missing or non-executable names are collected into
/// one error, so a run with two broken recipes reports both, not the first.
fn verify_dist(dist: &Path, expected: &[&str]) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    for name in expected {
        let path = dist.join(name);
        if !path.is_file() {
            problems.push(format!("{name}: missing from {}", dist.display()));
            continue;
        }
        // The mode check: a staged file none of the executable bits reach is
        // a build product that cannot run — catching it here beats a cryptic
        // tier failure later.
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path)
                .map_err(|e| anyhow::anyhow!("cannot stat {}: {e}", path.display()))?
                .permissions(),
        );
        if mode & 0o111 == 0 {
            problems.push(format!("{name}: present but not executable"));
        }
    }
    if !problems.is_empty() {
        bail!("dist/ verification failed:\n  {}", problems.join("\n  "));
    }
    Ok(())
}

/// The `build` command (spec-003 §7). At this stage `--dry-run` resolves and
/// reports (B1), and `--gates-only` runs the three compatibility gates of §4
/// against the live checkouts (B2); worktrees, builds and tiers land with
/// B3-B5.
pub fn run(ctx: &crate::Ctx, args: &BuildArgs) -> Result<std::process::ExitCode> {
    // `--clean` is a mode, not a modifier (zipline#71): it runs before the
    // tier-selection parse and the `--gates-only` branch, resolves no refs,
    // fetches nothing, and runs no gates — it must work on a workspace too
    // broken to resolve a build set, which is when it is needed.
    if args.clean {
        return run_clean(ctx, args);
    }

    // `--repo` parses but its stage has not landed (spec-003 §7.1); saying so
    // beats silently ignoring it.
    if args.repo.is_some() && !ctx.quiet {
        println!("note: --repo parses but is inert until its stage lands");
    }

    // The tier selection is parsed up front so an unknown or unimplemented
    // name is a usage error (exit 2 through the Err path) before anything
    // touches the filesystem. `--test none` behaviour is unchanged from B3.
    let selection = tiers::Selection::parse(args.test.as_deref())?;

    if args.gates_only {
        return run_gates(ctx, args);
    }

    let manifest = crate::config::load(&ctx.context.join(crate::config::MANIFEST_FILE))?;

    // `--tip` needs no build set at all: the repository list is the
    // binary-producing set of spec-003 §5 and the default branches come from
    // workspace.yaml (spec-003 §2.3). A named set supplies both instead. The
    // tip branch synthesizes a set so the emitted-manifest path (§3) is one
    // code path — which is also how a --tip run is promoted to a named set.
    let (set, resolution, skipped) = if args.tip {
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
        let set = BuildSet {
            version: SUPPORTED_VERSION,
            name: "tip".to_string(),
            repositories: repos
                .iter()
                .map(|(name, branch)| (name.to_string(), format!("origin/{branch}")))
                .collect(),
            allow_pin_drift: Vec::new(),
        };
        let resolution = resolve_tip(&ctx.workspace, &repos);
        (set, resolution, skipped)
    } else {
        let path = match &args.manifest {
            Some(path) => path.clone(),
            None => default_manifest_path(&ctx.context)?,
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("cannot read build set {}: {e}", path.display()))?;
        let set = parse(&text)?;
        validate_against(&set, &manifest)?;
        let resolution = resolve_set(&ctx.workspace, &set);
        (set, resolution, vec![])
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

    if ctx.dry_run {
        // The manifest a real run would write: built here so the shape is
        // exercised end to end, printed under --verbose, never written.
        let mut emitted = emit(&set, &resolved, args.tip)?;
        // The preview carries what the run already knows: the request and
        // the operator's notes. The generated notes and tests_skipped need
        // tier results a dry run does not have (PR #22 Codex P2).
        emitted.resolved.tests_requested = selection.requested().to_string();
        emitted.resolved.notes = args.note.clone();
        report_dry_run(ctx, &set.name, &resolved, &skipped, args, &selection);
        if ctx.verbose && !ctx.quiet {
            println!();
            println!("emitted manifest (would be written on a real run):");
            print!("{}", emitted_yaml(&emitted)?);
        }
        return Ok(std::process::ExitCode::SUCCESS);
    }

    // -- the real build (task B3) ---------------------------------------------
    for name in &skipped {
        if !ctx.quiet {
            println!("{name}: not in workspace.yaml, skipped");
        }
    }

    let build_dir = args
        .build_dir
        .clone()
        .unwrap_or_else(|| ctx.workspace.join(".zpr-build").join(&set.name));
    // The manifest's repository names, for the registration-side prune in
    // unregister_worktrees: the filesystem walk alone cannot see worktrees
    // whose directories were deleted by hand (zipline#71).
    let manifest_repo_names: Vec<&str> = manifest
        .repositories
        .iter()
        .map(|repo| repo.name.as_str())
        .collect();
    prepare_build_dir(&build_dir, ctx.force, &ctx.workspace, &manifest_repo_names)?;

    // What the emitted manifest records as its own provenance (spec-003 §3).
    let manifest_path = if args.tip {
        "--tip".to_string()
    } else {
        match &args.manifest {
            Some(path) => path.display().to_string(),
            None => default_manifest_path(&ctx.context)?.display().to_string(),
        }
    };
    let context_sha = crate::git::head_short(&ctx.context).unwrap_or_default();

    // The end-to-end tiers' prerequisite probes (task B5), resolved before
    // any building: an explicitly requested tier that cannot run is a
    // gate-style finding — this machine cannot run what was asked — so it
    // exits 1 before fifteen minutes of cargo, not after (approved Q1 on
    // zipline#62). A default-selected tier's failure becomes its skip reason.
    let mut netns_skip: Option<String> = None;
    let mut docker_skip: Option<String> = None;
    let mut valkey: Option<PathBuf> = None;
    let mut sudo_refresher: Option<tiers::SudoRefresher> = None;
    let mut netns_sudo: Option<tiers::SudoProvenance> = None;
    if selection.contains("netns") || selection.contains("docker") {
        let probes =
            tiers::Probes::gather(tiers::should_prime_sudo(args.prompt_for_sudo, &selection));
        // --prompt-for-sudo without a terminal on stdin is refused up front
        // (zipline#70 Step 2): under tty_tickets the prompt would hang or
        // cache against the wrong ticket, so the honest answer is an error
        // naming the requirement — never a hang, never a silent skip.
        if probes.sudo_prime == Some(tiers::PrimeOutcome::NoTty) {
            eprintln!(
                "error: --prompt-for-sudo needs a terminal on stdin \
                 (sudo's credential cache is keyed to the controlling tty); \
                 run interactively, or drop the flag"
            );
            return Ok(std::process::ExitCode::from(1));
        }
        valkey = probes.valkey_server.clone();
        // What the manifest will record for a netns tier that runs
        // (zipline#70 Step 6): primed credentials and a NOPASSWD host are
        // different provenances and are never conflated. None when sudo
        // does not work at all — the tier then skips or errors anyway.
        netns_sudo = match probes.sudo_prime {
            Some(tiers::PrimeOutcome::Primed) => Some(tiers::SudoProvenance::Primed),
            _ if probes.passwordless_sudo => Some(tiers::SudoProvenance::Nopasswd),
            _ => None,
        };
        // The refresher starts at the prime and only when one actually ran
        // (zipline#70 Step 4): on a NOPASSWD host there is no credential to
        // keep alive. It is handed to execute_build, which stops it when
        // the netns tier returns; Drop covers every earlier exit.
        if probes.sudo_prime == Some(tiers::PrimeOutcome::Primed) && selection.contains("netns") {
            sudo_refresher = Some(tiers::SudoRefresher::start(
                std::sync::Arc::new(tiers::LiveSudo),
                tiers::SUDO_REFRESH_INTERVAL,
            ));
        }
        for (tier, gate, skip) in [
            ("netns", tiers::netns_gate(&probes), &mut netns_skip),
            ("docker", tiers::docker_gate(&probes), &mut docker_skip),
        ] {
            if !selection.contains(tier) {
                continue;
            }
            match tiers::check_gate(tier, gate, selection.is_explicit(tier)) {
                Ok(reason) => *skip = reason,
                Err(error) => {
                    eprintln!("error: {error:#}");
                    return Ok(std::process::ExitCode::from(1));
                }
            }
        }
    }

    let ok = execute_build(&BuildInputs {
        set: &set,
        resolved: &resolved,
        workspace: &ctx.workspace,
        manifest: &manifest,
        build_dir: &build_dir,
        recipes: recipes::RECIPES,
        tip: args.tip,
        keep: args.keep,
        quiet: ctx.quiet,
        no_tarball: args.no_tarball,
        downgrade_pin_drift: args.allow_pin_drift,
        manifest_path,
        context_sha,
        selection: &selection,
        netns_skip,
        docker_skip,
        valkey,
        verbose: ctx.verbose,
        sudo_refresher: std::cell::Cell::new(sudo_refresher),
        netns_sudo,
        notes: &args.note,
    })?;
    if !ctx.quiet {
        println!("dist: {}", build_dir.join("dist").display());
        // A tier that did not run is stated, never silently absent — a green
        // silence must not overstate coverage (spec-003 §6).
        if selection.is_empty() {
            println!("tests: none run (--test none)");
        }
    }
    Ok(if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::from(1)
    })
}

/// The `build --clean` path (zipline#71): removes the build directory and
/// clears its worktree registrations, then exits. With `--build-dir` it
/// cleans that directory; without it, the whole `<workspace>/.zpr-build`
/// tree — cleaning is not per-set, because there is nothing to name a set
/// with. Then `git worktree prune` runs in every workspace-manifest
/// repository, which is what catches registrations whose directories a
/// person already deleted. No ref resolution, no fetch, no gates, no build;
/// the only manifest read is the repository list. Exit 0 even when there was
/// nothing to clean — cleaning an already-clean workspace is a success, not
/// an error. Under `--dry-run` the same report prints and nothing is removed
/// (spec-003 §7: a dry run writes nothing).
fn run_clean(ctx: &crate::Ctx, args: &BuildArgs) -> Result<std::process::ExitCode> {
    let manifest = crate::config::load(&ctx.context.join(crate::config::MANIFEST_FILE))?;
    let manifest_repo_names: Vec<&str> = manifest
        .repositories
        .iter()
        .map(|repo| repo.name.as_str())
        .collect();

    // The directories to remove: the named one, or every entry of the
    // default `.zpr-build` tree (one subdirectory per set name).
    let targets: Vec<PathBuf> = match &args.build_dir {
        Some(dir) => {
            if dir.exists() {
                vec![dir.clone()]
            } else {
                vec![]
            }
        }
        None => {
            let root = ctx.workspace.join(".zpr-build");
            if root.exists() { vec![root] } else { vec![] }
        }
    };

    if ctx.dry_run {
        if !ctx.quiet {
            if targets.is_empty() {
                println!("clean (dry-run): nothing to clean");
            } else {
                for dir in &targets {
                    println!("clean (dry-run): would remove {}", dir.display());
                }
            }
            println!(
                "clean (dry-run): would prune worktree registrations in {} \
                 workspace repositories",
                manifest_repo_names.len()
            );
            println!("dry-run: nothing was removed, and nothing was fetched");
        }
        return Ok(std::process::ExitCode::SUCCESS);
    }

    for dir in &targets {
        // Remove the tree first, then prune: `git worktree prune` drops
        // exactly the registrations whose directory is missing, so a prune
        // that runs before the deletion correctly retains every still-live
        // registration and clears nothing (Codex review on PR #16). With the
        // named directory this also sidesteps rooting the walk wrong: the
        // default target is the `.zpr-build` root whose worktrees live one
        // level down at `<set>/src/<repo>`, not at `src/<repo>`.
        std::fs::remove_dir_all(dir)
            .map_err(|e| anyhow::anyhow!("cannot remove {}: {e}", dir.display()))?;
        if !ctx.quiet {
            println!("clean: removed {}", dir.display());
        }
    }
    if targets.is_empty() && !ctx.quiet {
        println!("clean: nothing to clean");
    }
    // The manifest-wide prune runs after every deletion, and also when there
    // was no directory left to delete — the zipline#71 case is exactly
    // `rm -rf` ahead of the tool, which leaves registrations with no
    // directory behind them.
    prune_worktree_registrations(&ctx.workspace, &manifest_repo_names);
    if !ctx.quiet {
        println!(
            "clean: pruned worktree registrations in {} workspace repositories",
            manifest_repo_names.len()
        );
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// The `build --gates-only` path (spec-003 §4, stage B2): runs the three
/// compatibility gates against the live checkouts and prints one
/// `zpr-dev validate`-style report. Read-only by construction: it parses
/// manifests and lists tags, and never fetches or builds. The real build path
/// runs the same suite against its worktrees instead (spec-003 §4).
fn run_gates(ctx: &crate::Ctx, args: &BuildArgs) -> Result<std::process::ExitCode> {
    let manifest = crate::config::load(&ctx.context.join(crate::config::MANIFEST_FILE))?;

    // The repositories whose manifests the gates scan, and the tolerated
    // drift entries, come from the build set — synthesized under `--tip`
    // exactly as the dry-run path does (spec-003 §2.3).
    let (scan_names, drift, set_name) = if args.tip {
        let names: Vec<String> = BUILD_ORDER
            .iter()
            .filter(|wanted| manifest.repo(wanted).is_some())
            .map(|name| name.to_string())
            .collect();
        if names.is_empty() {
            bail!("--tip found none of the build-set repositories in workspace.yaml");
        }
        (names, Vec::new(), "tip".to_string())
    } else {
        let path = match &args.manifest {
            Some(path) => path.clone(),
            None => default_manifest_path(&ctx.context)?,
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("cannot read build set {}: {e}", path.display()))?;
        let set = parse(&text)?;
        validate_against(&set, &manifest)?;
        (
            set.repositories.keys().cloned().collect(),
            set.allow_pin_drift.clone(),
            set.name.clone(),
        )
    };

    // Gates-only scans the live checkouts: each scanned name maps to its
    // workspace directory.
    let mut scan: Vec<(String, PathBuf)> = scan_names
        .iter()
        .map(|name| (name.clone(), ctx.workspace.join(name)))
        .collect();
    add_derived_scan(&manifest, &ctx.workspace, &mut scan);

    let outcome = collect_gate_findings(&scan, &drift, args.allow_pin_drift, &ctx.workspace)?;
    let errors = print_findings(ctx.quiet, &set_name, &outcome.findings);
    Ok(if errors > 0 {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    })
}

/// Appends `zl-zpr-common`'s **live checkout** to a gate scan list — the
/// `--gates-only` path only. `zl-zpr-common` never appears in a build set
/// (spec-003 §2.1: its version is *derived* from what the consumers agree
/// on), but its own manifest participates in coherence — the real `rcu`
/// divergence is between `zl-zpr-common` and `adapter/ph`, and a crate
/// reached both directly and through `zpr` ends up in one binary twice.
/// Scanned whenever the workspace declares it. Live is correct *here*
/// because `--gates-only` is read-only by construction and scans the live
/// checkouts throughout; a real build gates the pinned revision instead —
/// see [`add_derived_scan_pinned`]. Nothing else is added: the
/// `zl-zpr-utils` checkout is deliberately not scanned, because no build in
/// this workspace consumes it (workspace.yaml's own note).
fn add_derived_scan(manifest: &Manifest, workspace: &Path, scan: &mut Vec<(String, PathBuf)>) {
    const DERIVED: &str = "zl-zpr-common";
    if manifest.repo(DERIVED).is_some() && !scan.iter().any(|(name, _)| name == DERIVED) {
        scan.push((DERIVED.to_string(), workspace.join(DERIVED)));
    }
}

/// Appends `zl-zpr-common` to a real build's gate scan **at the revision the
/// consumers pin** — what cargo will actually compile — via a throwaway
/// detached worktree under `src/`, returned for the caller to remove once
/// the gates have read it. Scanning the live checkout instead let gate 1
/// reject a compatible set (or miss a real disagreement) whenever the
/// checkout sat on a different commit than the pinned tag, and made the
/// emitted `pins:` block vary with unrelated local state instead of
/// round-tripping from the emitted manifest (Codex review on PR #8).
///
/// The pinned revision is derived from the consumer worktrees already in
/// `scan`: the single reference every `zl-zpr-common` git dependency agrees
/// on. When there is no such single reference — no consumer pins it, the
/// pins disagree (gate 1's error either way), the checkout is absent, or
/// the ref does not resolve locally — the live checkout is scanned as
/// before, which never *hides* a finding: it is the pre-fix behavior.
fn add_derived_scan_pinned(
    manifest: &Manifest,
    workspace: &Path,
    src: &Path,
    scan: &mut Vec<(String, PathBuf)>,
) -> Result<Option<PathBuf>> {
    const DERIVED: &str = "zl-zpr-common";
    if manifest.repo(DERIVED).is_none() || scan.iter().any(|(name, _)| name == DERIVED) {
        return Ok(None);
    }
    let repo = workspace.join(DERIVED);

    // The consumers' agreed reference for the derived repository, when there
    // is exactly one. URL matching is by repository name, the same join
    // point gate 2 uses (pins are https, workspace.yaml may be ssh).
    let names_derived = |url: &str| {
        url.rsplit('/')
            .next()
            .map(|last| last.strip_suffix(".git").unwrap_or(last) == DERIVED)
            .unwrap_or(false)
    };
    let (pins, _) = extract_scan_pins(scan)?;
    let mut refs: Vec<&str> = pins
        .iter()
        .filter(|pin| names_derived(&pin.url))
        .map(|pin| pin.reference.as_str())
        .collect();
    refs.sort_unstable();
    refs.dedup();

    if let [reference] = refs[..]
        && crate::git::is_repo(&repo)
        && let Ok(sha) = crate::git::rev_parse(&repo, reference)
    {
        let dest = src.join(DERIVED);
        crate::git::worktree_add(&repo, &dest, &sha)?;
        scan.push((DERIVED.to_string(), dest.clone()));
        return Ok(Some(dest));
    }

    // Fallback: the live checkout, exactly as --gates-only scans it.
    scan.push((DERIVED.to_string(), repo));
    Ok(None)
}

/// What one gate run produced: the findings for the report, plus the data the
/// emitted manifest records (spec-003 §3) — the extracted pins and the two
/// gate-3 versions, when they parsed.
struct GateOutcome {
    findings: Vec<gates::Finding>,
    pins: Vec<gates::PinOccurrence>,
    /// `[package].version` from the compiler's `Cargo.toml`, as `x.y.z`.
    zplc_version: Option<String>,
    /// `POLICY_MIN_COMPILER_*` from the visa service's `vs/src/config.rs`.
    vs_policy_min_compiler: Option<String>,
    /// How many gate-1 disagreements `--allow-pin-drift` turned from errors
    /// into warnings — zero when the pins agreed, whatever the flag said.
    /// The manifest's pin-drift note keys off this, not the flag, so it
    /// never claims a downgrade that did not happen (PR #22 Codex P2).
    pin_drift_downgraded: usize,
}

/// Extracts every captured pin from `scan` — pairs of display name and the
/// directory to read manifests from — reading each root `Cargo.toml` and its
/// literal workspace members. Non-fatal problems (no `Cargo.toml`, an
/// unreadable member) come back as findings beside the pins, so gate
/// orchestration and the derived-revision lookup share one reader.
fn extract_scan_pins(
    scan: &[(String, PathBuf)],
) -> Result<(Vec<gates::PinOccurrence>, Vec<gates::Finding>)> {
    let mut findings: Vec<gates::Finding> = Vec::new();
    let mut pins: Vec<gates::PinOccurrence> = Vec::new();
    for (name, dir) in scan {
        let root_path = dir.join("Cargo.toml");
        let Ok(root_text) = std::fs::read_to_string(&root_path) else {
            // A Go or docs repository has no Cargo.toml; that is a fact to
            // state, not a failure.
            findings.push(gates::Finding::new(
                gates::Severity::Info,
                format!("{name}: no Cargo.toml, no pins to check"),
            ));
            continue;
        };
        let root = gates::ManifestSource {
            path: format!("{name}/Cargo.toml"),
            text: root_text,
        };

        // Workspace members, read from the root manifest's literal member
        // list. The ZPR repositories use literal paths, not globs.
        let mut members: Vec<gates::ManifestSource> = Vec::new();
        if let Ok(table) = root.text.parse::<toml::Table>() {
            let listed = table
                .get("workspace")
                .and_then(toml::Value::as_table)
                .and_then(|ws| ws.get("members"))
                .and_then(toml::Value::as_array);
            for member in listed.into_iter().flatten() {
                let Some(member) = member.as_str() else {
                    continue;
                };
                let display = format!("{name}/{member}/Cargo.toml");
                let path = dir.join(member).join("Cargo.toml");
                match std::fs::read_to_string(&path) {
                    Ok(text) => members.push(gates::ManifestSource {
                        path: display,
                        text,
                    }),
                    // A listed member the gate cannot read is a member the
                    // gate cannot check: skipping it silently would let the
                    // set pass with that member's pins unexamined, so it is
                    // an error finding, not an omission (spec-003 §4.1).
                    Err(error) => findings.push(gates::Finding::new(
                        gates::Severity::Error,
                        format!("cannot read workspace member manifest {display}: {error}"),
                    )),
                }
            }
        }
        pins.extend(gates::extract_pins(&root, &members)?);
    }
    Ok((pins, findings))
}

/// Runs the three gates over `scan` — pairs of display name and the directory
/// to read manifests from, which are live checkouts under `--gates-only` and
/// detached worktrees in a real build. Gate 2's tag listing always reads the
/// live `workspace` checkouts: tags are repository-wide, not ref-specific.
fn collect_gate_findings(
    scan: &[(String, PathBuf)],
    drift: &[PinDrift],
    downgrade: bool,
    workspace: &Path,
) -> Result<GateOutcome> {
    // -- pin extraction over every scanned directory (gate 1 input) ----------
    let (pins, mut findings) = extract_scan_pins(scan)?;

    // -- gate 1: agreement ---------------------------------------------------
    let agreement = gates::gate_pin_agreement(&pins, drift, downgrade);
    // Gate 1 emits a warning only for a disagreement the flag downgraded
    // (agreement is OK, an allowed drift is INFO, and without the flag a
    // disagreement is an ERROR), so the warning count is the downgrade count.
    let pin_drift_downgraded = agreement
        .iter()
        .filter(|f| f.severity == gates::Severity::Warn)
        .count();
    findings.extend(agreement);

    // -- gate 1 lock scan: post-resolution dual versions (zipline#69) --------
    // Manifest pins cannot see a pin made inside a tagged git dependency, so
    // each scanned repository's resolved Cargo.lock is checked for any
    // ZPR-family crate at two or more versions. A repository without a lock
    // (a Go or docs repository, or a member directory) is a fact to state,
    // not a failure — the same tolerance the manifest reader shows.
    let mut locks: Vec<gates::LockSource> = Vec::new();
    for (name, dir) in scan {
        let lock_path = dir.join("Cargo.lock");
        match std::fs::read_to_string(&lock_path) {
            Ok(text) => locks.push(gates::LockSource {
                path: format!("{name}/Cargo.lock"),
                text,
            }),
            Err(_) => findings.push(gates::Finding::new(
                gates::Severity::Info,
                format!("{name}: no Cargo.lock, no resolution to scan"),
            )),
        }
    }
    findings.extend(gates::gate_lock_dual_versions(&locks));

    // -- gate 2: freshness, against the workspace checkouts' tags -----------
    findings.extend(gates::gate_freshness(&pins, |url| {
        // The pinned repository's checkout, located by the URL's repository
        // name. Pins use https URLs while workspace.yaml uses ssh, so the
        // name is the join point.
        let name = url
            .rsplit('/')
            .next()
            .map(|last| last.strip_suffix(".git").unwrap_or(last))?;
        let dir = workspace.join(name);
        if !crate::git::is_repo(&dir) {
            return None;
        }
        crate::git::tag_list(&dir).ok()
    }));

    // -- gate 3: zplc vs the visa service's POLICY_MIN_COMPILER --------------
    // Reads from the scanned directories (worktrees in a real build) when the
    // set names those repositories, falling back to the live checkouts so
    // gates-only coverage never narrows. A missing file is reported rather
    // than silently skipped (spec-003 §4.3). An unreadable value in a present
    // file is an error inside the parsers.
    let scan_dir = |wanted: &str| -> PathBuf {
        scan.iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, dir)| dir.clone())
            .unwrap_or_else(|| workspace.join(wanted))
    };
    let vs_config = scan_dir("zl-zpr-visaservice")
        .join("vs")
        .join("src")
        .join("config.rs");
    let zplc_manifest = scan_dir("zl-zpr-compiler").join("Cargo.toml");
    let mut zplc_version: Option<String> = None;
    let mut vs_policy_min_compiler: Option<String> = None;
    match (
        std::fs::read_to_string(&vs_config),
        std::fs::read_to_string(&zplc_manifest),
    ) {
        (Ok(config_text), Ok(manifest_text)) => {
            // A present but unreadable value is an error *finding*, not an
            // abort: aborting here would discard gate 1's and gate 2's
            // findings and exit with the command-error code 2, when the
            // report contract is an [ERROR] line and the gate-failure
            // code 1 (spec-003 §4).
            let minimum =
                gates::policy_min_compiler(&config_text, "zl-zpr-visaservice/vs/src/config.rs");
            let zplc = gates::package_version(&manifest_text, "zl-zpr-compiler/Cargo.toml");
            match (minimum, zplc) {
                (Ok(minimum), Ok(zplc)) => {
                    let display = |(a, b, c): (u64, u64, u64)| format!("{a}.{b}.{c}");
                    zplc_version = Some(display(zplc));
                    vs_policy_min_compiler = Some(display(minimum));
                    findings.extend(gates::gate_compiler_version(
                        zplc,
                        "zl-zpr-compiler/Cargo.toml",
                        minimum,
                        "zl-zpr-visaservice/vs/src/config.rs",
                    ));
                }
                (minimum, zplc) => {
                    for error in [minimum.err(), zplc.err()].into_iter().flatten() {
                        findings.push(gates::Finding::new(
                            gates::Severity::Error,
                            format!("gate 3 (zplc vs POLICY_MIN_COMPILER): {error:#}"),
                        ));
                    }
                }
            }
        }
        (vs, zplc) => {
            let mut missing: Vec<&str> = Vec::new();
            if vs.is_err() {
                missing.push("zl-zpr-visaservice/vs/src/config.rs");
            }
            if zplc.is_err() {
                missing.push("zl-zpr-compiler/Cargo.toml");
            }
            findings.push(gates::Finding::new(
                gates::Severity::Info,
                format!(
                    "gate 3 (zplc vs POLICY_MIN_COMPILER) skipped: {} not in this workspace",
                    missing.join(", ")
                ),
            ));
        }
    }

    Ok(GateOutcome {
        findings,
        pins,
        zplc_version,
        vs_policy_min_compiler,
        pin_drift_downgraded,
    })
}

/// Prints the findings in `zpr-dev validate` style (spec-003 §4) and returns
/// the error count, which decides the exit code.
fn print_findings(quiet: bool, set_name: &str, findings: &[gates::Finding]) -> usize {
    let mut errors = 0usize;
    let mut warnings = 0usize;
    if !quiet {
        println!("compatibility gates: build set {set_name}");
        println!();
    }
    for finding in findings {
        let tag = match finding.severity {
            gates::Severity::Ok => "[OK]",
            gates::Severity::Info => "[INFO]",
            gates::Severity::Warn => {
                warnings += 1;
                "[WARN]"
            }
            gates::Severity::Error => {
                errors += 1;
                "[ERROR]"
            }
        };
        if quiet {
            continue;
        }
        let mut lines = finding.message.lines();
        if let Some(first) = lines.next() {
            println!("{tag:<7} {first}");
        }
        for line in lines {
            println!("        {line}");
        }
    }
    if !quiet {
        println!();
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        if errors > 0 {
            println!(
                "Gates failed with {errors} error{} and {warnings} warning{}.",
                plural(errors),
                plural(warnings)
            );
        } else {
            println!("Gates passed with {warnings} warning{}.", plural(warnings));
        }
    }
    errors
}

/// Prints the §7.2 dry-run report: resolved shas, planned build order, planned
/// tiers with probe results, and the dist/ target. Read-only by construction —
/// the only processes it may spawn are the read-only tier probes. In
/// particular it never prompts: `--prompt-for-sudo` only changes what the
/// netns line *says* the real run would do (zipline#70 Step 5).
fn report_dry_run(
    ctx: &crate::Ctx,
    name: &str,
    resolved: &[Resolved],
    skipped: &[&str],
    args: &BuildArgs,
    selection: &tiers::Selection,
) {
    if ctx.quiet {
        return;
    }
    let build_dir = args.build_dir.as_deref();
    let prompt_for_sudo = args.prompt_for_sudo;
    let notes = &args.note;
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
    // gather(false): the dry-run report never prompts (zipline#70 Step 5) —
    // it probes read-only and *describes* what a real run with the flag
    // would do, via `netns_dry_run_text` below.
    let probes = tiers::Probes::gather(false);
    for (tier, gate) in [
        ("unit", tiers::TierGate::Run),
        ("netns", tiers::netns_gate(&probes)),
        ("docker", tiers::docker_gate(&probes)),
    ] {
        let request = if selection.contains(tier) {
            "would run".to_string()
        } else {
            format!("not requested (--test {tier})")
        };
        // The netns line accounts for --prompt-for-sudo; the gate result is
        // still computed above so the two never disagree on the probes. The
        // flag is routed through the same should_prime_sudo gate as the real
        // run, so a selection that will not prime is never described as
        // prompting (PR #15 review finding).
        if tier == "netns" {
            let text = tiers::netns_dry_run_text(
                &probes,
                tiers::should_prime_sudo(prompt_for_sudo, selection),
            );
            println!("  {tier:7} {request}; {text}");
            continue;
        }
        match gate {
            tiers::TierGate::Run => println!("  {tier:7} {request}; prerequisites present"),
            tiers::TierGate::Skip(reason) => println!("  {tier:7} {request}; {reason}"),
        }
    }
    println!();

    let build_dir = build_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ctx.workspace.join(".zpr-build").join(name));
    println!("dist: {}", build_dir.join("dist").display());
    // The operator's --note lines as the manifest would carry them; the
    // generated notes need tier results, which a dry run does not have.
    for note in notes {
        println!("note: {note}");
    }
    println!("dry-run: nothing was created, and nothing was fetched");
}

/// The prerequisite probes moved to `tiers::Probes` / `tiers::netns_gate` /
/// `tiers::docker_gate` in B5, where the runners consume them; the dry-run
/// report above reuses them so the two can never disagree.

// ---------------------------------------------------------------------------
// Emitted manifest (spec-003 §3)
// ---------------------------------------------------------------------------

/// The emitted manifest: the same schema as the input with every ref replaced
/// by its resolved sha, plus the diagnostic `resolved:` block a later read
/// ignores (spec-003 §3). B1 fixes the shape; B3–B5 populate the diagnostics.
#[derive(Debug, Serialize)]
pub struct EmittedManifest {
    pub version: u32,
    pub name: String,
    /// Repository name → 40-character sha, always.
    pub repositories: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_pin_drift: Vec<PinDrift>,
    /// Diagnostic only: ignored when the file is re-read as a build set,
    /// because `BuildSet` tolerates unknown keys by design (spec-003 §2.1).
    pub resolved: ResolvedBlock,
}

/// The `resolved:` diagnostic block. Every field a later stage populates is
/// optional or defaulted, so B1 can emit the shape with honest emptiness.
#[derive(Debug, Default, Serialize)]
pub struct ResolvedBlock {
    pub built_at: String,
    pub built_from: BuiltFrom,
    /// Host os/arch and toolchain versions that produced the binaries
    /// (spec-003 §3): recorded because reproducible *inputs* are guaranteed
    /// and byte-identical binaries are not — the toolchain is what varied.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub host: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<Pin>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub versions: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub binaries: Vec<Binary>,
    /// The literal `--test` value (`default` when absent): what was asked
    /// for, so a set gated on `unit` alone says so (zipline#87).
    pub tests_requested: String,
    /// Every known tier that did not execute, with the reason — whether it
    /// was left out of the selection, probe-skipped, or never reached
    /// because the build failed. Absent means every tier ran (zipline#87).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tests_skipped: BTreeMap<String, String>,
    /// Plain sentences naming each shortcoming of the run, generated first
    /// and the operator's `--note` text last. Absent means a clean, full
    /// run. This is the block a human reads first (zipline#87).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tiers: BTreeMap<String, Tier>,
}

/// What produced this manifest: the input manifest path, the context sha, and
/// whether `--tip` overrode the refs.
#[derive(Debug, Default, Serialize)]
pub struct BuiltFrom {
    pub manifest: String,
    pub context: String,
    pub tip: bool,
}

/// One agreed pin, recomputed by gate 1 for the emitted manifest (spec-003
/// §3): what cargo actually compiled against.
#[derive(Debug, Clone, Serialize)]
pub struct Pin {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub url: String,
    pub tag: String,
    pub pinned_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_available: Option<String>,
}

/// One staged binary's identity, recorded by B3 (spec-003 §3). The digest is
/// for *comparison* between builds, never a reproducibility claim.
#[derive(Debug, Serialize)]
pub struct Binary {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
    pub from: String,
}

/// One tier's outcome. A skipped tier always carries its reason. The
/// `repos` breakdown records each repository's result (approved Q3 on
/// zipline#61), skip-serialized when empty so the #59/#60 shape is
/// unchanged for tiers that carry no breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct Tier {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Repository → `passed`, `failed at ...`, or `skipped: ...`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub repos: BTreeMap<String, String>,
    /// netns only: how sudo was satisfied — `nopasswd` or `primed`
    /// (zipline#70 Step 6). Absent for skipped tiers and for every other
    /// tier; the two provenances are never conflated (issue constraint).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sudo: Option<tiers::SudoProvenance>,
}

impl Tier {
    /// The manifest record of one executed tier: overall status from the
    /// sweep (`failed` when any repository failed; skips do not fail it)
    /// and the per-repository breakdown, verbatim.
    pub fn from_outcome(outcome: &tiers::TierOutcome) -> Tier {
        Tier {
            status: if outcome.passed { "passed" } else { "failed" }.to_string(),
            reason: None,
            repos: outcome.repos.clone(),
            sudo: None,
        }
    }

    /// Attaches the netns tier's sudo provenance (zipline#70 Step 6).
    /// Builder-style so the docker/unit call sites stay untouched — only
    /// the netns run path claims one.
    pub fn with_sudo(mut self, sudo: tiers::SudoProvenance) -> Tier {
        self.sudo = Some(sudo);
        self
    }

    /// The manifest record of a tier that did not run, with why: a failed
    /// prerequisite probe, or a repository the set does not include. Always
    /// recorded — a skipped tier must be visible in the manifest and never
    /// presented as coverage (spec-003 §6; master plan B5 constraint).
    pub fn skipped(reason: &str) -> Tier {
        Tier {
            status: "skipped".to_string(),
            reason: Some(reason.to_string()),
            repos: BTreeMap::new(),
            sudo: None,
        }
    }
}

/// What the run did *not* cover (zipline#87): the `tests_skipped` map and the
/// `notes` list of the emitted manifest's `resolved:` block.
#[derive(Debug, Default, PartialEq)]
pub struct Coverage {
    /// Tier → reason, for every known tier that did not execute.
    pub skipped: BTreeMap<String, String>,
    /// Human sentences, generated in tier order then operator text verbatim.
    pub notes: Vec<String>,
}

/// Summarizes a run's coverage gaps for the emitted manifest (zipline#87).
/// Pure: it reads the selection, the tier results as they will be written,
/// whether the build failed before any tier could run, whether gate 1
/// actually downgraded a disagreement under `--allow-pin-drift` (not merely
/// whether the flag was passed), and the operator's `--note` text. A known tier is *skipped*
/// when it has no result (not selected, or build failed) or a `skipped`
/// result (probe or repository-missing skip, whose reason is reused so the
/// two records can never disagree). A tier that ran and failed is not
/// skipped — it ran — but it is a shortcoming the notes name.
pub fn coverage_summary(
    selection: &tiers::Selection,
    tier_results: &BTreeMap<String, Tier>,
    build_failed: bool,
    pin_drift_downgraded: bool,
    operator_notes: &[String],
) -> Coverage {
    let requested = selection.requested();
    let mut coverage = Coverage::default();

    // Not-run tiers first, in run order, so the notes read unit → netns →
    // docker whatever the map's own ordering.
    let mut not_run: Vec<(&str, String)> = Vec::new();
    for tier in tiers::known() {
        let reason = match tier_results.get(*tier) {
            Some(result) if result.status == "skipped" => result
                .reason
                .clone()
                .unwrap_or_else(|| "skipped".to_string()),
            Some(_) => continue, // ran: passed or failed
            None if !selection.contains(tier) => format!("not selected (--test {requested})"),
            None if build_failed => "not run: build failed".to_string(),
            // Selected, build succeeded, yet no result: the run stopped
            // before this tier for a reason the caller did not record.
            None => "not run".to_string(),
        };
        coverage.skipped.insert(tier.to_string(), reason.clone());
        not_run.push((tier, reason));
    }

    // `--test none` is one fact, not three enumerated absences.
    if selection.is_empty() {
        coverage
            .notes
            .push(format!("no tests ran (--test {requested})"));
    } else {
        for (tier, reason) in &not_run {
            coverage
                .notes
                .push(format!("{} did NOT run: {reason}", tiers::label(tier)));
        }
    }

    for tier in tiers::known() {
        if tier_results
            .get(*tier)
            .is_some_and(|result| result.status == "failed")
        {
            coverage.notes.push(format!(
                "{} FAILED; see tiers.{tier}.repos",
                tiers::label(tier)
            ));
        }
    }

    if pin_drift_downgraded {
        coverage.notes.push(
            "gate 1 pin disagreements were downgraded to warnings (--allow-pin-drift)".to_string(),
        );
    }

    coverage.notes.extend(operator_notes.iter().cloned());
    coverage
}

/// Builds the emitted manifest for a resolution (spec-003 §3): the input set's
/// name and drift entries with every ref replaced by its sha, plus the
/// `resolved:` block shape that B3–B5 fill in — present but honestly empty at
/// this stage, so nothing pretends a build or a tier ran.
pub fn emit(set: &BuildSet, resolved: &[Resolved], tip: bool) -> Result<EmittedManifest> {
    let repositories: BTreeMap<String, String> = resolved
        .iter()
        .map(|entry| (entry.repo.clone(), entry.sha.clone()))
        .collect();
    Ok(EmittedManifest {
        version: SUPPORTED_VERSION,
        name: set.name.clone(),
        repositories,
        allow_pin_drift: set.allow_pin_drift.clone(),
        resolved: ResolvedBlock {
            built_at: utc_now(),
            built_from: BuiltFrom {
                manifest: String::new(),
                context: String::new(),
                tip,
            },
            ..ResolvedBlock::default()
        },
    })
}

/// Serializes the emitted manifest as the YAML that lands in
/// `dist/zpr-set-<name>.yaml`.
pub fn emitted_yaml(manifest: &EmittedManifest) -> Result<String> {
    Ok(serde_yaml_ng::to_string(manifest)?)
}

// ---------------------------------------------------------------------------
// The real build (spec-003 §5, task B3)
// ---------------------------------------------------------------------------

/// Everything `execute_build` needs, gathered by `run()`: the resolved set,
/// where to build, which recipes to use (the fixture recipes in tests, the
/// contract-4 table in production), and what the emitted manifest should
/// record about its own provenance.
struct BuildInputs<'a> {
    set: &'a BuildSet,
    resolved: &'a [Resolved],
    workspace: &'a Path,
    /// The workspace manifest, for the derived `zl-zpr-common` gate scan.
    manifest: &'a Manifest,
    build_dir: &'a Path,
    recipes: &'a [recipes::Recipe],
    tip: bool,
    keep: bool,
    quiet: bool,
    no_tarball: bool,
    /// The `--allow-pin-drift` flag: gate 1 disagreements warn, not fail.
    downgrade_pin_drift: bool,
    /// What `built_from.manifest` records: the input path, or `--tip`.
    manifest_path: String,
    /// The context checkout's short sha, for `built_from.context`.
    context_sha: String,
    /// Which test tiers to run after a successful build (task B4).
    selection: &'a tiers::Selection,
    /// When set, the netns tier's prerequisite probe failed and the tier is
    /// recorded `skipped` with this reason (task B5; approved Q1 on
    /// zipline#62 — an *explicit* probe failure errors in `run()` before
    /// this struct is built, so a reason here is always a plain skip).
    netns_skip: Option<String>,
    /// Same, for the docker tier.
    docker_skip: Option<String>,
    /// Where the netns probe found `valkey-server`; `None` whenever the
    /// netns tier is skipped or unselected.
    valkey: Option<PathBuf>,
    /// `--verbose`: the netns scripts get `ZPR_TEST_VERBOSE=1`.
    verbose: bool,
    /// The operator's `--note` text, appended verbatim to the manifest's
    /// `notes` after the generated lines (zipline#87).
    notes: &'a [String],
    /// The credential refresher, present when `--prompt-for-sudo` actually
    /// primed (zipline#70 Step 4). The netns tier takes and stops it when
    /// it returns; `SudoRefresher`'s Drop covers every earlier exit, so no
    /// path leaks the thread. A `Cell` because `BuildInputs` is shared by
    /// reference and the tier must take ownership to stop it.
    sudo_refresher: std::cell::Cell<Option<tiers::SudoRefresher>>,
    /// The netns tier's sudo provenance for the manifest — `nopasswd` or
    /// `primed`, `None` when sudo does not work at all (zipline#70 Step 6).
    netns_sudo: Option<tiers::SudoProvenance>,
}

/// Worktrees, gates, recipes, staging, verification, the emitted manifest and
/// the tarball (task B3 steps 5-7). Returns `Ok(true)` on success and
/// `Ok(false)` on a gate or build failure — which is a gate-style exit 1, not
/// a command error — and `Err` only for failures outside the build itself
/// (unwritable build directory, a worktree that cannot be created).
///
/// Ordering invariants, from spec-003 §3-§5 and the task list:
/// - the gates run against the worktrees **before any compilation** — a set
///   whose pins disagree must fail in seconds, not after fifteen minutes of
///   cargo;
/// - the emitted manifest is written whenever the gates pass, **even when a
///   recipe fails**, with the failure reported — the most interesting set to
///   reproduce is the broken one; a gate failure emits nothing, because the
///   manifest contract is "emitted whenever the gates pass";
/// - worktrees are pruned on success (unless `keep`) and left in place on a
///   build failure, because they are what a person debugs with; on a gate
///   failure they are pruned, because nothing was built and the findings are
///   the debug artifact. `dist/` always survives.
fn execute_build(inputs: &BuildInputs) -> Result<bool> {
    let dist = inputs.build_dir.join("dist");
    let logs = inputs.build_dir.join("logs");
    let src = inputs.build_dir.join("src");
    std::fs::create_dir_all(&src)?;

    // -- worktrees: one per resolved repository that has a recipe ------------
    // (spec-003 §5: sources come from `git worktree add --detach`; the live
    // checkouts are never modified). A resolved repository without a recipe
    // is stated and skipped, not silently dropped.
    let mut worktrees: Vec<(&recipes::Recipe, PathBuf)> = Vec::new();
    for entry in inputs.resolved {
        let Some(recipe) = inputs
            .recipes
            .iter()
            .find(|recipe| recipe.repo == entry.repo)
        else {
            if !inputs.quiet {
                println!("{}: no build recipe, skipped", entry.repo);
            }
            continue;
        };
        let dest = src.join(&entry.repo);
        crate::git::worktree_add(&inputs.workspace.join(&entry.repo), &dest, &entry.sha)?;
        worktrees.push((recipe, dest));
    }

    // -- gates, against the worktrees, before any compilation ----------------
    // The derived zl-zpr-common is gated at the revision the consumers pin —
    // what cargo will compile — through a throwaway worktree removed as soon
    // as the gates have read it (Codex review on PR #8).
    let mut scan: Vec<(String, PathBuf)> = worktrees
        .iter()
        .map(|(recipe, dest)| (recipe.repo.to_string(), dest.clone()))
        .collect();
    let derived_worktree =
        add_derived_scan_pinned(inputs.manifest, inputs.workspace, &src, &mut scan)?;
    let outcome = collect_gate_findings(
        &scan,
        &inputs.set.allow_pin_drift,
        inputs.downgrade_pin_drift,
        inputs.workspace,
    );
    if let Some(dest) = derived_worktree {
        // Read by the gates above and never built: gone before compilation,
        // so a failure path cannot leave it behind for --force to trip on.
        crate::git::worktree_remove(&inputs.workspace.join("zl-zpr-common"), &dest)?;
    }
    let outcome = outcome?;
    let gate_errors = print_findings(inputs.quiet, &inputs.set.name, &outcome.findings);
    if gate_errors > 0 {
        // No manifest: it is emitted only when the gates pass (spec-003 §3).
        // Nothing was built, so the worktrees hold nothing to debug.
        for (recipe, dest) in &worktrees {
            crate::git::worktree_remove(&inputs.workspace.join(recipe.repo), dest)?;
        }
        let _ = std::fs::remove_dir(&src);
        return Ok(false);
    }

    // -- recipes, in table order (already build order) -----------------------
    // The first failure stops the build: later repositories may need this
    // one's output, and a half-built set must not look built.
    let mut failure: Option<String> = None;
    for (recipe, worktree) in &worktrees {
        if !inputs.quiet {
            println!("building {}...", recipe.repo);
        }
        let result = recipe
            .steps
            .iter()
            .try_for_each(|step| {
                recipes::run_step(recipe.repo, step, worktree, &logs, inputs.quiet)
            })
            .and_then(|()| recipes::stage_into(recipe, worktree, &dist));
        if let Err(error) = result {
            eprintln!("error: {error:#}");
            failure = Some(error.to_string());
            break;
        }
    }

    // -- verify dist/ (task B3 step 4) ---------------------------------------
    // Against the **whole distribution** the recipe table stages — never the
    // subset whose worktrees happened to exist. Deriving `expected` from the
    // worktrees let a set naming one repository "verify" a tarball holding a
    // fraction of the required binaries (Codex review on PR #8): a recipe
    // without a worktree stages nothing, so its binaries turn up missing
    // here and the build fails instead of shipping a subset.
    if failure.is_none() {
        let expected: Vec<&str> = inputs
            .recipes
            .iter()
            .flat_map(|recipe| recipe.staged.iter().map(|staged| staged.name))
            .collect();
        if let Err(error) = verify_dist(&dist, &expected) {
            eprintln!("error: {error:#}");
            failure = Some(error.to_string());
        }
    }

    // -- the test tiers (tasks B4-B5), against the worktrees and dist/ --------
    // Only after a fully verified build: a tier run against a half-built set
    // would test binaries that will never ship. Tier order is fixed: unit
    // (fast, per-repository `make test`), then netns, then docker — the two
    // end-to-end tiers of task B5, both run against `dist/`. A probe-skipped
    // tier is recorded `skipped: <reason>` and never presented as coverage.
    let mut tier_results: BTreeMap<String, Tier> = BTreeMap::new();
    let mut tier_failed = false;
    if failure.is_none() && inputs.selection.contains("unit") {
        if !inputs.quiet {
            println!("unit tier:");
        }
        let plans = tiers::unit_plan(inputs.recipes, &worktrees, &dist);
        let outcome = tiers::run_unit(&plans, &logs, inputs.quiet);
        tier_failed = !outcome.passed;
        tier_results.insert("unit".to_string(), Tier::from_outcome(&outcome));
    }

    // The netns tier: the seven integration scripts in the zl-zpr-core
    // worktree, against dist/. Skipped with the reason when the probe
    // failed, or when the set has no zl-zpr-core worktree to run in.
    if failure.is_none() && inputs.selection.contains("netns") {
        let core = worktrees
            .iter()
            .find(|(recipe, _)| recipe.repo == "zl-zpr-core")
            .map(|(_, dest)| dest.clone());
        // Taken unconditionally: whether the tier runs or skips, the
        // refresher has no work after this block (zipline#70 Step 4).
        // Stopping via take-then-drop keeps the one stop path.
        let refresher = inputs.sudo_refresher.take();
        match (&inputs.netns_skip, core, &inputs.valkey) {
            (Some(reason), _, _) => {
                if !inputs.quiet {
                    println!("netns tier: skipped ({reason})");
                }
                tier_results.insert("netns".to_string(), Tier::skipped(reason));
            }
            (None, None, _) => {
                let reason = "zl-zpr-core is not in this set";
                if !inputs.quiet {
                    println!("netns tier: skipped ({reason})");
                }
                tier_results.insert("netns".to_string(), Tier::skipped(reason));
            }
            (None, Some(core), valkey) => {
                if !inputs.quiet {
                    println!("netns tier:");
                }
                // The probe passed, so valkey was found; the fallback name
                // only defends against an inconsistent caller.
                let valkey = valkey
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("valkey-server"));
                let plan = tiers::netns_plan(&core, &dist, &valkey, inputs.verbose);
                let outcome = tiers::run_netns(&plan, &logs, inputs.quiet);
                tier_failed = tier_failed || !outcome.passed;
                // A tier that ran carries how its sudo was satisfied —
                // nopasswd host or primed credentials (zipline#70 Step 6).
                // The gate guarantees sudo worked, so a missing provenance
                // here is a caller bug worth surfacing in the manifest as
                // an absent field rather than a guessed one.
                let mut tier = Tier::from_outcome(&outcome);
                if let Some(sudo) = inputs.netns_sudo {
                    tier = tier.with_sudo(sudo);
                }
                tier_results.insert("netns".to_string(), tier);
            }
        }
        // The netns tier has returned (ran, skipped, passed or failed):
        // the credential has no further consumer, so stop refreshing it
        // now rather than at end of scope (zipline#70 Step 4).
        if let Some(refresher) = refresher {
            refresher.stop();
        }
    }

    // The docker tier: dist/ staged into the demo worktree's dns-demo/bin/,
    // deploy, DNS test, unconditional teardown. Same skip rules.
    if failure.is_none() && inputs.selection.contains("docker") {
        let demo = worktrees
            .iter()
            .find(|(recipe, _)| recipe.repo == "zl-zpr-demo")
            .map(|(_, dest)| dest.clone());
        match (&inputs.docker_skip, demo) {
            (Some(reason), _) => {
                if !inputs.quiet {
                    println!("docker tier: skipped ({reason})");
                }
                tier_results.insert("docker".to_string(), Tier::skipped(reason));
            }
            (None, None) => {
                let reason = "zl-zpr-demo is not in this set";
                if !inputs.quiet {
                    println!("docker tier: skipped ({reason})");
                }
                tier_results.insert("docker".to_string(), Tier::skipped(reason));
            }
            (None, Some(demo)) => {
                if !inputs.quiet {
                    println!("docker tier:");
                }
                let plan = tiers::docker_plan(&demo, &dist);
                let outcome = tiers::run_docker(&plan, &logs, inputs.quiet);
                tier_failed = tier_failed || !outcome.passed;
                tier_results.insert("docker".to_string(), Tier::from_outcome(&outcome));
            }
        }
    }

    // The dist/ guard (task B5): after any tier ran, the staged `ph` must
    // not be the enable-security-testing build that a2a-pubkey-test.sh
    // compiles into the worktree — that binary must never ship. A build
    // failure skips the check: dist/ is already known incomplete.
    if failure.is_none() {
        if let Err(error) = tiers::dist_ph_is_clean(&dist) {
            eprintln!("error: {error:#}");
            failure = Some(error.to_string());
        }
    }

    // -- the emitted manifest, written even on build or tier failure ---------
    // (spec-003 §3: emitted whenever the gates pass, failures recorded.)
    let mut emitted = emit(inputs.set, inputs.resolved, inputs.tip)?;
    emitted.resolved.built_from.manifest = inputs.manifest_path.clone();
    emitted.resolved.built_from.context = inputs.context_sha.clone();
    emitted.resolved.host = host_stamps();
    emitted.resolved.pins = pins_for_manifest(&outcome.pins, inputs.workspace);
    if let Some(zplc) = &outcome.zplc_version {
        emitted
            .resolved
            .versions
            .insert("zplc".to_string(), zplc.clone());
    }
    if let Some(minimum) = &outcome.vs_policy_min_compiler {
        emitted
            .resolved
            .versions
            .insert("vs_policy_min_compiler".to_string(), minimum.clone());
    }
    emitted.resolved.binaries = digest_binaries(&dist, &worktrees)?;
    // What the run did not cover, spelled out for the reader (zipline#87):
    // a green manifest must never leave the tiers it skipped to inference.
    let coverage = coverage_summary(
        inputs.selection,
        &tier_results,
        failure.is_some(),
        outcome.pin_drift_downgraded > 0,
        inputs.notes,
    );
    emitted.resolved.tests_requested = inputs.selection.requested().to_string();
    emitted.resolved.tests_skipped = coverage.skipped;
    emitted.resolved.notes = coverage.notes;
    emitted.resolved.tiers = tier_results;
    let manifest_file = dist.join(format!("zpr-set-{}.yaml", inputs.set.name));
    std::fs::write(&manifest_file, emitted_yaml(&emitted)?)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", manifest_file.display()))?;
    if !inputs.quiet {
        println!("emitted manifest: {}", manifest_file.display());
        // The same notes on the terminal, so the run's summary and the
        // file it wrote never disagree about coverage.
        for note in &emitted.resolved.notes {
            println!("note: {note}");
        }
    }

    if let Some(failure) = failure {
        // Worktrees stay for debugging; dist/ (with the manifest) survives.
        if !inputs.quiet {
            println!("build failed: {failure}");
            println!("worktrees left in {} for debugging", src.display());
        }
        return Ok(false);
    }
    if tier_failed {
        // Same retention rule as a build failure: the failing tests and
        // their fixtures live in the worktrees, so they are what a person
        // debugs with. No tarball: a set that failed its tests must not
        // look shippable, though dist/ (with the manifest recording the
        // failure) survives.
        if !inputs.quiet {
            println!("unit tier failed; see tiers.unit in the emitted manifest");
            println!("worktrees left in {} for debugging", src.display());
        }
        return Ok(false);
    }

    // -- tarball (task B3 step 6) --------------------------------------------
    if !inputs.no_tarball {
        write_tarball(&dist, &inputs.set.name, inputs.quiet)?;
    }

    // -- prune worktrees on success unless --keep (task B3 step 7) -----------
    if inputs.keep {
        if !inputs.quiet {
            println!("worktrees kept in {} (--keep)", src.display());
        }
    } else {
        for (recipe, dest) in &worktrees {
            crate::git::worktree_remove(&inputs.workspace.join(recipe.repo), dest)?;
        }
        // Only ever holds worktrees, so it is empty now; removing it keeps
        // the build directory to dist/ and logs/.
        let _ = std::fs::remove_dir(&src);
    }
    Ok(true)
}

/// The sha256 of one file, streamed, as lowercase hex.
fn sha256_hex(path: &Path) -> Result<String> {
    use sha2::Digest as _;
    let mut file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("cannot open {}: {e}", path.display()))?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Digests every staged binary in `dist/` for the emitted manifest's
/// `binaries:` block, attributing each to the repository whose recipe staged
/// it. A binary missing after a failed build is simply absent — honest
/// emptiness, matching what actually landed in `dist/`.
fn digest_binaries(dist: &Path, worktrees: &[(&recipes::Recipe, PathBuf)]) -> Result<Vec<Binary>> {
    let mut binaries: Vec<Binary> = Vec::new();
    for (recipe, _) in worktrees {
        for staged in recipe.staged {
            let path = dist.join(staged.name);
            if !path.is_file() {
                continue;
            }
            binaries.push(Binary {
                name: staged.name.to_string(),
                sha256: sha256_hex(&path)?,
                bytes: std::fs::metadata(&path)?.len(),
                from: recipe.repo.to_string(),
            });
        }
    }
    Ok(binaries)
}

/// The host block of the emitted manifest (spec-003 §3): os, arch, and the
/// version line of each toolchain that produced binaries. A tool that is not
/// installed is recorded as absent rather than failing the build — the demo
/// repo needs no Go on a machine that never builds `coredns`.
fn host_stamps() -> BTreeMap<String, String> {
    let mut host = BTreeMap::new();
    host.insert("os".to_string(), std::env::consts::OS.to_string());
    host.insert("arch".to_string(), std::env::consts::ARCH.to_string());
    for (name, args) in [
        ("rustc", &["--version"][..]),
        ("cargo", &["--version"][..]),
        ("go", &["version"][..]),
        ("capnp", &["--version"][..]),
    ] {
        let version = std::process::Command::new(name)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .map(str::to_string)
            });
        if let Some(version) = version {
            host.insert(name.to_string(), version);
        }
    }
    host
}

/// Writes `dist/zpr-set-<name>-linux-<arch>.tar.gz` holding everything in
/// `dist/` except the tarball itself (the emitted manifest is deliberately
/// inside). Shells out to `tar`, consistent with how git is invoked (spec-001
/// §6.1: no new crates beyond `toml` and `sha2`).
fn write_tarball(dist: &Path, name: &str, quiet: bool) -> Result<()> {
    let arch = std::env::consts::ARCH;
    let tarball = format!("zpr-set-{name}-linux-{arch}.tar.gz");
    let mut entries: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dist)?.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name != tarball {
            entries.push(file_name);
        }
    }
    entries.sort();

    let mut command = std::process::Command::new("tar");
    command.arg("-czf").arg(&tarball).arg("-C").arg(dist);
    command.args(&entries);
    command.current_dir(dist);
    let output = command
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run tar: {e}"))?;
    if !output.status.success() {
        bail!(
            "tar failed creating {tarball}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if !quiet {
        println!("tarball: {}", dist.join(&tarball).display());
    }
    Ok(())
}

/// Converts gate 1's raw occurrences into the emitted manifest's `pins:`
/// block: one entry per agreed `(crate, url, reference)` with every pinning
/// manifest listed, and `newest_available` computed against the workspace
/// checkouts' tags by the same prefix rule as gate 2. Disagreeing crates are
/// omitted — they cannot appear in a manifest that only exists because the
/// gates passed (an allow_pin_drift crate's divergence is recorded by its
/// drift entry instead).
fn pins_for_manifest(pins: &[gates::PinOccurrence], workspace: &Path) -> Vec<Pin> {
    let mut by_crate: BTreeMap<&str, Vec<&gates::PinOccurrence>> = BTreeMap::new();
    for pin in pins {
        by_crate.entry(&pin.crate_name).or_default().push(pin);
    }

    let mut result: Vec<Pin> = Vec::new();
    for (crate_name, occurrences) in by_crate {
        let mut variants: Vec<(&str, &str)> = occurrences
            .iter()
            .map(|p| (p.url.as_str(), p.reference.as_str()))
            .collect();
        variants.sort_unstable();
        variants.dedup();
        if variants.len() != 1 {
            continue;
        }
        let (url, reference) = variants[0];

        // Newest tag sharing the pin's prefix in the pinned repository's
        // local checkout, when both exist — same rule as gate 2.
        let newest_available = gates::split_tag(reference).and_then(|(prefix, pinned)| {
            let repo = url
                .rsplit('/')
                .next()
                .map(|last| last.strip_suffix(".git").unwrap_or(last))?;
            let dir = workspace.join(repo);
            if !crate::git::is_repo(&dir) {
                return None;
            }
            let newest = crate::git::tag_list(&dir)
                .ok()?
                .iter()
                .filter_map(|tag| match gates::split_tag(tag) {
                    Some((p, version)) if p == prefix => Some(version),
                    _ => None,
                })
                .max()?;
            (newest > pinned).then(|| format!("{prefix}{}", gates::join_version(&newest)))
        });

        result.push(Pin {
            crate_name: crate_name.to_string(),
            url: url.to_string(),
            tag: reference.to_string(),
            pinned_by: occurrences.iter().map(|p| p.file.clone()).collect(),
            newest_available,
        });
    }
    result
}

/// The current time as `YYYY-MM-DDTHH:MM:SSZ`, from the system clock and the
/// proleptic-Gregorian conversion of Howard Hinnant's `civil_from_days` —
/// spelled out here because spec-001 §6.1 keeps the dependency set free of a
/// date crate for the sake of one timestamp.
fn utc_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let (hour, minute, second) = (
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60,
    );

    // civil_from_days: days since 1970-01-01 -> (year, month, day).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
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

    /// A name that is not a single plain path component is rejected at parse
    /// time: the name lands in `.zpr-build/<name>` and the emitted file
    /// names, and `--force` removes that directory wholesale — so `..`, a
    /// separator or an absolute path could escape the build root and delete
    /// an arbitrary directory (Codex review on PR #8).
    #[test]
    fn traversal_names_are_rejected_naming_the_rule() {
        for name in ["'..'", "'.'", "'../evil'", "'/abs'", "'a/b'", "'a\\b'"] {
            let text = VALID.replace("name: 2026-09-17", &format!("name: {name}"));
            let error = parse(&text).unwrap_err().to_string();
            assert!(error.contains("path component"), "{name}: {error}");
        }
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
    /// bare origin so `origin/main` exists — what `--tip` resolves. The
    /// committed `Makefile` is what the unit tier's `make test` runs.
    fn workspace_with_repo() -> (tempfile::TempDir, PathBuf, String) {
        workspace_with_repo_and_makefile("test:\n\t@echo unit tested\n")
    }

    /// [`workspace_with_repo`] with the fixture's `Makefile` supplied, so a
    /// tier test can seed a failing `make test`.
    fn workspace_with_repo_and_makefile(makefile: &str) -> (tempfile::TempDir, PathBuf, String) {
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
        std::fs::write(scratch.join("Makefile"), makefile).unwrap();
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

    // -- emitted manifest (spec-003 §3) ---------------------------------------

    /// The round-trip property, normative in spec-003 §3: emit a resolved set,
    /// re-read the YAML as an input build set, and resolution is the identity —
    /// same repositories, same shas — with the `resolved:` block ignored.
    #[test]
    fn emitted_manifest_round_trips_as_a_build_set() {
        let (_tmp, workspace, sha) = workspace_with_repo();
        let set = one_repo_set("v0.3.1");
        let resolved = resolve_set(&workspace, &set).unwrap();

        let emitted = emit(&set, &resolved, false).unwrap();
        let yaml = emitted_yaml(&emitted).unwrap();
        assert!(yaml.contains("resolved:"), "{yaml}");

        // Re-read as an input build set: parses cleanly, refs are the shas.
        let reread = parse(&yaml).unwrap();
        assert_eq!(reread.name, set.name);
        assert_eq!(reread.repositories["zl-zpr-core"], sha);
        assert_eq!(reread.allow_pin_drift.len(), set.allow_pin_drift.len());

        // Resolving the re-read set yields the same shas: a sha resolves to
        // itself, so the emitted manifest reproduces its own resolution.
        let again = resolve_set(&workspace, &reread).unwrap();
        assert_eq!(again.len(), resolved.len());
        for (first, second) in resolved.iter().zip(&again) {
            assert_eq!(first.repo, second.repo);
            assert_eq!(first.sha, second.sha);
        }
    }

    /// The emitted repositories map holds full 40-character shas, never the
    /// input refs, and records the input's name and drift entries unchanged.
    #[test]
    fn emit_replaces_refs_with_shas_and_keeps_name_and_drift() {
        let (_tmp, workspace, sha) = workspace_with_repo();
        let set = parse(
            "version: 1\nname: 2026-09-17\nrepositories:\n  zl-zpr-core: main\n\
             allow_pin_drift:\n  - crate: rcu\n    reason: \"zipline#18\"\n",
        )
        .unwrap();
        let resolved = resolve_set(&workspace, &set).unwrap();

        let emitted = emit(&set, &resolved, true).unwrap();
        assert_eq!(emitted.version, 1);
        assert_eq!(emitted.name, "2026-09-17");
        assert_eq!(emitted.repositories["zl-zpr-core"], sha);
        assert_eq!(emitted.repositories["zl-zpr-core"].len(), 40);
        assert_eq!(emitted.allow_pin_drift[0].crate_name, "rcu");
        assert!(emitted.resolved.built_from.tip);
        // Honest emptiness: nothing pretends B3-B5 ran (spec-003 §1.2).
        assert!(emitted.resolved.pins.is_empty());
        assert!(emitted.resolved.binaries.is_empty());
        assert!(emitted.resolved.tiers.is_empty());
        // But the emission time is real.
        assert!(!emitted.resolved.built_at.is_empty());
    }

    /// The `tiers.unit` block (task B4 step 2 / approved Q3 on zipline#61):
    /// a `Tier` built from a tier outcome serializes with its overall status
    /// and the per-repository breakdown, and a skip-only tier is `passed`.
    #[test]
    fn tier_from_outcome_serializes_status_and_repo_breakdown() {
        let outcome = tiers::TierOutcome {
            passed: false,
            repos: [
                ("zl-zpr-compiler".to_string(), "passed".to_string()),
                (
                    "zl-zpr-visaservice".to_string(),
                    "failed at `pregen`: ...".to_string(),
                ),
                (
                    "zl-zpr-demo".to_string(),
                    "skipped: no unit tests (docker tier covers it)".to_string(),
                ),
            ]
            .into(),
        };
        let tier = Tier::from_outcome(&outcome);
        assert_eq!(tier.status, "failed");

        let yaml = serde_yaml_ng::to_string(&tier).unwrap();
        assert!(yaml.contains("status: failed"), "{yaml}");
        assert!(yaml.contains("zl-zpr-compiler: passed"), "{yaml}");
        assert!(yaml.contains("pregen"), "{yaml}");
        assert!(yaml.contains("zl-zpr-demo"), "{yaml}");
    }

    /// A `Tier` with no repos entries — the #59/#60 shape — serializes
    /// without a `repos:` key at all, so earlier manifests are unchanged
    /// (approved Q3: skip-serialized when empty).
    #[test]
    fn tier_without_repos_serializes_the_59_60_shape_unchanged() {
        let tier = Tier {
            status: "passed".to_string(),
            reason: None,
            repos: BTreeMap::new(),
            sudo: None,
        };
        let yaml = serde_yaml_ng::to_string(&tier).unwrap();
        assert!(!yaml.contains("repos"), "{yaml}");
        assert!(!yaml.contains("reason"), "{yaml}");
        assert_eq!(yaml.trim(), "status: passed");
    }

    // -- build directory lifecycle (task B3 step 2) ---------------------------

    /// A fresh build directory is created with `logs/` and `dist/` inside.
    #[test]
    fn prepare_build_dir_creates_logs_and_dist() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tip");
        prepare_build_dir(&dir, false, tmp.path(), &[]).unwrap();
        assert!(dir.join("logs").is_dir());
        assert!(dir.join("dist").is_dir());
    }

    /// A pre-existing build directory is refused with a message naming both
    /// the path and `--force` (approved Q1: refuse, never reuse — reused
    /// half-built state is how silent staleness gets shipped).
    #[test]
    fn prepare_build_dir_refuses_preexisting_naming_force() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tip");
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist").join("stale-binary"), "old\n").unwrap();

        let error = prepare_build_dir(&dir, false, tmp.path(), &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("tip"), "path not named: {error}");
        assert!(error.contains("--force"), "remedy not named: {error}");
        // Nothing was touched: the stale content is intact.
        assert!(dir.join("dist").join("stale-binary").exists());
    }

    /// `--force` removes the previous directory entirely and recreates it
    /// fresh: no stale file survives into the new run.
    #[test]
    fn prepare_build_dir_force_removes_and_recreates() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tip");
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist").join("stale-binary"), "old\n").unwrap();

        prepare_build_dir(&dir, true, tmp.path(), &[]).unwrap();
        assert!(!dir.join("dist").join("stale-binary").exists());
        assert!(dir.join("logs").is_dir());
        assert!(dir.join("dist").is_dir());
    }

    /// `--force` over a build directory holding worktrees retained by a
    /// previous failed run must *unregister* them, not just delete their
    /// directories: a raw `remove_dir_all` leaves the source repository's
    /// worktree registration behind, and the very next `git worktree add`
    /// fails with "missing but already registered worktree" — making the
    /// documented `--force` recovery path unusable (Codex review on PR #8).
    #[test]
    fn prepare_build_dir_force_unregisters_retained_worktrees() {
        let (_tmp, workspace, sha) = workspace_with_repo();
        let build_dir = workspace.join(".zpr-build").join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        // A retained worktree, as a failed run leaves it.
        let dest = build_dir.join("src").join("zl-zpr-core");
        crate::git::worktree_add(&workspace.join("zl-zpr-core"), &dest, &sha).unwrap();

        // The --force retry must clear the directory AND the registration...
        prepare_build_dir(&build_dir, true, &workspace, &["zl-zpr-core"]).unwrap();
        assert!(!dest.exists());
        let listing =
            crate::git::git(&workspace.join("zl-zpr-core"), &["worktree", "list"]).unwrap();
        assert_eq!(listing.lines().count(), 1, "stale registration: {listing}");

        // ...so the next run's worktree_add succeeds where it used to fail.
        crate::git::worktree_add(&workspace.join("zl-zpr-core"), &dest, &sha).unwrap();
    }

    /// A build directory deleted by hand — `rm -rf` with worktrees still
    /// registered — must not wedge the next build (zipline#71). Every source
    /// checkout still carries a registration pointing into the directory that
    /// is gone, and without a prune the next `git worktree add` fails with
    /// git's "missing but already registered worktree". The tool must recover
    /// on its own: a fresh `worktree_add` succeeds with no flag and no manual
    /// `git worktree prune`.
    #[test]
    fn worktree_add_succeeds_after_build_dir_deleted() {
        let (_tmp, workspace, sha) = workspace_with_repo();
        let build_dir = workspace.join(".zpr-build").join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        // A worktree, as a build run registers it.
        let dest = build_dir.join("src").join("zl-zpr-core");
        crate::git::worktree_add(&workspace.join("zl-zpr-core"), &dest, &sha).unwrap();

        // The by-hand deletion: the whole build directory goes, but every
        // registration in the source repository survives it.
        std::fs::remove_dir_all(&build_dir).unwrap();

        // The next run's worktree_add — same registered path, fresh build
        // directory — must succeed, self-healing the stale registration.
        // `src/` is pre-created as a multi-repository build would have it:
        // the first repository's add creates it, and only then does git's
        // stale-registration check resolve the later destinations' paths and
        // fail them with "missing but already registered worktree". (With no
        // resolvable parent the check is silently skipped and git records a
        // *duplicate* registration instead — the same stale state, hidden.)
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();
        std::fs::create_dir_all(build_dir.join("src")).unwrap();
        crate::git::worktree_add(&workspace.join("zl-zpr-core"), &dest, &sha).unwrap();
        assert!(dest.join("README.md").exists());
    }

    /// `--force` over a *half-wiped* build directory — `src/` deleted by hand,
    /// the rest still present — must still clear every registration
    /// (zipline#71). The old unregister loop derived the worktree list from
    /// the filesystem (`read_dir` on `src/`), and the failure mode is exactly
    /// the filesystem being gone: it enumerated nothing and every
    /// registration survived. The manifest-driven prune catches them.
    #[test]
    fn prepare_build_dir_force_unregisters_when_src_is_wiped() {
        let (_tmp, workspace, sha) = workspace_with_repo();
        let build_dir = workspace.join(".zpr-build").join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        // A retained worktree, then the by-hand half-wipe: src/ goes, the
        // build directory itself stays, so the --force path runs in full.
        let dest = build_dir.join("src").join("zl-zpr-core");
        crate::git::worktree_add(&workspace.join("zl-zpr-core"), &dest, &sha).unwrap();
        std::fs::remove_dir_all(build_dir.join("src")).unwrap();

        // --force must clear the registration it can no longer enumerate.
        prepare_build_dir(&build_dir, true, &workspace, &["zl-zpr-core"]).unwrap();
        let listing =
            crate::git::git(&workspace.join("zl-zpr-core"), &["worktree", "list"]).unwrap();
        assert_eq!(listing.lines().count(), 1, "stale registration: {listing}");
    }

    // -- dist/ verification (task B3 step 4) ----------------------------------

    /// Writes a mode-0755 stand-in binary named `name` into `dir`.
    fn fake_dist_binary(dir: &Path, name: &str) {
        let path = dir.join(name);
        std::fs::write(&path, format!("{name}\n")).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    /// A dist/ holding every expected name, each executable, verifies.
    #[test]
    fn verify_dist_passes_when_all_names_present_and_executable() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["zplc", "zpdump"] {
            fake_dist_binary(tmp.path(), name);
        }
        verify_dist(tmp.path(), &["zplc", "zpdump"]).unwrap();
    }

    /// A missing binary fails naming it — a recipe that silently produced
    /// nothing must not pass (task B3 step 4).
    #[test]
    fn verify_dist_names_missing_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        fake_dist_binary(tmp.path(), "zplc");
        let error = verify_dist(tmp.path(), &["zplc", "zpdump", "vs"])
            .unwrap_err()
            .to_string();
        assert!(error.contains("zpdump"), "{error}");
        assert!(error.contains("vs"), "{error}");
        assert!(!error.contains("zplc"), "present binary named: {error}");
    }

    /// A present but non-executable file fails the mode check naming it.
    #[test]
    fn verify_dist_names_non_executable_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("zplc"), "not executable\n").unwrap();
        let error = verify_dist(tmp.path(), &["zplc"]).unwrap_err().to_string();
        assert!(error.contains("zplc"), "{error}");
        assert!(error.contains("executable"), "{error}");
    }

    // -- orchestration: worktrees, manifest, digests, tarball, pruning --------
    // (task B3 steps 5-7, driven through execute_build with fixture recipes)

    /// A recipe whose one step copies a committed file to an executable
    /// "binary" — enough to exercise staging, digests and the tarball without
    /// compiling anything.
    fn ok_recipe() -> recipes::Recipe {
        recipes::Recipe {
            repo: "zl-zpr-core",
            steps: &[recipes::Step {
                name: "build",
                program: "sh",
                args: &[
                    "-c",
                    "mkdir -p out && cp README.md out/bin1 && chmod +x out/bin1",
                ],
            }],
            staged: &[recipes::Staged {
                name: "bin1",
                source: "out/bin1",
            }],
        }
    }

    /// A recipe whose one step fails.
    fn failing_recipe() -> recipes::Recipe {
        recipes::Recipe {
            repo: "zl-zpr-core",
            steps: &[recipes::Step {
                name: "boom",
                program: "sh",
                args: &["-c", "echo broken build; exit 3"],
            }],
            staged: &[],
        }
    }

    /// Builds the standard inputs for `execute_build` over the one-repo
    /// fixture workspace. The fixture repository has no Cargo.toml, so the
    /// gates report an INFO and pass — the gate path is exercised without a
    /// manifest fixture. The selection is `--test none`: the tier path has
    /// its own tests below.
    #[allow(clippy::too_many_arguments)]
    fn inputs<'a>(
        set: &'a BuildSet,
        resolved: &'a [Resolved],
        workspace: &'a Path,
        manifest: &'a Manifest,
        build_dir: &'a Path,
        recipes: &'a [recipes::Recipe],
        no_tarball: bool,
        keep: bool,
        selection: &'a tiers::Selection,
    ) -> BuildInputs<'a> {
        BuildInputs {
            set,
            resolved,
            workspace,
            manifest,
            build_dir,
            recipes,
            tip: false,
            keep,
            quiet: true,
            no_tarball,
            downgrade_pin_drift: false,
            manifest_path: "test-set.yaml".to_string(),
            context_sha: "deadbee".to_string(),
            selection,
            netns_skip: None,
            docker_skip: None,
            valkey: None,
            verbose: false,
            sudo_refresher: std::cell::Cell::new(None),
            netns_sudo: None,
            notes: &[],
        }
    }

    /// The `--test none` selection, for the B3-era tests above the tier ones.
    fn no_tiers() -> tiers::Selection {
        tiers::Selection::parse(Some("none")).unwrap()
    }

    /// The emitted manifest is written to `dist/zpr-set-<name>.yaml` **even
    /// when a recipe fails** (spec-003 §3: the most interesting set to
    /// reproduce is the one that broke), the failure is reported by the
    /// `false` return, and the worktree is left in place for debugging.
    #[test]
    fn execute_build_writes_manifest_even_when_a_recipe_fails() {
        let (_tmp, workspace, sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![failing_recipe()];
        let ok = execute_build(&inputs(
            &set,
            &resolved,
            &workspace,
            &manifest,
            &build_dir,
            &recipes,
            true,
            false,
            &no_tiers(),
        ))
        .unwrap();
        assert!(!ok, "a failing recipe must report failure");

        // The manifest exists, parses as a build set, and carries the sha.
        let manifest_path = build_dir.join("dist").join("zpr-set-t.yaml");
        let text = std::fs::read_to_string(&manifest_path).unwrap();
        let reread = parse(&text).unwrap();
        assert_eq!(reread.repositories["zl-zpr-core"], sha);

        // The worktree is left in place on failure: it is what a person
        // needs in order to debug (task B3 step 7).
        assert!(build_dir.join("src").join("zl-zpr-core").exists());
        // The failing step's log was written.
        assert!(build_dir.join("logs").join("zl-zpr-core-boom.log").exists());
    }

    /// A successful run: the written manifest round-trips (re-read resolution
    /// is the identity), the staged binary is digested with its size and
    /// origin recorded, toolchain stamps are present, and the worktree is
    /// pruned.
    #[test]
    fn execute_build_success_round_trips_digests_and_prunes() {
        let (_tmp, workspace, sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let ok = execute_build(&inputs(
            &set,
            &resolved,
            &workspace,
            &manifest,
            &build_dir,
            &recipes,
            true,
            false,
            &no_tiers(),
        ))
        .unwrap();
        assert!(ok);

        // dist/ holds the staged binary and the manifest.
        let dist = build_dir.join("dist");
        assert!(dist.join("bin1").is_file());
        let text = std::fs::read_to_string(dist.join("zpr-set-t.yaml")).unwrap();

        // Round-trip: re-read as an input set, resolution is the identity.
        let reread = parse(&text).unwrap();
        assert_eq!(reread.repositories["zl-zpr-core"], sha);
        let again = resolve_set(&workspace, &reread).unwrap();
        assert_eq!(again[0].sha, sha);

        // The diagnostic block records the binary digest and the source repo.
        assert!(text.contains("bin1"), "{text}");
        assert!(text.contains("from: zl-zpr-core"), "{text}");
        // A sha256 is 64 hex characters; spot-check one is present.
        assert!(
            text.lines()
                .any(|line| line.contains("sha256:") && line.trim().len() >= 64),
            "no sha256 in manifest: {text}"
        );
        // Toolchain stamps: rustc and cargo exist on any machine that builds
        // this tool.
        assert!(text.contains("rustc"), "{text}");

        // The worktree was pruned on success (no --keep).
        assert!(!build_dir.join("src").join("zl-zpr-core").exists());
        // The source checkout has no lingering worktree entry.
        let listing =
            crate::git::git(&workspace.join("zl-zpr-core"), &["worktree", "list"]).unwrap();
        assert_eq!(listing.lines().count(), 1, "{listing}");
    }

    /// Verification must cover the **whole distribution** the recipe table
    /// stages, not just the repositories the set happened to name: a set
    /// naming one repository used to derive `expected` from its own
    /// worktrees, so a tarball with a fraction of the binaries "verified"
    /// (Codex review on PR #8). A recipe with no worktree means its binaries
    /// cannot exist, and the build must fail rather than emit a subset.
    #[test]
    fn execute_build_fails_verification_when_the_set_skips_a_recipe() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        // The table stages bin1 (buildable here) and bin2 from a repository
        // the set does not name — so bin2 can never be staged.
        let recipes = vec![
            ok_recipe(),
            recipes::Recipe {
                repo: "zl-zpr-absent",
                steps: &[],
                staged: &[recipes::Staged {
                    name: "bin2",
                    source: "out/bin2",
                }],
            },
        ];
        let ok = execute_build(&inputs(
            &set,
            &resolved,
            &workspace,
            &manifest,
            &build_dir,
            &recipes,
            true,
            false,
            &no_tiers(),
        ))
        .unwrap();
        assert!(!ok, "a subset of the distribution must not verify");
    }

    /// Fixture for the derived-common gate: `zl-zpr-core` pins `zpr` from
    /// `zl-zpr-common` at `v0.26.0` and `zpr-utils` at `zpr-utils-v0.2.2`.
    /// The common checkout's manifest at the *tagged* revision agrees with
    /// core's pins, but its live `HEAD` has moved to a conflicting one.
    fn workspace_with_pinned_common() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");

        let init = |name: &str| -> PathBuf {
            let dir = workspace.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            setup_git(&dir, &["init", "-b", "main"]);
            setup_git(&dir, &["config", "user.name", "zpr-dev tests"]);
            setup_git(&dir, &["config", "user.email", "tests@example.invalid"]);
            setup_git(&dir, &["config", "commit.gpgsign", "false"]);
            dir
        };
        let commit = |dir: &Path, message: &str| {
            setup_git(dir, &["add", "-A"]);
            setup_git(dir, &["commit", "-m", message]);
        };

        let common = init("zl-zpr-common");
        let utils_pin = |tag: &str| {
            format!(
                "[package]\nname = \"zpr\"\nversion = \"0.26.0\"\n\n[dependencies]\n\
                 zpr-utils = {{ git = \"https://github.com/mkolehmainen/zl-zpr-utils.git\", tag = \"{tag}\" }}\n"
            )
        };
        std::fs::write(common.join("Cargo.toml"), utils_pin("zpr-utils-v0.2.2")).unwrap();
        commit(&common, "manifest agreeing with the consumers");
        setup_git(&common, &["tag", "v0.26.0"]);
        std::fs::write(common.join("Cargo.toml"), utils_pin("zpr-utils-v0.9.9")).unwrap();
        commit(&common, "live checkout moved past the pinned tag");

        let core = init("zl-zpr-core");
        std::fs::write(
            core.join("Cargo.toml"),
            "[package]\nname = \"core-fixture\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
             zpr = { git = \"https://github.com/mkolehmainen/zl-zpr-common.git\", tag = \"v0.26.0\" }\n\
             zpr-utils = { git = \"https://github.com/mkolehmainen/zl-zpr-utils.git\", tag = \"zpr-utils-v0.2.2\" }\n",
        )
        .unwrap();
        std::fs::write(core.join("README.md"), "fixture\n").unwrap();
        commit(&core, "core with pins");

        (tmp, workspace)
    }

    /// The derived `zl-zpr-common` gate scan must read the revision the
    /// consumers pin — what cargo will actually compile — never the live
    /// checkout: a live checkout sitting on a different commit made gate 1
    /// reject a compatible set (this test's RED) or miss a real
    /// disagreement, and made the emitted `pins` block vary with unrelated
    /// local state (Codex review on PR #8).
    #[test]
    fn execute_build_gates_common_at_the_pinned_revision_not_the_live_checkout() {
        let (_tmp, workspace) = workspace_with_pinned_common();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let ok = execute_build(&inputs(
            &set,
            &resolved,
            &workspace,
            &manifest,
            &build_dir,
            &recipes,
            true,
            false,
            &no_tiers(),
        ))
        .unwrap();
        assert!(ok, "the live checkout's drift must not fail a coherent set");

        // The emitted pins round-trip from the manifest: they record what
        // cargo compiles (the tagged revision's pin), not the live drift.
        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        assert!(text.contains("zpr-utils-v0.2.2"), "{text}");
        assert!(!text.contains("zpr-utils-v0.9.9"), "{text}");

        // The throwaway common worktree is gone: directory and registration.
        assert!(!build_dir.join("src").join("zl-zpr-common").exists());
        let listing =
            crate::git::git(&workspace.join("zl-zpr-common"), &["worktree", "list"]).unwrap();
        assert_eq!(listing.lines().count(), 1, "{listing}");
    }

    /// `--keep` leaves the worktree in place after a successful run.
    #[test]
    fn execute_build_keep_leaves_worktrees() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let ok = execute_build(&inputs(
            &set,
            &resolved,
            &workspace,
            &manifest,
            &build_dir,
            &recipes,
            true,
            true,
            &no_tiers(),
        ))
        .unwrap();
        assert!(ok);
        assert!(build_dir.join("src").join("zl-zpr-core").exists());
    }

    /// Without `--no-tarball` a `zpr-set-<name>-linux-<arch>.tar.gz` lands in
    /// `dist/`.
    #[test]
    fn execute_build_writes_tarball_unless_disabled() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let ok = execute_build(&inputs(
            &set,
            &resolved,
            &workspace,
            &manifest,
            &build_dir,
            &recipes,
            false,
            false,
            &no_tiers(),
        ))
        .unwrap();
        assert!(ok);

        let arch = std::env::consts::ARCH;
        let tarball = build_dir
            .join("dist")
            .join(format!("zpr-set-t-linux-{arch}.tar.gz"));
        assert!(tarball.is_file(), "missing {}", tarball.display());
    }

    // -- the unit tier through execute_build (task B4, zipline#61) -------------

    /// A unit-tier run records `tiers.unit` in the emitted manifest with the
    /// per-repository breakdown matching what ran: the fixture's `make test`
    /// passes, so the tier and its one repository are both `passed`.
    #[test]
    fn execute_build_records_a_passing_unit_tier_in_the_manifest() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let selection = tiers::Selection::parse(Some("unit")).unwrap();
        let ok = execute_build(&inputs(
            &set, &resolved, &workspace, &manifest, &build_dir, &recipes, true, false, &selection,
        ))
        .unwrap();
        assert!(ok, "a passing tier must not fail the build");

        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        assert!(text.contains("tiers:"), "{text}");
        assert!(text.contains("unit:"), "{text}");
        assert!(text.contains("status: passed"), "{text}");
        assert!(text.contains("zl-zpr-core: passed"), "{text}");
        // The tier ran in the worktree, and the step's log was written.
        assert!(build_dir.join("logs").join("zl-zpr-core-test.log").exists());
    }

    /// A failing `make test` fails the tier and the run (exit-1 path), the
    /// manifest still lands (spec-003 §3: emitted whenever the gates pass)
    /// recording `tiers.unit.status: failed` with the repository's entry
    /// naming the failing step, and the worktree survives for debugging.
    #[test]
    fn execute_build_records_a_failing_unit_tier_and_returns_failure() {
        let (_tmp, workspace, _sha) =
            workspace_with_repo_and_makefile("test:\n\t@echo broken; exit 1\n");
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let selection = tiers::Selection::parse(Some("unit")).unwrap();
        let ok = execute_build(&inputs(
            &set, &resolved, &workspace, &manifest, &build_dir, &recipes, true, false, &selection,
        ))
        .unwrap();
        assert!(!ok, "a failing tier must fail the run");

        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        assert!(text.contains("status: failed"), "{text}");
        assert!(text.contains("`test`"), "failing step not named: {text}");
        // The worktree is retained on a tier failure, like a build failure:
        // the failing tests are what a person debugs with.
        assert!(build_dir.join("src").join("zl-zpr-core").exists());
    }

    /// `--test none` still emits `tiers` empty — the #59/#60 output shape,
    /// byte-unchanged (approved Q3) — and prunes worktrees as before.
    #[test]
    fn execute_build_test_none_emits_empty_tiers_as_before() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let ok = execute_build(&inputs(
            &set,
            &resolved,
            &workspace,
            &manifest,
            &build_dir,
            &recipes,
            true,
            false,
            &no_tiers(),
        ))
        .unwrap();
        assert!(ok);
        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        assert!(!text.contains("tiers:"), "{text}");
        assert!(!build_dir.join("src").join("zl-zpr-core").exists());
        // ...but never silently (zipline#87): the request and the gap are
        // spelled out for the reader.
        assert!(text.contains("tests_requested: none"), "{text}");
        assert!(text.contains("no tests ran (--test none)"), "{text}");
    }

    /// A set gated on `--test unit` alone must say so (zipline#87): the
    /// request is recorded, every unselected tier lands in `tests_skipped`
    /// with the reason, and `notes` spells out for a human that the
    /// integration tiers never ran. `tiers:` itself is unchanged.
    #[test]
    fn execute_build_unit_only_records_unselected_tiers() {
        let (_tmp, workspace, _sha) = workspace_with_repo_and_makefile("test:\n\t@echo ok\n");
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let selection = tiers::Selection::parse(Some("unit")).unwrap();
        let ok = execute_build(&inputs(
            &set, &resolved, &workspace, &manifest, &build_dir, &recipes, true, false, &selection,
        ))
        .unwrap();
        assert!(ok);

        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        assert!(text.contains("tests_requested: unit"), "{text}");
        assert!(text.contains("tests_skipped:"), "{text}");
        assert!(text.contains("netns: not selected (--test unit)"), "{text}");
        assert!(
            text.contains("docker: not selected (--test unit)"),
            "{text}"
        );
        assert!(
            text.contains("netns integration tests did NOT run: not selected (--test unit)"),
            "{text}"
        );
        assert!(
            text.contains("docker end-to-end tests did NOT run: not selected (--test unit)"),
            "{text}"
        );
        // The unit tier itself ran and is not listed as skipped.
        assert!(!text.contains("unit: not selected"), "{text}");
        assert!(!text.contains("unit tests did NOT run"), "{text}");
    }

    /// Operator context from `--note` lands in `notes` after the generated
    /// lines, verbatim (zipline#87).
    #[test]
    fn execute_build_appends_operator_notes() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let notes = vec!["netns run by hand; see zipline#82".to_string()];
        let selection = no_tiers();
        let mut in_ = inputs(
            &set, &resolved, &workspace, &manifest, &build_dir, &recipes, true, false, &selection,
        );
        in_.notes = &notes;
        assert!(execute_build(&in_).unwrap());

        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        let generated = text.find("no tests ran (--test none)").expect(&text);
        let operator = text.find("netns run by hand; see zipline#82").expect(&text);
        assert!(generated < operator, "operator note must come last: {text}");
    }

    // -- the netns and docker tiers in the manifest (task B5, zipline#62) -------

    /// A probe-skipped tier serializes as `status: skipped` with the probe's
    /// reason and no repos breakdown — visible in the manifest, never
    /// presented as coverage (master plan B5 constraint).
    #[test]
    fn tier_skipped_serializes_status_and_reason() {
        let tier = Tier::skipped("docker not found");
        let yaml = serde_yaml_ng::to_string(&tier).unwrap();
        assert!(yaml.contains("status: skipped"), "{yaml}");
        assert!(yaml.contains("reason: docker not found"), "{yaml}");
        assert!(!yaml.contains("repos:"), "{yaml}");
    }

    /// A selected tier whose probe failed (default selection, approved Q1 on
    /// zipline#62) is recorded `skipped` with the reason in the emitted
    /// manifest, and the skip does not fail the run.
    #[test]
    fn execute_build_records_a_probe_skipped_tier() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let selection = tiers::Selection::parse(Some("docker")).unwrap();
        let mut in_ = inputs(
            &set, &resolved, &workspace, &manifest, &build_dir, &recipes, true, false, &selection,
        );
        in_.docker_skip = Some("docker not found".to_string());
        let ok = execute_build(&in_).unwrap();
        assert!(ok, "a probe skip must not fail the run");

        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        assert!(text.contains("docker:"), "{text}");
        assert!(text.contains("status: skipped"), "{text}");
        assert!(text.contains("reason: docker not found"), "{text}");
    }

    /// The netns tier's manifest entry records its sudo provenance
    /// (zipline#70 Step 6, approved Q1): `sudo: nopasswd` for a run on a
    /// passwordless host, `sudo: primed` for one on primed credentials —
    /// never conflated, and absent entirely on a skipped tier.
    #[test]
    fn tier_records_sudo_provenance_distinctly() {
        let outcome = tiers::TierOutcome {
            passed: true,
            repos: BTreeMap::new(),
        };

        let nopasswd = Tier::from_outcome(&outcome).with_sudo(tiers::SudoProvenance::Nopasswd);
        let yaml = serde_yaml_ng::to_string(&nopasswd).unwrap();
        assert!(yaml.contains("sudo: nopasswd"), "{yaml}");
        assert!(!yaml.contains("primed"), "{yaml}");

        let primed = Tier::from_outcome(&outcome).with_sudo(tiers::SudoProvenance::Primed);
        let yaml = serde_yaml_ng::to_string(&primed).unwrap();
        assert!(yaml.contains("sudo: primed"), "{yaml}");
        assert!(!yaml.contains("nopasswd"), "{yaml}");

        // No provenance claimed: the field is absent, not defaulted — a
        // skipped tier and the docker tier must never carry one.
        let unclaimed = Tier::skipped("missing: passwordless sudo");
        let yaml = serde_yaml_ng::to_string(&unclaimed).unwrap();
        assert!(!yaml.contains("sudo:"), "{yaml}");
    }

    /// A tier whose repository is not in the set is skipped naming it: the
    /// docker tier without a `zl-zpr-demo` worktree has nothing to deploy,
    /// and that absence must be stated, never silent (spec-003 §6).
    #[test]
    fn execute_build_skips_a_tier_whose_repository_is_not_in_the_set() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        // The fixture set holds only zl-zpr-core: no zl-zpr-demo worktree.
        let recipes = vec![ok_recipe()];
        let selection = tiers::Selection::parse(Some("docker")).unwrap();
        let in_ = inputs(
            &set, &resolved, &workspace, &manifest, &build_dir, &recipes, true, false, &selection,
        );
        let ok = execute_build(&in_).unwrap();
        assert!(ok, "a repository-absent skip must not fail the run");

        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        assert!(text.contains("docker:"), "{text}");
        assert!(text.contains("status: skipped"), "{text}");
        assert!(text.contains("zl-zpr-demo"), "{text}");
    }

    // -- coverage summary: tests_skipped and notes (zipline#87) ----------------

    /// A scan directory whose `Cargo.toml` pins `zpr` at `tag`.
    fn pinned_dir(root: &Path, name: &str, tag: &str) -> (String, PathBuf) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
                 zpr = {{ git = \"https://github.com/mkolehmainen/zl-zpr-common.git\", tag = \"{tag}\" }}\n"
            ),
        )
        .unwrap();
        (name.to_string(), dir)
    }

    /// The gate outcome counts the disagreements `--allow-pin-drift`
    /// actually downgraded — not whether the flag was passed (PR #22 Codex
    /// P2): agreeing pins under the flag downgrade nothing, a disagreement
    /// under the flag downgrades one, and without the flag it is an error.
    #[test]
    fn gate_outcome_counts_only_real_downgrades() {
        let tmp = tempfile::tempdir().unwrap();
        let agree = vec![
            pinned_dir(tmp.path(), "a", "v0.26.0"),
            pinned_dir(tmp.path(), "b", "v0.26.0"),
        ];
        let outcome = collect_gate_findings(&agree, &[], true, tmp.path()).unwrap();
        assert_eq!(outcome.pin_drift_downgraded, 0, "{:#?}", outcome.findings);

        let disagree = vec![
            pinned_dir(tmp.path(), "c", "v0.26.0"),
            pinned_dir(tmp.path(), "d", "v0.27.0"),
        ];
        let outcome = collect_gate_findings(&disagree, &[], true, tmp.path()).unwrap();
        assert_eq!(outcome.pin_drift_downgraded, 1, "{:#?}", outcome.findings);

        let outcome = collect_gate_findings(&disagree, &[], false, tmp.path()).unwrap();
        assert_eq!(outcome.pin_drift_downgraded, 0, "{:#?}", outcome.findings);
        assert!(
            outcome
                .findings
                .iter()
                .any(|f| f.severity == gates::Severity::Error),
            "{:#?}",
            outcome.findings
        );
    }

    /// `--allow-pin-drift` on a set whose pins already agree must not make
    /// the manifest claim disagreements were downgraded (PR #22 Codex P2).
    #[test]
    fn execute_build_allow_pin_drift_without_drift_adds_no_note() {
        let (_tmp, workspace, _sha) = workspace_with_repo();
        let set = one_repo_set("main");
        let resolved = resolve_set(&workspace, &set).unwrap();
        let manifest = workspace_manifest();
        let tmp = tempfile::tempdir().unwrap();
        let build_dir = tmp.path().join("t");
        prepare_build_dir(&build_dir, false, &workspace, &["zl-zpr-core"]).unwrap();

        let recipes = vec![ok_recipe()];
        let selection = no_tiers();
        let mut in_ = inputs(
            &set, &resolved, &workspace, &manifest, &build_dir, &recipes, true, false, &selection,
        );
        in_.downgrade_pin_drift = true;
        assert!(execute_build(&in_).unwrap());

        let text = std::fs::read_to_string(build_dir.join("dist").join("zpr-set-t.yaml")).unwrap();
        assert!(!text.contains("allow-pin-drift"), "{text}");
    }

    fn passed() -> Tier {
        Tier::from_outcome(&tiers::TierOutcome {
            passed: true,
            repos: BTreeMap::new(),
        })
    }

    fn failed() -> Tier {
        Tier::from_outcome(&tiers::TierOutcome {
            passed: false,
            repos: BTreeMap::new(),
        })
    }

    fn results(entries: &[(&str, Tier)]) -> BTreeMap<String, Tier> {
        entries
            .iter()
            .map(|(name, tier)| (name.to_string(), tier.clone()))
            .collect()
    }

    /// A run that selected everything and passed everything has nothing to
    /// confess: both fields are empty, so they are omitted from the YAML.
    #[test]
    fn coverage_is_silent_on_a_clean_full_run() {
        let selection = tiers::Selection::parse(None).unwrap();
        let tiers = results(&[
            ("unit", passed()),
            ("netns", passed()),
            ("docker", passed()),
        ]);
        let coverage = coverage_summary(&selection, &tiers, false, false, &[]);
        assert!(coverage.skipped.is_empty(), "{coverage:?}");
        assert!(coverage.notes.is_empty(), "{coverage:?}");
    }

    /// `--test unit` leaves the two end-to-end tiers unselected: each is
    /// recorded with the request that left it out, and the notes say in
    /// plain words that the integration tests did not run — in tier order.
    #[test]
    fn coverage_marks_unselected_tiers_not_selected() {
        let selection = tiers::Selection::parse(Some("unit")).unwrap();
        let tiers = results(&[("unit", passed())]);
        let coverage = coverage_summary(&selection, &tiers, false, false, &[]);
        assert_eq!(
            coverage.skipped,
            BTreeMap::from([
                (
                    "netns".to_string(),
                    "not selected (--test unit)".to_string()
                ),
                (
                    "docker".to_string(),
                    "not selected (--test unit)".to_string()
                ),
            ])
        );
        assert_eq!(
            coverage.notes,
            vec![
                "netns integration tests did NOT run: not selected (--test unit)",
                "docker end-to-end tests did NOT run: not selected (--test unit)",
            ]
        );
    }

    /// `--test none` is one note, not three: the reader needs the fact, not
    /// the enumeration. `tests_skipped` still lists every tier.
    #[test]
    fn coverage_test_none_is_a_single_note() {
        let selection = tiers::Selection::parse(Some("none")).unwrap();
        let coverage = coverage_summary(&selection, &BTreeMap::new(), false, false, &[]);
        assert_eq!(coverage.skipped.len(), 3, "{coverage:?}");
        assert_eq!(coverage.skipped["unit"], "not selected (--test none)");
        assert_eq!(coverage.notes, vec!["no tests ran (--test none)"]);
    }

    /// A probe-skipped tier keeps the probe's reason — the same text as
    /// `tiers.<name>.reason` — so the two never disagree.
    #[test]
    fn coverage_reuses_a_probe_skip_reason() {
        let selection = tiers::Selection::parse(None).unwrap();
        let tiers = results(&[
            ("unit", passed()),
            (
                "netns",
                Tier::skipped("missing: passwordless sudo, valkey-server"),
            ),
            ("docker", passed()),
        ]);
        let coverage = coverage_summary(&selection, &tiers, false, false, &[]);
        assert_eq!(
            coverage.skipped,
            BTreeMap::from([(
                "netns".to_string(),
                "missing: passwordless sudo, valkey-server".to_string()
            )])
        );
        assert_eq!(
            coverage.notes,
            vec!["netns integration tests did NOT run: missing: passwordless sudo, valkey-server"]
        );
    }

    /// A failed tier is not "skipped" — it ran — but it is a shortcoming the
    /// notes must name, after the not-run lines.
    #[test]
    fn coverage_notes_a_failed_tier_after_the_not_run_lines() {
        let selection = tiers::Selection::parse(Some("unit")).unwrap();
        let tiers = results(&[("unit", failed())]);
        let coverage = coverage_summary(&selection, &tiers, false, false, &[]);
        assert!(!coverage.skipped.contains_key("unit"), "{coverage:?}");
        assert_eq!(
            coverage.notes,
            vec![
                "netns integration tests did NOT run: not selected (--test unit)",
                "docker end-to-end tests did NOT run: not selected (--test unit)",
                "unit tests FAILED; see tiers.unit.repos",
            ]
        );
    }

    /// When the build itself failed no tier ran, whatever was selected: the
    /// selected tiers are recorded not run for that reason, and the
    /// unselected ones keep their own.
    #[test]
    fn coverage_build_failure_marks_selected_tiers_not_run() {
        let selection = tiers::Selection::parse(Some("unit,netns")).unwrap();
        let coverage = coverage_summary(&selection, &BTreeMap::new(), true, false, &[]);
        assert_eq!(coverage.skipped["unit"], "not run: build failed");
        assert_eq!(coverage.skipped["netns"], "not run: build failed");
        assert_eq!(
            coverage.skipped["docker"],
            "not selected (--test unit,netns)"
        );
        assert_eq!(coverage.notes.len(), 3, "{coverage:?}");
    }

    /// A gate-1 downgrade and the operator's own notes close the list, in
    /// that order, the operator's text verbatim.
    #[test]
    fn coverage_ends_with_pin_drift_then_operator_notes() {
        let selection = tiers::Selection::parse(None).unwrap();
        let tiers = results(&[
            ("unit", passed()),
            ("netns", passed()),
            ("docker", passed()),
        ]);
        let operator = vec!["first".to_string(), "second: with punctuation!".to_string()];
        let coverage = coverage_summary(&selection, &tiers, false, true, &operator);
        assert!(coverage.skipped.is_empty());
        assert_eq!(
            coverage.notes,
            vec![
                "gate 1 pin disagreements were downgraded to warnings (--allow-pin-drift)",
                "first",
                "second: with punctuation!",
            ]
        );
    }
}
