# Silent OIDC Re-authentication

> **Status: FILED AND BUILT (2026-09-17).** Umbrella
> [zipline#40](https://github.com/mkolehmainen/zipline/issues/40), seven tasks
> across `zl-zpr-compiler`, `zl-zpr-visaservice` and `zl-zpr-core`:
> R1 [#41](https://github.com/mkolehmainen/zipline/issues/41),
> R2 [#42](https://github.com/mkolehmainen/zipline/issues/42),
> R3 [#43](https://github.com/mkolehmainen/zipline/issues/43),
> R4 [#44](https://github.com/mkolehmainen/zipline/issues/44),
> R5 [#45](https://github.com/mkolehmainen/zipline/issues/45),
> R6 [#46](https://github.com/mkolehmainen/zipline/issues/46),
> R7 [#47](https://github.com/mkolehmainen/zipline/issues/47). R1–R6 are merged
> and closed; see *Issue map* below.
>
> **The loop does not close yet.** R7's end-to-end test found that the renewal
> tick, the tracked `auth_expires` and the visa-service connection all live on
> the **node's** `NodeToAdapter` link, while the `AuthAgent` `ph-cli` registers
> lives on the **adapter's** `AdapterToNode` link — so the node reaches its
> renewal deadline with no agent to ask. R5's design (below) assumed the two
> met on one link and its unit tests set both by hand, which is a state the
> production paths never produce together. Closing it needs a node-to-adapter
> credential request, for which no ZDP message exists; the plan's "no schema
> change is needed anywhere" claim covered the VSAPI and missed this hop.
> Tracked on [#47](https://github.com/mkolehmainen/zipline/issues/47).

**Supersedes:** OIDC-X3 in `docs/plans/2026-09-02-oidc-implementation-plan.md`
(*"Refresh tokens / `offline_access` / OS keyring in `ph-cli auth-agent`"*), and
resolves the *Credential lifetimes and re-authentication* open items in
`docs/OIDC.md:366-470`.

---

## Context

A user authenticated through Google is forced back through an interactive
browser login every `expiration_seconds` — one hour in the `mk1` testnet. There
is no renewal path at all: the VS's `reauthorize` handler is `unimplemented`
(`vs/src/vsapi_worker.rs:1384`), the node discards the authentication expiry it
is handed (`adapter/ph/src/visa_mgmt.rs:57`), and `ph-cli` throws away the
`refresh_token` from the token exchange (`adapter/cli/src/oidc.rs:273`). When the
hour runs out the actor becomes a zombie: the link stays up and authenticated
while every visa request silently fails, because the expired `user.zpr.authority`
can no longer satisfy the `has user.zpr.authority` condition
(`libeval/src/eval.rs:567`).

The blocker is not plumbing, it is the lifetime formula. The VS computes

```rust
// vs/src/connection_control.rs:571
let expires = token.auth_time + svc.lifetime();
```

and `auth_time` records *when the human last typed a password*. It does not move
when a credential is renewed. A refresh grant returns a new `id_token` whose
`auth_time` is unchanged, so under this formula renewal computes **the identical
expiry** and buys nothing. Anchoring the honored lifetime on the credential
rather than on the authentication event is the prerequisite for any silent
renewal, which is why this plan reworks the computation before touching the RPCs.

**Intended outcome:** a user logs in interactively once, works for a
policy-bounded session (12h by default) with renewals happening invisibly in the
background, and is re-prompted only when the session ceiling is reached or the
IdP withdraws the grant.

---

## The lifetime rework

`docs/OIDC.md:420` names three clocks and warns against conflating them. Silent
renewal needs a fourth fact tracked, and turns the three into two enforced
bounds.

| Value | Source | Moves on renewal? | Role |
|---|---|---|---|
| `exp` | token | yes | reject-only at validation. **Unchanged.** |
| `auth_time` | token | **no** | the human's login moment. Anchors the session ceiling. |
| `iat` | token | **yes** | **newly tracked.** When this credential was minted. Anchors the renewal window. |
| `expiration_seconds` | policy | — | renewal cadence |
| `max_auth_age_seconds` | policy | — | session ceiling |

```
renewal_expires = token.iat       + expiration_seconds      # fast clock, advances
session_expires = token.auth_time + max_auth_age_seconds     # slow clock, fixed
user.zpr.authority.expires = min(renewal_expires, session_expires)
```

`max_auth_age_seconds == 0` leaves `session_expires` unbounded, preserving
today's single-clock behaviour with the anchor moved from `auth_time` to `iat`.

**This anchor move is a deliberate behaviour change, and it needs the ceiling to
be safe.** Today, a token minted from a long-lived Google session (old
`auth_time`, fresh `iat`) yields a short or already-expired lifetime. After the
change it yields a full one. That is the fix being asked for — expiry tied to the
credential — but without a ceiling it means a browser session that Google keeps
alive for weeks can be re-presented indefinitely. Hence R1's compiler check.

**The `iat` fallback is a trap and must be closed.** `validate.rs:186` reads
`auth_time` and *falls back to `iat`* when the claim is absent. Google omits
`auth_time` unless `max_age` is sent in the authorization request. With the
fallback in place, `session_expires` would silently become
`iat + max_auth_age_seconds` — a value that advances on every renewal, making the
ceiling unenforceable while appearing to work. R2 rejects the fallback whenever
`allow_offline_access` is true, and R6 sends `max_age` so Google returns the
claim.

**Where the new values live.** `ValidatedToken` gains `iat`. The OIDC store's
admission cache entry grows from `(attrs, expires)` to carry `auth_time` and
`iat` as well (`vs/src/oidc/store.rs:126`), which is needed anyway for R3's
renewal checks. No new attribute and no new DB column: the effective `min` is
stamped onto `user.zpr.authority` exactly as today, so
`get_authentication_expiration` (`libeval/src/actor.rs:176`) and
`compute_expiration` (`vs/src/visareq_worker.rs:421`) keep working untouched —
visas continue to clamp to the authentication, they just clamp to a value that
now moves forward.

That cache is in-memory (`DashMap`), so a VS restart loses `auth_time`/`iat` and
forces OIDC actors to reconnect. That is already true today — the admission cache
is what vends their attributes — so restart behaviour is unchanged.

---

## Decisions taken

| # | Decision | Consequence |
|---|---|---|
| 1 | **Relax the nonce on `reauthorize` only.** | OIDC Core §12.2 says a refresh-grant `id_token` **SHOULD NOT** carry a `nonce`, and that if present it MUST be the original authorization request's — absent or original, never fresh — so it can never match a fresh challenge. On the reauth path the VS therefore performs no nonce check at all (accepting both branches, rather than requiring the claim) and binds to the live session instead: same `sub`, strictly increasing `iat`, unchanged `auth_time`, and a `zprAddr` that is a live actor on the calling node. Connect-path nonce checking is untouched. Recorded as a `SECURITY_MODEL.md` delta. **Correction (2026-09-18):** this row originally said the original nonce *must* be carried, which inverts §12.2's normative direction; the relaxation is unaffected but the fake IdP was modelling the discouraged branch and now omits the claim. |
| 2 | **Refresh token in-memory in `ph-cli auth-agent` only.** | `auth-agent <id>` already "runs until interrupted" (`main_args.rs:87`). The token lives in that process and dies with it: silent renewal for the whole working session, no at-rest credential, no new dependency, none of the persistence risk `docs/OIDC.md:455` weighs. Keyring persistence stays deferred and needs no redesign to add. **Caveat — see *Running as root* below:** the spec's mitigation that the token "lives in the *user's* session rather than the root daemon" (`docs/OIDC.md:459`) does **not** hold while `ph-cli` requires `sudo`. |
| 3 | **Reuse `max_auth_age_seconds` as the session ceiling.** | It already means "how old may the human's login be" and is already enforced at `validate.rs:202`. No new `policy.capnp` field, no schema bump. |
| 4 | **Log and disconnect when renewal fails.** | Matches the position stated at `docs/OIDC.md:468`. Requires making `revokeAuthentication` real: it is a stub on both sides (`vs/src/vss_worker.rs:163`, `adapter/ph/src/vss_worker.rs:36`). Per-namespace graceful degradation stays deferred as X2. |

---

## User-facing flow

Renewal only happens while an `AuthAgent` is registered and its process alive, so
which command a human runs decides whether they get it. No code change is needed
to make this work — `auth-agent` already calls `startLink` and therefore *starts
the link itself* — but it does need documenting, because today `connect` is the
obvious-looking choice and is the one that cannot renew.

| Command | Starts the link | Reports the outcome | Stays to renew |
|---|---|---|---|
| `ph-cli auth-agent <id>` | yes | no (`show-link`) | **yes** — blocks on SIGINT |
| `ph-cli connect <id>` | yes | yes, with exit codes | no — returns as soon as the link is up |

**The interactive flow is two steps, not three:**

```
1. ph (adapter) running               — daemon, systemd
2. ph-cli auth-agent <id>             — starts the link, browser opens once,
                                        process stays resident and renews
```

`ph-cli connect` is redundant in that flow. It keeps its value for CI and scripts,
where the blocking outcome report and the exit-code contract (2 declined,
3 timeout, 4 IdP unreachable, 5 VS rejected token, 6 policy denied, 7 device blob
rejected) are the point and the short process lifetime makes renewal moot.

**Do not run `connect` after `auth-agent` on the same link.** Both register, and
`set_auth_agent` (`link_state.rs:440`) overwrites — so the second registration
replaces a live agent with one that is about to exit.

### Running as root

`ph` needs root for the tun interface, and the control socket is bound with no
ownership or mode set (`adapter/ph/src/main.rs:254`), so `ph-cli` needs root too.
That is tracked separately as
[zipline#39](https://github.com/mkolehmainen/zipline/issues/39) and is **not** a
dependency of this plan. Until it lands, this plan assumes the agent runs as root,
which has three consequences:

- **Renewal is unaffected.** The refresh grant is a back-channel HTTPS POST to the
  token endpoint: no browser, no desktop, no loopback listener. Root is fine for it.
  The root constraint only ever touches the *interactive* leg.
- **The initial login needs `--no-browser`, and that is the documented norm here,
  not a workaround.** The printed URL is pasted into the user's own browser, which
  redirects to `http://127.0.0.1:<port>/callback` — a plain loopback TCP connect
  into the root process, so the cross-user boundary is irrelevant. The
  `OIDC_LOGIN_TIMEOUT` of 300s bounds the paste. Net effect versus today: the user
  pastes a URL **once per session ceiling** instead of once per
  `expiration_seconds`.

  ```
  sudo ph-cli auth-agent 2 --no-browser
  ```

- **Decision 2's rationale is weakened, and the plan says so rather than inheriting
  it.** The refresh token sits in a root-owned process for hours. The marginal risk
  is small — root already owns the tun device and could harvest an `id_token` at any
  interactive login — but the spec's "user's session, not the root daemon" mitigation
  is void until zipline#39 lands. `docs/SECURITY_MODEL.md` records it as written here.

---

## What the code does today (verified 2026-09-16)

| Assumption | Reality | Effect on plan |
|---|---|---|
| Re-authentication needs new wire format | **No schema change is needed anywhere.** `reauthorize @2 (req :ReauthRequest)` exists (`vs.capnp:328`); `ReauthRequest { zprAddr, blobs }` (`:379`) already accepts an `oidc` arm; `Connection { zprAddr, authExpires }` (`:477`) already carries the renewed expiry; `AuthAgent.getOidcCredential` already takes `allowOfflineAccess` and `interactive` (`cli.capnp:32`). | Zero work in `zl-zpr-policy`, `zl-zpr-vsapi`, `zl-zpr-common`. No version floors move except the compiler's. |
| `reauthorize` is partly wired | Handler returns `capnp::Error::unimplemented` (`vsapi_worker.rs:1384`). No node-side caller exists. | R3 + R5 build both ends. |
| The node knows when its actors' auth expires | It is told and discards it. `visa_mgmt.rs:57` destructures the response and uses `cr.zpr_addr` only; `LinkData` (`link_state.rs:258`) has no expiry field. Only `lntest` reads `auth_expires` (`libnode2/src/cli/handler.rs:193`). | R5 stashes it in `LinkData`. |
| `revokeAuthentication` works | Stubs on both sides. VS: `"revoke-auths not implemented"` (`vss_worker.rs:163`); the `vss_mgr::revoke_auths` wrapper (`:195`) is `#[allow(dead_code)]`. Node: `"not yet implemented"` warning, then a success ack (`adapter/ph/src/vss_worker.rs:36`). | R4 + R5 implement both. Decision 4 depends on it. |
| Something fires at expiry | Nothing does. Refresh is event-driven (`event_mgr.rs` on policy/trusted-service change) plus lazy on the next visa request. No timer. | R4 adds a sweep, mirroring `KeySource::spawn_refresher` (`vs/src/oidc/jwks.rs:321`, called at `policy_mgr.rs:133`). |
| `ph-cli` can do a non-interactive credential | `interactive = false` is rejected outright as `NonInteractiveUnsupported` (`adapter/cli/src/oidc.rs:296`), and the token-exchange parser keeps only `id_token` (`:273`). `allow_offline_access` reaches the CLI (`:346`) and is never used. | R6. |
| The renewal needs a new timer in `ph` | The keep-alive/echo tick already runs every 3s in `Active` (`config.rs:98`, armed in `link_state.rs` on entry to `Active`). | R5 rides the existing tick; no new timer. |
| `zpt` can test this end to end | `zpt` is a policy-eval harness fed pre-authenticated claims (`connect --ac k:v`, `integration-test/pregen/zpt-test-oidc.zpt`). It never exercises blob validation or RPCs. | R2's lifetime maths gets `zpt` expiry assertions; the renewal RPC is covered by VS unit tests and the R7 fake-IdP e2e. |

---

## Global constraints

Inherited from `docs/plans/2026-09-02-oidc-implementation-plan.md:34`, still
binding. Additions specific to this work:

- **Never log a refresh token.** Same rule as tokens, codes, and verifiers. The
  refresh grant's error body is free text from the IdP and must not be echoed —
  extract only RFC 6749 §5.2's `error` code, exactly as `oidc.rs:264` already does.
- **No new dependency trees.** Decision 2 exists partly to avoid the `keyring`
  crate. The refresh grant is one more `reqwest` form POST to the already-known
  token endpoint.
- **The connect path's nonce check is not touched.** Relaxation applies only to
  the `reauthorize` entry point, and the two paths must not share a validation
  function that takes a "skip nonce" boolean — pass the nonce expectation as an
  enum so the connect arm cannot be constructed with checking off by accident.
- **Renewal must never open a browser.** `interactive = false` means
  "satisfy from a stored refresh token or fail", per the `cli.capnp:28` contract.
- Every non-trivial function gets a doc comment; a found bug gets a failing test
  first; `make check` and `make test` clean, warnings are errors.

---

## Issue map

Work lands in three repositories. Suggested EPIC in `mkolehmainen/zipline` with
seven sub-issues.

| ID | Repo | Scope | Depends on |
|---|---|---|---|
| R1 | `zl-zpr-compiler` | `allow_offline_access` validation rules + fixtures, minor bump | — |
| R2 | `zl-zpr-visaservice` | Lifetime rework: track `iat`, dual-clock `min`, close the `iat` fallback | — |
| R3 | `zl-zpr-visaservice` | Implement `reauthorize` with session-bound validation | R2 |
| R4 | `zl-zpr-visaservice` | Authentication-expiry sweep + real `revokeAuthentication` | R2 |
| R5 | `zl-zpr-core` | Node: track `auth_expires`, schedule renewal, call `reauthorize`, honour revocation | R3, R4 |
| R6 | `zl-zpr-core` | `ph-cli auth-agent`: offline access, in-memory refresh token, non-interactive grant | R1 (for the policy shape) |
| R7 | `zl-zpr-core` + `zl-zpr-dev-context` | Fake-IdP renewal e2e + documentation | R5, R6 |

Order: R1 ∥ R2 → R3 ∥ R4 → R5 ∥ R6 → R7.

---

## R1 — Compiler validation (`zl-zpr-compiler`)

`src/config/trusted_service.rs` already requires `expiration_seconds > 0` for
`api = "oidc"` (`:344`) via `parse_expiration_seconds` (`:86`). Add, in the same
validation function so every OIDC declaration routes through it:

- `allow_offline_access = true` requires `max_auth_age_seconds > 0`.
  Error: `trusted_service {id}: allow_offline_access requires max_auth_age_seconds (the session ceiling)`
- `max_auth_age_seconds`, when non-zero, must be `>= expiration_seconds`. A
  ceiling shorter than the renewal cadence makes renewal unreachable.
  Error: `trusted_service {id}: max_auth_age_seconds must be >= expiration_seconds`

**Acceptance:** two `bad-oidc-*.zplc` fixtures asserting each message verbatim;
one `test-oidc-offline.zplc` that compiles and `zpdump`s with
`allow_offline_access = true`, `expiration_seconds = 3600`,
`max_auth_age_seconds = 43200`. Compiler minor bumped; the VS's
`POLICY_MIN_COMPILER_MINOR` raised to match in R2.

## R2 — Lifetime rework (`zl-zpr-visaservice`)

- `oidc/validate.rs`: add `iat` to `ValidatedToken`. Split the `auth_time`
  resolution so the `iat` fallback (`:186`) is available only when the provider
  does **not** set `allow_offline_access`; with offline access on, a missing
  `auth_time` is `OidcError::Rejected("auth_time required for a renewable session")`.
- `oidc/store.rs`: `admit()` records `auth_time` and `iat` beside the attrs and
  expiry; add `session_ceiling()` returning
  `auth_time + max_auth_age_seconds` (or `None` when the knob is 0).
- `connection_control.rs:571`: replace the single expression with the dual-clock
  `min`. Keep the comment's explanation of *why* `exp` plays no part.
- Raise `POLICY_MIN_COMPILER_MINOR` to R1's compiler minor.

**Acceptance:** unit tests for (a) `iat`-anchored renewal window, (b) ceiling
wins when it is sooner, (c) missing `auth_time` + offline access is rejected,
(d) missing `auth_time` without offline access still falls back to `iat`,
(e) `max_auth_age_seconds = 0` leaves the window unbounded. A `zpt` assertion
pinning `user.zpr.authority`'s expiry to the computed `min`.

## R3 — `reauthorize` (`zl-zpr-visaservice`)

Implement `vsapi_worker.rs:1384`. Reuse the existing connect-path machinery
rather than duplicating it: pin one `PolicySnapshot`
(`asm.policy_mgr.get_current_snapshot()`), resolve the provider with
`psnap.oidc_service_for_issuer`, and re-run `authorize_connection` so a policy
change since connect is applied.

Validation on this path, replacing the nonce equality check:

| Check | Failure |
|---|---|
| `zprAddr` is a live actor on the calling node | `ParamError` |
| provider is the same trusted service that admitted it | `AuthError` |
| `sub` equals the admitted `sub` | `AuthError` |
| `iat` strictly greater than the recorded `iat` | `AuthError` (replay) |
| `auth_time` equals the recorded `auth_time` | `AuthError` (different session — must reconnect) |
| everything else — signature, `iss`, `aud`, `exp`, `kid`, `alg`, `hd`, `email_verified` | unchanged |

Then re-`admit()`, recompute the dual-clock expiry, persist the actor, and return
`Connection { zprAddr, authExpires }`. A ceiling already passed is a clean
rejection, not an internal error — the node's cue to tear down.

**Acceptance:** unit tests per table row; a test proving the connect path still
rejects a mismatched nonce (no regression); a test that a renewed actor's
`user.*` attributes keep the same values and a later expiry; a test that
`authorize_connection` denial on reauth surfaces `PolicyDenied`.

## R4 — Expiry sweep and revocation (`zl-zpr-visaservice`)

- Implement `VssCmd::RevokeAuthsByZprAddr` at `vss_worker.rs:163`, mirroring
  `vss_do_revoke_visas`. Drop the `#[allow(dead_code)]` on
  `vss_mgr::revoke_auths` (`:195`).
- Add a periodic authentication-expiry sweep, mirroring
  `KeySource::spawn_refresher` (`oidc/jwks.rs:321`) and spawned alongside it.
  Period: `MIN_VISA_LIFETIME` (30s, `config.rs:80`) — fine-grained enough that a
  revocation lands inside the shortest possible visa lifetime. For each connected
  actor whose `get_authentication_expiration()` has passed: log with the actor's
  ZPR address and the gate, call `revoke_auths`, and drop it from the actor store.

**Acceptance:** a test that an actor past its authentication expiry is revoked by
one sweep pass and absent afterwards; a test that an actor inside its window is
untouched; a test that a VSS outage during a sweep leaves the actor for the next
pass rather than half-removing it.

## R5 — Node (`zl-zpr-core`, `adapter/ph`)

- `LinkData` (`link_state.rs:258`) gains `auth_expires: Option<SystemTime>`,
  populated from `Connection` in `visa_mgmt.rs:57` — the value currently dropped —
  and in `send_deferred_vs_connect` (`:22`).
- On the existing keep-alive tick in `Active`, renew when
  `now >= auth_expires - lead`, where `lead = min(AUTH_RENEWAL_LEAD, remaining/2)`
  and `AUTH_RENEWAL_LEAD` is a new 300s constant in `config.rs`. The halving keeps
  short `expiration_seconds` values workable.
- Renewal: call the link's registered `AuthAgent.getOidcCredential(...,
  interactive: false)`; on success build an `OidcBlob` and call
  `VSHandle.reauthorize`; on success update `auth_expires` from the response.
  Populate the blob's `nonce` with the current challenge-derived value
  (`auth.rs:289`) even though the VS ignores it on this path — no empty-string
  special case on the wire.
- **`auth_agent.is_some()` is not a liveness test.** `set_auth_agent`
  (`link_state.rs:440`) stores an `Option<AuthAgentHandle>` that is an mpsc sender
  into the bridge task at `admin_worker.rs:749`, and that task loops on
  `rx.recv()` without ever noticing its capnp client disconnected. After
  `ph-cli connect` exits — which it does as soon as the link is up — the slot
  stays `Some` holding a bridge to a dead client. Two changes:
  - The bridge task clears the link's `auth_agent` back to `None` when its
    client's RPC connection drops, so the slot reflects reality.
  - The renewal path treats a bridge send/call failure as "no agent" and clears
    the slot, rather than retrying a corpse every tick.
- Failure — no agent registered, agent error, or a `reauthorize` rejection — logs
  at `warn` with the reason and records it in `last_auth_failure` so `showLink`
  surfaces it. With **no** agent registered, do not attempt renewal at all: log
  once at the first renewal deadline and let the VS sweep revoke when the window
  actually closes. A registered-but-failing agent gets one attempt per tick.
- Implement `VSSMessage::RevokeAuth` (`adapter/ph/src/vss_worker.rs:36`): for each
  address docked here, terminate the actor and drop its visas through the existing
  disconnect path.

**Acceptance:** unit tests for the lead calculation (including `remaining/2`
clamping and an already-past expiry); a test that a link with no `AuthAgent` logs
once and attempts no renewal; a test that a dropped agent client clears the
`auth_agent` slot (the `connect`-exits case, which `test_has_auth_agent` at
`link_state.rs:466` can assert directly); a test that `RevokeAuth` for a docked
address removes its visas and for an unknown address is a no-op ack.

## R6 — `ph-cli auth-agent` (`zl-zpr-core`, `adapter/cli`)

- Authorization request: when `idp.allow_offline_access`, add the standard
  `offline_access` scope **and** `access_type=offline` with `prompt=consent`
  (Google's mechanism; RFC 6749 §3.1 requires an authorization server to ignore
  unrecognised parameters, so this is safe against other IdPs). Send `max_age` so
  Google returns a real `auth_time` — R2 now requires the claim.
- Token exchange (`oidc.rs:232`): return the `refresh_token` alongside the
  `id_token` instead of discarding it. The function's contract becomes a small
  struct; keep the existing "never echo the response body" error handling.
- The agent holds the refresh token in memory, keyed by issuer, for the life of
  the process. Never written to disk, never logged, never returned over the RPC.
- `interactive: false` (`oidc.rs:296`): if a refresh token is held, POST
  `grant_type=refresh_token` to the token endpoint and return the fresh
  `id_token`; otherwise return the existing non-interactive error. An
  `invalid_grant` response drops the stored token so the next attempt fails fast
  rather than replaying a dead credential.
- **Fix the silent browser failure.** `open_in_browser` (`oidc.rs:488`) does
  `Command::new("xdg-open").arg(url).spawn()`. Run as root, `spawn()` *succeeds* —
  the exec works and the failure happens inside `xdg-open` afterwards — so
  `OidcCliError::Browser` never fires and a user who forgets `--no-browser` hangs
  until the 300s `OIDC_LOGIN_TIMEOUT` with no diagnostic. Fall back to printing the
  URL, with a one-line explanation, when `geteuid() == 0` or neither `DISPLAY` nor
  `WAYLAND_DISPLAY` is set. Three lines, and it removes the sharpest edge in the
  root-run flow above.

**Acceptance:** a `wiremock`-style test (the pattern at `oidc.rs:780`) asserting
the refresh POST carries `grant_type=refresh_token` and the `client_secret` only
for a confidential client; a test that a non-interactive request with no stored
token never contacts the browser or the authorization endpoint; a test that
`invalid_grant` clears the stored token; a test asserting `offline_access` and
`max_age` appear in the authorization URL only when policy allows offline access;
a test that the browser fallback triggers on a headless environment and that the
printed URL carries the S256 challenge but no verifier.

## R7 — Integration and documentation

- Extend the fake-IdP e2e (the OIDC-D5 harness) with a renewal scenario: connect
  interactively, drive the clock past the renewal lead, assert a `reauthorize`
  crossed the wire, that no authorization-endpoint request was made, and that
  traffic keeps flowing across the boundary. Then revoke the fake IdP's refresh
  token and assert the actor is disconnected and its visas gone.
- `docs/OIDC.md`: rewrite *Credential lifetimes and re-authentication* for the
  dual-clock model; update *Silent re-authentication requires a refresh token* to
  record decision 2; move the `docs/OIDC.md:468` disconnect position from
  aspiration to implemented; refresh `## Implementation status`.
- `docs/SECURITY_MODEL.md`: record the decision-1 delta — what the reauth path
  binds to in place of the nonce, and why a compromised node is not a new threat.
- `adapter/cli/README` (and the `connect` / `auth-agent` `--help` text): record
  the *User-facing flow* table above — `auth-agent` is how a human logs in and
  keeps a session alive; `connect` is the scripted, non-renewing form. No code
  change, but without it users will reach for `connect` and never renew.
- `docs/plans/2026-09-02-oidc-implementation-plan.md`: mark X3 superseded by this
  plan; leave X2 deferred with a note that decision 4 chose disconnect.

**Landed as:** `integration-test/one-node-oidc-renewal-test.sh` plus refresh-grant
and `--revoke-refresh` support in `integration-test/lib/fake-idp.py`, the
`oidc-renewal.zplc` fixture (which reuses `oidc-test.zpl` unchanged), an
`oidc-renewal-integration-test` job in `.github/workflows/adapter.yml`, and the
documentation above. The netns run needs root and is the operator's step; the
fake-IdP smoke test covers the refresh grant with no privileges.

---

## Deferred

| ID | Item | Why |
|---|---|---|
| X2 | Per-namespace graceful degradation instead of disconnect | Decision 4 chose disconnect. Needs namespace-scoped revocation; `revokeAuthentication` is per actor. |
| X3a | OS keyring persistence for the refresh token | Decision 2. Additive: a storage trait behind `--persist-credential`, no redesign. |
| X3b | VS-pushed renewal via `requestAuthentication @6` (`vs.capnp:653`) | Unimplemented on both sides. Node-driven pull covers renewal and works while the VS is disconnected. Push is for "policy changed, re-prove now". |
| X3c | User-held keypair bound to `sub` at first login | The cryptographically stronger alternative to decision 1. Revisit if the nonce relaxation fails security review. |

---

## Verification

Per-repo, in dependency order:

```
zl-zpr-compiler:    make check && make test
zl-zpr-visaservice: make check && make test
                    integration-test/zpt-test-oidc.sh
zl-zpr-core:        make check && make test
```

End to end against the real `mk1` testnet, which is where the symptom was
observed:

1. Set `allow_offline_access = true`, `expiration_seconds = 3600`,
   `max_auth_age_seconds = 43200` on `[trusted_services.google]` in
   `~/src/zpr-testnets/mk1/policy/oidctest.zplc`; recompile and install.
2. `sudo ph-cli auth-agent <id> --no-browser` — this starts the link; paste the
   printed URL into a browser and complete the Google login once. Leave the
   process running. (`--no-browser` is required while zipline#39 is open; see
   *Running as root*.)
3. `ph-cli show-link <id>` — confirm the link is up and that an authentication
   expiry is now reported (it is absent today).
4. Wait past 55 minutes with traffic flowing. Expect: no browser, a
   `reauthorize` in the VS log, and the reported expiry stepping forward. This
   is the primary acceptance signal.
5. Revoke the app's access in the Google account console, then wait for the next
   renewal. Expect a logged failure, revocation, and the actor disconnected
   within one sweep period.
6. Restart `ph-cli auth-agent` and confirm the next connect is interactive —
   decision 2's intended limit.

For a same-day check without waiting an hour, set `expiration_seconds = 120`
and `max_auth_age_seconds = 600`; R1's `>=` rule permits it and the renewal lead
halves to 60s.
