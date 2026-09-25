//! The three compatibility gates of `zpr-dev build` (spec-003 §4).
//!
//! Gate 1 (§4.1, error): every ZPR-family git dependency pin must agree across
//! the whole set — one `(url, tag-or-rev)` per crate. Gate 2 (§4.2, warning):
//! an agreed pin sitting behind the newest local tag is reported, so being
//! behind is a decision and not an accident. Gate 3 (§4.3, error): the
//! compiler's `[package].version` must satisfy the visa service's
//! `POLICY_MIN_COMPILER_*` under `libeval::pio::check_version`'s rule —
//! major ==, minor ==, patch >=.
//!
//! Everything here is pure with respect to its inputs (manifest text in,
//! findings out) except gate 2's tag listing, which reads the local git
//! checkouts through `crate::git`. Until B3 provides detached worktrees the
//! gates run against the live checkouts behind `build --gates-only`
//! (spec-003 §4); the functions themselves do not care which they are given.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result};

use super::PinDrift;

/// Severity of one gate finding, in `zpr-dev validate` report style
/// (spec-003 §4): only errors decide the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Ok,
    Info,
    Warn,
    Error,
}

/// One finding a gate produced. `message` may span lines; the first line is
/// the summary and continuation lines are already indented for the report.
#[derive(Debug)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

impl Finding {
    /// Builds one finding; callers include the gate orchestration in
    /// `build/mod.rs`, so this is crate-visible.
    pub(crate) fn new(severity: Severity, message: impl Into<String>) -> Finding {
        Finding {
            severity,
            message: message.into(),
        }
    }
}

/// Gate 1 — shared-dep agreement (spec-003 §4.1). Groups the extracted pins
/// by crate and requires a single `(url, tag-or-rev)` pair across the whole
/// set: a crate at two tags is a disagreement, and so is one tag reached from
/// two URLs (the double-`cslab` trap of `docs/BUILD.md`). An `allow_pin_drift`
/// entry suppresses exactly its crate's finding, echoing the reviewed reason;
/// `downgrade` (the `--allow-pin-drift` flag) turns every disagreement into a
/// warning for a one-off.
pub fn gate_pin_agreement(
    pins: &[PinOccurrence],
    drift: &[PinDrift],
    downgrade: bool,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();

    // Group occurrences by crate. A BTreeMap keeps report order deterministic.
    let mut by_crate: BTreeMap<&str, Vec<&PinOccurrence>> = BTreeMap::new();
    for pin in pins {
        by_crate.entry(&pin.crate_name).or_default().push(pin);
    }

    let mut agreeing = 0usize;
    for (crate_name, occurrences) in &by_crate {
        // The distinct (url, kind, reference) triples this crate is pinned
        // at. All three components must be single: two tags is a
        // disagreement, one tag reached from two URLs is a disagreement (the
        // double-`cslab` trap), and the same reference text through two
        // kinds (`tag = "release"` vs `branch = "release"`) is a
        // disagreement too, because git can resolve them to different
        // commits.
        let mut variants: Vec<(&str, RefKind, &str)> = occurrences
            .iter()
            .map(|p| (p.url.as_str(), p.kind, p.reference.as_str()))
            .collect();
        variants.sort_unstable();
        variants.dedup();

        if variants.len() <= 1 {
            agreeing += 1;
            continue;
        }

        // A reviewed allow_pin_drift entry suppresses this crate's finding,
        // echoing its reason so the tolerance is visible in every report.
        if let Some(entry) = drift.iter().find(|d| d.crate_name == *crate_name) {
            findings.push(Finding::new(
                Severity::Info,
                format!(
                    "pin drift allowed for crate `{crate_name}`: {}",
                    entry.reason
                ),
            ));
            continue;
        }

        // The spec-003 §4.1 message: every variant with every file and line
        // that pins it, the kind spelled (`tag v0.26.0`) so same-text
        // variants stay distinguishable, inheritance marked, and the remedy
        // on the last line.
        let mut message = format!("pin disagreement: crate `{crate_name}`");
        for (url, kind, reference) in &variants {
            let pinned_by: Vec<String> = occurrences
                .iter()
                .filter(|p| p.url == *url && p.kind == *kind && p.reference == *reference)
                .map(|p| {
                    let suffix = if p.inherited { " (inherited)" } else { "" };
                    format!("{}:{}{suffix}", p.file, p.line)
                })
                .collect();
            message.push_str(&format!(
                "\n  {} {reference}  {url}  {}",
                kind.label(),
                pinned_by.join(", ")
            ));
        }
        message.push_str(
            "\n  => cut one tag and bump every consumer, or record it in allow_pin_drift",
        );

        let severity = if downgrade {
            Severity::Warn
        } else {
            Severity::Error
        };
        findings.push(Finding::new(severity, message));
    }

    if agreeing > 0 {
        let crates = if agreeing == 1 { "crate" } else { "crates" };
        findings.push(Finding::new(
            Severity::Ok,
            format!("pin agreement: {agreeing} {crates} pinned consistently"),
        ));
    }
    findings
}

/// Gate 2 — freshness (spec-003 §4.2; warning, never an error). For each
/// agreed pin, `local_tags` supplies the tags of the pinned repository's
/// workspace checkout (`None` when it is not checked out — an `INFO`, not a
/// failure, because cargo fetches the tag from GitHub either way). Tags are
/// compared in semantic-version order, not creation order, and only against
/// tags sharing the pinned tag's name prefix, so `rcu-v0.1.2` is never
/// measured against `cslab-v0.1.2`.
pub fn gate_freshness(
    pins: &[PinOccurrence],
    local_tags: impl Fn(&str) -> Option<Vec<String>>,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();

    // Freshness is defined over *agreed* pins (spec-003 §4.2): a crate whose
    // pins disagree is gate 1's error, and warning about staleness on top of
    // it would be noise about a set that is already incoherent. Agreement
    // matches gate 1's variant identity exactly, kind included.
    let mut variants: BTreeMap<&str, Vec<(&str, RefKind, &str)>> = BTreeMap::new();
    for pin in pins {
        let entry = variants.entry(pin.crate_name.as_str()).or_default();
        let variant = (pin.url.as_str(), pin.kind, pin.reference.as_str());
        if !entry.contains(&variant) {
            entry.push(variant);
        }
    }

    // One check per distinct (crate, url, reference): several files pinning
    // the same tag get one line, not one line each.
    let mut seen: Vec<(&str, &str, &str)> = Vec::new();
    for pin in pins {
        // A rev has no version ordering to compare against tags.
        if pin.kind == RefKind::Rev {
            continue;
        }
        if variants[pin.crate_name.as_str()].len() > 1 {
            continue;
        }
        let key = (
            pin.crate_name.as_str(),
            pin.url.as_str(),
            pin.reference.as_str(),
        );
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);

        let Some((prefix, pinned_version)) = split_tag(&pin.reference) else {
            // A tag with no trailing version (e.g. a date stamp) has no
            // ordering either; stay silent rather than guess.
            continue;
        };
        let Some(tags) = local_tags(&pin.url) else {
            findings.push(Finding::new(
                Severity::Info,
                format!(
                    "crate `{}`: no local checkout for {}; freshness not checked \
                     (cargo fetches the tag either way)",
                    pin.crate_name, pin.url
                ),
            ));
            continue;
        };

        // The newest tag sharing this pin's prefix, in semantic-version
        // order — `zl-zpr-utils` tags several crates in one repository, so
        // `rcu-v*` never competes with `zpr-utils-v*` or `cslab-v*`.
        let newest = tags
            .iter()
            .filter_map(|tag| match split_tag(tag) {
                Some((p, version)) if p == prefix => Some(version),
                _ => None,
            })
            .max();
        if let Some(newest) = newest
            && newest > pinned_version
        {
            findings.push(Finding::new(
                Severity::Warn,
                format!(
                    "crate `{}` is pinned at {}; {} has {prefix}{}",
                    pin.crate_name,
                    pin.reference,
                    repo_display(&pin.url),
                    join_version(&newest),
                ),
            ));
        }
    }
    findings
}

