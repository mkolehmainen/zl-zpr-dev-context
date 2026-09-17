# Machine Hostnames in DNS — `device.hostname` over the visa service admin API

**Status:** Draft
**Date:** 2026-09-17
**Repo state this plan was written against:** `zl-zpr-visaservice` @ `8cf3ab5` (`zipline`),
`zl-zpr-coredns` @ `f4202c4` (`main`), `zl-zpr-demo` @ `8ef671f` (`zipline`),
`zl-zpr-dev-context` @ `b6a1be0`, `zl-zpr-common` tag `v0.26.0`.

> Process note: per `skills/zpr/SKILL.md`, each task below becomes one GitHub issue and is
> worked on a feature branch off `zipline`, PR targeting `zipline`. Read that skill before
> filing. The *Issue map* and dependency graph here are documentation derived from the
> issues' `Blocked by:` lines — if they disagree, the issues win and this prose needs fixing.

**Goal.** A user who adds a machine to a zipline-managed ZPRnet and names it `somename` can
`ping6 somename.<zone>` and `ssh somename.<zone>` from inside the ZPRnet. The name is
assigned by the network's naming authority, delivered to the visa service as an
authenticated attribute, and resolved by the same CoreDNS `zpr` plugin that already
resolves ZPL service names.

**Why this is not the existing service lookup.** DNS today answers only for ZPL-declared
services with a live provider (`2026-09-15-dns-integration.md`). A machine is not a
service: it may provide several (ssh, ping6) or none, and naming it after one of them is
unnatural — nobody pings a service name. See **Resolved while planning** for why the
X.509 CN is not the answer either.

**Architecture.**

```
  naming authority (zipline control plane / attrfile)
        │  trusted service: returns  hostnames -> device.hostname{}
        ▼
  Visa Service ── authenticated claim ──▶ Actor ──▶  host:<NAME> |- zpr_addr
        │                                            actor:<ZADDR>:hostnames -> SET[name]
        │  GET /admin/hosts/{name}   (resolve key)
        ▼
  CoreDNS + zpr plugin ──▶ AAAA
        │
   somename.demo. AAAA?  ──▶ /admin/services/somename → 404
                          ──▶ /admin/hosts/somename   → { zpr_addr: "fd5a:5052:8888::a1" }
   ◀── AAAA fd5a:5052:8888::a1 TTL 30
```

Three repositories change, plus operator-side configuration and one fixture:

| Part | Where | What changes |
|---|---|---|
| Visa service | `zl-zpr-visaservice` | `device.hostname` claimed into a `host:<NAME>` index at actor write time; per-value first-claim-wins; `GET /admin/hosts/{name}` under the existing `resolve` permission; conflicts reported on `GET /admin/actors/{addr}`. **No new permission.** |
| CoreDNS plugin | `zl-zpr-coredns` | One extra lookup in `ServeDNS`: service first, then host. No Corefile change. |
| Naming authority | operator-side | Returns `hostnames -> device.hostname{}` from a trusted service. In the demo this is the `file` api and `attrfile.json`. |
| End-to-end demo | `zl-zpr-demo/dns-demo` | Named machines in `attrfile.json`, an ICMP6 `ping` service so `ping6` is permitted, `iputils-ping` in the image, and new cases in the existing `test-dns.sh`. |

**Name semantics (normative).**

- `device.hostname` is multi-valued (`{}`). Every value is a name for the same actor:
  aliases are the mechanism, not a special case.
- Each value is claimed **independently**. Losing one value never costs the others.
- A value must be a single valid lowercase DNS label: `[a-z0-9]([a-z0-9-]*[a-z0-9])?`,
  1–63 bytes. An invalid value is rejected and logged; it is **never** transformed into a
  valid one (see **Resolved while planning**: no mangling).
- Names live in **one flat namespace with service names**: `somename.<zone>` and
  `web.<zone>` are both one label under the zone. The plugin tries the service lookup
  first, then the host lookup.
- Ambiguity is prevented at claim time, not at resolution time: a host claim whose value
  equals a service name declared in the current policy is rejected. Policy is the
  operator's declared intent and wins.
- First claim wins. A second actor claiming a live name is rejected, logged at `error!`,
  counted, and reported on the admin API. The name keeps pointing at the original actor.
- A claim already held by the claiming actor is not a collision: the test is
  "unset, or already mine", so re-authentication and attribute refresh are idempotent.
