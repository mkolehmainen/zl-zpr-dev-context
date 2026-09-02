# feature: OIDC

- Context: Part of the ZPR project. Implementation plan: `docs/plans/2026-09-02-oidc-implementation-plan.md`. Umbrella issue: org-zpr/zpr-visaservice#317.


## Vision

The zpr system has just one way for a user to authenticate. When the user
launchs the ph in adapter mode, authentication with the visa service happens
based on a pre-shared RSA public key.

I'd like to add a new way: Visa Open ID Connect. For example, using Google.

For this to work, the visa service needs to be configured with a trusted service
for this purpose (eg, Google).

To reach google:
- The google open ID API endpoint will need to be already proxied on the ZPR
  network, or
- The adapter (and maybe visa service) can speack normal IP to the google
  service (ie, not use a ZPR interface for the auth channel).

Is it possible to do this with zero configuration settings on the adapter? The
node has a way to tell an adapter about existing authentication services. But
does it have a way to tell the adapter the "type"? How will adapter know to kick
off authentication with Google?

The proposed flow:

- User starts the ZPR adapter.

- As the adapter is a CLI program, the adapter will prompt the user on the
  terminal. Something like:

```
Authentication with Google required. Press any key to launch a browser window.
```

- Browser launches to the Google OAuth login sequence.

- User enters credentials, and then browser redirects back to the local ZPR process.

- Adapter grabs the tokens (or whatever it is) and these are forwarded to the
  node as part of the ZPR authentication process.

- ZPR visa service gets the authentication message and confims authenticity of
  the tokens from Google. Not sure if this requries a call to google from the
  visa service.

- Visa service sends back success message and user is now authenticated to the
  ZPRnet.


## PLAN and DETAILS

### Summary

Add OpenID Connect as an authentication method for ZPR actors, using Google as
the first provider.

The central finding of the design work is that **OIDC is not a new
authentication architecture for ZPR.** ZPR already models a two-sided
authentication service: an actor-facing half that issues a credential and a
visa-service-facing half that validates it and returns attributes
(`validation/2`, see `zpr-compiler/README_ZPLC.md`). Neither half is
implemented. OIDC is the first real implementation of that slot, with Google
supplying the credential.

The adapter is the OIDC **Relying Party**, using the RFC 8252 native-app
pattern: a public client with no client secret, PKCE, and a loopback redirect.
The visa service validates the resulting `id_token` offline against Google's
JWKS. Nothing about Google is proxied onto the ZPRnet, and the visa service
needs no direct internet access.

### How this differs from the Vision above

The Vision section is preserved as written. Several of its assumptions changed
during design; where they conflict, this section governs.

| Vision assumption | Resolution |
|---|---|
| "Is it possible to do this with zero configuration settings on the adapter?" | **Yes.** A public client's `client_id`, issuer URL, and scopes are not secrets, so the node supplies them. The adapter needs no local Google configuration. |
| "Does [the node] have a way to tell the adapter the type?" | Nearly. The node already holds `ServiceDescriptor.service_uri` with a scheme (`zpr-common/src/vsapi_types/services.rs:20`), but the ZDP `ASA` TLV flattens it to a bare `SocketAddr` (`zpr-core/adapter/ph/src/mgmt/handlers.rs:285`), discarding it. See *Wire format changes*. |
| "The google open ID API endpoint will need to be already proxied on the ZPR network, or the adapter can speak normal IP" | Both, for different legs. Adapter and browser reach Google over normal IP. The visa service reaches only the JWKS endpoint, over the ZPRnet to a `CONNECT` proxy that tunnels to Google — so nothing of Google's is *proxied* onto the ZPRnet in the reverse-proxy sense, which would be unsafe. |
| "Adapter grabs the tokens ... forwarded to the node as part of the ZPR authentication process" | Correct, and the adapter exchanges the authorization code itself. The visa service receives an `id_token`, never an authorization code. |
| "Not sure if this requires a call to google from the visa service" | No per-authentication call. Only a periodic, cacheable, stale-tolerant JWKS fetch. |
| "the visa service needs to be configured with a trusted service for this purpose (eg, Google)" | Correct, and the declaration must scope *what Google is trusted to assert* — see `allowed_domains`. |

Additionally: `zpr-bas` and its hand-rolled `zpr-oauthrsa` protocol are
**deprecated and out of scope**. This design does not extend them. The existing
`OAuthRsa` client in `zpr-core/adapter/ph/src/auth.rs:431` and the hardcoded
`HARD_CODED_BAS_TLS_CERT_PEM` at `auth.rs:44` are legacy and should be removed
as part of this work.

### Prerequisites and ordering

Two pieces of work should land **before** the OIDC implementation.

