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
~/src/zpr/
├── zpr-dev-context/     this repository — shared context, cloned separately
├── zpr-core/
├── zpr-common/
├── zpr-visaservice/
├── zpr-vsapi/
├── zpr-compiler/
├── zpr-policy/
├── zpr-rfcs/
├── zpr-demo/
├── zpr-utils/
└── zpr-dev-tools/
```

All are public repositories under the `org-zpr` GitHub organization at
`git@github.com:org-zpr/<name>.git`, and all track `main`.

Use `zpr-dev status` to see the state of every checkout at once.

---

## At a glance

| Repository | Language | What it is |
|---|---|---|
| [`zpr-core`](#zpr-core) | Rust | Core ZPR components — the node and its adapters |
| [`zpr-common`](#zpr-common) | Rust | Shared `zpr` crate: protocol types, constants, IDL |
| [`zpr-visaservice`](#zpr-visaservice) | Rust | The Visa Service and its policy evaluator |
| [`zpr-vsapi`](#zpr-vsapi) | Cap'n Proto | IDL for the Visa Service API |
| [`zpr-compiler`](#zpr-compiler) | Rust | `zplc`, the ZPL policy compiler |
| [`zpr-policy`](#zpr-policy) | Cap'n Proto | IDL for the binary policy descriptor |
| [`zpr-rfcs`](#zpr-rfcs) | LaTeX / Docker | Public ZPR RFCs — the architectural reference |
| [`zpr-demo`](#zpr-demo) | HCL | Runnable ZPRnet demonstrations |
| [`zpr-utils`](#zpr-utils) | Rust | Non-ZPR-specific utility crates |
| [`zpr-dev-tools`](#zpr-dev-tools) | Docker | Development and build tooling |

---

## How they fit together

```text
zpr-policy  ─┐                        (Cap'n Proto schemas, vendored as
zpr-vsapi   ─┴─► zpr-common            submodules inside zpr-common)
                    │
                    ├──► zpr-core          the node
                    └──► zpr-visaservice   the visa service
                                ▲
                 zpr-compiler ──┘   emits the signed binary policy
                                    the visa service evaluates
```

- **`zpr-common` is the hub.** It packages the shared types — addresses,
  distinguished names, packet metadata, the NODE–VS API structures, and the
  binary policy format — and vendors `zpr-policy` and `zpr-vsapi` as Git
  submodules so the Cap'n Proto schemas travel with it.
- **`zpr-core` and `zpr-visaservice` both depend on `zpr-common`**, pulled via
  Git in `Cargo.toml`; no manual setup is required.
- **`zpr-compiler` closes the policy loop.** It compiles ZPL source into a
  signed binary policy that the visa service loads; the signing key must match
  the one the visa service is configured with.
- **`zpr-utils` and `zpr-dev-tools` are leaves** — nothing in the protocol path
  depends on them.
- **`zpr-rfcs` is the specification**, not code. Read it before changing
  protocol behavior.

---

## The repositories

### `zpr-core`

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

### `zpr-common`

The shared `zpr` crate. Anything used by more than one ZPR service belongs
here rather than being duplicated.

- Shared Rust types: addresses, DNs, packet metadata, and the helpers for
  writing and serializing them.
- Feature-gated wrappers over the policy and VSAPI types.
- IDL sources for the ZPR sub-protocols, included as **Git submodules** —
  `zpr-policy/` and `zpr-vsapi/`. Clone recursively, or run
  `git submodule update --init`, or the build will not find the schemas.

### `zpr-visaservice`

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

Depends on `zpr-common` for the NODE–VS API structures and the binary policy
format. Requires Rust edition 2024, `make`, OpenSSL, and a running
Redis/Valkey at runtime. Build and test with `make` / `make test`.

`libeval` is where an issuance decision is actually made — changes there change
the security posture of the whole system.

### `zpr-vsapi`

`vs.capnp`: the Cap'n Proto IDL describing the Visa Service API.

Consumed as a submodule of `zpr-common` rather than depended on directly.
Schemas are pre-release and field names, types, and structure are all still
subject to revision.

### `zpr-compiler`

The ZPL compiler. `zplc` translates ZPL source into the binary policy the visa
service evaluates, and `zpdump` inspects a compiled policy.

```bash
./zplc -k path/to/rsa-key.pem path/to/policy.zpl
```

The RSA key signs the binary policy and must match the key the visa service is
configured with. Configuration defaults to a `.zplc` file beside the `.zpl`
source; `-c` overrides it. Built with `make`.

`zpl.bnf` is the grammar, and `test-data/` holds the compiler's ZPL fixtures.

### `zpr-policy`

`policy.capnp`: the Cap'n Proto IDL for the ZPR policy descriptor — the wire
format `zpr-compiler` emits and `libeval` consumes.

Like `zpr-vsapi`, it is consumed as a submodule of `zpr-common`, and its
schemas are pre-release.

### `zpr-rfcs`

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

### `zpr-demo`

Runnable demonstrations of ZPR. Each has its own `README.md`.

```text
containerized-demo/   a running ZPRnet in containers
multinode-demo/       multiple nodes
iot-demo/             ZPR integrated with the Oracle IoT platform
```

The fastest way to see a working ZPRnet without assembling one by hand.

### `zpr-utils`

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

### `zpr-dev-tools`

Development and build tooling for the project, currently the `docker/`
images used to produce reproducible build environments.

---

## Repositories outside the workspace

| Repository | Why it is not cloned |
|---|---|
| `zpr-dev-context` | This repository. It is the context checkout, cloned by `zpr-dev setup` before the manifest is read, so it is not listed as a manifest entry. |
| `zpr-bas` | ZPR Basic Authentication Service. Deprecated. Excluded from the default workspace by request; clone it by hand if you need it. |

The organization also holds several private repositories that are not part of
the standard workspace and are not listed here.

---

## Adding a repository to the workspace

1. Add a `name` / `url` entry to [`workspace.yaml`](../workspace.yaml).
   `default_branch` defaults to `main`, so omit it unless it differs.
2. Add the repository to the table and a section to this document.
3. Run `zpr-dev setup` — existing checkouts are left untouched and only the
   new repository is cloned.

Repository-specific instructions belong in that repository's `AGENTS.repo.md`,
not here. Cross-repository architecture belongs in this `docs/` directory.
