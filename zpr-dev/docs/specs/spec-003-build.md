# SPEC-003: `zpr-dev build` — compatible build sets

Status: draft — B1 (schemas, ref resolution, `--dry-run`) implemented; gates,
builds and tiers specified here land in B2–B5
Date: 2026-09-17
Parent spec: `spec-001-zpr-dev.md`
Master plan: `docs/plans/2026-09-17-build-sets.md` (umbrella
`mkolehmainen/zipline#57`)

`zpr-dev build` builds the whole ZPR binary set — `vs`, `vs-admin`, `vsapikey`,
`zpt`, `zpr-dashboard`, `zplc`, `zpdump`, `ph`, `ph-cli`, `coredns` — from a
manifest that names a tag, branch or commit per repository, or from the tip of
each default branch. Before compiling anything it proves the set is coherent
(§4); after compiling it runs the test tiers (§6) against those exact binaries
and emits a manifest that rebuilds the same set later (§3).

This document is the auditable description of the feature. The gates in §4 are
the security-relevant part: their wording is what a reviewer checks the code
against.

---

## 1. Scope

### 1.1 In scope (this spec)

- The input **build-set** schema (§2) and the **emitted-manifest** schema (§3).
- Ref resolution: every manifest value resolved to a 40-character commit sha
  against the local checkouts, with no fetch (§2.3).
- The three compatibility gates as normative rules (§4).
- Build order, recipes and `dist/` staging (§5).
- Test tiers and their environment contract (§6).
- The CLI surface and exit codes (§7).

### 1.2 Implementation status

The spec is written whole so the gates and schemas can be reviewed together,
but it lands in stages (master plan, "Issue map"):

| Stage | Implements | Status |
|---|---|---|
| B1 | §2, §3 (schema + round-trip), §2.3 resolution, `--dry-run` (§7.2) | implemented |
| B2 | §4 gates | specified only |
| B3 | §5 worktrees, builds, `dist/`, emitted manifest, tarball | specified only |
| B4 | §6 `unit` tier | specified only |
| B5 | §6 `netns` and `docker` tiers | specified only |

Until B3 lands, the `resolved:` block of §3 is a fixed shape with empty or
defaulted fields; B3–B5 populate it.

### 1.3 Out of scope (recorded in the master plan, not scheduled)

No `[patch]` or path override of `zl-zpr-common`; no container build; no CI
wiring; no parallel repository builds or `target/` reuse; no signing or
publishing; no per-repository tagging of a set.

---

## 2. Input manifest: the build set

A build set lives at `zl-zpr-dev-context/build-sets/<name>.yaml`.

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

### 2.1 Rules

- Only **binary-producing** repositories appear. `zl-zpr-common`,
  `zl-zpr-policy` and `zl-zpr-vsapi` are deliberately absent: cargo consumes
  them from Git by tag, so pinning them here would describe something that
  does not happen. The set's `zl-zpr-common` version is *derived* from what
  the consumers agree on (gate 1, §4.1).
- A key under `repositories:` that is not a repository in `workspace.yaml` is
  an error naming the key. `url` and `default_branch` come from
  `workspace.yaml`; nothing is duplicated here.
- `version: 1` is the only accepted version. `name` must be non-empty (it
  names the build directory) and `repositories` must be non-empty.
- Unknown keys are ignored, matching `workspace.yaml`'s tolerance
  (spec-001 §3.1: no `deny_unknown_fields` anywhere), so the schema can grow
  ahead of the tool. This is also what makes the emitted manifest (§3) valid
  input: its `resolved:` block is an unknown key by design.
- An `allow_pin_drift` entry without a non-empty `reason` is an error naming
  the crate. The reason is the reviewed record of a known divergence.

### 2.2 Default manifest selection

`--manifest` defaults to the newest file in `<context>/build-sets/`, by file
name in descending lexicographic order — set names are dates, so the newest
name is the newest set, and a rename cannot silently change which set builds.
Only regular files ending `.yaml` or `.yml` count. An absent or empty
`build-sets/` directory is an error telling the user to pass `--manifest` or
`--tip`.

### 2.3 Ref resolution

Every `repositories:` value is resolved to a full 40-character commit sha with
`git rev-parse --verify <ref>^{commit}` in the named repository's workspace
checkout:

- A tag, a branch, a short sha and a full sha all resolve; an annotated tag
  resolves to the commit it points at (`^{commit}`).
- Under `--tip` every manifest value is ignored and
  `origin/<default_branch>` from `workspace.yaml` is resolved instead —
  `zipline` for the forks, `main` for `zl-zpr-coredns`.
- **No fetch, ever.** Resolution runs against whatever the local checkout
  already has; `zpr-dev` treats a fetch as a mutation (spec-001 §5.1), and
  `--dry-run` must not perform one. A tag pushed but never fetched therefore
  fails as an unknown ref; an opt-in `--fetch` is open question 4 on the
  umbrella, not part of this spec.
