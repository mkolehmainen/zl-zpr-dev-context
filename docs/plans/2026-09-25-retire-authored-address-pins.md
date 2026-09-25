# Retire authored `zpr.addr` pins: static adapter addresses come from a trusted service

**Status:** IN FLIGHT — umbrella [zipline#106](https://github.com/mkolehmainen/zipline/issues/106), blocked by [zipline#105](https://github.com/mkolehmainen/zipline/issues/105).
**Date:** 2026-09-25
**Repo state this plan was written against:** `zl-zpr-compiler` @ `2fa2ab5`, `zl-zpr-core` @ `0cab5d1`, `zl-zpr-demo` @ `19c9b72`, `zl-zpr-visaservice` @ `c3fdf9c`, `zl-zpr-dev-context` @ `c2fd468`, all on `zipline`. Line references are against those commits.

> Process note: per `skills/zpr/SKILL.md`, each task below becomes one GitHub issue in `mkolehmainen/zipline` and is worked on a feature branch off `zipline`, PR targeting `zipline`.

**Goal.** An adapter's static ZPR address has exactly one authored source: a trusted service that vends `device.zpr_addr` (zipline#99). The policy-text pin, `["zpr.addr", "<addr>"]` in a `.zplc` `provider` list, becomes a compile error that tells the author what to write instead. **If you want a static adapter address, at minimum you declare an `api = "file"` trusted service that vends it.** Node addresses and the visa service's own address are out of this rule (see *Decisions*).

This closes the Deferred item "Retiring `zpr.addr` in `define ... with`" in `docs/VISA_SERVICE.md` and upstream [zpr-compiler#133](https://github.com/org-zpr/zpr-compiler/issues/133).

---

## Background

**What "`define ... with zpr.addr`" actually is today.** The Deferred item's title is loose. In ZPL text it does not work: `define Web as a service with zpr.addr:'…'` domain-prefixes the key to `service.zpr.addr`, and the compile fails with "not found in any trusted service". The only authored pin that works is in the `.zplc`:

```toml
[services.web]
provider = [["device.zpr.adapter.cn", "web.demo"], ["zpr.addr", "fd5a:5052:8888::80"]]
```

`vec_to_attributes` (`zl-zpr-compiler/src/fabric_util.rs:16-31`) special-cases the key: it builds a ZPR-internal attribute through `Attribute::try_zpr_internal_attr`, bypassing `parse_domain`, which would otherwise reject it. `resolve_attributes` (`src/weaver.rs:572-579`) then attributes it to the default trusted service. The provider list becomes the service's join policy, so the pin reaches the visa service as a `zpr.addr eq X` join condition. `pins_zpr_addr` (`zl-zpr-visaservice/libeval/src/eval.rs:22-30`) recognizes it and commits the adapter's matching request.

**The node path uses the same visa-service mechanism and stays.** The weaver writes `zpr.addr` into each node's provider attributes from `[nodes.X] zpr_address` (`src/weaver.rs:709-712`, calling `try_zpr_internal_attr` directly rather than through `vec_to_attributes`). Node addresses are topology: the compiler plans links and routes from them. So `pins_zpr_addr`, the pin-versus-grant precedence and the conflict check in `authorize_connection` (`vs/src/connection_control.rs:1300-1380`) all stay. **This plan changes no visa-service code.** What goes away is the authored adapter pin, and with it the need to write the same address twice, in adapter config and in policy.

**Who uses authored pins** (`git grep 'zpr\.addr'` at the commits above):

| Where | Fixtures |
|---|---|
| `zl-zpr-core/integration-test/pregen/` | `v6-1node-2actor-ping.zplc`, `v6-1node-3actor-ping.zplc`, `oidc-test.zplc`, `oidc-renewal.zplc`, `oidc-file-interplay.zplc`, `attr-query.zplc`, `v4-1node-2actor-ping.zplc`, `v4-1node-3actor-ping.zplc` |
| `zl-zpr-demo/dns-demo/` | `zpr-conf/admin/dns-demo.zplc.template` (web `::80`, resolver `::53`) |
| `zl-zpr-demo/multinode-demo/` | `zpr-conf/admin/multinode-demo.zplc.template` (OciWeb `::8`, PremWeb `::9`) |
| `zl-zpr-demo/iot-demo/` | **none.** Every adapter is dynamic; only the node has a `zpr_address`. |
| `zl-zpr-compiler` | `README_ZPLC.md:452-461` documents the pin; `fabric_util.rs:75-80` tests it |

Several core fixtures (`v6-1node-*`, `v4-1node-*`, `attr-query`) give a service **only** `["zpr.addr", …]` as its provider. There the address *is* the service's provider identity. Those fixtures need a real provider attribute (the adapter CN, which the matching `.zpl` already uses in its `define … as a service with device.zpr.adapter.cn:…`), not just a deleted line.

**Nothing tests the `device.zpr_addr` grant end to end today.** zipline#99 has unit coverage in `connection_control.rs` only. Moving the netns fixtures to grants gives it integration coverage for the first time.

---

## Decisions

- **Hard compile error, not a deprecation warning.** Early-release code with no compatibility burden (`skills/zpr/SKILL.md`, "Project invariants"), and a warning trips `--Werror` anyway. The error names `device.zpr_addr` and the `file`-store shape, so the fix is in the message.
- **Nodes keep `zpr_address`, and the visa service keeps `fd5a:5052::1`.** Both are fixed by the network's own configuration, not granted to an endpoint. Node addresses feed the compiler's topology. The visa service address is a compile-time constant on both sides (`docs/VISA_SERVICE.md`, "Constants that must stay in sync"). Neither goes through `vec_to_attributes`.
- **No visa-service change and no policy-compiler floor bump.** An old policy carrying an authored pin is still a signed policy granting an address, so honouring it is not a hole. Bumping `POLICY_MIN_COMPILER_MINOR` to refuse it would buy nothing. Every in-tree `.bin2` is regenerated anyway.
- **Adapters that bring up a static TUN keep `zpr_addr` in their config.** The request is scrubbed (no pin matches), the grant supplies the same address, and the adapter's `GrantedAddressMismatch` check passes. The request and the grant must agree; that is the operator's contract, and the adapter already reports a mismatch loudly.
- **One store per deployment, keyed on the adapter CN.** `device.zpr.adapter.cn` is authenticated for every bootstrap adapter, and `dns-demo`'s `machines` store already keys on it. Where a deployment already has a device-keyed `file` store, extend that store rather than adding a second (dns-demo: `machines` becomes the naming *and* addressing authority).
- **Depends on #105 and does not work around its absence.** Without #105 an address-only store is pruned unless a ZPL statement references it. Every fixture here would need a throwaway reference, which is the trap this work is meant to end.

---

## Dependency graph and order

```
#105 compiler: retain VS-interpreted attribute providers   (zl-zpr-compiler)   [outside this plan]
 ├─► T1  core integration fixtures: pins -> file-store grants        (zl-zpr-core)
 └─► T2  demos: dns-demo, multinode-demo, iot-demo                   (zl-zpr-demo)
        T1 + T2 ─► C1  compiler rejects authored zpr.addr + docs    (zl-zpr-compiler, zl-zpr-dev-context)
                        └─► Z1  integration tier at tip + plan retirement   (zl-zpr-dev-context)
```

T1 and T2 move every in-tree pin to a grant while the compiler still *accepts* pins, so each lands green on its own. They touch disjoint repositories and can run in parallel. C1 must come after both: once it merges, any pin left in the tree fails to compile. Z1 owns the cross-cutting acceptance run (the zipline#95 postmortem rule) and retires this plan.

The umbrella and T1 and T2 each carry a native `blockedBy` on #105. Blockers are not inherited from an umbrella by `next-issue.py`, so the first children need their own.

## Issue map

| Task | Repo | Issue |
|---|---|---|
| Umbrella | — | [zipline#106](https://github.com/mkolehmainen/zipline/issues/106) |
| T1 | `zl-zpr-core` | [zipline#107](https://github.com/mkolehmainen/zipline/issues/107) |
| T2 | `zl-zpr-demo` | [zipline#108](https://github.com/mkolehmainen/zipline/issues/108) |
| C1 | `zl-zpr-compiler`, `zl-zpr-dev-context` | [zipline#109](https://github.com/mkolehmainen/zipline/issues/109) |
| Z1 | `zl-zpr-dev-context` | [zipline#110](https://github.com/mkolehmainen/zipline/issues/110) |

---

## Cross-repository contract: the address store

Every deployment that wants a static adapter address declares a trusted service shaped like this. Any API works (zipline#99); `file` is the minimum.

```toml
[trusted_services.addresses]
api = "file"
returns_attributes = ["zpr_addr -> device.zpr_addr"]
expiration_seconds = 3600
```

```json
{ "device.zpr.adapter.cn": { "adapter1": { "zpr_addr": ["fd5a:5052:8888::1:1"] } } }
```

The visa service reads `<file_ts_dir>/<service-id>.json`, and `file_ts_dir` defaults to the directory of `vs.toml` (`docs/VISA_SERVICE.md`, "Configuration"). After #105 no ZPL reference is needed to keep the store woven. The grant then passes the same static-range checks a pin did: inside `fd5a:5052::/32`, not `fd5a:5052::1`, outside the managed pools, not held by a live actor.

---

## Tasks

### T1: core integration fixtures use `device.zpr_addr` grants (`zl-zpr-core`) — [zipline#107](https://github.com/mkolehmainen/zipline/issues/107)

**Blocked by:** #105.

- [ ] Add `integration-test/pregen/addresses.json`: one CN-keyed store covering `adapter1`/`adapter2`/`adapter3` at the addresses the scripts already use (`fd5a:5052:8888::1:1`, `::2:1`, `::3:1`). If two fixtures disagree about a CN's address, reconcile them to one value rather than adding a second file.
- [ ] In each `.zplc` from the *Background* table: delete every `["zpr.addr", …]` provider entry, and add the `[trusted_services.addresses]` block from the contract. Where a service's provider was *only* the pin, replace it with `["device.zpr.adapter.cn", "<cn>"]`, matching the service's `define` in the `.zpl`. Replace the zipline#96 "pin the adapter's static ZPR address" comments with the grant explanation.
- [ ] `attr-query.zplc`: adapter1 stays dynamic (the one dynamic-path test, zipline#88). Only adapter2's entry goes into the store, and `attr-query-data.json` is untouched.
- [ ] Copy `addresses.json` into the vs working directory with **one helper** in `lib/common_funcs.sh`, called from every script whose policy declares the store. Don't copy it separately in each script.
- [ ] Regenerate every `.bin2` with `make -C integration-test/pregen rebuild` and a `zplc` that has #105.
- [ ] The v4 fixtures (`v4-1node-*`) back only `unused_or_outdated/one-node-v4-test.sh` and are not runnable (their `10.253.x` addresses fail the static-range check, which is a pre-existing limitation). Convert their configs so `make rebuild` succeeds, and don't try to run them.
- [ ] Assert the grant in at least one netns script: `vs-admin actors` shows the adapter at its store address. That is the first end-to-end test of zipline#99.

**Acceptance.** `git grep '"zpr.addr"' integration-test/pregen/*.zplc` is empty. All seven netns-tier scripts pass (`make docker-test`, or `zpr-dev build --test netns` with a manifest pinning this branch and #105's compiler). `attr-query-test.sh` and `one-node-oidc-renewal-test.sh` are outside the tier but use converted fixtures, so run both by hand and quote the results. This touches addressing, so the gate escalation in `skills/zpr/SKILL.md` applies.

### T2: demos use `device.zpr_addr` grants (`zl-zpr-demo`) — [zipline#108](https://github.com/mkolehmainen/zipline/issues/108)

**Blocked by:** #105.

- [ ] **dns-demo.** In `dns-demo.zplc.template`, delete the `zpr.addr` entries from `web` and `zpr-dns`, and add `"zpr_addr -> device.zpr_addr"` to the existing `machines` store's `returns_attributes`. In `machines.json`, add `zpr_addr` for `web.demo` (`fd5a:5052:8888::80`) and `dns.demo` (`::53`, a new entry). `deploy-docker.sh` already copies `machines.json`. Update the `adapter-*-conf.toml.template` "must match dns-demo.zplc.template" comments to point at `machines.json`, the `README.md` address table ("pinned service addr" becomes "granted by `machines`"), and every comment in `dns-demo.zplc.template`, `dns-demo.zpl` and `local-compute/` that calls these addresses pinned. `test-dns.sh`'s address assertions keep passing unchanged: the addresses don't move.
- [ ] **multinode-demo** (config only: multinode is not runnable against current `zl-zpr-core`, so don't attempt a run). Delete the two `zpr.addr` provider entries. Add an `addresses` `file` store for `ociweb.demo` (`::8`) and `premweb.demo` (`::9`), and copy its JSON in `local-compute/deploy-docker.sh` and `oci-compute/deploy-zpr.sh` wherever `attrfile.json` is copied. Fix `README.md:217` ("the `zpr.addr`s the policy declares") and the `zpr-conf/README.md` address notes. Leave `work/` alone: it is historical scratch.
- [ ] **iot-demo.** It has no authored pins, so no config change. Recompile `setup/iot-demo.zplc` and `setup/zpr-full-access.zplc` with the new compiler to prove they still build, commit the regenerated `iot-demo.bin2` only if it differs, and check the READMEs for any claim that static adapter addresses come from policy.
- [ ] `containerized-demo` is obsolete and out of scope.

**Acceptance.** `git grep '"zpr.addr"' -- '*.zplc*'` in `zl-zpr-demo` is empty. The docker tier (`zpr-dev build --test docker`: dns-demo deploy plus `test-dns.sh`) passes against a set with #105's compiler, and the result is quoted in the PR. The multinode and iot policies compile; no run is claimed for them.

### C1: the compiler rejects an authored `zpr.addr` (`zl-zpr-compiler`, `zl-zpr-dev-context`) — [zipline#109](https://github.com/mkolehmainen/zipline/issues/109)

**Blocked by:** T1 (#107), T2 (#108).

- [ ] **Failing tests first** (`src/fabric_util.rs` tests and `tests/zpl-test.rs`): a `[services.X] provider` with `["zpr.addr", …]` fails to compile, and the error names `device.zpr_addr`. Cover every `provider` site that routes through `vec_to_attributes` (`src/weaver.rs:357, 400, 677, 1225, 1235`): services, nodes, `visa_service.admin_attrs`, proxy providers. A node's `zpr_address` still emits the `zpr.addr` join condition (existing node tests stay green). Replace the `fabric_util.rs:75-80` test that asserts the pin is *allowed*.
- [ ] **Change.** Drop the `KATTR_ADDR` special case in `vec_to_attributes` and reject the key with a `ConfigError` along the lines of: "`zpr.addr` cannot be set in policy; grant a static adapter address with a trusted service that returns `device.zpr_addr` (see README_ZPLC.md)". Remove the `KATTR_ADDR` arm in `resolve_attributes` (`weaver.rs:572-579`) **if** nothing else reaches it once the node path is accounted for; otherwise narrow its comment to the node case. Keep the `KATTR_ADDR` constant for the node emitter. Repoint the zpr-compiler#133 comments at this umbrella.
- [ ] **Human docs** (`zl-zpr-compiler/README_ZPLC.md:452-461`): replace the pin example with the address-store example from the contract above.
- [ ] **Agent docs** (`zl-zpr-dev-context`, separate PR): `docs/VISA_SERVICE.md`, where "Authenticating actors" (l.170-203) describes adapter pins, now says pins come only from node `zpr_address`, and adapters are granted by a trusted service. `docs/SYSTEM_OVERVIEW.md:263-271` ("a `zpr.addr` pin in an adapter's `define`"). `docs/ROUTING.md:395-397` (the "Static service address" row). `docs/DNS.md:82-104` (the resolver's "pinned" address and its `.zplc` example). `docs/ATTRIBUTE_SERVICE.md:85` ("same checks as a policy-pinned static address"). `docs/ZPL.md:72` (static addresses as `.zplc` content). Run `zpr-dev sync`.
- [ ] Version: follow `docs/BUILD.md` ("Versions and tags"). Refusing previously valid input is a compatibility change for policy authors; the binary format and the visa service's floor are unchanged.

**Acceptance.** New tests fail before and pass after; `make check` is clean in `zl-zpr-compiler`. Every `.zplc` in `zl-zpr-core`, `zl-zpr-demo` (except `containerized-demo`) and `zl-zpr-visaservice` compiles with this compiler. No doc under `zl-zpr-dev-context/docs/` or `README_ZPLC.md` still tells an author to pin an adapter address in policy.

### Z1: integration tier at tip, then retire this plan (`zl-zpr-dev-context`) — [zipline#110](https://github.com/mkolehmainen/zipline/issues/110)

**Blocked by:** C1 (#109).

- [ ] After C1's last PR merges, run the full `zpr-dev build` gate with `--test netns` and `--test docker` against a set at tip, and quote both results on the umbrella. A failure here is an escalation (file it, and say the umbrella is not done), not a footnote.
- [ ] Retire this plan per `skills/zpr/SKILL.md` ("Plan retirement"). In `docs/VISA_SERVICE.md` `## Design decisions`, add an entry recording the retirement and the decisions above. Delete the "Retiring `zpr.addr` in `define ... with`" Deferred item, and rewrite the static-address-grants entry that lists the policy pin as precedence source 1 for adapters. Add this plan's row to `docs/plans/README.md`, delete this file and its `AGENTS.md` row, grep the workspace for its file name, and run `zpr-dev sync`.
- [ ] Comment on zpr-compiler#133 (upstream) with a pointer to the umbrella. That is an outward post, so confirm with the operator first.

**Acceptance.** Both tiers green at tip, quoted on the umbrella. The plan file is gone and no reference to it remains.

---

## Out of scope

- **Node addresses as grants.** They are topology; see *Decisions*.
- **The compile-time pool check** (a separate Deferred item in `docs/VISA_SERVICE.md`).
- **`zpt` and the `examples/milestone2` walkthrough.** `zpt` sets `zpr.addr` as an evaluation input, which stays valid. milestone2 is prototype-era.
- **`containerized-demo`**: obsolete.

## Open questions

None at filing.
