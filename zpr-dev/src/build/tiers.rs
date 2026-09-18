//! The test tiers of spec-003 §6 (task B4): parsing the `--test` selection,
//! and the `unit` tier — each built repository's own `make test` in build
//! order, preceded in `zl-zpr-visaservice` by `make pregen ZPLC=<dist>/zplc`
//! so the visa service's policy fixtures are compiled by the set's own
//! compiler (the dynamic form of gate 3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::recipes;

/// The tiers this tool implements today, in run order. Approved decision on
/// zipline#61 (Q1): `default` — and no `--test` flag at all — means every
/// implemented tier, so `docker` joins this list when B5 lands and nothing
/// about the flag changes.
const IMPLEMENTED: &[&str] = &["unit"];

/// Every tier spec-003 §6 names, implemented or not, in run order. A known
/// but unimplemented name gets a "lands in B5" error rather than the unknown-
/// name error, so the remedy is obvious from the message.
const KNOWN: &[&str] = &["unit", "netns", "docker"];

/// Which tiers a run executes, parsed from `--test` (spec-003 §7). Empty
/// means `--test none`: build only, no tier runs and none is reported.
#[derive(Debug, PartialEq)]
pub struct Selection {
    /// Tier names in run order, deduplicated.
    tiers: Vec<&'static str>,
}

impl Selection {
    /// Parses the `--test` flag. `None` (flag absent) and `default` select
    /// every implemented tier; `none` selects nothing; `all` asks for every
    /// known tier; otherwise the value is a comma-separated list of tier
    /// names. An unknown name, a known-but-unimplemented name, and the
    /// special words mixed into a list are all usage errors — they surface
    /// as exit 2 through `run`'s `Err` path.
    pub fn parse(flag: Option<&str>) -> Result<Selection> {
        // Absent and `default` are the same selection by definition
        // (approved Q1 on zipline#61): every implemented tier.
        let text = flag.unwrap_or("default");
        match text {
            "default" => {
                return Ok(Selection {
                    tiers: IMPLEMENTED.to_vec(),
                });
            }
            "none" => return Ok(Selection { tiers: Vec::new() }),
            // `all` asks for every known tier, and an asked-for tier that
            // cannot run is an error, never a silent skip (spec-003 §6) —
            // so while any tier is unimplemented, `all` is an error too.
            "all" => {
                bail!(
                    "--test all asks for every tier, but netns and docker land in B5; \
                     use --test unit (or default) until then"
                );
            }
            _ => {}
        }

        // A comma-separated list of tier names. The special whole-selection
        // words are rejected inside a list: `unit,none` has no coherent
        // meaning.
        let mut tiers: Vec<&'static str> = Vec::new();
        for name in text.split(',') {
            let name = name.trim();
            match KNOWN.iter().find(|known| **known == name) {
                Some(known) if IMPLEMENTED.contains(known) => {
                    if !tiers.contains(known) {
                        tiers.push(known);
                    }
                }
                Some(known) => bail!(
                    "--test {known} is not available yet: the netns and docker tiers land in B5"
                ),
                None => bail!(
                    "--test {name:?} is not a tier; valid values: none, default, all, \
                     or a comma-separated list of {}",
                    KNOWN.join(", ")
                ),
            }
        }
        Ok(Selection { tiers })
    }

    /// True when no tier was selected (`--test none`).
    pub fn is_empty(&self) -> bool {
        self.tiers.is_empty()
    }

    /// True when `tier` was selected. Consumed by the wire-up (B4 step 4);
    /// the allow goes with that commit.
    #[allow(dead_code)]
    pub fn contains(&self, tier: &str) -> bool {
        self.tiers.contains(&tier)
    }
}

// ---------------------------------------------------------------------------
// The unit tier (task B4 steps 1-2)
// ---------------------------------------------------------------------------

/// One command the unit tier runs, with the repository's worktree as the
/// working directory. `name` labels the log file and attributes a failure —
/// a failing `pregen` must be named `pregen`, because a `zplc` that cannot
/// compile the visa service's own fixtures is exactly the incompatibility
/// this tier exists to catch.
#[allow(dead_code)] // constructed by the wire-up (B4 step 4)
#[derive(Debug)]
pub struct TierStep {
    pub name: &'static str,
    pub program: &'static str,
    /// Owned, unlike `recipes::Step`: the `ZPLC=<dist>/zplc` argument is
    /// computed per run.
    pub args: Vec<String>,
}

