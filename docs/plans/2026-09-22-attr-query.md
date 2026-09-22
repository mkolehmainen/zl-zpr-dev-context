# Attribute Query — networked attribute sources and the `zpr-attr/1` API

**Status:** IN FLIGHT — umbrella issue not yet filed. Spec: `docs/ATTRIBUTE_SERVICE.md`. While this plan is in flight it wins over the spec wherever they disagree.
**Date:** 2026-09-22
**Repo state this plan was written against:** `zl-zpr-dev-context` @ `53cc516`, `zl-zpr-policy` @ `434b276` (tag `v0.11.0`), `zl-zpr-common` @ `4b3ddf4` (tag `v0.28.0`; compiler and visa service both pin `v0.28.0`), `zl-zpr-compiler` @ `d4e306c` (package `0.18.1`), `zl-zpr-visaservice` @ `17ac0fd` (workspace `0.19.1`, `POLICY_MIN_COMPILER_MINOR = 18`), all on `zipline`.

> **ZPR process rule (`skills/zpr/SKILL.md`):** every GitHub issue gets its own plan posted as
> an issue comment *before* implementation. This document is the **master plan**: it fixes the
> ordering, the cross-repository interface contracts, and the scope and acceptance criteria of
> each issue. When an issue is picked up, expand its section here into the bite-sized TDD plan
> on the issue, against the code as it is *then*.

---

## Background

The visa service makes use of trusted services to discover attributes about the actors
(actors is what we call the participants in the ZPRnet) in the system.

Trusted services are used in two ways: (1) for actor authentication, and (2) for fetching
additional attributes.

So far in the implementation we have:

- **bootstrap** — a special trusted service baked into the visa service which can
  authenticate an actor based on an RSA key.
- **oidc** — a trusted service for authentication that can authenticate using the OIDC
  protocol (e.g. with Google).
- **file** — a trusted service for returning attributes where all the attributes are present
  in a flat JSON file.

To support our **zipline** (hosted ZPRnets) initiative we want to be able to query a database
for attributes. Going forward we will want to eventually integrate with common directory
services (e.g. LDAP, Microsoft Active Directory, and services like Okta).

The zipline system will have some sort of database back end into which users are adding
attributes for their actors. We want to design a "trusted service API" that it can implement
(since we are authoring zipline concurrently with ZPR) which will support the required visa
service operations.

This API implemented by zipline becomes a **reference API** that anyone could implement on
top of any attribute source. Clearly we would want to hook directly up to common systems
(e.g. Active Directory), but the existence of a simple API layer that the visa service speaks
natively is helpful. Consider it the MCP for attribute sources.

The deliverables are:

1. Solid software architecture internally for handling attribute services.
2. Concrete solution for interfacing with zipline.

### The operations a visa service needs

Architecturally the visa service uses a single unified interface (a trait), with the details
of each scheme in its own module. Against that interface the visa service needs:

1. **SCHEMA** — the attribute types a service returns. Only tangentially useful for producing
   visas (the visa service can validate policy configuration against it); the real use is for
   policy editors that check policy as it is written.
2. **QUERY** — the fundamental operation: "give me all the attributes you have for this
   actor", passing the actor's identity attribute(s), optionally restricted to the attribute
   keys the visa service is interested in.
3. **STREAM** — reactive push: a standing "send me attribute changes for this actor" so the
   service can update the visa service in real time.
4. **SATISFIES** — not needed until we support assertions: an attribute expression in, the
   identity keys that satisfy it out (`satisfies role=consultant, location=finland`).

All attributes are tuples `(KEY, VALUE, EXPIRATION)`. To these ZPR adds the source (which
service the attribute came from) and the type (single-valued, multi-valued, or tag).

---

## Findings

Traced by reading the forks' `zipline` branches on 2026-09-22. They set the plan's weight:
the internal architecture is already there, so the work is the wire API, one new store, and
the schema plumbing to carry its configuration.

