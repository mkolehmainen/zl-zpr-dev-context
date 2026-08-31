//! Command implementations. Every command receives the resolved [`Ctx`] and
//! returns the process exit code it wants (spec §6.4): `0` success, `1`
//! validation errors, `2` command or configuration error (reported as an
//! `anyhow::Error` instead, which `main` maps to `2`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

use crate::Ctx;
use crate::config::{self, Manifest};
use crate::generate::{self, Action, RepoPlan};
use crate::{git, hermes};

/// Loads the workspace manifest from the context checkout. Every command that
/// needs the manifest goes through here, so the path lives in one place.
fn load_manifest(ctx: &Ctx) -> Result<Manifest> {
    config::load(&ctx.context.join(config::MANIFEST_FILE))
}

/// Prints a progress line unless `--quiet` (spec §5.1). Findings printed by
/// `validate` are not progress and do not go through here.
fn report(ctx: &Ctx, message: impl AsRef<str>) {
    if !ctx.quiet {
        println!("{}", message.as_ref());
    }
}

/// Creates the workspace directory if it is absent (spec §5.2 step 1).
fn ensure_workspace(ctx: &Ctx) -> Result<()> {
    if ctx.workspace.is_dir() {
        return Ok(());
    }
    if ctx.dry_run {
        report(
            ctx,
            format!("would create workspace {}", ctx.workspace.display()),
        );
        return Ok(());
    }
    std::fs::create_dir_all(&ctx.workspace)
        .with_context(|| format!("cannot create workspace {}", ctx.workspace.display()))
}

/// Clones `url` into `dir` when that directory is absent, and returns whether a
/// clone happened — or would have, under `--dry-run`. An existing directory is
/// left *entirely* alone: no fetch, no checkout, no branch change (spec §5.2).
fn clone_if_absent(ctx: &Ctx, url: &str, dir: &Path, branch: Option<&str>) -> Result<bool> {
    if dir.is_dir() {
        if ctx.verbose {
            report(
                ctx,
                format!("{} already present, left untouched", dir.display()),
            );
        }
        return Ok(false);
    }
    if ctx.dry_run {
        report(ctx, format!("would clone {url} into {}", dir.display()));
        return Ok(true);
    }
    report(ctx, format!("cloning {url} into {}", dir.display()));
    git::clone(url, dir, branch).with_context(|| format!("cannot clone {url}"))?;
    Ok(true)
}

/// Clones the workspace, generates context files, then validates (spec §5.2).
pub fn setup(
    ctx: &Ctx,
    context_url: &str,
    branch: Option<&str>,
    no_clone: bool,
) -> Result<ExitCode> {
    ensure_workspace(ctx)?;
    clone_if_absent(ctx, context_url, &ctx.context, branch)?;

    // Under `--dry-run` the context clone did not actually happen, so there is
    // no manifest to read and no repository set to report: the intended clone is
    // the whole of the output (spec §5.1).
    if !ctx.context.is_dir() {
        report(
            ctx,
            "would then read the workspace manifest from the cloned context repository",
        );
        return Ok(ExitCode::SUCCESS);
    }

    let manifest = load_manifest(ctx)?;

    let mut cloned = 0;
    let mut skipped = 0;
    for repo in &manifest.repositories {
        let dir = ctx.workspace.join(&repo.name);
        // `--no-clone` skips cloning entirely; an already-present checkout is
        // untouched either way.
        if no_clone && !dir.is_dir() {
            skipped += 1;
            continue;
        }
        if clone_if_absent(ctx, &repo.url, &dir, Some(&repo.default_branch))? {
            cloned += 1;
        }
    }
    let present = manifest.repositories.len() - cloned - skipped;
    let verb = if ctx.dry_run { "would clone" } else { "cloned" };
    report(
        ctx,
        format!("{verb} {cloned} repositories, {present} already present, {skipped} skipped"),
    );

    generate_context(ctx, &manifest)?;
    validate(ctx)
}

/// The context checkout's display name: its directory name, which is what
/// `status` shows in the table and what `update --repo` accepts.
fn context_name(ctx: &Ctx) -> String {
    generate::absolute(&ctx.context)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| config::CONTEXT_DIR_NAME.to_string())
}