/// Splits a tag into its name prefix and trailing dotted-number version:
/// `v0.26.0` → (`v`, [0, 26, 0]), `rcu-v0.1.2` → (`rcu-v`, [0, 1, 2]).
/// `None` when the tag does not end in a dotted number sequence.
/// Crate-visible so the emitted manifest's `newest_available` (spec-003 §3)
/// is computed by the same rule as this gate's warning.
pub(crate) fn split_tag(tag: &str) -> Option<(&str, Vec<u64>)> {
    // The version part is the longest trailing run of digits and dots.
    let start = tag
        .rfind(|c: char| !c.is_ascii_digit() && c != '.')
        .map(|i| i + 1)
        .unwrap_or(0);
    let (prefix, version) = tag.split_at(start);
    if version.is_empty() {
        return None;
    }
    let numbers: Option<Vec<u64>> = version.split('.').map(|part| part.parse().ok()).collect();
    numbers.map(|numbers| (prefix, numbers))
}

/// Re-joins a parsed version for display: [0, 27, 0] → `0.27.0`. Crate-visible
/// alongside [`split_tag`].
pub(crate) fn join_version(version: &[u64]) -> String {
    version
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// The repository name a URL points at, for messages: the last path segment
/// without `.git`. Falls back to the whole URL rather than failing.
fn repo_display(url: &str) -> &str {
    url.rsplit('/')
        .next()
        .map(|last| last.strip_suffix(".git").unwrap_or(last))
        .unwrap_or(url)
}

/// The three components the visa service names in `POLICY_MIN_COMPILER_*`.
pub type Version3 = (u64, u64, u64);

/// Reads `POLICY_MIN_COMPILER_{MAJOR,MINOR,PATCH}` out of the visa service's
/// `vs/src/config.rs` text. An unreadable constant is an error naming the
/// file: nothing else checks this compatibility, so silently skipping it
/// would be worse than failing (spec-003 §4.3).
pub fn policy_min_compiler(config_rs: &str, file: &str) -> Result<Version3> {
    let read = |suffix: &str| -> Result<u64> {
        let name = format!("POLICY_MIN_COMPILER_{suffix}");
        // The declaration is `pub const <name>: u32 = <n>;` — matched
        // structurally (name, then `=`, then integer, then `;`) rather than
        // by exact spacing, so a reformat does not break the gate.
        for line in config_rs.lines() {
            let Some(after_name) = line.split_once(&name).map(|(_, rest)| rest) else {
                continue;
            };
            let Some(value) = after_name.split_once('=').map(|(_, rest)| rest) else {
                continue;
            };
            let value = value.trim().trim_end_matches(';').trim();
            return value.parse().with_context(|| {
                format!("cannot parse `{name}` in {file}: not an integer: {value:?}")
            });
        }
        anyhow::bail!("cannot find `{name}` in {file}")
    };
    Ok((read("MAJOR")?, read("MINOR")?, read("PATCH")?))
}

/// Reads `[package].version` out of the compiler's `Cargo.toml` text. A
/// missing or unparseable version is an error naming the file (spec-003 §4.3).
pub fn package_version(cargo_toml: &str, file: &str) -> Result<Version3> {
    let table: toml::Table = cargo_toml
        .parse()
        .with_context(|| format!("cannot parse {file}"))?;
    let version = table
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .with_context(|| format!("no [package].version in {file}"))?;
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch))) => Ok((major, minor, patch)),
        _ => anyhow::bail!("cannot parse [package].version {version:?} in {file}"),
    }
}

/// Gate 3 — compiler / visa service version (spec-003 §4.3; error). Applies
/// `libeval::pio::check_version`'s rule exactly: **major ==, minor ==,
/// patch >=** (`libeval/src/pio.rs`; pre-1.0, major and minor must match and
/// the compiler's patch must be at least the minimum). The file names appear
/// in the failure message so the fix is obvious.
pub fn gate_compiler_version(
    zplc: Version3,
    zplc_file: &str,
    minimum: Version3,
    vs_file: &str,
) -> Vec<Finding> {
    let (major, minor, patch) = zplc;
    let (min_major, min_minor, min_patch) = minimum;
    let zplc_display = format!("{major}.{minor}.{patch}");
    let min_display = format!("{min_major}.{min_minor}.{min_patch}");

    // The check_version rule, verbatim: majors equal, minors equal, patch at
    // least the minimum.
    let compatible = major == min_major && minor == min_minor && patch >= min_patch;
    if compatible {
        return vec![Finding::new(
            Severity::Ok,
            format!(
                "zplc {zplc_display} satisfies the visa service's \
                 POLICY_MIN_COMPILER {min_display}"
            ),
        )];
    }
    vec![Finding::new(
        Severity::Error,
        format!(
            "zplc {zplc_display} cannot produce policy for this vs\n  \
             {zplc_file}  version = \"{zplc_display}\"\n  \
             {vs_file}  POLICY_MIN_COMPILER = {min_display}\n  \
             rule: major ==, minor ==, patch >=  (libeval/src/pio.rs check_version)"
        ),
    )]
}

/// How a captured git dependency names its source revision. Ordered so it can
/// participate in the sorted variant identity of gate 1: the *kind* is part of
/// what a pin means, because git resolves `tag = "release"` and
/// `branch = "release"` to potentially different commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Tag,
    Rev,
    Branch,
}

impl RefKind {
    /// The manifest key that declares this kind (`tag = ...`), which is how
    /// reports spell it so the reader can find the declaration.
    pub fn label(self) -> &'static str {
        match self {
            RefKind::Tag => "tag",
            RefKind::Rev => "rev",
            RefKind::Branch => "branch",
        }
    }
}

/// One occurrence of a git-pinned dependency in one `Cargo.toml`.
#[derive(Debug, Clone)]
pub struct PinOccurrence {
    /// The crate cargo resolves — the dependency key, or its `package` rename.
    pub crate_name: String,
    /// The `git = <url>` value, verbatim: a URL differing only by host or
    /// owner is a different source on purpose (spec-003 §4.1).
    pub url: String,
    /// The tag, rev or branch value.
    pub reference: String,
    pub kind: RefKind,
    /// Workspace-relative display path of the pinning `Cargo.toml`.
    pub file: String,
    /// 1-based line of the declaration; 0 when it could not be located.
    pub line: usize,
    /// True when the pin reaches this file through `{ workspace = true }`
    /// rather than a literal `git = ...` — attribution matters because a
    /// member the gate cannot see is a member the gate cannot check.
    pub inherited: bool,
}

/// One `Cargo.toml` handed to pin extraction: its display path and its text.
pub struct ManifestSource {
    pub path: String,
    pub text: String,
}

/// One resolved `Cargo.lock` handed to the dual-version scan: its display
/// path and its text.
pub struct LockSource {
    pub path: String,
    pub text: String,
}