### Finding 1 — deliverable 1 already exists

`TrustedServiceInterface` (`zl-zpr-visaservice/vs/src/trusted_services/mod.rs`) is the single
unified interface: `get_attributes_for_actor(identities)`, `flush()`, `current_revision()`,
`get_source_id()`. `FileAttributeStore` (`file_attribute_store.rs`) and `OidcTrustedService`
(`vs/src/oidc/store.rs`) implement it; `TrustedServicesMgr` (`manager.rs`) fans a query out to
every store concurrently and tracks, per actor and per source, the revision the actor was last
refreshed from. The factory (`factory.rs`) is an `api`-string dispatch. Adding a networked
store is **one module plus one factory arm**; no refactor precedes it, and this plan does not
invent one.

### Finding 2 — STREAM's receiving half already exists

`flush()` bumps a store's revision; `DELETE /admin/services/{id}/cache`
(`vs/src/admin_service.rs`, `flush_service_cache`) calls it and queues
`VsEvent::TrustedServiceChange`; the handler (`vs/src/event_mgr.rs`,
`handle_trusted_service_change`) re-queries every actor behind a live visa and re-checks those
visas, revoking any that no longer hold. `stale_sources_for_actor` narrows each refresh to the
sources whose revision moved. A push from an attribute service therefore needs only an admin
endpoint that (a) bumps the revision or (b) forgets named actors' revision records, then queues
the same event. No subscription protocol, no new worker.

### Finding 3 — the type of an attribute is policy's decision

Single, multi and tag come from the `returns_attributes` mapping via `AttributeMapper`
(`attribute_mapper.rs`); a store only supplies `name -> values`. The wire response can
therefore be that simple. Expiry is the store's to set: the file store uses the policy TTL
(`expires_in(remaining)`), and nothing today lets a source ask for a shorter one. The spec adds
an optional per-attribute `expires_at`, clamped to the policy TTL — policy shortens, never
extends (SECURITY_MODEL.md).

### Finding 4 — the legacy network slot is not reusable

`validation/1` and `validation/2` (`zl-zpr-compiler/src/zpl.rs`) are the BAS-era network API:
`provider` tuples, `client`/`service` fabric names, `cert_path`. The visa service factory
rejects both, `zpr-bas` is deprecated and excluded from the workspace (REPOSITORIES.md), and
the shape (a ZPR-internal service reached over the fabric with HMAC-signed calls) is the wrong
one for a hosted database. A new `api` value avoids inheriting semantics nothing implements.

### Finding 5 — zipline has no code yet

`~/src/zipline` is documentation only (`README.md`: "No implementation code yet"). There is no
database schema to fit; the API defined here *is* the contract zipline builds to. That is the
right order — zipline should not shape the visa service's notion of an attribute — and it is
why the reference server (V3) exists: a second implementation, however small, keeps the spec
honest before the first real one lands.

---

## Decisions

Taken 2026-09-22 with the operator, recorded here so no issue re-opens them.