**1. Class specs must emit a presence condition (compiler) — [zpr-compiler#144](https://github.com/org-zpr/zpr-compiler/issues/144).** An unconstrained
class spec currently compiles to *no* client conditions — `cli_condition` is
empty and `zpr-compiler/src/fabric.rs:192` prints `(NONE)`. So:

```zpl
allow users to access services.
```

means "any actor may reach any declared service," not "any authenticated user."
The class name `users` is decorative in the compiled output. It requires no
user attributes — not even the presence of a user identity.

This is a pre-existing gap, not one OIDC introduces, but OIDC is what makes it
reachable, because OIDC is what makes people write user-centric rules. Every
class spec should compile to at least a presence condition
(`AttrOp::has`, `zpr-policy/policy.capnp:116`) on the namespace's authority
attribute:

| ZPL | Compiles to |
|---|---|
| `allow users to access services.` | `has user.zpr.authority` |
| `allow users on laptops to access services.` | `has user.zpr.authority` and `has device.zpr.authority` |

Doing this **first** fails closed: nothing authenticates a user today, so bare
user rules go from "match everything" to "match nothing," and start matching
real users once OIDC lands. Doing it second would mean shipping OIDC on
semantics we intend to change underneath it, plus a throwaway lint.

Cost: policy fixtures that use bare `users` to mean "any actor" will stop
matching and need auditing — they should say `devices` where they mean devices.
This is honest work the corpus needs anyway, since nothing in the system
currently distinguishes the two.

Prerequisite for the prerequisite: the marker attribute must be named, and
named in a space no trusted service can claim (zpr-compiler#146). Both are
specified below under *Credential lifetimes*.

**2. Identity-keyed attribute lookups (visa service) —
[zpr-visaservice#310](https://github.com/org-zpr/zpr-visaservice/issues/310).**
`connection_control.rs:452` keys all trusted-service attribute lookups on the
CN:

```rust
for ts_results in asm.ts_mgr.get_attributes_for_actor(endpoint_cn).await {
```

The `TODO(#201 follow-up)` above it notes that policy can now declare identity
attributes but this lookup runs *before* `approve_connection` assigns identity
keys, so it needs restructuring rather than a parameter change. Issue #310
tracks it and records the intended API: *send all the IDs over the link and let
the attribute store sort it out.*

**This is security-critical once user-only authentication exists.** In the
user-only case the CN is unauthenticated and attacker-chosen (see below), so an
actor could present a valid Google login for their own account while claiming a
privileged machine's CN and inherit that machine's attributes from every
trusted service. #310 is therefore a hard dependency of user-only support, not a
cleanup.

Note this was always the intent — identity attribute values were meant to be
the lookup keys. CN was simply the first and only identity ZPR had.

### Architecture

Roles, in OIDC terms:

| Role | Who |
|---|---|
| OpenID Provider | Google |
| Relying Party | the ZPR adapter (`ph`), via `ph-cli` |
| End User | the human at the terminal |

Channels:

| Leg | Transport |
|---|---|
| Browser → Google (login) | normal IP, the host's existing internet path |
| `ph-cli` → Google token endpoint | normal IP |
| Google → loopback redirect | `http://127.0.0.1:<ephemeral>/callback` |
| `ph-cli` → `ph` | existing capnp RPC over the Unix socket in `get_data_home()` |
| adapter → node (blobs) | ZDP `AcquireZprAddressRequest` |
| node → visa service | VSAPI `ConnectRequest.blobs` |
| **visa service → Google JWKS** | **over the ZPRnet to an on-net `CONNECT` proxy, then TLS end-to-end to Google** |

The visa service has no direct internet route. It reaches the JWKS endpoint
through an on-net HTTP forward proxy: it issues
`CONNECT www.googleapis.com:443`, then performs ordinary TLS with Google
*through* the tunnel, validating Google's certificate itself.

**The proxy is untrusted.** It sees only ciphertext, so it can deny service but
cannot forge a key set. It is a reachability mechanism, not a trust component,
and the visa-service-to-proxy leg carries ZPR's own visa enforcement and A2A
integrity on top. See *Security requirements* for the alternative model that
must not be built.

Standing up the proxy is the deployer's burden, and it is off-the-shelf
software — squid, tinyproxy, anything that speaks `CONNECT` — with a ZPR
adapter in front. It is declared in policy as an ordinary TCP service, so it
needs no new L7 builtin, and several trusted services may share one proxy.

**The visa service performs no OIDC discovery.** `jwks_uri` is pinned in the
ZPLC. Discovery would require a second host (Google's discovery document is on
`accounts.google.com` while the keys are on `www.googleapis.com`), doubling the
egress surface for no benefit. The adapter still discovers normally, since it
has real internet access. A pinned `jwks_uri` is also more auditable — a policy
reader can see exactly what the visa service will fetch — and because policy is
signed, it is arguably a stronger assertion than a fetched discovery document.

Two properties make the fetch itself comfortable:

- The fetch is **public data, cacheable, and stale-tolerant.** If the gateway is
  down, authentication continues on cached keys.
- It is **periodic, not per-authentication**, so it adds no latency to the
  connection path — which matters, since the visa service "sits in the path of
  every new flow and is the obvious bottleneck" (`docs/VISA_SERVICE.md`).

For cold start, before any fetch has succeeded, an initial key set is seeded in
policy. Precedent: `zpr-compiler/src/policybuilder.rs:275` already embeds
trusted-service certificates via `write_service_cert`.

**Why the adapter and not the visa service exchanges the code.** Having the
visa service redeem the authorization code would keep "the Visa Service
performs the actual authentication" literally true, and would let it hold the
PKCE verifier so a stolen code were useless to the endpoint. It was rejected
because it requires live outbound internet from the most security-critical
component, synchronously, on every authentication — meaning a Google outage
stops all network joins, and repeated connect attempts become an outbound-HTTPS
amplification vector. The cost of the choice is that the visa service must
implement JWT validation correctly; see *Security requirements*.

### The three authentication cases

An endpoint is a 4-tuple including an optional user identity
(`docs/SECURITY_MODEL.md`). Google authenticates a **user**; it says nothing
about the device. Both must therefore be independently expressible, and all
three combinations are supported.

| Blobs presented | `device.zpr.authority` | `user.zpr.authority` | Device CN |
|---|---|---|---|
| Device RSA only *(today)* | `zpr-bootstrap` | *absent* | **authenticated** |
| OIDC only *(new)* | *absent* | `google` | **claimed, unauthenticated** |
| Both *(new)* | `zpr-bootstrap` | `google` | **authenticated** |

Consequences:

- **`connection_control.rs:220` must accept more than one blob.** It currently
  rejects anything but exactly one, though `ConnectRequest.blobs` is already a
  list on the wire.
- **The CN must stop being unconditionally authenticated.**
  `connection_control.rs:424` pushes it into `authd_claims` on every path. With
  no device blob nothing has proven it — and `auth.rs:215` notes that adapters
  using self-generated keys send no certificate, so there is no cert CN to bind
  to either. It must go to `unauthd_claims` unless a device blob validated it.
  `authorize_connection` already takes both lists.
- **Failure rule.** A blob that is *presented and fails validation* fails the
  whole connection. A blob that is *absent* is not a failure — that namespace
  simply has no authority, and rules requiring it do not match. Silently
  downgrading a rejected user credential to device-only is rejected: it produces
  a working-but-mysteriously-limited connection and invites later holes.
- Note that bootstrap RSA authenticates a "device" only because
  `libeval/src/attribute.rs:28` namespaces the CN as `device.zpr.adapter.cn`.
  Nothing cryptographic distinguishes device from user; the credential is an
  opaque possession proof. The real distinction is **liveness** — a key file
  authenticates unattended, forever, with no human present. A deployment that
  wants an unattended *user* credential (a service account) can therefore
  declare a `[bootstrap]` entry as a user credential and receive
  `user.zpr.authority:zpr-bootstrap`. This is admissible by design but not
  implemented here.

### Adapter, CLI, and daemon split

`ph` is a service, eventually started by `systemd`. A daemon cannot own an
interactive browser flow. `ph-cli` already talks to `ph` over capnp RPC
(`zpr-core/adapter/admin-api/cli.capnp`), and `setCaptureFile @3` already passes
an *interface* — so capability passing is established idiom.

**`ph-cli` performs the entire OIDC flow; `ph` never talks to Google.**

```capnp
interface AuthAgent {
    # Provided by ph-cli, called by ph when a browser (and maybe a human) is needed.
    getOidcCredential @0 (issuer :Text, clientId :Text, scopes :List(Text),
                          nonce :Data, interactive :Bool)
                       -> (result :SuccessOrError, idToken :Text);
}

# on CmdLineInter:
startLink @12 (id :UInt32, authAgent :AuthAgent) -> (result :SuccessOrError);
```

The daemon supplies what the node told it, plus the nonce from
`ZdpInitAuthenticationPayload` (`auth.rs:81`) so the `id_token` is bound to this
ZPR authentication attempt. The CLI does discovery, PKCE, browser launch,
loopback listener, and token exchange, and returns only the `id_token`.

`interactive` tells the agent whether it may involve the human.
`interactive = true` is initial login: prompt and open a browser.
`interactive = false` is renewal: satisfy the request from a stored refresh
token if `allow_offline_access` permits one, and otherwise fail rather than
surfacing a browser window the user did not ask for. See *Credential
lifetimes* for why renewal cannot be done silently through the browser.

This keeps the loopback listener and the browser in the same user session
(which is what makes the redirect work), and keeps outbound internet HTTP and
Google TLS trust decisions out of a root daemon.

**Two registrars, one interface:**

| Registrar | Shape | Use |
|---|---|---|
| `ph-cli connect` | foreground, blocking, exits when the link is up | interactive login |
| `ph-cli auth-agent` | long-running, per-login-session, a `systemd` **user** unit | unattended re-authentication |

This is the `polkit` / `ssh-agent` pattern: a privileged system daemon calls
back into a per-session agent for what only the session can provide.
`ph-cli connect` is `startLink` with an agent attached.

**Flow inside `ph-cli`:**

1. Bind a loopback listener on `127.0.0.1:0` — literal IP, never `localhost`
   (DNS rebinding), never `0.0.0.0`. The ephemeral port goes into
   `redirect_uri`.
2. Discovery: `GET {issuer}/.well-known/openid-configuration` →
   `authorization_endpoint`, `token_endpoint`, `jwks_uri`.
3. Generate the PKCE verifier and S256 challenge, and a `state`. Use the
   daemon-supplied `nonce`.
4. Prompt, then open the browser.
5. On callback: verify `state`, serve a "you can close this window" page, shut
   the listener down immediately — single use.
6. `POST` the token endpoint with code + verifier.
7. Return the `id_token`.

**FSM impact.** `do_https_authenticate` (`link_state.rs:1363`) already spawns a
task that awaits an arbitrarily slow operation and emits
`AuthenticationSuccess` / `AuthenticationFailure`, so the browser flow drops
into the existing shape. It does resolve the TODO at `link_state.rs:1417`
("Should we have a state to represent waiting-for-authentication?"): yes — add
`WaitForUserAuth` with an `OIDC_USER_INTERACTION_TIMEOUT` distinct from
`VS_AUTHENTICATION_TIMEOUT`, because existing timeouts are sized for
machine-speed operations.

**`connect` blocks** until the link is up, with progress indication, and returns
a meaningful exit code.

**Error taxonomy.** `LinkEvent::AuthenticationFailure` currently carries no
reason. Good feedback here is essential — these are the cases users hit, and
they demand different actions:

| Failure | Message must convey |
|---|---|
| Wrong Google account | "signed in as alice@gmail.com; this network accepts only example.com" |
| Google `access_denied` | the user declined at the consent screen |
| Timeout | no response within the interaction timeout |
| Discovery / network | cannot reach the issuer — not an auth problem |
| Visa service rejected the token | misconfiguration, **not** the user's fault; distinct exit code |
| Device blob failed | do not blame the Google login |
| **Authenticated, but policy denied join** | login worked; this endpoint is not permitted |

The last is the sharpest: "your login failed" and "your login succeeded but
policy will not admit this endpoint" require completely different responses and
are currently indistinguishable. A reason code must propagate visa service →
node → adapter → CLI.

### Credential lifetimes and re-authentication

`vs/src/config.rs:78` hardcodes a 4-hour authentication lifetime:

```rust
pub const DEFAULT_AUTH_EXPIRATION: Duration = Duration::from_secs(4 * 60 * 60);
```

It is a compile-time constant, not even a `vs.toml` setting, applied to
`zpr.authority` at `connection_control.rs:482` regardless of which credential
was presented. Nothing derives it from anything. It is a placeholder, and it is
not set in stone.

`docs/SECURITY_MODEL.md` already states the rule the code does not follow:
expirations are set by the source, policy may **shorten but not extend** them,
and an identity's lifetime is the minimum across its component
authentications.

**Move the authority attribute into the class namespaces.**
`libeval/src/attribute.rs:31` defines `AUTHORITY` as `zpr.authority` — outside
any class namespace, which is exactly why there is one global authority per
actor. It becomes `user.zpr.authority` and `device.zpr.authority`, each with
its own expiration, and each an identity key for its namespace.

**The `zpr.` infix is deliberate: the attribute stays reserved.** Moving into
the class namespaces buys per-namespace expiry and per-namespace identity keys.
It is not about handing the attribute to the deployment. `<class>.zpr.*` is the
established spelling for a ZPR-owned attribute living inside a class domain —
`device.zpr.adapter.cn` and the tag encoding `<class>.zpr.tag.<name>` both use
it. The bare spelling `user.authority` would instead sit in the deployment's own
attribute space, where "authority" is a plausible business attribute (signing
authority, approval authority, an org authority level), so a trusted service
could declare it in good faith and forge the marker. The cost is that the rare
method-discriminating rule reads longer; the common case never names the
attribute at all.

The governing invariant: **a `<ns>.zpr.authority` attribute is installed exactly
when a non-expired authentication exists for that namespace.** So the *presence*
of `user.zpr.authority` is the marker that a live user authentication exists
(this is what the compiler prerequisite, zpr-compiler#144, depends on), and its
*value* records the method:

```zpl
allow users to access services.                                      # any authenticated user
allow user.zpr.authority:google users to access finance-services.    # interactively authenticated
```

That invariant holds only while nothing but the visa service can install the
attribute, and today nothing enforces that: a declared trusted service can name
any key in a class domain's `zpr.` space on the right of a `returns_attributes`
mapping, the CN included. zpr-compiler#146 adds the reserved-namespace check at
that declaration site. Until it lands, the marker is advisory.

**OIDC has three clocks. Conflating them is the trap.**

| Claim | Meaning | Use |
|---|---|---|
| `exp` (~1h for Google) | how long the *assertion* is fresh | reject a token presented after it — nothing more |
| `auth_time` | when the human actually logged in | anchor the ZPR lifetime here |
| policy `expiration_seconds` | how long ZPR honors that login | the shortening knob |

Setting the ZPR user-authentication lifetime from the token's `exp` would make
users re-authenticate **hourly**. The rule is
`auth_time + expiration_seconds`, with `exp` used only to reject a stale
assertion at validation time.

**RSA bootstrap has no source-imposed expiry.** The source is the visa service
checking a public key from its own policy. Re-proving possession of a static key
file demonstrates nothing new: revocation in ZPR is immediate, every packet is
visa-checked and A2A-integrity-checked, and attribute freshness is handled by
*attribute* expiry, which refreshes independently. The device lifetime is
therefore a policy knob with no natural value.

Device and user lifetimes are **independent**. Tying the device lifetime to the
user's was considered and rejected: it couples an unattended credential to an
attended one for no security gain and forces device re-authentication churn
driven by an unrelated event. The good part — an endpoint being only as fresh as
its least-fresh component — comes from per-namespace expiry without the
coupling.

**Silent re-authentication requires a refresh token.** `prompt=none` is a
front-channel redirect requiring the browser's cookie jar; web applications use
a hidden iframe, but a native agent has no equivalent and would pop a visible
browser window on every renewal. True background renewal needs
`offline_access` and a back-channel POST, as `gcloud`, `aws sso`, and `gh` all
do. Because that is a real trade, it is a policy decision:

```toml
allow_offline_access = false   # default
```

The trade: a refresh token lets an attacker who has compromised the endpoint
keep authenticating as that user without the user present, until revoked.
Against that, the token lives in the *user's* session rather than the root
daemon, belongs in the OS keyring, and is scoped to `openid email profile`
with no Google API access — and a compromised endpoint can already harvest an
`id_token` whenever the user logs in, so what is added is persistence, not
initial access. Deployments that will not accept persistence leave it off and
accept a prompt every `expiration_seconds`.

**When user authentication expires with no agent registered:** log and
disconnect. Graceful degradation — dropping only the user namespace so
device-only rules keep working while user rules stop matching — is the correct
long-term behavior and is enabled by per-namespace expiry. It needs two changes
beyond this work. First, `get_authentication_expiration`
(`libeval/src/actor.rs:175`) returns a single actor-level expiration computed as
the **minimum** over the authority and identity-key attributes, so one expired
namespace expires the whole actor and triggers disconnect. Second,
`revokeAuthentication(addrs)` (`vs.capnp:640`) revokes an actor wholesale rather
than a namespace. Revoking the *visas* that depended on the expired namespace
needs nothing new — `visa_reconciler` already re-evaluates live visas when
attributes change. Deferred.

### ZPLC configuration

A new trusted-service API, modeled on `api = "file"` — the existing precedent
for a trusted service with **no ZPR network presence**.

```toml
[trusted_services.google]
api             = "oidc"
issuer          = "https://accounts.google.com"
jwks_uri        = "https://www.googleapis.com/oauth2/v3/certs"
client_id       = "1234567890-abcdef.apps.googleusercontent.com"
scopes          = ["openid", "email", "profile"]
allowed_domains = ["example.com", "eu.example.com"]   # REQUIRED
seed_jwks       = "google-jwks.json"                  # cold-start keys
service         = "google-jwks-proxy"                 # optional; defaults to google-vs

expiration_seconds   = 43200    # honor a login for 12h from auth_time
max_auth_age_seconds = 86400    # optional: refuse an older login
allow_offline_access = false

returns_attributes = [
  "sub   -> user.oidc-subject",
  "email -> user.email",
  "hd    -> user.domain",
]
identity_attributes = ["sub"]

[services.google-jwks-proxy]
protocol = "tcp"
port     = 3128
provider = [["device.zpr.adapter.cn", "proxy1.zpr"]]

[bootstrap]
expiration_seconds = 14400   # device auth lifetime; 0 = life of the installed policy
```

**The JWKS proxy needs no hand-written ZPL.** `service` already means "the
service ID used in the **services** block for the visa-service facing service
provided by this trusted service" (`README_ZPLC.md`), defaulting to
`<TSNAME>-vs`, and the compiler already weaves visa-service-to-trusted-service
communication rules from the trusted-service declaration for `validation/2`.
The `CONNECT` proxy is exactly a visa-service-facing on-net service, so the
existing property and the existing weaving apply unchanged, and the compiler
generates the allow rule.

Compiler rules:

| Property | Rule | Why |
|---|---|---|
| `service` | **optional, warned if omitted** | names the JWKS `CONNECT` proxy. Omitting it means the visa service needs direct internet egress; `zplc` warns, and `--Werror` makes that fatal for deployments that care. Omission keeps the integration test simple (fake IdP on localhost, no proxy) without making a permissive posture silent. |
| `client` | **rejected** | the adapter talks to Google directly; there is no on-net client-facing service |
| `provider` | **rejected** | no ZPR actor provides Google |
| `cert_path` | **rejected** | with `CONNECT`, the tunneled TLS to Google is what gets verified, against system roots; the visa-service-to-proxy hop needs no pinned certificate because ZPR already protects it |
| `jwks_uri` | **required** | the visa service does no discovery; see *Architecture* |
| `allowed_domains` | **required**; `["*"]` is the explicit "any Google account" opt-in | required-with-explicit-opt-out fails closed when forgotten |
| `identity_attributes` | **required**, must be `["sub"]` | identity attributes must be immutable and unique |
| `issuer` | `https://`, no query or fragment | discovery is runtime; the compiler must do no network I/O or builds stop being reproducible |
| `[services.google-*]` | **error if present** | unlike `validation/2`, there is no on-net service either side |

Two deliberate calls:

**Reject `email` as an identity attribute, with a specific error message.**
Workspace email addresses are mutable and reusable after an employee leaves;
`sub` is the stable pairwise identifier. This is the highest-value new compiler
check in the feature — it catches a misconfiguration that would otherwise
silently transfer a departed employee's network access to their replacement.

**No new `zpr-*` L7 builtin.** `ZPR_L7_BUILTINS`
(`zpr-compiler/src/protocols.rs:35`) exists to generate *on-net communication
rules*. With the adapter as RP there is no on-net authentication traffic to
permit, and adding an entry would recreate the hand-rolled-protocol shape being
retired.

**`allowed_domains` is a scoping of the trusted source, not an access rule.**
`docs/SECURITY_MODEL.md` puts it plainly: attributes are only as trustworthy as
their source, which is why trusted services are named in policy rather than
discovered. Trusting "Google" is trust in two billion accounts; what a
deployment means is *Google is trusted to speak for example.com*. An `id_token`
for `alice@gmail.com` is not a forgery — it is a true assertion outside the
scope of what this source is trusted for. So an out-of-domain token yields no
user identity at all, no `user.*` attributes appear, and no rule anywhere can
admit that account. This is why the domain constraint does **not** need to
appear in every policy rule.

### Compiled policy

`Service.kind.trusted @3 :Text` (`zpr-policy/policy.capnp:70`) already takes an
API name, so `trusted("oidc")` needs no union change. The IdP configuration has
nowhere to live:

```capnp
struct TrustedService {
  serviceId         @0 :Text;
  expirationSeconds @1 :UInt32;
  returnsAttrs      @2 :List(AttrMapping);
  identityAttrs     @3 :List(Text);
  oidc              @4 :OidcConfig;   # only when kind is trusted("oidc")
}

struct OidcConfig {
  issuer            @0 :Text;
  jwksUri           @1 :Text;   # pinned; the VS performs no discovery
  clientId          @2 :Text;
  scopes            @3 :List(Text);
  allowedDomains    @4 :List(Text);
  maxAuthAgeSeconds @5 :UInt32;
  allowOfflineAccess @6 :Bool;
  seedJwks          @7 :Text;   # JSON JWKS, cold start only
  jwksProxyService  @8 :Text;   # service ID of the CONNECT proxy; empty = direct egress
}
```

New capnp field numbers are backward compatible, but the visa service's minimum
compiler version must bump from 0.15.0 — the constants are
`POLICY_MIN_COMPILER_{MAJOR,MINOR,PATCH}` at `vs/src/config.rs:29-31`. This is the
canonical three-repository change: grammar and config in `zpr-compiler`, schema
in `zpr-policy`, consumer in `zpr-visaservice`.

### ZPL language impact: none

No grammar change is required. Three candidates were checked:

**Policy against Google-derived attributes** — already works;
`returns_attributes` lands them in the normal namespace:

```zpl
allow domain:example.com employees to access wiki-services.
never allow domain:contractors.example.com users to access finance-services.
```

**Requiring both a user and a device authentication** — already expressible,
because `on` is positional (`docs/ZPL.md:184`):

```zpl
allow sales employees on managed laptops to access customer databases.
```

That statement *is* "an OIDC-authenticated user on an RSA-authenticated
device." With no OIDC blob the `user.*` attributes are absent and it cannot
match.

**Requiring a specific authentication method** — expressible once `authority`
is namespaced, via ordinary attribute matching (see *Credential lifetimes*).

No new ZPL statements are required of a deployment either. The visa
service-to-JWKS-proxy rule is woven by the compiler from the trusted-service
declaration — see *ZPLC configuration*.

Alternatives considered and rejected for the domain problem: forcing every rule
to use a domain-constrained subclass (opt-in security with a silent failure
mode; scatters a deployment-wide invariant across N rules), and having the
compiler silently inject the domain constraint into every user spec (introduces
action-at-a-distance, where policy text reads broader than what it compiles
to — unacceptable for a system whose goal is auditability).

### Visa service changes

| Change | Location |
|---|---|
| Accept more than one blob | `connection_control.rs:220` |
| Implement the OIDC blob arm (currently `"external auth not yet supported"`) | `connection_control.rs:253` |
| JWT validation against cached JWKS | new module |
| JWKS fetch via `CONNECT` proxy (or direct), with cache and stale tolerance | new |
| Weave the visa-service-to-JWKS-proxy allow rule from the `service` property; warn when omitted | `zpr-compiler` |
| CN to `unauthd_claims` unless a device blob validated it | `connection_control.rs:424` |
| `zpr.authority` → `user.zpr.authority` / `device.zpr.authority`, per-namespace expiry, per-namespace identity keys | `connection_control.rs:482`, `libeval/src/attribute.rs:31` |
| Teach `get_authentication_expiration` about namespaced authorities | `libeval/src/actor.rs:175` — it reads the single `key::AUTHORITY` today and would find neither new attribute |
| Instantiate a non-`file` trusted-service API | `vs/src/trusted_services/factory.rs` |
| Off-net service descriptor (no ZPR address, no port) | `vs/src/actor_mgr.rs:580` — `uri_for_service` currently errors unless the service has exactly one scope with a port, and hardcodes the `zpr-oauthrsa` scheme at `:588` |
| Identity-keyed attribute lookups | `connection_control.rs:451` — issue #310. Note the lookup runs before identity keys *and* before the ZPR address is allocated, so keys must come from `Policy::identity_attr_keys()` intersected with the authenticated claims |
| Revision cache keyed on ZPR address, purged on disconnect | `trusted_services/manager.rs:19`, `event_mgr.rs:109` (a no-op stub today) — addresses are recycled, so a stale entry would be inherited by a later actor |
| Attribute refresh is a no-op without a CN, so user-only actors never refresh | `actor_attributes.rs:116` (early return) and `:74` (paired `commit_revisions` skip) |
| Reason codes on authentication failure | VSAPI |

### Wire format changes

**Two blobs need no ZDP header change.** `ZdpAcquireZprAddressHeader` carries a
single `blob_len` (`handlers.rs:501`), but the field is already "a base64
encoded json object" (`handlers.rs:487`) and `decode_blob` (`auth.rs:301`)
already parses to `serde_json::Value` before dispatching on `blob_type`. Let it
accept a top-level **array** as well as an object. Legacy single-object blobs
keep parsing; the node→VS leg is already `List(AuthBlob)`.

**New blob type:**

```json
{ "blob_type": "OIDC", "issuer": "https://accounts.google.com", "id_token": "<JWT>" }
```

plus an `oidc @2 :OidcBlob` arm on the `AuthBlob` union in `zpr-vsapi/vs.capnp`.

The `issuer` is a **selector, not a trust input** — it tells the visa service
which declared IdP to check against when several are configured. `client_id`
and `allowed_domains` always come from policy. The visa service must never take
`client_id` from the blob; that is how an attacker substitutes their own Google
application and their own user population.

**Service descriptor and ASA TLV.** `ServiceDescriptor`
(`zpr-common/src/vsapi_types/services.rs:17`) assumes an on-net service with a
ZPR address; Google has an issuer URL instead. And the ZDP `ASA` TLV carries
only a `SocketAddr` (`handlers.rs:285`), so the scheme the node already holds
never reaches the adapter. Both need an off-net IdP shape carrying issuer,
`client_id`, and scopes. The `TODO` at `actor_mgr.rs:571` anticipates exactly
this.

**AAA address becomes conditional.** `link_state.rs:1178` hard-fails
non-bootstrap authentication when no AAA address is present. The AAA mechanism
exists so an adapter can reach an *on-net* auth service before it owns a ZPR
address; with OIDC over normal IP it is unnecessary. That gate must become
mode-aware rather than mandatory.

### Security requirements

Non-negotiable, in addition to the above:

- **Match on `hd`, never on the email domain.** Google sets `hd` only for
  Workspace accounts. A *consumer* Google account can be registered against an
  arbitrary address, so it can present `email: alice@example.com` with
  `email_verified: true` and **no `hd` at all**. Any implementation checking
  `email.endsWith("@example.com")` admits an attacker-created consumer account
  impersonating the corporate domain. **Absent `hd` fails the domain check.**
- Require `email_verified` before mapping `email` to any attribute.
- Validate `iss`, `aud` (against the policy `client_id`), `exp`, `nonce`, and
  `kid`. Reject `alg: none` and any algorithm not in an explicit allowlist.
- Use a vetted JWT library. Do not hand-roll JWS parsing. Algorithm confusion
  and unverified `aud` are the classic failure modes here, and this is the code
  option A adds in exchange for keeping the visa service off the internet.
- The OIDC `nonce` comes from the visa service via
  `ZdpInitAuthenticationPayload`, binding the token to this ZPR authentication
  attempt.
- PKCE S256 mandatory. The verifier never leaves `ph-cli`.
- Loopback redirect only, on `127.0.0.1`, single use, `state` verified.
- **Ordinary TLS verification against system roots for all Google traffic.**
  `auth.rs:482` and `auth.rs:519` currently set
  `danger_accept_invalid_certs(true)` with a `TODO`, which existed because BAS
  used a self-signed certificate. Against Google that flag turns the token
  exchange into an unauthenticated channel. It must not reach the OIDC path,
  and it should be deleted along with the BAS certificate at `auth.rs:44`.
- **The JWKS proxy must be a `CONNECT` forward proxy, never a reverse proxy,
  and the JWKS URL must never be rewritten.** This is the intuitive design and
  it is catastrophic: a reverse proxy — or any scheme that substitutes the
  proxy's ZPR address for the JWKS hostname and lets TLS terminate at the
  proxy — can serve a forged key set. Whoever controls that box can then mint
  signing keys and forge **any** user's identity on the ZPRnet, elevating a
  piece of plumbing to the trust level of Google itself. With `CONNECT`, TLS is
  end-to-end and the proxy sees only ciphertext.

  The workable variant, if a `CONNECT` proxy is unavailable, is a pure TCP
  passthrough where the visa service connects to the proxy's address but still
  sends SNI for the real hostname and validates the certificate for that name
  (as `curl --resolve` does). It keeps TLS end-to-end but requires one proxy per
  upstream host, so `CONNECT` is preferred.
- Never log authorization codes, `id_token`s, refresh tokens, or PKCE
  verifiers.

### Testing

**Compiler fixtures** (`zpr-compiler/test-data/`, following
`m3-ping-and-http.zpl` and its `.zplc`): a valid `api = "oidc"` block compiles
and `zpdump` shows the `OidcConfig`; `service` / `client` / `provider` /
`cert_path` are rejected; missing `allowed_domains` is rejected; a non-`https`
issuer is rejected; **`email` as an identity attribute is rejected.**

**JWT validation — table-driven, offline, fixed vectors.** Needs a fixture
keypair and a small token minter.

| Input | Expected |
|---|---|
| valid token | accept |
| `alg: none` | reject |
| HS256 signed using the RSA public key as the HMAC secret | reject (algorithm confusion) |
| `aud` = attacker's client_id | reject |
| wrong `iss` | reject |
| `exp` in the past | reject |
| missing or mismatched `nonce` | reject |
| **`hd` absent** (consumer account) | reject |
| `hd` present, not in `allowed_domains` | reject |
| `email_verified: false` | `email` not mapped |
| unknown `kid` | reject |
| `auth_time` older than `max_auth_age_seconds` | reject |

**`zpt` evaluation tests**, extending
`zpr-visaservice/integration-test/zpt-test-connect.sh`, which already asserts
`identity_keys == ["device.zpr.adapter.cn", "user.bas_id"]`: connect
device-only, user-only, and both; assert `identity_keys` and which policies
match. Once the class-presence prerequisite lands, assert that a bare
`allow users ...` rule does **not** match a device-only actor.

**Visa service unit tests.** Per-namespace authority stamping and independent
expiry; CN landing in `unauthd_claims` when no device blob was presented;
multiple blobs accepted; a presented-but-invalid blob failing the whole
connection. And, per the `AGENTS.md` rule that a found bug gets a failing test
first: **a user-only actor claiming another endpoint's CN must not inherit that
CN's trusted-service attributes.** That is the regression test for the
escalation at `connection_control.rs:452`, and it should be written before the
#310 restructure.

**Adapter unit tests.** Multi-blob JSON array round-trip *and* legacy
single-object blobs still parsing; PKCE S256 derivation against RFC 7636
vectors; loopback callback rejected on `state` mismatch; listener bound to
`127.0.0.1` only.

**Integration test with a fake IdP** — the only new infrastructure. A small
local OpenID Provider serving a discovery document, an authorization endpoint
that auto-redirects with no UI, a token endpoint, and a JWKS. Following
`zpr-core/integration-test/one-node-test.sh`, the whole flow then runs in CI
with no Google and no browser: `--no-browser` plus an HTTP client that follows
the redirect to the loopback URL. It is also the only practical way to test
JWKS key rotation and the stale-cache path.

**What CI cannot cover.** Real Google. This needs a manual release checklist: a
real Workspace domain for the happy path, a consumer gmail account to verify
the domain rejection actually rejects, and the refresh / `offline_access` path.
The `hd`-absent case in particular is the one a fake IdP is most likely to
model wrongly.

### Deferred

| Item | Why deferred |
|---|---|
| Class specs emit presence conditions | Breaking change across all class specs; tracked as [zpr-compiler#144](https://github.com/org-zpr/zpr-compiler/issues/144), and should land **before** this work |
| Graceful degradation on user-auth expiry | Needs partial revocation in the visa service |
| `[bootstrap]` entries declared as user credentials | Admissible by design; no current need |
| Providers other than Google | The design is provider-generic; only Google is validated |
| A2A confidentiality, anti-replay, k-of-n concurrence | Pre-existing gaps, unrelated |

### Open questions

- Does a `CONNECT`-capable proxy with a ZPR adapter already exist in the target
  environments, or does one need standing up? Providing it is the deployer's
  burden, and `zplc` will warn if the trusted service declares no `service`.
- Which HTTP client does the visa service use for the JWKS fetch, and does it
  support `CONNECT` proxying with end-to-end TLS? (`reqwest` does, and is
  already a dependency on the adapter side.)
- Google's "Desktop app" client type has historically required a
  `client_secret` at the token endpoint even for public clients. Per RFC 8252 it
  is not confidential and can be distributed, but this must be confirmed
  against Google's current requirements, and it changes what the node ships to
  the adapter.
- Should `ph-cli auth-agent` ship a `systemd` user unit, or is registration
  left to the deployment?


## Implementation status

Checked against the code on 2026-09-02 (`zpr-compiler` 0.16.0 at `c58b532`,
`zpr-visaservice` 0.18.0 at `5b73daa`, `zpr-core` at `2e17c92`). **Nothing in
this document's OIDC design is implemented yet.** Work is tracked under
[zpr-visaservice#317](https://github.com/org-zpr/zpr-visaservice/issues/317)
and sequenced by `docs/plans/2026-09-02-oidc-implementation-plan.md`; the
plan's *What changed since the spec* table is authoritative where this document
and the code disagree.

**Implemented (the prerequisites):**

- **Class specs emit a presence condition** — zpr-compiler#144, merged as
  PR #145. `allow users to access services.` compiles to
  `has user.zpr.authority`; the marker keys are `user.zpr.authority` and
  `device.zpr.authority` (`zpr-compiler/src/zpl.rs`). Compiler version 0.16.0.
- **Identity-keyed trusted-service lookups** — zpr-visaservice#310, merged as
  PR #320. Lookups use `Policy::lookup_identity_keys()`; the revision cache is
  keyed by ZPR address and purged on disconnect; attribute refresh works for
  actors with no CN.
- **CN is not promoted to an authenticated claim** by `authorize_connection`;
  only the RSA path does so after verifying the signature.

**Not yet:**

- The visa service still installs the single `zpr.authority` attribute and
  accepts compiler 0.15 policies, so a 0.16 policy's bare `allow users ...`
  rule matches nothing until the visa service adopts the namespaced markers
  (plan issue C0).
- `api = "oidc"`, `OidcConfig`, the `oidc` auth blob, JWT validation, the JWKS
  source, the `AuthAgent` RPC, and the `ph-cli` relying-party flow do not exist.
- The reserved `zpr.` sub-namespace check for trusted-service declarations
  (zpr-compiler#146) is open with PR #147 in review; until it merges the
  authority marker is advisory.
- `zpr-bas` and the adapter's `OAuthRsa` client are deprecated and still
  present; the hardcoded BAS certificate expired on 2026-04-16.

Line numbers cited in the design sections above are from the 2026-09-01
checkouts and several have moved; verify against the source before relying on
one.
