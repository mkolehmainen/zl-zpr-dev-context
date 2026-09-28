# Policy-install re-authentication; bootstrap authentication does not expire

**Status:** IN FLIGHT — umbrella zipline#118.
**Date:** 2026-09-28
**Repo state this plan was written against:** `zl-zpr-core` @ `25f9e93`, `zl-zpr-visaservice` @ `881f7ba`, `zl-zpr-vsapi` (via `zl-zpr-common` v0.29.0), `zl-zpr-dev-context` @ `2225875`, all on `zipline`. Line references are against those commits.

> Process note: per `skills/zpr/SKILL.md`, each task below becomes one GitHub issue in `mkolehmainen/zipline` and is worked on a feature branch off `zipline`, PR targeting `zipline`.

**Goal.** Two changes:
1. **RSA bootstrap authentication no longer expires.** Re-proving possession of a static key on a timer demonstrates nothing new (`docs/OIDC.md:485-497`). Bootstrap key revocation may come later.
2. **Installing a policy makes every connected actor, nodes and adapters alike, re-authenticate against the new policy.** The visa service asks each node over the VSS-API. The node re-authenticates itself over its existing VS-API session and relays the request to its docked adapters over ZDP. Every authority an actor holds is re-proved: RSA against the new snapshot's bootstrap keys, OIDC silently through the AuthAgent. An actor that has not re-authenticated under the new policy by a deadline is revoked.

This makes a bootstrap key removed from policy take effect within one deadline of the install. Today it never takes effect for a connected actor. It also closes the `TODO: request re-auth all nodes` at `vs/src/event_mgr.rs:182`.

---

## Background

**Today an RSA adapter is disconnected about every 4 h and a policy install re-checks no one's authentication.**
- The visa service stamps bootstrap `device.zpr.authority` and the `zpr.vs.bootstrap.ident` identity attribute with `DEFAULT_AUTH_EXPIRATION` (4 h, `vs/src/config.rs:86`).
  - `device.zpr.authority` stamps: `connection_control.rs:284-288`, `:360-364`.
  - `zpr.vs.bootstrap.ident` mints: `:420-435`, `:1001-1018`, `:1096-1111`.
- `get_authentication_expiration` is the minimum over the authorities and identity keys (`libeval/src/actor.rs:176-196`).
- `auth_sweep` revokes expired adapters every 30 s. The node can't renew an RSA-only link: `maybe_renew_auth` has no `renewal_identity` (`adapter/ph/src/link_state.rs:2738`), and `reauthorize_actor` accepts only one OIDC blob (`connection_control.rs:895-911`).
- On policy install, `handle_policy_updated` (`event_mgr.rs:178-317`) refreshes attributes, re-evaluates *stored* node attributes (`revalidate_nodes`, `:365`, which "does not check authentication") and re-checks visas. It never re-checks authentication (TODOs at `:182` and `:363`).

**Latent bug.** An RSA+OIDC actor's `device.zpr.authority` passes the reauthorize claim-rebuild filter unchanged (`connection_control.rs:956-967`), so its expiry stays pinned at connect + 4 h however often OIDC renews. Making bootstrap auth non-expiring removes the pin.

**The building blocks mostly exist.**
- **VSS-API:** `requestAuthentication @6 (addrs :List(IpAddr)) -> (ack :Ack)` is in the schema (`zl-zpr-vsapi/vs.capnp:642-654`, added "so vs can request re-auth"). Both repos already vendor it, but the node doesn't implement it (`libnode2/src/vss.rs:273-`) and the visa service never sends it.
- **VS→node calls:** one worker per node, with serialised, acked commands (`vs/src/vss_mgr.rs`, `vss_worker.rs`). `vss_do_revoke_auths` (`vss_worker.rs:648-681`) is the template. `set_services_all_nodes` (`event_mgr.rs:145-176`) is the template for fanning out to every node.
- **Node self re-auth without a TCP restart:** `VSCommandState` keeps the `vs_service` bootstrap capability for the whole session (`libnode2/src/vsconn.rs:174-177`). The node can call `connect(ctype=Reconnect)` → `challenge` → `authenticate` on it again. With `Reconnect` the visa service keeps the node's docked adapters, visas and router links, and refreshes the node actor (`vsapi_worker.rs:613-648, 1009-1024`). The only thing blocking this is the node-side `is_connected()` guard (`vsconn.rs:487-490`).
- **Node→adapter:** ZDP `RenewAuthenticationRequest`/`Response` (142/143) already exist. The node mints a fresh HMAC'd challenge (`send_renewal_credential_request`, `link_state.rs:2940`), and the adapter already holds its bootstrap key in memory and can sign any challenge (`BootstrapKey::authenticate_blob`, `auth.rs:410`). Two gaps: the adapter answers 142 only with OIDC (`:3052-3065`), and the node sends it only when a deadline passes.
- **Tracking who still owes a re-auth:** `approve_connection` stamps `zpr.vinst` = the policy generation (`libeval/src/eval.rs:335`), and only connect and reauthorize run it. So an actor's `zpr.vinst` is the generation it was last authenticated under. No new per-actor state is needed.