| Question | Decision | Why |
|---|---|---|
| Wire protocol | HTTPS + JSON | Anyone can implement it; `reqwest` and `axum` are already in `vs`; the admin API is already HTTPS/JSON. Cap'n Proto RPC and gRPC rejected as hostile to third-party implementers. |
| Caller authentication and reach | TLS (optional CA pin in policy) + bearer token from `vs.toml`; direct URL only | Secret never in the signed, distributed policy. HMAC-per-call (RFC-13.1) rejected: no gain over TLS + bearer here, and canonicalisation is a trap for implementers. On-net reach via `service` reserved. |
| v1 scope | QUERY + SCHEMA + change webhook | Rides Finding 2. True STREAM (SSE) and SATISFIES documented as reserved endpoints. |
| `api` value | `zpr-attr/1` | Names the contract and its version; leaves room for `zpr-attr/2`. |
| Identity keys on the wire | ZPR key names (`user.sub`, `device.zpr.adapter.cn`, `user.zpr.authority`) | Mirrors the `file` store JSON; no reverse mapping; the authority marker has no service-side name. |
| SCHEMA use by the visa service | Fetch once at store build, `warn` on mismatch, never fail | Catches typos and the `user.email` keying mistake; keeps policy authoritative and installs independent of the service being up. |
| SCHEMA format | SCIM 2.0 attribute definitions (RFC 7643 §7) inside a small envelope carrying `identityKeys` | Directories already speak SCIM, so a SCIM-backed service copies its `Schema.attributes` through; `canonicalValues` gives editors allowed-value lists for free. JSON Schema rejected for the schema itself: it describes JSON shapes, and single/multi/tag all travel as `values: [...]`. Reserved as a later per-value constraint. |
| Machine-readable API description | `docs/zpr-attr-v1.openapi.yaml`, non-normative | Implementers (zipline first) get a document tools can consume; V3's contract tests hold the reference server to it. The spec stays normative so two sources cannot silently diverge. |
| Where the contract lives | `docs/ATTRIBUTE_SERVICE.md`; this plan sequences | A completed plan is a frozen historical record; the contract must stay current. |

---

## Global constraints

- **No change to evaluation semantics.** `libeval` is untouched; the new store produces
  ordinary `Attribute`s with a source and an expiry, and everything downstream already handles
  them.
- **No new `.zpl` grammar.** Only the `.zplc` configuration learns a new `api` value.
- **A `policy.capnp` change is unavoidable** and therefore a `zl-zpr-policy` tag, a
  `zl-zpr-common` tag, and a pin bump in both consumers. This is the plan's one hard
  cross-repository coupling; it is confined to P1 → C1 and everything downstream reads the new
  tag. Do not add a second schema change anywhere in this plan.
- **Version floors after this work:** `zl-zpr-policy` `v0.12.0`; `zl-zpr-common` `v0.29.0`;
  `zl-zpr-compiler` `0.19.0`; `zl-zpr-visaservice` `0.20.0` with
  `POLICY_MIN_COMPILER_MINOR = 19`. The visa service's version check is near-exact on minor
  (BUILD.md, *Versions and tags*), so the compiler and visa service bumps land together in a
  build set (D1).
- **Fail closed on every new error path.** Transport error, bad status, malformed body, type
  disagreement, `409` — all yield `Err` from the store, which the existing refresh path turns
  into an indeterminate actor and a denied visa. Every one of these is a test.
- **The secret is never in policy, never logged.** Attribute values never above `debug`.
- **Build gate on every PR:** `make check` and `make test`; warnings are errors. Every
  non-trivial function carries a doc comment (`AGENTS.md`). A found bug gets a failing test
  before the fix.
- **Fixture naming in `zl-zpr-compiler/test-data`:** `test-*.zpl` must compile and `zpdump`;
  deliberately failing fixtures are `bad-*.zpl`.
- **Never edit or commit a repo's generated `AGENTS.md`/`CLAUDE.md`.** Never let a `Cargo.lock`
  with a local path reach a PR. During C1 → K1/V1 development use the `[patch]`-free worktree
  procedure in BUILD.md.
- **Each issue names the repositories it touches and touches nothing else.** The zipline
  product's server-side implementation is tracked in the zipline repository against the spec;
  it is a consumer of this plan, not an issue in it.

---

## Cross-repository interface contracts

These are fixed here so the issues can proceed independently once their blockers merge.

### 1. `AttrQueryConfig` in `policy.capnp` (`zl-zpr-policy`, P1)