- **A hostname names, it does not authorize.** Resolving `somename.<zone>` says where the
  machine is; whether the caller may reach it is still decided by policy and the visa, on
  the protocol and port being used. So reaching a named machine at all requires a service
  covering that traffic — including ICMP6 for `ping6`, which H4 Step 2 has to declare. The
  hostname index is never consulted by the enforcement path.
- An index entry's lifetime is the **actor record's**, not the attribute's. Trusted-service
  attributes expire (`expiration_seconds`); a machine's name must not vanish mid-session
  because an attribute cache lapsed.

---

## Global constraints

- **No policy schema change.** `zl-zpr-policy`'s `policy.capnp`, the `zpr` crate and the
  compiler are untouched: `device.hostname` is an ordinary non-reserved attribute that
  `returns_attributes` already accepts. No `zpr-common` bump. If a task appears to need
  one, stop and revisit.
- **No new API-key permission.** `GET /admin/hosts/{name}` is gated by the existing
  `can_resolve()` (`vs/src/admin_apikeys.rs`), alongside the two service endpoints. The
  resolver's key gains no new reach: one name in, one address out.
- **The name is never self-asserted.** It must arrive as an *authenticated* claim from a
  trusted service. An adapter's own unauthenticated claims are committed to the actor
  whenever a join policy matches (`libeval/src/eval.rs:264-282`), so a self-asserted
  hostname would let any machine name itself anything — including another machine's name.
  No task may accept `device.hostname` from the unauthenticated claim set.
- **Never silently rename.** Rejected claims are visible to the operator, not repaired by
  the VS. Reachability is the naming authority's job (it returns a unique id alongside the
  friendly name), not the resolver's.
- **The `resolve` key must not gain actor visibility.** `GET /admin/hosts/{name}` returns
  one record for one requested name. No listing endpoint is added for `resolve`; a
  `read`-gated `GET /admin/hosts` is optional and separate.
- **Build gates.** Rust: `cargo build`, `cargo fmt -- --check`, `cargo test`, warnings are
  errors; every non-trivial function has a doc comment; a found bug gets a failing test
  first. Go: `go vet`, `go test ./...`, `gofmt -l` empty.
- **Branches and tags.** Feature branches from `zipline`, PRs target `zipline`; tags carry
  a `zl-` prefix; never edit a repo's generated `AGENTS.md`/`CLAUDE.md`; never let a
  `Cargo.lock` with a local path reach a PR (`docs/REPOSITORIES.md`).

---

## Cross-repository interface contracts

### 1. The attribute (new; operator-side configuration)

```toml
[trusted_services.machines]
api = "file"                                    # or the zipline control plane's api
returns_attributes = ["hostnames -> device.hostname{}"]
expiration_seconds = 3600
```

`device.hostname` sits outside the ZPR-reserved `zpr.` sub-namespace of the `device.` class
domain, so `parse_return_mappings` accepts it from a non-default trusted service
(`zl-zpr-compiler/src/config/trusted_service.rs:139`). `device.zpr.hostname` and any other
`device.zpr.*` spelling would be **rejected at compile time** and is not an option.

Keyed on the actor's lookup identity, exactly as `dns-demo/zpr-conf/admin/attrfile.json`
already does for `alice`:

```json
{ "device.zpr.adapter.cn": { "web.demo": { "hostnames": ["webhost", "m-7f3a2b"] } } }
```

The key is the machine's CN — its cryptographic identity, `web.demo` here — and the values
are the names it answers to. The two are deliberately unrelated strings.

The naming authority is expected to return one value that is unique by construction (its
own machine id) alongside any friendly aliases, so a lost alias never costs reachability.
The VS does not require this and does not check for it.

### 2. Database layout (new; `zl-zpr-visaservice/vs/src/db/actor.rs`)

```
host:<NAME>                 |- zpr_addr -> string
actor:<ZADDR>:hostnames     -> SET[ <name> ]
actor:<ZADDR>               |- hostname_conflicts -> JSON [ <name> ]
```

`hostname_conflicts` is a field on the existing actor hash (`actor_key_for`,
`vs/src/db/actor.rs:536`), rewritten on every claim attempt so it always describes the
latest one rather than accumulating. It is display data for H2, never an input to a
decision.

`<NAME>` is percent-encoded through `KeyString` exactly like `service_key_for`
(`vs/src/db/actor.rs:548`, `vs/src/db/mod.rs:161`) — a no-op for any name that passed label
validation, and kept anyway so the key layer never depends on the validator. Mirrors
`KEY_SERVICE` /
`actor_services_key_for` (`:29`, `:554`); add `KEY_HOST` and `host_key_for` /
`actor_hostnames_key_for` beside them.