/// Gate 1's post-resolution complement (zipline#69): scans each repository's
/// resolved `Cargo.lock` and flags any ZPR-family crate present at two or
/// more versions. Manifest-level pin extraction cannot see a pin made
/// *inside* a tagged git dependency — `zpr-utils-v0.2.2` pinning `zpr` at
/// `v0.8.1` never becomes a `PinOccurrence`, yet both copies ship in the
/// binary — so the lock, which records what cargo actually resolved, is
/// where transitive drift surfaces.
///
/// Known limitation, deliberate (issue #69's option 1): the lock records
/// post-resolution facts, so a finding names the lock and the resolved
/// sources, not the `Cargo.toml` line that pinned the stale version — that
/// file lives inside a tagged artifact this workspace does not check out.
/// No network access, no manifest walking. An unreadable or unparseable
/// lock is an error finding, not a skip: a lock the scan cannot read is a
/// resolution it cannot vouch for.
pub fn gate_lock_dual_versions(locks: &[LockSource]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let mut clean = 0usize;

    for lock in locks {
        let table: toml::Table = match lock.text.parse() {
            Ok(table) => table,
            Err(error) => {
                findings.push(Finding::new(
                    Severity::Error,
                    format!("cannot parse {}: {error}", lock.path),
                ));
                continue;
            }
        };

        // Every ZPR-family package's versions, keyed by name. Only git
        // sources are considered: family membership is a property of the
        // repository URL, and a registry crate cannot be ZPR-family.
        let mut by_name: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
        let packages = table
            .get("package")
            .and_then(toml::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for package in packages {
            let Some(package) = package.as_table() else {
                continue;
            };
            let (Some(name), Some(version)) = (
                package.get("name").and_then(toml::Value::as_str),
                package.get("version").and_then(toml::Value::as_str),
            ) else {
                continue;
            };
            // A path-resolved workspace member has no `source`; it cannot
            // be a duplicated dependency, so it is skipped along with
            // registry crates.
            let Some(source) = package.get("source").and_then(toml::Value::as_str) else {
                continue;
            };
            if is_zpr_family(source) {
                by_name.entry(name).or_default().push((version, source));
            }
        }

        let mut lock_clean = true;
        for (name, mut versions) in by_name {
            versions.sort_unstable();
            versions.dedup();
            if versions.len() <= 1 {
                continue;
            }
            lock_clean = false;
            // The finding names the lock and every resolved (version,
            // source) pair, and states the limitation: the pinning file is
            // inside a tagged artifact, so the remedy points at the family
            // repositories rather than a file and line.
            let mut message = format!(
                "dual-version: crate `{name}` resolved at {} versions in {}",
                versions.len(),
                lock.path
            );
            for (version, source) in &versions {
                message.push_str(&format!("\n  v{version}  {source}"));
            }
            message.push_str(
                "\n  => a tagged ZPR-family dependency pins this crate at a stale version; \
                 fix the pin in that repository and re-tag (the lock cannot name the \
                 pinning file)",
            );
            findings.push(Finding::new(Severity::Error, message));
        }
        if lock_clean {
            clean += 1;
        }
    }

    if clean > 0 {
        let locks_word = if clean == 1 { "lock" } else { "locks" };
        findings.push(Finding::new(
            Severity::Ok,
            format!("lock scan: {clean} {locks_word} free of dual-version ZPR-family crates"),
        ));
    }
    findings
}

/// True when the git URL is ZPR-family — our own forks or upstream — whose
/// pins must agree even without a `rev` (spec-003 §4.1). Both https and ssh
/// spellings are recognized.
fn is_zpr_family(url: &str) -> bool {
    [
        "github.com/mkolehmainen/",
        "github.com:mkolehmainen/",
        "github.com/org-zpr/",
        "github.com:org-zpr/",
    ]
    .iter()
    .any(|owner| url.contains(owner))
}

/// Finds the 1-based line where dependency `key` is declared: the first line
/// starting (after indentation) with the key, followed by a separator so
/// `capnp` never matches `capnp-futures`, and containing `needle` (`"git"` for
/// a literal pin, `"workspace"` for an inherited one) so a registry
/// declaration of the same key elsewhere in the file is not the match.
/// Returns 0 when not found — a message without a line beats no message.
fn dep_line(text: &str, key: &str, needle: &str) -> usize {
    for (index, line) in text.lines().enumerate() {
        if let Some(rest) = line.trim_start().strip_prefix(key) {
            let separated = matches!(rest.chars().next(), Some(' ' | '\t' | '=' | '.'));
            if separated && line.contains(needle) {
                return index + 1;
            }
        }
    }
    0
}

/// Walks every table in a parsed manifest and collects `(key, table)` for each
/// entry carrying a `git` key — dependency tables, `[workspace.dependencies]`
/// and `[patch.*]` alike, without hardcoding table names the manifests might
/// grow past.
fn collect_git_deps<'a>(table: &'a toml::Table, out: &mut Vec<(&'a str, &'a toml::Table)>) {
    for (key, value) in table {
        if let toml::Value::Table(inner) = value {
            if inner.get("git").is_some_and(toml::Value::is_str) {
                out.push((key, inner));
            } else {
                collect_git_deps(inner, out);
            }
        }
    }
}

/// Collects every dependency key declared `{ workspace = true }` (or the
/// dotted `key.workspace = true` spelling, which parses identically).
fn collect_workspace_inherited<'a>(table: &'a toml::Table, out: &mut Vec<&'a str>) {
    for (key, value) in table {
        if let toml::Value::Table(inner) = value {
            if inner.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                out.push(key);
            } else {
                collect_workspace_inherited(inner, out);
            }
        }
    }
}

/// Builds the occurrence for one `git = ...` dependency table, applying the
/// capture rule of spec-003 §4.1: ZPR-family URLs always, any other URL only
/// when pinned by `rev` (the `emilazy/capnproto-rust` fork case). A git
/// dependency floating on its default branch has nothing to compare and is
/// not captured.
fn occurrence(key: &str, dep: &toml::Table, file: &str, text: &str) -> Option<PinOccurrence> {
    let url = dep.get("git")?.as_str()?.to_string();
    let reference = |name: &str| dep.get(name).and_then(toml::Value::as_str);
    let (kind, reference) = if let Some(tag) = reference("tag") {
        (RefKind::Tag, tag)
    } else if let Some(rev) = reference("rev") {
        (RefKind::Rev, rev)
    } else if let Some(branch) = reference("branch") {
        (RefKind::Branch, branch)
    } else {
        return None;
    };
    if !(is_zpr_family(&url) || kind == RefKind::Rev) {
        return None;
    }
    // `package = "..."` renames the dependency key; the crate is what cargo
    // resolves, so it is what the gate groups by.
    let crate_name = dep
        .get("package")
        .and_then(toml::Value::as_str)
        .unwrap_or(key)
        .to_string();
    Some(PinOccurrence {
        crate_name,
        url,
        reference: reference.to_string(),
        kind,
        file: file.to_string(),
        line: dep_line(text, key, "git"),
        inherited: false,
    })
}

/// Extracts every captured pin from one repository's manifests: the root
/// `Cargo.toml` plus each workspace member's. A member declaring
/// `{ workspace = true }` for a crate the root pins by git inherits that pin
/// and the occurrence is attributed to the member — this is how
/// `zl-zpr-core/adapter/ph` gets `zpr`, and missing it would blind the gate
/// to a whole member (spec-003 §4.1).
pub fn extract_pins(
    root: &ManifestSource,
    members: &[ManifestSource],
) -> Result<Vec<PinOccurrence>> {
    let root_table: toml::Table = root
        .text
        .parse()
        .with_context(|| format!("cannot parse {}", root.path))?;

    let mut pins: Vec<PinOccurrence> = Vec::new();

    // The root's own literal pins: dependency tables, the
    // `[workspace.dependencies]` table and any `[patch.*]` table alike.
    let mut root_deps: Vec<(&str, &toml::Table)> = Vec::new();
    collect_git_deps(&root_table, &mut root_deps);
    for (key, dep) in &root_deps {
        if let Some(pin) = occurrence(key, dep, &root.path, &root.text) {
            pins.push(pin);
        }
    }

    // What `{ workspace = true }` in a member resolves to: only the
    // `[workspace.dependencies]` table, never `[patch.*]` — cargo's rule.
    let workspace_deps = root_table
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|ws| ws.get("dependencies"))
        .and_then(toml::Value::as_table);

    for member in members {
        let member_table: toml::Table = member
            .text
            .parse()
            .with_context(|| format!("cannot parse {}", member.path))?;

        // The member's own literal git pins.
        let mut member_deps: Vec<(&str, &toml::Table)> = Vec::new();
        collect_git_deps(&member_table, &mut member_deps);
        for (key, dep) in &member_deps {
            if let Some(pin) = occurrence(key, dep, &member.path, &member.text) {
                pins.push(pin);
            }
        }

        // Inherited pins: each `{ workspace = true }` key whose workspace
        // entry is itself a captured git pin, attributed to the member.
        let Some(workspace_deps) = workspace_deps else {
            continue;
        };
        let mut inherited: Vec<&str> = Vec::new();
        collect_workspace_inherited(&member_table, &mut inherited);
        inherited.sort_unstable();
        inherited.dedup(); // dependencies + dev-dependencies name the same key once
        for key in inherited {
            let Some(dep) = workspace_deps.get(key).and_then(toml::Value::as_table) else {
                continue;
            };
            if let Some(pin) = occurrence(key, dep, &member.path, &member.text) {
                pins.push(PinOccurrence {
                    line: dep_line(&member.text, key, "workspace"),
                    inherited: true,
                    ..pin
                });
            }
        }
    }

    Ok(pins)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures lifted verbatim from the real manifests, so the tests exercise
    // exactly the shapes the gate must read (zipline#59).
    // Line numbers asserted below are line numbers in these constants, which
    // match the source files they were lifted from.

    /// `zl-zpr-core/Cargo.toml`: a workspace root with git pins in
    /// `[workspace.dependencies]` and a rev-pinned non-ZPR fork in
    /// `[patch.crates-io]`.
    const CORE_ROOT: &str = r#"# zpr-core