---

## Decisions

- **Bootstrap authentication doesn't expire.**
  - Bootstrap `device.zpr.authority` is stamped far-future, like `vs.zpr` today (`VS_AUTH_EXPIRATION`), for adapters and nodes.
  - `zpr.vs.bootstrap.ident` takes the actor's authority expiry (the minimum of its `device`/`user` authorities) instead of a fixed 4 h, so it no longer gates on its own. An actor with no authority keeps `DEFAULT_AUTH_EXPIRATION`.
  - OIDC lifetimes are unchanged.
  - Bootstrap key revocation is deferred.
- **Every policy install asks every connected actor to re-authenticate.** There is no diffing of what changed: one rule, easy to audit.
- **Every authority is re-proved, and the set must match exactly.** The adapter answers one 142 with one blob per namespace it originally authenticated with (SS for `device`, OIDC for `user`). The node rejects a reply whose set differs, and so does the visa service. If any leg fails, the whole re-auth fails. So an OIDC actor whose AuthAgent can't refresh silently is revoked at the deadline and must log in again.
- **Enforcement is by deadline, not by reply.**
  - The install records `(V = new vinst, T = now + reauth_deadline)`. `reauth_deadline` is a visa-service setting, default 300 s.
  - Once `now > T`, any actor whose `zpr.vinst < V` is revoked: adapters through the existing batched `revokeAuthentication`, nodes through `cc.disconnect`.
  - A rejection doesn't revoke early. The actor stays until `T`, and a retry may still succeed.
  - With several installs, the **earliest unmet deadline** applies. Authenticating under the newest generation satisfies all older ones, so a stream of installs can't postpone enforcement.
- **The visa service targets actors explicitly.** Each node gets `requestAuthentication(addrs)` listing its own address plus its docked adapters (`actor_mgr.get_adapters_connected_to_node`, `vs/src/actor_mgr.rs:373`). Each sweep pass re-sends to actors still owing. The node dedups through the existing `renewal_in_flight` flag.
- **A node re-authenticates in place.** It re-runs `connect(Reconnect)`/`challenge`/`authenticate` on its existing `vs_service` and swaps in the new `VSHandle` only on success. There's no TCP restart, and visas and adapter links are undisturbed. The visa service checks the node's key against the pinned new snapshot, as it does at connect.
- **SS on `reauthorize`**, the adapter leg:
  - the target is a live actor admitted through the calling node;
  - the actor holds `device.zpr.authority = bootstrap`;
  - `ssb.cn` equals its authenticated CN;
  - the signature verifies against the pinned snapshot's bootstrap key;
  - `|now − ts| ≤ MAX_CLOCK_SKEW_SECS` (180 s).

  There is no monotonic-`ts` rule. Freshness comes from the node-minted, HMAC'd challenge (at most 120 s old). Only the docking node could replay, and a compromised docking node is already SECURITY_MODEL Case 2.
- **Node culling at visa-service startup switches to last-seen age.** `actor_mgr::refresh_state` (`actor_mgr.rs:73-112`) culls nodes by authentication expiry, and with non-expiring bootstrap auth it would keep dead nodes forever. It changes to "not seen for `DEFAULT_AUTH_EXPIRATION`", preserving today's 4 h behavior.
- **No compatibility shims.** An old node answers `@6` with capnp "unimplemented", and an old adapter answers 142 without SS. Their actors are revoked at the deadline and reconnect fresh. That is acceptable under the early-release rule (`skills/zpr/SKILL.md`, "Project invariants"). **The ordering in the dependency graph keeps the tree itself from ever doing this.**