### 3. `Db` trait (changed; `zl-zpr-visaservice/vs/src/db/mod.rs:61`)

```rust
/// Sets `field` only if it is absent. Returns true when this call set it —
/// the atomic claim primitive for a uniquely-owned name.
async fn hset_nx(&self, key: &str, field: &str, value: &str) -> DbResult<bool>;
```

Today it returns `DbResult<()>` and discards the result in both implementations
(`vs/src/db/db_redis.rs:132-136`, `vs/src/db/db_fake.rs:414-424`), so there is no way to
learn whether a claim succeeded. Returning `bool` makes first-claim-wins atomic and removes
any read-then-write race. It has two existing callers, both
`hset_nx(&base_key, "ctime", ...)` (`actor.rs:206` and `:392`), and both are indifferent to
the result; `let _ =` keeps them compiling.

### 4. Admin API (new; `zl-zpr-visaservice/admin-http-api.txt`)

```
GET https://[fd5a:5052::1]:8182/admin/hosts/{name}      X-API-Key: <resolve key>
  200  HostDescriptor { hostname, zpr_addr, actor_cn }
  401  key missing/malformed/unknown
  403  key lacks resolve permission
  404  no such hostname
  500  server error
```

```rust
// zl-zpr-visaservice/admin-api-types/src/admin_api_types.rs
pub struct HostDescriptor {
    pub hostname: String,
    pub zpr_addr: String,
    /// Display label of the owning actor; may be empty.
    pub actor_cn: String,
}
```

`ActorDescriptor` (`admin_api_types.rs:97`) gains one field, so an operator can see why a
machine is not resolvable under the name they gave it:

```rust
    /// Hostname values this actor claimed that were refused because another actor
    /// or a policy service already held them. Empty in the normal case.
    pub hostname_conflicts: Vec<String>,
```

`resolve` keys see only `/admin/hosts/{name}`. `ActorDescriptor` stays `read`-gated.

### 5. Plugin resolution order (changed; `zl-zpr-coredns/plugin/zpr/zpr.go`)

For exactly one label under the zone:

| service lookup | host lookup | answer |
|---|---|---|
| found | (not attempted) | AAAA `zpr_addr`, or NODATA for non-AAAA |
| 404 | found | AAAA `zpr_addr`, or NODATA for non-AAAA |
| 404 | 404 | NXDOMAIN + SOA |
| failure | (not attempted) | SERVFAIL |
| 404 | failure | SERVFAIL |

Service first, so a hostname can never shadow a service even if the VS's claim-time check
is somehow bypassed. Apex, multi-label, lowercasing, TTLs and the Corefile are unchanged.

---

## Dependency graph and order

```
H0 ──▶ S1 ──▶ H1 ──▶ H2 ──┐
                          ├──▶ H4
H3 ───────────────────────┘

H0  test gate: a device.* attribute from a declared trusted service reaches the actor
S1  fix try_update_actor's unconditional service-entry delete
H1  device.hostname index + per-value claim rule
H2  GET /admin/hosts/{name}, conflict reporting, admin API doc
H3  coredns: host lookup after the service lookup
H4  dns-demo: a named machine, end to end
```

H0 first: it retires this plan's one real unknown at near-zero cost. S1 before H1 because
both edit `try_update_actor` and H1 copies the pattern S1 corrects. H3 is independent from
the start — its contract is fixed in §5 above and its tests fake the admin API — so it runs
in parallel with H0/S1/H1/H2 and only meets them in H4.

---

## Issue map

| ID | Repo | Title | Blocked by |
|---|---|---|---|
| H0 | zl-zpr-visaservice | Test gate: a `file` trusted service's `device.*` attribute survives onto the actor record | — |
| S1 | zl-zpr-visaservice | Bug: `try_update_actor` deletes another actor's live `service:<name>` entry on re-auth | — |
| H1 | zl-zpr-visaservice | `device.hostname` claimed into a `host:<NAME>` index; per-value first-claim-wins, label validation, policy-service precedence | H0, S1 |
| H2 | zl-zpr-visaservice | `GET /admin/hosts/{name}` under `resolve`; `hostname_conflicts` on `ActorDescriptor`; admin API doc | H1 |
| H3 | zl-zpr-coredns | `zpr` plugin: host lookup after the service lookup per §5 | — |
| H4 | zl-zpr-demo | `dns-demo`: named machines in `attrfile.json`, an ICMP6 `ping` service, `ping6 webhost.demo` from the client, and the alias/collision/precedence/invalid-name controls in `test-dns.sh` | H2, H3 |