/// The repositories `update` targets (spec §5.3): the context checkout alone by
/// default, plus every manifest repository under `--all`, or exactly the one
/// named by `--repo` — which may be the context checkout. An unknown name is an
/// error rather than empty output, as in `status`.
fn update_targets(
    ctx: &Ctx,
    manifest: &Manifest,
    all: bool,
    repo: Option<&str>,
) -> Result<Vec<(String, PathBuf)>> {
    let mut targets = vec![(context_name(ctx), ctx.context.clone())];
    if all || repo.is_some() {
        for entry in &manifest.repositories {
            targets.push((entry.name.clone(), ctx.workspace.join(&entry.name)));
        }
    }
    if let Some(wanted) = repo {
        targets.retain(|(name, _)| name == wanted);
        if targets.is_empty() {
            bail!("unknown repository: {wanted}");
        }
    }
    Ok(targets)
}

/// Fetches and fast-forwards one repository, returning the line to report
/// (spec §5.3). Every skip condition leaves the repository byte-identical: the
/// only mutating calls are `git fetch` and `git merge --ff-only`, and both are
/// reached only after the checks above them pass.
fn update_one(ctx: &Ctx, dir: &Path) -> Result<String> {
    if !git::is_repo(dir) {
        return Ok("skipped, not a git repository".to_string());
    }
    if git::is_dirty(dir)? {
        return Ok("skipped, local modifications".to_string());
    }
    // Checked before the upstream, because `ahead_behind` reports `None` for a
    // detached `HEAD` too and the two deserve different reasons.
    if git::branch(dir)?.is_none() {
        return Ok("skipped, detached HEAD".to_string());
    }
    if git::ahead_behind(dir)?.is_none() {
        return Ok("skipped, no upstream".to_string());
    }
    if ctx.dry_run {
        return Ok("would fetch and fast-forward".to_string());
    }

    git::fetch(dir).with_context(|| format!("cannot fetch in {}", dir.display()))?;
    let before = git::head_short(dir)?;
    // The current branch is fast-forwarded whatever it is; `default_branch` is
    // used only when cloning (spec §5.3). An upstream that is already an
    // ancestor makes this a no-op, which is the `current` case below.
    if !git::ff_merge(dir)? {
        return Ok("skipped, cannot fast-forward".to_string());
    }
    let after = git::head_short(dir)?;
    Ok(if before == after {
        "current".to_string()
    } else {
        format!("{before} -> {after}")
    })
}

