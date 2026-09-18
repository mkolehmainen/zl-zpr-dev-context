//! The five build recipes of spec-003 §5 (contract 4 of the master plan) and
//! their staging lists, plus the step runner that shells out with the worktree
//! as the working directory, tees output to `logs/<repo>-<step>.log`, and
//! echoes the last 40 lines on a non-zero exit.
//!
//! Each repository's own `Makefile` stays authoritative: nothing here knows a
//! `cargo` flag a repository's build already knows, and `zl-zpr-visaservice`'s
//! `make release` staging is reused (its `build-release/` is the staging
//! source), not reimplemented. Actual compilation is exercised by the
//! acceptance run; the unit tests cover the table's shape.

use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

/// One command a recipe runs, with the worktree as its working directory.
/// `name` labels the log file (`logs/<repo>-<name>.log`).
#[derive(Debug)]
pub struct Step {
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
}

/// One binary a recipe stages into `dist/`: the worktree-relative path it is
/// built at, and the name it is staged under.
#[derive(Debug)]
pub struct Staged {
    pub name: &'static str,
    pub source: &'static str,
}

/// One repository's build recipe: the steps to run in order, and what to
/// stage afterwards.
#[derive(Debug)]
pub struct Recipe {
    pub repo: &'static str,
    pub steps: &'static [Step],
    pub staged: &'static [Staged],
}

/// The contract-4 table, in build order — the same order as
/// [`super::BUILD_ORDER`], asserted by test. Step 1 comes first because the
/// visa service's `pregen` and the demo's policy compilation both need `zplc`;
/// step 5 builds nothing because the `docker` tier copies `dist/` into
/// `dns-demo/bin/` instead (spec-003 §5).
pub const RECIPES: &[Recipe] = &[
    Recipe {
        repo: "zl-zpr-compiler",
        steps: &[Step {
            name: "build",
            program: "cargo",
            args: &["build", "--release"],
        }],
        staged: &[
            Staged {
                name: "zplc",
                source: "target/release/zplc",
            },
            Staged {
                name: "zpdump",
                source: "target/release/zpdump",
            },
        ],
    },
    Recipe {
        // `make release` runs its own `make clean` first, so the visa service
        // is compiled twice per set (release here, debug again if the unit
        // tier's `make test` runs). Recorded in contract 4, not optimized.
        repo: "zl-zpr-visaservice",
        steps: &[Step {
            name: "release",
            program: "make",
            args: &["release"],
        }],
        staged: &[
            Staged {
                name: "vs",
                source: "build-release/vs",
            },
            Staged {
                name: "vs-admin",
                source: "build-release/vs-admin",
            },
            Staged {
                name: "vsapikey",
                source: "build-release/vsapikey",
            },
            Staged {
                name: "zpt",
                source: "build-release/zpt",
            },
            Staged {
                name: "zpr-dashboard",
                source: "build-release/zpr-dashboard",
            },
        ],
    },
    Recipe {
        repo: "zl-zpr-core",
        steps: &[Step {
            name: "build",
            program: "cargo",
            args: &["build", "--release"],
        }],
        staged: &[
            Staged {
                name: "ph",
                source: "target/release/ph",
            },
            Staged {
                name: "ph-cli",
                source: "target/release/ph-cli",
            },
        ],
    },
    Recipe {
        repo: "zl-zpr-coredns",
        steps: &[Step {
            name: "build",
            program: "make",
            args: &["build"],
        }],
        staged: &[Staged {
            name: "coredns",
            source: "bin/coredns",
        }],
    },
    Recipe {
        // The demo builds nothing: its Makefile only populates dns-demo/bin/
        // from sibling repositories, and the docker tier (B5) copies dist/
        // there instead so the DNS test exercises the set's own binaries.
        repo: "zl-zpr-demo",
        steps: &[],
        staged: &[],
    },
];

/// The recipe for one repository, when it has one. `None` is not an error:
/// a build set may resolve repositories outside the recipe table, and the
/// caller reports them rather than building them.
pub fn recipe_for(repo: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|recipe| recipe.repo == repo)
}