---

## Cross-repository interface contracts

**K1: VSS `requestAuthentication @6 (addrs) -> Ack`** (schema unchanged):
- `addrs` names actors docked to, or equal to, the receiving node.
- The node acks once it has *started* re-auth for each listed address. `processed` counts the addresses accepted.
- Unknown addresses are skipped and not counted.
- The node's own address means "re-authenticate yourself to the VS".
- The ack reports the request, not the outcome. Outcomes arrive as `authenticate`/`reauthorize` calls, and the visa service judges them by `zpr.vinst`.

**K2: VS-API node self re-auth.** `connect(ctype=Reconnect)` → `challenge` → `authenticate` on an existing session with a live node actor:
- the node actor is re-approved under the current snapshot, which stamps `zpr.vinst`;
- docked adapters, visas and router links are untouched;
- VSS worker duplication is handled (today only a warning, `vsapi_worker.rs:1226-1234`);
- on key failure the node actor is left as it was, and the deadline rule decides.

**K3: VS-API `reauthorize`** (wire unchanged; semantics widen):
- `blobs` carries 1..N blobs, with at most one per namespace.
- The namespace set must equal the live actor's authorities. Otherwise `ParamError`.
- The SS arm is as in *Decisions*. Its failures are a generic `AuthError`, like OIDC's `reject()` (`connection_control.rs:708`).
- On success, policy is re-run in the pinned snapshot (stamping `zpr.vinst`), each renewed authority is re-stamped, and `authExpires` is the new minimum.

**K4: ZDP 142/143** (wire unchanged; semantics widen):
- The node sends 142 on demand for any listed adapter, not only when a deadline passes.
- A 143 `Success` carries a base64-JSON **array** (`encode_blobs`, `auth.rs:346-374`) with one blob per namespace the link used, each bound to the echoed challenge.
- If the adapter can't produce every leg, it replies `AuthUnavailable`.

---

## Dependency graph and order

```
V1 VS: bootstrap never expires; ident follows authority; last-seen node culling   (zl-zpr-visaservice)
V2 VS: reauthorize per-namespace exact set + SS arm                               (zl-zpr-visaservice)
N1 node: requestAuthentication @6 + in-place self re-auth (K1, K2 node side)      (zl-zpr-core)
N2 node+adapter: on-demand 142, SS/OIDC answer, set check, multi-blob reauth (K4)  (zl-zpr-core)
V2 + N1 + N2 ─► V3 VS: install records (V,T), fans out @6, re-sends, sweep revokes; K2 VS side   (zl-zpr-visaservice)
V1 + V3 ─► I1 integration: one-node-policy-reauth-test.sh                          (zl-zpr-core)
            └─► Z1 netns tier + docs + plan retirement                              (zl-zpr-dev-context)
```