/// Fast-forwards the targeted repositories, then regenerates (spec §5.3).
pub fn update(ctx: &Ctx, all: bool, repo: Option<&str>, no_generate: bool) -> Result<ExitCode> {
    let manifest = load_manifest(ctx)?;
    for (name, dir) in update_targets(ctx, &manifest, all, repo)? {
        let outcome = update_one(ctx, &dir)?;
        report(ctx, format!("{name}: {outcome}"));
    }
    // Regeneration picks up the new context `HEAD` in the generated header.
    if !no_generate {
        generate_context(ctx, &manifest)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// One row of the `status` repository table (spec §5.4).
struct RepoStatus {
    name: String,
    /// Branch name, `detached`, or `-` when there is no repository to ask.
    branch: String,
    /// `clean` or `modified` for a checkout; `missing` or `not a git repository`
    /// otherwise, which §5.4's two values do not cover.
    state: &'static str,
    /// Commits ahead of, and behind, the upstream; `None` when there is none.
    tracking: Option<(usize, usize)>,
}

impl RepoStatus {
    /// The UPSTREAM column, derived from `tracking` so the human table and the
    /// porcelain record cannot disagree.
    fn upstream(&self) -> String {
        match self.tracking {
            None => "no upstream".to_string(),
            Some((0, 0)) => "current".to_string(),
            Some((ahead, 0)) => format!("ahead {ahead}"),
            Some((0, behind)) => format!("behind {behind}"),
            Some((ahead, behind)) => format!("ahead {ahead} behind {behind}"),
        }
    }
}

/// Inspects one checkout. No network access: `tracking` reflects the last fetch
/// (spec §5.4).
fn repo_status(name: &str, dir: &Path) -> Result<RepoStatus> {
    if !git::is_repo(dir) {
        return Ok(RepoStatus {
            name: name.to_string(),
            branch: "-".to_string(),
            state: if dir.is_dir() {
                "not a git repository"
            } else {
                "missing"
            },
            tracking: None,
        });
    }
    Ok(RepoStatus {
        name: name.to_string(),
        branch: git::branch(dir)?.unwrap_or_else(|| "detached".to_string()),
        state: if git::is_dirty(dir)? {
            "modified"
        } else {
            "clean"
        },
        tracking: git::ahead_behind(dir)?,
    })
}

/// The AGENT CONTEXT verdict for one repository, from the Step 5 plan: a file
/// that is absent or differs is stale, and only a byte-identical pair is
/// current (spec §4.6).
fn context_state(action: Action) -> &'static str {
    match action {
        Action::Create | Action::Update => "stale",
        Action::Unchanged => "current",
        Action::Foreign => "not generated by zpr-dev",
        Action::RepoMissing => "missing repository",
    }
}

/// Reports repository and generated-context state; no network access (spec §5.4).
pub fn status(ctx: &Ctx, porcelain: bool, repo: Option<&str>) -> Result<ExitCode> {
    let manifest = load_manifest(ctx)?;
    let plans = generate::plan(ctx, &manifest)?;

    // The context checkout leads the table under its directory name. It holds no
    // generated context of its own, so it appears in no other section.
    let mut rows = vec![repo_status(&context_name(ctx), &ctx.context)?];
    for entry in &manifest.repositories {
        rows.push(repo_status(&entry.name, &ctx.workspace.join(&entry.name))?);
    }

    // `--repo` restricts both sections; the context checkout is a legitimate
    // target even though it has no generated-context record.
    let mut plans: Vec<_> = plans.iter().collect();
    if let Some(wanted) = repo {
        rows.retain(|row| row.name == wanted);
        plans.retain(|plan| plan.name == wanted);
        if rows.is_empty() {
            bail!("unknown repository: {wanted}");
        }
    }

    if porcelain {
        print_porcelain(&rows, &plans);
    } else {
        print_table(&ctx.workspace, &rows, &plans);
    }
    Ok(ExitCode::SUCCESS)
}

/// The human table of §5.4: a WORKSPACE line, the repository table, then the
/// generated-context verdicts.
fn print_table(workspace: &Path, rows: &[RepoStatus], plans: &[&RepoPlan]) {
    println!("WORKSPACE {}", workspace.display());

    // Column widths follow the widest value, headers included, so the table
    // stays aligned whatever the repository names are.
    let name_width = width("REPOSITORY", rows.iter().map(|row| row.name.len()));
    let branch_width = width("BRANCH", rows.iter().map(|row| row.branch.len()));
    let state_width = width("STATUS", rows.iter().map(|row| row.state.len()));

    println!();
    println!(
        "{:<name_width$}  {:<branch_width$}  {:<state_width$}  UPSTREAM",
        "REPOSITORY", "BRANCH", "STATUS"
    );
    for row in rows {
        println!(
            "{:<name_width$}  {:<branch_width$}  {:<state_width$}  {}",
            row.name,
            row.branch,
            row.state,
            row.upstream()
        );
    }

    if !plans.is_empty() {
        println!();
        println!("AGENT CONTEXT");
        for plan in plans {
            println!("{:<name_width$}  {}", plan.name, context_state(plan.action));
        }
    }
}

/// The widest of a header and a column's values.
fn width(header: &str, values: impl Iterator<Item = usize>) -> usize {
    values
        .chain(std::iter::once(header.len()))
        .max()
        .unwrap_or(0)
}

/// Machine-readable records, tab-separated. The field order is contract and
/// will not change within v0.x (spec §5.4):
///
/// ```text
/// repo   <name>  <branch>  <clean|modified|missing|not a git repository>  <ahead>  <behind>
/// agent  <name>  <current|stale|missing repository>
/// ```
///
/// `<branch>` is `-` when there is no repository, `detached` when `HEAD` is
/// detached; `<ahead>` and `<behind>` are `-` when there is no upstream.
fn print_porcelain(rows: &[RepoStatus], plans: &[&RepoPlan]) {
    for row in rows {
        let (ahead, behind) = match row.tracking {
            Some((ahead, behind)) => (ahead.to_string(), behind.to_string()),
            None => ("-".to_string(), "-".to_string()),
        };
        println!(
            "repo\t{}\t{}\t{}\t{ahead}\t{behind}",
            row.name, row.branch, row.state
        );
    }
    for plan in plans {
        println!("agent\t{}\t{}", plan.name, context_state(plan.action));
    }
}

/// Writes the generated context files; no network access (spec §5.5). The plan
/// and the writing both live in `generate`, so `sync` is just the report.
pub fn sync(ctx: &Ctx) -> Result<ExitCode> {
    let manifest = load_manifest(ctx)?;
    generate_context(ctx, &manifest)?;
    Ok(ExitCode::SUCCESS)
}

/// Renders and writes the generated context files and reports the counts. This
/// is the whole of `sync`; `setup` runs it as its generation step, and so will
/// `update` (spec §4.6).
fn generate_context(ctx: &Ctx, manifest: &Manifest) -> Result<()> {
    let plans = generate::plan(ctx, manifest)?;
    let summary = generate::apply(ctx, &plans)?;

    // `apply` counts files and ignores repositories that were never cloned, so
    // the "not checked out" tally is counted here (spec §4.6).
    let missing = plans
        .iter()
        .filter(|plan| plan.action == Action::RepoMissing)
        .count();

    let verb = if ctx.dry_run { "would write" } else { "wrote" };
    report(
        ctx,
        format!(
            "{verb} generated context: {} created, {} updated, {} unchanged",
            summary.created, summary.updated, summary.unchanged
        ),
    );
    if missing > 0 {
        report(
            ctx,
            format!(
                "skipped {missing} repositor{} not checked out",
                plural_y(missing)
            ),
        );
    }
    // `apply` has already named each file it refused to touch; this is the
    // tally, so a skip cannot be lost in the scroll of a large workspace.
    if summary.skipped_foreign > 0 {
        report(
            ctx,
            format!(
                "left {} file{} alone: not generated by zpr-dev (run: zpr-dev validate)",
                summary.skipped_foreign,
                plural(summary.skipped_foreign)
            ),
        );
    }
    Ok(())
}

/// The running tally of `validate` findings (spec §7). Every check emits its
/// line as it runs, so the report is in check order; only the counts are kept.
#[derive(Default)]
struct Report {
    errors: usize,
    warnings: usize,
}

impl Report {
    /// Prints one finding. The tag column is padded so the messages line up,
    /// `[ERROR]` being the widest tag (spec §7).
    fn line(tag: &str, message: &str) {
        println!("{tag:<6} {message}");
    }

    fn ok(&self, message: impl AsRef<str>) {
        Self::line("[OK]", message.as_ref());
    }

    /// Informational only: nothing is wrong, and the exit code is unaffected.
    fn info(&self, message: impl AsRef<str>) {
        Self::line("[INFO]", message.as_ref());
    }

    fn warn(&mut self, message: impl AsRef<str>) {
        self.warnings += 1;
        Self::line("[WARN]", message.as_ref());
    }

    fn error(&mut self, message: impl AsRef<str>) {
        self.errors += 1;
        Self::line("[ERROR]", message.as_ref());
    }

    /// Prints the closing summary and returns the exit code: `1` when any check
    /// failed, `0` when only warnings were raised (spec §6.4).
    fn finish(self) -> ExitCode {
        println!();
        if self.errors > 0 {
            println!(
                "Validation failed with {} error{} and {} warning{}.",
                self.errors,
                plural(self.errors),
                self.warnings,
                plural(self.warnings)
            );
            return ExitCode::from(1);
        }
        println!(
            "Validation completed with {} warning{}.",
            self.warnings,
            plural(self.warnings)
        );
        ExitCode::SUCCESS
    }
}

/// The `s` of a plural count.
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// The `y`/`ies` of a plural "repository" count.
fn plural_y(count: usize) -> &'static str {
    if count == 1 { "y" } else { "ies" }
}

