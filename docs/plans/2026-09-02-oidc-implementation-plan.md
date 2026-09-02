# OIDC Authentication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **ZPR process rule (`zpr-dev-context/skills/zpr/SKILL.md`):** every GitHub issue gets its own plan posted as an issue comment *before* implementation. This document is the **master plan**: it fixes the ordering, the cross-repository interface contracts, and the scope and acceptance criteria of each issue. When an issue is picked up, expand its section here into the bite-sized TDD plan on the issue, against the code as it is *then*. Several of the spec's line references were already stale by the time this plan was written (see *What changed since the spec*), which is why per-issue plans are deferred to pickup.

**Goal:** Add OpenID Connect (Google first) as a second, independently-expiring authentication method for ZPR actors, with the adapter as Relying Party and the visa service validating `id_token`s offline against a cached JWKS.

**Architecture:** `ph-cli` runs the RFC 8252 native-app flow (PKCE, loopback redirect) and returns an `id_token` to the `ph` daemon over the existing capnp admin RPC; `ph` sends it to the node as an `OIDC` auth blob alongside (or instead of) the RSA bootstrap blob; the node forwards both to the visa service as `ConnectRequest.blobs`; the visa service validates the token against Google's JWKS (seeded from policy, refreshed through an on-net `CONNECT` proxy), stamps `user.zpr.authority` with its own expiry, and installs the mapped claims through the normal trusted-service attribute path. Policy declares Google as `api = "oidc"` trusted service in the ZPLC.

**Tech Stack:** Rust 2024, Cap'n Proto (`zpr-policy`, `zpr-vsapi` via `zpr-common`), `jsonwebtoken` 10 (already in `vs`), `reqwest` 0.13 (already in workspace locks of both `vs` and `ph`), `sha2`/`rand`/`base64`/`url` (already in `zpr-core` lock), shell integration tests on Linux netns.

**Spec:** `docs/OIDC.md` in this repository (the `## PLAN and DETAILS` section governs; this plan supersedes it where the two conflict, and each such point is called out).

**Repo state this plan was written against:** `zpr-compiler` `c58b532` (0.16.0), `zpr-visaservice` `5b73daa` (0.18.0), `zpr-core` `2e17c92`, `zpr-common` `v0.25.1`, `zpr-policy` `v0.10.0`.

## Global Constraints

- **Security review posture:** every item under *Security requirements* in the spec is non-negotiable; none may be simplified away. In particular: match on `hd` never on email domain, absent `hd` fails; `email_verified` required before mapping `email`; validate `iss`, `aud`, `exp`, `nonce`, `kid`; reject `alg: none` and anything outside an explicit allowlist (`RS256` only for Google); never log tokens, codes, verifiers, or refresh tokens; `CONNECT` forward proxy only, JWKS URL never rewritten; PKCE S256 mandatory; loopback on `127.0.0.1` only, single use, `state` verified; ordinary TLS verification against system roots on every Google leg.
- **`client_id` and `allowed_domains` come from policy, never from the blob.** The blob's `issuer` is a selector only.
- **No new dependency trees** unless an issue says so explicitly. The only planned additions are `reqwest.workspace = true` in `vs/Cargo.toml` and `reqwest`, `url`, `serde_json`, `base64`, `sha2`, `rand` in `adapter/cli/Cargo.toml` (all already in the respective `Cargo.lock`s).
- **Build gate on every PR:** `make check` and `make test` in the repo; warnings are errors. Every non-trivial function has a doc comment (`AGENTS.md`). A found bug gets a failing test before the fix.
- **Attribute names (fixed):** `user.zpr.authority`, `device.zpr.authority`, `device.zpr.adapter.cn`. Authority *values*: `zpr-bootstrap` for the RSA method, the trusted-service id (e.g. `google`) for OIDC.
- **Version floors after this work:** compiler `0.17.0`, visa service `POLICY_MIN_COMPILER_MINOR = 17`, `zpr-common` `v0.26.0`, visa service release `v0.19.0`.
- **Fixture naming in `zpr-compiler/test-data`:** `test-*.zpl` must compile and `zpdump`; deliberately failing fixtures are `bad-*.zpl`.
- **Never edit or commit a repo's generated `AGENTS.md`/`CLAUDE.md`.** Never let a `Cargo.lock` with a local path reach a PR.

---

## What changed since the spec (verified 2026-09-02)

| Spec assumption | Reality on `main` today | Effect on plan |
|---|---|---|
| Prerequisite zpr-compiler#144 must land first | **Done.** PR #145 merged; compiler is 0.16.0 and emits `has user.zpr.authority` / `has device.zpr.authority` (`src/zpl.rs:56-57`, `src/allow.rs:57-84`). | Removed from plan. |
| Prerequisite zpr-visaservice#310 must land first | **Done.** PR #320 (+ #322 fix) merged. Lookups use `Policy::lookup_identity_keys()` + `lookup_identities()` (`connection_control.rs:451-473`); revision cache is keyed by ZPR address and purged in `disconnect`; the CN-is-None early return in `actor_attributes.rs` is gone and has a regression test using a `user.sub`-shaped actor. | Removed. Spec's "revision cache" and "attribute refresh no-op" rows in *Visa service changes* are already satisfied. |
| zpr-compiler#146 (reserved `zpr.` namespace) | **Open, PR #147 in review**, CI green. | Tracked as OIDC-B0; not on the critical path but should merge before B1. |
| The marker attribute name was undecided (`user.authority` vs `user.zpr.authority`) | Compiler emits `user.zpr.authority`. Visa service still installs `zpr.authority` (`libeval/src/attribute.rs:31`) and pins `POLICY_MIN_COMPILER_MINOR = 15`. | **A compiler-0.16 policy with `allow users …` currently matches nothing on the visa service.** OIDC-C0 (namespaced authority + min compiler bump) is the first, urgent visa-service issue. |
| "CN is unconditionally pushed into `authd_claims` at `connection_control.rs:424`" | No longer. `authorize_connection` deliberately does not promote the CN (`:434-437`); only the RSA path (`:370`) and the VS-self path (`:293`) push it. | Spec item collapses to "keep it that way, add the user-only regression test". |
| Spec proposes a new `oidc` arm on `AuthBlob` | Correct; today's arms are `ss @0` and `ac @1` (`vs.capnp:384-389`). `ReauthRequest.blobs` (`vs.capnp:379-382`) must also accept it. | Included in OIDC-A2. |
| Spec says "reason codes on authentication failure — VSAPI" needs adding | `Result(T)` already carries `Error { code :ErrorCode, message, retryIn }` with `authError`, `invalidSignature`, `temporarilyUnavailable`, `paramError` (`vs.capnp:592-610`). Only "authenticated but policy denied" is missing. | A2 adds one enum value, `policyDenied @10`. The real gap is on the **node**, which drops the error and hardcodes `ResponseCode::Success` (`link_state.rs:1343`). |
| "Which HTTP client does the visa service use, and does it support CONNECT?" | `vs` has no HTTP client; `reqwest` is a workspace dependency used by `vs-admin`. `reqwest::Proxy::https(...)` does `CONNECT` with end-to-end TLS. `jsonwebtoken` 10 is already a `vs` dependency (encode-only today). | Resolved: add `reqwest.workspace = true` to `vs`. No new lock entries. |
| Google Desktop clients and `client_secret` | Google's native-app doc marks `client_secret` *optional* and exempts only Android/iOS/Chrome clients, not Desktop. | Carry an **optional, non-secret** `client_secret` end to end (ZPLC → policy → VS → node → adapter). Settle in the manual Google checklist (D5). |
| Nonce comes "from the visa service via `ZdpInitAuthenticationPayload`" | The payload's 8-byte `nonce` is the **node's** HMAC challenge, not a visa-service value (`auth.rs:79-90`). The visa service never sees the link key. | Nonce contract (below): adapter derives the OIDC nonce from the node's 48-byte challenge; the node verifies the challenge exactly as it does for SS blobs and forwards the expected nonce to the visa service. |
| `ACTOR_AUTHENTICATION_TIMEOUT` unmentioned | The **node** arms a 120 s timer (`config.rs:68`, `link_state.rs:1241`) for the whole out-of-band auth; a human at a consent screen will exceed it. | D1 raises it; D2 adds the adapter-side interaction timeout under it. |
| `HARD_CODED_BAS_TLS_CERT_PEM` should be removed | It **expired 2026-04-16**; the BAS path only works because of `danger_accept_invalid_certs(true)`. Also `visa_mgmt.rs:105-108` parses a `SocketAddr` string as `IpAddr`, so every AC blob carries `asa_addr = 0.0.0.0`. | The BAS/AC path is already dead. D4 deletes it. |
| `[bootstrap] expiration_seconds` in ZPLC | Not in any schema; needs a new `Policy`-level capnp field and compiler support. | **Deferred** (OIDC-X1). Device lifetime stays `DEFAULT_AUTH_EXPIRATION` for now. |

---

## Cross-repository interface contracts

These are the things that must be agreed before parallel work starts. Everything below is fixed by this plan; a change to any of it is a plan deviation to be recorded on the umbrella issue.

### 1. `policy.capnp` (zpr-policy) — OIDC-A1

```capnp
# Additional details for Trusted Services
struct TrustedService {
  serviceId         @0 :Text; # Copied from Service.id
  expirationSeconds @1 :UInt32;
  returnsAttrs      @2 :List(AttrMapping);
  identityAttrs     @3 :List(Text);
  oidc              @4 :OidcConfig;   # populated only when Service.kind.trusted == "oidc"
}

# Configuration for an `api = "oidc"` trusted service. The visa service performs
# no OIDC discovery: everything it needs is pinned here and signed with the policy.
struct OidcConfig {
  issuer             @0 :Text;        # e.g. https://accounts.google.com
  jwksUri            @1 :Text;        # pinned; e.g. https://www.googleapis.com/oauth2/v3/certs
  clientId           @2 :Text;
  clientSecret       @3 :Text;        # optional; NOT a secret for public clients (RFC 8252 s8.5); "" = none
  scopes             @4 :List(Text);  # e.g. ["openid","email","profile"]
  allowedDomains     @5 :List(Text);  # matched against the `hd` claim; ["*"] = any account (explicit opt-in)
  maxAuthAgeSeconds  @6 :UInt32;      # 0 = unlimited
  allowOfflineAccess @7 :Bool;
  seedJwks           @8 :Text;        # JSON JWKS document for cold start
  jwksProxyService   @9 :Text;        # fabric service id of the CONNECT proxy; "" = direct egress
}
```