[workspace]
resolver = "3"
members = ["adapter/admin-api", "adapter/ph", "adapter/cli", "libnode2"]

[workspace.package]
version = "0.7.0"
edition = "2024"
license = "Apache-2.0"

[workspace.dependencies]
aws-lc-rs = "1.17"
capnp = { version = "0.25.0", features = ["sync_reader"] }
capnp-futures = "0.25"
capnp-rpc = "0.25.0"
cbpf-rs = { git = "https://github.com/mkolehmainen/zl-zpr-utils.git", tag = "cbpf-rs-v0.2.0" }
clap = { version = "4.5.23", features = ["derive"] }
clap_complete = "4.5.57"
internet-checksum = "0.2.1"
ipnet = "2.11.0"
pcap = "2.0.0"
strum = { version = "0.27.1", features = ["derive"] }
thiserror = "2.0.18"
tokio = { version = "1.48.0", features = ["full"] }
tokio-util = { version = "0.7.17", features = ["rt", "compat"] }
tracing = "0.1.41"
tracing-subscriber = "0.3.20"
x509-cert = "0.3.0"
zpr = { git = "https://github.com/mkolehmainen/zl-zpr-common.git", tag = "v0.27.0", features = ["vsapi", "policy"] }
zpr-ext = { git = "https://github.com/mkolehmainen/zl-zpr-utils.git", tag = "zpr-ext-v0.5.3" }
zpr-utils = { git = "https://github.com/mkolehmainen/zl-zpr-utils.git", tag = "zpr-utils-v0.2.2" }

[profile.release]
panic = "abort"

[patch.crates-io]
capnp = { git = "https://github.com/emilazy/capnproto-rust.git", rev = "cfbcb9b" }
capnp-futures = { git = "https://github.com/emilazy/capnproto-rust.git", rev = "cfbcb9b" }
capnp-rpc = { git = "https://github.com/emilazy/capnproto-rust.git", rev = "cfbcb9b" }
capnpc = { git = "https://github.com/emilazy/capnproto-rust.git", rev = "cfbcb9b" }
"#;

    /// `zl-zpr-core/adapter/ph/Cargo.toml`: a member with literal git pins of
    /// its own (`cslab`, `rcu`) plus `{ workspace = true }` inheritance of the
    /// root's `zpr`, `zpr-ext`, `zpr-utils` and `cbpf-rs` pins.
    const PH: &str = r#"[package]
name = "ph"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
admin-api = { path = "../admin-api" }
aws-lc-rs = { workspace = true }
arrayref = { version = "0.3.7" }
base64 = "0.22.1"
blake3 = { version = "1.5.4" }
bytes = { version = "1.11.1" }
capnp = { workspace = true }
capnp-futures = { workspace = true, optional = true }
capnp-rpc = { workspace = true }
cbpf-rs = { workspace = true }
chrono = "0.4.39"
clap = { workspace = true, features = ["env"] }
clap_complete = { workspace = true, optional = true }
cslab = { git = "https://github.com/mkolehmainen/zl-zpr-utils.git", tag = "cslab-v0.1.2" }
curve25519-dalek = "4.1.3"
dashmap = { version = "6.0.1" }
ed25519-dalek = { version = "3.0.0", features = ["pkcs8", "alloc"] }
enum-map = { version = "2.7.3" }
enumset = "1.1.5"
hdrhistogram = { version = "7.5.4" }
internet-checksum = { workspace = true }
ipnet = { workspace = true }
itertools = "0.14"
libc = { version = "0.2.183" }
libnode = { package = "libnode2", path = "../../libnode2" }
nix = { version = "0.29.0", features = ["fs", "ioctl", "net", "poll", "uio", "user"] }
open-enum = { version = "0.5.1" }
pkcs8 = "0.11"
rcu = { git = "https://github.com/mkolehmainen/zl-zpr-utils.git", tag = "rcu-v0.1.2", default-features = false }
replace_with = "0.1.8"
reqwest = { version = "0.13.1", features = ["json", "query"] }
serde = { version = "1.0.214", features = ["derive"] }
serde_json = "1.0.140"
sha2 = "0.10"
snow = "0.10.0"
socket2 = { version = "0.5.7", features = ["all"] }
strum = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tokio-util = { workspace = true }
toml = "0.9.11"
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
url = "2.5.4"
x25519-dalek = { version = "3.0.0", features = ["getrandom", "reusable_secrets"] }
x509-cert = { workspace = true, features = ["builder"] }
zerocopy = { version = "0.8.3", features = ["derive", "std"] }
zpr = { workspace = true }
zpr-ext = { workspace = true, features = ["bytes", "tokio", "socket2", "zerocopy"] }
zpr-utils = { workspace = true }

[target.'cfg(target_os = "linux")'.dependencies]
io-uring = { version = "0.7.4", optional = true }
tokio-tun = { version = "0.11.5" }

[dev-dependencies]
futures = "0.3.31"
serial_test = "4.0"
tokio = { workspace = true, features = ["test-util"] }

[features]
default = ["io-uring", "rcu-crossbeam-epoch"]
ci = []
enable-security-testing = []
rcu-aarc = ["rcu/rcu-aarc", "zpr/rcu-aarc"]
rcu-arc-swap = ["rcu/rcu-arc-swap", "zpr/rcu-arc-swap"]
rcu-crossbeam-epoch = ["rcu/rcu-crossbeam-epoch", "zpr/rcu-crossbeam-epoch"]
rcu-mutex-arc = ["rcu/rcu-mutex-arc", "zpr/rcu-mutex-arc"]
rcu-rwlock = ["rcu/rcu-rwlock", "zpr/rcu-rwlock"]
complete = ["clap_complete"]
capnp-ancillary = ["dep:capnp-futures", "capnp-futures/tokio-unix-fd-stream"]

[[bin]]
name = "ph"
path = "src/main.rs"

[[bin]]
name = "generate_completions"
path = "src/generate_completions.rs"
required-features = ["complete"]
"#;

    /// `zl-zpr-visaservice/Cargo.toml`: a workspace root pinning `zpr` in
    /// `[workspace.dependencies]`.
    const VS_ROOT: &str = r#"# zpr-visaservice
[workspace]
resolver = "3"
members = ["vs", "libeval", "zpt", "vs-admin", "admin-api-types"]

[workspace.package]
version = "0.19.0"
edition = "2024"
license = "Apache-2.0"

[workspace.dependencies]
base64 = "0.22.1"
bytes = "1.10.1"
capnp = { version = "0.25.0", features = ["sync_reader"] }
capnp-rpc = "0.25.0"
chrono = "0.4.42"
clap = { version = "4.5.50", features = ["derive"] }
colored = "3.0.0"
flate2 = "1.0.34"
openssl = "0.10.80"
rand = "0.9.2"
reqwest = "0.13.1"
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.149"
serde_with = { version = "3.18.0", default-features = false, features = ["std", "macros", "base64", "chrono"] }
thiserror = "2.0.18"
tracing = "0.1.41"
tracing-subscriber = "0.3.20"
zpr = { git = "https://github.com/mkolehmainen/zl-zpr-common.git", tag = "v0.27.0", features = ["vsapi", "policy"]}
"#;

    /// `zl-zpr-compiler/Cargo.toml`: a plain package pinning `zpr` — at
    /// `v0.26.0`, one tag behind the other consumers.
    const COMPILER: &str = r#"[package]
name = "zplc"
version = "0.18.0"
edition = "2024"

[dependencies]
base64 = "0.22.1"
bytes = "1.10.1"
capnp = "0.25.0"
chrono = "0.4.39"
clap = { version = "4.5.23", features = ["derive"] }
colored = "3.0.0"
hex = "0.4.3"
nix = { version = "0.30.1", features = ["hostname"] }
openssl = "0.10.70"
prost = "0.13.4"
ring = "0.17.8"
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.150"
thiserror = "2.0.8"
toml = "0.9.7"
zpr = { git = "https://github.com/mkolehmainen/zl-zpr-common.git", tag = "v0.26.0", features = ["policy"] }
"#;

    /// `zl-zpr-common/Cargo.toml`: pins `rcu` at tag `zpr-utils-v0.1.0` while
    /// `adapter/ph` pins the same crate and URL at `rcu-v0.1.2` — the real
    /// divergence of master-plan Finding 1.
    const COMMON: &str = r#"[package]