```capnp
struct TrustedService {
  serviceId         @0 :Text;
  expirationSeconds @1 :UInt32;
  returnsAttrs      @2 :List(AttrMapping);
  identityAttrs     @3 :List(Text);
  oidc              @4 :OidcConfig;
  attrQuery         @5 :AttrQueryConfig;   # populated only when Service.kind.trusted == "zpr-attr/1"
}

# Configuration for an `api = "zpr-attr/1"` attribute service. The bearer token the
# visa service presents is deliberately NOT here: policy is signed and distributed.
struct AttrQueryConfig {
  url            @0 :Text;    # https base URL; the visa service appends /query and /schema
  caCertPem      @1 :Text;    # PEM certificate block(s) to trust for this service; "" = system roots
  timeoutSeconds @2 :UInt32;  # whole-request timeout; compiler defaults 5, caps 30
}
```

### 2. Rust mirror (`zl-zpr-common`, C1)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttrQueryConfig {
    pub url: String,
    pub ca_cert_pem: Option<String>,   // "" on the wire == None
    pub timeout_seconds: u32,
}

pub struct TrustedService {
    // ... existing ...
    /// Populated only for `api = "zpr-attr/1"` trusted services.
    pub attr_query: Option<AttrQueryConfig>,
}
```

Pointer-absent decodes to `None`; `None` encodes as an unset pointer (`has_attr_query() ==
false`). Same as `oidc`.

### 3. The `zpr-attr/1` wire API (K1 emits its config; V1 speaks it; V3 serves it)

By reference: `docs/ATTRIBUTE_SERVICE.md`, *The wire protocol*. The `.zplc` surface, the
request and response shapes, the status-code table, and the read-order rules there are the
contract. An issue that needs to change any of them stops and amends the spec first.

### 4. The `changed` admin endpoint and the `notify` permission (`zl-zpr-visaservice`, V2)

```
POST /admin/services/{id}/changed        X-API-Key: <notify | readwrite>
  body {}                                 -> flush_one(id) + TrustedServiceChange
  body {"identities":[{"<zpr key>":"<value>"}, ...]}
                                          -> forget_source_revision(actor, id) for every
                                             connected actor carrying a listed pair,
                                             then TrustedServiceChange
  202 | 400 | 403 | 404
```

`Permission::Notify` is a fourth `admin_apikeys::Permission` value, serialised `notify`;
`can_notify()` is true for `Notify` and `ReadWrite`; `can_resolve`/`can_read`/`can_write`
are false for it. `vsapikey` accepts `notify`.

### 5. `vs.toml` `[core] ts_secrets_dir` (`zl-zpr-visaservice`, V1)

Default `"."`, rebased against the config file's directory like `file_ts_dir`. The store reads
`<ts_secrets_dir>/<service-id>.token`, trimmed. Missing or empty → `TrustedServiceInit`.

---

## Dependency graph and order

```
S1 (spec + this plan)
 └─> P1 (policy.capnp) ─> C1 (common mirror, tag)
                            ├─> K1 (compiler)  ──────────┐
                            └─> V1 (vs store) ─> V2 (vs changed endpoint) ─> V3 (reference server + contract tests)
                                                                              │
                                             K1 + V3 ─────────────────────────┴─> E1 (core: netns end-to-end) ─> D1 (docs status, build set)