`Service.kind.trusted @3 :Text` carries `"oidc"`; no union change. `expirationSeconds` on the record is the **user authentication lifetime** for an OIDC service (`auth_time + expirationSeconds`), reusing the existing field.

### 2. `vs.capnp` (zpr-vsapi) — OIDC-A2

```capnp
struct AuthBlob {
  union {
    ss   @0 :SelfSignedBlob;
    ac   @1 :AuthCodeBlob;     # legacy BAS; removed in OIDC-D4 follow-up
    oidc @2 :OidcBlob;
  }
}

struct OidcBlob {
  issuer  @0 :Text;   # selector: which declared trusted service to validate against. Never a trust input.
  idToken @1 :Text;   # the JWT, verbatim
  nonce   @2 :Text;   # expected `nonce` claim (see nonce contract). The node has verified its freshness.
}

enum ServiceT {
  actorAuthentication @0;
  oidcAuthentication  @1;
}

struct ServiceDescriptor {
  stype      @0 :ServiceT;
  serviceId  @1 :Text;
  serviceUri @2 :Text;            # for oidcAuthentication: the issuer URL
  zprAddr    @3 :IpAddr;          # unspecified (::) for off-net services
  oidc       @4 :OidcClientConfig; # set when stype == oidcAuthentication
}

# What a Relying Party needs. All public data.
struct OidcClientConfig {
  issuer             @0 :Text;
  clientId           @1 :Text;
  clientSecret       @2 :Text;   # "" = none
  scopes             @3 :List(Text);
  allowOfflineAccess @4 :Bool;
}

enum ErrorCode {
  # ... existing @0..@9 unchanged ...
  policyDenied @10;   # authentication succeeded; no join policy admits this endpoint
}
```

Visa-service error mapping for the connect path (used by C5, consumed by D1/D2):

| Condition | `ErrorCode` |
|---|---|
| blob presented, JWT fails any validation (sig, `iss`, `aud`, `exp`, `nonce`, `kid`, alg) | `invalidSignature` |
| `hd` absent or not in `allowedDomains`; `auth_time` older than `maxAuthAgeSeconds` | `authError` |
| blob names an `issuer` with no declared trusted service | `paramError` |
| no key set available at all (no seed, fetch never succeeded) | `temporarilyUnavailable` (with `retryIn`) |
| all blobs valid, `approve_connection` finds no matching join policy | `policyDenied` |

### 3. Rust mirrors in `zpr-common` — OIDC-A3

- `policy_types::TrustedService` gains `pub oidc: Option<OidcConfig>`; `write_to`/`TryFrom` round-trip it. New `pub struct OidcConfig` with the ten fields above (`String`, `Vec<String>`, `u32`, `bool`); `client_secret: Option<String>` and `jwks_proxy_service: Option<String>` map `""` ↔ `None`.
- `vsapi_types::AuthBlob` gains `Oidc(OidcBlob)`; `pub struct OidcBlob { pub issuer: String, pub id_token: String, pub nonce: String }`.
- `vsapi_types::ServiceDescriptor` **stops dropping `stype`**: add `pub stype: ServiceT` (enum `ActorAuthentication`, `OidcAuthentication`) and `pub oidc: Option<OidcClientConfig>`; `get_socket_addr()` returns `None` for `OidcAuthentication`.
- Tag `v0.26.0`.

### 4. ZDP adapter→node blob (zpr-core) — OIDC-D1/D2

The `AcquireZprAddressRequest` blob stays "base64 of JSON", but the JSON may now be a **top-level array** of blob objects. A bare object still parses (legacy).

```json
[
  {"blob_type":"SS","ts":1756850000,"cn":"laptop1.zpr","challenge":"<b64 48 bytes>","sig":"<b64>"},
  {"blob_type":"OIDC","issuer":"https://accounts.google.com","id_token":"<JWT>","challenge":"<b64 48 bytes>"}
]
```

`BLOB_TYPE_OIDC = "OIDC"`. `ZdpOidcBlob { blob_type, issuer, id_token, challenge }`.

**Nonce contract.** `challenge` is the same 48-byte `nonce||ctime||hmac` from `ZdpInitAuthenticationPayload` that SS blobs carry. The adapter sends `nonce = base64url_nopad(SHA-256(challenge_bytes))` as the OIDC `nonce` request parameter. The node verifies the challenge HMAC and age with the existing `verify_blob_challenge` logic (`auth.rs:218`), recomputes the same hash, and puts it in `OidcBlob.nonce`. The visa service requires `id_token.nonce == OidcBlob.nonce`. Result: the token is bound to this link's authentication attempt, freshness is enforced by the node (the only party holding the link key), and the visa service needs no new state.

### 5. ZDP node→adapter IdP advertisement (zpr-core) — OIDC-D1/D2

New TLV type `DataType::OIDC_IDP = 9` in `adapter/ph/src/tlv.rs`, value `TlvValue::Str` holding JSON:

```json
{"issuer":"https://accounts.google.com","client_id":"1234-abc.apps.googleusercontent.com","client_secret":"","scopes":["openid","email","profile"],"allow_offline_access":false}
```

One TLV per OIDC `ServiceDescriptor` the visa service pushed. The existing `ASA` TLV keeps carrying `SocketAddr`s for on-net auth services (legacy BAS) until D4 removes them.

### 6. Admin RPC (zpr-core `adapter/admin-api/cli.capnp`) — OIDC-D2/D3

```capnp
interface CmdLineInter {
    # ... @0..@16 unchanged ...
    startLink @12 (id: UInt32, authAgent: AuthAgent) -> (result: SuccessOrError);  # authAgent added; optional (null) for device-only links
}

# Provided by ph-cli; called by ph when a credential requiring a user session is needed.
interface AuthAgent {
    getOidcCredential @0 (issuer :Text, clientId :Text, clientSecret :Text, scopes :List(Text),
                          allowOfflineAccess :Bool, nonce :Text, interactive :Bool)
                      -> (result :SuccessOrError, idToken :Text);
}
```

Adding a parameter to an existing method is wire-compatible in Cap'n Proto (params are a struct). `interactive = false` means "satisfy from a stored refresh token or fail"; the agent must never open a browser on a non-interactive request.

### 7. Attributes installed by the visa service — OIDC-C0/C5

| Attribute | When | Value | Expiry | Identity key? |
|---|---|---|---|---|
| `device.zpr.authority` | RSA (`SS`) blob validated | `zpr-bootstrap` | `DEFAULT_AUTH_EXPIRATION` (unchanged, 4 h) | yes (device namespace) |
| `device.zpr.adapter.cn` | RSA blob validated | CN | as today | yes (as today) |
| `user.zpr.authority` | OIDC blob validated | trusted-service id (`google`) | `auth_time + expirationSeconds` (fallback anchor: `iat` when `auth_time` absent — Google omits it unless `max_age` was requested) | yes (user namespace) |
| mapped claims (`user.oidc-subject`, `user.email`, `user.domain`, …) | OIDC blob validated | from token via `returns_attributes` | same as `user.zpr.authority` | `sub`'s mapping, via `identity_attributes` |

`libeval::attribute::key` gains `USER_AUTHORITY = "user.zpr.authority"`, `DEVICE_AUTHORITY = "device.zpr.authority"`, `AUTHORITY_METHOD_BOOTSTRAP = "zpr-bootstrap"`. `key::AUTHORITY` (`zpr.authority`) is **deleted**. `get_authentication_expiration` takes the minimum over both authority attributes and the identity keys (whole-actor expiry; per-namespace graceful degradation stays deferred per the spec).

### 8. Release choreography

```
zpr-policy tag ──┐
zpr-vsapi  tag ──┴─► zpr-common v0.26.0 ─► zpr-compiler 0.17.0 (B2)
                                        ─► zpr-visaservice (C1..C5) ─► v0.19.0-rc.1 tarball ─► zpr-core D1..D5 (VISA_SERVICE_RELEASE = v0.19.0-rc.1)
                                                                    ─► v0.19.0 after core integration test passes ─► core bumps pin to v0.19.0
```

The `-rc.1` step exists because `zpr-core` CI can only consume a **published release tarball** (`.github/workflows/adapter.yml:39`, `robinraju/release-downloader`), there is no release automation in `zpr-visaservice`, and we should not cut a final visa-service release before the integration test has run against it. Precedent: `v0.8.0-rc.1`. Locally, `VS_BIN=…/zpr-visaservice/target/debug/vs integration-test/one-node-test.sh` skips the tarball entirely.

---

## Dependency graph and order

```
            ┌────────── B0 (#146 / PR #147, in review) ──────────┐
            │                                                    ▼
   A1 ─┐    │   B1 zplc: api="oidc" parse+validate ──────────► B2 zplc: codegen, weave, zpdump, 0.17.0
   A2 ─┼─► A3 zpr-common v0.26.0 ─┬──────────────────────────────┘
            │                     ├─► C1 multi-blob connect ─┐
   C0 VS namespaced authority ────┤                          ├─► C5 OIDC connect arm + zpt tests ─► VS v0.19.0-rc.1
   C2 VS JWT validation (pure) ───┼─► C3 JWKS source ────────┤
                                  └─► C4 oidc TS + descriptor┘
                                  └─► D1 node: schema bump, TLV, blob array, error reason, timeout
                                       D2 adapter FSM + AuthAgent RPC ─┐
   D3 ph-cli RP flow (standalone first) ───────────────────────────────┴─► D5 fake-IdP integration test, CI pin ─► VS v0.19.0 final
                                                                      D4 delete BAS/AC legacy (any time after D2)
```

**Start immediately, in parallel:** A1, A2, B1, C0, C2, D3 (against a local fake IdP, as a standalone `ph-cli oidc-login` debugging subcommand that D2 later wires to `AuthAgent`).

**Critical path:** A1/A2 → A3 → C1/C3/C4 → C5 → rc tarball → D1/D2 → D5.

---

## Issue map

Umbrella: **zpr-visaservice#317** (exists, body is a placeholder). Sub-issues below, one per row; GitHub sub-issues may live in any repo of the org. Suggested titles are final; bodies are the sections that follow.