/// Runs the workspace health checks (spec §7). Every check runs — findings are
/// accumulated rather than stopping at the first — and errors alone decide the
/// exit code. `setup` reuses this as its final step (§5.2).
pub fn validate(ctx: &Ctx) -> Result<ExitCode> {
    let mut report = Report::default();

    // The context checkout holds the manifest and the shared context, so
    // nothing below it can be checked without one.
    if !ctx.context.is_dir() {
        report.error(format!(
            "context repository missing: {}",
            ctx.context.display()
        ));
        return Ok(report.finish());
    }
    if git::is_repo(&ctx.context) {
        report.ok("context repository");
    } else {
        report.error(format!(
            "context repository is not a git repository: {}",
            ctx.context.display()
        ));
    }

    // `config::load` already performs the manifest checks of §7 — parses,
    // `version == 1`, non-empty repositories, unique non-empty names — so there
    // is no second implementation here. It reports the first problem it finds.
    let manifest = match load_manifest(ctx) {
        Ok(manifest) => {
            report.ok("workspace manifest");
            manifest
        }
        Err(err) => {
            report.error(format!("workspace manifest: {err:#}"));
            return Ok(report.finish());
        }
    };

    check_repositories(ctx, &manifest, &mut report);
    check_shared_context(ctx, &manifest, &mut report);
    check_shared_skills(ctx, &manifest, &mut report);

    Ok(report.finish())
}