---

## Phase H0 — Test gate

### Task H0: prove a `device.*` trusted-service attribute reaches the actor

The whole design rests on one unproven step: that an attribute in the `device.` class
domain, vended by a declared (non-default) trusted service and keyed on
`device.zpr.adapter.cn`, is committed to the actor record. The compiler accepts the mapping
and the runtime path looks right, but every `returns_attributes` example in the repos vends
`user.*`. This is the same class of unknown as `2026-09-15-dns-integration.md`'s Finding 1,
and it is cheap to settle.

**Files:** `vs/src/connection_control.rs` tests (the `CapturingTrustedService` /
`FailingTrustedService` fixtures at `:1723` and `:1228` are the shape to copy).

- [ ] Step 1: A test-only trusted service vending `device.hostname` with values
  `["somename", "m-7f3a2b"]` for identity `("device.zpr.adapter.cn", "<cn>")`. Connect an
  adapter with that CN; assert the resulting actor carries `device.hostname` with both
  values, and that the attribute's source is the trusted service, not the peer.
- [ ] Step 2: Assert the negative: the same attribute presented by the *peer* as an
  unauthenticated claim does not become an authenticated identity attribute. This is the
  constraint every later task depends on, so it is pinned here.
- [ ] Step 3: A compiler-side test that `returns_attributes = ["h -> device.hostname{}"]`
  compiles and `["h -> device.zpr.hostname"]` is rejected — locking in why the attribute is
  spelled the way it is (`zl-zpr-compiler/src/config/trusted_service.rs:139`).
- [ ] Step 4: Build gate.

**Acceptance:** Steps 1–3 pass. If Step 1 fails — a `device.*` attribute from a declared
trusted service does *not* reach the actor — record it under **Findings** and stop: H1
onward need a different delivery mechanism (most likely policy-granted, this plan's
rejected approach B) and the plan needs revisiting before any of it is written.

---

## Phase S — Pre-existing bug

### Task S1: `try_update_actor` deletes another actor's live service entry

`clean_up` is careful: before deleting `service:<name>` it checks the entry still points at
the departing actor, with the comment *"The stale names may actually be valid names on new
actors. So we need to check the zaddr value before deleting."*
(`vs/src/db/actor.rs:92-102`). `try_update_actor` does no such check — it deletes every
`service:<name>` in the actor's own set unconditionally (`vs/src/db/actor.rs:215-219`).

