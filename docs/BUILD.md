# Building and Testing ZPR

How to build, test, and check every repository in the workspace, and what the
cross-repository build dependencies actually are.

Each repository's own `README.md` and `Makefile` are authoritative for that
repository; when they disagree with this document, they are correct and this
file needs updating. See [`REPOSITORIES.md`](REPOSITORIES.md) for what each
repository contains.

---

## Prerequisites

| Tool | Why | Needed by |
|---|---|---|
| Rust, stable toolchain ([rustup](https://rustup.rs/)) | Edition 2024 | every Rust repo |
| `make` | drives the builds | every repo |
| `build-essential` / Xcode CLI tools | C toolchain for native crates | every Rust repo |
| `pkg-config`, `libssl-dev` | the `openssl` crate | `zl-zpr-visaservice`, `zl-zpr-compiler` |
| `capnproto` (the `capnp` binary) | compiles the Cap'n Proto schemas | `zl-zpr-common`, `zl-zpr-core`, `zl-zpr-compiler` |
| `libpcap-dev` | packet capture in `ph-cli` | `zl-zpr-core/adapter/cli` |
| Go | the `zpr-dashboard` CLI | `zl-zpr-visaservice` |
| Valkey or Redis | required by `vs` **at runtime** | running a ZPRnet, `zl-zpr-core` integration tests |
| `openssl` CLI | generating keys and certificates | running a ZPRnet |
| `plantuml` | architecture diagrams | `zl-zpr-core` (`make diagrams`) |
| Docker | reproducible build env, RFC PDFs, demos | optional |

Debian/Ubuntu:

```bash
sudo apt install build-essential make pkg-config libssl-dev capnproto \
                 libpcap-dev golang valkey
```

macOS:

```bash
brew install make pkg-config openssl capnp go valkey
```

The canonical prerequisite list is the dev-env image in
`zl-zpr-dev-tools/docker/dev-env/Dockerfile` — if a build needs something new, it
is added there.

---

## Git access

Rust dependencies between ZPR repositories are declared with **HTTPS Git URLs**
in `Cargo.toml`. If you authenticate to GitHub over SSH, tell Git to rewrite
them:

```bash
git config --global url.git@github.com:.insteadOf https://github.com/
```

If Cargo's built-in Git client fails to authenticate, have it shell out to
`git` instead — this is what CI does:

```bash
export CARGO_NET_GIT_FETCH_WITH_CLI=true
```

Go work additionally needs:

```bash
go env -w GOPRIVATE="github.com/org-zpr/*,github.com/mkolehmainen/*"
```

---

## Common conventions

Every Rust repository exposes the same `make` targets, so the same four
commands work everywhere:

```bash
make          # build (default goal)
make test     # unit tests
make check    # cargo fmt --check, then build with warnings denied
make clean    # cargo clean
```

`make check` is what CI enforces, and it is stricter than a plain build:
formatting must be clean and **warnings are errors**. Run it before pushing.
The CI equivalent is:

```bash
cargo fmt --check
cargo build --all-targets --config 'build.rustflags = ["-D", "warnings"]'
```

**`zl-zpr-core` has no root `make check`** — only its `libnode2` member does.
Run the CI equivalent above from the workspace root there instead.

CI builds every Rust repository through the shared reusable workflows in
`zl-zpr-dev-tools/.github/workflows/` (`rust-build-test.yml`, `rust-test.yml`), so
a green local `make test && make check` is a good predictor of a green CI run.

**On the forks there is no CI at all: Actions is disabled on all ten, by
decision.** So that prediction is moot — the local gate is not a predictor of a
remote one, it *is* the gate. A PR reporting no checks is the intended state.

None of that CI could run. `pr-notify.yml` needs `SLACK_ALERT_WEBHOOK_URL`, and
every caller of the shared `rust-build-test.yml` / `rust-test.yml` /
`go-build-test.yml` reusable workflows needs `ZPR_CICD_RO_TOKEN`, which the
reusable workflow declares **required** — and a fork inherits no secrets, so
those jobs failed within seconds, before any step ran, leaving every PR red for
a reason no PR could fix.

The switch is repository-level, so the workflow files are untouched and nothing
in `.github/` diverges from upstream. `skills/zpr/scripts/fork-ci.sh` reports the
state and reverses it (`--enable`); re-enabling means also supplying the two
secrets, or the same failures return.

---

## Per-repository builds

### `zl-zpr-common`

A Cargo workspace whose features gate the Cap'n Proto bindings, plus **two Git
submodules** carrying the IDL. Initialize them first or the build cannot find
the schemas:

```bash
make submodules-pull      # git submodule update --init --recursive
make build                # cargo build --all-targets -F all
make test                 # cargo test -F policy,vsapi,rcu-crossbeam-epoch
make bench                # cargo bench --features vsapi,rcu-aarc
```

Features: `policy`, `vsapi`, `all`. `build.rs` compiles the schemas from
`zl-zpr-policy/` and `zl-zpr-vsapi/`, so `capnproto` must be installed.

`make submodules-update` moves the submodules to the latest upstream commit —
that is a deliberate dependency bump, not routine setup.

Two invocation traps here, both of which look like a broken repository:

- A bare `cargo build --all-targets` gates off `serde` and fails in
  `packet_info.rs` with a misleading "unresolved import `serde`". Use
  `make build`, which passes `-F all`.
- `make check` (`cargo rustc --lib -- -D warnings`) does *not* pass `-F all`, so
  it fails the same way on clean `main`. That is a known pre-existing issue in
  this repository, not something your change caused.

### `zl-zpr-core`

A Cargo workspace with members `adapter/admin-api`, `adapter/ph`,
`adapter/cli`, and `libnode2`.

```bash
make          # cargo build && cargo build -p libnode2 --all-features
make test
make diagrams # PlantUML diagrams, needs the plantuml command
```

Binaries land in `./target/debug`. The packet handler `ph` is the one thing you
need to run a node or an adapter. `adapter/cli` (`ph-cli`) needs
`libpcap-dev`.

Each member also has its own `Makefile`, so a single component can be built in
isolation — CI does exactly this, one job per member.

### `zl-zpr-visaservice`

A Cargo workspace plus one Go component.

```bash
make            # build-rs (cargo build --all-targets) + build-go (zpr-dashboard)
make test       # cargo test, Go tests, then the shell integration tests
make check      # fmt and warning checks across every member
make release    # release tarball in build-release/, plus release-linux-<arch>.tar.gz
```

`make release` collects `vs`, `vs-admin`, `vsapikey`, `zpt`, and
`zpr-dashboard` into `build-release/` and tars it up. That tarball is what
`zl-zpr-core`'s integration tests consume.

`vs` needs a running Valkey/Redis at runtime, but not to build or to run the
unit tests.

### `zl-zpr-compiler`

```bash
make        # cargo build --all-targets
make test   # cargo test --lib, --bins, then the full suite
```

Produces `zplc` (ZPL → binary policy) and `zpdump` (inspect a compiled
policy):

```bash
./zplc -k path/to/rsa-key.pem path/to/policy.zpl
```

The RSA key signs the policy and must match the key the visa service is
configured with. Configuration defaults to the `.zplc` file beside the `.zpl`
source; `-c` overrides it.

### `zl-zpr-utils`

Independent crates — `cbpf-rs`, `cslab`, `rcu`, `zpr-ext`, `zl-zpr-utils` — each
built and CI-checked on its own:

```bash
cd cslab && cargo build --all-targets && cargo test
```

`cslab` and `rcu` are lock-free and carry [`loom`](https://docs.rs/loom) models
behind `cfg(loom)`. No CI job runs them, so exercise concurrency changes
locally:

```bash
RUSTFLAGS="--cfg loom" cargo test --release
```

### `zl-zpr-vsapi`, `zl-zpr-policy`

Cap'n Proto schemas only — nothing to build. They are consumed as submodules of
`zl-zpr-common`, and `zl-zpr-common`'s `build.rs` compiles them.

### `zl-zpr-rfcs`

PDFs are built from Markdown with pandoc, via the Docker image in `tools/`:

```bash
cd tools && docker build . --tag rfcgen:latest && cd ..
docker run --rm -v "$PWD":/work -w /work rfcgen:latest \
    sh -lc "git config --global --add safe.directory /work && make"
```

Output goes to `pdf/`. Unix line endings are enforced, so on a machine that has
ever been configured otherwise: `git config --global core.autocrlf input`.
Feedback on an RFC happens in GitHub Discussions on that repository.

### `zl-zpr-demo`

Each demo has its own `README.md`; `containerized-demo` also has a
`README-DEV.md` covering how to cut a new release. The demo build compiles a
policy with `zplc` and packages binaries with configuration into a versioned
release, optionally inside Docker (`USE_DOCKER=1`).

Running a published demo needs no build at all — image, binaries, and config
are released together and must share the same `YYYYMMDD` version.

### `zl-zpr-dev-tools`

Holds the Docker images (`docker/dev-env`) and the reusable CI workflows. Not a
build target itself.

### `zl-zpr-dev-context`

This repository. The `zpr-dev` tool is an ordinary Cargo project:

```bash
cd zpr-dev && cargo build && cargo test
```

---

## Cross-repository dependencies

Shared Rust crates are consumed **from Git by tag**, not by local path:

```toml
zpr       = { git = "https://github.com/mkolehmainen/zl-zpr-common.git", tag = "v0.28.0", ... }
zpr-ext   = { git = "https://github.com/mkolehmainen/zl-zpr-utils.git",  tag = "zpr-ext-v0.5.3" }
cbpf-rs   = { git = "https://github.com/mkolehmainen/zl-zpr-utils.git",  tag = "cbpf-rs-v0.2.0" }
```

> **Every ZPR-family URL now points at `mkolehmainen`.** `zipline#17` repointed
> the `zpr` crate and `zl-zpr-common`'s submodules; `zipline#18` finished the
> job for everything sourced from `zpr-utils` — `cbpf-rs`, `zpr-ext`,
> `zpr-utils`, and `cslab`/`rcu` in `zl-zpr-core/adapter/ph` — by repointing
> and re-tagging `zl-zpr-utils` first, so its crates cross-pin each other
> inside the fork and cargo sees one source per crate. One residue outlived
> #18: `zl-zpr-common` pinned `rcu` by the repo-wide tag `zpr-utils-v0.1.0`
> rather than `rcu`'s own tag line, so it never followed `rcu`'s releases;
> `zl-zpr-common` `v0.28.0` re-pins it to `rcu-v0.1.2` (same source, and for
> `rcu/src/` an identical tree). The full history is in
> `docs/plans/2026-09-17-build-sets.md`, *Finding 1*.
>
> Gate 1 of `zpr-dev build` (see "Compatible build sets" below) now checks
> this class of drift on every set: the same crate at two tags, or at one tag
> from two URLs, is an error before anything compiles.

Two consequences worth knowing:

1. **A local edit to `zl-zpr-common` does not affect a `zl-zpr-core` build.** The
   consumer pins a tag — and, in the forks, a tag in a different repository. To test a change across repositories, either tag and
   push it, or temporarily point the dependency at your checkout:

   ```toml
   zpr = { path = "../zl-zpr-common", features = ["vsapi", "policy"] }
   ```

   Revert that before committing — the pin is intentional.

   Do **not** reach for `[patch]` in `.cargo/config.toml` instead. A bare patch
   entry is *silently ignored*: cargo prints "patch was not used in the crate
   graph" and keeps building against the pinned tag, so the build looks like it
   picked up your change when it did not. Forcing the patch to take requires
   `cargo update -p zpr`, which rewrites the tracked `Cargo.lock` to a local
   absolute path. **A `Cargo.lock` containing a local path must never reach a
   PR** — strip the patch and restore the lockfile before committing.

   A type change in `zl-zpr-common` ripples into both `zl-zpr-core` and
   `zl-zpr-visaservice`. Search usages across every affected checkout, not just
   the repository you started in.

2. **Version bumps are explicit.** Updating shared types means tagging
   `zl-zpr-common` and bumping the tag in each consumer. Bump **every**
   consumer in the same round: `zpr-dev build`'s gate 1 (below) treats
   consumers pinning different tags of the same crate as an error, so a set
   cannot be cut while a bump is half-applied.

`zl-zpr-core` also patches `capnp` and friends to a fork
(`emilazy/capnproto-rust`) via `[patch.crates-io]`; keep that patch section in
sync when bumping Cap'n Proto.

### Versions and tags

What a repository's own `[package] version` in `Cargo.toml` means, and when a
PR moves it. The per-repository rules:

1. **A version means compatibility, not activity.** Bump it when something
   *outside* the repository has to care — a wire format, a container format, a
   published crate API — not because code changed. "What am I running?" is
   answered by the `git describe` suffix that every binary stamps into its
   `--version` output (`zipline#64`), not by the version number.

2. **`zl-zpr-compiler`'s version is load-bearing — the only one anything
   reads.** The compiler stamps its `CARGO_PKG_VERSION` into every
   `PolicyContainer` it emits (`zl-zpr-compiler/src/compiler.rs:1-2`, written at
   `src/policybinaryv2.rs:208-210`), and the visa service refuses to load a
   container that fails `libeval::pio::check_version`
   (`zl-zpr-visaservice/libeval/src/pio.rs:74`). That check is a near-exact
   match, not a floor: the container's **major must equal** the minimum's, the
   **minor must equal** it too, and the **patch must be greater than or
   equal**. The minimum is `POLICY_MIN_COMPILER_{MAJOR,MINOR,PATCH}`
   (`zl-zpr-visaservice/vs/src/config.rs:29-31`).

   So: bump the compiler's **minor** when the container's meaning or format
   changes, and move `POLICY_MIN_COMPILER_*` with it **in the same build set**.
   **Never move the minor for a non-policy reason** — doing so makes the visa
   service reject policy at runtime for a reason unrelated to policy. Patch
   bumps are free: an old minimum accepts a newer patch.

3. **`zl-zpr-common`, and the `zl-zpr-vsapi` / `zl-zpr-policy` submodules it
   carries: unchanged from consequence 2 above** — tag the crate, then bump
   the pin in every consumer in the same round. That rule is stated there;
   this list only places it among the others.

4. **Everything else** (`zl-zpr-visaservice`, `zl-zpr-core`, `zl-zpr-coredns`,
   `zl-zpr-demo`): no bump per merge. Bump when compatibility changes, or when
   cutting a tag.

5. **A tag matches the cargo version.** Existing practice, now written down:
   when cutting a tag, tag the version the `Cargo.toml` already carries (the
   compiler's `v0.18.1` tags version `0.18.1`). Where a repository's version
   has moved past its newest tag, the next tag continues from the version, not
   from the old tag line.

Why not bump-per-change: a version line that moves in every PR makes every PR
conflict with every other on that line, and two branches rebased past each
other can ship the same number for different code. It would also dilute the
compiler's version — the only one with a runtime consumer — into a change
counter. Build identity is the `git describe` stamp's job (`zipline#64`).

---

## Compatible build sets

A **build set** is the answer to "these binaries go together": one YAML file
naming a ref — tag, branch, or sha — per binary-producing repository, which
`zpr-dev build` proves coherent, builds, tests, and turns into a manifest that
rebuilds the same set later. The full specification is
`zl-zpr-dev-context/zpr-dev/docs/specs/spec-003-build.md`; this section is the
working knowledge.

### The two manifests

- **Input manifest** — `zl-zpr-dev-context/build-sets/<name>.yaml`, committed
  and reviewed. Names the five binary-producing repositories
  (`zl-zpr-compiler`, `zl-zpr-visaservice`, `zl-zpr-core`, `zl-zpr-coredns`,
  `zl-zpr-demo`) with one ref each. `zl-zpr-common`, `zl-zpr-policy` and
  `zl-zpr-vsapi` are deliberately absent: cargo consumes them from Git by tag,
  so the set's `zl-zpr-common` version is *derived* from what the consumers
  agree on. An optional `allow_pin_drift:` list tolerates a named crate's pin
  disagreement; every entry must carry a non-empty `reason` — the reviewed
  record of a divergence someone has decided to live with.
- **Emitted manifest** — `dist/zpr-set-<name>.yaml`, written by every run
  whose gates pass (even when a build step or test tier later fails — the most
  interesting set to reproduce is the one that broke). Same schema with every
  ref resolved to a 40-character sha, plus a diagnostic `resolved:` block
  (pins, versions, host toolchain, per-binary sha256, tier results) that a
  later read ignores. Feeding it back as `--manifest` rebuilds the same set:
  a sha resolves to itself.

Refs resolve against the **local checkouts, with no fetch** — `zpr-dev`
treats a fetch as a mutation. A tag pushed but never fetched fails as an
unknown ref; fetch first (e.g. `zpr-dev update --all`).

### The three gates

Gates run **before compilation** — a set whose pins disagree must fail in
seconds, not after fifteen minutes of cargo. Findings accumulate into one
`zpr-dev validate`-style report (`[OK]`/`[WARN]`/`[ERROR]`); any error exits 1.

1. **Shared-dep agreement (error).** Every ZPR-family or `rev`-pinned git
   dependency in every manifest of the set — plus `zl-zpr-common`'s own
   manifest, at the revision the consumers pin — must resolve to a single
   `(crate, url, tag-or-rev)`. Two tags for one crate, or one tag from two
   URLs, is an error naming every pinning file and line. An
   `allow_pin_drift` entry suppresses one crate's finding and echoes its
   reason; `--allow-pin-drift` downgrades all findings to warnings for a
   one-off.
2. **Freshness (warning, never an error).** A pin sitting behind the newest
   tag in its repository's checkout warns, ordered by semantic version. A set
   may legitimately sit behind; the warning makes that a decision, not an
   accident.
3. **Compiler / visa-service version (error).** `zplc`'s package version must
   satisfy the visa service's `POLICY_MIN_COMPILER_*` (major ==, minor ==,
   patch >=). A set that fails this builds perfectly and then loads no policy
   at runtime, so it is checked statically here and dynamically again by the
   `unit` tier's `pregen`.

**What gate 1 does *not* check.** It reads the root `Cargo.toml` and literal
workspace members of the repositories it scans — **manifests only, never
`Cargo.lock`** — so a pin carried *transitively* by a git dependency is
invisible. Concretely: `zpr-utils-v0.2.2`'s own manifest pins `zpr v0.8.1`,
so `zl-zpr-core/Cargo.lock` holds two `zpr` crates while gate 1 truthfully
reports the *manifest* pins consistent. `allow_pin_drift` cannot express this
either — there is no finding to suppress. A green gate 1 is agreement among
the manifests it read, not a single-version guarantee for the compiled
binaries; [zipline#69](https://github.com/mkolehmainen/zipline/issues/69)
tracks the blind spot and whether to widen the gate.

### The test tiers

| Tier | What runs | Prerequisites | Probe-gated |
|---|---|---|---|
| `unit` | `make test` per built repository, with `make pregen ZPLC=<dist>/zplc` first in `zl-zpr-visaservice` | none beyond the build | no |
| `netns` | the seven `zl-zpr-core/integration-test/` scripts, against `dist/` binaries | Linux, passwordless `sudo`, `valkey-server`, `python3` | yes |
| `docker` | `dns-demo` deploy + `test-dns.sh` + `docker compose down -v` | `docker`, `docker compose` | yes |

`--test` selects: `none`, `default` (the flag absent means the same), `all`,
or a comma-separated list. `default` selects every tier above with the
probe-gated ones allowed to skip: a tier whose prerequisites are missing is
**skipped with the reason recorded** — in the output and in the emitted
manifest, so a green run never overstates coverage. The same failure on an
explicitly requested tier (named in a list, or via `--test all`) is an error:
the machine cannot run what was asked.

### Cutting a new set

1. Fetch everything the set will pin: `zpr-dev update --all` (resolution
   itself never fetches).
2. `zpr-dev build --tip --test all` — `--tip` ignores refs and resolves
   `origin/<default_branch>` everywhere (`zipline`; `main` for
   `zl-zpr-coredns`). On a machine that cannot run a tier, drop it from the
   list and let the skip be recorded instead.
3. Review `dist/zpr-set-<name>.yaml`: the resolved shas, the `pins:` block,
   the tier results including recorded skips.
4. Copy it into `build-sets/<date>.yaml` and commit it. There is no separate
   authoring step; the emitted manifest *is* the set.
5. Reproduce before relying on it: `zpr-dev build --manifest
   build-sets/<date>.yaml` must resolve to the same shas and recompute the
   same `pins:` block.

Quick checks along the way: `zpr-dev build --tip --gates-only` runs just the
gates against the live checkouts (read-only, seconds), and `--dry-run` prints
the resolved shas, build order, tier probes and target `dist/` without
creating anything.

Build directory: `<workspace>/.zpr-build/<name>` (`--build-dir` overrides);
sources come from detached worktrees, so the live checkouts are never
touched. A directory left by a previous run is refused; `--force` removes and
recreates it. Exit codes are `zpr-dev`'s usual contract: 0 success or
warnings, 1 gate/build/test failure, 2 usage or configuration error.

---

## Integration tests

### `zl-zpr-visaservice`

Shell-based, driven by `make test` at the repository root, or directly:

```bash
cd integration-test && make test    # zpt-test.sh, zpt-test-connect.sh, tag-test.sh
```

These exercise `libeval` through `zpt` and need no node and no Valkey.

Test policies are pre-compiled and checked in. Regenerating them needs `zplc`
on `PATH` (or `ZPLC=/path/to/zplc`):

```bash
make pregen     # integration-test/pregen: recompile the .zpl fixtures
```

### `zl-zpr-core`

`integration-test/` stands up a real ZPRnet — node, visa service, and adapters
— on network namespaces. It is not run by `make test`; it needs binaries from
*other* repositories placed next to the scripts:

```text
integration-test/vs                 from zl-zpr-visaservice
integration-test/vs-admin           from zl-zpr-visaservice
integration-test/valkey-server      or set VALKEY_SERVER_BIN
target/debug/ph, target/debug/ph-cli   built here by make
```

The simplest source for the visa-service binaries is a `zl-zpr-visaservice`
release tarball (`make release` there, or download a published release) unpacked
into `integration-test/`. Then:

```bash
ZPR_TEST_VERBOSE=1 VALKEY_SERVER_BIN=/usr/bin/valkey-server \
    integration-test/one-node-v6-test.sh
```

Other entry points, each a standalone script:

| Script | Covers |
|---|---|
| `one-node-test.sh`, `one-node-v6-test.sh` | device-only authentication, IPv4 / IPv6 |
| `capture-test.sh` | packet capture |
| `one-node-oidc-test.sh` | OIDC login through the fake IdP, plus JWKS key rotation |
| `oidc-file-interplay-test.sh` | an `oidc` and a `file` trusted service in one policy |
| `one-node-oidc-renewal-test.sh` | silent OIDC renewal and disconnect-on-revocation. Takes several minutes: the renewal cadence is a real wall clock. It was the acceptance criterion for the node-to-adapter credential request, which landed as zipline#66; its CI job is no longer gated off, but the test has not yet been observed passing (see `docs/OIDC.md`, *Not yet*) |
| `fake-idp-smoke-test.sh` | the fake IdP's own endpoints. **Needs no root and no netns** — run it first when an OIDC test misbehaves |

Useful overrides: `DEBUG_TARGETS` (default `all=INFO`), `PH_BIN`, `VS_BIN`,
`NETEM_PARAMS` for link impairment.

The OIDC tests need a `vs` built from a `zl-zpr-visaservice` that carries the
feature under test; a stale binary left in `integration-test/` from an earlier
checkout is the usual cause of a confusing failure.

These tests create network namespaces and veth pairs with `sudo ip`, so they
are Linux-only and need passwordless `sudo` to run unattended.

**Without host `sudo`: run them in Docker.** `integration-test/Makefile` runs the
same scripts as root inside a throwaway privileged container, so the only host
prerequisite is a running Docker daemon and membership in the `docker` group.
The binaries are still built on the host and bind-mounted in; the image
(`integration-test/Dockerfile`, Ubuntu 26.04 by default so its glibc is at least
as new as a current host's) only supplies the tooling and `valkey-server`:

```bash
make -C integration-test docker-test                        # every *-test.sh, PASS/FAIL summary
make -C integration-test docker-test TEST=one-node-test.sh  # one script, args allowed
make -C integration-test docker-shell                       # root shell for debugging
```

`make integration-test-docker` at the repository root is the same as the first
line. The container runs `--privileged` because `ip netns`, `/dev/net/tun` and
io_uring (which Docker's default seccomp profile blocks) all need it. If the
host's glibc is newer than the image's, the binaries will not load; pass
`BASE_IMAGE=ubuntu:<host release>` to `docker-image` / `docker-test`.

---

## What to build to run a ZPRnet

A minimal ZPRnet is a node and a visa service, so three binaries:

| Binary | Repository | Role |
|---|---|---|
| `ph` | `zl-zpr-core` (`adapter/ph`) | packet handler — runs as node *or* adapter |
| `vs` | `zl-zpr-visaservice` (`vs`) | the visa service |
| `zplc` | `zl-zpr-compiler` | compiles the policy `vs` evaluates |

Plus a running Valkey/Redis for `vs`.

`zl-zpr-core/README.md` has the full walkthrough: generating the bootstrap RSA
keys, the CA and signed noise certificates, the node and adapter TOML configs,
the visa service TLS credentials, and starting everything in order. That
procedure is runtime setup rather than build, so it is not duplicated here.

---

## Building in the container

For a build environment identical to CI, use the dev-env image from
`zl-zpr-dev-tools/docker/dev-env` — Debian 12 with the toolchain, `capnproto`,
`libpcap`, and Valkey already installed. Published images are in the upstream
[org-zpr packages area](https://github.com/orgs/org-zpr/packages); forking a
repository does not copy its packages, so pull the images from there.

```bash
docker run --rm -it -v "$PWD":/work -w /work <dev-env-image> make
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `capnp: command not found`, or `build.rs` fails in `zl-zpr-common` | Cap'n Proto compiler missing | install `capnproto` |
| `zl-zpr-common` build cannot find `.capnp` files | submodules not initialized | `make submodules-pull` |
| Cargo cannot authenticate to `github.com/org-zpr/...` or `github.com/mkolehmainen/...` | SSH-only credentials, or Cargo's Git client | set `url.insteadOf`, or `CARGO_NET_GIT_FETCH_WITH_CLI=true` |
| `openssl` crate fails to build | dev headers missing | install `libssl-dev` / `pkg-config` |
| `pcap` crate fails to build | headers missing | install `libpcap-dev` |
| `vs` exits at startup | no Valkey/Redis | `systemctl start valkey-server`, or run `valkey-server` |
| Visa service rejects a policy | policy signed with the wrong key | re-run `zplc -k` with the key `vs` is configured with |
| CI fails but the local build passed | `make check` not run — warnings are errors in CI | `make check` |
| A change in `zl-zpr-common` has no effect on a consumer | the consumer pins a Git tag | tag and bump, or use a temporary `path` dependency |
| "patch was not used in the crate graph" | a bare `[patch]` cannot override a tag pin | use a temporary `path` dependency instead; never commit a locally-pathed `Cargo.lock` |
| `unresolved import `serde`` in `zl-zpr-common/packet_info.rs` | built without `-F all` | `make build`; `make check` fails this way on clean `main` too |