/// Each manifest repository's checkout, plus the informational tally of
/// repository-specific context files (spec §7). The two failure conditions and
/// their wording are the ones `status` reports in its STATUS column.
fn check_repositories(ctx: &Ctx, manifest: &Manifest, report: &mut Report) {
    let mut healthy = 0;
    let mut with_local = 0;
    for repo in &manifest.repositories {
        let dir = ctx.workspace.join(&repo.name);
        if !dir.is_dir() {
            report.error(format!("{}: missing", repo.name));
        } else if !git::is_repo(&dir) {
            report.error(format!("{}: not a git repository", repo.name));
        } else {
            healthy += 1;
        }
        if dir.join(&repo.context.local).is_file() {
            with_local += 1;
        }
    }
    if healthy == manifest.repositories.len() {
        report.ok(format!("{healthy} source repositories"));
    }
    // Absence is legitimate (spec §7), so this never affects the exit code.
    report.info(format!(
        "repository-specific context in {with_local} of {} repositories",
        manifest.repositories.len()
    ));
}

/// The shared context file, whether the generated files are up to date, and
/// whether its documentation references resolve (spec §7).
fn check_shared_context(ctx: &Ctx, manifest: &Manifest, report: &mut Report) {
    let shared = ctx.context.join(generate::SHARED_CONTEXT_FILE);
    let body = match std::fs::read_to_string(&shared) {
        Ok(body) => body,
        Err(err) => {
            report.error(format!("shared context {}: {err}", shared.display()));
            return;
        }
    };
    report.ok("shared context");

    // Drift is a warning, not an error: a hand-edited generated file and a
    // merely stale one are indistinguishable on disk (spec §1.4.5). Repositories
    // that were never cloned are already reported by `check_repositories`.
    match generate::plan(ctx, manifest) {
        Ok(plans) => {
            // A foreign file is an error rather than drift: `sync` cannot clear
            // it, so it needs a human to rename or delete the file. Named per
            // file, because the fix is per file.
            for plan in &plans {
                for file in &plan.files {
                    if file.action == Action::Foreign {
                        report.error(format!(
                            "{}: {} exists but was not generated by zpr-dev; \
                             rename it to AGENTS.repo.md to have it included, or delete it",
                            plan.name,
                            file.path.display()
                        ));
                    }
                }
            }

            let stale = plans
                .iter()
                .filter(|plan| matches!(plan.action, Action::Create | Action::Update))
                .count();
            if stale == 0 {
                report.ok("generated context");
            } else {
                report.warn(format!(
                    "generated context stale in {stale} repositor{} (run: zpr-dev sync)",
                    plural_y(stale)
                ));
            }
        }
        Err(err) => report.error(format!("generated context: {err:#}")),
    }

    let broken = broken_doc_references(&body, &ctx.context, &manifest.documentation.root);
    if broken.is_empty() {
        report.ok("documentation references");
    } else {
        for reference in broken {
            report.error(format!(
                "documentation reference does not resolve: {reference}"
            ));
        }
    }
}

