# Build Sets Plan — reproducible, compatibility-gated builds of the whole ZPR binary set

**Status:** COMPLETE (2026-09-21) — umbrella [zipline#57](https://github.com/mkolehmainen/zipline/issues/57); B1–B6 all landed. Gate 1's transitive-pin blind spot is follow-up work: [zipline#69](https://github.com/mkolehmainen/zipline/issues/69) (see *Finding 4*).
**Date:** 2026-09-17
**Repo state this plan was written against:** `zl-zpr-dev-context` @ `8bfd061`, `zl-zpr-core` @ `dd43b4b`, `zl-zpr-visaservice` @ `2cf9912`, `zl-zpr-compiler` @ `f63302c` (package version `0.18.0`), `zl-zpr-coredns` @ `f4202c4`, `zl-zpr-demo` @ `8ef671f`, `zl-zpr-common` @ `5fbbff8` (tag `v0.27.0`; consumers pin `v0.26.0`).

> Process note: per `skills/zpr/SKILL.md`, each task below becomes one GitHub issue in `mkolehmainen/zipline` and is worked on a feature branch off `zipline`, PR targeting `zipline`. Read that skill before filing. Every task in this plan lands in **one** repository, `zl-zpr-dev-context`.

**Goal.** One command builds the whole binary set — `vs`, `vs-admin`, `vsapikey`, `zpt`, `zpr-dashboard`, `zplc`, `zpdump`, `ph`, `ph-cli`, `coredns` — from a manifest that names a tag, branch or commit per repository, or from the tip of each default branch. Before compiling anything it *proves* the set is coherent; after compiling it runs every integration test that exists against those exact binaries; and it emits a manifest that rebuilds the same set later.

---

## Why this is needed

Nothing today says "these binaries go together", and each of the three things that makes a set compatible is invisible at build time.

**1. Cargo pins, not checkouts, decide what compiles.** Shared crates are consumed from Git by tag (`docs/BUILD.md`, "Cross-repository dependencies"). `zl-zpr-core`, `zl-zpr-visaservice` and `zl-zpr-compiler` each pin the `zpr` crate — today all three say `v0.26.0`, by diligence rather than by any check. A checkout of `zl-zpr-common` in the workspace has no effect on any of them. Related pins already disagree: `zl-zpr-common/Cargo.toml:21` pins `rcu` at tag `zpr-utils-v0.1.0` while `zl-zpr-core/adapter/ph/Cargo.toml:36` pins `rcu-v0.1.2`. (Resolved 2026-09-21 — see *Finding 1*.)

**2. The visa service hardcodes the compiler version it will accept.** `zl-zpr-visaservice/vs/src/config.rs:29-31` sets `POLICY_MIN_COMPILER_{MAJOR,MINOR,PATCH} = 0, 18, 0`, and `libeval/src/pio.rs`'s `check_version` requires **major equal, minor equal, patch greater or equal** — not a floor, a near-exact match. `zl-zpr-compiler` stamps its `CARGO_PKG_VERSION` into every `PolicyContainer` (`src/compiler.rs:2`, `src/policybinaryv2.rs:208-210`). A set whose `zplc` is `0.19.x` against a `vs` wanting `0.18.0` builds perfectly and then fails to load any policy at runtime.

**3. The tests that would catch a bad set need foreign binaries staged by hand.** `zl-zpr-core/integration-test/` wants `vs` and `vs-admin` copied in from `zl-zpr-visaservice`; `zl-zpr-demo/dns-demo/` wants all ten binaries in `bin/`; `zl-zpr-visaservice/integration-test/pregen` wants `zplc` on `PATH`. So the one thing that would answer "is this set compatible?" is the thing nobody runs routinely.

---

## Architecture

A new `zpr-dev build` subcommand. `zpr-dev` already owns `workspace.yaml`, workspace and context path resolution, the `git` plumbing in `src/git.rs`, `--dry-run`, and the 0/1/2 exit-code contract; a build set is one more thing described by a manifest and applied to the same repository list.

```
build-sets/2026-09-17.yaml          workspace.yaml
  (refs: tag | branch | sha)          (url, default_branch)
            \                          /
             \                        /
              v                      v
        +-------------------------------------+
        |  zpr-dev build                      |
        |  1. resolve every ref -> sha        |
        |  2. git worktree --detach per repo  |   <-- live checkouts untouched
        |  3. GATES (no compilation yet)      |
        |       a. shared git-dep agreement   |
        |       b. freshness warning          |
        |       c. zplc <-> vs min version    |
        |  4. build in dependency order       |
        |  5. stage into dist/                |
        |  6. run test tiers against dist/    |
        |  7. emit manifest + tarball         |
        +-------------------------------------+
                          |
                          v
   .zpr-build/<name>/dist/  ten binaries
                            zpr-set-<name>.yaml   <-- rebuilds this set
                            zpr-set-<name>-linux-<arch>.tar.gz
```

The emitted manifest is the deliverable that makes a set nameable: it is valid input to a later `zpr-dev build`, so "the set we shipped in September" is a file, not a memory.

---

## Global constraints

- **The live workspace is never modified.** No branch switch, no checkout, no fetch of a source repository's working tree, no build inside it. Sources come from `git worktree add --detach`, which shares the object store and leaves `HEAD`, the current branch and any dirty files alone — the same guarantee `zpr-dev update` makes today (`zpr-dev/README.md`, "`zpr-dev update`"). A repository that is missing, not a Git repository, or lacks the requested ref is an error naming the repository and the ref.
- **Gates run before compilation.** A set whose pins disagree must fail in seconds, not after fifteen minutes of `cargo`. Findings accumulate and print together in the style of `zpr-dev validate`, then the command exits 1.
- **Each repository's own `Makefile` stays authoritative.** Recipes here are thin wrappers (`docs/BUILD.md`: a repository's `README.md` and `Makefile` are correct and this documentation follows). `zpr-dev build` does not learn `cargo` flags a repository's `Makefile` already knows, and does not reimplement `zl-zpr-visaservice`'s `make release` staging.
- **No manifest of a built repository is ever rewritten.** No `[patch]`, no path dependency, no `Cargo.lock` edit. `docs/BUILD.md` records why a bare `[patch]` is a trap and why a locally-pathed `Cargo.lock` must never reach a PR; this tool creates no situation where either could.
- **A skipped test is reported as skipped, with its reason.** A green run must never overstate coverage. This is a release gate for a security system; "we did not run the netns tests on this machine" has to be visible in the output *and* in the emitted manifest.
- **Reproducible inputs, not reproducible binaries.** The emitted manifest guarantees the same *sources* and the same *pins*. Byte-identical binaries are not claimed: `cargo` output varies with toolchain and path. The manifest records `sha256` per binary so a later rebuild can be compared, and records the toolchain versions that produced them.
- **Rust gates.** `cargo fmt --check`, `cargo build` with warnings denied, `cargo test`; every non-trivial function carries a comment; a bug gets a failing test before its fix (`AGENTS.md`, `skills/rust-coding-guidelines/SKILL.md`).
- **Dependencies stay minimal.** `spec-001` §6.1 deliberately excludes an async runtime, an HTTP client, regex, colors and progress bars. This work adds exactly two crates — `toml` (to read pins out of `Cargo.toml`) and `sha2` (digests) — and amends §6.1 to say so. No `cargo-metadata`, no `git2`.