/// One repository's part of the unit-tier plan: commands to run, or a skip
/// with its reason. A repository with nothing to test is reported `skipped`,
/// never silently omitted (issue constraint; approved Q2 on zipline#61).
#[allow(dead_code)] // constructed by the wire-up (B4 step 4)
#[derive(Debug)]
pub enum RepoPlan {
    /// Run `steps` in order in `worktree`; the first failure fails the
    /// repository and the sweep moves on to the next one.
    Run {
        repo: String,
        worktree: PathBuf,
        steps: Vec<TierStep>,
    },
    /// Nothing to run here, and the reason says why.
    Skip { repo: String, reason: String },
}

/// The unit tier's outcome: overall pass/fail and the per-repository
/// breakdown that lands in the emitted manifest's `tiers.unit.repos`
/// (approved Q3 on zipline#61).
#[allow(dead_code)] // constructed by the wire-up (B4 step 4)
#[derive(Debug)]
pub struct TierOutcome {
    /// False when any repository failed. Skips do not fail the tier.
    pub passed: bool,
    /// Repository → `passed`, `failed: ...` (naming the failing step and its
    /// log), or `skipped: ...` (with the reason).
    pub repos: BTreeMap<String, String>,
}

/// Builds the unit tier's plan from the worktrees a build produced: `make
/// test` in each built repository **in build order** (the recipe table's
/// order, whatever order the worktrees arrived in), preceded in
/// `zl-zpr-visaservice` by `make pregen ZPLC=<dist>/zplc` so the policy
/// fixtures its tests load are compiled by the set's own compiler. A
/// repository whose recipe builds nothing has no unit suite to run and is
/// planned as a skip (approved Q2: `zl-zpr-demo`; the docker tier covers it).
/// Pure planning: nothing here executes a command.
#[allow(dead_code)] // called by the wire-up (B4 step 4)
pub fn unit_plan(worktrees: &[(&recipes::Recipe, PathBuf)], dist: &Path) -> Vec<RepoPlan> {
    let mut plans: Vec<RepoPlan> = Vec::new();
    // Iterating the recipe table, not the input, is what makes the plan
    // build-ordered: the table's order is BUILD_ORDER, asserted by test.
    for recipe in recipes::RECIPES {
        let Some((_, worktree)) = worktrees.iter().find(|(wt, _)| wt.repo == recipe.repo) else {
            continue; // not in this set: nothing was built, nothing to test
        };

        // A recipe with no build steps builds nothing, so there is no unit
        // suite to run against the set (approved Q2 on zipline#61).
        if recipe.steps.is_empty() {
            plans.push(RepoPlan::Skip {
                repo: recipe.repo.to_string(),
                reason: "no unit tests (docker tier covers it)".to_string(),
            });
            continue;
        }

        let mut steps: Vec<TierStep> = Vec::new();
        if recipe.repo == "zl-zpr-visaservice" {
            // `pregen` recompiles the visa service's policy fixtures with
            // the set's own zplc before `make test` loads them — the dynamic
            // form of gate 3, and the reason this tier exists (spec-003 §6).
            steps.push(TierStep {
                name: "pregen",
                program: "make",
                args: vec![
                    "pregen".to_string(),
                    format!("ZPLC={}", dist.join("zplc").display()),
                ],
            });
        }
        steps.push(TierStep {
            name: "test",
            program: "make",
            args: vec!["test".to_string()],
        });
        plans.push(RepoPlan::Run {
            repo: recipe.repo.to_string(),
            worktree: worktree.clone(),
            steps,
        });
    }
    plans
}