/// The documentation references in `body` that do not resolve to a real file
/// under the context checkout (spec §7). A reference is any token starting with
/// the manifest's documentation root; tokens are split on whitespace and on the
/// punctuation that surrounds a Markdown link, so `[x](docs/A.md)` yields
/// `docs/A.md`. Deliberately independent of §4.4's rewrite list, which names
/// only directories and so cannot say which document beneath one is missing.
fn broken_doc_references(body: &str, context: &Path, docs_root: &str) -> Vec<String> {
    let prefix = format!("{docs_root}/");
    let mut broken: Vec<String> = Vec::new();
    for token in body.split(|c: char| c.is_whitespace() || "()[]<>\"'`,;:!".contains(c)) {
        // A leading `./` is the same reference; a trailing `.` is sentence
        // punctuation, not part of the path.
        let reference = token
            .strip_prefix("./")
            .unwrap_or(token)
            .trim_end_matches('.');
        // `exists`, not `is_file`: a reference to the documentation
        // directory itself (`docs/`) names a real thing and is not broken.
        if !reference.starts_with(&prefix) || context.join(reference).exists() {
            continue;
        }
        if !broken.iter().any(|seen| seen == reference) {
            broken.push(reference.to_string());
        }
    }
    broken
}

/// `agent.hermes.shared_skills`, checked for existence only — nothing else acts
/// on it in v0.1 (spec §7). Absent from the manifest means nothing to check.
fn check_shared_skills(ctx: &Ctx, manifest: &Manifest, report: &mut Report) {
    let Some(relative) = manifest
        .agent
        .hermes
        .as_ref()
        .and_then(|hermes| hermes.shared_skills.as_deref())
    else {
        return;
    };
    let dir = ctx.context.join(relative);
    if dir.is_dir() {
        report.ok(format!("shared skills {relative}"));
    } else {
        report.warn(format!(
            "shared skills directory missing: {} (declared as agent.hermes.shared_skills)",
            dir.display()
        ));
    }
}

// ---------------------------------------------------------------------------
// `agent` (spec-002)
// ---------------------------------------------------------------------------

/// The user's home directory, which is where the Hermes configuration lives
/// (spec-002 §3.3). `main` defaults an unset `$HOME` to empty for workspace
/// resolution; here it has to be a hard error, because there is no sensible
/// fallback for another program's configuration file.
fn home_dir() -> Result<PathBuf> {
    match std::env::var("HOME") {
        Ok(value) if !value.trim().is_empty() => Ok(PathBuf::from(value)),
        _ => bail!("cannot locate the hermes configuration: $HOME is not set"),
    }
}

/// The absolute shared skills directory the manifest declares, or `None` when it
/// declares none (spec-002 §2). Existence is *not* checked here: `configure`
/// treats an absent directory as an error and `status` reports it, so the check
/// belongs to each caller.
fn shared_skills(ctx: &Ctx, manifest: &Manifest) -> Option<PathBuf> {
    let relative = manifest
        .agent
        .hermes
        .as_ref()
        .and_then(|hermes| hermes.shared_skills.as_deref())?;
    // Absolute, through the same helper `status` and generation use, so the two
    // cannot disagree about what "absolute" means (spec-002 §2).
    Some(generate::absolute(&ctx.context.join(relative)))
}

/// Reads the Hermes configuration. An absent file is the expected failure on a
/// machine where Hermes has been installed but never started, so its message
/// carries the remedy rather than leaving the developer to guess (spec-002 §3.3).
fn read_hermes_config(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => bail!(
            "hermes configuration not found: {} (run hermes once to create it)",
            path.display()
        ),
        Err(err) => bail!("cannot read {}: {err}", path.display()),
    }
}

