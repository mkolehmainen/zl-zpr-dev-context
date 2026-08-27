//! Thin wrappers over the installed `git` binary (spec §6.3). Git operations are
//! never reimplemented here: every function shells out to `git` and interprets
//! its output. Nothing in this module resets, rebases, stashes, switches
//! branches, or pushes — the safety invariants of spec §11 hold by construction.

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

/// Runs `git` with `args` in `dir` and returns its trimmed stdout. A nonzero
/// exit becomes an error carrying git's stderr.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run git in {}: {e}", dir.display()))?;

    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// True when `dir` is itself the root of a Git working tree. The comparison
/// against the reported top level matters: without it, any plain directory
/// *inside* a repository would answer true.
pub fn is_repo(dir: &Path) -> bool {
    let Ok(toplevel) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        return false;
    };
    match (std::fs::canonicalize(toplevel), std::fs::canonicalize(dir)) {
        (Ok(top), Ok(here)) => top == here,
        _ => false,
    }
}

/// The abbreviated commit hash of `HEAD`.
pub fn head_short(dir: &Path) -> Result<String> {
    git(dir, &["rev-parse", "--short", "HEAD"])
}

/// The current branch name, or `None` when `HEAD` is detached.
pub fn branch(dir: &Path) -> Result<Option<String>> {
    let name = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    // `--abbrev-ref` reports the literal string "HEAD" for a detached head.
    Ok(if name == "HEAD" { None } else { Some(name) })
}

/// True when the working tree has staged or unstaged changes to *tracked*
/// files.
///
/// Spec §6.3 words this as "`git status --porcelain` non-empty", which would
/// also count untracked files — but `zpr-dev` deliberately leaves the generated
/// `AGENTS.md` and `CLAUDE.md` untracked (§1.4.3, no ignore-file management), so
/// every synced repository would read as dirty forever: `status` would always
/// say `modified` and `update --all` would always skip. `--untracked-files=no`
/// costs nothing in safety — an untracked file that a fast-forward would
/// overwrite still makes `git merge --ff-only` refuse, which [`ff_merge`]
/// reports as `cannot fast-forward`.
pub fn is_dirty(dir: &Path) -> Result<bool> {
    Ok(!git(dir, &["status", "--porcelain", "--untracked-files=no"])?.is_empty())
}

/// Commits ahead of, and behind, the upstream of the current branch. `None`
/// when there is no upstream to compare against (which includes a detached
/// `HEAD`), so callers can report "no upstream" rather than an error.
pub fn ahead_behind(dir: &Path) -> Result<Option<(usize, usize)>> {
    let Ok(counts) = git(dir, &["rev-list", "--left-right", "--count", "HEAD...@{u}"]) else {
        return Ok(None);
    };
    // Output is "<ahead>\t<behind>": left of the three-dot range is HEAD.
    let mut fields = counts.split_whitespace();
    match (fields.next(), fields.next()) {
        (Some(ahead), Some(behind)) => Ok(Some((ahead.parse()?, behind.parse()?))),
        _ => bail!("unexpected git rev-list output: {counts:?}"),
    }
}

/// Clones `url` into `dest`, optionally at a specific branch. The parent
/// directory is created if needed, since `setup` may be cloning into a
/// workspace that does not exist yet.
pub fn clone(url: &str, dest: &Path, branch: Option<&str>) -> Result<()> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;

    let dest = dest.to_string_lossy();
    let mut args = vec!["clone"];
    if let Some(branch) = branch {
        args.push("--branch");
        args.push(branch);
    }
    args.push(url);
    args.push(&dest);

    git(parent, &args)?;
    Ok(())
}

/// Fetches from the default remote. Does not modify the working tree.
pub fn fetch(dir: &Path) -> Result<()> {
    git(dir, &["fetch"])?;
    Ok(())
}

