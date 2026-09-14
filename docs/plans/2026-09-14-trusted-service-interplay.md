# Trusted Service Interplay

> **Status: COMPLETE (2026-09-14).** All five issues landed: C1
> (zipline#23, compiler PR #4), V1 (zipline#24, vs PR #9), V2 (zipline#25,
> vs PR #10), V3 (zipline#26, vs PR #12), I1 (zipline#27 — the e2e fixture in
> `zl-zpr-core` and the documentation edits in this repository). The V1
> outcome, including the join-matching answer, is recorded under Finding 3
> below.


## Background

Specific use case:

I have two trusted services, an oidc one for Google, and a file one that
has extra attributes.  The attributes file is being used to add
additional attributes to actors authenticated via google.  To key the
attributes I may use the `email` attribute returned from google.

What seems reasonable:

The relevant parts of the ZPLC file:

```toml
[trusted_services.happyfile]
api = "file"
returns_attributes = ["hair_color -> user.hair_color", "lazy -> #user.lazy"]
expiration_seconds = 3600

[trusted_services.google]
api = "oidc"
issuer = "https://accounts.google.com"
jwks_uri = "https://www.googleapis.com/oauth2/v3/certs"
client_id = "???"
allowed_domains = ["*"]
expiration_seconds = 3600
# service is the ZPR service for the CONNECT proxy. Without it the visa service
# needs direct internet access to hit tge jwks endpoint.
# service = "google-jwks"
returns_attributes = ["sub -> user.sub", "email -> user.email"]
identity_attributes = ["sub"]
```

Then in my `happyfile.json`:

```json
{
  "device.zpr.adapter.cn": {
    "node.zpr.org": {
      "hair_color": ["brown"]
    },
    "user.zpr.org": {
      "hair_color": ["red"],
      "lazy": ["yup"]
    },
    "vs.zpr": {
      "hair_color": ["blond"]
    }
  },
  "user.sub": {
    "asjad912je12j": {
      "lazy": ["yes"]
    }
  }
}
```

The relevant policy statement:
```
allow lazy users to access Web.
```

What is interesting here is that usually the ZPL compiler will factor
out the google trusted service since there is no policy referencing any
of its properties.  However, the visa service will need that trusted
service to obtain the 'user.email' key.

## Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox
> (`- [x]`) syntax for tracking.
>
> **ZPR process rule (`skills/zpr/SKILL.md`):** every GitHub issue gets its own plan posted
> as an issue comment *before* implementation. This document is the **master plan**: it fixes
> the ordering, the cross-repository interface contracts, and the scope and acceptance
> criteria of each issue.

**Goal:** Make the configuration in *Background* work end to end — an `api = "oidc"` service
authenticating the user, and an `api = "file"` service keyed on that service's identity
attribute adding attributes on top.

**Repo state this plan was written against:** `zl-zpr-compiler` `28353e0` (0.17.0),
`zl-zpr-visaservice` `a9a4dcd` (`POLICY_MIN_COMPILER_MINOR = 17`), `zl-zpr-dev-context`
`0ebc1ae`, all on `zipline`.

### Correction to the Background