/// Copies the configuration aside before it is rewritten, overwriting any
/// previous backup (spec-002 §4.5). `zpr-dev` does not own this file, so the
/// developer gets an undo even though the verification in `hermes::verify` makes
/// a damaging edit unreachable.
fn back_up(path: &Path) -> Result<()> {
    let backup = suffixed(path, ".bak");
    std::fs::copy(path, &backup)
        .with_context(|| format!("cannot write backup {}", backup.display()))?;
    Ok(())
}

/// Writes through a temporary file in the same directory and renames it over the
/// original, so a process that dies mid-write cannot leave Hermes with a
/// truncated configuration (spec-002 §4.5).
fn write_atomically(path: &Path, text: &str) -> Result<()> {
    let temporary = suffixed(path, ".tmp");
    std::fs::write(&temporary, text)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))
}

/// `path` with `suffix` appended to its file name — `config.yaml` plus `.bak`
/// gives `config.yaml.bak`, keeping the sibling files next to the original.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Prints the changed region as a unified-diff hunk, for `--dry-run --verbose`
/// (spec-002 §3.2). The edit only ever inserts lines or replaces one line with
/// two, so the common prefix and suffix of the two line lists bound the change
/// exactly and no general diff algorithm is needed.
fn print_hunk(before: &str, after: &str) {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();

    let head = std::iter::zip(&old, &new)
        .take_while(|(a, b)| a == b)
        .count();
    let tail = std::iter::zip(old[head..].iter().rev(), new[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    println!(
        "@@ -{},{} +{},{} @@",
        head + 1,
        old.len() - head - tail,
        head + 1,
        new.len() - head - tail
    );
    for line in &old[head..old.len() - tail] {
        println!("-{line}");
    }
    for line in &new[head..new.len() - tail] {
        println!("+{line}");
    }
}

/// Adds the workspace's shared skills directory to Hermes' `skills.external_dirs`
/// (spec-002 §3). Every failure leaves the file byte-identical: the edit is
/// computed and verified in full before the first byte is written.
pub fn agent_configure_hermes(ctx: &Ctx) -> Result<ExitCode> {
    let manifest = load_manifest(ctx)?;

    let Some(skills) = shared_skills(ctx, &manifest) else {
        bail!("manifest declares no agent.hermes.shared_skills; nothing to configure");
    };
    // Pointing Hermes at a directory that is not there is worse than doing
    // nothing. `validate` keeps treating the same condition as a warning, because
    // a stale manifest entry should not fail an otherwise healthy workspace.
    if !skills.is_dir() {
        bail!("shared skills directory missing: {}", skills.display());
    }
    let path = skills.to_string_lossy().into_owned();

    let config = hermes::config_path(&home_dir()?);
    let text = read_hermes_config(&config)?;

    // A refusal names the condition; `hermes` supplies the remedy. Prefixing the
    // path is all that is added here, so the message reads as one sentence.
    let edited = hermes::add_external_dir(&text, &path)
        .map_err(|err| anyhow::anyhow!("{}: {err:#}", config.display()))?;

    let Some(edited) = edited else {
        report(
            ctx,
            format!("hermes shared skills already configured: {path}"),
        );
        return Ok(ExitCode::SUCCESS);
    };

    if ctx.dry_run {
        report(ctx, format!("would configure hermes shared skills: {path}"));
        if ctx.verbose {
            print_hunk(&text, &edited);
        }
        return Ok(ExitCode::SUCCESS);
    }

    back_up(&config)?;
    write_atomically(&config, &edited)?;
    report(ctx, format!("configured hermes shared skills: {path}"));
    Ok(ExitCode::SUCCESS)
}

/// Reports Hermes' configuration state (spec-002 §5). Only agents that need
/// global configuration appear: Claude and Codex are configured by the generated
/// `AGENTS.md` and `CLAUDE.md` alone, which the top-level `status` already
/// reports on.
pub fn agent_status(ctx: &Ctx) -> Result<ExitCode> {
    let manifest = load_manifest(ctx)?;
    let config = hermes::config_path(&home_dir()?);
    let skills = shared_skills(ctx, &manifest);

    println!("Hermes");
    field("installed", &installed_field(&config));
    field(
        "shared skills",
        &shared_skills_field(&config, skills.as_deref()),
    );
    field("skill source", &skill_source_field(skills.as_deref()));
    field("context", &context_field(ctx, &manifest));

    // `agent status` reports; it does not judge (spec-002 §5.3).
    Ok(ExitCode::SUCCESS)
}

/// One `agent status` row. The label column is wide enough for the longest
/// label, so the values line up.
fn field(label: &str, value: &str) {
    println!("  {label:<19}{value}");
}

/// Whether Hermes has a configuration file. Deliberately *not* a `$PATH` probe:
/// a developer may have the binary installed under a name we cannot guess, and
/// the configuration file is the thing that actually matters (spec-002 §1.3.2).
/// The path is shown either way, so "no" is never ambiguous about what was
/// looked for.
fn installed_field(config: &Path) -> String {
    if config.is_file() {
        format!("yes ({})", config.display())
    } else {
        format!("no ({} not found)", config.display())
    }
}

/// Whether our shared skills directory is listed in `skills.external_dirs`.
fn shared_skills_field(config: &Path, skills: Option<&Path>) -> String {
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return "not configured".to_string();
        }
        Err(err) => return format!("unreadable: {err}"),
    };
    // A document `configure` would refuse to edit is reported here rather than
    // hidden, since it is the reason `configure` will fail.
    let dirs = match hermes::external_dirs(&text) {
        Ok(dirs) => dirs,
        Err(err) => return format!("unreadable: {err:#}"),
    };

    let ours = skills.map(|dir| dir.to_string_lossy().into_owned());
    if ours
        .as_deref()
        .is_some_and(|our| dirs.iter().any(|dir| dir == our))
    {
        return "configured".to_string();
    }
    if dirs.is_empty() {
        return "not configured".to_string();
    }
    format!(
        "configured elsewhere ({} other director{})",
        dirs.len(),
        plural_y(dirs.len())
    )
}