| ID | Repo | Title | Blocked by |
|---|---|---|---|
| A1 | zpr-policy | Add `OidcConfig` to `TrustedService` | — |
| A2 | zpr-vsapi | Add `OidcBlob`, `OidcClientConfig`, `ServiceT.oidcAuthentication`, `ErrorCode.policyDenied` | — |
| A3 | zpr-common | OIDC schema bump: Rust mirrors for OidcConfig/OidcBlob/ServiceDescriptor, tag v0.26.0 | A1, A2 |
| B0 | zpr-compiler | (#146, PR #147) Reserve the `zpr.` sub-namespace from declared trusted services | — |
| B1 | zpr-compiler | `api = "oidc"` trusted-service configuration: parsing and validation | B0 (soft) |
| B2 | zpr-compiler | `api = "oidc"`: emit `OidcConfig`, weave the JWKS-proxy rule, `zpdump`, bump to 0.17.0 | A3, B1 |
| C0 | zpr-visaservice | Namespaced authority attributes and compiler 0.16 floor (follow-up to zpr-compiler#145) | — |
| C1 | zpr-visaservice | Accept multiple auth blobs; presented-and-invalid fails, absent is not a failure | A3, C0 |
| C2 | zpr-visaservice | Offline `id_token` validation module with table-driven vectors | — |
| C3 | zpr-visaservice | JWKS key source: policy seed, `CONNECT`-proxied refresh, stale tolerance | A3, C2 |
| C4 | zpr-visaservice | `oidc` trusted-service implementation and off-net IdP `ServiceDescriptor` | A3 |
| C5 | zpr-visaservice | OIDC blob on the connect path: authority stamping, claim mapping, error codes, `zpt` tests; min compiler 0.17 | C1–C4, B2 |
| D1 | zpr-core | Node: zpr-common v0.26, `OIDC_IDP` TLV, blob arrays, forward OIDC blobs, propagate failure reason, auth timeout | A3 |
| D2 | zpr-core | Adapter: `AuthAgent` RPC, `WaitForUserAuth` state, interaction timeout, failure reasons to CLI, AAA gate | D1 |
| D3 | zpr-core | `ph-cli`: OIDC Relying Party flow (`connect`, `auth-agent`, `--no-browser`) | — (D2 for wiring) |
| D4 | zpr-core | Remove BAS/`OAuthRsa` legacy: hardcoded cert, `danger_accept_invalid_certs`, AC blob | D2 |
| D5 | zpr-core | Fake-IdP integration test, `VISA_SERVICE_RELEASE` bump, manual Google checklist | C5 release, D1–D3 |
| X1 | zpr-compiler + zpr-policy + zpr-visaservice | `[bootstrap] expiration_seconds` (device auth lifetime knob) | deferred |
| X2 | zpr-visaservice | Per-namespace graceful degradation on user-auth expiry | deferred |

---

## Phase A — Schemas and shared types

### Task A1: `policy.capnp` — `OidcConfig`

Repo `zpr-policy`. **Files:** Modify `policy.capnp:77-83`.

**Produces:** the `TrustedService.oidc @4` field and `struct OidcConfig` exactly as in *Contract 1*.

- [ ] **Step 1:** Add the struct and field verbatim from Contract 1, with the comments.
- [ ] **Step 2:** `capnp compile -o- policy.capnp >/dev/null` succeeds (there is no other build here).
- [ ] **Step 3:** Commit `feat: OidcConfig for api="oidc" trusted services`; PR; after merge tag `v0.11.0`.

**Acceptance:** schema compiles; ordinals `@4` on `TrustedService` and `@0..@9` on `OidcConfig` match Contract 1 exactly.

### Task A2: `vs.capnp` — `OidcBlob`, `OidcClientConfig`, `ServiceT`, `ErrorCode`

Repo `zpr-vsapi`. **Files:** Modify `vs.capnp:384-389` (`AuthBlob`), `:592-610` (`Error`/`ErrorCode`), `:657-666` (`ServiceDescriptor`, `ServiceT`).

- [ ] **Step 1:** Apply Contract 2 verbatim. Do not touch `SelfSignedBlob` or `AuthCodeBlob`.
- [ ] **Step 2:** `capnp compile -o- vs.capnp >/dev/null`.
- [ ] **Step 3:** Commit `feat: OIDC auth blob, IdP service descriptor, policyDenied error`; PR; tag on merge.

**Acceptance:** `AuthBlob.oidc @2`, `ServiceDescriptor.oidc @4`, `ServiceT.oidcAuthentication @1`, `ErrorCode.policyDenied @10`.

### Task A3: `zpr-common` — Rust mirrors and tag `v0.26.0`

Repo `zpr-common`. **Files:** Modify `.gitmodules` pointers (`zpr-policy`, `zpr-vsapi`), `src/policy_types/trusted_service.rs:26-32,136-197`, `src/vsapi_types/auth.rs:5-35`, `src/vsapi_types/services.rs:15-92`. **Test:** inline `mod tests` in each.

**Interfaces — Produces (exact):**

```rust
// src/policy_types/trusted_service.rs
/// Pinned OpenID Connect provider configuration for an `api = "oidc"` trusted service.
/// Mirrors `OidcConfig` in policy.capnp. See spec-OIDC.md "ZPLC configuration".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OidcConfig {
    pub issuer: String,
    pub jwks_uri: String,
    pub client_id: String,
    pub client_secret: Option<String>,      // "" on the wire == None
    pub scopes: Vec<String>,
    pub allowed_domains: Vec<String>,
    pub max_auth_age_seconds: u32,          // 0 == unlimited
    pub allow_offline_access: bool,
    pub seed_jwks: String,
    pub jwks_proxy_service: Option<String>, // "" on the wire == None
}
pub struct TrustedService { /* existing 4 fields */ pub oidc: Option<OidcConfig>, }

// src/vsapi_types/auth.rs
pub enum AuthBlob { SS(SelfSignedBlob), AC(AuthCodeBlob), Oidc(OidcBlob) }
#[derive(Debug, Clone)]
pub struct OidcBlob { pub issuer: String, pub id_token: String, pub nonce: String }

// src/vsapi_types/services.rs
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ServiceT { ActorAuthentication, OidcAuthentication }
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct OidcClientConfig { pub issuer: String, pub client_id: String, pub client_secret: Option<String>, pub scopes: Vec<String>, pub allow_offline_access: bool }
pub struct ServiceDescriptor { pub stype: ServiceT, pub service_id: String, pub service_uri: String, pub zpr_addr: IpAddr, pub oidc: Option<OidcClientConfig> }
impl ServiceDescriptor { pub fn get_socket_addr(&self) -> Option<SocketAddr> /* None when stype == OidcAuthentication */ }
```

Also add `write_to` for `ServiceDescriptor` if one does not exist (the visa service needs to encode it in `setServices`); check `services.rs` first, reuse if present.

- [ ] **Step 1:** Bump both submodule pointers to the A1/A2 tags; `make submodules-pull && make build`.
- [ ] **Step 2 (test first):** round-trip tests: `TrustedService` with `oidc: Some(..)` → `write_to` → `TryFrom` equals original; `oidc: None` → reader `has_oidc() == false`; `AuthBlob::Oidc` `TryFrom` reader; `ServiceDescriptor` with `OidcAuthentication` decodes `oidc` and `get_socket_addr()` is `None`; `ActorAuthentication` behaviour unchanged (existing tests still pass).
- [ ] **Step 3:** Implement; `make build && make test`.
- [ ] **Step 4:** Grep both consumers for every `ServiceDescriptor {` / `AuthBlob::` match to list the compile breaks the tag bump will cause (`zpr-core`: `visa_mgmt.rs:96-113`, `link_state.rs:880-888`, `libnode2/src/vss.rs:417`; `zpr-visaservice`: `connection_control.rs:238-259`, `actor_mgr.rs:429-437`). Put the list in the PR description.
- [ ] **Step 5:** Commit, PR, tag `v0.26.0` on merge.

**Acceptance:** `make build`, `make test` green; the four new/changed types round-trip; consumers' break list posted.

---

## Phase B — Compiler (`zpr-compiler`)

### Task B0: zpr-compiler#146 / PR #147 (in review)

Already written. Definition of done per the ZPR skill (approved, threads resolved, CI green, mergeable). Nothing in this plan depends on it except that B1's `returns_attributes` for `oidc` must go through the same `parse_return_mappings` choke point so the reserved-namespace check covers OIDC declarations for free.

### Task B1: `api = "oidc"` configuration parsing and validation

**Files:** Modify `src/zpl.rs:19-24` (add `TS_API_OIDC`), `src/config/mod.rs:146-158` (`TrustedService` gains `oidc: Option<OidcTsConfig>`), `src/config/trusted_service.rs:16-34` (`warn_unknown_ts_property` list), `:176-298` (`parse_trusted_service` dispatch at `:198`), new `parse_oidc_trusted_service` beside `parse_file_trusted_service` (`:131-173`); tests in `src/config/trusted_service.rs` `mod test` (`:300+`); fixtures `test-data/bad-oidc-*.zplc` only if a whole-compile test is needed (config unit tests are sufficient here).

**Produces:**

```rust
// src/zpl.rs
pub const TS_API_OIDC: &str = "oidc";

// src/config/mod.rs
/// The `api = "oidc"` properties of a trusted service (see spec-OIDC.md "ZPLC configuration").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OidcTsConfig {
    pub issuer: String,
    pub jwks_uri: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub allowed_domains: Vec<String>,
    pub seed_jwks_path: Option<PathBuf>,   // resolved relative to the .zplc; contents embedded in B2
    pub max_auth_age_seconds: u32,
    pub allow_offline_access: bool,
}
// on config::TrustedService:
pub oidc: Option<OidcTsConfig>,
```

Validation rules (each is one unit test; error text is the contract, since `compilation.rs` tests assert on messages):

| Property | Rule | Error / warning text |
|---|---|---|
| `issuer` | required; `https://`; no query, no fragment | `trusted_service {id}: issuer must be an https URL without query or fragment` |
| `jwks_uri` | required; `https://` | `trusted_service {id}: jwks_uri is required and must be https` |
| `client_id` | required, non-empty | `trusted_service {id}: client_id is required` |
| `client_secret` | optional string | — |
| `scopes` | optional; default `["openid","email","profile"]`; must contain `openid` | `trusted_service {id}: scopes must include "openid"` |
| `allowed_domains` | **required**, non-empty; `["*"]` allowed | `trusted_service {id}: allowed_domains is required (use ["*"] to accept any account)` |
| `seed_jwks` | optional path; existence checked in B2 when embedding | — |
| `expiration_seconds` | reuse `parse_expiration_seconds`; must be `> 0` for oidc | `trusted_service {id}: expiration_seconds is required for api="oidc"` |
| `max_auth_age_seconds` | optional u32, default 0 | — |
| `allow_offline_access` | optional bool, default false | — |
| `returns_attributes` | required, ≥1, via `parse_return_mappings` (B0 check applies) | existing messages |
| `identity_attributes` | **required**, must be exactly `["sub"]`; `email` gets a dedicated message | `trusted_service {id}: identity_attributes must be ["sub"]`; for email: `trusted_service {id}: "email" cannot be an identity attribute: addresses are mutable and reusable; use "sub"` |
| `service` | optional; **warn** when omitted | `trusted_service {id}: no "service" declared; the visa service will need direct internet egress to reach jwks_uri` |
| `client` | rejected | `trusted_service {id}: "client" is not allowed for api="oidc" (the adapter talks to the provider directly)` |
| `provider` | rejected | `trusted_service {id}: "provider" is not allowed for api="oidc"` |
| `cert_path` | rejected | `trusted_service {id}: "cert_path" is not allowed for api="oidc" (TLS to the provider is verified against system roots)` |
| `prefix` | rejected (as for `file`) | existing pattern |

- [ ] **Step 1 (tests first):** in `trusted_service.rs mod test`, one `#[test]` per row above using the existing `body(toml)` + `parse_trusted_service(id, &t, &CompilationCtx::default()).unwrap_err()` shape (model: `test_file_forbidden_properties_rejected` `:394-413`), plus `test_oidc_minimal_valid` asserting the parsed `OidcTsConfig` and defaults, and `test_oidc_missing_service_warns` using a `CompilationCtx` with `werror = true` to turn the warning into an `Err(CompilationError::Warning(..))`.
- [ ] **Step 2:** Run: `cargo test -p zplc config::trusted_service` → all new tests FAIL (no `oidc` arm).
- [ ] **Step 3:** Implement `parse_oidc_trusted_service`, the `:198` dispatch, `warn_unknown_ts_property` additions (`issuer`, `jwks_uri`, `client_id`, `client_secret`, `scopes`, `allowed_domains`, `seed_jwks`, `max_auth_age_seconds`, `allow_offline_access`). Do **not** touch `weaver.rs` yet (B2).
- [ ] **Step 4:** `make test && make check` green.
- [ ] **Step 5:** Commit `feat(config): parse and validate api="oidc" trusted services`; PR.

**Acceptance:** every row's test passes; existing `file`/`validation/2` tests untouched; `email` identity attribute produces the dedicated message.

### Task B2: codegen, weaving, `zpdump`, version 0.17.0

**Files:** Modify `Cargo.toml:3` (0.17.0) and `:21` (`zpr` tag `v0.26.0`), `src/weaver.rs:1229-1366` (`add_trusted_services`), `:1375-1434` (`check_ts_components`, the `else` at `:1424`), `:1181-1226` (`resolve_trusted_service_providers`, skip `oidc` like `file` at `:1204`), `src/fabric.rs:38-50` (`TrustedServiceSpec.oidc: Option<zpr::policy_types::OidcConfig>`), `:270-301`, `src/policybuilder.rs:259-279` (`set_connects` protocol guard: `api != TS_API_FILE && api != TS_API_OIDC`), `src/dumpv2.rs:270-310`; fixtures `test-data/test-oidc.zpl`, `test-data/test-oidc.zplc`, `test-data/google-jwks-seed.json`; tests in `tests/zpl-test.rs` (model `test_file_trusted_service_end_to_end` `:315-412`).

**Consumes:** `config::OidcTsConfig` (B1), `zpr::policy_types::OidcConfig` (A3).

Behaviour:

1. **No protocol, no provider.** An `oidc` TS takes the `file` early-out shape in `add_trusted_services` (`:1261-1282`): a `TrustedServiceSpec` with the VS as sole provider attr, `protocol: None`, and `oidc: Some(config)`. `check_ts_components` never runs for it, so `weaver.rs:1424` is not reached; but add `TS_API_OIDC` to that `else` arm's guard anyway so the error text stays accurate.
2. **Proxy rule weaving.** If `service` was declared: the fabric service it names must exist in `[services.*]` (error `trusted_service {id}: service "{name}" is not declared in [services]`), and the compiler adds `allow visa service access to trusted service {id}` **targeting that fabric service** via `self.fabric.add_condition_to_service(false, &proxy_service_id, &[cn == vs.zpr], &[], &[], true, None, &pline)` — the same call as `:1348-1362`, different target. `OidcConfig.jwks_proxy_service = service id`. If omitted, `jwks_proxy_service = None` (warning already emitted in B1).
3. **Seed JWKS.** Read `seed_jwks_path` relative to the `.zplc` directory; must parse as JSON with a top-level `keys` array (error `trusted_service {id}: seed_jwks "{path}" is not a JWKS document`); embed verbatim in `OidcConfig.seed_jwks`. Absent → `""`.
4. **Emit.** `Fabric::add_trusted_service` copies `oidc` into the `zpr::policy_types::TrustedService` record (A3), so `write_to` emits it with no compiler-side capnp code.
5. **`zpdump`.** In the `TRUSTED SERVICES` section print, when `ts.oidc.is_some()`: `issuer`, `jwks_uri`, `client_id`, `scopes`, `allowed_domains`, `expiration`, `max_auth_age`, `offline_access`, `jwks_proxy_service`, and `seed_jwks: <n> keys`. Never print `client_secret`'s value; print `client_secret: (set)` or `(none)`.
6. **`[services.google-*]` is an error** only if `[services.<id>]` or `[services.<id>-vs]` or `[services.<id>-client]` exists for an oidc TS `<id>` and is not the declared proxy `service`: `trusted_service {id}: api="oidc" has no on-net service; remove [services.{name}]`.

- [ ] **Step 1 (fixture):** `test-data/test-oidc.zpl` = copy of `test-file.zpl` minus BAS, plus `allow domain:example.com users to access Webby.`; `test-oidc.zplc` = the spec's ZPLC block (`[trusted_services.google]` with `service = "google-jwks-proxy"`, `[services.google-jwks-proxy] protocol="tcp" port=3128 provider=[["device.zpr.adapter.cn","proxy1.zpr"]]`), `seed_jwks = "google-jwks-seed.json"` (a two-key RSA JWKS generated once with `openssl` + a tiny script, checked in).
- [ ] **Step 2 (tests first):** `tests/zpl-test.rs::test_oidc_trusted_service_end_to_end`: compile, `decode_records`, assert one `TrustedService` with `service_id == "google"`, `oidc.issuer == "https://accounts.google.com"`, `jwks_proxy_service == Some("google-jwks-proxy")`, `seed_jwks` parses to 2 keys, `identity_attrs == ["sub"]`; assert the join policy list contains a client policy on `google-jwks-proxy` whose `cli_condition == [device.zpr.adapter.cn EQ vs.zpr]`; assert the `Service` for `google` has `kind == trusted("oidc")` and **no** endpoints. Add `bad-oidc-services-block.zpl/.zplc` (has `[services.google-vs]`) asserting the error text; `bad-oidc-missing-proxy-service.zplc` (`service = "nope"`).
- [ ] **Step 3:** Run → FAIL. Implement 1–6. `can_compile_misc_test_policies` now also sweeps `test-oidc` through `dump_v2` (covers 5).
- [ ] **Step 4:** Bump `Cargo.toml` version to `0.17.0` and `zpr` tag to `v0.26.0`; `make test && make check`.
- [ ] **Step 5:** Commit `feat: compile api="oidc" trusted services to OidcConfig; weave JWKS proxy rule; 0.17.0`; PR. PR body must say: *visa service follow-up: `POLICY_MIN_COMPILER_MINOR` 16 → 17 in C5*.

**Acceptance:** fixture compiles and dumps; the woven proxy rule is present and targets the proxy service; the two `bad-*` fixtures fail with the specified text; version and tag bumped.

---

## Phase C — Visa service (`zpr-visaservice`)

### Task C0: Namespaced authority attributes and compiler 0.16 floor — **urgent**

The compiler already emits `has user.zpr.authority`; until this lands, every bare `allow users …` rule from a 0.16 policy matches nothing. This is the follow-up promised in zpr-compiler PR #145.

**Files:** Modify `libeval/src/attribute.rs:30-31`, `libeval/src/actor.rs:167-192`, `vs/src/connection_control.rs:189-193, 241-242, 273-277, 290, 306-312, 489-494`, `vs/src/config.rs:30` (`15` → `16`), `libeval/src/policy.rs:280-304` (`lookup_identity_keys` doc mentions `zpr.authority`); audit `integration-test/*.zpt` sources and `pregen/` for `zpr.authority`; `zpt-test-connect.sh`. Tests: `connection_control.rs mod tests` (`:745+`), `libeval/src/actor.rs` tests.

**Produces (Contract 7):** `key::USER_AUTHORITY`, `key::DEVICE_AUTHORITY`, `key::AUTHORITY_METHOD_BOOTSTRAP`; `key::AUTHORITY` removed.

Behaviour:
- Every site that today pushes `key::AUTHORITY` with value `self.authority` on an RSA-verified path pushes `device.zpr.authority = "zpr-bootstrap"` instead. `self.authority` (`vs.zpr/<ident>`) remains the JWT `iss`; it is no longer an attribute value.
- `authorize_connection` (`:489-494`) stops adding a blanket authority; the blob arms own it (this is what makes C1/C5 possible). The identity-key registration `add_identity_key(usize::MAX, key::AUTHORITY)` becomes: register `device.zpr.authority` if present, `user.zpr.authority` if present.
- `get_authentication_expiration` = min over `{device.zpr.authority, user.zpr.authority} ∩ present` ∪ identity keys; `None` only if none present.

- [ ] **Step 1 (failing tests):** `test_rsa_path_installs_device_authority_bootstrap` (authenticate a node with a signed challenge via existing helpers `gen_rsa_test_keypair`/`sign_node_challenge`/`make_policy_with_bootstrap_key`; assert actor has `device.zpr.authority == ["zpr-bootstrap"]`, no `zpr.authority`, no `user.zpr.authority`, and `identity_keys` contains `device.zpr.authority`); `actor.rs::test_expiration_min_over_namespaced_authorities` (two authorities with different expiry → min); `test_policy_below_0_16_rejected` (use `make_container_bytes(0,15,0,..)`).
- [ ] **Step 2:** Run → FAIL. Implement. Delete `key::AUTHORITY`; let the compiler find every use.
- [ ] **Step 3:** Fixture audit: `grep -rn 'zpr.authority' integration-test/ zpt/` ; regenerate `pregen` with a 0.16 `zplc` (`make pregen`, `ZPLC=../zpr-compiler/target/debug/zplc`). Fix any `.zpt` that used bare `users` to mean "any actor" (PR #145's audit says none in the compiler corpus; verify here).
- [ ] **Step 4:** Extend `zpt-test-connect.sh` with a fourth object: device-only claims against a policy containing `allow users to access Webby.` → assert **no** match (fail-closed proof of #144 + C0 together).
- [ ] **Step 5:** `make test && make check`; commit `feat: per-namespace authority attributes; require compiler >= 0.16`; PR.

**Acceptance:** 0.15 policies rejected; RSA path yields `device.zpr.authority:zpr-bootstrap`; `zpt` proves bare `allow users` does not admit a device-only actor.

### Task C1: Multiple auth blobs

**Files:** Modify `vs/src/connection_control.rs:215-260` (`authenticate_adapter_or_node`), `:221-223` (drop the `> 1` rejection), the reauth path that reads `ReauthRequest.blobs`. Tests in the inline module.

Behaviour (spec *Failure rule*):
- Iterate `req.blobs`. Each arm returns `Result<BlobOutcome, ServiceError>` where `BlobOutcome { authd: Vec<Attribute>, namespace: Namespace /* Device | User */ }`. The first `Err` aborts the whole connection with that error (presented-and-invalid fails). Zero blobs → `ServiceError::Param("at least one auth blob is required")`. Two blobs of the same namespace → `ServiceError::Param("duplicate {namespace} authentication")`.
- CN handling is already correct (never promoted by `authorize_connection`); pin it with a test.
- `AuthBlob::Oidc(_)` arm in C1 returns `ServiceError::Internal("OIDC not yet supported")` — replaced in C5. `AuthBlob::AC(_)` keeps its current error.

- [ ] **Step 1 (failing tests):** `test_two_blobs_ss_and_oidc_stub_fails_whole_connection` (valid SS + `Oidc` stub → `Err`, and **no actor persisted**); `test_zero_blobs_rejected`; `test_duplicate_device_blob_rejected` (two valid SS blobs); `test_cn_not_authenticated_without_device_blob` (call `authorize_connection` with `unauthd_claims = [CN]`, `authd_claims = [user.zpr.authority, user.oidc-subject]` against `make_trusted_service_policy_with_identity(...)`; assert the actor's `device.zpr.adapter.cn` attribute is **absent** or unauthenticated per `scrub_adapter_claims` semantics, and `identity_keys` does not contain the CN); **regression test the spec demands:** `test_user_only_actor_claiming_foreign_cn_gets_no_cn_attributes` — register a capturing trusted service (`register_capturing_ts`) that returns `device.role = admin` for identity `(device.zpr.adapter.cn, "server1.zpr")`; connect user-only with `unauthd` CN `server1.zpr`; assert the capture shows the lookup identities contained only the `user.oidc-subject` pair and the actor has no `device.role`.
- [ ] **Step 2:** Run → FAIL. Implement the loop and `BlobOutcome`.
- [ ] **Step 3:** `make test && make check`; commit `feat: accept multiple auth blobs; presented-and-invalid fails closed`; PR.

**Acceptance:** all five tests pass; existing single-SS tests unchanged.

### Task C2: Offline `id_token` validation module

**Files:** Create `vs/src/oidc/mod.rs`, `vs/src/oidc/validate.rs`; tests inline plus fixture keypair `vs/tests/data/oidc-test-rsa.pem` and a test-only minter. No dependency changes (`jsonwebtoken` 10 with `rust_crypto` is already in `vs/Cargo.toml:33`).

**Produces:**

```rust
/// Everything the validator needs from policy for one provider. Built from
/// `zpr::policy_types::OidcConfig` in C4; kept separate so this module has no policy dependency.
pub struct IdpParams<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub allowed_domains: &'a [String],   // ["*"] = any
    pub max_auth_age: Option<Duration>,
    pub clock_skew: Duration,            // use config::MAX_CLOCK_SKEW_SECS
}

/// Claims we keep after validation. Everything else in the token is dropped.
pub struct ValidatedToken {
    pub sub: String,
    pub email: Option<String>,   // present only when email_verified == true
    pub hd: Option<String>,
    pub auth_time: SystemTime,   // `auth_time` claim, else `iat`
    pub raw_claims: serde_json::Map<String, serde_json::Value>, // for returns_attributes mapping
}

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("token signature or header invalid: {0}")] Signature(String),  // -> ErrorCode::invalidSignature
    #[error("token rejected: {0}")]                    Rejected(String),   // -> ErrorCode::authError (hd, max_auth_age)
    #[error("unknown key id {0}")]                     UnknownKid(String), // -> invalidSignature (after one JWKS refresh attempt in C3)
    #[error("no signing keys available")]              NoKeys,             // -> temporarilyUnavailable
}

/// Validate `id_token` against `keys` (a JWKS) and `params`, requiring `expected_nonce`.
/// Allowlist: RS256 only. Rejects `alg: none`, HS*, ES*, PS*.
pub fn validate_id_token(id_token: &str, keys: &jsonwebtoken::jwk::JwkSet, params: &IdpParams, expected_nonce: &str, now: SystemTime) -> Result<ValidatedToken, OidcError>;
```

Domain rule: if `allowed_domains == ["*"]`, skip; else `hd` must be present **and** in the list; email domain is never consulted.

- [ ] **Step 1 (test-only minter):** `#[cfg(test)] mod mint { pub fn token(claims: serde_json::Value, kid: &str, alg: Algorithm, key: &EncodingKey) -> String }` plus `fn test_jwks() -> JwkSet` from the fixture PEM (`kid = "k1"`).
- [ ] **Step 2 (table test):** one `#[test]` per row of the spec's *JWT validation* table, exactly: valid → `Ok` with `sub`, `hd`, `email`; `alg: none` → `Signature`; HS256 with the RSA public key bytes as HMAC secret → `Signature` (algorithm confusion); wrong `aud` → `Signature`; wrong `iss` → `Signature`; `exp` past → `Signature`; missing nonce → `Signature`; mismatched nonce → `Signature`; **`hd` absent** → `Rejected`; `hd` not allowed → `Rejected`; `email_verified: false` → `Ok` with `email == None`; unknown `kid` → `UnknownKid`; `auth_time` older than `max_auth_age` → `Rejected`; `allowed_domains == ["*"]` with no `hd` → `Ok`; no `auth_time` → `auth_time == iat`.
- [ ] **Step 3:** Run → FAIL. Implement with `jsonwebtoken::decode_header` → `kid` → `JwkSet::find` → `DecodingKey::from_jwk` → `Validation::new(RS256)` with `set_audience`, `set_issuer`, `leeway = clock_skew`, `validate_exp = true`, then manual `nonce`/`hd`/`email_verified`/`auth_time` checks.
- [ ] **Step 4:** `make test && make check`; commit `feat(oidc): offline id_token validation with vector tests`; PR.

**Acceptance:** all 15 vectors pass; module has zero imports from `crate::` other than `config::MAX_CLOCK_SKEW_SECS`.

### Task C3: JWKS key source

**Files:** Create `vs/src/oidc/jwks.rs`; modify `vs/Cargo.toml` (add `reqwest = { workspace = true, features = ["json", "rustls-tls"] }` — check which TLS feature the workspace `reqwest` already uses in `vs-admin` and match it), `vs/src/config.rs:119-152` (optional `oidc_refresh_seconds: u64`, default 3600, in `CoreSection` + `Default`). Tests: inline with a local `axum` server (already a dep) serving a JWKS, and a local `CONNECT`-speaking stub (a ~40-line tokio TCP handler that answers `HTTP/1.1 200` and splices) — this also gives D5 a proxy to reuse.

**Consumes:** `zpr::policy_types::OidcConfig` (A3). **Produces:**

```rust
/// Cached signing keys for one provider. Seeded from policy; refreshed periodically and
/// on unknown `kid`; never discarded on fetch failure (stale tolerance).
pub struct KeySource { /* ArcSwap<Arc<JwkSet>>, last_ok: Mutex<Option<SystemTime>>, cfg */ }
impl KeySource {
    pub fn from_policy(cfg: &OidcConfig, proxy: Option<Url>) -> Result<Self, OidcError>; // parses seed_jwks; NoKeys if empty and no proxy/direct route
    pub fn current(&self) -> Arc<JwkSet>;
    pub async fn refresh(&self) -> Result<(), OidcError>;  // GET jwks_uri via reqwest with optional Proxy::https(proxy), timeout 10s, TLS verified against system roots
    pub fn spawn_refresher(self: Arc<Self>, period: Duration) -> JoinHandle<()>;
}
```

Proxy resolution (`Option<Url>`): when `cfg.jwks_proxy_service` is `Some(id)`, look up the actors currently providing service `id` (**reuse** `actor_db.list_services_for_actor` / the reverse index if one exists — check `vs/src/db` before adding a helper; if none, add `ActorDb::providers_of_service(&str) -> Vec<IpAddr>`) and the port from the policy `Service.endpoints` scope; build `http://[zpr-addr]:port`. If no provider is connected yet, `refresh()` returns `Err(Rejected("proxy not reachable"))` and the seed keeps serving. Re-resolve on each refresh (providers come and go).

- [ ] **Step 1 (failing tests):** `test_seed_serves_before_first_fetch`; `test_refresh_replaces_keys` (axum JWKS, direct); `test_refresh_failure_keeps_stale_keys` (server returns 500); `test_refresh_via_connect_proxy` (stub proxy in front of the axum server; assert the proxy saw exactly `CONNECT 127.0.0.1:<port>`); `test_no_seed_no_route_is_nokeys`.
- [ ] **Step 2:** Run → FAIL. Implement. Never log the response body.
- [ ] **Step 3:** `make test && make check`; commit `feat(oidc): JWKS key source with seed, CONNECT-proxied refresh, stale tolerance`; PR.

**Acceptance:** stub proxy sees `CONNECT` (never `GET https://…`); stale keys survive a failed refresh; nothing new in `Cargo.lock` beyond `reqwest` feature unification.

### Task C4: `oidc` trusted-service implementation and IdP `ServiceDescriptor`

**Files:** Create `vs/src/oidc/store.rs`; modify `vs/src/trusted_services/factory.rs:16-87` (dispatch on `api`), `vs/src/trusted_services/mod.rs` (export), `vs/src/actor_mgr.rs:429-437, 564-616` (`uri_for_service` and `get_auth_services_list`), `vs/Cargo.toml` (`zpr` tag `v0.26.0`), `Cargo.lock`. Tests inline; `test_helpers.rs::make_trusted_service_policy_with_identity` extended with an `oidc: Option<OidcConfig>` variant or a sibling helper `make_oidc_policy(...)`.

**Consumes:** `TrustedServiceInterface` (`mod.rs:54-79`), `AttributeMapper`, `KeySource` (C3), `IdpParams` (C2). **Produces:**

```rust
/// An `api = "oidc"` trusted service. Unlike file/network services it cannot be *queried*
/// for an arbitrary identity: the claims arrive with the token. The connect path calls
/// `admit` after validation; `get_attributes_for_actor` then serves those claims so the
/// normal ts_mgr union/conflict/refresh machinery applies unchanged.
pub struct OidcTrustedService {
    id: String,                       // trusted-service id, e.g. "google"; also the authority value
    cfg: OidcConfig,
    mapper: AttributeMapper,
    keys: Arc<KeySource>,
    admitted: DashMap<String /* sub */, (Vec<Attribute>, SystemTime /* expires */)>,
    revision: AtomicU64,
}
impl OidcTrustedService {
    pub fn params(&self) -> IdpParams<'_>;
    pub fn keys(&self) -> &KeySource;
    pub fn lifetime(&self) -> Duration;    // record.expiration_seconds
    /// Map `token.raw_claims` through `returns_attributes`, stamp `expires`, cache under `sub`, bump revision.
    pub fn admit(&self, token: &ValidatedToken, expires: SystemTime) -> Result<Vec<Attribute>, ServiceError>;
}
#[async_trait] impl TrustedServiceInterface for OidcTrustedService { /* identities matching (mapped sub key, sub) -> cached attrs; flush clears admitted */ }
```

`factory.rs`: `api` match with arms `"file"` → existing, `"oidc"` → `OidcTrustedService::new(record, key_source)`, `_` → existing error. `TrustedServiceDefinition` gains `oidc: Option<OidcConfig>` so `PartialEq` still detects changes. Also expose `TrustedServicesMgr::oidc_service_for_issuer(&str) -> Option<Arc<OidcTrustedService>>` for C5 (store a second, typed list beside the `dyn` one).

`actor_mgr.rs`: `get_auth_services_list` emits, for each `ServiceType::Trusted("oidc")` service, a `ServiceDescriptor { stype: OidcAuthentication, service_id, service_uri: issuer, zpr_addr: Ipv6Addr::UNSPECIFIED, oidc: Some(OidcClientConfig{..}) }`. `uri_for_service` gains an `oidc` arm returning the issuer (and its tests at `:770-870` gain a case).

- [ ] **Step 1 (failing tests):** `factory::test_oidc_definition_builds_oidc_store`; `store::test_admit_then_lookup_by_sub` (admit a `ValidatedToken`, lookup `[(user.oidc-subject, sub)]` → mapped attrs with the given expiry, source id == "google"); `store::test_lookup_unknown_sub_is_empty`; `store::test_email_not_mapped_when_unverified` (token with `email: None` → no `user.email`); `actor_mgr::test_auth_services_list_includes_oidc_descriptor`.
- [ ] **Step 2:** Bump `zpr` to `v0.26.0`; fix the A3 break list; run → FAIL on the new tests. Implement.
- [ ] **Step 3:** `make test && make check`; commit `feat(oidc): oidc trusted-service store and IdP service descriptor`; PR.

**Acceptance:** policy with an `oidc` TS loads (today it is rejected wholesale at `factory.rs:39-44`); descriptor pushed to nodes carries issuer/client_id/scopes; admit→lookup round-trips.

### Task C5: OIDC blob on the connect path

**Files:** Modify `vs/src/connection_control.rs` (the `Oidc` arm from C1; error mapping where `ServiceError` becomes a VSAPI `Error`), `vs/src/config.rs:30` (`16` → `17`), `integration-test/zpt-test-connect.sh` + a new `.zpt` source and `pregen`. Tests inline.

**Consumes:** C1 loop, C2 `validate_id_token`, C3 `KeySource`, C4 `OidcTrustedService`, B2 fixture compiler.

Arm behaviour:
1. `ts_mgr.oidc_service_for_issuer(&blob.issuer)` else `Param` → `paramError`.
2. `validate_id_token(&blob.id_token, &svc.keys().current(), &svc.params(), &blob.nonce, now)`; on `UnknownKid` call `svc.keys().refresh().await` once and retry once; on `NoKeys` → `temporarilyUnavailable` with `retry_in = 30`.
3. `expires = min(token.auth_time + svc.lifetime(), token.exp?)` — no: per spec, `exp` is used only to reject; `expires = token.auth_time + svc.lifetime()`.
4. `attrs = svc.admit(&token, expires)?`; push `user.zpr.authority = svc.id` with `expires` into `authd`; identity-key registration for `user.zpr.authority` and the mapped `sub` key happens in `authorize_connection` via `lookup_identity_keys()` (already includes mapped identity attrs after #310).
5. Do **not** push the token's claims directly; they arrive through `ts_mgr.get_attributes_for_actor(&identities)` (C4), which also fires for the mapped `sub`.
6. `approve_connection` returning "no join policy matched" → `policyDenied`.

- [ ] **Step 1 (failing tests):** `test_oidc_only_connect_yields_user_authority_and_claims` (mint with C2's test minter against a policy from `make_oidc_policy`; assert `user.zpr.authority == ["google"]`, `user.oidc-subject`, `user.email`, `user.domain`; `identity_keys == ["user.oidc-subject", "user.zpr.authority"]` ordering per `lookup_identity_keys` then authorities; no `device.*`); `test_ss_plus_oidc_connect_yields_both_authorities`; `test_oidc_wrong_nonce_fails_whole_connection_invalid_signature`; `test_oidc_consumer_account_no_hd_rejected_auth_error`; `test_oidc_unknown_issuer_param_error`; `test_oidc_no_keys_temporarily_unavailable`; `test_valid_login_no_join_policy_is_policy_denied`; `test_user_authority_expiry_is_auth_time_plus_lifetime` (mint with `auth_time = now - 1h`, lifetime 12h → expires ≈ now + 11h, **not** `exp`).
- [ ] **Step 2:** Run → FAIL. Implement. Bump `POLICY_MIN_COMPILER_MINOR` to 17 (B2 must be merged; compile the `zpt` fixtures with the 0.17 `zplc`).
- [ ] **Step 3 (`zpt`):** new `integration-test/zpt-test-oidc.zpt` (+ `.zpl/.zplc` from `test-oidc` in B2) driving three `APPROVE_CONNECTION` requests: device-only (`authd`: CN, `device.zpr.authority`), user-only (`authd`: `user.oidc-subject`, `user.zpr.authority:google`, `user.domain:example.com`; `unauthd`: CN), both. Assert `identity_keys` and which policies matched (`allow domain:example.com users …` matches user-only and both; a device-only rule matches device-only and both; `allow users to access services.` does not match device-only).
- [ ] **Step 4:** `make test && make check`; commit `feat(oidc): validate OIDC blobs on connect; user.zpr.authority; policyDenied; compiler >= 0.17`; PR.
- [ ] **Step 5 (release):** after merge, `make release`, upload `release-linux-x86_64.tar.gz` to a GitHub pre-release tagged `v0.19.0-rc.1` (`gh release create v0.19.0-rc.1 --prerelease release-linux-x86_64.tar.gz`). Final `v0.19.0` is cut in D5 after the integration test passes.

**Acceptance:** all eight unit tests and the `zpt` script pass; error codes match Contract 2's table; a 0.16 policy is now rejected.

---

## Phase D — Core (`zpr-core`)

### Task D1: Node side

**Files:** Modify `Cargo.toml` (`zpr` tag `v0.26.0`), `adapter/ph/src/tlv.rs:23-35, 97, 141-151, 289-360` (`OIDC_IDP = 9`, `Str` value, encoder/parser), `adapter/ph/src/auth.rs:34-37, 122-151, 299-323` (`BLOB_TYPE_OIDC`, `ZdpOidcBlob`, `decode_blobs` returning `Vec<AuthBlob>`), `adapter/ph/src/link_state.rs:824-920` (`process_acquire_zpr_address_request`: verify every blob's challenge, forward all), `:1881-1901` (`get_available_asa_addresses` → also return OIDC descriptors), `:692-694`, `adapter/ph/src/mgmt/requests.rs:67-93` (`send_hello_success_response` gains `oidc_idps: &[OidcIdpInfo]`), `adapter/ph/src/visa_mgmt.rs:42-146` (map `AuthBlob::Oidc`; return the VS `Error.code` on failure instead of `LinkEvent::Error`), `link_state.rs:1333-1347, 1451-1459` (`send_grant_zpr_address_request` with a failure `ResponseCode`), `adapter/ph/src/zdp.rs` (`ResponseCode` variants: add `AuthFailed`, `PolicyDenied`, `AuthUnavailable` if absent — check the enum first), `adapter/ph/src/config.rs:68` (`ACTOR_AUTHENTICATION_TIMEOUT` 120 s → 330 s), `libnode2/src/vss.rs:405-442` (A3 `ServiceDescriptor` shape). Tests: `tlv.rs mod tests`, `auth.rs mod test`, `visa_mgmt.rs`.

**Produces:**

```rust
// adapter/ph/src/auth.rs
pub const BLOB_TYPE_OIDC: &str = "OIDC";
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ZdpOidcBlob { pub blob_type: String, pub issuer: String, pub id_token: String, pub challenge: String /* b64 of 48 bytes */ }
pub enum AuthBlob { SelfSigned(ZdpSelfSignedBlob), AuthCode(ZdpAuthCodeBlob), Oidc(ZdpOidcBlob) }
/// Decode a base64 JSON blob string into one or more blobs. Accepts a bare object (legacy) or an array.
pub fn decode_blobs(blob_str: &str) -> Result<Vec<AuthBlob>, AuthError>;
/// Encode several blobs as a JSON array, base64.
pub fn encode_blobs(blobs: &[AuthBlob]) -> String;
/// The OIDC nonce bound to a node challenge: base64url_nopad(SHA-256(challenge_bytes)).
pub fn oidc_nonce_for_challenge(challenge: &[u8; 48]) -> String;
impl ZdpOidcBlob { pub fn verify_challenge(&self, key: &[u8; 32]) -> Result<[u8; 48], AuthError>; /* HMAC + MAX_BLOB_AGE_SECONDS, same rules as verify_blob_challenge */ }

// adapter/ph/src/tlv.rs
pub const OIDC_IDP: TlvType = 9;   // JSON-encoded OidcIdpInfo (Str)
// adapter/ph/src/auth.rs (shared by node and adapter roles of the same binary)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct OidcIdpInfo { pub issuer: String, pub client_id: String, pub client_secret: Option<String>, pub scopes: Vec<String>, pub allow_offline_access: bool }
```

- [ ] **Step 1 (failing tests):** `auth::test_decode_blobs_legacy_object`, `test_decode_blobs_array_ss_and_oidc`, `test_decode_blobs_unknown_type_errors`, `test_oidc_nonce_is_b64url_sha256`, `test_oidc_blob_verify_challenge_rejects_bad_hmac_and_old_ctime`; `tlv::test_put_and_parse_oidc_idp` (round-trip `OidcIdpInfo` JSON); `visa_mgmt::test_build_connect_request_two_blobs_maps_oidc_with_nonce`.
- [ ] **Step 2:** Run → FAIL. Implement. In `process_acquire_zpr_address_request`: `SelfSigned` → existing `check_self_signed_blob`; `Oidc` → `verify_challenge` then `OidcBlob { nonce: oidc_nonce_for_challenge(&c) }`; any verification failure → `process_error_response`.
- [ ] **Step 3:** Failure propagation: in `visa_mgmt::authorize_connect`, on `Err` carrying a VSAPI `Error`, emit a new `LinkEvent::ReceivedAuthorizeFailure(ErrorCode, String)`; handler sends `send_grant_zpr_address_request(asm, link_id, ResponseCode::from(code), &[])` **before** `initiate_close`. Map: `invalidSignature`/`authError`/`paramError` → `AuthFailed`; `policyDenied` → `PolicyDenied`; `temporarilyUnavailable` → `AuthUnavailable`; everything else → existing error path.
- [ ] **Step 4:** Check the mgmt path can carry a ~3 KB blob (JWT + SS in one array). Locate the mgmt packet size limit in `adapter/ph/src/mgmt/core.rs` (`new_heap_packet`) and `zdpr.rs` fragmentation; if the limit is below 4 KB, raise it or document the ceiling as a `ponytail:` comment and add a test that a 3 KB blob round-trips through `parse_acquire_zpr_address_request`.
- [ ] **Step 5:** `make && make test && make check`; commit `feat(node): OIDC blobs, IdP advertisement TLV, VS failure reasons to adapter`; PR.

**Acceptance:** node forwards both blobs; the adapter receives a non-success `ResponseCode` when the VS rejects; a 3 KB blob round-trips; `OIDC_IDP` TLV round-trips.

### Task D2: Adapter FSM and `AuthAgent` RPC

**Files:** Modify `adapter/admin-api/cli.capnp:1-21` (Contract 6), `adapter/ph/src/admin_worker.rs:368-391` (`start_link` stores the agent client on the link), `adapter/ph/src/link_state.rs:120-165` (`LinkState::WaitForUserAuth`; `LinkEvent::ReceivedHelloResponse` gains `Option<Vec<OidcIdpInfo>>`; `AuthenticationSuccess(Vec<AuthBlob>)`; `AuthenticationFailure(AuthFailureReason)`), `:1099-1225` (`process_init_auth`: choose method(s) from `bootstrap` config + OIDC IdP presence; AAA gate applies only to the legacy ASA path), `:1349-1433` (`do_oidc_authenticate` beside `do_https_authenticate`; `process_authentication_success` accepts `WaitForUserAuth`), `adapter/ph/src/mgmt/handlers.rs:248-343` (parse `OIDC_IDP`), `:568+` (grant failure code → `ReceivedGrantZprAddressRequest` carries the reason), `adapter/ph/src/config.rs` (`OIDC_USER_INTERACTION_TIMEOUT = 300 s`). Tests: `link_state.rs mod tests` (`#[tokio::test(start_paused = true)]` + `LocalSet`), `handlers.rs` (new test module), `admin_worker.rs`.

**Produces:**

```rust
/// Why an out-of-band authentication attempt failed, as reported to ph-cli. Mirrors the spec's error taxonomy.
#[derive(Clone, Debug, PartialEq, strum::IntoStaticStr)]
pub enum AuthFailureReason {
    NoAgent,                      // OIDC required but startLink had no AuthAgent
    UserDeclined,                 // agent returned access_denied
    InteractionTimeout,           // OIDC_USER_INTERACTION_TIMEOUT
    IdpUnreachable(String),       // discovery/token endpoint failure (not an auth problem)
    AgentError(String),
    VisaServiceRejected(String),  // ResponseCode::AuthFailed  -> misconfiguration / token rejected
    PolicyDenied,                 // ResponseCode::PolicyDenied -> login worked, endpoint not admitted
    AuthUnavailable,              // ResponseCode::AuthUnavailable
    DeviceBlobRejected,
}
```

Flow in `process_init_auth` (non-bootstrap or bootstrap+OIDC): collect blobs — if `config.bootstrap` is set, produce the SS blob now; if the HelloResponse carried an `OidcIdpInfo` and a `startLink` agent is registered, enter `WaitForUserAuth`, arm `OIDC_USER_INTERACTION_TIMEOUT`, `spawn_local` a task calling `agent.get_oidc_credential(issuer, client_id, client_secret, scopes, allow_offline_access, oidc_nonce_for_challenge(&challenge), interactive=true)`; on return build `ZdpOidcBlob` and emit `AuthenticationSuccess(blobs)`; on error emit `AuthenticationFailure(reason)`. If an IdP was advertised but no agent is registered and no bootstrap key exists → `AuthenticationFailure(NoAgent)`. `process_authentication_success` sends `encode_blobs(&blobs)`.

The `startLink` handler holds the `AuthAgent` client for the link's lifetime (renewal uses `interactive=false`; if the call fails → `AuthenticationFailure(NoAgent)` → log and disconnect, per spec). Failure reasons reach `ph-cli` through `showLink`'s text for now and through the blocking `connect` return in D3.

- [ ] **Step 1 (failing tests):** `handlers::test_hello_response_parses_oidc_idp_tlv`; `link_state::test_wait_for_user_auth_times_out_with_interaction_timeout` (paused clock, no agent reply → `Error` state, reason `InteractionTimeout`); `test_no_agent_with_idp_and_no_bootstrap_fails_with_no_agent`; `test_agent_token_becomes_oidc_blob_with_challenge_nonce` (fake `AuthAgent::Server` in-process returning a fixed token; assert the encoded blob array contains SS+OIDC and the OIDC `challenge` equals the InitAuth payload); `test_grant_failure_code_maps_to_reason` (`PolicyDenied`).
- [ ] **Step 2:** Run → FAIL. Implement. Keep `do_https_authenticate` untouched (D4 deletes it).
- [ ] **Step 3:** `make && make test && make check`; commit `feat(adapter): OIDC via AuthAgent callback, WaitForUserAuth, failure reasons`; PR.

**Acceptance:** the five tests pass; a link started without an agent on a device-only network behaves exactly as today.

### Task D3: `ph-cli` Relying Party flow

Can start on day one as a standalone subcommand `ph-cli oidc-login --issuer --client-id [--client-secret] --scopes --nonce [--no-browser]` that prints the `id_token` to stdout (test harness for the flow); D2 then wires the same function behind `AuthAgent`.

**Files:** Modify `adapter/cli/Cargo.toml` (add `reqwest = { version = "0.13", features = ["json","rustls-tls"] }`, `url`, `serde`, `serde_json`, `base64 = "0.22"`, `sha2 = "0.10"`, `rand = "0.8"`; versions must match existing lock entries so no new resolution); create `adapter/cli/src/oidc.rs`; modify `adapter/cli/src/main_args.rs:28-76` (`Connect { id: u32, #[arg(long)] no_browser: bool }`, `AuthAgent { id: u32 }`, `OidcLogin {..}` hidden/debug), `adapter/cli/src/main.rs:169-260` (dispatch; `connect` keeps the RPC connection open for the duration of `startLink` and prints progress). Tests inline in `oidc.rs`.

**Produces:**

```rust
pub struct Pkce { pub verifier: String, pub challenge: String }
/// RFC 7636 S256: 32 random bytes -> base64url_nopad verifier (43 chars); challenge = base64url_nopad(SHA-256(verifier)).
pub fn pkce_s256() -> Pkce;
pub fn pkce_challenge_for(verifier: &str) -> String;   // for the RFC vector test
pub struct Discovery { pub authorization_endpoint: Url, pub token_endpoint: Url }
pub async fn discover(issuer: &Url, http: &reqwest::Client) -> Result<Discovery, OidcCliError>;
/// Bind 127.0.0.1:0, return the listener and the redirect_uri "http://127.0.0.1:<port>/callback".
pub fn bind_loopback() -> Result<(tokio::net::TcpListener, Url), OidcCliError>;
/// Accept exactly one request; verify `state`; reply with a "you can close this window" page; return the code.
pub async fn await_callback(listener: TcpListener, expected_state: &str, timeout: Duration) -> Result<String, OidcCliError>;
pub async fn exchange_code(token_endpoint: &Url, client_id: &str, client_secret: Option<&str>, code: &str, verifier: &str, redirect_uri: &Url, http: &reqwest::Client) -> Result<String /* id_token */, OidcCliError>;
/// The whole flow. `open_browser` = false prints the URL instead (CI / --no-browser).
pub async fn login(idp: &OidcIdpInfo, nonce: &str, open_browser: bool, timeout: Duration) -> Result<String, OidcCliError>;
```

Browser launch is `std::process::Command::new("xdg-open")` (Linux) / `"open"` (macOS); no crate. Refresh tokens / `offline_access` / keyring: **not in D3**; `interactive=false` returns `OidcCliError::NonInteractiveUnsupported` until a follow-up issue adds it (spec permits: default `allow_offline_access = false`).

- [ ] **Step 1 (failing tests):** `test_pkce_rfc7636_vector` (verifier `dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk` → challenge `E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM`); `test_pkce_verifier_length_and_charset`; `test_bind_loopback_is_127_0_0_1` (assert `local_addr().ip() == 127.0.0.1`, port != 0, redirect uri form); `test_callback_rejects_state_mismatch` (connect to the listener with `GET /callback?code=x&state=wrong` → `Err(StateMismatch)`, and listener is closed after); `test_callback_accepts_matching_state_once`; `test_exchange_code_posts_verifier_and_optional_secret` (local `axum`/`hyper` stub asserting form fields `grant_type=authorization_code`, `code`, `code_verifier`, `redirect_uri`, `client_id`, and `client_secret` only when `Some`); `test_login_no_browser_end_to_end_against_fake_idp` (in-process fake IdP: discovery doc, `/auth` that 302s to the redirect_uri with `code` and the given `state`, `/token` returning a fixed `id_token`; assert the `nonce` query parameter reached `/auth`).
- [ ] **Step 2:** Run → FAIL. Implement. Never print or log `code`, `id_token`, or `verifier` (assert in the end-to-end test that a captured log buffer does not contain the token).
- [ ] **Step 3:** `connect` UX: prints `Authentication with <issuer> required. Opening browser…` (or the URL with `--no-browser`), blocks until `startLink` returns, exits `0` on link up; non-zero exit codes per reason: `2` user declined, `3` timeout, `4` IdP unreachable, `5` visa service rejected token (misconfiguration), `6` policy denied, `7` device blob rejected. `auth-agent` registers and runs until SIGINT.
- [ ] **Step 4:** `make && make test && make check`; commit `feat(ph-cli): OIDC relying-party flow, connect and auth-agent commands`; PR.

**Acceptance:** RFC vector passes; listener is loopback-only and single-use; fake-IdP end-to-end passes with `--no-browser`; exit codes distinguish the seven failure classes.

### Task D4: Delete BAS / `OAuthRsa` legacy

**Files:** Delete from `adapter/ph/src/auth.rs`: `HARD_CODED_BAS_TLS_CERT_PEM` (`:44-76`), `OAuthRsa` and its impl (`:186-192, 409-569`), `ZdpAuthCodeBlob`/`BLOB_TYPE_AC`/`AuthBlob::AuthCode`, `PreauthResp`/`AuthReq`; `link_state.rs` `do_https_authenticate` (`:1349-1393`) and the ASA/AAA/rsaoauth gates (`:1164-1181`) that only served it; `config.rs:170-176, 286-302, 468-472, 608` (`rsaoauth`, `bas_key`); `visa_mgmt.rs:96-113` AC arm; `tlv.rs` `ASA` stays (harmless) unless nothing emits it — then delete too. Keep the `ac @1` capnp arm (removing a union arm is a schema break; mark `# deprecated` in a later vsapi bump).

- [ ] **Step 1:** `grep -rn 'danger_accept_invalid_certs\|HARD_CODED_BAS\|OAuthRsa\|bas_key\|rsaoauth\|BLOB_TYPE_AC' adapter libnode2` → list; delete; let the compiler guide.
- [ ] **Step 2:** Update `README.md` / config docs that mention `bas_key`. `make && make test && make check`.
- [ ] **Step 3:** Commit `chore: remove deprecated BAS/OAuthRsa client (issue #861)`; PR referencing zpr-core#861.

**Acceptance:** zero occurrences of `danger_accept_invalid_certs` in `zpr-core`; all tests green.

### Task D5: Fake-IdP integration test, CI pin, manual checklist

**Files:** Create `integration-test/lib/fake-idp.py` (stdlib `http.server`, ~120 lines: `/.well-known/openid-configuration`, `/auth` → 302 with `code`+`state`, `/token` → JSON with an RS256 `id_token` minted with a checked-in test key and the `nonce` echoed from `/auth`, `/jwks`; `--rotate` flag to switch `kid` for the rotation test), `integration-test/one-node-oidc-test.sh` (copy `one-node-test.sh`; adapter1 gets **no** `--bootstrap-key`, adapter2 gets both; `ph-cli connect 1 --no-browser` with `BROWSER=` unset and the printed URL fetched by `curl -L` inside the adapter's netns; asserts carrier on all TUNs and a `ping_test`; then `--rotate`, restart adapter1, assert login still succeeds after the VS refreshes), `integration-test/pregen/` (`oidc-test.zpl/.zplc` compiled with 0.17 `zplc`; `issuer = http://127.0.0.1:9000` is **not** `https` — so either the compiler rule gets an `--allow-insecure-issuer` test-only escape or the fake IdP serves TLS with a test CA added to the VS/adapter trust store. **Decision: serve TLS with a test CA** via `SSL_CERT_FILE`, keeping the compiler rule absolute), `.github/workflows/adapter.yml:39` (`VISA_SERVICE_RELEASE: v0.19.0-rc.1`, then `v0.19.0`), and a new job `oidc-integration-test` mirroring `basic-integration-test`; `docs/` or `README`: *Manual OIDC release checklist* (real Workspace happy path; consumer gmail must be rejected with the `hd` message; `client_secret` required-or-not for the Desktop client recorded as the outcome; `offline_access` path noted as not implemented).

- [ ] **Step 1:** Fake IdP + a `bash` smoke test that `curl`s all four endpoints.
- [ ] **Step 2:** The integration script, run locally with `VS_BIN` pointing at a `zpr-visaservice` build of C5.
- [ ] **Step 3:** CI job + pin bump to `v0.19.0-rc.1`; PR; when green, cut visa service `v0.19.0` (final) and bump the pin in a one-line follow-up PR.
- [ ] **Step 4:** Commit the checklist; close the umbrella when the checklist has been run once against real Google and the outcome recorded on #317.

**Acceptance:** CI runs device-only, user-only, and both-blobs adapters through a fake IdP with no browser; key rotation and stale-cache paths exercised; pin at `v0.19.0`.

---

## Deferred (tracked, not scheduled)

| ID | Item | Why deferred |
|---|---|---|
| X1 | `[bootstrap] expiration_seconds` (device auth lifetime from policy) | New `Policy`-level capnp field + compiler + VS; independent of OIDC; device stays at `DEFAULT_AUTH_EXPIRATION`. |
| X2 | Per-namespace graceful degradation on user-auth expiry | Needs partial revocation (`revokeAuthentication` is per actor) — spec *Deferred*. |
| X3 | Refresh tokens / `offline_access` / OS keyring in `ph-cli auth-agent` | `allow_offline_access` plumbing exists end to end after A–D; the agent returns `NonInteractiveUnsupported` until this lands. |
| X4 | Remove `ac @1` from `AuthBlob` and `zpr-oauthrsa` from `ZPR_L7_BUILTINS` | Schema/compiler breaks; do in a later coordinated bump. |
| X5 | `systemd` user unit for `ph-cli auth-agent` | Packaging; spec open question. |
| — | `[bootstrap]` entries as user credentials; non-Google providers; A2A gaps | Spec *Deferred*. |

---

## Self-review against the spec

- **Spec coverage.** *Prerequisites*: done (recorded). *Architecture* channels: A2/A3/C3/C4/D1/D2/D3. *Three authentication cases*: C1/C5 + `zpt` in C5 + integration in D5. *Adapter/CLI/daemon split*: D2/D3 (AuthAgent, two registrars, `interactive` flag; FSM `WaitForUserAuth`; error taxonomy → `AuthFailureReason` + exit codes). *Credential lifetimes*: C0 (namespaced authority, min-expiry), C5 (`auth_time + expiration_seconds`, `exp` reject-only), `allow_offline_access` plumbed (X3 for the agent side). *ZPLC configuration* rules table: B1 (each row is a test) and B2 (weaving, `[services.google-*]` error). *Compiled policy*: A1/A3/B2, version floors. *ZPL language impact: none*: no grammar task, correct. *Visa service changes* table: rows 1–8 in C0–C5; rows "revision cache" and "attribute refresh" already done (recorded). *Wire format changes*: D1 (array blobs, `OIDC` blob), A2/A3/D1 (descriptor + TLV), D2 (AAA gate mode-aware). *Security requirements*: Global Constraints + C2 vectors + D3 tests + C3 `CONNECT` assertion + D4 deletion. *Testing*: compiler fixtures (B1/B2), JWT table (C2), `zpt` (C0/C5), VS unit tests (C0/C1/C5), adapter unit tests (D1/D3), fake-IdP integration (D5), manual checklist (D5). *Open questions*: proxy existence → C3 degrades to seed keys, D5 stands one up in test; HTTP client → resolved; `client_secret` → optional end to end, settled in D5; systemd unit → X5.
- **Placeholder scan.** No TBD/TODO; every rejected property has its error text; every test is named with its assertion.
- **Type consistency.** `OidcConfig` field names identical in Contract 1 (capnp), A3 (Rust), B1 (`OidcTsConfig` — compiler-side, adds `seed_jwks_path`, resolved to `seed_jwks` in B2). `OidcBlob { issuer, id_token, nonce }` identical in A2/A3/D1. `OidcClientConfig` (A2/A3) and `OidcIdpInfo` (D1/D2/D3) carry the same five fields. `AuthFailureReason` defined in D2, consumed by D3's exit codes. `BlobOutcome` (C1) consumed by C5. `KeySource`/`IdpParams`/`ValidatedToken` (C2/C3) consumed by C4/C5.

---

## Filing procedure (proposed; nothing filed yet)

1. **Commit the spec** to `zpr-dev-context/docs/OIDC.md` (with an `## Implementation status` section per the `docs/` convention) so every issue can link a stable URL, and drop this plan beside it as `docs/plans/2026-09-02-oidc.md`. Alternative: paste the spec into #317's body; weaker because it will not track implementation status.
2. **Rewrite zpr-visaservice#317** as the umbrella: title `OIDC authentication (umbrella)`, body = the *Goal*, *What changed since the spec*, *Dependency graph*, *Release choreography*, and the *Issue map* table with live links.
3. **Create the sub-issues** with `gh issue create --repo org-zpr/<repo> --title … --body-file <section>.md --label enhancement` (C0 gets `bug` too), then attach each as a GitHub sub-issue of #317 (cross-repo is allowed within an org):
   ```sh
   CHILD_ID=$(gh api repos/org-zpr/<repo>/issues/<n> -q .id)
   gh api -X POST repos/org-zpr/zpr-visaservice/issues/317/sub_issues -F sub_issue_id="$CHILD_ID"
   ```
   Order of creation = the Issue map order, so the sub-issue list reads top-to-bottom as the execution order. Record "blocked by" as the first line of each body (GitHub has no native blocked-by field).
4. **Project board:** add every sub-issue to `ref impl` (project 1) with Status `Todo`; put A1, A2, B1, C0, C2, D3 into the current iteration (they have no blockers).
5. **At pickup** of each issue, the assignee expands its section into the bite-sized TDD plan as an issue comment (ZPR skill rule), implements on `<login>/<issue#>-oidc-<id>`, and notes deviations in the PR.