- An unknown ref is an error naming **both** the repository and the ref. A
  repository directory that is missing, or is not a Git repository, is an
  error naming the repository.

---

## 3. Emitted manifest: the reproducibility contract

Written to `dist/zpr-set-<name>.yaml`. The **same schema as the input**, with
every ref replaced by its resolved 40-character sha, plus a diagnostic
`resolved:` block that a later read ignores:

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
  versions:
    zplc: "0.18.0"
    vs_policy_min_compiler: "0.18.0"
  binaries:
    - { name: vs, sha256: "...", bytes: 18234512, from: zl-zpr-visaservice }
  tiers:
    unit:   { status: passed }
    netns:  { status: skipped, reason: "no passwordless sudo" }
```

Normative behavior:

- `zpr-dev build --manifest dist/zpr-set-<name>.yaml` must resolve to exactly
  the shas listed and recompute the same `pins` block. Nothing outside the
  file, and the repositories it names, is needed. Because a sha resolves to
  itself, re-reading an emitted manifest as input is the identity: this is the
  round-trip property, and it is tested (§8).
- The manifest is emitted **whenever the gates pass**, even if a build step or
  a test tier fails, with the failure recorded in `tiers:`. The most
  interesting set to reproduce is the one that broke.
- `sha256` per binary is for *comparison*, not a reproducibility claim.
  Reproducible **inputs** are guaranteed — same sources, same pins; byte
  identical binaries are not, because cargo output varies with toolchain and
  path. The `host:` block records the toolchain that produced them.
- Promoting a `--tip` run to a named set is: run it, review, copy
  `dist/zpr-set-<name>.yaml` into `build-sets/`, commit. No separate
  authoring step.

---

## 4. The three gates (normative)

Gates run **before compilation**, against the worktrees (or, until B3, the
live checkouts). A set whose pins disagree must fail in seconds, not after
fifteen minutes of cargo. Findings accumulate and print together in the style
of `zpr-dev validate` (`[OK]` / `[WARN]` / `[ERROR]` lines); any surviving
error exits 1.

### 4.1 Gate 1 — shared-dep agreement (error)

Parse every `Cargo.toml` in every worktree, including workspace members, and
collect each dependency declared with `git = <url>` where the URL is
ZPR-family (`github.com/mkolehmainen/*`, `github.com/org-zpr/*`) or otherwise
pinned by `rev` (the `emilazy/capnproto-rust` fork). Group by `(crate, url)`
and require a single `tag` (or `rev`) across the whole set. Report every
disagreement as:

```text
ERROR pin disagreement: crate `zpr` (https://github.com/mkolehmainen/zl-zpr-common.git)
  v0.26.0  zl-zpr-core/Cargo.toml:29 (+ adapter/ph, inherited)
  v0.27.0  zl-zpr-visaservice/Cargo.toml:29
  => cut one tag and bump every consumer, or record it in allow_pin_drift
```

The same crate at the same tag from **different URLs** is also a
disagreement: that is what gives cargo two `cslab` crates from one commit and
silently breaks `RcuBox<RcuCslabReader<T>>` (`docs/BUILD.md`). An
`allow_pin_drift` entry suppresses one crate's finding and its reason is
echoed in the output and the emitted manifest; `--allow-pin-drift` downgrades
all findings to warnings for a one-off.

### 4.2 Gate 2 — freshness (warning, never an error)

For each agreed pin whose repository is in the workspace, list tags in that
checkout and warn when a newer one exists, ordering tags by semantic version,
not creation date:

```text
WARN  crate `zpr` is pinned at v0.26.0; zl-zpr-common has v0.27.0
```

A set may legitimately sit behind; the warning makes that a decision, not an
accident. A missing local checkout is an `INFO`, not a failure: cargo fetches
the tag from GitHub either way.

### 4.3 Gate 3 — compiler / visa service version (error)

Read `POLICY_MIN_COMPILER_MAJOR`, `_MINOR`, `_PATCH` from the visa-service
worktree's `vs/src/config.rs` and `version` from `[package]` in the compiler
worktree's `Cargo.toml`. Apply `libeval::pio::check_version`'s rule exactly —
**major equal, minor equal, patch greater or equal** — and on failure report:

```text
ERROR zplc 0.19.0 cannot produce policy for this vs
  zl-zpr-compiler/Cargo.toml:3                version = "0.19.0"
  zl-zpr-visaservice/vs/src/config.rs:29-31   POLICY_MIN_COMPILER = 0.18.0
  rule: major ==, minor ==, patch >=  (libeval/src/pio.rs check_version)
```

A value that cannot be parsed from either file is an error naming the file:
silently skipping this check would be worse than failing, because nothing else
checks it. The gate is duplicated dynamically by the `unit` tier's `pregen`
step (§6), which recompiles the visa service's policy fixtures with the set's
own `zplc`.

---

## 5. Build order, recipes and `dist/`

Sources come from `git worktree add --detach` per repository at the resolved
sha — the live checkouts are never modified: no branch switch, no checkout, no
fetch, no build inside them. Dropping a dangling worktree registration is not
a modification in this sense: `worktree_add` runs `git worktree prune` on the
source repository first (zipline#71), so a build directory deleted by hand —
which strands a registration in every source repository — recovers on its own
with no flag and no manual `git worktree prune`. Prune is scoped by git's own
definition: it drops only registrations whose directory is already gone, so a
live worktree, including one a developer created themselves, is never at
risk. Everything is a release build. Ordered so each
step's output is available to the next:

| # | Repository | Command(s) | Staged into `dist/` |
|---|---|---|---|
| 1 | `zl-zpr-compiler` | `cargo build --release` | `zplc`, `zpdump` |
| 2 | `zl-zpr-visaservice` | `make release` | `vs`, `vs-admin`, `vsapikey`, `zpt`, `zpr-dashboard` |
| 3 | `zl-zpr-core` | `cargo build --release` | `ph`, `ph-cli` |
| 4 | `zl-zpr-coredns` | `make build` (Go) | `coredns` |
| 5 | `zl-zpr-demo` | none | none |

- Step 1 comes first because the visa service's `pregen` and the demo's
  policy compilation both need `zplc`.
- Step 2 reuses `zl-zpr-visaservice`'s own `make release` rather than
  reimplementing its staging; the double compile that implies (release for
  `dist/`, debug for `make test`) is accepted and recorded.
- Step 5 builds nothing: the `docker` tier copies `dist/` into
  `dns-demo/bin/`, so the DNS test exercises the set's own binaries.
- Each repository's own `Makefile` stays authoritative; recipes here are thin
  wrappers. No manifest of a built repository is ever rewritten: no
  `[patch]`, no path dependency, no `Cargo.lock` edit.
- Per-step output goes to `logs/<repo>-<step>.log`; the last 40 lines are
  echoed on failure. Build directory: `<workspace>/.zpr-build/<name>`
  (`--build-dir` overrides), `dist/` inside it.

## 6. Test tiers

| Tier | What runs | Prerequisites | Default |
|---|---|---|---|
| `unit` | `make test` in each built repository, with `make pregen ZPLC=<dist>/zplc` first in `zl-zpr-visaservice` | none beyond the build | run |
| `netns` | the seven `zl-zpr-core/integration-test/` scripts | Linux, passwordless `sudo` **or** `--prompt-for-sudo`, `valkey-server`, `python3` | `--test netns` |
| `docker` | `dns-demo` deploy, `test-dns.sh`, `docker compose down -v` | `docker`, `docker compose` | `--test docker` |

- A tier that was asked for and cannot run is an error under `--test all`; a
  tier that was not asked for is reported `skipped` with the probe's failure
  text as the reason. A skipped test is always visible in the output **and**
  in the emitted manifest — a green run must never overstate coverage.
- `netns` binaries come from `dist/` via `PH_BIN` / `VS_BIN` /
  `VS_ADMIN_BIN` / `VALKEY_SERVER_BIN`; nothing is copied into
  `integration-test/`. The script list is explicit, not globbed.
- `a2a-pubkey-test.sh` needs a `ph` built with `enable-security-testing`;
  that binary must never be staged into `dist/`.
- The `docker` tier always runs `docker compose down -v`, pass or fail.
- `--prompt-for-sudo` (zipline#70): opt-in. When the netns tier is selected
  and `sudo -n true` fails, prompt once, up front — `sudo -v` with inherited
  stdio, immediately after ref resolution and before any compilation — then
  re-probe `sudo -n true`, so a sudoers with `timestamp_timeout=0` degrades
  to the ordinary skip/error path with a note instead of dying mid-tier. A
  background thread runs `sudo -n -v` every 60s from the prime until the
  netns tier returns, so a run longer than sudo's timestamp timeout does not
  lose the credential mid-tier. The flag requires a terminal on stdin (exit 1
  otherwise, never a hang), never prompts under `--dry-run` or `--gates-only`,
  and never weakens the probe: without it, behaviour is exactly as before.
  The emitted manifest records the netns tier's sudo provenance as
  `sudo: nopasswd` or `sudo: primed` — the two are never conflated.

  Known ceiling: `tty_tickets` makes this a workstation-only convenience —
  it works because the netns children inherit our controlling terminal. It
  is not a path to running the netns tier in CI, and should not be
  documented as one. CI still needs a host with passwordless sudo, or a
  container.

## 7. CLI surface

```text
zpr-dev build [--manifest <path> | --tip] [--test <list>] [--repo <name>]
              [--build-dir <path>] [--keep] [--allow-pin-drift] [--no-tarball]
              [--prompt-for-sudo] [--clean]

--manifest <path>   build set to build; default: newest file in build-sets/ (§2.2)
--tip               ignore every ref; use origin/<default_branch> everywhere
--test <list>       none | default | unit | netns | docker | all (comma-separated)
--repo <name>       build only this repository and its prerequisites
--build-dir <path>  default <workspace>/.zpr-build/<name>
--keep              keep worktrees after a successful run
--allow-pin-drift   gate 1 disagreements warn instead of failing
--no-tarball        skip the dist tarball
--prompt-for-sudo   prompt once for the sudo password before the run (§6)
--clean             remove the build directory and clear its worktree
                    registrations, then exit (zipline#71)
```

`--clean` is a mode, not a modifier: it conflicts with `--manifest`, `--tip`,
`--test`, `--repo`, `--keep`, `--gates-only`, `--allow-pin-drift`,
`--no-tarball` and `--prompt-for-sudo` (usage error, exit 2). `--build-dir`
stays allowed — it scopes the clean to that directory; without it the whole
`<workspace>/.zpr-build` tree goes. Cleaning is not per-set: there is no
manifest resolution and nothing to name a set with. After removing the
directory, `git worktree prune` runs in every workspace-manifest repository,
which is what catches registrations whose directories a person already
deleted. `--clean` resolves no refs, fetches nothing, and runs no gates — it
must work on a workspace too broken to resolve a build set. It never runs
`worktree remove --force` on a path the tool did not create; the registration
side is cleared only by prune, whose blast radius is limited to
already-missing directories by git's own definition. Cleaning an already-clean
workspace is a success (exit 0), not an error, and `--keep`'s contract is
untouched: deliberately retained worktrees remain until something —
`--force`, `--clean`, or a person — removes them.

### 7.1 Global options and exit codes

Global `--workspace`, `--context`, `--verbose`, `--quiet`, `--dry-run` and the
exit codes are inherited from spec-001 §5.1 and §6.4 unchanged: `0` success or
warnings, `1` gate/build/test failure, `2` command or configuration error.
`--manifest` together with `--tip` is a usage error (exit 2), enforced by the
argument parser. Flags whose stages have not landed (`--test`, `--repo`,
`--build-dir`, `--keep`, `--allow-pin-drift`, `--no-tarball` before B2–B5)
parse but are inert; `--dry-run` reports the stage as not yet implemented
rather than pretending.

### 7.2 `--dry-run`

Prints the resolved shas, the planned build order, the planned tier list with
prerequisite probe results, and the target `dist/` path — and creates
nothing. No worktree, no directory, no log file, and **no `git fetch`**:
`zpr-dev`'s existing rule that a fetch is already a mutation applies
(spec-001 §5.1). Probes that are themselves read-only (`sudo -n true`,
`docker` on `PATH`) may run; anything else is reported as "would probe".
Final output is a one-screen summary in the existing `zpr-dev` style.

`--dry-run --clean` follows the same contract: it prints the clean report —
what would be removed and which repositories would be pruned — and removes
nothing; the build directory still exists afterwards (zipline#71).

## 8. Testing

- **Schema unit tests** (`src/build/mod.rs`): a valid manifest parses; an
  unknown top-level key is tolerated; `version: 2` is rejected; a repository
  name absent from `workspace.yaml` is rejected naming it; an
  `allow_pin_drift` entry without a `reason` is rejected; default-manifest
  selection picks the newest name and errors clearly on an absent or empty
  `build-sets/`.
- **Ref-resolution tests** against the `Fixture` of spec-001 §8.1, extended
  with tags and branches: a tag, a branch, a short sha and a full sha resolve
  to the same 40-character sha; an unknown ref errors naming repository and
  ref; `--tip` resolves `origin/<default_branch>` and ignores manifest
  values; a missing or non-Git repository directory errors.
- **`--dry-run` integration tests**: `build --tip --dry-run` prints a
  resolved sha per repository and creates nothing — the workspace tree is
  byte-identical before and after; an unresolvable ref exits 1 naming
  repository and ref; `--manifest` with `--tip` exits 2.
- **Round-trip test**: serialize a resolved set as the emitted manifest,
  re-read it as an input build set, and assert the resolution is identical
  and the `resolved:` block is ignored.
- Gate tests (B2) and worktree/recipe/tier tests (B3–B5) are specified in the
  master plan's task sections and land with their stages.

## 9. Dependencies

B1 adds no crates. `toml` (reading pins out of `Cargo.toml`, gate 1) arrives
with B2, and `sha2` (binary digests) with B3 — each recorded in spec-001 §6.1
when it lands, so the manifest and the spec never disagree. No
`cargo-metadata`, no `git2`, no async runtime.