/// Fast-forwards the current branch onto its upstream. Returns `false` when a
/// fast-forward is not possible — an expected outcome for a diverged branch,
/// not a failure — and leaves the repository untouched in that case.
pub fn ff_merge(dir: &Path) -> Result<bool> {
    Ok(git(dir, &["merge", "--ff-only", "@{u}"]).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Pins identity and signing per repository so results never depend on the
    /// developer's global Git configuration.
    fn configure(dir: &Path) {
        git(dir, &["config", "user.name", "zpr-dev tests"]).unwrap();
        git(dir, &["config", "user.email", "tests@example.invalid"]).unwrap();
        git(dir, &["config", "commit.gpgsign", "false"]).unwrap();
    }

    /// Creates a working repository at `dir` with one commit.
    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-b", "main"]).unwrap();
        configure(dir);
        commit_file(dir, "README.md");
    }

    /// Writes `name` and commits it.
    fn commit_file(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), format!("{name} contents\n")).unwrap();
        git(dir, &["add", "-A"]).unwrap();
        git(dir, &["commit", "-m", &format!("add {name}")]).unwrap();
    }

    /// Creates a bare origin under `root`, seeded with one commit.
    fn seeded_origin(root: &Path) -> PathBuf {
        let origin = root.join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "--bare", "-b", "main"]).unwrap();
        advance_origin(root, &origin, "seed.md");
        origin
    }

    /// Moves the bare origin forward by one commit, via a throwaway clone —
    /// the only way to add a commit to a bare repository without reimplementing
    /// git. (The push lives in the fixture; no command ever pushes.)
    fn advance_origin(root: &Path, origin: &Path, file: &str) {
        let scratch = root.join(format!("scratch-{file}"));
        clone(&origin.to_string_lossy(), &scratch, None).unwrap();
        configure(&scratch);
        commit_file(&scratch, file);
        git(&scratch, &["push", "origin", "HEAD:main"]).unwrap();
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    /// A bare origin with one commit plus a fresh clone of it at `root/work`.
    fn origin_and_clone(root: &Path) -> (PathBuf, PathBuf) {
        let origin = seeded_origin(root);
        let work = root.join("work");
        clone(&origin.to_string_lossy(), &work, None).unwrap();
        configure(&work);
        (origin, work)
    }

    #[test]
    fn is_repo_false_for_plain_directory() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_repo(tmp.path()));
    }

    #[test]
    fn is_repo_true_after_init() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert!(is_repo(tmp.path()));
        // A subdirectory of a repository is not itself a repository.
        let nested = tmp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        assert!(!is_repo(&nested));
    }

    #[test]
    fn head_short_returns_seven_or_more_hex_chars() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let sha = head_short(tmp.path()).unwrap();
        assert!(sha.len() >= 7, "unexpectedly short sha: {sha:?}");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "not hex: {sha:?}"
        );
    }

    #[test]
    fn branch_returns_name_on_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert_eq!(branch(tmp.path()).unwrap().as_deref(), Some("main"));
    }

    #[test]
    fn branch_returns_none_when_detached() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let first = git(tmp.path(), &["rev-parse", "HEAD"]).unwrap();
        commit_file(tmp.path(), "second.md");
        git(tmp.path(), &["checkout", &first]).unwrap();
        assert_eq!(branch(tmp.path()).unwrap(), None);
    }

    #[test]
    fn is_dirty_false_when_clean_true_after_edit() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert!(!is_dirty(tmp.path()).unwrap());

        // An untracked file alone is not dirtiness: that is what the generated
        // context files look like in every synced repository.
        std::fs::write(tmp.path().join("AGENTS.md"), "generated\n").unwrap();
        assert!(!is_dirty(tmp.path()).unwrap());

        std::fs::write(tmp.path().join("README.md"), "edited\n").unwrap();
        assert!(is_dirty(tmp.path()).unwrap());
    }

    #[test]
    fn ahead_behind_none_without_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        assert_eq!(ahead_behind(tmp.path()).unwrap(), None);
    }

    #[test]
    fn ahead_behind_counts_after_origin_advances() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, work) = origin_and_clone(tmp.path());
        advance_origin(tmp.path(), &origin, "later.md");
        fetch(&work).unwrap();
        assert_eq!(ahead_behind(&work).unwrap(), Some((0, 1)));
    }

    #[test]
    fn ff_merge_true_when_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, work) = origin_and_clone(tmp.path());
        let before = head_short(&work).unwrap();
        advance_origin(tmp.path(), &origin, "later.md");
        fetch(&work).unwrap();

        assert!(ff_merge(&work).unwrap());
        assert_ne!(head_short(&work).unwrap(), before);
        assert!(work.join("later.md").exists());
    }

    #[test]
    fn ff_merge_false_on_divergence() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, work) = origin_and_clone(tmp.path());
        commit_file(&work, "local.md");
        let before = head_short(&work).unwrap();
        advance_origin(tmp.path(), &origin, "remote.md");
        fetch(&work).unwrap();

        assert!(!ff_merge(&work).unwrap());
        assert_eq!(head_short(&work).unwrap(), before);
        assert!(!is_dirty(&work).unwrap());
    }
}