The Background's closing paragraph says the visa service needs `google` "to obtain the
`user.email` key". It does not: an attribute store is queried by *identity attributes*
only, and `email` is not one — `docs/OIDC.md` requires `identity_attributes = ["sub"]`
and gives the reason (workspace addresses are mutable and reusable, so keying on one
silently transfers a departed employee's access to their replacement). The corrected
`happyfile.json` above keys on `user.sub`. The reason `google` must survive compilation
is more general, and is the subject of Finding 1.

---

## Findings

Three defects were traced through the code. Two are real and in scope; the third was
investigated and withdrawn, and is recorded so it is not re-investigated.

### Finding 1 — the compiler prunes the OIDC provider (confirmed)

`Weaver::resolve_attributes` (`zl-zpr-compiler/src/weaver.rs:592`) marks a trusted service
used only when some ZPL statement references an attribute in that service's
`returns_attributes`. `resolve_trusted_service_providers` (`weaver.rs:1179`) then widens
that set to a fixpoint, but only through *provider* attributes, and skips `file` and
`oidc` outright — neither has providers. In the Background's configuration nothing
references `user.sub` or `user.email`, so `google` is never woven and
`oidc_service_for_issuer` returns `None` for every login attempt.

Naming the provider in policy does not help. `weaver.rs:588` resolves both authority keys
to the **default** trusted service, deliberately and correctly (they are visa-service
markers, not attributes any external service reports). So
`allow user.zpr.authority:google users to access Web` also leaves `google` pruned.

### Finding 2 — runtime attribute chaining (withdrawn; already works)

Keyed on `user.sub`, no visa-service change is needed. The chain, verified end to end:

1. The OIDC blob arm stamps `user.zpr.authority = google` and the mapped `sub` attribute
   into `authd_claims` (`vs/src/connection_control.rs:588-604`).
2. `identity_attributes = ["sub"]` resolves through `google`'s own `returns_attrs` to the
   ZPR key `user.sub`, which lands in `Policy::lookup_identity_keys()`
   (`libeval/src/policy.rs:582`, `:296`).
3. `lookup_identities` intersects that key set with the authenticated claims
   (`vs/src/trusted_services/mod.rs:49`), producing `("user.sub", "asjad912je12j")`.
4. `FileAttributeStore::get_attributes_for_actor` looks up
   `attributes["user.sub"]["asjad912je12j"]` directly — its `ActorAttributes` map is keyed
   by `(identity key, identity value)` exactly for this
   (`vs/src/trusted_services/file_attribute_store.rs:70`).

The same holds on the post-connect refresh path (`vs/src/actor_attributes.rs:132`), where
`user.sub` is already on the actor.

This only works because the query key is a declared identity attribute. Keying a file
store on a non-identity attribute such as `user.email` cannot work by construction: at
connect time the lookup set is computed from `authd_claims` *before* any store is queried,
so an attribute that only exists inside another store's cache is never available as a key.
That is a structural property worth preserving, not a limitation to engineer around.

### Finding 3 — `user.zpr.authority` collides between the two services (to be confirmed by test)

`happyfile`'s `lazy -> #user.lazy` maps to the ZPR key `user.zpr.tag.lazy`
(`vs/src/trusted_services/attribute_mapper.rs:44`). That key starts with `user.`, so
`derive_user_authority` (`vs/src/trusted_services/mod.rs:81`) mints
`user.zpr.authority = happyfile` for it. `google`'s arm has already set the same key to
`google`. `Actor::attrs` is a key-unique map (`libeval/src/actor.rs:124`) and
`approve_connection_detailed` commits claims with `insert` in slice order
(`libeval/src/eval.rs:288`), so the last authority pushed silently overwrites the first.

**Which one wins is not alphabetical.** Stores are queried in `policy.list_services()`
order (`vs/src/trusted_services/factory.rs:95`), which is the compiler's fabric-insertion
order — stable for a given policy, but not sorted by trusted-service id. So the symptom
depends on policy emission order, and a configuration that happens to work today can break
on an unrelated recompile. That makes this a latent bug, not a reliably reproducible one,
and V1's tests must pin the order explicitly rather than rely on the ids.

The consequence is not confined to the attribute itself:

- `allow user.zpr.authority:google users to access Web` stops matching.
- `user.zpr.authority` is fed into `lookup_identities` unconditionally
  (`trusted_services/mod.rs:55`), and `OidcTrustedService::get_attributes_for_actor` gates
  on it: `vouched_here` requires `authority == self.id` (`vs/src/oidc/store.rs:212-217`).
  With `happyfile` installed, the next refresh gets an empty result from `google`, prunes
  `user.sub` and `user.email` as no-longer-vended, and then `happyfile`'s own lookup by
  `user.sub` finds nothing and `#user.lazy` disappears too. **The actor loses `lazy`
  mid-session and the visa is denied.**
- The join decision and the resulting actor can disagree. `match_join_policies` is handed
  the raw claim slice (`libeval/src/policy.rs:189`), so *both* authority values are visible
  while join policies are matched, but only one survives onto the actor. A join policy may
  therefore admit the connection on `google`'s authority and yield an actor carrying
  `happyfile`'s. V1 should confirm or rule this out; it does not change the fix, but it
  does change how the failure reads in the logs.

The Background's own `happyfile.json` triggers this on the device-CN path as well:
`hair_color -> user.hair_color` is a `user.*` attribute, so a device entry alone is enough
to derive the competing authority.

This finding was traced by reading, not by execution. **Task V1 is the gate:** if its test
passes as written against today's code, the finding is wrong and Tasks V2/V3 drop out of
this plan.

#### V1 outcome (2026-09-14, zipline#24, vs PR #9): Finding 3 CONFIRMED by execution

All three defect tests failed exactly as predicted against the pre-fix code:

- **Connect path:** the actor's `user.zpr.authority` came out `["happyfile"]`
  where `["google"]` was expected — the decorating store's derived authority,
  pushed after the OIDC arm's stamp, won the last-writer race in `Actor::attrs`.
- **Refresh path, symptom 1:** a TTL refresh touching only the decorating store
  displaced the authority the same way.
- **Refresh path, symptom 2:** from the displaced state, google's `vouched_here`
  gate answered empty → `user.sub` was pruned as no-longer-vended → the file
  store's lookup missed → `user.zpr.tag.lazy` was lost. The actor lost `lazy`
  mid-session, as predicted.

**The join-matching answer** (Finding 3's last bullet): a join policy
conditioned on `user.zpr.authority:google` does **NOT** match when the
displaced `happyfile` value is also in the claim slice. `match_join_policies`
does receive both authority values, but `JPolicy::matches`
(`libeval/src/joinpolicy.rs:124`) evaluates *every* attribute whose key matches
a condition — its comment "We assume the key appears only once in the incoming
list" names the assumption this defect violates — so the `happyfile` entry
fails the `Eq google` check and vetoes the whole policy. The defect therefore
surfaced at connect time as a `policyDenied` refusal ("no join policy admits
this connection"), **not** as a silently-connected actor carrying the wrong
authority. An operator debugging it sees a refused join against a policy that
looks like it should match.

V2 and V3 fixed it (three-arg `derive_user_authority` with the `!=` guard;
authority threaded through connect first-wins and refresh); the V1 tests were
un-ignored and pass, and I1's end-to-end fixture re-asserts the invariant
across a live refresh cycle.

---

## Global constraints

- **`docs/OIDC.md`'s `identity_attributes = ["sub"]` rule stands.** Nothing in this plan
  relaxes it, and no new mechanism lets a store be keyed on a non-identity attribute.
  `sub` remains the only thing the network treats as the user's identity.
- **No schema change.** `zl-zpr-policy`'s `policy.capnp` is untouched, so there is no
  version coupling between the two code changes and no `zpr-common` bump. Do not
  introduce one; if a task appears to need one, stop and revisit the design.
- **No new `.zplc` surface.** The Background's configuration must compile unmodified.
- **Build gate on every PR:** `make check` and `make test`; warnings are errors. Every
  non-trivial function carries a doc comment (`AGENTS.md`). A found bug gets a failing
  test before the fix.
- **Fixture naming in `zl-zpr-compiler/test-data`:** `test-*.zpl` must compile and
  `zpdump`; deliberately failing fixtures are `bad-*.zpl`.
- **Never edit or commit a repo's generated `AGENTS.md`/`CLAUDE.md`.** Never let a
  `Cargo.lock` with a local path reach a PR.

---

## Cross-repository interface contracts

**There are none.** This is the plan's most useful property and the reason the two code
tasks are independent:

| Boundary | Change |
|---|---|
| `policy.capnp` (`zl-zpr-policy`) | none — `TrustedService.identityAttrs` already carries everything both sides need |
| `zpr-common` Rust mirrors | none |
| Compiler → visa service | none. The compiler emits *more* `TrustedService` records than before; the visa service already handles any number |
| Version floors | unchanged: compiler `0.17.0`, `POLICY_MIN_COMPILER_MINOR = 17` |

An older visa service loading a policy from a fixed compiler simply gets an OIDC provider
it would previously have been missing. A fixed visa service loading a policy from an older
compiler behaves exactly as it does today.

---

## Dependency graph and order

```
C1 (compiler: retain identity vendors) ─┐
                                        ├──> I1 (end-to-end integration)
V1 (test: prove Finding 3) ──> V2 ──> V3┘
```

`C1` and `V1` have no dependency on each other and may be worked in parallel. `V2` and `V3`
are gated on `V1` confirming Finding 3. `I1` needs both repos fixed.

## Issue map

| ID | Repo | Title | Blocked by |
|---|---|---|---|
| C1 | zl-zpr-compiler | Never prune a trusted service that vends identity attributes | — |
| V1 | zl-zpr-visaservice | Failing test: a decorating file store displaces the authenticator's `user.zpr.authority` | — |
| V2 | zl-zpr-visaservice | `derive_user_authority`: the authenticating service keeps the authority | V1 |
| V3 | zl-zpr-visaservice | Connect and refresh paths pass the existing authority to `derive_user_authority` | V2 |
| I1 | zl-zpr-dev-context | End-to-end fixture and docs for the oidc + file interplay | C1, V3 |

---

## Phase C — Compiler (`zl-zpr-compiler`)

### Task C1: Never prune a trusted service that vends identity attributes

**Scope.** A trusted service declaring a non-empty `identity_attributes` is a query key for
every attribute store in the policy — a `file` store's JSON is keyed by identity attribute
and value — so whether ZPL references *its own* returned attributes says nothing about
whether the visa service needs it. Retain it unconditionally.

Only `oidc` and `validation/2` can reach this rule: `config/trusted_service.rs:166` rejects
`identity_attributes` on `api = "file"`, and `:550` rejects it on `default`. The rule
therefore reads as intended — **identity vendors are never pruned; attribute overlays still
are.**

**Steps.**

- [x] Write the failing test in `weaver.rs`, mirroring the existing
      `test_trusted_service_transitive_use`: declare an `oidc` service and a `file`
      service, reference only the file service's attribute, assert both are woven.
      Confirm it fails today with only the file service present.
- [x] Add `Weaver::retain_identity_vendors(&mut self, config, ctx)`. Enumerate
      `config.must_get_keys("/trusted_services")` **sorted** (deterministic diagnostics),
      read `/trusted_services/{ts}/id_attributes` as `ConfigItem::KeySet` (the accessor
      already used at `weaver.rs:1418`), and call `add_used_trusted_service` for any
      non-empty list. Skip `zpl::DEFAULT_TRUSTED_SERVICE_ID`.
- [x] Call it from `add_trusted_services` **immediately before**
      `resolve_trusted_service_providers`, so a newly retained service's own provider
      attributes still resolve through the existing fixpoint (this matters for a
      `validation/2` identity vendor and for an `oidc` service naming a JWKS proxy).
- [x] Emit `ctx.info()` naming each service retained by this rule rather than by an
      attribute reference. `info`, not `warn`: for `oidc` this is now the normal case, and
      it must not trip `--Werror`.
- [x] Add `test-data/test-oidc-file-interplay.zpl` / `.zplc` from the Background, and
      assert via `zpdump` that both trusted services appear in the binary policy.

**Acceptance criteria.**

- The Background's `.zplc` compiles with both `google` and `happyfile` in the woven fabric.
- A `file` service unreferenced by ZPL is still pruned (the existing
  `test_trusted_service_transitive_use` sibling test must keep passing unchanged).
- `make check` and `make test` clean.

**Known trade-off, to be stated in the doc comment.** A `validation/2` service that
declares `identity_attributes` and is genuinely dead now gets woven, giving the visa
service a network dependency it previously pruned. This is the price of the rule and it is
the right price: the compiler cannot see a file store's JSON keys, so it cannot prove an
identity vendor is unused. The `ctx.info()` line makes each such retention visible at build
time.

---

## Phase V — Visa service (`zl-zpr-visaservice`)

### Task V1: Failing test — a decorating file store displaces the authenticator

**This task is the gate for V2 and V3.** It changes no production code.

- [x] Unit test in `vs/src/trusted_services/mod.rs` (alongside the existing tests at
      `:149-208`): `derive_user_authority("happyfile", [user.zpr.tag.lazy])` currently
      returns `Some(happyfile)` with no regard for an authority already held.
- [x] Connect-path test in `vs/src/connection_control.rs`, using the fake trusted-service
      harness at `:1743`: authenticate via an OIDC blob, have a file store vend a `user.*`
      attribute, and assert the resulting actor's `user.zpr.authority == "google"`.
      Expected to fail today with `"happyfile"`. Pin the store order in the harness
      explicitly — do not rely on the ids sorting the way you want.
- [x] While in there, check whether a join policy conditioned on
      `user.zpr.authority:google` matches even when the actor ends up with `happyfile`
      (see Finding 3's last bullet), and record the answer in this document.
- [x] Refresh-path test in `vs/src/actor_attributes.rs`: after a refresh in which both
      stores answer, assert the authority is still `google` and that `user.sub` and the
      file store's tag both survive.

**If these pass unchanged against today's code, stop.** Finding 3 is wrong; close V2 and
V3, and record the correction in this document.

### Task V2: `derive_user_authority` — the authenticating service keeps the authority

- [x] Change the signature in `vs/src/trusted_services/mod.rs:81`:

```rust
pub(crate) fn derive_user_authority(
    source_id: &str,
    ts_attrs: &[Attribute],
    existing_authority: Option<&str>,
) -> Option<Attribute>
```

- [x] Return `None` when `existing_authority` is `Some(other)` and `other != source_id`.

**The `!=` is load-bearing.** When the existing authority *is* this source, derivation must
still proceed: that is how the authenticating service re-stamps its expiry against its own
user record on every refresh (issue #324's purpose — the authority must not outlive the
record it vouches for). Blocking on `Some(_)` unconditionally would freeze the expiry and
let the authority lapse mid-session. Cover both arms with tests.

- [x] Extend the doc comment to state the ownership rule and why: a service that decorates
      an already-identified actor is not the authority for that identity.

### Task V3: Pass the existing authority at both call sites

- [x] `vs/src/connection_control.rs:735` — read the current authority out of
      `authd_claims` into an **owned** `Option<String>` before the call; the loop body
      pushes into `authd_claims`, so holding a borrow across the call will not compile.
      Recompute it **inside** the loop, per iteration.
- [x] `vs/src/actor_attributes.rs:166` — read from
      `actor.get_attribute(key::USER_AUTHORITY)`.
- [x] Confirm the V1 tests now pass.

**Behaviour change to note in the PR.** Two file stores both vending `user.*` with no IdP
present go from last-wins to first-wins in `policy.list_services()` order. Both were
arbitrary and neither is alphabetical; the gain is that the *authenticated* identity can no
longer be displaced at all, which is the property that matters. Two file stores with no IdP
still resolve arbitrarily between themselves — acceptable, and out of scope here. The
single-file-store case (issues #144 / #324) is unaffected: with no prior authority,
derivation proceeds exactly as today.

**Acceptance criteria for V2+V3.**

- All three V1 tests pass.
- The authority still refreshes its expiry on each pass from its own source.
- `make check` and `make test` clean.

---

## Phase I — Integration and documentation

### Task I1: End-to-end fixture and docs

- [x] Integration test exercising the Background's configuration against the fake IdP
      (the `D5` harness from the OIDC plan): connect with a Google token, confirm the
      actor carries `user.sub`, `user.email`, `user.zpr.tag.lazy` and
      `user.zpr.authority = google`, and that `allow lazy users to access Web` issues a
      visa. Let at least one refresh cycle elapse and re-assert — that is what catches a
      regression of Finding 3, which is invisible on the connect path alone.
- [x] `docs/ZPL.md` — document that a trusted service vending identity attributes is
      always woven, and why reference-based pruning cannot decide the question.
- [x] `docs/OIDC.md` — `## Implementation status`: an `api = "oidc"` service is retained
      regardless of whether policy references its attributes.
- [x] `docs/SECURITY_MODEL.md` — the `user.zpr.authority` ownership rule: the service that
      verified the credential holds it; a service that only adds attributes never displaces
      it.
- [x] `docs/VISA_SERVICE.md` — record that attribute stores are queried by identity
      attributes only, and the structural reason (the lookup set is fixed before any store
      is queried).
- [x] Update this document with the V1 outcome.

---

## Out of scope

- **Keying an attribute store on a non-identity attribute** (the Background's original
  `user.email`). Rejected on the merits, and structurally impossible on the connect path;
  see Finding 2.
- **Making `user.zpr.authority` multi-valued.** It would model reality more faithfully and
  would fix the `vouched_here` gate more directly, but it touches evaluation semantics and
  every consumer of the key. Revisit only if a deployment genuinely needs two concurrent
  user authorities.
- **Making `user.zpr.authority:<id>` resolve to that trusted service** in the weaver
  instead of to `default`. Task C1 makes it unnecessary, and `weaver.rs:588`'s current
  behaviour is correct on its own terms.