---

## Cross-repository interface contracts

### 1. Input manifest (new; `zl-zpr-dev-context/build-sets/<name>.yaml`)

```yaml
version: 1
name: 2026-09-17              # names the build and its dist directory

repositories:                 # value is a tag, branch or sha; resolved to a sha
  zl-zpr-compiler:    v0.18.0
  zl-zpr-visaservice: v0.19.0-rc.1
  zl-zpr-core:        zpr-0.3.1
  zl-zpr-coredns:     f4202c4
  zl-zpr-demo:        demo-20251010

allow_pin_drift:              # optional; each entry MUST carry a reason
  - crate: rcu
    reason: "zl-zpr-common pins zpr-utils-v0.1.0, ph pins rcu-v0.1.2; zipline#18"
```

- Only **binary-producing** repositories appear. `zl-zpr-common`, `zl-zpr-policy` and `zl-zpr-vsapi` are deliberately absent: they are consumed by `cargo` from Git by tag, so pinning them here would describe something that does not happen. The set's `zl-zpr-common` version is *derived* from what the consumers agree on (gate 1) — building `zpr-common` is not tied to a version; the agreement is what matters.
- A key that is not a repository in `workspace.yaml` is an error. `url` and `default_branch` come from `workspace.yaml`; nothing is duplicated here.
- `version: 1` is the only accepted version. Unknown keys are ignored, matching `workspace.yaml`'s tolerance (`src/config.rs`, "no `deny_unknown_fields` anywhere"), so the schema can grow ahead of the tool.
- `--tip` ignores every value and uses `origin/<default_branch>` from `workspace.yaml` — `zipline` for the forks, `main` for `zl-zpr-coredns`.

### 2. Emitted manifest (new; `dist/zpr-set-<name>.yaml`) — the reproducibility contract

The **same schema as the input**, with every ref replaced by its resolved 40-character sha, plus a diagnostic `resolved:` block that a later read ignores:

```yaml
version: 1
name: 2026-09-17

repositories:
  zl-zpr-compiler:    3f1c0a9e...   # 40-char sha, always
  zl-zpr-visaservice: 2cf9912b...
  zl-zpr-core:        dd43b4b1...
  zl-zpr-coredns:     f4202c4d...
  zl-zpr-demo:        8ef671f0...

allow_pin_drift:
  - crate: rcu
    reason: "..."

resolved:                          # diagnostic; ignored as input
  built_at: 2026-09-17T18:22:04Z
  built_from: { manifest: build-sets/2026-09-17.yaml, context: 8bfd061, tip: false }
  host: { os: linux, arch: x86_64, rustc: 1.9x.y, cargo: 1.9x.y, go: 1.2x.y, capnp: 1.0.2 }
  pins:                            # what cargo actually compiled against
    - { crate: zpr, url: "https://github.com/mkolehmainen/zl-zpr-common.git", tag: v0.26.0,
        pinned_by: [zl-zpr-core, zl-zpr-core/adapter/ph, zl-zpr-visaservice, zl-zpr-compiler],
        newest_available: v0.27.0 }
    - { crate: rcu, url: "...zl-zpr-utils.git", tag: rcu-v0.1.2, pinned_by: [...] }
  versions:
    zplc: "0.18.0"
    vs_policy_min_compiler: "0.18.0"
  binaries:
    - { name: vs, sha256: "...", bytes: 18234512, from: zl-zpr-visaservice }
    # ... one per staged binary
  tiers:
    unit:   { status: passed }
    netns:  { status: skipped, reason: "no passwordless sudo" }
    docker: { status: failed,  reason: "test-dns.sh section 3 (liveness)" }
```

Normative behavior:

- `zpr-dev build --manifest dist/zpr-set-2026-09-17.yaml` must resolve to exactly the shas listed and recompute the same `pins` block. Nothing outside the file (and the repositories it names) is needed.
- The manifest is emitted **whenever the gates pass**, even if a build step or a test tier fails, with the failure recorded. The most interesting set to reproduce is the one that broke.
- `sha256` per binary is for *comparison*, not for a reproducibility claim (see Global constraints).
- Promoting a `--tip` run to a named set is: run it, review, copy `dist/zpr-set-<name>.yaml` into `build-sets/`, commit. No separate authoring step.

