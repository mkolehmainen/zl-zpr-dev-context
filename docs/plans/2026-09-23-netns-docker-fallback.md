# netns Docker fallback — `zpr-dev build` runs the netns tier in a container when host sudo is missing

**Status:** COMPLETE (2026-09-23) — umbrella [zipline#90](https://github.com/mkolehmainen/zipline/issues/90). N1 ([#91](https://github.com/mkolehmainen/zipline/issues/91)) withdrawn (Finding 1); N2 ([#92](https://github.com/mkolehmainen/zipline/issues/92)) and N3 ([#93](https://github.com/mkolehmainen/zipline/issues/93)) merged (zl-zpr-dev-context PRs #24/#25); N4 ([#94](https://github.com/mkolehmainen/zipline/issues/94)) is this documentation closure.
**Date:** 2026-09-23
**Repo state this plan was written against:** `zl-zpr-dev-context` @ `ed91470`, `zl-zpr-core` @ `e56899a` (`integration-test/Makefile` last changed by `c163628`, zipline#84 follow-up), both on `zipline`.

> Process note: per `skills/zpr/SKILL.md`, each task below becomes one GitHub issue in `mkolehmainen/zipline` and is worked on a feature branch off `zipline`, PR targeting `zipline`. Every remaining task (N2–N4) lands in `zl-zpr-dev-context`; `zl-zpr-core` needs no change (Finding 1).

**Goal.** `zpr-dev build --test all` (or `--test netns`) on a workstation without passwordless `sudo` runs the netns integration tier inside the privileged Docker container that zipline#84 added to `zl-zpr-core/integration-test/`, instead of failing at the gate. The host route stays first choice; the container is a fallback; the emitted manifest says which one ran. Nothing is silently weaker: a run that could use neither still skips or errors exactly as today.

---

## Background

zipline#84 gave `zl-zpr-core/integration-test/` a `Makefile` that runs the same `*-test.sh` scripts as root inside a throwaway `--privileged` container (`make docker-test TEST=<script>`), bind-mounting the workspace so host-built binaries and the sibling-repository symlinks resolve unchanged. Its scope was the Makefile, the Dockerfile and a `docs/BUILD.md` paragraph; `zpr-dev build` was not touched.

So today `zpr-dev`'s netns gate (`zpr-dev/src/build/tiers.rs`, `netns_gate`) knows one route: Linux, passwordless `sudo` (or a credential primed by `--prompt-for-sudo`, zipline#70), `valkey-server` and `python3` on the host. Under `--test all` every tier is explicit, and an explicit tier whose gate fails is an error (`check_gate`, spec-003 §6), which is the observed failure:

```
error: --test netns was requested but cannot run: missing: passwordless sudo (or pass --prompt-for-sudo)
```

The `docker` tier `zpr-dev` already has is unrelated: it deploys `zl-zpr-demo/dns-demo` and runs `test-dns.sh`.

## Findings

### Finding 1 — the Makefile's default mount does not include `dist/`, but the override already works

`integration-test/Makefile` computes `WORKSPACE := $(abspath $(CURDIR)/../..)` and mounts it at the same absolute path. From a live checkout that is the workspace root, which holds every sibling repository. From a `zpr-dev` build the scripts run in `<build-dir>/src/zl-zpr-core/integration-test`, so the default mount is `<build-dir>/src`, and the staged binaries `zpr-dev` points the scripts at live in `<build-dir>/dist` — outside it. So `zpr-dev` must pass `WORKSPACE=<build-dir>`.

The first draft of this plan claimed `:=` blocks that and scheduled a one-line `?=` change in `zl-zpr-core` (N1, zipline#91). That was wrong, and Codex's review of PR #23 caught it: a variable given on the `make` command line overrides every ordinary assignment in the makefile (GNU Make manual, *Overriding Variables*), and `make -n -C integration-test docker-test TEST=one-node-test.sh WORKSPACE=/tmp/x` on GNU Make 4.4.1 prints `-v /tmp/x:/tmp/x` against the unmodified Makefile. `:=` is also the better choice: an ambient exported `WORKSPACE` in the operator's shell cannot leak into the mount, which `?=` would have allowed. N1 is withdrawn; the `zl-zpr-core` floor for the container route is `c163628` (Finding 2), the commit that forwards the environment, not a new one.

### Finding 2 — the Makefile already forwards the overrides `zpr-dev` sets

`c163628` added `FORWARD_ENV`: every `PH_BIN`, `PH_DEBUG_BIN`, `VS_BIN`, `VS_ADMIN_BIN`, `VALKEY_SERVER_BIN`, `ZPR_TEST_VERBOSE`, … that is set in the caller's environment is passed into the container with a bare `-e NAME`. `zpr-dev`'s `netns_plan` sets exactly those on the child environment, so running `make docker-test` per script under the same `env` reuses everything and `zpr-dev` never learns a `docker run` flag. The one override that must **not** be forwarded is `VALKEY_SERVER_BIN`: the image ships its own at `/usr/bin/valkey-server` and a host path would not exist inside the container.

### Finding 3 — the `a2a-pubkey-test.sh` prep build needs no change

`netns_plan` builds the `enable-security-testing` `ph` with `cargo` on the host in the `zl-zpr-core` worktree and points `PH_BIN` at `<worktree>/target/debug/ph`. That path is under `<build-dir>/src`, inside the mount either way. The Makefile's default `TESTS` set excludes `a2a-pubkey-test.sh`, but `TEST=<script>` runs whatever it is given, and `zpr-dev` always names the script.

### Finding 4 — per-script invocation preserves the manifest's breakdown

`run_netns` records one entry per script in `tiers.netns.repos` (`passed` / `failed: …` / `failed at prep …`) and one log file per script. Calling `make docker-test TEST=<script>` once per script keeps that shape and the "continue after a failing script" rule intact. Calling the Makefile's run-all target once would collapse seven results into one and use its glob rather than `zpr-dev`'s blessed list.

### Finding 5 — a container run is a different provenance

The emitted manifest records `tiers.netns.sudo: nopasswd | primed` and spec-003 §6 says the two are never conflated (zipline#70). A run as root inside `--privileged` — which also lifts seccomp so io_uring works — is a third provenance and must be visible the same way, plus a coverage note (zipline#87), so a green manifest never implies the tests ran on the host.

### Finding 6 — the N3 manual run: the container route works end to end

Recorded from zl-zpr-dev-context PR #25's verification section (zipline#93,
acceptance's manual step). `ZPR_WORKSPACE=$HOME/zl_src ./target/debug/zpr-dev
build --tip --test all --force` ran on a host with no passwordless sudo, no
host `valkey-server`, and a Docker daemon up — **exit 0**, all seven netns
scripts passed through the container:

```
netns tier (docker fallback; host route unavailable: missing: passwordless sudo (or pass --prompt-for-sudo), valkey-server):
one-node-test.sh: passed
one-node-v6-test.sh: passed
one-node-oidc-test.sh: passed
capture-test.sh: passed
oidc-file-interplay-test.sh: passed
fake-idp-smoke-test.sh: passed
a2a-pubkey-test.sh: passed
```

The emitted manifest recorded exactly contract 2: `tiers.netns` with
`status: passed`, `sudo: container`, all seven scripts `passed` under
`repos`, and the generated `resolved.notes` line quoting the host route's
real gap. No host with passwordless sudo was available for a host-route
control, so the host-route shape (`sudo: nopasswd`/`primed`, no note, plan
byte-identical to pre-#93) is verified by the test suite instead.

## Decisions

- **Automatic fallback, no new flag.** Operator decision 2026-09-23. Host route runnable → host. Otherwise container route runnable → container. Otherwise skip/error as today. No `--netns-runner`; if forcing the container on a host with working sudo is ever wanted, that is a new small issue.
- **The fallback covers any host-route miss, not only sudo.** A host with sudo but no `valkey-server` also falls to the container, which carries one. The rule is "host route can run" as a whole, so the reason text can list both routes' gaps.
- **`--prompt-for-sudo` remains an explicit host request.** Given the flag: a successful prime → host. A failed prime (wrong password, `timestamp_timeout=0`) → container if available, and the existing diagnostic note is kept in the output. No terminal on stdin → exit 1 as today; the flag was misused, and guessing the container would hide that.
- **`zpr-dev` drives `zl-zpr-core`'s Makefile; it does not own `docker run`.** Global constraint from the build-sets plan: each repository's own `Makefile` stays authoritative.
- **The provenance value is `sudo: container`.** The scripts still call `sudo`; inside the container it is satisfied by being root. Keeping the field name preserves the manifest shape for existing consumers.
- **The container's provenance carries the real host-route failure.** The container may be chosen because sudo is missing *or* because `valkey-server` / `python3` is (second decision above), so `NetnsRunner::Container` carries the host route's reason string and every message that explains the fallback — stdout header, coverage note — quotes it. A fixed "no passwordless sudo" text would record false provenance (Codex review of PR #23).
- **No `BASE_IMAGE` knob in `zpr-dev`.** A host-vs-image glibc mismatch surfaces as script failures whose log shows the loader error; the manual workaround (`make docker-image BASE_IMAGE=ubuntu:<host release>` before the build, same image tag) is documented as a known ceiling. Add the knob only if someone hits it.

## Global constraints

- **A skipped or downgraded test is visible in the output and in the emitted manifest.** Unchanged from the build-sets plan and spec-003 §6; the container route is a downgrade in provenance, not in coverage, and is recorded as such.
- **Probes stay read-only and dry-run safe.** The new daemon probe is `docker info` (exit status only), permitted under `--dry-run` alongside `sudo -n true` (spec-003 §7.2).
- **The live workspace is never modified.** The container mounts `<build-dir>` only. The live `zl-zpr-core` checkout is not mounted and not touched.
- **Selection logic is pure and unit tested with injected probes**, in the style of `netns_gate` / `docker_gate` / `check_gate` today. Real `docker` and real `sudo` are never invoked by a test.
- **Rust gates** as in `skills/rust-coding-guidelines/SKILL.md`; a bug gets a failing test before its fix. No new crates.

## Cross-repository interface contracts

### 1. `integration-test/Makefile` invocation (`zl-zpr-core` @ `c163628` defines; `zpr-dev` N3 consumes)

```
make -C <core-worktree>/integration-test docker-test TEST=<script> WORKSPACE=<build-dir>
```

- `WORKSPACE` is overridden from the command line as GNU Make semantics already allow (Finding 1); the Makefile is not changed. Default remains `$(abspath $(CURDIR)/../..)`.
- Environment: the `FORWARD_ENV` names, when set in the `make` process's environment, reach the container verbatim. The caller sets `PH_BIN`, `PH_DEBUG_BIN`, `VS_BIN`, `VS_ADMIN_BIN` to paths under `WORKSPACE`, and must ensure `VALKEY_SERVER_BIN` is **absent** from that environment — not merely un-set by itself, since `run_env_command` inherits the operator's environment and an exported `VALKEY_SERVER_BIN` would otherwise be forwarded (Codex review of PR #23).
- Exit status is the script's. The image is built on first use (`docker-test` depends on `docker-image`, cached thereafter) and its build output is part of the invocation's stdout/stderr.
- Working directory inside the container is `<core-worktree>/integration-test`, which is under `WORKSPACE`.

### 2. Emitted manifest (`zpr-dev` N3 defines; spec-003 §3/§6 and `docs/BUILD.md` N4 document)

```yaml
tiers:
  netns:
    status: passed
    sudo: container            # nopasswd | primed | container
    repos: { one-node-test.sh: passed, ... }
resolved:
  notes:
    - "netns integration tests ran in a privileged Docker container, not on the host (host route unavailable: missing: passwordless sudo (or pass --prompt-for-sudo))"
```

`sudo` is absent for a skipped tier, as today. The note is generated only when the container route ran, and its parenthetical is the host route's actual failure text, whatever it was.

## Dependency graph and order

```
N2 (zpr-dev: probe + runner selection) ─> N3 (zpr-dev: container runner, manifest) ─> N4 (docs, plan COMPLETE)
```

Strictly serial, all in one repository. N3 consumes the selection type from N2; N4 documents what N3 emits. (N1 was withdrawn — Finding 1.)

## Issue map

Filed in `mkolehmainen/zipline` under umbrella [#90](https://github.com/mkolehmainen/zipline/issues/90); the umbrella carries these as sub-issues in this order.

| ID | Repo | Title | Blocked by |
|---|---|---|---|
| ~~N1 · [#91](https://github.com/mkolehmainen/zipline/issues/91)~~ | zl-zpr-core | *Withdrawn:* `WORKSPACE` is already overridable from the `make` command line (Finding 1) | — |
| N2 · [#92](https://github.com/mkolehmainen/zipline/issues/92) | zl-zpr-dev-context | `zpr-dev build`: Docker daemon probe and netns runner selection (host → container → skip) | — |
| N3 · [#93](https://github.com/mkolehmainen/zipline/issues/93) | zl-zpr-dev-context | `zpr-dev build`: run the netns tier through `make docker-test`; `sudo: container`; coverage note | N2 |
| N4 · [#94](https://github.com/mkolehmainen/zipline/issues/94) | zl-zpr-dev-context | spec-003 §6, `docs/BUILD.md`, plan COMPLETE | N3 |

---

## Phase N1 — withdrawn

Task N1 (`zl-zpr-core`, zipline#91) proposed changing `WORKSPACE :=` to `?=`. Withdrawn 2026-09-23: the command-line override already works (Finding 1). Nothing lands in `zl-zpr-core`.

---

## Phase N2 — `zpr-dev`: probe and selection (`zl-zpr-dev-context`)

### Task N2: Docker daemon probe and netns runner selection

**Scope.** The pure half: decide which route the netns tier takes, and say so under `--dry-run`. No execution changes. Until N3 lands, a `Container` selection must be refused **at the gate**: `netns_gate` maps it to `TierGate::Skip("docker fallback selected but not implemented yet (zipline#93)")`, so an explicit `--test netns` / `--test all` still errors through `check_gate` and a default selection still skips visibly. It must *not* be a run-time skip behind a `Run` gate — skipped outcomes do not fail `execute_build`, so that would let an explicitly requested tier exit 0 without running (Codex review of PR #23). (Alternative if preferred at planning-comment time: land N2 and N3 in one PR.)

- [ ] `tiers::Probes` gains `docker_daemon: bool` — `docker info` succeeds — gathered only when `docker` is on `PATH`, like `docker_compose` today.
- [ ] New `tiers::NetnsRunner { Host(SudoProvenance), Container { host_reason: String } }` — `host_reason` is the host route's own missing-prerequisite text, the same words the skip would have used — and a pure `tiers::select_netns_runner(&Probes) -> Result<NetnsRunner, String>` implementing the *Decisions* rule. The `Err` text names both routes' gaps, e.g. `missing: passwordless sudo (or pass --prompt-for-sudo), valkey-server; docker fallback unavailable: docker daemon not reachable`. The existing `sudo -v succeeded but credentials did not cache` note is appended when it applies.
- [ ] `netns_gate` is expressed in terms of `select_netns_runner` so there is one source of truth: `Ok(Host(_))` → `TierGate::Run`; `Ok(Container { .. })` → `TierGate::Skip("docker fallback selected but not implemented yet (zipline#93)")` until N3 flips it to `Run`; `Err(reason)` → `TierGate::Skip(reason)`.
- [ ] `docker_gate` for the existing `docker` tier also requires `docker_daemon`, with the reason `docker daemon not reachable` (a `docker` on `PATH` with no daemon fails that tier late today; the probe is free once it exists).
- [ ] `netns_dry_run_text` reports the selected route: unchanged for host; for the container, `docker fallback selected (host route unavailable: <host_reason>); not implemented yet (zipline#93)` under N2, becoming `would run in docker (host route unavailable: <host_reason>)` in N3; with `--prompt-for-sudo` and the container as the fallback, `would prompt for sudo (--prompt-for-sudo); on failure would fall back to docker`. Dry-run must never claim the container *would run* before N3 exists.
- [ ] `execute_build`'s probe block (`src/build/mod.rs`, the `selection.contains("netns")` arm) stores the `NetnsRunner` in `BuildInputs` in place of the separate `netns_sudo` / `netns_skip` pair for this tier.
- [ ] Unit tests, probes injected: host wins when both routes work; container chosen when only sudo is missing; container chosen when only `valkey-server` is missing; skip with a two-part reason when neither works; `docker` on `PATH` without a daemon does not select the container; `--prompt-for-sudo` outcomes `Primed` → host, `PromptFailed` / `CacheDisabled` → container when available (note retained), `NoTty` → still the up-front exit 1. Dry-run text for each.
- [ ] Integration test: on a fake-probe run where sudo is missing and docker is present, `--test all` (explicit) still exits 1 at the gate with the `not implemented yet (zipline#93)` reason, and `--dry-run` shows the selection text above.

**Acceptance.** `cargo test` green; `make check` clean; `zpr-dev build --dry-run --test all` on this workstation reports the container as selected and not yet implemented, naming the host route's gap. An explicit `--test netns` still fails at the gate. Real `docker` is not invoked by any test.

---

## Phase N3 — `zpr-dev`: container runner and manifest (`zl-zpr-dev-context`)

### Task N3: run the netns tier through `make docker-test`

**Scope.** Execute the `Container` selection, record it.

- [ ] `netns_gate`: `Ok(Container { .. })` → `TierGate::Run` (removing N2's placeholder skip); `netns_dry_run_text` says `would run in docker (host route unavailable: <host_reason>)`.
- [ ] `netns_plan` takes the `NetnsRunner`. For `Container`, each `NetnsScript` runs `make` with args `-C <core-worktree>/integration-test docker-test TEST=<script> WORKSPACE=<build-dir>`, working directory the worktree, the same `env` as the host route **minus** `VALKEY_SERVER_BIN`, **and** `VALKEY_SERVER_BIN` explicitly removed from the child's inherited environment: `NetnsScript` gains an `env_remove: Vec<&'static str>` applied with `Command::env_remove` in `run_env_command`, because that function inherits this process's environment and only adds the listed pairs (Codex review of PR #23). The `a2a-pubkey-test.sh` prep step is unchanged (Finding 3). For `Host`, the plan is byte-identical to today's.
- [ ] Before planning a container run, check the worktree's `integration-test/Makefile` exists and defines `FORWARD_ENV` (contract 1's floor, `zl-zpr-core` @ `c163628`). If not: the tier is skipped — or errors when explicit, through `check_gate` — with reason `zl-zpr-core @ <sha> predates the Docker runner (integration-test/Makefile with FORWARD_ENV, zl-zpr-core c163628)`.
- [ ] `run_netns` needs no change beyond consuming the plan; log files stay `logs/netns-<script>.log`. The first container script's log includes the image build.
- [ ] `SudoProvenance` gains `Container` (serializes `container`); the netns `Tier` gets it from the runner. `Tier::with_sudo`'s comment and the `Tier.sudo` field comment list all three values.
- [ ] Coverage (zipline#87): when the container route ran, `resolved.notes` gains the generated line from contract 2 with `host_reason` in its parenthetical. Not when the host route ran.
- [ ] Stdout: `netns tier (docker fallback; host route unavailable: <host_reason>):` as the tier header when the container is used, so an operator watching the run sees the real cause without opening the manifest.
- [ ] Tests: plan shape for `Container` (program `make`, `TEST=` per script, `WORKSPACE=` is the build dir, `VALKEY_SERVER_BIN` in `env_remove`, prep step present for a2a only); an **executed** child — e.g. `sh -c 'test -z "$VALKEY_SERVER_BIN"'` through `run_env_command` with `VALKEY_SERVER_BIN` set in the test process's environment — proving the removal reaches the child; Makefile-floor check both ways; manifest serialization `sudo: container`; the generated note present iff the container ran and quoting the host reason (integration test on the emitted manifest, in the zipline#87 test style); `--verbose` still exports `ZPR_TEST_VERBOSE=1`.
- [ ] Manual verification recorded in the PR: `zpr-dev build --tip --test all` on a host without passwordless sudo and with Docker runs all seven scripts in the container and emits `sudo: container` plus the note. Record which scripts passed; a script failing for reasons unrelated to the runner (see `docs/BUILD.md` notes on `one-node-oidc-renewal-test.sh`) is recorded, not fixed here.

**Acceptance.** `cargo test` green; `make check` clean; the manual run above; a host with passwordless sudo produces a manifest identical in shape to today's (`sudo: nopasswd`, no new note).

---

## Phase N4 — Documentation and closure (`zl-zpr-dev-context`)

### Task N4: spec-003 §6, `docs/BUILD.md`, plan COMPLETE

- [ ] `zpr-dev/docs/specs/spec-003-build.md` §6: the netns row's prerequisites become "Linux, and either (passwordless `sudo` or `--prompt-for-sudo`, `valkey-server`, `python3`) or a reachable Docker daemon"; a bullet describes the host → container → skip rule, the `sudo: container` provenance, the generated note, and the `zl-zpr-core` floor; the `--prompt-for-sudo` bullet's *Known ceiling* paragraph ends "CI still needs a host with passwordless sudo, or a Docker daemon — the container fallback is the CI path" instead of "or a container". §3's sample manifest shows the third `sudo` value. §1.2 implementation status row for B5 notes the fallback.
- [ ] `docs/BUILD.md`: the tier table's netns row lists the Docker daemon as the alternative prerequisite; the "Without host `sudo`: run them in Docker" paragraph says `zpr-dev build` does this automatically and records `sudo: container`, and keeps the manual `make` commands for running outside a build; the glibc / `BASE_IMAGE` ceiling stays as the workaround.
- [ ] `zpr-dev/README.md` if it lists tier prerequisites.
- [ ] This document: `**Status:** COMPLETE (<date>)`, issue links, the manual-run outcome from N3 recorded under *Findings*.

**Acceptance.** `zpr-dev validate` clean; `zpr-dev sync` regenerates context; every path and function this plan cites exists.

---

## Out of scope

- **`--netns-runner host|docker|auto`.** Decided against for now; a small follow-up if forcing the container on a sudo-capable host is wanted.
- **`BASE_IMAGE` passthrough from `zpr-dev`.** Known ceiling with a manual workaround; see *Decisions*.
- **Running the `unit` or `docker` tiers in a container.** The unit tier needs no privilege; the docker tier already is one.
- **Narrowing `--privileged`** to a capability set. Belongs to `zl-zpr-core/integration-test/Makefile` and is recorded there as a deliberate simplification.
- **The `one-node-oidc-renewal-test.sh` script** is not in `zpr-dev`'s blessed list and is not added here.

## Resolved while planning

- Whether to call the Makefile's run-all target once or `TEST=<script>` per script → per script (Finding 4).
- Whether `zpr-dev` should own `docker run` flags → no; the Makefile is authoritative (Decisions).
- Whether a missing `valkey-server` on the host should also fall to the container → yes (Decisions).
- Whether `WORKSPACE` needs changing in `zl-zpr-core` → **no**; the first draft said yes and was corrected by Codex review of PR #23 (Finding 1). N1/zipline#91 withdrawn.
- Whether omitting `VALKEY_SERVER_BIN` from the plan's `env` is enough → no; the child inherits the process environment, so it must be removed explicitly (N3).
- Whether N2 may land with a run-time skip for the container selection → no; the refusal must be at the gate so explicit requests still error (N2).

## Open questions

None blocking. If N2 and N3 are judged too entangled at plan-comment time, they may be delivered in one PR against both issues; the issue split is for review granularity, not a hard boundary.