/// The absolute path `configure` would write.
fn skill_source_field(skills: Option<&Path>) -> String {
    match skills {
        None => "not declared in the manifest".to_string(),
        Some(dir) if !dir.is_dir() => format!("missing: {}", dir.display()),
        Some(dir) => dir.display().to_string(),
    }
}

/// Whether the generated context files are in place, rolled up from the same
/// plan `status` and `validate` consume (spec §4.6) rather than from a second
/// notion of staleness. Reported worst-first, matching the plan's own ordering.
fn context_field(ctx: &Ctx, manifest: &Manifest) -> String {
    let plans = match generate::plan(ctx, manifest) {
        Ok(plans) => plans,
        Err(err) => return format!("unavailable: {err:#}"),
    };

    let foreign = plans
        .iter()
        .flat_map(|plan| &plan.files)
        .filter(|file| file.action == Action::Foreign)
        .count();
    if foreign > 0 {
        return format!(
            "{foreign} file{} not generated by zpr-dev (run: zpr-dev validate)",
            plural(foreign)
        );
    }

    let stale = plans
        .iter()
        .filter(|plan| matches!(plan.action, Action::Create | Action::Update))
        .count();
    if stale > 0 {
        return format!(
            "stale in {stale} repositor{} (run: zpr-dev sync)",
            plural_y(stale)
        );
    }

    let missing = plans
        .iter()
        .filter(|plan| plan.action == Action::RepoMissing)
        .count();
    if missing > 0 {
        return format!("{missing} repositor{} not checked out", plural_y(missing));
    }
    "ready".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference to the documentation directory itself resolves; a reference
    /// to a file that is not there does not.
    #[test]
    fn directory_reference_is_not_broken_but_missing_file_is() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/HERE.md"), "x").unwrap();

        let body = "- `docs/` -> knowledge. See [here](docs/HERE.md) and docs/GONE.md.";
        assert_eq!(
            broken_doc_references(body, dir.path(), "docs"),
            vec!["docs/GONE.md".to_string()]
        );
    }
}