So when two actors provide the same service name (which the DB permits: the second
provider's `hset` simply overwrites `service:<name>`), a re-authentication or attribute
refresh of the *first* actor deletes the *second* actor's live entry, and the service
becomes unresolvable although a provider is connected. The attribute-refresh path reaches
this on a timer (`vs/src/actor_attributes.rs:73` → `actor_mgr.rs:156` → `update_actor`), so
it does not need a reconnect to trigger.

This is the same failure mode zipline#31 (A2) fixed for the CN index. It is fixed here
rather than inside H1 because H1 copies this function's structure, and because the bug is
live today independent of hostnames.

**Files:** `vs/src/db/actor.rs:215-219`, plus tests.

- [ ] Step 1 (test first): two actors, both with service `web` in their `zpr.services`
  attribute; add A, add B (B now owns `service:web`), then `update_actor(A)`. Assert
  `get_zpr_addr_for_service("web")` still returns B's address. Fails before Step 2.
- [ ] Step 2: Give the delete loop the same owner check `clean_up` uses; factor the shared
  "release these names if and only if I hold them" logic into one function both call, so the
  two paths cannot drift again (DRY, and H1 reuses it for hostnames).
- [ ] Step 3: Assert the ordinary case still works: `update_actor(A)` where A holds
  `service:web` re-points nothing and leaves the entry intact; a service dropped from A's
  attribute is released.
- [ ] Step 4: Build gate.

**Acceptance:** Step 1's test fails on `zipline` and passes after Step 2; the existing
`actor.rs` and `admin_service.rs` suites stay green.

---

## Phase H — Visa service

### Task H1: the hostname index and its claim rule

**Files:**
- `vs/src/db/mod.rs:61` — `hset_nx` returns `bool` (contract 3); `vs/src/db/db_redis.rs:132`,
  `vs/src/db/db_fake.rs:414` — both implementations return whether they set.
- `vs/src/db/actor.rs:28-31` — `KEY_HOST`; `:536-560` — `host_key_for`,
  `actor_hostnames_key_for`.
- `vs/src/db/actor.rs:343` (`try_add_actor`) and `:133` (`update_actor`/`try_update_actor`)
  — claim on both paths, via one shared function. The refresh path
  (`actor_attributes.rs:73`) goes through `update_actor`, so a name can change mid-session
  and must be re-claimed and released there, not only on connect.
- `vs/src/db/actor.rs:61` (`clean_up`) and `:429` (`rm_actor_by_zpr_addr`) — release
  hostnames with S1's shared owner-checked helper.
- `vs/src/db/actor.rs` — `get_zpr_addr_for_hostname`, `list_hostnames_for_actor`, beside
  their service equivalents at `:274` and `:331`.
- `vs/src/counters.rs` — a `hostname_claim_rejected` counter.
- `libeval/src/attribute.rs` — **no** new `key::` constant: `device.hostname` is
  operator-namespace, not ZPR-owned, and naming it in `key::` would invite someone to add it
  to a reserved-namespace check.

**Interfaces — Produces (exact):** contracts 2 and 3 above.

- [ ] Step 1 (test first), in `vs/src/db/actor.rs` tests against `FakeDb`:
  - one actor, `device.hostname = ["somename","m-7f3a2b"]` → both names resolve to its
    address; `list_hostnames_for_actor` returns both.
  - second actor claims `somename` and `other` → `somename` still resolves to the first
    actor, `other` resolves to the second, and the rejection is counted. **Per-value**, not
    per-attribute.
  - `update_actor` on the holder → its claims survive (idempotent: "unset, or already mine").
  - holder disconnects → `somename` is free and the loser does *not* inherit it (no
    re-claim on release; the loser re-claims on its next refresh, which is a normal
    attribute-refresh cycle, not a special path).
  - non-holder disconnects → the holder's entry survives. This is the zipline#31 regression
    restated for hostnames, and it is the test S1's shared helper exists to keep passing.
  - invalid values (`Some.Name`, `x`*64, empty, `_x`, `-x`, `x-`) are rejected, logged, not
    transformed; valid siblings in the same attribute still claim.
  - a value equal to a service name in the current policy is rejected.
- [ ] Step 2: Implement contract 3 (`hset_nx` → `bool`) and the claim/release helper.
  Validation lives in one function with a doc comment stating the label grammar.
- [ ] Step 3: Wire the policy-service precedence check. The claim path needs the current
  policy's service names; pass them in rather than reaching for global state from the DB
  layer, so the DB layer stays testable against `FakeDb` alone.
- [ ] Step 4: Record rejected values on the actor record so H2 can report them without
  re-deriving anything.
- [ ] Step 5: Build gate.

**Acceptance:** every Step 1 assertion passes; the full `zl-zpr-visaservice` workspace suite
is green; no policy, compiler or `zpr-common` change in the diff.

### Task H2: admin API and operator visibility

**Files:**
- `admin-api-types/src/admin_api_types.rs:97` — `ActorDescriptor.hostname_conflicts`;
  new `HostDescriptor` beside `ServiceDescriptor` at `:144`.
- `vs/src/admin_service.rs:179-195` — route `/admin/hosts/{capture}`;
  `:861` (`get_service`) is the handler shape to copy.
- `vs/src/admin_service.rs:558` (`get_actor`) — populate `hostname_conflicts`.
- `admin-http-api.txt` — the new endpoint, which permissions accept it, and the
  first-claim-wins/no-mangling semantics. Bump the "Current as of" date.
- `vs-admin/src/vsclient.rs` + `vs-admin` CLI — a `hosts get <name>` subcommand, mirroring
  `services get`, so `commands/demo-vs-admin` can drive H4's checks.

**Interfaces — Produces (exact):** contract 4 above.

- [ ] Step 1 (test first): insert a `resolve` key with `ReloadableApiKeys::insert_for_test`
  (`admin_apikeys.rs:166`) following `test_flush_service_cache_read_key_forbidden`
  (`admin_service.rs:2767`). Assert `GET /admin/hosts/{name}` → 200 for a claimed name,
  404 for an unclaimed one, and that a `resolve` key still gets 403 on every `read`
  endpoint — the permission surface must not widen.
- [ ] Step 2: Assert `GET /admin/actors/{addr}` shows the rejected value in
  `hostname_conflicts` for the losing actor and an empty vector for the winner.
- [ ] Step 3: Implement the handler and the descriptor field.
- [ ] Step 4: `vs-admin hosts get`, and the `admin-http-api.txt` update.
- [ ] Step 5: Build gate.

**Acceptance:** Step 1–2 assertions pass; `vs-admin` against a `read` key behaves exactly as
before; a `resolve` key reaches exactly three endpoints.

---

## Phase H3 — CoreDNS plugin

### Task H3: host lookup after the service lookup

**Files:** `plugin/zpr/client.go` (a `lookupHost` beside `lookupService`, sharing the
request/status/decode helper — the two differ only in path), `plugin/zpr/zpr.go:64-76`
(the two-step lookup per contract 5), `plugin/zpr/zpr_test.go`, `README.md`.

**Produces:** the table in contract 5, observable through `dig`.

- [ ] Step 1 (test first): table cases against `net/http/httptest`, each asserting rcode,
  answer RRs, authority SOA, **and** which admin paths the fake saw:
  - `web.demo. AAAA`, services 200 → NOERROR + AAAA; the fake saw no `/admin/hosts/` call.
  - `somename.demo. AAAA`, services 404 + hosts 200 → NOERROR + AAAA.
  - `nope.demo. AAAA`, both 404 → NXDOMAIN + SOA with MINIMUM == `negative_ttl`.
  - `somename.demo. A`, services 404 + hosts 200 → NODATA + SOA.
  - services 500 → SERVFAIL, and the fake saw no hosts call (no fallback on failure: a
    service whose lookup failed must not be answered from the host namespace).
  - services 404 + hosts 500 / 401 / non-JSON / unparseable `zpr_addr` → SERVFAIL.
  - `a.b.demo. AAAA` → NXDOMAIN with zero HTTP calls (unchanged).
  - `SOMENAME.demo. AAAA` → hosts path is `/admin/hosts/somename`.
- [ ] Step 2: Refactor `lookupService` and the new `lookupHost` onto one shared
  request/decode function taking the path segment. `ServeDNS` stays under ~80 lines.
- [ ] Step 3: `Ready()` unchanged (`GET /admin/services` is still the readiness probe —
  hosts need no separate probe and adding one would double startup traffic).
- [ ] Step 4: README: contract 5's table and the one-flat-namespace rule, including that a
  service name wins.
- [ ] Step 5: `go vet`, `go test ./...`, `gofmt -l` empty, `make build && bin/coredns -plugins | grep dns.zpr`.

**Acceptance:** Step 1 green; no Corefile syntax change; a machine name resolves, an unknown
name is NXDOMAIN, a failed service lookup is SERVFAIL and never falls through to hosts.

---

## Phase H4 — Integration

### Task H4: a named machine in `dns-demo`, end to end

**Files:** `dns-demo/zpr-conf/admin/dns-demo.zpl` (declare the `machines` trusted service
reference so the store is woven — see the deviation note already recorded in that file for
`attrfile`; plus a `ping` service and its `Allow`), `dns-demo.zplc.template`
(`[trusted_services.machines]` per contract 1, and `[protocols.ping]` /
`[services.ping]` per Step 2), `zpr-conf/admin/attrfile.json` (hostnames for the `web.demo`
and `client.demo` CNs), `dns-demo/Dockerfile` (add `iputils-ping`: the image currently has
`curl`, `wget` and `dnsutils` but no `ping6`), `local-compute/test-dns.sh`, `README.md`.

**Extends, does not replace.** `local-compute/test-dns.sh` already exists (241 lines) with
`banner`/`ok`/`fail`/`finish`, a `cdig` helper that digs from the `client` container against
the resolver, `rcode`, `wait_rcode` for liveness deadlines, and `resolve_key_status` for
checking what the resolve key may reach. Every step below reuses those; no new harness.

**Produces:** `test-dns.sh` proves machine-name resolution, aliases, and all three negative
controls unattended.

- [ ] Step 1: Name the web machine `webhost` (plus its unique id) and the client `alicebox`.
  `commands/demo-vs-admin hosts get webhost` → `zpr_addr == "fd5a:5052:8888::80"`.
- [ ] Step 2: From `client`: `cdig AAAA webhost.demo` returns `fd5a:5052:8888::80`, and
  `curl http://webhost.demo/` succeeds through `/etc/resolv.conf` (the existing test's
  `web.demo` check, restated for a machine name).
  Then `ping6 -c1 webhost.demo`, which needs policy of its own — ICMP6 is expressible but
  the demo does not currently declare it:
  ```toml
  [protocols.ping]
  l4protocol = "ICMP6"
  icmp_type = "request-response"
  icmp_codes = [128, 129]
  ```
  with a `ping` service provided by `web.demo` and `Allow access:all users to access ping.`
  (shape from `zl-zpr-visaservice/integration-test/pregen/zpt-test.zplc:24-27` and
  `zpt-test.zpl:4,16`). This is the goal statement executed, and it is also the step that
  demonstrates a hostname is a name and not an authorization.
- [ ] Step 3: Alias check — the machine's unique-id value resolves to the same address as
  its friendly name.
- [ ] Step 4: Collision control: give `client.demo` the hostname `webhost` too, restart it,
  and assert (a) `hosts get webhost` still returns the web machine's address, (b)
  `demo-vs-admin actors get <client addr>` lists `webhost` under `hostname_conflicts`, (c)
  the VS log carries the rejection at `error!`.
- [ ] Step 5: Precedence control: give a machine the hostname `web` (a declared service) and
  assert the claim is rejected and `web.demo` still resolves to the service's provider.
- [ ] Step 6: Invalid-name control: a hostname of `Not_A_Label` is rejected and does not
  resolve in any transformed form — specifically, `not-a-label.demo` is NXDOMAIN.
- [ ] Step 7: `test-dns.sh` runs Steps 2–6 and exits non-zero on any failure, in the existing
  SUCCESS/FAILED banner style. README documents the walk-through.

**Acceptance:** from a clean checkout,
`make ZPR_ROOT=... && local-compute/deploy-docker.sh && local-compute/test-dns.sh` exits 0.

---

## Findings

Recorded as tasks discover them. Format:
`### Finding N — <claim> (confirmed | withdrawn | to be confirmed by test)`.

### Finding 1 — a `device.*` attribute from a declared trusted service reaches the actor record (to be confirmed by H0)

`parse_return_mappings` accepts any attribute outside the `zpr.` sub-namespace
(`zl-zpr-compiler/src/config/trusted_service.rs:139`), and trusted-service results are
pushed onto `authd_claims` (`vs/src/connection_control.rs:715-740`), which
`approve_connection` commits unconditionally (`libeval/src/eval.rs:264-282`). But every
`returns_attributes` example in the repos vends `user.*`, and the lookup is keyed on the
identity set, so nothing has exercised a device-class attribute from a declared service.
H0 settles it. Everything from H1 onward depends on it.

---

## Out of scope (tracked, not scheduled)

- **`GET /admin/hosts` (list).** Useful for operators, `read`-gated, and not needed by the
  resolver. Add when something needs it; keeping it out preserves "a `resolve` key can ask
  about a name it already knows, and nothing else".
- **PTR / reverse lookups.** Now more attractive, since a hostname is a better PTR target
  than a service name, but it still widens the `resolve` scope to an address-keyed lookup.
  Unchanged from `2026-09-15-dns-integration.md`.
- **Policy-declared host names.** This plan's rejected approach B (see **Resolved while
  planning**). Contract 2's index is source-agnostic, so if policy ever grants a hostname it
  can populate the same `host:<NAME>` entries without touching the plugin or the admin API.