name = "zpr"
version = "0.25.1"
edition = "2024"
license = "Apache-2.0"

[dependencies]
base64 = { version = "0.22.1", optional = true }
bytes = { version = "1.10.1", optional = true }
capnp = { version = "0.25", optional = true }
colored = { version = "3.0.0", optional = true}
criterion = { version = "0.8.2 "}
flate2 = { version = "1.0.34", optional = true}
open-enum = { version = "0.5" }
serde = { version = "1.0.228", features = ["derive"], optional = true }
zerocopy = { version = "0.8", features = ["derive"] }
thiserror = "1.0.65"
url = "2.5.4"
range-set-blaze = { version = "0.5.0" }
ip_network_table-deps-treebitmap = { version = "0.5.0" }
rcu = { git = "https://github.com/mkolehmainen/zl-zpr-utils.git", tag = "zpr-utils-v0.1.0", default-features = false, optional = true }
rand = "0.10.1"
x25519-dalek = { version = "3.0.0", optional = true }

[build-dependencies]
capnpc = "0.25"

[features]
rcu-aarc = ["rcu/rcu-aarc", "rcu"]
rcu-arc-swap = ["rcu/rcu-arc-swap", "rcu"]
rcu-crossbeam-epoch = ["rcu/rcu-crossbeam-epoch", "rcu"]
rcu-mutex-arc = ["rcu/rcu-mutex-arc", "rcu"]
rcu-rwlock = ["rcu/rcu-rwlock", "rcu"]
policy = ["capnp", "bytes", "base64", "colored", "flate2", "serde"]
vsapi = ["capnp", "x25519-dalek"]
all = ["policy", "vsapi"]

