# ZPR Repository Inventory

The repositories that make up the standard ZPR development workspace, what
each one holds, and how they depend on one another.

The authoritative list is [`workspace.yaml`](../workspace.yaml) at the root of
this repository — `zpr-dev setup` clones exactly what it names. This document
describes that set; when the two disagree, `workspace.yaml` is correct and this
file needs updating.

---

## Workspace layout

`zpr-dev setup` clones every repository side by side under the workspace root,
each into a directory matching its repository name:

```text
~/src/zl_zpr/
├── zl-zpr-dev-context/     this repository — shared context, cloned separately
├── zl-zpr-core/
├── zl-zpr-common/
├── zl-zpr-visaservice/
├── zl-zpr-vsapi/
├── zl-zpr-compiler/
├── zl-zpr-policy/
├── zl-zpr-rfcs/
├── zl-zpr-demo/
├── zl-zpr-utils/
└── zl-zpr-dev-tools/
```

All are public repositories under the [`mkolehmainen`](https://github.com/mkolehmainen)
GitHub account at `git@github.com:mkolehmainen/<name>.git`, and all check out
`zipline` (see "Branch model" below).

Each is a fork of the corresponding upstream repository in the
[`org-zpr`](https://github.com/org-zpr) organization — `zl-zpr-core` forks
`org-zpr/zpr-core`, `zl-zpr-common` forks `org-zpr/zpr-common`, and so on.

> **The forks are renamed, not rewired.** Only the repository names and the
> clone URLs in `workspace.yaml` changed. Inside each fork, cross-repository
> references are untouched and still resolve to `org-zpr`: the Cargo Git
> dependencies in `Cargo.toml`, the submodule URLs in `zl-zpr-common`'s
> `.gitmodules`, and the reusable CI workflow in every `.github/workflows/`.
> Repointing those is separate work — see the notes in `BUILD.md`.

Use `zpr-dev status` to see the state of every checkout at once.

---

## Branch model

Every fork carries two long-lived branches:

| Branch | Role |
|---|---|
| `zipline` | **The working branch, and the repository default.** Feature branches start here and pull requests target it. |
| `main` | **A read-only mirror of upstream `main`.** Never commit to it. |

Keeping `main` pristine is what makes the fork maintainable: syncing upstream is
always a fast-forward that can never conflict, `git diff main...zipline` is
exactly the zipline delta, and contributing a change back upstream means
branching off `main` and cherry-picking rather than untangling merges.

Sync `main` from upstream server-side, which needs no local checkout:

```bash
gh repo sync mkolehmainen/zl-zpr-<name> --source org-zpr/zpr-<name>
```

Do **not** point `main` at the upstream remote with
`git branch main --set-upstream-to=upstream/main` — a later `git push` on `main`
would then try to write to `org-zpr`, which will fail confusingly.

To bring upstream work into zipline, **merge, never rebase**:

```bash
git checkout zipline
git merge main          # after main has been synced
```

`zipline` is long-lived and shared, so rebasing it rewrites published history.
Merge commits on `zipline` are expected and fine.

### Tag names need a `zl-` prefix

The forks inherited upstream's tags (`zl-zpr-common` arrived carrying `v0.7.1`
through `v0.9.0`), and upstream keeps adding more. Because the Cargo
dependencies pin *tags*, an unprefixed zipline tag can collide with an upstream
tag of the same name and make `git fetch --tags` ambiguous. Tag zipline releases
as `zl-v0.26.0`, not `v0.26.0`.

---

## At a glance

| Repository | Language | What it is |
|---|---|---|
| [`zl-zpr-core`](#zl-zpr-core) | Rust | Core ZPR components — the node and its adapters |
| [`zl-zpr-common`](#zl-zpr-common) | Rust | Shared `zpr` crate: protocol types, constants, IDL |
| [`zl-zpr-visaservice`](#zl-zpr-visaservice) | Rust | The Visa Service and its policy evaluator |
| [`zl-zpr-vsapi`](#zl-zpr-vsapi) | Cap'n Proto | IDL for the Visa Service API |
| [`zl-zpr-compiler`](#zl-zpr-compiler) | Rust | `zplc`, the ZPL policy compiler |
| [`zl-zpr-policy`](#zl-zpr-policy) | Cap'n Proto | IDL for the binary policy descriptor |
| [`zl-zpr-rfcs`](#zl-zpr-rfcs) | LaTeX / Docker | Public ZPR RFCs — the architectural reference |
| [`zl-zpr-demo`](#zl-zpr-demo) | HCL | Runnable ZPRnet demonstrations |
| [`zl-zpr-utils`](#zl-zpr-utils) | Rust | Non-ZPR-specific utility crates |
| [`zl-zpr-dev-tools`](#zl-zpr-dev-tools) | Docker | Development and build tooling |

---

## How they fit together

```text
zl-zpr-policy ─┐                     (Cap'n Proto schemas, vendored as
zl-zpr-vsapi  ─┴─► zl-zpr-common      submodules inside zl-zpr-common)
                       │
                       ├──► zl-zpr-core          the node
                       └──► zl-zpr-visaservice   the visa service
                                    ▲
                 zl-zpr-compiler ───┘   emits the signed binary policy
                                        the visa service evaluates
```

- **`zl-zpr-common` is the hub.** It packages the shared types — addresses,
  distinguished names, packet metadata, the NODE–VS API structures, and the
  binary policy format — and vendors `zl-zpr-policy` and `zl-zpr-vsapi` as Git
  submodules so the Cap'n Proto schemas travel with it.
- **`zl-zpr-core` and `zl-zpr-visaservice` both depend on `zl-zpr-common`**, pulled via
  Git in `Cargo.toml`; no manual setup is required.
- **`zl-zpr-compiler` closes the policy loop.** It compiles ZPL source into a
  signed binary policy that the visa service loads; the signing key must match
  the one the visa service is configured with.
- **`zl-zpr-utils` and `zl-zpr-dev-tools` are leaves** — nothing in the protocol path
  depends on them.
- **`zl-zpr-rfcs` is the specification**, not code. Read it before changing
  protocol behavior.

---

## The repositories

### `zl-zpr-core`

Core ZPR components: the node implementation, its adapters, and the
integration tests that exercise a running ZPRnet.

```text
adapter/            node adapters
libnode2/           the node library
integration-test/   end-to-end tests
examples/           worked examples
diagrams/           architecture diagrams
packet_walk.md      a packet's path through the node
```

A Cargo workspace driven by `make`. The build pulls tools and libraries from
several of the repositories below, so it expects the standard workspace layout.

> Pre-release: breaking changes may land without notice, and the end-to-end
> security features are not all implemented yet.

### `zl-zpr-common`

The shared `zpr` crate. Anything used by more than one ZPR service belongs
here rather than being duplicated.

- Shared Rust types: addresses, DNs, packet metadata, and the helpers for
  writing and serializing them.
- Feature-gated wrappers over the policy and VSAPI types.
- IDL sources for the ZPR sub-protocols, included as **Git submodules** —
  the directories are still named `zpr-policy/` and `zpr-vsapi/` and still point
  at `org-zpr`, because `.gitmodules` was not rewritten when the fork was
  renamed. Clone recursively, or run `git submodule update --init`, or the build
  will not find the schemas.

### `zl-zpr-visaservice`

The ZPR Visa Service: policy evaluation and visa issuance.

```text
vs                  the visa service itself (aka "v2vs")
libeval             the evaluator — compares described traffic to policy
                    to decide whether a visa is issued
zpt                 ZPR Policy Tester, a CLI for exercising libeval
vs-admin            CLI administration client for the vs HTTPS admin API
zpr-dashboard       CLI dashboard for monitoring visa service activity
admin-api-types     data structures shared by vs and vs-admin
integration-test    shell-based integration tests, including libeval
                    evaluation tests driven through zpt
tools               helper scripts, including zpr-pki for PKI operations
admin-http-api.txt  reference for the vs HTTPS admin API
config-example.yaml annotated example vs configuration
```

Depends on `zl-zpr-common` for the NODE–VS API structures and the binary policy
format. Requires Rust edition 2024, `make`, OpenSSL, and a running
Redis/Valkey at runtime. Build and test with `make` / `make test`.

`libeval` is where an issuance decision is actually made — changes there change
the security posture of the whole system.

### `zl-zpr-vsapi`

`vs.capnp`: the Cap'n Proto IDL describing the Visa Service API.

Consumed as a submodule of `zl-zpr-common` rather than depended on directly.
Schemas are pre-release and field names, types, and structure are all still
subject to revision.

### `zl-zpr-compiler`

The ZPL compiler. `zplc` translates ZPL source into the binary policy the visa
service evaluates, and `zpdump` inspects a compiled policy.

```bash
./zplc -k path/to/rsa-key.pem path/to/policy.zpl
```

The RSA key signs the binary policy and must match the key the visa service is
configured with. Configuration defaults to a `.zplc` file beside the `.zpl`
source; `-c` overrides it. Built with `make`.

`zpl.bnf` is the grammar, and `test-data/` holds the compiler's ZPL fixtures.

### `zl-zpr-policy`

`policy.capnp`: the Cap'n Proto IDL for the ZPR policy descriptor — the wire
format `zl-zpr-compiler` emits and `libeval` consumes.

Like `zl-zpr-vsapi`, it is consumed as a submodule of `zl-zpr-common`, and its
schemas are pre-release.

### `zl-zpr-rfcs`

The publicly available RFCs for Zero-trust Packet Routing. This is the
architectural source of truth; the code implements what the RFCs describe.

Start here:

| RFC | Topic |
|---|---|
| 12 | ZPR overview — the problem and the approach |
| 4 | Terminology — the glossary for everything else |
| 15 | ZPL policy language overview |
| 16 | ZPR's concept of identity |

PDFs live under `pdf/`; they build from source via the Docker image in
`tools/`. Not every internal RFC is published.

### `zl-zpr-demo`

Runnable demonstrations of ZPR. Each has its own `README.md`.

```text
containerized-demo/   a running ZPRnet in containers
multinode-demo/       multiple nodes
iot-demo/             ZPR integrated with the Oracle IoT platform
```

The fastest way to see a working ZPRnet without assembling one by hand.

### `zl-zpr-utils`

Utility crates that are not ZPR-specific and could stand alone.

```text
cbpf-rs/     classic BPF handling
cslab/       an RCU-friendly concurrent slab allocator
rcu/         read-copy-update primitives
zpr-ext/     extension helpers
zpr-utils/   the utility crate itself
```

`cslab` and `rcu` are lock-free data structures verified under
[`loom`](https://docs.rs/loom); treat changes to them as concurrency-critical.

### `zl-zpr-dev-tools`

Development and build tooling for the project, currently the `docker/`
images used to produce reproducible build environments.

---

## Repositories outside the workspace

| Repository | Why it is not cloned |
|---|---|
| `zl-zpr-dev-context` | This repository. It is the context checkout, cloned by `zpr-dev setup` before the manifest is read, so it is not listed as a manifest entry. |
| `org-zpr/zpr-bas` | ZPR Basic Authentication Service. Deprecated. Not forked, and excluded from the default workspace by request; clone it from upstream by hand if you need it. |

The upstream organization also holds several private repositories that are not
part of the standard workspace, are not forked, and are not listed here.

---

## Adding a repository to the workspace

1. Add a `name` / `url` entry to [`workspace.yaml`](../workspace.yaml).
   `default_branch` defaults to `main`, so omit it unless it differs.
2. Add the repository to the table and a section to this document.
3. Run `zpr-dev setup` — existing checkouts are left untouched and only the
   new repository is cloned.

Repository-specific instructions belong in that repository's `AGENTS.repo.md`,
not here. Cross-repository architecture belongs in this `docs/` directory.