- **Scoped/tenant-qualified names.** Unnecessary while one naming authority owns the
  namespace per ZPRnet. If that changes, qualify the index key rather than mangling values.
- **Notifying the naming authority of a rejected claim.** H2 exposes conflicts on the admin
  API for a poller. A push (`vs/src/event_mgr.rs`) is the follow-up if polling proves too
  slow to matter.
- **Multi-answer RRsets for a shared name.** DNS-native, and wrong for a machine name:
  `ping6 somename` should not be a coin flip. Deliberately rejected, not deferred.

---

## Resolved while planning

- **Why not the X.509 CN.** CNs are not unique and not indexed, on purpose: zipline#31 (A2)
  re-keyed actor storage by ZPR address precisely because the old CN→address map was a
  single-slot `DashMap<String, IpAddr>` where the second connect overwrote the first, and
  `clean_up` then orphaned the survivor. `test_shared_cn_actors_remain_distinct`
  (`vs/src/db/actor.rs:950`) pins "two connected actors may share one CN". CNs are also
  optional, unconstrained X.509 strings — the demo's bootstrap holds `vs.zpr`, `web.demo`
  and `alice`, a mix of dotted names and bare labels, and nothing stops one containing
  characters illegal in a DNS label. A purpose-built, validated name attribute separates
  cryptographic identity from network naming instead of overloading one string with both.