### 3. Gate rules (normative)

**Gate 1 — shared-dep agreement (error).** Parse every `Cargo.toml` in every worktree, including workspace members, and collect each dependency declared with `git = <url>` where the URL host/owner is ZPR-family (`github.com/mkolehmainen/*`, `github.com/org-zpr/*`) or otherwise pinned by `rev` (the `emilazy/capnproto-rust` fork). Group by `(crate, url)` and require a single `tag` (or `rev`) across the whole set. Report every disagreement as:

```
ERROR pin disagreement: crate `zpr` (https://github.com/mkolehmainen/zl-zpr-common.git)
  v0.26.0  zl-zpr-core/Cargo.toml:29 (+ adapter/ph, inherited)
  v0.27.0  zl-zpr-visaservice/Cargo.toml:29
  => cut one tag and bump every consumer, or record it in allow_pin_drift
```

The same crate at the same tag from **different URLs** is also a disagreement: `docs/BUILD.md` records that this is what gives cargo two `cslab` crates from one commit and silently breaks `RcuBox<RcuCslabReader<T>>`. An entry in `allow_pin_drift` suppresses one crate's finding and its reason is echoed in the output and the emitted manifest; `--allow-pin-drift` downgrades all findings to warnings for a one-off.

**Gate 2 — freshness (warning, never an error).** For each agreed pin whose repository is in the workspace, list tags in that checkout and warn when a newer one exists:

```
WARN  crate `zpr` is pinned at v0.26.0; zl-zpr-common has v0.27.0
```

A set may legitimately sit behind — `docs/BUILD.md`: "Consumers can legitimately sit on different tags for a while". The warning exists so that is a decision, not an accident. A missing local checkout is an `INFO`, not a failure: the tag is fetched by cargo from GitHub either way.

**Gate 3 — compiler / visa service version (error).** Read `POLICY_MIN_COMPILER_MAJOR`, `_MINOR`, `_PATCH` from the visa-service worktree's `vs/src/config.rs` and `version` from `[package]` in the compiler worktree's `Cargo.toml`. Apply `libeval::pio::check_version`'s rule exactly — major equal, minor equal, patch greater or equal — and on failure report:

```
ERROR zplc 0.19.0 cannot produce policy for this vs
  zl-zpr-compiler/Cargo.toml:3                version = "0.19.0"
  zl-zpr-visaservice/vs/src/config.rs:29-31   POLICY_MIN_COMPILER = 0.18.0
  rule: major ==, minor ==, patch >=  (libeval/src/pio.rs check_version)
```

A value that cannot be parsed from either file is an error naming the file: silently skipping this check would be worse than failing, because the whole point is that nothing checks it today. The gate is duplicated dynamically by the `unit` tier's `pregen` step (contract 5), which recompiles the visa service's policy fixtures with the set's own `zplc`.

### 4. Build order, recipes and `dist/` contents

Ordered so each step's output is available to the next. Everything is a release build — what a demo and a release tarball use.

| # | Repository | Command(s) | Staged into `dist/` |
|---|---|---|---|
| 1 | `zl-zpr-compiler` | `cargo build --release` | `zplc`, `zpdump` |
| 2 | `zl-zpr-visaservice` | `make release` | `vs`, `vs-admin`, `vsapikey`, `zpt`, `zpr-dashboard` |
| 3 | `zl-zpr-core` | `cargo build --release` | `ph`, `ph-cli` |
| 4 | `zl-zpr-coredns` | `make build` (Go) | `coredns` |
| 5 | `zl-zpr-demo` | none | none |

- Step 1 comes first because the visa service's `pregen` and the demo's policy compilation both need `zplc`.
- Step 2 reuses `zl-zpr-visaservice`'s existing `make release`, which already runs `make clean`, `cargo build -r --all-targets`, builds `zpr-dashboard`, writes `build-release/vs_sysinfo.txt` and tars the result. `dist/` is populated by copying `build-release/`, and that repository's own tarball is kept beside the set's. The `make clean` means the visa service is compiled twice (release for `dist/`, debug for `make test`); that is the price of not reimplementing its release staging, and it is recorded here rather than optimized away.
- Step 5 builds nothing: `zl-zpr-demo/dns-demo/Makefile` exists only to populate `dns-demo/bin/` from sibling repositories, so the `docker` tier copies `dist/` into `dns-demo/bin/` instead. `dist/` and that `bin/` hold exactly the same ten names, so the DNS test then exercises the set's own binaries rather than a fresh build of something adjacent to it.
- `zl-zpr-common`, `zl-zpr-policy`, `zl-zpr-vsapi` are never built: cargo fetches `zpr` by tag. `zl-zpr-utils` is not built either — `docs/BUILD.md`: "the `zl-zpr-utils` checkout in the workspace is not what any build consumes."
- Per-step output goes to `logs/<repo>-<step>.log`, and the last 40 lines are echoed on failure.

### 5. Test tiers and the environment contract with the test scripts

| Tier | What runs | Prerequisites | Default |
|---|---|---|---|
| `unit` | `make test` in each built repository, with `make pregen ZPLC=<dist>/zplc` first in `zl-zpr-visaservice` | none beyond the build | run |
| `netns` | `zl-zpr-core/integration-test/`: `one-node-test.sh`, `one-node-v6-test.sh`, `one-node-oidc-test.sh`, `capture-test.sh`, `oidc-file-interplay-test.sh`, `fake-idp-smoke-test.sh`, `a2a-pubkey-test.sh` | Linux, passwordless `sudo`, `valkey-server`, `python3` | `--test netns` |
| `docker` | `zl-zpr-demo/dns-demo`: `local-compute/deploy-docker.sh`, then `local-compute/test-dns.sh`, then `docker compose down -v` | `docker`, `docker compose` | `--test docker` |