[[bench]]
name = "ft_lookup_benchmark"
harness = false
"#;

    /// Shorthand: a manifest source with a display path.
    fn source(path: &str, text: &str) -> ManifestSource {
        ManifestSource {
            path: path.to_string(),
            text: text.to_string(),
        }
    }

    /// The core workspace with its `ph` member — the richest real extraction
    /// case: root pins, patch revs, member pins, member inheritance.
    fn core_pins() -> Vec<PinOccurrence> {
        extract_pins(
            &source("zl-zpr-core/Cargo.toml", CORE_ROOT),
            &[source("zl-zpr-core/adapter/ph/Cargo.toml", PH)],
        )
        .unwrap()
    }

    /// Finds the one occurrence matching crate name and file, panicking with
    /// the full pin list when absent so failures are diagnosable.
    fn find<'a>(pins: &'a [PinOccurrence], crate_name: &str, file: &str) -> &'a PinOccurrence {
        pins.iter()
            .find(|p| p.crate_name == crate_name && p.file == file)
            .unwrap_or_else(|| panic!("no pin of `{crate_name}` in {file}: {pins:#?}"))
    }

    #[test]
    fn extracts_git_tag_pin_with_file_and_line() {
        let pins = core_pins();
        let zpr = find(&pins, "zpr", "zl-zpr-core/Cargo.toml");
        assert_eq!(zpr.url, "https://github.com/mkolehmainen/zl-zpr-common.git");
        assert_eq!(zpr.reference, "v0.27.0");
        assert_eq!(zpr.kind, RefKind::Tag);
        assert_eq!(zpr.line, 29);
        assert!(!zpr.inherited);

        let cbpf = find(&pins, "cbpf-rs", "zl-zpr-core/Cargo.toml");
        assert_eq!(cbpf.reference, "cbpf-rs-v0.2.0");
        assert_eq!(cbpf.line, 16);
    }

    /// The Cap'n Proto fork is not ZPR-family, but its `rev` pin still
    /// captures — and `capnp`'s patch line must not be confused with the
    /// registry `capnp` in `[workspace.dependencies]` (spec-003 §4.1).
    #[test]
    fn extracts_rev_pinned_non_zpr_fork_from_patch_table() {
        let pins = core_pins();
        let capnp = find(&pins, "capnp", "zl-zpr-core/Cargo.toml");
        assert_eq!(capnp.url, "https://github.com/emilazy/capnproto-rust.git");
        assert_eq!(capnp.reference, "cfbcb9b");
        assert_eq!(capnp.kind, RefKind::Rev);
        assert_eq!(capnp.line, 37);
        // All four fork crates are captured.
        for name in ["capnp", "capnp-futures", "capnp-rpc", "capnpc"] {
            find(&pins, name, "zl-zpr-core/Cargo.toml");
        }
    }

    #[test]
    fn extracts_branch_pin() {
        let text = "[dependencies]\nfoo = { git = \"https://github.com/mkolehmainen/zl-zpr-x.git\", branch = \"zipline\" }\n";
        let pins = extract_pins(&source("x/Cargo.toml", text), &[]).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].kind, RefKind::Branch);
        assert_eq!(pins[0].reference, "zipline");
    }

    /// A member's `{ workspace = true }` resolves against the workspace root
    /// and is attributed to the member: this is how `adapter/ph` gets `zpr`,
    /// and missing it blinds the gate to a whole member.
    #[test]
    fn member_workspace_true_inherits_root_pin_attributed_to_member() {
        let pins = core_pins();
        let zpr = find(&pins, "zpr", "zl-zpr-core/adapter/ph/Cargo.toml");
        assert!(zpr.inherited);
        assert_eq!(zpr.reference, "v0.27.0");
        assert_eq!(zpr.url, "https://github.com/mkolehmainen/zl-zpr-common.git");
        assert_eq!(zpr.line, 55);
        // The member's own literal pins are captured too.
        let rcu = find(&pins, "rcu", "zl-zpr-core/adapter/ph/Cargo.toml");
        assert_eq!(rcu.reference, "rcu-v0.1.2");
        assert_eq!(rcu.line, 36);
        assert!(!rcu.inherited);
        let cslab = find(&pins, "cslab", "zl-zpr-core/adapter/ph/Cargo.toml");
        assert_eq!(cslab.line, 21);
    }

    /// Registry and `path` dependencies are not pins; a member inheriting a
    /// registry workspace dependency (`capnp`) inherits no pin from it.
    #[test]
    fn registry_and_path_dependencies_are_ignored() {
        let pins = core_pins();
        for name in [
            "clap",
            "libnode",
            "libnode2",
            "admin-api",
            "reqwest",
            "toml",
        ] {
            assert!(
                !pins.iter().any(|p| p.crate_name == name),
                "`{name}` must not be captured: {pins:#?}"
            );
        }
        // capnp is captured once (the patch rev at the root), never through
        // ph's `capnp = {{ workspace = true }}` against the registry entry.
        assert_eq!(
            pins.iter().filter(|p| p.crate_name == "capnp").count(),
            1,
            "{pins:#?}"
        );
        // The full census of the core+ph fixture: 8 at the root (4 ZPR tags +
        // 4 fork revs), 2 literal in ph, 4 inherited by ph.
        assert_eq!(pins.len(), 14, "{pins:#?}");
    }

    /// A non-ZPR git URL without a `rev` has nothing pinned to compare.
    #[test]
    fn non_zpr_git_url_without_rev_is_ignored() {
        let text = "[dependencies]\nbar = { git = \"https://github.com/serde-rs/json.git\", branch = \"master\" }\n";
        let pins = extract_pins(&source("x/Cargo.toml", text), &[]).unwrap();
        assert!(pins.is_empty(), "{pins:#?}");
    }

    /// The other real manifests extract their known pins: the compiler's
    /// `zpr` at `v0.26.0` (line 22) and common's `rcu` at `zpr-utils-v0.1.0`
    /// (line 21).
    #[test]
    fn compiler_and_common_fixtures_extract() {
        let pins = extract_pins(&source("zl-zpr-compiler/Cargo.toml", COMPILER), &[]).unwrap();
        assert_eq!(pins.len(), 1, "{pins:#?}");
        assert_eq!(pins[0].crate_name, "zpr");
        assert_eq!(pins[0].reference, "v0.26.0");
        assert_eq!(pins[0].line, 22);

        let pins = extract_pins(&source("zl-zpr-common/Cargo.toml", COMMON), &[]).unwrap();
        assert_eq!(pins.len(), 1, "{pins:#?}");
        assert_eq!(pins[0].crate_name, "rcu");
        assert_eq!(pins[0].reference, "zpr-utils-v0.1.0");
        assert_eq!(pins[0].line, 21);

        let pins = extract_pins(&source("zl-zpr-visaservice/Cargo.toml", VS_ROOT), &[]).unwrap();
        assert_eq!(pins.len(), 1, "{pins:#?}");
        assert_eq!(pins[0].reference, "v0.27.0");
        assert_eq!(pins[0].line, 29);
    }

    // -- gate 1: shared-dep agreement (spec-003 §4.1) -------------------------

    /// Hand-built occurrence for the gate tests, which are about grouping and
    /// reporting rather than extraction.
    fn pin(
        crate_name: &str,
        url: &str,
        reference: &str,
        file: &str,
        line: usize,
        inherited: bool,
    ) -> PinOccurrence {
        PinOccurrence {
            crate_name: crate_name.to_string(),
            url: url.to_string(),
            reference: reference.to_string(),
            kind: RefKind::Tag,
            file: file.to_string(),
            line,
            inherited,
        }
    }

    const COMMON_URL: &str = "https://github.com/mkolehmainen/zl-zpr-common.git";
    const UTILS_URL: &str = "https://github.com/mkolehmainen/zl-zpr-utils.git";

    /// Every finding at or above `severity`.
    fn at_least(findings: &[Finding], severity: Severity) -> Vec<&Finding> {
        findings.iter().filter(|f| f.severity >= severity).collect()
    }

    #[test]
    fn gate1_agreeing_set_passes() {
        let pins = vec![
            pin(
                "zpr",
                COMMON_URL,
                "v0.27.0",
                "zl-zpr-core/Cargo.toml",
                29,
                false,
            ),
            pin(
                "zpr",
                COMMON_URL,
                "v0.27.0",
                "zl-zpr-visaservice/Cargo.toml",
                29,
                false,
            ),
            pin(
                "cslab",
                UTILS_URL,
                "cslab-v0.1.2",
                "zl-zpr-core/adapter/ph/Cargo.toml",
                21,
                false,
            ),
        ];
        let findings = gate_pin_agreement(&pins, &[], false);
        assert!(
            at_least(&findings, Severity::Warn).is_empty(),
            "{findings:#?}"
        );
        // The pass is stated, not silent: an [OK] line naming the census.
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Ok && f.message.contains("2 crates")),
            "{findings:#?}"
        );
    }

    /// A disagreement lists every tag with every file and line pinning it,
    /// marks inheritance, and names the remedy (spec-003 §4.1).
    #[test]
    fn gate1_disagreement_lists_every_tag_file_and_line() {
        let pins = vec![
            pin(
                "zpr",
                COMMON_URL,
                "v0.26.0",
                "zl-zpr-compiler/Cargo.toml",
                22,
                false,
            ),
            pin(
                "zpr",
                COMMON_URL,
                "v0.27.0",
                "zl-zpr-core/Cargo.toml",
                29,
                false,
            ),
            pin(
                "zpr",
                COMMON_URL,
                "v0.27.0",
                "zl-zpr-core/adapter/ph/Cargo.toml",
                55,
                true,
            ),
        ];
        let findings = gate_pin_agreement(&pins, &[], false);
        let errors = at_least(&findings, Severity::Error);
        assert_eq!(errors.len(), 1, "{findings:#?}");
        let message = &errors[0].message;
        for needle in [
            "pin disagreement",
            "`zpr`",
            COMMON_URL,
            "v0.26.0",
            "v0.27.0",
            "zl-zpr-compiler/Cargo.toml:22",
            "zl-zpr-core/Cargo.toml:29",
            "zl-zpr-core/adapter/ph/Cargo.toml:55",
            "inherited",
            "allow_pin_drift",
        ] {
            assert!(message.contains(needle), "missing {needle:?}: {message}");
        }
    }

    /// The same crate at the same tag from two URLs is also a disagreement —
    /// the double-`cslab` trap of `docs/BUILD.md` (spec-003 §4.1).
    #[test]
    fn gate1_same_tag_from_two_urls_is_a_disagreement() {
        let org_url = "https://github.com/org-zpr/zpr-utils.git";
        let pins = vec![
            pin(
                "cslab",
                UTILS_URL,
                "cslab-v0.1.2",
                "zl-zpr-core/adapter/ph/Cargo.toml",
                21,
                false,
            ),
            pin(
                "cslab",
                org_url,
                "cslab-v0.1.2",
                "zl-zpr-utils/rcu/Cargo.toml",
                10,
                false,
            ),
        ];
        let findings = gate_pin_agreement(&pins, &[], false);
        let errors = at_least(&findings, Severity::Error);
        assert_eq!(errors.len(), 1, "{findings:#?}");
        assert!(
            errors[0].message.contains(UTILS_URL),
            "{}",
            errors[0].message
        );
        assert!(errors[0].message.contains(org_url), "{}", errors[0].message);
    }

    /// The same crate, URL and reference *text* via different reference kinds
    /// is a disagreement too: git resolves `tag = "release"` and
    /// `branch = "release"` to potentially different commits, so the kind is
    /// part of the variant identity (spec-003 §4.1).
    #[test]
    fn gate1_same_reference_text_with_different_kinds_is_a_disagreement() {
        let mut tagged = pin(
            "zpr",
            COMMON_URL,
            "release",
            "zl-zpr-core/Cargo.toml",
            29,
            false,
        );
        tagged.kind = RefKind::Tag;
        let mut branched = pin(
            "zpr",
            COMMON_URL,
            "release",
            "zl-zpr-visaservice/Cargo.toml",
            29,
            false,
        );
        branched.kind = RefKind::Branch;
        let findings = gate_pin_agreement(&[tagged, branched], &[], false);
        let errors = at_least(&findings, Severity::Error);
        assert_eq!(errors.len(), 1, "{findings:#?}");
        // The report names both kinds so the reader can tell the variants
        // apart — the reference text alone is identical.
        let message = &errors[0].message;
        assert!(message.contains("tag"), "{message}");
        assert!(message.contains("branch"), "{message}");
    }

    /// The disagreement report spells each variant's kind (`tag v0.26.0`),
    /// so a mixed-kind disagreement is diagnosable from the message alone.
    #[test]
    fn gate1_disagreement_report_names_each_variants_kind() {
        let mut tagged = pin(
            "zpr",
            COMMON_URL,
            "v0.26.0",
            "zl-zpr-compiler/Cargo.toml",
            22,
            false,
        );
        tagged.kind = RefKind::Tag;
        let mut revved = pin(
            "zpr",
            COMMON_URL,
            "abc1234",
            "zl-zpr-core/Cargo.toml",
            29,
            false,
        );
        revved.kind = RefKind::Rev;
        let findings = gate_pin_agreement(&[tagged, revved], &[], false);
        let errors = at_least(&findings, Severity::Error);
        assert_eq!(errors.len(), 1, "{findings:#?}");
        let message = &errors[0].message;
        assert!(message.contains("tag v0.26.0"), "{message}");
        assert!(message.contains("rev abc1234"), "{message}");
    }

    /// An `allow_pin_drift` entry suppresses exactly its crate, echoing the
    /// reviewed reason; other disagreements still fail (spec-003 §4.1).
    #[test]
    fn gate1_allow_pin_drift_suppresses_one_crate_and_echoes_reason() {
        let pins = vec![
            pin(
                "rcu",
                UTILS_URL,
                "zpr-utils-v0.1.0",
                "zl-zpr-common/Cargo.toml",
                21,
                false,
            ),
            pin(
                "rcu",
                UTILS_URL,
                "rcu-v0.1.2",
                "zl-zpr-core/adapter/ph/Cargo.toml",
                36,
                false,
            ),
            pin(
                "zpr",
                COMMON_URL,
                "v0.26.0",
                "zl-zpr-compiler/Cargo.toml",
                22,
                false,
            ),
            pin(
                "zpr",
                COMMON_URL,
                "v0.27.0",
                "zl-zpr-core/Cargo.toml",
                29,
                false,
            ),
        ];
        let drift = vec![PinDrift {
            crate_name: "rcu".to_string(),
            reason: "zl-zpr-common pins zpr-utils-v0.1.0, ph pins rcu-v0.1.2; zipline#18"
                .to_string(),
        }];
        let findings = gate_pin_agreement(&pins, &drift, false);
        // rcu is suppressed: no error names it, and its reason is echoed.
        let errors = at_least(&findings, Severity::Error);
        assert_eq!(errors.len(), 1, "{findings:#?}");
        assert!(errors[0].message.contains("`zpr`"), "{}", errors[0].message);
        assert!(
            findings.iter().any(|f| f.severity == Severity::Info
                && f.message.contains("`rcu`")
                && f.message.contains("zipline#18")),
            "{findings:#?}"
        );
    }

    /// `--allow-pin-drift` downgrades every disagreement to a warning, so the
    /// command exits 0 (spec-003 §4.1).
    #[test]
    fn gate1_downgrade_turns_errors_into_warnings() {
        let pins = vec![
            pin(
                "zpr",
                COMMON_URL,
                "v0.26.0",
                "zl-zpr-compiler/Cargo.toml",
                22,
                false,
            ),
            pin(
                "zpr",
                COMMON_URL,
                "v0.27.0",
                "zl-zpr-core/Cargo.toml",
                29,
                false,
            ),
        ];
        let findings = gate_pin_agreement(&pins, &[], true);
        assert!(
            at_least(&findings, Severity::Error).is_empty(),
            "{findings:#?}"
        );
        let warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.severity == Severity::Warn)
            .collect();
        assert_eq!(warnings.len(), 1, "{findings:#?}");
        assert!(
            warnings[0].message.contains("pin disagreement"),
            "{}",
            warnings[0].message
        );
    }

    // -- gate 2: freshness (spec-003 §4.2) ------------------------------------

    /// A tag listing keyed by URL, standing in for the workspace checkouts.
    fn tags_for<'a>(
        url_tags: &'a [(&'a str, &'a [&'a str])],
    ) -> impl Fn(&str) -> Option<Vec<String>> + 'a {
        move |url: &str| {
            url_tags
                .iter()
                .find(|(u, _)| *u == url)
                .map(|(_, tags)| tags.iter().map(|t| t.to_string()).collect())
        }
    }

    /// Pinned behind the newest local tag warns, naming pin and newest
    /// (spec-003 §4.2) — and never errors.
    #[test]
    fn gate2_pin_behind_newest_tag_warns() {
        let pins = vec![pin(
            "zpr",
            COMMON_URL,
            "v0.26.0",
            "zl-zpr-compiler/Cargo.toml",
            22,
            false,
        )];
        let listing = [(COMMON_URL, ["v0.25.1", "v0.26.0", "v0.27.0"].as_slice())];
        let findings = gate_freshness(&pins, tags_for(&listing));
        assert!(
            at_least(&findings, Severity::Error).is_empty(),
            "{findings:#?}"
        );
        let warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.severity == Severity::Warn)
            .collect();
        assert_eq!(warnings.len(), 1, "{findings:#?}");
        for needle in ["`zpr`", "v0.26.0", "v0.27.0"] {
            assert!(
                warnings[0].message.contains(needle),
                "missing {needle:?}: {}",
                warnings[0].message
            );
        }
    }

    /// Pinned at the newest tag is silent — a set at the tip needs no note.
    #[test]
    fn gate2_pin_at_newest_tag_is_silent() {
        let pins = vec![pin(
            "zpr",
            COMMON_URL,
            "v0.27.0",
            "zl-zpr-core/Cargo.toml",
            29,
            false,
        )];
        let listing = [(COMMON_URL, ["v0.26.0", "v0.27.0"].as_slice())];
        let findings = gate_freshness(&pins, tags_for(&listing));
        assert!(findings.is_empty(), "{findings:#?}");
    }

    /// A missing local checkout is an INFO, not a failure: cargo fetches the
    /// tag from GitHub either way (spec-003 §4.2).
    #[test]
    fn gate2_missing_checkout_is_info_not_failure() {
        let pins = vec![pin(
            "zpr",
            COMMON_URL,
            "v0.26.0",
            "zl-zpr-compiler/Cargo.toml",
            22,
            false,
        )];
        let findings = gate_freshness(&pins, |_| None);
        assert!(
            at_least(&findings, Severity::Warn).is_empty(),
            "{findings:#?}"
        );
        let infos: Vec<_> = findings
            .iter()
            .filter(|f| f.severity == Severity::Info)
            .collect();
        assert_eq!(infos.len(), 1, "{findings:#?}");
        assert!(infos[0].message.contains("`zpr`"), "{}", infos[0].message);
    }

    /// Tag ordering is semantic-version order, not creation or lexicographic
    /// order: `v0.9.1` is older than `v0.15.0` (spec-003 §4.2).
    #[test]
    fn gate2_orders_tags_by_semantic_version_not_lexicographically() {
        let pins = vec![pin(
            "zpr",
            COMMON_URL,
            "v0.9.1",
            "zl-zpr-core/Cargo.toml",
            29,
            false,
        )];
        // Lexicographically v0.9.1 > v0.15.0; semantically it is behind.
        let listing = [(COMMON_URL, ["v0.9.1", "v0.15.0"].as_slice())];
        let findings = gate_freshness(&pins, tags_for(&listing));
        let warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.severity == Severity::Warn)
            .collect();
        assert_eq!(warnings.len(), 1, "{findings:#?}");
        assert!(
            warnings[0].message.contains("v0.15.0"),
            "{}",
            warnings[0].message
        );
    }

    /// Only tags sharing the pin's name prefix compete: `zl-zpr-utils` tags
    /// several crates in one repository, and a `cslab-v*` pin must never be
    /// measured against `zpr-utils-v*` or `rcu-v*`.
    #[test]
    fn gate2_compares_only_tags_with_the_same_prefix() {
        let pins = vec![pin(
            "cslab",
            UTILS_URL,
            "cslab-v0.1.0",
            "zl-zpr-core/adapter/ph/Cargo.toml",
            21,
            false,
        )];
        let listing = [(
            UTILS_URL,
            [
                "cslab-v0.1.0",
                "cslab-v0.1.2",
                "rcu-v0.1.2",
                "zpr-utils-v9.9.9",
            ]
            .as_slice(),
        )];
        let findings = gate_freshness(&pins, tags_for(&listing));
        // cslab-v0.1.0 is behind cslab-v0.1.2 — and only cslab-v* competes,
        // so the far-newer zpr-utils tag is not the one named.
        let warnings: Vec<_> = findings
            .iter()
            .filter(|f| f.severity == Severity::Warn)
            .collect();
        assert_eq!(warnings.len(), 1, "{findings:#?}");
        assert!(
            warnings[0].message.contains("cslab-v0.1.2"),
            "{}",
            warnings[0].message
        );
        assert!(
            !warnings[0].message.contains("zpr-utils-v9.9.9"),
            "{}",
            warnings[0].message
        );
    }

    /// Freshness applies to **agreed** pins only (spec-003 §4.2: "for each
    /// agreed pin"): a crate whose pins disagree is gate 1's finding, and a
    /// freshness warning on top of that error would be noise.
    #[test]
    fn gate2_skips_disagreeing_pins() {
        let pins = vec![
            pin(
                "rcu",
                UTILS_URL,
                "rcu-v0.1.2",
                "zl-zpr-core/adapter/ph/Cargo.toml",
                36,
                false,
            ),
            pin(
                "rcu",
                UTILS_URL,
                "zpr-utils-v0.1.0",
                "zl-zpr-common/Cargo.toml",
                21,
                false,
            ),
        ];
        let listing = [(
            UTILS_URL,
            ["rcu-v0.1.2", "zpr-utils-v0.1.0", "zpr-utils-v0.2.2"].as_slice(),
        )];
        let findings = gate_freshness(&pins, tags_for(&listing));
        assert!(findings.is_empty(), "{findings:#?}");
    }

    /// A rev pin has no version ordering; the freshness gate skips it rather
    /// than comparing a commit hash against tags.
    #[test]
    fn gate2_skips_rev_pins() {
        let mut fork = pin(
            "capnp",
            "https://github.com/emilazy/capnproto-rust.git",
            "cfbcb9b",
            "zl-zpr-core/Cargo.toml",
            37,
            false,
        );
        fork.kind = RefKind::Rev;
        let findings = gate_freshness(&[fork], |_| Some(vec!["v1.0.0".to_string()]));
        assert!(findings.is_empty(), "{findings:#?}");
    }

    // -- gate 3: compiler / visa service version (spec-003 §4.3) --------------
    //
    // The oracle is libeval/src/pio.rs check_version: pre-1.0, the found
    // (compiler) version must have major and minor exactly equal to the
    // minimum's, and patch >= the minimum's. Compare these cases by eye
    // against that function when reviewing.

    /// The real `vs/src/config.rs` lines 28-31, verbatim.
    const VS_CONFIG: &str = "\
// We only load policy files built by this version or later.
pub const POLICY_MIN_COMPILER_MAJOR: u32 = 0;
pub const POLICY_MIN_COMPILER_MINOR: u32 = 18;
pub const POLICY_MIN_COMPILER_PATCH: u32 = 0;
";

    #[test]
    fn gate3_parses_the_real_config_constants() {
        assert_eq!(
            policy_min_compiler(VS_CONFIG, "vs/src/config.rs").unwrap(),
            (0, 18, 0)
        );
    }

    /// An unreadable constant errors naming the file — silently skipping the
    /// check would be worse than failing (spec-003 §4.3).
    #[test]
    fn gate3_unparseable_config_errors_naming_the_file() {
        let error = policy_min_compiler("pub const OTHER: u32 = 1;\n", "vs/src/config.rs")
            .unwrap_err()
            .to_string();
        assert!(error.contains("vs/src/config.rs"), "{error}");
        assert!(error.contains("POLICY_MIN_COMPILER"), "{error}");
    }

    #[test]
    fn gate3_parses_package_version_from_cargo_toml() {
        assert_eq!(
            package_version(COMPILER, "zl-zpr-compiler/Cargo.toml").unwrap(),
            (0, 18, 0)
        );
    }

    /// A manifest without `[package].version` errors naming the file.
    #[test]
    fn gate3_missing_package_version_errors_naming_the_file() {
        let error = package_version("[workspace]\nmembers = []\n", "zl-zpr-compiler/Cargo.toml")
            .unwrap_err()
            .to_string();
        assert!(error.contains("zl-zpr-compiler/Cargo.toml"), "{error}");
        assert!(error.contains("version"), "{error}");
    }

    /// The check_version rule, exactly: equal passes, higher patch passes,
    /// lower patch fails, higher minor fails, higher major fails.
    #[test]
    fn gate3_applies_check_version_rule_exactly() {
        let ok = |zplc: Version3, min: Version3| {
            let findings = gate_compiler_version(zplc, "c.toml", min, "config.rs");
            at_least(&findings, Severity::Error).is_empty()
        };
        assert!(ok((0, 18, 0), (0, 18, 0)), "equal versions must pass");
        assert!(ok((0, 18, 4), (0, 18, 0)), "higher patch must pass");
        assert!(!ok((0, 18, 0), (0, 18, 4)), "lower patch must fail");
        assert!(!ok((0, 19, 0), (0, 18, 0)), "higher minor must fail");
        assert!(!ok((0, 17, 9), (0, 18, 0)), "lower minor must fail");
        assert!(!ok((1, 18, 0), (0, 18, 0)), "higher major must fail");
    }

    /// The failure message carries both file references and the rule, per the
    /// spec-003 §4.3 report format.
    #[test]
    fn gate3_failure_names_both_files_and_the_rule() {
        let findings = gate_compiler_version(
            (0, 19, 0),
            "zl-zpr-compiler/Cargo.toml",
            (0, 18, 0),
            "zl-zpr-visaservice/vs/src/config.rs",
        );
        let errors = at_least(&findings, Severity::Error);
        assert_eq!(errors.len(), 1, "{findings:#?}");
        let message = &errors[0].message;
        for needle in [
            "zplc 0.19.0",
            "cannot produce policy",
            "zl-zpr-compiler/Cargo.toml",
            "zl-zpr-visaservice/vs/src/config.rs",
            "0.18.0",
            "major ==, minor ==, patch >=",
        ] {
            assert!(message.contains(needle), "missing {needle:?}: {message}");
        }
    }

    /// A compatible pair is reported [OK], naming both versions.
    #[test]
    fn gate3_pass_is_reported_ok() {
        let findings = gate_compiler_version((0, 18, 4), "c.toml", (0, 18, 0), "config.rs");
        assert_eq!(findings.len(), 1, "{findings:#?}");
        assert_eq!(findings[0].severity, Severity::Ok);
        assert!(
            findings[0].message.contains("0.18.4"),
            "{}",
            findings[0].message
        );
        assert!(
            findings[0].message.contains("0.18.0"),
            "{}",
            findings[0].message
        );
    }

    // -- gate 1 lock scan: post-resolution dual versions (zipline#69) ---------

    /// Builds one lock source for the scan tests.
    fn lock(path: &str, text: &str) -> LockSource {
        LockSource {
            path: path.to_string(),
            text: text.to_string(),
        }
    }

    /// The zipline#69 blind spot: a lock carrying one ZPR-family crate at two
    /// versions is an error naming the lock, the crate and both versions —
    /// manifest-level gate 1 cannot see a pin inside a tagged git dependency,
    /// so the resolved lock is where the dual version surfaces.
    #[test]
    fn lock_scan_dual_zpr_family_version_is_an_error() {
        const LOCK: &str = r#"
version = 4

[[package]]
name = "zpr"
version = "0.8.1"
source = "git+https://github.com/mkolehmainen/zl-zpr-common.git?tag=v0.8.1#71ead993"

[[package]]
name = "zpr"
version = "0.28.0"
source = "git+https://github.com/mkolehmainen/zl-zpr-common.git?tag=v0.28.0#3abdacfe"

[[package]]
name = "serde"
version = "1.0.219"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let findings = gate_lock_dual_versions(&[lock("zl-zpr-core/Cargo.lock", LOCK)]);
        let errors = at_least(&findings, Severity::Error);
        assert_eq!(errors.len(), 1, "{findings:#?}");
        let message = &errors[0].message;
        for needle in [
            "zpr",
            "zl-zpr-core/Cargo.lock",
            "0.8.1",
            "0.28.0",
            "tag=v0.8.1",
            "tag=v0.28.0",
        ] {
            assert!(message.contains(needle), "missing {needle:?}: {message}");
        }
    }

    /// A clean lock passes with a stated [OK] census, not silence, and a
    /// non-ZPR-family crate at two versions (cargo's normal semver-major
    /// duplication) is not our finding to raise.
    #[test]
    fn lock_scan_clean_and_foreign_duplicates_pass() {
        const LOCK: &str = r#"
version = 4

[[package]]
name = "zpr"
version = "0.28.0"
source = "git+https://github.com/mkolehmainen/zl-zpr-common.git?tag=v0.28.0#3abdacfe"

[[package]]
name = "syn"
version = "1.0.109"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "syn"
version = "2.0.100"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let findings = gate_lock_dual_versions(&[lock("zl-zpr-core/Cargo.lock", LOCK)]);
        assert!(
            at_least(&findings, Severity::Warn).is_empty(),
            "{findings:#?}"
        );
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Ok && f.message.contains("1 lock")),
            "{findings:#?}"
        );
    }

    /// An unparseable lock is an error finding naming the lock — skipping it
    /// silently would let the set pass with that repository's resolution
    /// unexamined, the exact overstatement this scan exists to prevent.
    #[test]
    fn lock_scan_unparseable_lock_is_an_error() {
        let findings = gate_lock_dual_versions(&[lock("zl-zpr-core/Cargo.lock", "not = [toml")]);
        let errors = at_least(&findings, Severity::Error);
        assert_eq!(errors.len(), 1, "{findings:#?}");
        assert!(
            errors[0].message.contains("zl-zpr-core/Cargo.lock"),
            "{}",
            errors[0].message
        );
    }
}
