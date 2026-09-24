# Static ZPR addresses are granted, never asserted

**Status:** IN FLIGHT — issues not yet filed; see *Issue map*.
**Date:** 2026-09-24
**Repo state this plan was written against:** `zl-zpr-visaservice` @ `f2ad61b`, `zl-zpr-core` @ `95d316f`, `zl-zpr-compiler` @ `3a7cc4e`, `zl-zpr-dev-context` @ `20cb538`, all on `zipline`. Line references are against those commits.

> Process note: per `skills/zpr/SKILL.md`, each task below becomes one GitHub issue in `mkolehmainen/zipline` and is worked on a feature branch off `zipline`, PR targeting `zipline`.

**Goal.** A ZPR address is something the visa service grants, never something the peer asserts. Today an adapter can name its own address with `--zpr-addr` and the visa service commits it whenever *any* join policy matches, without range, pool or collision checks. Afterwards an endpoint gets an address in exactly one of three ways, static addresses live in their own range, and the visa service refuses anything that would mask an address it manages.

---

## Background

`EvalContext::approve_connection` (`zl-zpr-visaservice/libeval/src/eval.rs:208-277`) copies a requested `zpr.addr` from the adapter's unauthenticated claims into the join-policy query, and if the match set is non-empty commits the whole query, requested address included, to the actor. `ConnectionControl::authorize_connection` (`vs/src/connection_control.rs:1238-1252`) then sees an actor that already has an address and skips pool allocation. The matching policy does not have to mention `zpr.addr`. Since join policies are minted per service provider, any adapter that provides any service can self-assign any address.

The code's own comments describe the intended semantics and acknowledge the gap: `eval.rs:196-198` says the connection "will fail if the policy specifies a different address" (there is no mechanism), `eval.rs:250` reads "TODO: Currently we have no way to set a static addr from policy", and `eval.rs:238-241` explains the squatting risk but closed only the **no-match** case.

Consequences today:

- **Address squatting.** Actors are keyed by ZPR address. A requested address held by a live actor overwrites that actor's record.
- **No range check.** `zl-zpr-core/integration-test/pregen/oidc-file-interplay.zpl` documents an adapter keeping `fd00:1:1::1`, outside `fd5a:5052::/32` entirely.
- **Pool double-allocation.** A self-asserted address inside `fd5a:5052:adda:1::/64` is never taken out of the pool; only the node path reserves (`vs/src/vsapi_worker.rs:944`). The pool can hand the same address to another adapter, and `release_zpr_addr` errors at disconnect for an address it never allocated.
- Dock-side source-address enforcement (zipline#83) limits what a squatter can *send*. It does not protect the actor records or the pool.

The only thing that confirms an address today is a join policy whose conditions contain `zpr.addr eq X`. The compiler emits one for nodes from `zpr_address` in the `.zplc` (`zl-zpr-compiler/src/config/node_link.rs:57`) and accepts `zpr.addr` in a `define ... with` (`fabric_util.rs:19-23`, `weaver.rs:575`). Nothing requires an adapter's join policy to carry one.

---

## The rule

An endpoint's address comes from exactly one of three sources, in this precedence:

| Source | How it is expressed | Who confirms it |
|---|---|---|
| **1. Policy pin** | `zpr_address` on a node in the `.zplc`; `define ... with zpr.addr:X` for an adapter. Compiles to a `zpr.addr eq X` join condition. | A **matched** join policy carries a `zpr.addr` condition. |
| **2. Trusted-service grant** | A declared trusted service returns `device.zpr_addr` for the device's identity. | The attribute arrives as an **authenticated** claim. |
| **3. Pool allocation** | Nothing. | The visa service's `NetMgr`. Default. |

The peer's requested `zpr.addr` (adapter `--zpr-addr`, node connect param) is a **check, never a grant**: it is honoured only when source 1 or 2 names the same address. Otherwise it is scrubbed and source 3 applies; the adapter then refuses the substituted address itself (`GrantedAddressMismatch`, `zl-zpr-core/adapter/ph/src/link_state.rs:1493`) with a message telling the operator to drop `--zpr-addr`, and a node that joined over a link is rejected at `vs/src/vsapi_worker.rs:954`.

Every static address (sources 1 and 2) must satisfy, before it is accepted:

- inside `fd5a:5052::/32` (`net_mgr::is_zpr_addr`) and not the visa service address `fd5a:5052::1`;
- **not** inside a managed pool (`net_mgr.is_managed_address`: `fd5a:5052:90de:1::/64`, `fd5a:5052:adda:1::/64`, IPv4 equivalents). Static addresses live in a different address space from the pools, so the pool can never double-allocate one and there is nothing to reserve or undo;
- not held by a live actor (`actor_mgr.get_actor_by_zpr_addr`). The second claimant is rejected and logged, never silently renumbered.

`fd5a:5052:8888::/64` is the de facto static range: every pinned adapter address in `zl-zpr-demo` is in it, and every pinned node address in the tree (`fd5a:5052:90de::1`, `::2`, `::10`; `fd5a:5052::2` in the core integration tests) is outside the node pool. Nothing that exists is rejected by these checks except the core integration tests' `fd00:1:x::1` adapters, which Phase P renumbers first.

---

## Dependency graph and order

```
P1  core integration tests: pin and renumber          (zl-zpr-core)
 └─► A1  approve_connection: request is a check        (zl-zpr-visaservice, libeval)
 └─► A2  authorize_connection: static-address checks   (zl-zpr-visaservice, vs) + docs
        └─► A3  device.zpr_addr trusted-service grant  (zl-zpr-visaservice, vs) + docs
```

P1 lands under the *current* visa service and must merge before A1 or A2, because both make the visa service reject the addresses the tests use today. A1 and A2 are independent of each other. A3 needs A2 (its checks) and is cleanest after A1.

No compiler or policy-schema change anywhere in this plan. `device.zpr_addr` is an ordinary device attribute `returns_attributes` already accepts.

## Issue map

| Task | Repo | Issue |
|---|---|---|
| P1 | `zl-zpr-core` | to be filed |
| A1 | `zl-zpr-visaservice` | to be filed |
| A2 | `zl-zpr-visaservice`, `zl-zpr-dev-context` (docs) | to be filed |
| A3 | `zl-zpr-visaservice`, `zl-zpr-dev-context` (docs) | to be filed |

---

## Phase P — Prerequisite

### Task P1: core integration tests stop depending on the loophole (`zl-zpr-core`)

Every script in `integration-test/` starts adapters through `lib/common_funcs.sh:97-117` with `--tun-if tun0 --zpr-addr <static>`, and no `.zpl` in `pregen/` pins those addresses. All of them depend on the current behaviour and break when A1 or A2 lands.

**Change.**

- Renumber `A_ZPR_ADDR`, `B_ZPR_ADDR`, `C_ZPR_ADDR` from `fd00:1:x::1` into `fd5a:5052:8888::/64` in every `*-test.sh` (e.g. `one-node-oidc-test.sh:70-73`), and set `ZPR_SUBNET` to `fd5a:5052::/32`. Pick addresses that do not collide with the demos' `::8`, `::9`, `::53`, `::80`.
- Add `zpr.addr:'<addr>'` to each adapter's `define` in the `pregen/*.zpl` policies and regenerate the `.bin2` files. This is source 1 and needs no visa service change.
- Leave `attr-query-test.sh:427-436,641-654`, which already exercises dynamic addressing for adapter1, as the one dynamic-path test.

**Acceptance.** Every netns test passes under the unmodified visa service at `f2ad61b`, and passes again after A1 and A2. `grep -r 'fd00:1' integration-test/` is empty.

Option considered and deferred: converting the tests to dynamic addressing throughout. More work, and the static tun setup is worth keeping as coverage of source 1.

---

## Phase A — Visa service

### Task A1: `approve_connection` treats the requested address as a check (`libeval`)

**Change** in `EvalContext::approve_connection` (`libeval/src/eval.rs:208-277`): commit the requested `zpr.addr` only if at least one **matched** join policy has an `AttrExp` whose key is `zpr.addr`. `JPolicy.matches` (`libeval/src/joinpolicy.rs:14`) already exposes the conditions. When no matched policy pins it, take the existing no-match scrub path so the caller allocates from the pool. Update the doc comment at `eval.rs:188-207` and remove the TODO at `:250`.

Scrub-and-allocate rather than hard-deny: it reuses the existing path, both peers already reject a substituted address with a clear message, and a deny from the visa service is indistinguishable at the adapter from a policy denial, which is the worse diagnostic for what is usually a misconfiguration.

**Tests** (`libeval/src/eval.rs`; the existing tests at `:1047-1134` use `tests/zpl/basic.zplc`, whose node policy pins `zpr.addr`, so they keep passing):

- Join policy matches on CN only, request present: address scrubbed, actor has no `zpr.addr`.
- Join policy matches with `zpr.addr eq X`, request X: committed.
- Join policy matches with `zpr.addr eq X`, request Y: no match, scrubbed.
- Two policies match, one with `zpr.addr eq X`, request X: committed.
- Unauthenticated claim `device.zpr_addr` (not `zpr.addr`): ignored, never reaches the actor. (Already true, `:1103`; pin it because A3 reads that key from the actor.)

**Acceptance.** Tests above pass; `cargo test` in `libeval` and `vs` green; the `vs/src/connection_control.rs:2889` test that documents "unauthenticated claims only for `zpr.addr` under a matching join policy" is updated to the new rule.

### Task A2: `authorize_connection` enforces the static-address space (`vs`)

**Change** in `ConnectionControl::authorize_connection` (`vs/src/connection_control.rs:1238-1252`): when the approved actor carries a `zpr.addr`, apply the three checks from *The rule* (range and not the visa service address; not `is_managed_address`; not a live actor) before accepting it. Failure is an `AuthError` naming the check. Remove the in-pool reservation on the node path (`vs/src/vsapi_worker.rs:944`, `undo.took_zpr_addr`), which the pool check makes dead.

**Docs** (`zl-zpr-dev-context`): add a `fd5a:5052:8888::/64` "static, policy-granted" row to the addressing table in `docs/SYSTEM_OVERVIEW.md:255-260`, and state the pools-versus-static split there. Record the fix in `docs/VISA_SERVICE.md`'s implementation status.

**Tests** (`vs/src/connection_control.rs`, `authorize_connection`):

- Pinned address inside `fd5a:5052:adda:1::/64` or `fd5a:5052:90de:1::/64`: rejected, pool untouched.
- Pinned address outside `fd5a:5052::/32`, or equal to `fd5a:5052::1`: rejected.
- Pinned address held by a live actor: rejected, existing actor record unchanged.
- Pinned address in `fd5a:5052:8888::/64`: accepted, pool untouched.
- Node with an in-pool `zpr_address`: rejected (replaces whatever covered the reservation path).

**Acceptance.** Tests above pass; the demos' pinned addresses and every `zpr_address` in the tree still authenticate (P1 has already moved the core tests). The pool ranges remain defined only in `vs/src/config.rs`; an author who pins an in-pool address learns of it at join time. Teaching `zplc` the ranges is out of scope.

### Task A3: a trusted service may grant the address (`vs`)

**Operator-side configuration** (no code): any declared trusted service maps a field to `device.zpr_addr`. With the `file` api:

```toml
[trusted_services.addresses]
api = "file"
returns_attributes = ["addr -> device.zpr_addr"]
expiration_seconds = 3600
```

```json
{ "device.zpr.adapter.cn": { "adapter1": { "addr": ["fd5a:5052:8888::1"] } } }
```

Trusted-service attributes already land in the authenticated claims before `approve_connection` runs (`vs/src/connection_control.rs:1173-1207`) and are always committed, so the attribute reaches the actor today with no compiler, schema or store change.

**Change** in `authorize_connection`, after `approve_connection` and before pool allocation: if the actor carries an authenticated `device.zpr_addr`, run the A2 checks on its value and set it as the actor's `zpr.addr`. If the actor *also* has a policy-pinned `zpr.addr` that differs, reject: two sources disagree, and that is an operator error to surface, not resolve. The adapter's own request has already been scrubbed by A1 unless a pin matched it, so the request can never override a grant; if the adapter demanded a different address it reports `GrantedAddressMismatch` itself.

**Docs** (`zl-zpr-dev-context`): document `device.zpr_addr` in `docs/VISA_SERVICE.md` alongside the static-address privilege paragraph at `:169`, and in `docs/ATTRIBUTE_SERVICE.md` as a well-known attribute the visa service acts on, the way `docs/DNS.md` documents `device.hostname`.

**Tests** (`vs/src/connection_control.rs`):

- Authenticated `device.zpr_addr X`, request Y, join policy matches on CN only: actor ends with `zpr.addr X`.
- Authenticated `device.zpr_addr X`, no request: actor ends with X, pool untouched.
- Authenticated `device.zpr_addr` failing any A2 check: rejected.
- Authenticated `device.zpr_addr X` and policy pin `zpr.addr Y`: rejected.
- Two connecting actors granted the same X: second rejected, first untouched.
- `device.zpr_addr` present only in unauthenticated claims: ignored, pool allocates.

**Acceptance.** Tests above pass. An end-to-end check in `zl-zpr-core/integration-test` or `zl-zpr-demo/dns-demo`: one adapter with no `zpr.addr` in its `define` and no `--zpr-addr`, given an address by a `file` store, connects at that address and is reachable there.

---

## Out of scope (tracked, not scheduled)

- **Compile-time pool check.** `zplc` does not know the visa service's pool ranges; an in-pool pin is a join-time error. Worth doing once the ranges are stable enough to share through `zl-zpr-common`.
- **Converting the core integration tests to dynamic addressing.** See P1.
- **Retiring `zpr.addr` in `define ... with`** (zpr-compiler#133). Source 1 keeps working; whether policy text should carry addresses at all once source 2 exists is a separate question.
- **Persisting pool state** (the `TODO: Update redis` in `net_mgr.rs:157`). Unrelated to this plan but adjacent.

## Resolved while planning

- **Why the address is a `device` attribute, not `user`.** An endpoint is the device carrying its flows plus an *optional* user and service (`docs/SECURITY_MODEL.md:101-104`); "a device may have no user, one, or many" (`:118`). Nodes and headless servers have an address and no user. A user roaming between machines carries their attributes but not the address, which the adapter's tun interface holds. Actors are keyed by ZPR address, one per adapter, and the natural lookup key is the device identity `device.zpr.adapter.cn`.
- **Why `device.zpr_addr` and not `zpr.addr` or `device.zpr.addr` in the mapping.** Same reasoning the hostname plan used for `device.hostname` over `device.zpr.cname` (`2026-09-17-machine-hostname-dns.md`, *Resolved while planning*). ZPR owns the `zpr.` sub-namespace inside every class; `parse_return_mappings` rejects it from any declared service (`zl-zpr-compiler/src/config/trusted_service.rs:144`), so a reserved name could only ever be populated from policy. An ordinary device attribute is deliverable by a trusted service today with no compiler or schema change. `zpr_addr` rather than `addr` so it cannot be confused with an operator's own address field, which matters because the visa service acts on it. Underscores are fine (`user.bas_id` exists in compiler test data).
- **Why two keys for one concept.** `zpr.addr` stays the actor's committed address, the peer's request, and the node pin; `device.zpr_addr` is the grant. The hostname index has the same attribute-to-internal-state shape. Renaming the internal key would touch every repository for no security gain.
- **Why any trusted service may grant, not just `file`.** The mapping must be declared in the signed `.zplc`, so it is explicit which service may return an address. A declared trusted service already vends the attributes that decide which join and communication policies match, and ZPR grants access by attribute, not by address, with dock-side enforcement (zipline#83) pinning an actor to the address it was granted. Choosing an address is therefore no new power. ZPR does not care which static addresses are used, only that they cannot mask addresses it manages or its own special addresses, which the A2 checks enforce for every source alike.
- **Why static addresses are outside the pools rather than reserved in them.** Reserve-in-pool needs `take_zpr_addr`, an undo path, and a release path that knows which addresses were reserved versus allocated; the node path has all three today and the adapter path has none. A separate address space needs one rejection and no bookkeeping. Nothing in the tree pins an in-pool address.
- **Why scrub-and-allocate rather than deny in `approve_connection`.** See A1. Either is a one-branch change; the peers' existing mismatch diagnostics make scrubbing the more legible failure.
- **Why not fix only the no-match case.** That was done (`eval.rs:238-241`) and left the any-match case, which is the one every service provider hits.

## Open questions

None blocking. The issue numbers are filled in when P1–A3 are filed.

## Related

- `docs/VISA_SERVICE.md:169`: "does it get special privileges such as a static ZPR address" describes the intended model.
- `docs/plans/2026-09-17-machine-hostname-dns.md`: the precedent for a trusted-service-vended, visa-service-interpreted device attribute.
- zipline#83: dock-side source-address enforcement.
- zpr-compiler#133: `zpr.addr` in provider clauses.
- Workspace root `mac-exploration.md`: the macOS adapter discussion that surfaced this.