```

`K1` and `V1` touch disjoint repositories and share only contract 2, which `C1` has fixed by
the time either starts, so they may run in parallel. `V2` follows `V1` because it shares the
manager and factory files. `E1` needs a compiler that emits the config and a visa service that
consumes it.

## Issue map

Filed in `mkolehmainen/zipline`; each names its fork. The umbrella carries these as sub-issues
in this order.

| ID | Repo | Title | Blocked by |
|---|---|---|---|
| S1 | zl-zpr-dev-context | `docs/ATTRIBUTE_SERVICE.md` and this master plan | — |
| P1 | zl-zpr-policy | `AttrQueryConfig` and `TrustedService.attrQuery @5` | S1 |
| C1 | zl-zpr-common | Mirror `AttrQueryConfig`; round-trip tests; bump the `zpr-policy` submodule; tag `v0.29.0` | P1 |
| K1 | zl-zpr-compiler | Parse `api = "zpr-attr/1"`, emit `AttrQueryConfig`, fixtures, version `0.19.0` | C1 |
| V1 | zl-zpr-visaservice | `AttrQueryStore`: query, schema check, `ts_secrets_dir`, factory arm, min-compiler `0.19.0` | C1 |
| V2 | zl-zpr-visaservice | `POST /admin/services/{id}/changed` and `Permission::Notify` | V1 |
| V3 | zl-zpr-visaservice | `zpr-attr-server` reference implementation and protocol contract tests | V2 |
| E1 | zl-zpr-core | netns end-to-end test: OIDC login + `zpr-attr/1` decoration + `changed` revocation | K1, V3 |
| D1 | zl-zpr-dev-context | Docs status updates; committed build set; plan COMPLETE | E1 |

---

## Phase S — Specification (`zl-zpr-dev-context`)

### Task S1: `docs/ATTRIBUTE_SERVICE.md` and this plan

**Scope.** The durable contract as a `docs/` specification with an `## Implementation status`
section, this plan in the house format, and a required-reading row in `AGENTS.md`.

- [x] Write `docs/ATTRIBUTE_SERVICE.md`.
- [x] Rewrite this document from the draft into the master-plan format.
- [x] Add the `AGENTS.md` required-reading row.
- [x] Write `docs/zpr-attr-v1.openapi.yaml` from the spec's wire-protocol section.
- [ ] File the umbrella and S1–D1 in `mkolehmainen/zipline`, wire `blockedBy`, run
      `scripts/board-sync.py --apply`, and record the issue numbers in the *Issue map* and the
      *Status* line.

**Acceptance.** `zpr-dev sync` regenerates context cleanly; every code location the spec cites
exists at the cited path; the issue map and dependency graph agree.

---

## Phase P/C — Schema and mirror

### Task P1: `AttrQueryConfig` in `policy.capnp` (`zl-zpr-policy`)

- [ ] Add `AttrQueryConfig` and `TrustedService.attrQuery @5` exactly as in contract 1, with
      the comments shown.
- [ ] Tag `v0.12.0` after merge.

**Acceptance.** `capnp compile` succeeds; `zl-zpr-common`'s build against the new submodule
commit compiles unchanged (the field is additive).

**Trade-off to state in the PR.** Cap'n Proto is forward compatible, so an older visa service
reading a policy with `attrQuery` set simply ignores the field — and then rejects the service
as an unsupported `api`, which is the correct fail-closed behaviour. The coordinated version
bump is forced by the compiler-version check, not by the schema.

### Task C1: Mirror and tag (`zl-zpr-common`)

- [ ] Bump the `zpr-policy` submodule to P1's commit.
- [ ] Add `AttrQueryConfig` and `TrustedService.attr_query` to
      `src/policy_types/trusted_service.rs` as in contract 2: `TryFrom<Reader>` and
      `WriteTo<Builder>`, `""` ↔ `None` for `ca_cert_pem`, pointer-absent ↔ `None` for the
      whole struct. Follow the `oidc` arms line for line.
- [ ] Tests mirroring the OIDC ones: full round-trip, `None` round-trip
      (`!reader.has_attr_query()`), empty-string decode to `None`.
- [ ] Update every `TrustedService { ..., oidc: None }` literal in the crate's tests to carry
      `attr_query: None`.
- [ ] Tag `v0.29.0` after merge.

**Acceptance.** `make` with `-F all`, `make test`, and the existing OIDC round-trip tests
unchanged and passing.

---

## Phase K — Compiler (`zl-zpr-compiler`)

### Task K1: Parse and emit `api = "zpr-attr/1"`

**Scope.** The `.zplc` surface in the spec's *Declaring an attribute service in policy*, the
weaver treating the new kind like `file`, and the config reaching the binary policy.