- **Why `device.hostname` and not `device.zpr.cname`.** ZPR owns the `zpr.` sub-namespace
  inside every class domain; a declared trusted service returning one is rejected at compile
  time because it could forge an identity key or an authority marker
  (`zl-zpr-compiler/src/config/trusted_service.rs:124-145`, tests at `:744`). A reserved
  name could therefore only be populated by ZPR itself — i.e. from policy — which is
  approach B. Dropping `zpr.` makes the attribute deliverable by a trusted service with no
  schema change at all.
- **Why not policy-declared names (approach B).** The evaluator could grant a hostname the
  way it grants `zpr.services` (`libeval/src/eval.rs:295-301`), which is authenticated by
  construction and gives a clean ZPL story. But it is the three-repository change
  `docs/REPOSITORIES.md` warns about — grammar, schema, evaluator — and it makes every
  machine addition a policy recompile and install. Wrong shape for a managed network where
  machines come and go. Kept as a possible second source for the same index; see Out of scope.
- **Why not one service per machine.** It already works and needs no code, and it is exactly
  the unnaturalness this plan exists to remove: `ping6 alice-ssh.zpr`, one service per
  machine per protocol.
- **Why no mangling.** Rewriting a duplicate `somename` to `somename-1` was considered and
  rejected. (a) Nobody learns the mangled name — the operator typed `somename` into the
  control plane and that is what they will type into `ping6`. (b) It is unstable: which
  machine holds the plain name depends on connect order, so restarting a fleet in a
  different order silently re-points a name at a different machine. In a system where the
  name selects who receives your traffic, that is also cheaply exploitable — connect first,
  hold the name. (c) `somename-1` may be a name a machine legitimately owns, so mangling
  needs a probe-until-free loop, and the loop can take a name another machine is about to
  claim. First-claim-wins plus a loud, operator-visible rejection keeps names stable and
  failures diagnosable.
