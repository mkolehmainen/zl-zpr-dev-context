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
| `pkg-config`, `libssl-dev` | the `openssl` crate | `zpr-visaservice`, `zpr-compiler` |
| `capnproto` (the `capnp` binary) | compiles the Cap'n Proto schemas | `zpr-common`, `zpr-core`, `zpr-compiler` |
| `libpcap-dev` | packet capture in `ph-cli` | `zpr-core/adapter/cli` |
| Go | the `zpr-dashboard` CLI | `zpr-visaservice` |
| Valkey or Redis | required by `vs` **at runtime** | running a ZPRnet, `zpr-core` integration tests |
| `openssl` CLI | generating keys and certificates | running a ZPRnet |
| `plantuml` | architecture diagrams | `zpr-core` (`make diagrams`) |
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
`zpr-dev-tools/docker/dev-env/Dockerfile` — if a build needs something new, it
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
go env -w GOPRIVATE="github.com/org-zpr/*"
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

CI builds every Rust repository through the shared reusable workflows in
`zpr-dev-tools/.github/workflows/` (`rust-build-test.yml`, `rust-test.yml`), so
a green local `make test && make check` is a good predictor of a green CI run.

---

## Per-repository builds

### `zpr-common`

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
`zpr-policy/` and `zpr-vsapi/`, so `capnproto` must be installed.

`make submodules-update` moves the submodules to the latest upstream commit —
that is a deliberate dependency bump, not routine setup.

### `zpr-core`

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

### `zpr-visaservice`

A Cargo workspace plus one Go component.

```bash
make            # build-rs (cargo build --all-targets) + build-go (zpr-dashboard)
make test       # cargo test, Go tests, then the shell integration tests
make check      # fmt and warning checks across every member
make release    # release tarball in build-release/, plus release-linux-<arch>.tar.gz
```

`make release` collects `vs`, `vs-admin`, `vsapikey`, `zpt`, and
`zpr-dashboard` into `build-release/` and tars it up. That tarball is what
`zpr-core`'s integration tests consume.

`vs` needs a running Valkey/Redis at runtime, but not to build or to run the
unit tests.

### `zpr-compiler`

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

### `zpr-utils`

Independent crates — `cbpf-rs`, `cslab`, `rcu`, `zpr-ext`, `zpr-utils` — each
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

### `zpr-vsapi`, `zpr-policy`

Cap'n Proto schemas only — nothing to build. They are consumed as submodules of
`zpr-common`, and `zpr-common`'s `build.rs` compiles them.

### `zpr-rfcs`

PDFs are built from Markdown with pandoc, via the Docker image in `tools/`:

```bash
cd tools && docker build . --tag rfcgen:latest && cd ..
docker run --rm -v "$PWD":/work -w /work rfcgen:latest \
    sh -lc "git config --global --add safe.directory /work && make"
```

Output goes to `pdf/`.

### `zpr-demo`

Each demo has its own `README.md`; `containerized-demo` also has a
`README-DEV.md` covering how to cut a new release. The demo build compiles a
policy with `zplc` and packages binaries with configuration into a versioned
release, optionally inside Docker (`USE_DOCKER=1`).

Running a published demo needs no build at all — image, binaries, and config
are released together and must share the same `YYYYMMDD` version.

### `zpr-dev-tools`

Holds the Docker images (`docker/dev-env`) and the reusable CI workflows. Not a
build target itself.

### `zpr-dev-context`

This repository. The `zpr-dev` tool is an ordinary Cargo project:

```bash
cd zpr-dev && cargo build && cargo test
```

---

## Cross-repository dependencies

Shared Rust crates are consumed **from Git by tag**, not by local path:

```toml
zpr       = { git = "https://github.com/org-zpr/zpr-common.git", tag = "v0.25.0", ... }
zpr-ext   = { git = "https://github.com/org-zpr/zpr-utils.git",  tag = "zpr-ext-v0.5.3" }
cbpf-rs   = { git = "https://github.com/org-zpr/zpr-utils.git",  tag = "cbpf-rs-v0.2.0" }
```

Two consequences worth knowing:

1. **A local edit to `zpr-common` does not affect a `zpr-core` build.** The
   consumer pins a tag. To test a change across repositories, either tag and
   push it, or temporarily point the dependency at your checkout:

   ```toml
   zpr = { path = "../zpr-common", features = ["vsapi", "policy"] }
   ```

   Revert that before committing — the pin is intentional.

2. **Version bumps are explicit.** Updating shared types means tagging
   `zpr-common` and bumping the tag in each consumer. Consumers can legitimately
   sit on different tags for a while, as `zpr-core` and `zpr-visaservice`
   frequently do.

`zpr-core` also patches `capnp` and friends to a fork
(`emilazy/capnproto-rust`) via `[patch.crates-io]`; keep that patch section in
sync when bumping Cap'n Proto.

---

## Integration tests

### `zpr-visaservice`

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

### `zpr-core`

`integration-test/` stands up a real ZPRnet — node, visa service, and adapters
— on network namespaces. It is not run by `make test`; it needs binaries from
*other* repositories placed next to the scripts:

```text
integration-test/vs                 from zpr-visaservice
integration-test/vs-admin           from zpr-visaservice
integration-test/valkey-server      or set VALKEY_SERVER_BIN
target/debug/ph, target/debug/ph-cli   built here by make
```

The simplest source for the visa-service binaries is a `zpr-visaservice`
release tarball (`make release` there, or download a published release) unpacked
into `integration-test/`. Then:

```bash
ZPR_TEST_VERBOSE=1 VALKEY_SERVER_BIN=/usr/bin/valkey-server \
    integration-test/one-node-v6-test.sh
```

Other entry points are `one-node-test.sh` and `capture-test.sh`. Useful
overrides: `DEBUG_TARGETS` (default `all=INFO`), `PH_BIN`, `VS_BIN`,
`NETEM_PARAMS` for link impairment.

These tests create network namespaces and veth pairs with `sudo ip`, so they
are Linux-only and need passwordless `sudo` to run unattended.

---

## What to build to run a ZPRnet

A minimal ZPRnet is a node and a visa service, so three binaries:

| Binary | Repository | Role |
|---|---|---|
| `ph` | `zpr-core` (`adapter/ph`) | packet handler — runs as node *or* adapter |
| `vs` | `zpr-visaservice` (`vs`) | the visa service |
| `zplc` | `zpr-compiler` | compiles the policy `vs` evaluates |

Plus a running Valkey/Redis for `vs`.

`zpr-core/README.md` has the full walkthrough: generating the bootstrap RSA
keys, the CA and signed noise certificates, the node and adapter TOML configs,
the visa service TLS credentials, and starting everything in order. That
procedure is runtime setup rather than build, so it is not duplicated here.

---

## Building in the container

For a build environment identical to CI, use the dev-env image from
`zpr-dev-tools/docker/dev-env` — Debian 12 with the toolchain, `capnproto`,
`libpcap`, and Valkey already installed. Published images are in the
[org-zpr packages area](https://github.com/orgs/org-zpr/packages).

```bash
docker run --rm -it -v "$PWD":/work -w /work <dev-env-image> make
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `capnp: command not found`, or `build.rs` fails in `zpr-common` | Cap'n Proto compiler missing | install `capnproto` |
| `zpr-common` build cannot find `.capnp` files | submodules not initialized | `make submodules-pull` |
| Cargo cannot authenticate to `github.com/org-zpr/...` | SSH-only credentials, or Cargo's Git client | set `url.insteadOf`, or `CARGO_NET_GIT_FETCH_WITH_CLI=true` |
| `openssl` crate fails to build | dev headers missing | install `libssl-dev` / `pkg-config` |
| `pcap` crate fails to build | headers missing | install `libpcap-dev` |
| `vs` exits at startup | no Valkey/Redis | `systemctl start valkey-server`, or run `valkey-server` |
| Visa service rejects a policy | policy signed with the wrong key | re-run `zplc -k` with the key `vs` is configured with |
| CI fails but the local build passed | `make check` not run — warnings are errors in CI | `make check` |
| A change in `zpr-common` has no effect on a consumer | the consumer pins a Git tag | tag and bump, or use a temporary `path` dependency |