- **`unit` already includes the visa service's integration tests.** `zl-zpr-visaservice/Makefile`'s `test` target runs `cargo test`, the `zpr-dashboard` Go tests, builds `zpt`, and then `make -C integration-test` (`zpt-test.sh`, `zpt-test-connect.sh`, `zpt-test-oidc.sh`, `tag-test.sh`, `admin-actor-test.sh`). Running `make pregen ZPLC=<dist>/zplc` first means those tests load policy compiled by the set's own `zplc` — the dynamic form of gate 3. No separate tier: duplicating that Makefile's sequencing here would only drift from it.
- **`netns` binaries come from `dist/`, not from hand-staged copies.** The scripts already support it (`one-node-test.sh:9-13`): export `PH_BIN`, `PH_DEBUG_BIN`, `VS_BIN`, `VS_ADMIN_BIN` at `dist/` and `VALKEY_SERVER_BIN` at the system `valkey-server`, so nothing is copied into `integration-test/`. `ZPR_TEST_VERBOSE=1` under `--verbose`; `DEBUG_TARGETS` left at its default.
- **`a2a-pubkey-test.sh` needs a differently-built `ph`.** It launches the node with `--security-testing-mangle-forwarded-pings`, which requires `cargo build -p ph --features enable-security-testing`. That build stays in the worktree's `target/`, is pointed at with `PH_BIN` for that one script, and must **never** be staged into `dist/`. If that feature build fails, the one script is reported failed and the rest of the tier still runs.
- **The script list is explicit, not globbed.** `integration-test/unused_or_outdated/` stays out, and adding a script to the set's gate is a reviewed change. Same for the visa service list, which lives in its own Makefile.
- **Prerequisite probing never silently reduces coverage.** A tier that was asked for and cannot run is an error under `--test all`; a tier that was not asked for is reported `skipped` with the reason. `sudo -n true` is the sudo probe; nothing is ever run under `sudo` that the scripts do not run themselves.
- The `docker` tier always runs `docker compose down -v` in the demo, pass or fail, and reports if teardown itself fails.

### 6. CLI surface (new; `zpr-dev/src/main.rs`)

```text
zpr-dev build [--manifest <path> | --tip] [--test <list>] [--repo <name>]
              [--build-dir <path>] [--keep] [--allow-pin-drift] [--no-tarball]

--manifest <path>   build set to build; default: newest file in build-sets/
--tip               ignore every ref; use origin/<default_branch> everywhere
--test <list>       none | default | unit | netns | docker | all (comma-separated)
--repo <name>       build only this repository and its prerequisites
--build-dir <path>  default <workspace>/.zpr-build/<name>
--keep              keep worktrees after a successful run
--allow-pin-drift   gate 1 disagreements warn instead of failing
--no-tarball        skip the dist tarball
```

Global `--workspace`, `--context`, `--verbose`, `--quiet`, `--dry-run` and the exit codes (0 success or warnings, 1 gate/build/test failure, 2 command or configuration error) are inherited unchanged. `--dry-run` prints the resolved shas, the gate results and the command list, and creates nothing — consistent with `zpr-dev`'s existing rule that a `git fetch` already counts as a mutation. Final output is a one-screen summary: resolved shas, gate results, per-repository build status, per-tier status, `dist/` path.

---

## Dependency graph and order

```
B1 (schema, resolution, --dry-run)
 |
 +--> B2 (gates)          ----+
 |                            |
 +--> B3 (worktrees, builds,  |
          dist, emitted       |
          manifest)           |
            |                 |
            v                 |
          B4 (unit tier) <----+
            |
            v
          B5 (netns + docker tiers)
            |
            v
          B6 (docs + first committed build set)
```

B2 and B3 are independent of each other and can be worked in parallel once B1 lands; B2 alone already answers "is this set even coherent?" without building anything, so it is the more valuable of the two to have first.

---

## Issue map