- **Reachability without mangling.** Because `device.hostname` is multi-valued and claimed
  per value, the naming authority can return a friendly alias *and* a value unique by
  construction (its own machine id). A collision then costs only the alias; the machine
  stays reachable. Uniqueness stays with the component that has a database able to enforce
  it, and the VS's job shrinks to defending the invariant it cannot trust — one trusted
  service colliding with another, or a race between two connects.
- **Flat namespace, checked at claim time.** Machine names and service names share one label
  position under the zone, so `ping6 somename.zpr` needs no extra label and nothing new to
  learn. The alternative — a `somename.host.zpr` subzone — is unambiguous by construction
  but makes the common case uglier to defend against a collision a single naming authority
  can simply not create. Ambiguity is instead prevented where the names are assigned: a host
  claim colliding with a policy service name is rejected. The plugin *also* tries services
  first (contract 5), so the resolver is correct even if the claim-time check is bypassed.
- **`hset_nx` is the claim primitive, and it currently throws its answer away.** Both
  implementations discard whether they set the field (`db_redis.rs:132-136`,
  `db_fake.rs:414-424`). Returning `bool` (contract 3) is a one-line trait change with two
  implementations and one indifferent existing caller, and it removes the read-then-write
  race a hand-rolled claim would have.
- **`clean_up` checks ownership before releasing a name; `try_update_actor` does not.**
  `actor.rs:92-102` vs `:215-219`. That asymmetry is a live bug in the services path, filed
  as S1, and it is the exact failure the hostname index must not reproduce — hence the
  shared owner-checked release helper.
- **Index lifetime follows the actor, not the attribute.** `zpr.services` is
  `NEVER_EXPIRES` because policy derives it; a trusted-service attribute expires on
  `expiration_seconds` and is refreshed through `actor_attributes.rs:73`. Tying a name's
  index entry to attribute expiry would make machines vanish from DNS mid-session on a
  cache lapse, so entries live and die with the actor record.
- **The name must never be self-asserted.** `approve_connection` commits a peer's
  unauthenticated claims whenever a join policy matches
  (`libeval/src/eval.rs:264-282`), so accepting `device.hostname` from that set would let
  any machine claim any name. H0 Step 2 pins the negative.
- **One naming authority per ZPRnet** (operator decision, 2026-09-17). Collisions are
  therefore races and control-plane bugs rather than an expected steady state, which is why
  first-claim-wins with loud reporting is sufficient and name scoping is not needed.

## Open questions

None outstanding at plan time. The one design risk — whether a `device.*` attribute from a
declared trusted service reaches the actor record — is scheduled as H0 and tracked as
Finding 1, and it gates every other task in the Visa Service phase.