/// Runs a unit-tier plan: each repository's steps in order, logging through
/// the same shape as the build steps (`logs/<repo>-<step>.log`, last 40
/// lines echoed on failure unless `quiet`). A failing step fails its
/// repository naming the step, and the sweep **keeps going** so one run
/// reports every broken repository rather than only the first (issue
/// step 2). Prints one line per repository unless `quiet`.
#[allow(dead_code)] // called by the wire-up (B4 step 4)
pub fn run_unit(plans: &[RepoPlan], logs: &Path, quiet: bool) -> TierOutcome {
    let mut outcome = TierOutcome {
        passed: true,
        repos: BTreeMap::new(),
    };
    for plan in plans {
        match plan {
            RepoPlan::Skip { repo, reason } => {
                // Visible, never omitted: a tier entry that did not run
                // always says why (issue constraint; approved Q2).
                if !quiet {
                    println!("{repo}: skipped ({reason})");
                }
                outcome
                    .repos
                    .insert(repo.clone(), format!("skipped: {reason}"));
            }
            RepoPlan::Run {
                repo,
                worktree,
                steps,
            } => {
                // The first failing step fails the repository — a failed
                // `pregen` must not let `make test` run against stale
                // fixtures — but the sweep continues to the next repository
                // so one run reports every broken one (issue step 2).
                let failure = steps.iter().find_map(|step| {
                    recipes::run_command(
                        repo,
                        step.name,
                        step.program,
                        &step.args,
                        worktree,
                        logs,
                        quiet,
                    )
                    .err()
                    .map(|error| (step.name, error))
                });
                match failure {
                    None => {
                        if !quiet {
                            println!("{repo}: passed");
                        }
                        outcome.repos.insert(repo.clone(), "passed".to_string());
                    }
                    Some((step, error)) => {
                        // The entry names the failing step and carries the
                        // run_command error, which names the log path.
                        if !quiet {
                            println!("{repo}: FAILED at step `{step}`");
                        }
                        outcome.passed = false;
                        outcome
                            .repos
                            .insert(repo.clone(), format!("failed at `{step}`: {error}"));
                    }
                }
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flag absent entirely selects every implemented tier — today
    /// exactly `unit` (approved Q1 on zipline#61).
    #[test]
    fn absent_flag_selects_the_implemented_tiers() {
        let selection = Selection::parse(None).unwrap();
        assert!(!selection.is_empty());
        assert!(selection.contains("unit"));
        assert!(!selection.contains("netns"));
        assert!(!selection.contains("docker"));
    }

    /// `default` is the spelled-out form of the absent flag.
    #[test]
    fn default_matches_the_absent_flag() {
        assert_eq!(
            Selection::parse(Some("default")).unwrap(),
            Selection::parse(None).unwrap()
        );
    }

    /// `none` selects nothing: build only, byte-identical to B3 behaviour.
    #[test]
    fn none_selects_nothing() {
        let selection = Selection::parse(Some("none")).unwrap();
        assert!(selection.is_empty());
        assert!(!selection.contains("unit"));
    }

    /// A single implemented tier by name.
    #[test]
    fn unit_selects_exactly_unit() {
        let selection = Selection::parse(Some("unit")).unwrap();
        assert!(selection.contains("unit"));
        assert!(!selection.is_empty());
    }

    /// A comma-separated list is accepted, tolerating spaces and duplicates,
    /// as long as every name is implemented.
    #[test]
    fn comma_list_of_implemented_tiers_is_accepted() {
        let selection = Selection::parse(Some("unit, unit")).unwrap();
        assert!(selection.contains("unit"));
    }

    /// A known tier whose stage has not landed is rejected naming the stage,
    /// not treated as unknown: `netns` and `docker` land in B5.
    #[test]
    fn unimplemented_tier_is_rejected_naming_its_stage() {
        for name in ["netns", "docker", "unit,docker"] {
            let error = Selection::parse(Some(name)).unwrap_err().to_string();
            assert!(error.contains("B5"), "{name}: {error}");
        }
    }

    /// An unknown name is rejected listing the valid values, so the fix is
    /// obvious from the message (exit 2 through `run`'s `Err` path).
    #[test]
    fn unknown_tier_is_rejected_listing_the_valid_names() {
        for flag in ["bogus", "unit,bogus", ""] {
            let error = Selection::parse(Some(flag)).unwrap_err().to_string();
            assert!(error.contains("unit"), "{flag}: {error}");
            assert!(error.contains("none"), "{flag}: {error}");
        }
    }

    /// `none`, `default` and `all` describe whole selections, not tiers:
    /// mixing them into a list is a usage error.
    #[test]
    fn special_words_in_a_list_are_rejected() {
        for flag in ["unit,none", "none,unit", "unit,default", "unit,all"] {
            assert!(Selection::parse(Some(flag)).is_err(), "{flag}");
        }
    }

    /// `all` asks for every known tier, and today that is an error naming
    /// the unimplemented ones — an asked-for tier that cannot run must never
    /// silently degrade (spec-003 §6).
    #[test]
    fn all_is_an_error_while_tiers_are_unimplemented() {
        let error = Selection::parse(Some("all")).unwrap_err().to_string();
        assert!(error.contains("B5"), "{error}");
    }

    // -- the unit tier's plan (issue steps 1-2) --------------------------------

    /// Worktrees for the named recipe-table repositories, each in a fake
    /// directory named after its repository.
    fn fixture_worktrees(repos: &[&str]) -> Vec<(&'static recipes::Recipe, PathBuf)> {
        repos
            .iter()
            .map(|name| {
                let recipe = recipes::RECIPES
                    .iter()
                    .find(|recipe| recipe.repo == *name)
                    .expect("fixture names a recipe-table repository");
                (recipe, PathBuf::from(format!("/wt/{name}")))
            })
            .collect()
    }

    /// The plan for a repository whose recipe built something is `make test`
    /// in its worktree; `zl-zpr-visaservice` gets `make pregen ZPLC=<dist>/zplc`
    /// prepended, named `pregen` so a failure attributes to the step that
    /// proves compiler compatibility.
    #[test]
    fn unit_plan_runs_make_test_with_pregen_first_in_the_visa_service() {
        let worktrees = fixture_worktrees(&["zl-zpr-compiler", "zl-zpr-visaservice"]);
        let plans = unit_plan(&worktrees, Path::new("/build/dist"));
        assert_eq!(plans.len(), 2);

        let RepoPlan::Run { repo, steps, .. } = &plans[0] else {
            panic!("compiler must run, not skip: {:?}", plans[0]);
        };
        assert_eq!(repo, "zl-zpr-compiler");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].name, "test");
        assert_eq!(steps[0].program, "make");
        assert_eq!(steps[0].args, ["test"]);

        let RepoPlan::Run {
            repo,
            worktree,
            steps,
        } = &plans[1]
        else {
            panic!("visa service must run, not skip: {:?}", plans[1]);
        };
        assert_eq!(repo, "zl-zpr-visaservice");
        assert_eq!(worktree, Path::new("/wt/zl-zpr-visaservice"));
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].name, "pregen");
        assert_eq!(steps[0].program, "make");
        assert_eq!(steps[0].args, ["pregen", "ZPLC=/build/dist/zplc"]);
        assert_eq!(steps[1].name, "test");
        assert_eq!(steps[1].args, ["test"]);
    }

    /// A repository whose recipe builds nothing has no unit suite: it is
    /// planned as a skip with the reason, never omitted (approved Q2 on
    /// zipline#61 — `zl-zpr-demo`; the docker tier covers it).
    #[test]
    fn unit_plan_skips_a_repository_that_builds_nothing_with_a_reason() {
        let worktrees = fixture_worktrees(&["zl-zpr-demo"]);
        let plans = unit_plan(&worktrees, Path::new("/build/dist"));
        assert_eq!(plans.len(), 1);
        let RepoPlan::Skip { repo, reason } = &plans[0] else {
            panic!("demo must skip, not run: {:?}", plans[0]);
        };
        assert_eq!(repo, "zl-zpr-demo");
        assert!(reason.contains("no unit tests"), "{reason}");
        assert!(reason.contains("docker"), "{reason}");
    }

    /// The plan is in build order — the recipe table's order — whatever
    /// order the worktrees arrived in.
    #[test]
    fn unit_plan_is_in_build_order_regardless_of_input_order() {
        let worktrees = fixture_worktrees(&["zl-zpr-coredns", "zl-zpr-compiler"]);
        let plans = unit_plan(&worktrees, Path::new("/d"));
        let repos: Vec<&str> = plans
            .iter()
            .map(|plan| match plan {
                RepoPlan::Run { repo, .. } | RepoPlan::Skip { repo, .. } => repo.as_str(),
            })
            .collect();
        assert_eq!(repos, ["zl-zpr-compiler", "zl-zpr-coredns"]);
    }

    // -- the unit tier's runner (issue steps 1-2) ------------------------------

    /// A one-step run plan whose command is a shell fixture.
    fn fixture_plan(repo: &str, dir: &Path, name: &'static str, script: &str) -> RepoPlan {
        RepoPlan::Run {
            repo: repo.to_string(),
            worktree: dir.to_path_buf(),
            steps: vec![TierStep {
                name,
                program: "sh",
                args: vec!["-c".to_string(), script.to_string()],
            }],
        }
    }

    /// A passing sweep: every repository records `passed`, the tier passes,
    /// and each step's output landed in its log.
    #[test]
    fn run_unit_records_passes_and_writes_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let plans = vec![fixture_plan("repo-a", tmp.path(), "test", "echo tested")];
        let outcome = run_unit(&plans, &logs, true);
        assert!(outcome.passed);
        assert_eq!(outcome.repos["repo-a"], "passed");
        let log = std::fs::read_to_string(logs.join("repo-a-test.log")).unwrap();
        assert!(log.contains("tested"), "{log}");
    }

    /// A failing repository fails the tier naming the failing step and the
    /// log path in its entry — and the sweep keeps going, so a later
    /// repository still runs and reports (issue step 2).
    #[test]
    fn run_unit_keeps_going_after_a_failure_and_names_the_step() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let plans = vec![
            fixture_plan("repo-bad", tmp.path(), "pregen", "echo mismatch; exit 1"),
            fixture_plan("repo-good", tmp.path(), "test", "echo fine"),
        ];
        let outcome = run_unit(&plans, &logs, true);
        assert!(!outcome.passed);
        let bad = &outcome.repos["repo-bad"];
        assert!(bad.starts_with("failed"), "{bad}");
        assert!(bad.contains("pregen"), "failing step not named: {bad}");
        assert!(bad.contains("repo-bad-pregen.log"), "log not named: {bad}");
        // The sweep kept going: the later repository ran and passed.
        assert_eq!(outcome.repos["repo-good"], "passed");
    }

    /// A failing first step stops that repository's later steps — `pregen`
    /// failing means its `make test` must not run against stale fixtures —
    /// while the sweep still proceeds to other repositories.
    #[test]
    fn run_unit_stops_a_repository_at_its_first_failing_step() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let marker = tmp.path().join("ran-anyway");

        let plans = vec![RepoPlan::Run {
            repo: "repo-vs".to_string(),
            worktree: tmp.path().to_path_buf(),
            steps: vec![
                TierStep {
                    name: "pregen",
                    program: "sh",
                    args: vec!["-c".to_string(), "exit 1".to_string()],
                },
                TierStep {
                    name: "test",
                    program: "sh",
                    args: vec!["-c".to_string(), format!("touch {}", marker.display())],
                },
            ],
        }];
        let outcome = run_unit(&plans, &logs, true);
        assert!(!outcome.passed);
        assert!(outcome.repos["repo-vs"].contains("pregen"));
        assert!(!marker.exists(), "test step ran after pregen failed");
    }

    /// A planned skip lands in the outcome as `skipped: <reason>` and does
    /// not fail the tier — visible, never omitted (approved Q2).
    #[test]
    fn run_unit_reports_skips_without_failing_the_tier() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();

        let plans = vec![
            fixture_plan("repo-a", tmp.path(), "test", "true"),
            RepoPlan::Skip {
                repo: "zl-zpr-demo".to_string(),
                reason: "no unit tests (docker tier covers it)".to_string(),
            },
        ];
        let outcome = run_unit(&plans, &logs, true);
        assert!(outcome.passed, "a skip must not fail the tier");
        assert_eq!(
            outcome.repos["zl-zpr-demo"],
            "skipped: no unit tests (docker tier covers it)"
        );
    }
}