- V1, V2, N1 and N2 are independent and can run in parallel.
- **V3 must merge after V2, N1 and N2.** V3 is where enforcement starts. Before the node and adapter can answer, every install would revoke every actor.
- N2 depends on N1 only for the dispatch hook (`@6` → per-adapter 142); the two may share one PR if that's simpler.
- The umbrella and each first child carry a native `blockedBy` where one applies (`next-issue.py` doesn't inherit blockers from an umbrella).

## Issue map

| Task | Repo | Issue |
|---|---|---|
| Umbrella | — | zipline#118 |
| V1 | `zl-zpr-visaservice` | zipline#119 |
| V2 | `zl-zpr-visaservice` | zipline#120 |
| N1 | `zl-zpr-core` | zipline#121 |
| N2 | `zl-zpr-core` | zipline#122 |
| V3 | `zl-zpr-visaservice` | zipline#123 |
| I1 | `zl-zpr-core` | zipline#124 |
| Z1 | `zl-zpr-dev-context` | zipline#125 |

---

## Tasks

### V1: bootstrap authentication doesn't expire (`zl-zpr-visaservice`)
- [ ] **Bug test first (it fails today):** an RSA+OIDC actor's `authExpires` after an OIDC `reauthorize` is later than connect + `DEFAULT_AUTH_EXPIRATION` (build on `reauth_fixture`, `connection_control.rs:4068`).
- [ ] Add one helper for the bootstrap authority expiry (far-future, shared with the `vs.zpr` case) and use it at every bootstrap `DEVICE_AUTHORITY` stamp (`:284-288`, `:360-364`, `:450-454`, `:468-472`).
- [ ] Add one helper for the `zpr.vs.bootstrap.ident` expiry: the minimum of the actor's authority expiries, falling back to `DEFAULT_AUTH_EXPIRATION` when there is none. Use it at the three mint sites (`:420-435`, `:1001-1018`, `:1096-1111`).
- [ ] `actor_mgr::refresh_state` (`actor_mgr.rs:73-112`) culls nodes by last-seen age (`DEFAULT_AUTH_EXPIRATION`) instead of authentication expiry.
- [ ] Tests:
  - RSA-only adapter: `authExpires` is far-future and `auth_sweep` never revokes it;
  - OIDC-only: unchanged;
  - node: far-future;
  - `refresh_state` culls a node last seen more than 4 h ago and keeps a recent one.
  - Update `test_connect_auth_expires_*` (`vsapi_worker.rs:1806-1883`).

**Acceptance:** tests pass; OIDC renewal and `auth_sweep` suites pass unchanged; visas still clamp to `MAX_VISA_LIFETIME`.

### V2: `reauthorize` accepts a per-namespace set with an SS arm (`zl-zpr-visaservice`)
- [ ] Replace the one-blob / OIDC-only guard (`connection_control.rs:892-911`) with K3's set rule.
- [ ] Add an SS reauth arm (the counterpart of `reauthenticate_oidc_blob`, `:700`) that reuses `authenticate_ss_blob` (`:499`) and adds the device-authority and skew checks.
- [ ] Re-stamp `device.zpr.authority` in the claim rebuild (`:956-967`) when the device namespace is renewed.
- [ ] Tests, modeled on the OIDC reauth suite (`:4106-4645`):
  - SS renews in place and stamps the current `zpr.vinst`;
  - key removed in the snapshot → `AuthError`;
  - key rotated → `AuthError`;
  - CN mismatch → `AuthError`;
  - skewed `ts` → `AuthError`;
  - no device authority → `ParamError`;
  - set mismatch → `ParamError`;
  - duplicate namespace → `ParamError`;
  - SS+OIDC renews both;
  - policy denial → `PolicyDenied`;
  - flip `…non_oidc_blob_param_error` (`:4558`).
- [ ] Update the `reauthorize_actor` doc comment and the VS-API header comment (`vs.capnp:131-135`) in `zl-zpr-vsapi` as a comment-only change.

**Acceptance:** new tests pass; the OIDC reauth suite passes unchanged.

### N1: node implements `requestAuthentication` and in-place self re-auth (`zl-zpr-core`)
- [ ] `libnode2/src/vss.rs`:
  - add a `VSSMessage::RequestAuthentication(addrs, ack_tx)` variant and a `request_authentication` server method, modeled on `revoke_authentication` (`:338-400`);
  - ack per K1;
  - add the arm to the exhaustive CLI match (`libnode2/src/cli/handler.rs:205-240`).
- [ ] `adapter/ph/src/vss_worker.rs`: dispatch on each address.
  - The node's own address sends `VS2Command::Reauthenticate`.
  - An adapter address calls N2's on-demand hook. Until N2 lands, log it and skip the address, not counting it in `processed`.
- [ ] `libnode2/src/vsconn.rs`:
  - add `VS2Command::Reauthenticate`, which bypasses the `is_connected()` guard (`:487-490`);
  - it re-runs `do_connect` with `StateFlag::HasState` on the existing `vs_service` and swaps `vs_handle` only on success;
  - on failure it logs and keeps the old handle, since the deadline decides;
  - at most one in flight.
- [ ] Remove the stale TODO at `adapter/ph/src/vs_worker.rs:25`.
- [ ] Tests:
  - `@6` with the node's own address triggers exactly one `Reauthenticate`;
  - a second one while in flight is coalesced;
  - `Reauthenticate` success swaps the handle and failure keeps it;
  - visas and adapter peers are untouched;
  - unknown addresses aren't counted in the ack.

**Acceptance:** unit tests pass; against a VS without V3, a manually sent `@6` re-authenticates the node with no tether or visa loss.

### N2: on-demand adapter re-auth with SS (`zl-zpr-core`)
- [ ] Record the link's authenticated namespace set at auth time, alongside `RenewalIdentity` (`link_state.rs:1289-1294, 1403`).
- [ ] Add `pub fn request_renewal_now(&self, asm)`:
  - requires Active and `NodeToAdapter`;
  - sets `renewal_in_flight`, or returns if it's already set;
  - calls `send_renewal_credential_request` (`:2940`);
  - N1's dispatcher reaches it through `peer_table.find` on the actor address (pattern at `vss_worker.rs:95-101`).
- [ ] The deadline-driven `maybe_renew_auth` keeps working for OIDC, and a device-only link never has a deadline due (V1).
- [ ] Adapter `process_renew_auth_request` (`:3003`):
  - build the blob array for the link's namespaces: SS via `BootstrapKey::authenticate_blob`, plus the existing OIDC AuthAgent leg;
  - reply `AuthUnavailable` if any leg is missing.
- [ ] Node `process_renew_auth_response` (`:3157`):
  - decode the array;
  - require the set to equal the recorded set;
  - check each blob with `check_self_signed_blob` (`:1329`) or `check_oidc_blob` (`:1373`);
  - send them all in one `ReauthRequest`, extending the single-OIDC builder at `:2887`.
- [ ] Tests, modeled on the 142/143 suite (`:3628-4557`):
  - an on-demand request sends 142 and is coalesced while in flight;
  - the adapter answers with SS and no agent;
  - an adapter using SS+OIDC but with no agent replies `AuthUnavailable`;
  - set mismatch rejected;
  - wrong-challenge SS rejected;
  - a valid SS reaches `reauthorize` with one SS blob;
  - SS+OIDC reaches it with both.

**Acceptance:** unit tests pass; the existing OIDC renewal tests pass unchanged.

### V3: policy install triggers re-authentication and enforces it (`zl-zpr-visaservice`)
- [ ] **Test first (it fails today):** an adapter whose bootstrap key is removed by a policy install is revoked once `reauth_deadline` has passed.
- [ ] Add a `VssCmd::RequestAuth(addrs)` and `vss_do_request_auths`, modeled on `vss_do_revoke_auths` (`vss_worker.rs:648-681`), with a `VssHandle::request_auths` method.
- [ ] `handle_policy_updated` (`event_mgr.rs:178-317`):
  - after the invalid-node disconnect (`:255-268`) and the setTopology fan-out (`:291-302`), record `(V, T)` and fan out `request_auths` to every node, as `set_services_all_nodes` does;
  - resolve the `:182` and `:363` TODOs.
- [ ] Add a `reauth_deadline` visa-service setting (default 300 s) with the other operational settings. Persist pending `(V, T)` so a VS restart doesn't forget the obligation.
- [ ] `auth_sweep`, second rule:
  - re-send `request_auths` to actors with `zpr.vinst < V` while `now ≤ T`;
  - once `now > T`, revoke them: adapters through the existing batched per-node `revokeAuthentication` (removed only on a positive ack), nodes through `cc.disconnect(.., Admin)`;
  - apply the earliest-unmet-deadline rule and prune satisfied `(V, T)` entries.
- [ ] K2 VS side:
  - a node `Reconnect` authenticate re-approves under the current snapshot (stamping `zpr.vinst`) without disturbing docked adapters, visas or links;
  - handle the duplicate VSS worker at `vsapi_worker.rs:1226-1234`;
  - verify that a node dropped by `cc.disconnect` is refused on its next VS-API call (today its `VSHandleImpl` stays usable, `vsapi_worker.rs:173-176`); fix it if not, with a test.
- [ ] Tests:
  - an actor re-authenticated under V is never revoked;
  - a silent actor is revoked after T and not before;
  - with two installs V1 then V2, re-authenticating under V2 satisfies both and the earlier deadline still applies to laggards;
  - a node that answers is kept and a node that doesn't is disconnected with its adapters;
  - revocation waits for a positive ack;
  - pending `(V, T)` survives a VS restart.

**Acceptance:** tests pass; the policy-install suites pass unchanged.

### I1: integration test (`zl-zpr-core`)
- [ ] Add `integration-test/one-node-policy-reauth-test.sh`, modeled on `one-node-oidc-renewal-test.sh`, with `reauth_deadline` set short (for example 60 s).
  - **Leg 1:** an RSA adapter and the node survive three policy installs with continuous ping and no tether drop. The logs show one node self re-auth and one adapter re-auth per install.
  - **Leg 2:** an OIDC adapter with a refresh token (fake IdP) survives an install.
  - **Leg 3:** install a policy without adapter2's bootstrap key. adapter2 is dropped within `reauth_deadline` plus one sweep period, and the other adapter is unaffected.
  - **Leg 4:** install a policy without the node's bootstrap key. The node is disconnected.
- [ ] Wire it into CI next to the OIDC renewal test (`.github/workflows/adapter.yml:307`).

**Acceptance:** passes locally in the netns tier and in CI.

### Z1: netns tier, docs, plan retirement (`zl-zpr-dev-context`)
- [ ] Add the script to the netns tier's explicit list, and update the drift guard (zipline#103).
- [ ] Docs:
  - `docs/VISA_SERVICE.md`:
    - authentication lifetimes: bootstrap never expires; OIDC per trusted service;
    - replace the unimplemented grace-period paragraph at `:216` with policy-install re-authentication and its deadline;
    - add `reauth_deadline` to the operational bounds (`:370`).
  - `docs/SECURITY_MODEL.md`:
    - the identity-lifetime rule;
    - the SS arm on `reauthorize` in *Silent re-authentication*;
    - "Authentication expiry is enforced" now also covers install-driven revocation.
  - `docs/OIDC.md`:
    - `:485-497` keeps its rationale and adds how policy changes are now enforced;
    - drop the "Device authentication lifetime from policy" Deferred row (it's moot);
    - update the "VS-pushed renewal via `requestAuthentication`" row, which is now done for policy install.
  - `docs/ZPL.md:304`: `default` is ZPR's own trusted service, not a Noise-certificate checker.
- [ ] Retire this plan per "Plan retirement" in `skills/zpr/SKILL.md`: decisions into `docs/VISA_SERVICE.md` `## Design decisions`, delete the file, drop its `AGENTS.md` row, add a row to `docs/plans/README.md`.

**Acceptance:** `zpr-dev validate` is clean; `zpr-dev build` runs the netns tier green at tip, including the new script.

---

## Findings (recorded while planning)

- **Dual-actor expiry pin:** RSA+OIDC actors are swept at connect + 4 h regardless of OIDC renewal. V1 fixes this.
- **Policy install never re-checks authentication** (`event_mgr.rs:182, 363`). A removed bootstrap key never takes effect for a connected actor. V3 fixes this.
- **`requestAuthentication @6` exists in the schema but is unimplemented on both sides.**
- **A node dropped by `cc.disconnect` is never told,** and its VS-API capability may stay usable. V3 verifies and fixes this.
- **`ctype=Reset` orphans adapter actor records** (`actor_mgr.rs:128-141` calls `node_db.remove_node` directly). Out of scope; file separately.
- **No VS-side skew check on the adapter *connect* path.** `ssb.timestamp` is signed but never checked there.
- **`VISA_SERVICE.md:216` describes a grace-period notice that isn't implemented.** Z1 corrects it.
- **`libeval/src/attribute.rs:12` `NEVER_EXPIRES` has an extra `60` factor** (about 6000 years). It's harmless.
- **`default`'s `cert_path` is written to policy but unread** by the visa service.

## Out of scope / deferred

| Item | Why deferred |
|---|---|
| Bootstrap key revocation | Future work; policy-install re-auth covers removal from policy |
| Diffing policy to re-auth only affected actors | One uniform rule is easier to audit; revisit if install-time load matters |
| VS skew check on the adapter *connect* path | Independent hardening |
| `ctype=Reset` orphaned adapter records | Pre-existing bug; separate issue |
| Graceful per-namespace degradation | Already deferred in `OIDC.md`; re-auth is all-or-nothing |

## Resolved while planning

- An earlier draft renewed RSA on a timer, with a policy lifetime knob in `[trusted_services.default]`. It was dropped: timer renewal of a static key adds nothing, while policy-install re-auth enforces what actually changes.
- Scope: nodes and adapters.
- Non-responders: revoked at the deadline.
- OIDC leg: re-proved like every other leg; exact set.
- Targeting: the visa service lists addresses per node.
- Replay: node-minted challenge plus VS skew check; no monotonic `ts`.
