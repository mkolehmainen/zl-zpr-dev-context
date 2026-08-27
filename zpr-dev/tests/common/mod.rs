//! Fixture harness for the integration tests (spec §8.1).
//!
//! Every fixture is a self-contained workspace in a `tempfile::TempDir`: bare
//! origin repositories addressed by `file://` URL, and a context checkout cloned
//! from one of them. No network access and no credentials are ever required.
//!
//! ```text
//! <tmp>/origins/zpr-dev-context.git    bare, seeded with the shared context
//! <tmp>/origins/zpr-core.git           bare, seeded with a README
//! <tmp>/origins/zpr-common.git         bare, seeded with a README
//! <tmp>/workspace/zpr-dev-context/     cloned checkout (has an upstream)
//! <tmp>/workspace/<name>/              only after `clone_repos` or `setup`
//! ```

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use tempfile::TempDir;

/// Directory name of the context checkout inside the workspace (spec §3.3).
pub const CONTEXT: &str = "zpr-dev-context";

/// The source repositories every fixture provides.
pub const REPOS: [&str; 2] = ["zpr-core", "zpr-common"];

/// The fixture's shared context body. The documentation reference is what
/// Step 5's rewriting turns into an absolute path.
const AGENTS_BODY: &str = "\
Shared ZPR conventions for the fixture workspace.

See [the example](docs/EXAMPLE.md) for the house style.
";

const EXAMPLE_DOC: &str = "# Example\n\nFixture documentation.\n";

/// Makes scratch directories and fixture commits unique across the whole test
/// binary, whose tests run in parallel.
static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Builds a command whose Git behavior is independent of the developer's
/// machine: no global or system configuration, a fixed identity, and no
/// credential prompts. Set per child process, so nothing mutates this
/// process's environment (see the Step 3 notes on `std::env::set_var`).
fn sanitized(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "zpr-dev tests")
        .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
        .env("GIT_COMMITTER_NAME", "zpr-dev tests")
        .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("ZPR_WORKSPACE");
    command
}

/// Runs `git` in `dir` and returns its trimmed stdout, panicking with git's
/// stderr on failure — a fixture that cannot set itself up is a test error.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = sanitized("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run git in {}: {e}", dir.display()));

    assert!(
        output.status.success(),
        "git {} failed in {}: {}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Writes `body` to `path`, creating parent directories as needed.
fn write_file(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

pub struct Fixture {
    /// Held only to keep the temporary directory alive for the test's duration.
    _tmp: TempDir,
    /// The temporary root, holding `origins/` and `workspace/`.
    pub root: PathBuf,
    /// Passed to the binary as `--workspace`.
    pub workspace: PathBuf,
    /// `<workspace>/zpr-dev-context`, where the binary resolves it by default.
    pub context: PathBuf,
    origins: PathBuf,
}

impl Fixture {
    /// Assembles the §8.1 fixture: three seeded bare origins and a context
    /// checkout cloned from its own origin, so `update` has an upstream to
    /// fast-forward onto. The source repositories are *not* cloned — call
    /// [`Fixture::clone_repos`] for that, or let `setup` do it.
    pub fn new() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        // Canonicalized so the `file://` URLs and any path assertions match the
        // paths the binary reports.
        let root = tmp.path().canonicalize().unwrap();
        let workspace = root.join("workspace");
        let origins = root.join("origins");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&origins).unwrap();

        let fixture = Fixture {
            _tmp: tmp,
            context: workspace.join(CONTEXT),
            root,
            workspace,
            origins,
        };

        for name in REPOS {
            let readme = format!("# {name}\n");
            fixture.create_origin(name, &[("README.md", &readme)]);
        }
        let manifest = fixture.manifest();
        fixture.create_origin(
            CONTEXT,
            &[
                ("AGENTS.md", AGENTS_BODY),
                ("docs/EXAMPLE.md", EXAMPLE_DOC),
                ("workspace.yaml", &manifest),
            ],
        );
        git(
            &fixture.workspace,
            &["clone", &fixture.origin_url(CONTEXT), CONTEXT],
        );
        fixture
    }

    /// The `file://` URL of a fixture origin, usable as a manifest URL or as
    /// `setup --context-url`.
    pub fn origin_url(&self, name: &str) -> String {
        format!(
            "file://{}",
            self.origins.join(format!("{name}.git")).display()
        )
    }

    /// The fixture's `workspace.yaml`. Deliberately minimal: no `agent` block,
    /// so tests that care about `agent.hermes.shared_skills` add one themselves.
    fn manifest(&self) -> String {
        let mut yaml = String::from("version: 1\nworkspace:\n  name: fixture\nrepositories:\n");
        for name in REPOS {
            yaml.push_str(&format!(
                "  - name: {name}\n    url: {}\n",
                self.origin_url(name)
            ));
        }
        yaml.push_str("documentation:\n  root: docs\n");
        yaml
    }

    /// Creates a bare origin for `name` and seeds it with one commit.
    fn create_origin(&self, name: &str, files: &[(&str, &str)]) {
        let bare = self.origins.join(format!("{name}.git"));
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--bare", "-b", "main"]);
        self.push_to_origin(name, files);
    }

    /// Adds one commit containing `files` to `name`'s origin, through a
    /// throwaway clone — the only way to move a bare repository forward without
    /// reimplementing git. The push lives here in the fixture; no command in
    /// `zpr-dev` ever pushes (spec §11).
    fn push_to_origin(&self, name: &str, files: &[(&str, &str)]) {
        let scratch = self.root.join(format!(
            "scratch-{name}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        git(
            &self.root,
            &["clone", &self.origin_url(name), &scratch.to_string_lossy()],
        );
        for (rel, body) in files {
            write_file(&scratch.join(rel), body);
        }
        git(&scratch, &["add", "-A"]);
        git(&scratch, &["commit", "-m", &format!("fixture: {name}")]);
        git(&scratch, &["push", "origin", "HEAD:main"]);
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    /// Advances `name`'s origin by one commit, leaving any existing checkout of
    /// it one commit behind.
    pub fn commit_to_origin(&self, name: &str) {
        let file = format!("upstream-{}.md", COUNTER.fetch_add(1, Ordering::Relaxed));
        self.push_to_origin(name, &[(&file, "added upstream\n")]);
    }

    /// Clones every source repository into the workspace, for tests that need
    /// checkouts present without going through `setup`.
    pub fn clone_repos(&self) {
        for name in REPOS {
            git(&self.workspace, &["clone", &self.origin_url(name), name]);
        }
    }

    /// Runs the compiled binary against this fixture's workspace.
    pub fn run(&self, args: &[&str]) -> Output {
        sanitized(env!("CARGO_BIN_EXE_zpr-dev"))
            .current_dir(&self.root)
            .arg("--workspace")
            .arg(&self.workspace)
            .args(args)
            .output()
            .expect("cannot run the zpr-dev binary")
    }

    /// Reads a workspace-relative file, panicking if it is absent.
    pub fn read(&self, rel: &str) -> String {
        let path = self.workspace.join(rel);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    /// Writes a workspace-relative file, creating parent directories as needed.
    pub fn write(&self, rel: &str, body: &str) {
        write_file(&self.workspace.join(rel), body);
    }

    /// True when a workspace-relative path exists.
    pub fn exists(&self, rel: &str) -> bool {
        self.workspace.join(rel).exists()
    }

    /// Modification time of a workspace-relative file, for asserting that a
    /// second `sync` writes nothing.
    pub fn mtime(&self, rel: &str) -> SystemTime {
        std::fs::metadata(self.workspace.join(rel))
            .unwrap()
            .modified()
            .unwrap()
    }
}
