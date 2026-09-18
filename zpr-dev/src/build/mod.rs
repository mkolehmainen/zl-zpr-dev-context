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
}

/// Creates the build directory `<build_dir>` with `logs/` and `dist/` inside.
/// A directory left over from a previous run is refused naming the path and
/// `--force` (approved decision on zipline#60: reuse of half-built state is
/// how silent staleness gets shipped); `force` removes it entirely and
/// recreates it fresh.
fn prepare_build_dir(dir: &Path, force: bool) -> Result<()> {
    if dir.exists() {
        if !force {
            bail!(
                "build directory {} already exists from a previous run; \
                 rerun with --force to remove and rebuild it",
                dir.display()
            );
        }
        std::fs::remove_dir_all(dir)
            .map_err(|e| anyhow::anyhow!("cannot remove {}: {e}", dir.display()))?;
    }
    for sub in ["logs", "dist"] {
        std::fs::create_dir_all(dir.join(sub))
            .map_err(|e| anyhow::anyhow!("cannot create {}/{sub}: {e}", dir.display()))?;
    }
    Ok(())
}

/// The `build` command (spec-003 §7). At this stage `--dry-run` resolves and
/// reports (B1), and `--gates-only` runs the three compatibility gates of §4
/// against the live checkouts (B2); worktrees, builds and tiers land with
/// B3-B5.
pub fn run(ctx: &crate::Ctx, args: &BuildArgs) -> Result<std::process::ExitCode> {
    // Flags whose stages have not landed parse but are inert (spec-003 §7.1);
    // saying so beats silently ignoring them.
    for (flag, set) in [
        ("--test", args.test.is_some()),
        ("--repo", args.repo.is_some()),
        ("--keep", args.keep),
        ("--no-tarball", args.no_tarball),
    ] {
        if set && !ctx.quiet {
            println!("note: {flag} parses but is inert until its stage (B3-B5) lands");
        }
    }

    if args.gates_only {
        return run_gates(ctx, args);
    }

    if !ctx.dry_run {
        bail!(
            "only `build --dry-run` and `build --gates-only` are implemented at \
             this stage (B2); worktrees and builds land with B3 (spec-003 §1.2)"
        );
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

    // The manifest that a real run would write (B3): built here so the shape
    // is exercised end to end, printed under --verbose, never written.
    let emitted = emit(&set, &resolved, args.tip)?;
    report_dry_run(
        ctx,
        &set.name,
        &resolved,
        &skipped,
        args.build_dir.as_deref(),
    );
    if ctx.verbose && !ctx.quiet {
        println!();
        println!("emitted manifest (would be written by B3):");
        print!("{}", emitted_yaml(&emitted)?);
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// The `build --gates-only` path (spec-003 §4, stage B2): runs the three
/// compatibility gates against the live checkouts — worktrees arrive with B3 —
/// and prints one `zpr-dev validate`-style report. Read-only by construction:
/// it parses manifests and lists tags, and never fetches or builds.
fn run_gates(ctx: &crate::Ctx, args: &BuildArgs) -> Result<std::process::ExitCode> {
    let manifest = crate::config::load(&ctx.context.join(crate::config::MANIFEST_FILE))?;

    // The repositories whose manifests the gates scan, and the tolerated
    // drift entries, come from the build set — synthesized under `--tip`
    // exactly as the dry-run path does (spec-003 §2.3).
    let (mut scan, drift, set_name) = if args.tip {
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

    // `zl-zpr-common` never appears in a build set (spec-003 §2.1: its
    // version is *derived* from what the consumers agree on), but its own
    // manifest participates in coherence — the real `rcu` divergence is
    // between `zl-zpr-common` and `adapter/ph`, and a crate reached both
    // directly and through `zpr` ends up in one binary twice. Scan it
    // whenever the workspace declares it. Nothing else is added: the
    // `zl-zpr-utils` checkout is deliberately not scanned, because no build
    // in this workspace consumes it (workspace.yaml's own note).
    const DERIVED: &str = "zl-zpr-common";
    if manifest.repo(DERIVED).is_some() && !scan.iter().any(|name| name == DERIVED) {
        scan.push(DERIVED.to_string());
    }

    let mut findings: Vec<gates::Finding> = Vec::new();

    // -- pin extraction over every scanned checkout (gate 1 input) ----------
    let mut pins: Vec<gates::PinOccurrence> = Vec::new();
    for name in &scan {
        let dir = ctx.workspace.join(name);
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

    // -- gate 1: agreement ---------------------------------------------------
    findings.extend(gates::gate_pin_agreement(
        &pins,
        &drift,
        args.allow_pin_drift,
    ));

    // -- gate 2: freshness, against the workspace checkouts' tags -----------
    findings.extend(gates::gate_freshness(&pins, |url| {
        // The pinned repository's checkout, located by the URL's repository
        // name. Pins use https URLs while workspace.yaml uses ssh, so the
        // name is the join point.
        let name = url
            .rsplit('/')
            .next()
            .map(|last| last.strip_suffix(".git").unwrap_or(last))?;
        let dir = ctx.workspace.join(name);
        if !crate::git::is_repo(&dir) {
            return None;
        }
        crate::git::tag_list(&dir).ok()
    }));

    // -- gate 3: zplc vs the visa service's POLICY_MIN_COMPILER --------------
    // Runs when both repositories are checked out; a missing checkout is
    // reported rather than silently narrowing coverage (spec-003 §4.3). An
    // unreadable value in a present file is an error inside the parsers.
    let vs_config = ctx
        .workspace
        .join("zl-zpr-visaservice")
        .join("vs")
        .join("src")
        .join("config.rs");
    let zplc_manifest = ctx.workspace.join("zl-zpr-compiler").join("Cargo.toml");
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

    // -- the report, in `zpr-dev validate` style (spec-003 §4) ---------------
    let mut errors = 0usize;
    let mut warnings = 0usize;
    if !ctx.quiet {
        println!("compatibility gates: build set {set_name}");
        println!();
    }
    for finding in &findings {
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
        if ctx.quiet {
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
    if !ctx.quiet {
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
    Ok(if errors > 0 {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    })
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<Pin>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub versions: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub binaries: Vec<Binary>,
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

/// One agreed pin, recomputed by gate 1. Constructed by B2; the shape is fixed
/// here so the emitted manifest's contract is complete in one document.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct Pin {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub url: String,
    pub tag: String,
    pub pinned_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_available: Option<String>,
}

/// One staged binary's identity. Constructed by B3; shape fixed here.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct Binary {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
    pub from: String,
}

/// One tier's outcome. A skipped tier always carries its reason. Constructed
/// by B4–B5; shape fixed here.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct Tier {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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

    // -- build directory lifecycle (task B3 step 2) ---------------------------

    /// A fresh build directory is created with `logs/` and `dist/` inside.
    #[test]
    fn prepare_build_dir_creates_logs_and_dist() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("tip");
        prepare_build_dir(&dir, false).unwrap();
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

        let error = prepare_build_dir(&dir, false).unwrap_err().to_string();
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

        prepare_build_dir(&dir, true).unwrap();
        assert!(!dir.join("dist").join("stale-binary").exists());
        assert!(dir.join("logs").is_dir());
        assert!(dir.join("dist").is_dir());
    }
}