/// Runs one step with `worktree` as the working directory, writing combined
/// stdout+stderr to `logs/<repo>-<step>.log`. On a non-zero exit the last 40
/// lines of that log are echoed (unless `quiet`) and the error names the
/// repository, the step and the log path, so the failure is diagnosable from
/// the report alone.
pub fn run_step(repo: &str, step: &Step, worktree: &Path, logs: &Path, quiet: bool) -> Result<()> {
    let log_path = logs.join(format!("{repo}-{}.log", step.name));

    let output = Command::new(step.program)
        .args(step.args)
        .current_dir(worktree)
        .output()
        .map_err(|e| {
            anyhow::anyhow!(
                "cannot run {} for {repo} in {}: {e}",
                step.program,
                worktree.display()
            )
        })?;

    // One log per step, stdout then stderr: the tee'd record a person reads
    // when a build fails, and the source of the 40-line echo below.
    let mut log = std::fs::File::create(&log_path)
        .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", log_path.display()))?;
    log.write_all(&output.stdout)?;
    log.write_all(&output.stderr)?;

    if output.status.success() {
        return Ok(());
    }

    if !quiet {
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(40);
        eprintln!(
            "--- last {} lines of {} ---",
            lines.len() - start,
            log_path.display()
        );
        for line in &lines[start..] {
            eprintln!("{line}");
        }
    }
    bail!(
        "{repo}: step `{}` failed ({}); full output in {}",
        step.name,
        output.status,
        log_path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every repository in build order has exactly one recipe, in that order —
    /// the table and `BUILD_ORDER` must never drift apart.
    #[test]
    fn recipes_cover_build_order_in_order() {
        let recipe_repos: Vec<&str> = RECIPES.iter().map(|r| r.repo).collect();
        assert_eq!(recipe_repos, super::super::BUILD_ORDER.to_vec());
    }

    /// The staged names across all recipes are exactly the ten binaries of
    /// spec-003 §5, each staged once.
    #[test]
    fn staged_names_are_exactly_the_ten_binaries() {
        let mut names: Vec<&str> = RECIPES
            .iter()
            .flat_map(|r| r.staged.iter().map(|s| s.name))
            .collect();
        names.sort_unstable();
        let mut expected = vec![
            "coredns",
            "ph",
            "ph-cli",
            "vs",
            "vs-admin",
            "vsapikey",
            "zpdump",
            "zplc",
            "zpr-dashboard",
            "zpt",
        ];
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    /// Contract 4 row by row: the command each recipe runs and where it
    /// stages from. `zl-zpr-visaservice` stages from its own `build-release/`
    /// (its `make release` staging is reused, not reimplemented) and
    /// `zl-zpr-demo` builds nothing.
    #[test]
    fn recipe_commands_and_staging_match_contract_4() {
        let by_repo = |name: &str| RECIPES.iter().find(|r| r.repo == name).unwrap();

        let compiler = by_repo("zl-zpr-compiler");
        assert_eq!(compiler.steps.len(), 1);
        assert_eq!(compiler.steps[0].program, "cargo");
        assert_eq!(compiler.steps[0].args, ["build", "--release"]);
        assert!(
            compiler
                .staged
                .iter()
                .all(|s| s.source.starts_with("target/release/"))
        );

        let vs = by_repo("zl-zpr-visaservice");
        assert_eq!(vs.steps.len(), 1);
        assert_eq!(vs.steps[0].program, "make");
        assert_eq!(vs.steps[0].args, ["release"]);
        assert!(
            vs.staged
                .iter()
                .all(|s| s.source.starts_with("build-release/"))
        );
        assert_eq!(vs.staged.len(), 5);

        let core = by_repo("zl-zpr-core");
        assert_eq!(core.steps[0].program, "cargo");
        assert_eq!(core.steps[0].args, ["build", "--release"]);

        let coredns = by_repo("zl-zpr-coredns");
        assert_eq!(coredns.steps[0].program, "make");
        assert_eq!(coredns.steps[0].args, ["build"]);
        assert_eq!(coredns.staged[0].source, "bin/coredns");

        let demo = by_repo("zl-zpr-demo");
        assert!(demo.steps.is_empty());
        assert!(demo.staged.is_empty());
    }

    /// Staging sources are worktree-relative: never absolute, never escaping
    /// the worktree with `..`.
    #[test]
    fn staging_sources_are_worktree_relative() {
        for recipe in RECIPES {
            for staged in recipe.staged {
                assert!(
                    !staged.source.starts_with('/') && !staged.source.contains(".."),
                    "{}: {}",
                    recipe.repo,
                    staged.source
                );
            }
        }
    }

    /// `run_step` writes the step's combined output to the named log file and
    /// succeeds on exit 0.
    #[test]
    fn run_step_logs_output_and_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let step = Step {
            name: "hello",
            program: "sh",
            args: &["-c", "echo out; echo err >&2"],
        };
        run_step("fixture", &step, tmp.path(), &logs, false).unwrap();

        let log = std::fs::read_to_string(logs.join("fixture-hello.log")).unwrap();
        assert!(log.contains("out"), "{log}");
        assert!(log.contains("err"), "{log}");
    }

    /// A failing step errors naming the repository, the step and the log
    /// path, and the log retains the output that explains the failure.
    #[test]
    fn run_step_failure_names_repo_step_and_log() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let step = Step {
            name: "boom",
            program: "sh",
            args: &["-c", "echo the reason; exit 3"],
        };
        let error = run_step("fixture", &step, tmp.path(), &logs, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("fixture"), "{error}");
        assert!(error.contains("boom"), "{error}");
        assert!(error.contains("fixture-boom.log"), "{error}");

        let log = std::fs::read_to_string(logs.join("fixture-boom.log")).unwrap();
        assert!(log.contains("the reason"), "{log}");
    }
}