- [ ] Pin `zpr = { ..., tag = "v0.29.0" }`.
- [ ] `src/zpl.rs`: `pub const TS_API_ATTR_QUERY: &str = "zpr-attr/1";`.
- [ ] `src/config/mod.rs`: `TrustedService.attr_query: Option<AttrQueryTsConfig>` alongside
      `oidc`, and `parse_attribute_tuples`-style plumbing into the record the weaver writes.
- [ ] `src/config/trusted_service.rs`: `parse_attr_query_trusted_service`, modelled on
      `parse_file_trusted_service` + `parse_oidc_trusted_service`:
  - `url` required, `is_https_url_with_host`, no `?`/`#`; trailing `/` stripped.
  - `ca_cert_path` optional; read the file relative to the `.zplc`; require at least one
    `-----BEGIN CERTIFICATE-----` block; embed the contents.
  - `timeout_seconds` optional, integer `1..=30`, default `5`.
  - `expiration_seconds` required and `> 0` (the visa service enforces the 60 s floor).
  - `returns_attributes` required, ≥ 1, through `parse_return_mappings` so the reserved
    `zpr.` namespace check applies.
  - Reject `identity_attributes`, `provider`, `client`, `cert_path`, `prefix` — one
    diagnostic each, naming the property and the api.
  - Reject `service` with the reserved-property message from the spec.
  - Guard the builtin `default` id, as `file` and `oidc` do.
  - Teach `warn_unknown_ts_property` the three new names for this api only.
- [ ] Weaver: confirm `resolve_trusted_service_providers` skips the new kind as it skips
      `file` and `oidc` (no provider to widen through), and that with no
      `identity_attributes` the identity-vendor retention rule never fires. Add the
      negative test: an unreferenced `zpr-attr/1` service is pruned.
- [ ] Emit: `policybinaryv2.rs` writes `attr_query` through the common mirror. `zpdump` prints
      it (url, `caCertPem` as `<N bytes>`, timeout — never the PEM body).
- [ ] Fixtures: `test-data/test-attr-query.zpl/.zplc` (an `oidc` + `zpr-attr/1` pair on the
      oidc-file-interplay pattern), and `bad-attr-query-http-url.zpl`,
      `bad-attr-query-identity-attrs.zpl`, `bad-attr-query-service-reserved.zpl`, each with
      the expected diagnostic asserted.
- [ ] Version `0.19.0`.

**Acceptance.** All new fixtures behave; `zpdump` shows the record; `make check`,
`make test` clean; `test-oidc-file-interplay` unchanged.

---

## Phase V — Visa service (`zl-zpr-visaservice`)

### Task V1: `AttrQueryStore`

**Scope.** The spec's *The visa service side*: the store, the secrets directory, the factory
arm, the install-time schema check. Nothing in `connection_control.rs`,
`actor_attributes.rs` or `libeval` changes.

- [ ] Pin `zpr` `v0.29.0`; `POLICY_MIN_COMPILER_MINOR = 19`; workspace version `0.20.0`.
- [ ] `config.rs`: `ts_secrets_dir: Option<PathBuf>` (default `"."`), rebased like
      `file_ts_dir`; tests for the relative and absolute cases beside the existing ones.
- [ ] `trusted_services/attr_query_store.rs` — `AttrQueryStore`, `TrustedServiceInterface`:
  - Construction: read `<id>.token` (trimmed; missing/empty → `TrustedServiceInit`); build
    the `reqwest::Client` (no redirects, `add_root_certificate` for each PEM block, timeout).
  - `get_attributes_for_actor`: `POST {url}/query` with the request body from the spec; read
    the body under a 1 MiB cap (factor `MAX_JWKS_BYTES` and the chunked reader out of
    `oidc/jwks.rs` into a shared helper rather than copying them); apply the five read-order
    rules from the spec; stamp source and clamped expiry.
  - `flush`, `current_revision`, `get_source_id` as in the OIDC store.