| ID | Repo | Title | Blocked by |
|---|---|---|---|
| B1 · [#58](https://github.com/mkolehmainen/zipline/issues/58) | zl-zpr-dev-context | `spec-003-build.md`; build-set and emitted-manifest schemas; ref resolution; `zpr-dev build --dry-run` | — |
| B2 · [#59](https://github.com/mkolehmainen/zipline/issues/59) | zl-zpr-dev-context | The three compatibility gates, with unit tests | B1 |
| B3 · [#60](https://github.com/mkolehmainen/zipline/issues/60) | zl-zpr-dev-context | Worktrees, per-repo recipes, `dist/`, emitted manifest, tarball | B1 |
| B4 · [#61](https://github.com/mkolehmainen/zipline/issues/61) | zl-zpr-dev-context | `unit` tier, including `pregen` with the set's `zplc` | B3 |
| B5 · [#62](https://github.com/mkolehmainen/zipline/issues/62) | zl-zpr-dev-context | `netns` and `docker` tiers | B4 |
| B6 · [#63](https://github.com/mkolehmainen/zipline/issues/63) | zl-zpr-dev-context | `docs/BUILD.md` section, `zpr-dev/README.md`, retarget the `AGENTS.md` reading row, first committed build set | B5 |

---

## Phase B — `zpr-dev build`

### Task B1: Specification, schemas, ref resolution, `--dry-run` ([zipline#58](https://github.com/mkolehmainen/zipline/issues/58), merged)

**Files (new):**
- `zpr-dev/docs/specs/spec-003-build.md` — written first and in the style of `spec-001`: scope and out-of-scope, the two schemas, the three gates as normative rules, build order, tiers, CLI, exit codes, testing. This document is the auditable description of the gates; the code follows it.
- `zpr-dev/src/build/mod.rs` — `BuildSet` (`Deserialize`), ref resolution, `--dry-run` reporting. Split to `gates.rs`, `recipes.rs`, `tiers.rs` as B2-B5 land; keep any one file under ~600 lines.

**Files (changed):**
- `zpr-dev/src/main.rs` — `Command::Build { manifest, tip, test, repo, build_dir, keep, allow_pin_drift, no_tarball }` and its dispatch arm.
- `zpr-dev/src/config.rs` — no schema change; expose a lookup from repository name to `&Repo` so the build set never restates a URL or a default branch.
- `zpr-dev/src/git.rs` — add `rev_parse(dir, rev) -> Result<String>` (full sha, error naming the rev when unknown) and `tag_list(dir) -> Result<Vec<String>>`. Keep the existing `git()` wrapper; no new process plumbing.
- `zpr-dev/Cargo.toml` — add `toml` and `sha2`.
- `zpr-dev/docs/specs/spec-001-zpr-dev.md` — §1.2 note that building is in scope for the tool via spec-003; §6.1 record the two new dependencies and why.

**Produces (exact):** contracts 1, 2 and 6 above.

- [x] Step 1: Write `spec-003-build.md`. Review it before writing code — the gates are the security-relevant part and their wording is the artifact.
- [x] Step 2 (test first): unit tests for the build-set schema — a valid file; an unknown top-level key tolerated; `version: 2` rejected; a repository name absent from `workspace.yaml` rejected naming it; an `allow_pin_drift` entry without a `reason` rejected; `--manifest` and `--tip` together rejected as a usage error (exit 2).
- [x] Step 3: Implement `BuildSet` parsing and default-manifest selection (newest file in `build-sets/`, with a clear error when the directory is absent or empty).
- [x] Step 4 (test first, then implement): ref resolution. A tag, a branch, a short sha and a full sha each resolve to a 40-character sha; an unknown ref errors naming repository and ref; `--tip` resolves `origin/<default_branch>`; a missing or non-Git repository directory errors. Tests use the existing `Fixture` in `zpr-dev/tests/common/mod.rs`, which already creates throwaway origins and clones.
- [x] Step 5: `--dry-run` output: resolved shas, the planned build order, the planned tier list with prerequisite probe results, and the target `dist/` path. Nothing on disk changes; no `git fetch`.
- [x] Step 6: `cargo test && make check`.

**Acceptance:** `zpr-dev build --tip --dry-run` on this workspace prints a resolved sha for each of the five repositories and creates nothing; `zpr-dev status` afterwards is byte-identical to before; every Step 2 and Step 4 case is covered by a test.

---

### Task B2: The three compatibility gates ([zipline#59](https://github.com/mkolehmainen/zipline/issues/59), merged)

**Files:**
- `zpr-dev/src/build/gates.rs` (new) — pin extraction, the three gates, the accumulating report.
- `zpr-dev/src/build/mod.rs` — call the gates after worktrees exist (B3) or, until then, directly against the live checkouts behind `--gates-only`.

**Produces (exact):** contract 3 above, including the three message formats.

- [x] Step 1 (test first): pin extraction from `Cargo.toml` text — `git` + `tag`; `git` + `rev`; `git` + `branch`; a `[workspace.dependencies]` table; a member declaring `{ workspace = true }` resolved against the workspace root and attributed to it (this is how `zl-zpr-core/adapter/ph` gets `zpr`, and missing it would make the gate blind to a whole member); a plain registry dependency ignored; a `path` dependency ignored; a non-ZPR git URL (`emilazy/capnproto-rust`) captured because it is `rev`-pinned; a URL differing only by host/owner treated as a *different* source. Fixtures are the real snippets from `zl-zpr-core/Cargo.toml`, `adapter/ph/Cargo.toml`, `zl-zpr-visaservice/Cargo.toml`, `zl-zpr-compiler/Cargo.toml` and `zl-zpr-common/Cargo.toml`.
- [x] Step 2 (test first): gate 1. An agreeing set passes; a disagreeing set fails listing each tag with every file and line that pins it; a crate at one tag from two URLs fails; an `allow_pin_drift` entry suppresses exactly that crate and echoes its reason; `--allow-pin-drift` turns every finding into a warning and the command still exits 0.
- [x] Step 3 (test first): gate 2. Pinned `v0.26.0` with `v0.27.0` present in the checkout warns and exits 0; pinned at the newest tag is silent; no local checkout emits `INFO` and does not fail. Tag ordering is by semantic version, not creation date, and the test includes `v0.9.1` against `v0.15.0` to prove it.
- [x] Step 4 (test first): gate 3. `0.18.0` against `(0,18,0)` passes; `0.18.4` against `(0,18,0)` passes; `0.18.0` against `(0,18,4)` fails; `0.19.0` and `1.18.0` fail; a `config.rs` missing a constant, and a `Cargo.toml` missing `[package].version`, each fail naming the file. The oracle is `libeval/src/pio.rs`'s `check_version`; the test comment cites it so the two can be compared by eye in review.
- [x] Step 5: Implement. Findings accumulate into one report printed in `validate` style (`[OK]` / `[WARN]` / `[ERROR]` lines) and exit 1 if any error survives.
- [x] Step 6: `cargo test && make check`.

**Acceptance:** run against this workspace, the gates report the two real findings — the `rcu` disagreement (error) and `zpr` pinned at `v0.26.0` behind `v0.27.0` (warning) — and nothing else; adding the `rcu` entry to `allow_pin_drift` leaves only the warning and exit 0. Forcing a mismatch by hand (compiler `version = "0.19.0"`) produces the gate 3 message with both file paths.

> **Correcting note (2026-09-21, from B6).** This acceptance was met when B2 landed and is
> kept as the record of it, but it no longer describes the workspace. Both predicted findings
> have since been *fixed* rather than tolerated, and the second changed shape first: by the
> time B6 came to run the gates, `zl-zpr-visaservice` and `zl-zpr-core` had moved to `v0.27.0`
> while `zl-zpr-compiler` had not, so `zpr` presented as a gate 1 **pin disagreement (error)**
> and never as the gate 2 freshness **warning** predicted here. `zpr-dev build --tip
> --gates-only` against the workspace today reports no findings at all. See *Findings*.

---

### Task B3: Worktrees, recipes, `dist/`, emitted manifest ([zipline#60](https://github.com/mkolehmainen/zipline/issues/60), merged)

**Files:**
- `zpr-dev/src/build/recipes.rs` (new) — the five recipes of contract 4 and the staging lists.
- `zpr-dev/src/build/mod.rs` — worktree lifecycle, `dist/`, emitted manifest, tarball.
- `zpr-dev/src/git.rs` — add `worktree_add(repo, dest, sha)` (`git worktree add --detach`) and `worktree_remove(repo, dest)` (`git worktree remove --force`, then `git worktree prune`).

**Produces (exact):** contracts 2 and 4 above.

- [x] Step 1 (test first): worktree lifecycle on a `Fixture` repository — a worktree appears at the requested sha; the source checkout's branch, `HEAD` and dirty files are unchanged afterwards; removal leaves no entry in `git worktree list`; an existing build directory from a previous run is reused or refused with a clear message rather than half-overwritten.
- [x] Step 2: Implement the recipes, each shelling out with the worktree as the working directory, output tee'd to `logs/<repo>-<step>.log`, and the last 40 lines echoed on a non-zero exit. `zl-zpr-visaservice` stages from its own `build-release/`.
- [x] Step 3: Stage `dist/`, then verify the expected ten names exist and are executable — a recipe that silently produces nothing must not pass.
- [x] Step 4 (test first): the emitted manifest. Emit, re-read as a build set, and assert the resolution is identical (round-trip); assert the `resolved:` block is ignored on re-read; assert it is written even when a later step fails.
- [x] Step 5: `sha256` per binary, host toolchain versions (`rustc --version`, `cargo --version`, `go version`, `capnp --version`), and the tarball `zpr-set-<name>-linux-<arch>.tar.gz` unless `--no-tarball`.
- [x] Step 6: Prune worktrees on success unless `--keep`; leave them on failure and say so, because they are what a person needs in order to debug. `dist/` always survives.
- [x] Step 7: `cargo test && make check`.

**Acceptance:** `zpr-dev build --tip --test none` produces a `dist/` with all ten binaries and an emitted manifest; `zpr-dev build --manifest dist/zpr-set-<name>.yaml --test none --dry-run` reports the identical five shas; `git -C <repo> status` in every source checkout is unchanged from before the run.

---

### Task B4: The `unit` tier ([zipline#61](https://github.com/mkolehmainen/zipline/issues/61), merged)

**Files:** `zpr-dev/src/build/tiers.rs` (new), `zpr-dev/src/build/mod.rs`.

**Produces (exact):** the `unit` row of contract 5 and the `tiers:` block of contract 2.

- [x] Step 1: Run `make test` per built repository, in build order, logging as in B3. For `zl-zpr-visaservice`, run `make pregen ZPLC=<dist>/zplc` first and fail the tier if `pregen` fails — a `zplc` that cannot compile the visa service's own fixtures is exactly the incompatibility this plan exists to catch.
- [x] Step 2: Record per-repository pass/fail into the emitted manifest's `tiers.unit`, and keep going after a failure so one run reports every broken repository rather than the first.
- [x] Step 3 (test first): tier selection and reporting logic — `--test none`, `default`, `unit`, a comma-separated list, an unknown name (exit 2) — exercised without executing any command.
- [x] Step 4: `cargo test && make check`.

**Acceptance:** `zpr-dev build --tip` on this workspace runs `pregen` with the set's `zplc`, then every repository's `make test`, and reports each; the emitted manifest's `tiers.unit.status` matches what was printed. A deliberately broken fixture (`zplc` from a mismatched compiler) fails the tier with `pregen` named as the failing step.

---

### Task B5: The `netns` and `docker` tiers ([zipline#62](https://github.com/mkolehmainen/zipline/issues/62), merged)

**Files:** `zpr-dev/src/build/tiers.rs`.

**Produces (exact):** the `netns` and `docker` rows of contract 5.

- [x] Step 1: Prerequisite probes — Linux, `sudo -n true`, `valkey-server` on `PATH` or `VALKEY_SERVER_BIN`, `python3`, `docker`, `docker compose version`. Each probe's failure text is the `skipped` reason, and is what `--test all` turns into an error.
- [x] Step 2: `netns` — run the seven scripts of contract 5 in order with `PH_BIN`, `PH_DEBUG_BIN`, `VS_BIN`, `VS_ADMIN_BIN`, `VALKEY_SERVER_BIN` exported at `dist/` and the system valkey. Continue after a failing script; record each. No copying into `integration-test/`.
- [x] Step 3: `a2a-pubkey-test.sh` gets its own `cargo build -p ph --features enable-security-testing` in the `zl-zpr-core` worktree, and `PH_BIN` pointed at that binary for that script only. Add a check that this binary never lands in `dist/`.
- [x] Step 4: `docker` — copy `dist/`'s ten binaries into the `zl-zpr-demo` worktree's `dns-demo/bin/` (skipping that repository's `make` entirely), run `local-compute/deploy-docker.sh`, then `local-compute/test-dns.sh`, then `docker compose down -v` unconditionally, reporting a teardown failure separately from a test failure.
- [x] Step 5: Record both tiers in the emitted manifest, `skipped` reasons included.
- [x] Step 6: `cargo test && make check`.

**Acceptance:** `zpr-dev build --tip --test all` on this host runs all seven netns scripts and the DNS test against `dist/` and reports each; on a host without Docker, `--test docker` exits 1 saying why, while a plain `zpr-dev build --tip` reports `docker: skipped, docker not found` and exits 0.

---

### Task B6: Documentation and the first committed build set ([zipline#63](https://github.com/mkolehmainen/zipline/issues/63), done)

**Files:**
- `docs/BUILD.md` — new "Compatible build sets" section: what a set is, the two manifests, the three gates, the tiers, how to cut a new set. While in this file, fix what has gone stale: the `zpr` example tag (`v0.25.1` → `v0.26.0`) and the `zpr-utils` URLs, which now point at `mkolehmainen` in `zl-zpr-core` and `zl-zpr-common`, not `org-zpr`.
- `AGENTS.md` — retarget the build-set required-reading row this plan added: point it at `docs/BUILD.md` and `zpr-dev/docs/specs/spec-003-build.md` rather than at this plan, which is spent once B6 lands.
- `zpr-dev/README.md` — the `build` section, the `build-sets/` layout, and the "It is not a build system and does not replace Git" line, whose first clause stops being true.
- `build-sets/2026-09-17.yaml` (new) — generated by a `--tip` run, reviewed, committed. The build directory needs no `.gitignore` entry: `<workspace>/.zpr-build/` sits beside the checkouts, inside none of them.

- [x] Step 1: Write the `docs/BUILD.md` section and fix the stale facts.
- [x] Step 2: `zpr-dev sync`, then confirm `zpr-dev validate` is clean (a documentation reference that does not resolve is an error there).
- [x] Step 3: Run `zpr-dev build --tip --test all`, review the emitted manifest, commit it as `build-sets/2026-09-17.yaml`.
- [x] Step 4: Rebuild from the committed set and diff the `resolved.pins` and `resolved.versions` blocks against the original run.

**Acceptance:** a person who has read only `docs/BUILD.md` can cut a set and reproduce it; `zpr-dev validate` is clean; the committed set rebuilds to the same shas and the same pin set.

---

## Findings

### Finding 1 — the `rcu` pin already disagrees

`zl-zpr-common/Cargo.toml:21` pins `rcu` at tag `zpr-utils-v0.1.0`; `zl-zpr-core/adapter/ph/Cargo.toml:36` pins `rcu-v0.1.2`. Gate 1 will flag this on the first run. It is the same class of problem `docs/BUILD.md` describes for `cslab` (two crates from one commit via different sources), and `mkolehmainen/zipline#18` covers the repointing. Until then it belongs in `allow_pin_drift` with that issue number as its reason — which is the feature working as intended: a known divergence recorded in a reviewed file rather than discovered at runtime.

**Resolved 2026-09-21 — fixed, not suppressed.** `zipline#18` closed without this part. Its
stated acceptance was "no `org-zpr` URLs", which it met; its plan comment then scoped
`zl-zpr-common` out ("already on mkolehmainen; the issue does not name that repo") — true of
the URL and silent about the tag. The root cause was older: `zl-zpr-common` `e4bd655`
(2026-03-25) rewrote a repo-wide `v0.1.0` tag to `zpr-utils-v0.1.0` for an **`rcu`**
dependency, where `rcu-v0.1.0` names the identical commit (`17e29a1`). That detached
`zl-zpr-common` from `rcu`'s tag line, so it never followed `rcu-v0.1.1` or `rcu-v0.1.2` and
no audit searching for "rcu" found it.

The fix was a re-pin, not an upgrade — `git diff zpr-utils-v0.1.0 rcu-v0.1.2 -- rcu/src/` is
empty. `zl-zpr-common` `3abdacf` (tag `v0.28.0`) pins `rcu-v0.1.2`, and
`zl-zpr-core/Cargo.lock` now holds exactly one `rcu` and one `cslab` — which was also
`zipline#18`'s own second acceptance clause, previously unmet. **No `allow_pin_drift` entry
was needed, and the first committed set should not carry one:** its reason would have cited a
closed issue, leaving a permanent exception that no reader could justify.

### Finding 2 — consumers are one tag behind `zl-zpr-common`

Consumers pin `zpr` `v0.26.0`; `zl-zpr-common` is tagged `v0.27.0` at `5fbbff8` (`ReauthRequest vsapi_types wrapper`). Gate 2 warns and does not fail. Whether the first committed set should bump to `v0.27.0` first is a decision for B6, not for this plan.

**Resolved 2026-09-21 — B6 answered "bump first".** The split widened before it closed:
`zl-zpr-visaservice` and `zl-zpr-core` moved to `v0.27.0` while `zl-zpr-compiler` stayed at
`v0.26.0`, so this stopped being a gate 2 warning and became a gate 1 error. All three now
pin `v0.28.0` — the same `zl-zpr-common` tag that carries Finding 1's `rcu` re-pin, so one
tag closed both. `zplc` is `0.18.1` against `POLICY_MIN_COMPILER 0.18.0`, which gate 3
accepts.

### Finding 3 — `docs/BUILD.md` is stale in two ways the gates would have caught

Its dependency example says `zpr` `v0.25.1` (now `v0.26.0`) and shows `zpr-ext`/`cbpf-rs` from `org-zpr` (now `mkolehmainen` in the manifests actually checked out). Both are fixed in B6. This is the strongest argument for the emitted manifest: the authoritative record of what a set pinned should be generated, not prose.

### Finding 4 — gate 1 cannot see a transitive pin, and one is wrong today (added 2026-09-21)

Found while confirming Findings 1 and 2. `zl-zpr-core/Cargo.lock` carries **two** `zpr`
crates: `v0.28.0` directly, and `v0.8.1` transitively, because `zpr-utils-v0.2.2`'s own
`zpr-utils/Cargo.toml:11` pins it there. Gate 1 does not report this and structurally cannot:
`zpr-dev/src/build/mod.rs:638-677` reads the root `Cargo.toml` and literal workspace members
of the **five repositories in the set**, and `zpr-utils` is a git dependency with no
worktree. `allow_pin_drift` is not an option either — there is no finding to suppress.

So `pin agreement: 10 crates pinned consistently` is true of what the gate inspects and false
of what ships. It compiles today only because the one type crossing the boundary,
`VsapiIpProtocol`, is `pub type … = u8` in both versions; a newtype or `open_enum` there turns
`zl-zpr-core/adapter/ph/src/defs.rs:88` into an `E0308`. Filed as
[zipline#69](https://github.com/mkolehmainen/zipline/issues/69), which also weighs whether to
widen gate 1. Not a blocker for B6 — a set can be cut today — but B6 should record the
decision rather than let the first committed set imply coverage the gate does not have.

---

## Out of scope (recorded, not scheduled)

- **No `[patch]` or path override of `zl-zpr-common`.** An untagged `zpr-common` therefore cannot be integration-tested by this tool: cut a tag. `docs/BUILD.md` records why a bare `[patch]` is silently ignored against a tag pin and why the `cargo update` that forces it must never reach a PR. If tip-of-main work on `zpr-common` genuinely stalls on this, a `--link-common` flag that rewrites only a throwaway worktree is the shape to add — no earlier.
- **No container build.** `dist/` is built on the host, so the `docker` tier can fail if the host's glibc is newer than `ubuntu:24.04`'s. If that bites, add `--in-container` using `zl-zpr-dev-tools/docker/dev-env`, which `docs/BUILD.md` already documents as the CI-identical environment.
- **No CI wiring.** Actions is disabled on all the forks by decision (`docs/BUILD.md`), so this *is* the gate, run by hand.
- **No parallel repository builds and no `target/` reuse across sets.** Each set compiles cold, roughly fifteen minutes. A shared per-repository `CARGO_TARGET_DIR` is the first thing to try if that becomes the bottleneck.
- **No signing, publishing or release-notes generation.** The tarball is a local artifact.
- **No per-repository tagging of a set.** See Open questions.
- **No `zpr-dev build` inside the demo repositories' own release flows.** `containerized-demo` keeps its `README-DEV.md` procedure; converting it is separate work.

---

## Resolved while planning

- **The build set does not pin `zl-zpr-common`.** The first draft pinned it and compared that ref to the consumers' tags. That is backwards: nothing in the set compiles the checkout, so the meaningful invariant is that the consumers *agree*, at whatever tag. Gate 1 asserts agreement, gate 2 warns when the agreed tag is behind, and the agreed tag is recorded in the emitted manifest. Generalizing the same rule over every shared Git dependency then came free, including the `zpr-utils` crates and the Cap'n Proto fork.
- **`vs-int` is not a separate tier.** `zl-zpr-visaservice`'s own `make test` already runs `make -C integration-test`. A separate tier would have had to reimplement that sequencing and would drift from it. The tier is `unit`, preceded by `make pregen ZPLC=<dist>/zplc` so those tests exercise the set's own compiler.
- **Release binaries for `dist/`, debug for `make test`.** `dns-demo` and `zl-zpr-visaservice`'s `make release` both use release; `make test` and the netns scripts default to debug. Rather than force one profile, `dist/` is release and the netns scripts are pointed at `dist/` with `PH_BIN`/`VS_BIN`. The cost is compiling twice, which is stated in contract 4 rather than hidden.
- **Gate 3 is static *and* dynamic.** Parsing `POLICY_MIN_COMPILER_*` out of `vs/src/config.rs` gives a readable message before anything compiles; `pregen` with the set's `zplc` proves the same thing for real. Both are kept: the static one fails fast and explains, the dynamic one cannot be fooled by a parser bug.

---

## Open questions

1. **Where do build sets live?** Proposed: `zl-zpr-dev-context/build-sets/`, beside `workspace.yaml`, since `zpr-dev` already reads that repository and a set is meaningless without it. The alternative is `zl-zpr-dev-tools`, beside the reusable CI workflows.
2. **Does a shipped set also get a git tag in each repository?** Proposed: no — the committed manifest is the identifier, and a sha is a stronger reference than a tag that can move. Revisit if a set ever needs to be reconstructed from a repository rather than from this one.
3. **Should the first committed set bump `zpr` to `v0.27.0` across all consumers first?** That is a real change in three repositories with its own testing; B6 can either do it or commit the `v0.26.0` set and let the bump be its own issue.
4. **Should `zpr-dev build` fetch?** Currently it resolves refs against whatever the local checkouts already have, so a tag pushed but not fetched fails with "unknown ref". Adding an opt-in `--fetch` is cheap; making it the default would break `zpr-dev`'s rule that a fetch is a mutation and `--dry-run` must not perform one.