- [ ] Schema check at build: `GET {url}/schema`; parse the SCIM attribute definitions
      (unknown SCIM fields ignored); warn per missing name, per spelling/definition mismatch
      under the spec's correspondence table (including `complex`), and when `identityKeys`
      is disjoint from `policy.lookup_identity_keys()`; `info` on any failure. Runs once,
      inside `build_services`, never fails the install.
- [ ] `factory.rs`: `TS_API_ATTR_QUERY`; accept it in `trusted_service_definitions` (require
      `attr_query`, reject a missing record); build arm passing `ts_secrets_dir` and the
      policy.
- [ ] Tests, using an in-process TLS mock on the `spawn_tls_jwks_server` pattern (a tiny
      `axum` router the test controls):
  - happy path: all three types, `expires_at` clamped both ways, unmapped names dropped;
  - unknown actor → `Ok(empty)`;
  - each failure: non-200 (incl. `404`, `409`, `500`), timeout, oversized body, malformed
    body, single-valued with two values, `https` pin mismatch → `Err`;
  - empty `values` on a non-tag → attribute absent;
  - missing token file → install fails; bearer header present on the wire;
  - schema check: each warning fires; endpoint absent → install succeeds;
  - through `TrustedServicesMgr` + `refresh_expired_attributes`: an `Err` makes the actor
    indeterminate and pruned; a revision bump re-queries.

**Acceptance.** `make check`, `make test`; a policy from K1's `test-attr-query.zplc`
installs against the mock and an actor keyed on `user.sub` receives the mapped attributes.

### Task V2: `POST /admin/services/{id}/changed` and `Permission::Notify`

- [ ] `admin_apikeys.rs`: `Permission::Notify` (`"notify"`), `can_notify()`; `vsapikey`
      accepts it; existing `can_*` predicates return `false` for it (tests).
- [ ] `manager.rs`: `forget_source_revision(&self, zpr_addr, source)`; test that it makes
      exactly that source stale for exactly that actor.
- [ ] `admin_service.rs`: the route per contract 4. Body `{}` → `flush_one` + event (share
      the body of `flush_service_cache`). Body with `identities` → for each connected actor
      (`actor_mgr.list_actors`) carrying any listed pair, `forget_source_revision`; then
      event. Return `202`; `400` on a malformed body; `404` when `{id}` is not a trusted
      service; `403` without `can_notify`.
- [ ] `admin-http-api.txt`: document the endpoint and the `notify` level; note the
      `DELETE .../cache` equivalence.
- [ ] Tests on the admin test assembly: permission matrix; targeted body leaves an unlisted
      actor's revision intact; end-to-end through `handle_trusted_service_change` a listed
      actor is re-queried and an unlisted one is not.

**Acceptance.** `make check`, `make test`; `admin-http-api.txt` updated.

### Task V3: `zpr-attr-server` and protocol contract tests

**Scope.** A second implementation of the server side, small enough to read in one sitting,
that serves a `file`-store JSON over `zpr-attr/1`. It is the fixture for E1, the executable
example for zipline's implementers, and the thing that keeps the spec honest.

- [ ] New workspace crate `zpr-attr-server` (binary; `axum`, `serde_json`, `tokio`,
      `rustls` via the workspace — no new dependency trees). Flags: `--listen`, `--cert`,
      `--key`, `--token-file`, `--data <file.json>`, `--vs-url` + `--vs-api-key` (optional,
      for `--notify` mode below).
  - `POST /query`: look up every identity pair in the JSON (same union-and-conflict rule as
    `FileAttributeStore`; conflict → `409`); honour the `attributes` hint; never emit
    `expires_at` unless the JSON entry carries one.
  - `GET /schema`: SCIM attribute definitions from a `_schema` key in the JSON, or a
      definition list derived from the data (`string`, `multiValued` when any entry has more
      than one value) if absent; `identityKeys` from the JSON's top-level keys.
  - Bearer check → `401`; anything else → `400`/`500` per the spec table.
  - `--notify [identity=value ...]`: a subcommand that posts `changed` to a visa service —
    the operator's and E1's way to simulate an attribute change.
- [ ] Contract tests (`zpr-attr-server/tests/`): drive the router in-process with every
      request/response example in the spec, byte-for-byte on the JSON shapes, and validate
      each exchange against `docs/zpr-attr-v1.openapi.yaml` (vendor the file into the crate's
      `tests/` with its source path in a comment; no network); then point V1's
      `AttrQueryStore` at the in-process server and assert the mapped attributes.
- [ ] Not staged into `make release` or the build set: a development and test tool. Say so in
      its `README.md`, with the spec as the only normative reference.

**Acceptance.** `make check`, `make test`; a reader can implement the protocol from the spec
plus this crate without reading `vs`.

---

## Phase E — End to end (`zl-zpr-core`)

### Task E1: netns integration test

**Scope.** The proof the pieces fit, on the harness the oidc-file-interplay work built
(`integration-test/oidc-file-interplay-test.sh`, `pregen/oidc-file-interplay.zplc`).

- [ ] `integration-test/pregen/attr-query.zpl/.zplc`: the interplay fixture with the `file`
      store replaced by a `zpr-attr/1` service pointing at `zpr-attr-server` on the loopback,
      CA-pinned to a test certificate.
- [ ] `integration-test/attr-query-test.sh`: start `zpr-attr-server` with a `user.sub`-keyed
      JSON; connect with the fake IdP token; assert the actor carries the mapped attributes
      and `user.zpr.authority = google`; issue a visa on `allow contractors to access Web`;
      edit the JSON to drop `contractor`; `zpr-attr-server --notify user.sub=...`; assert
      the visa is revoked and a re-request is denied. Then the untargeted `{}` body once.
- [ ] Wire it into the netns tier alongside the interplay test.

**Acceptance.** Passes under `zpr-dev build --tier netns`; the revocation assertion fails if
V2's event is not queued (verified by running once with the notify step removed).

---

## Phase D — Documentation and closure (`zl-zpr-dev-context`)

### Task D1: status updates and build set

- [ ] `docs/ATTRIBUTE_SERVICE.md` `## Implementation status`: what landed, with issue links,
      and the version floors.
- [ ] `docs/VISA_SERVICE.md`: the *Trusted-service attribute stores are file-backed only*
      divergence bullet becomes history; add `zpr-attr/1` to *Inside `vs`* and to the
      configuration list (`ts_secrets_dir`); admin API paragraph mentions `changed` and the
      `notify` key level.
- [ ] `docs/SECURITY_MODEL.md`: *Networked attribute sources* under implementation status is
      no longer true — say what is; add the RFC-13.1 HMAC departure note by reference.
- [ ] `docs/ZPL.md`: the `[trusted_services.<NAME>]` table row lists `zpr-attr/1`.
- [ ] `build-sets/<date>.yaml`: a set at the post-plan floors, built and gated with
      `zpr-dev build`.
- [ ] This document: `**Status:** COMPLETE (<date>)`, issue links, the V-outcomes recorded.

---

## Out of scope

- **A true STREAM** (`GET {url}/stream`, server-sent events) and **SATISFIES**. Both
  reserved in the spec with a paragraph on intent; neither has a consumer yet.
- **Batch multi-actor query.** The visa service refreshes one actor at a time.
- **On-net reach** through a ZPR `service`. Reserved property; rejected in `zpr-attr/1`.
- **Zipline's server implementation.** Tracked in the zipline repository against the spec.
- **Directory adapters** (LDAP, AD, Okta). Each is a server-side implementation of the same
  protocol; none is a visa service change.
- **Refactoring bootstrap or the OIDC store onto a different abstraction.** Finding 1: the
  trait is right as it is.
- **Fixing the `file` store's first-value leniency** on a single-valued attribute with several
  values. Noted in the spec as a divergence; a separate small issue if wanted.
- **Rate limiting `changed`.** Same cost as an administrator's cache flush; revisit if a
  notifier misbehaves.
