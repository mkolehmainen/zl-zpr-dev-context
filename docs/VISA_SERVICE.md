# The ZPR Visa Service

The Visa Service examines a description of attempted communication and decides,
from policy and trusted-source attributes, whether to issue a **visa**. Granted
visas are distributed to every node along the communication path.

It is the primary security control of a ZPRnet. Two properties dominate its
design:

- **Correctness** — visa granting *is* the enforcement decision, so the policy
  application logic must be flawless.
- **Performance** — it sits in the path of every new flow and is the obvious
  bottleneck.

Read this before changing issuance, revocation, attribute handling, or
evaluation. Changes to issuance behavior should also be read against
[ZPL.md](ZPL.md) (what policy can express) and ZRFC 12.

## Sources

| Source | Status |
|---|---|
| *ZPR Visa Service Specification / Design Document* (2025-09-03) | The design intent. Explicitly work-in-progress, and parts are marked sketchy or TODO by its authors. |
| `zl-zpr-visaservice/` | What actually runs. Workspace version 0.18.0. |
| `zl-zpr-visaservice/admin-http-api.txt` | The admin API, current as of 2026-08-10 and changing frequently. |
| `zl-zpr-visaservice/libeval/README.md` | The evaluator's API and staging model. |

The design document predates the current implementation and has drifted; §
"Where the implementation diverges from the design document" records the gaps
worth knowing. When the two disagree, **the code is what runs**.

---

## Responsibilities

Beyond issuing visas, in the design document's priority order: an
administrative interface for installing policy and removing visas or
components; routing and addressing for the ZPRnet; tracking active visas and
connected components; removing them when policy or conditions change;
maintaining state across node reboots and disconnections; testing active visas
against a proposed policy; redundancy for failover; a logging interface; and
signaling for policy-triggered events, kept distinct from logging.

Explicitly **excluded** from this implementation, though present in the ZPR
RFCs: Byzantine-style visa-granting agreements, running multiple policies in
parallel, and federated visa services for load balancing.

---

## Repository layout

| Crate | What it is |
|---|---|
| `vs` | The visa service itself, "v2vs" — the v2 distinguishes it from the prototype. Also builds `vsapikey` for generating admin API keys. |
| `libeval` | The evaluator. Compares described traffic to policy and returns the decision. Used by `vs` and `zpt`. |
| `zpt` | ZPR Policy Tester — a REPL and batch runner over `libeval`, for testing how a policy evaluates without standing up a ZPRnet. |
| `vs-admin` | CLI client for the HTTPS admin API. |
| `zpr-dashboard` | Terminal dashboard for monitoring visa service activity. Go, not Rust — a Bubble Tea TUI over the admin API. |
| `admin-api-types` | Request/response types shared by `vs` and `vs-admin`. |
| `integration-test` | Shell-driven integration tests, including `libeval` evaluation tests through `zpt`. |
| `tools` | Helper scripts, including `zpr-pki` for PKI operations. |

Most of it depends on `zl-zpr-common` for the NODE–VS API structures and the
binary policy format, pulled by Git tag in `Cargo.toml` — no manual setup.
Requires Rust edition 2024, `make`, OpenSSL, and a running Redis/Valkey.

### Inside `vs`

Long-lived **managers** own state; **workers** run the per-request and
background work.

| Component | Responsibility |
|---|---|
| `policy_mgr` | The one true source of the current policy. Updates arrive asynchronously from administrators and ripple outward: visas may stop being valid, actors may be forced to disconnect, node connections may change. |
| `actor_mgr` | Actors, and nodes too. |
| `visa_mgr` | Creation, storage, and retrieval of visas. |
| `topology_mgr` | The graph of nodes and links *as it exists* (policy describes how it is *intended* to be), with pathfinding and route selection. Write-through to state. |
| `router` | The in-memory node graph and route cache. Knows nothing of adapters — resolve actors to their docking nodes first. |
| `net_mgr` | Addressing. |
| `connection_control` | New connections to the ZPRnet, node and adapter: authenticate, then authorize against policy. |
| `event_mgr` | High-level events with system-wide consequences, such as actor joins and leaves. Fire-and-forget: it cannot report success back to a caller. |
| `visa_reconciler` | Re-evaluates live visas against a policy snapshot and queues revocations for those that no longer hold. |
| `visareq_worker` | "The beating heart" — one worker per visa request. |
| `vsapi_worker` / `vss_worker` | The node-facing APIs; one VSS worker per node. |
| `db_worker` | Renews the shared database lock that keeps exactly one visa service instance active, and terminates the process if it is lost. |
| `signal_worker` | Emits policy-triggered signals. |
| `deny_log` | A bounded in-memory window of recent denials (500 entries), collapsed on the 5-tuple so a chatty source cannot flush it. Not persisted. |
| `trusted_services/` | Attribute sources and the mapping from their names onto ZPL attribute names. |
| `db/` | State: actors, visas, nodes, links, policy. Valkey/Redis backed, with a fake for tests. |

---

## Lifecycle

### Bootstrap

A ZPRnet needs a visa service, a docking node, and a trusted authentication
service connected before adapter authentication can work. Those come up on
static configuration, called **bootstrap** — which is also enough to run an
entirely statically authenticated ZPRnet.

Bootstrap configuration maps Common Names from ZPR link keys to RSA public key
files; that list is exactly the set of adapters permitted to authenticate
statically, and it must include the visa service adapter. The visa service
reserves the link key name **`vs.zpr`**.

The protocol: the adapter signs challenge data from the node with its RSA
private key, the node forwards the adapter CN and the signature to the visa
service, and the visa service verifies it against the configured public key.

### Docking and trusted services

The visa service activates by docking with a node through its adapter. The
adapter establishes a link with its designated CN, the node identifies the visa
service by that CN, and the node then authenticates to the visa service over
the VS-API using its own private RSA key.

A trusted service can only join through a node once a visa service is
docked — nodes cannot authenticate connections by themselves. Each time a new
trusted service connects, the visa service refreshes attributes for every
connected actor.

### Loading policy

A policy is required at startup; later updates arrive over the admin API. The
policy carries a compiler version and the visa service rejects versions it does
not support — this build requires **compiler 0.15.0 or later**.

A policy can be **tested without loading it**: the service reports which
existing visas the proposed policy would deny. That is only possible because it
retains enough state to re-run the requests behind active visas.

A policy may also change the network topology.

### Authenticating actors

Every authentication-capable trusted service is named in policy and must be
connected for authentication to succeed; if none is reachable and the actor
cannot use bootstrap authentication, authentication fails.

The visa service passes authentication-service addresses to nodes, which pass
them to connecting adapters. A trusted service returns a token carrying the
attributes policy asks for, some of which policy designates as **identity
attributes**. The visa service then queries other trusted services with those
identity attributes to complete the actor's profile.

With attributes in hand it evaluates the actor against policy: may it
communicate on the ZPRnet at all, may it host services, does it get special
privileges such as a static ZPR address, and is it explicitly denied? An actor
that passes receives a ZPR address and joins.

Authentication expires. As expiry approaches the visa service tells the docking
node over the VSS-API so the actor can re-authenticate; the grace period is a
visa service setting. Default authentication lifetime is **4 hours** — except
the visa service's own identity attributes, which are pinned ~100 years out so
it can never expire itself.

### Granting a visa

Nodes request visas. Before evaluating, the visa service gathers both actors
and their attributes; a visa is denied outright if an actor is unknown, and
expired attributes are refreshed from trusted services first. An expired
authentication token is a denial.

The evaluator receives the current policy, both actors with their attributes,
and a description of the attempt: source and destination ZPR addresses, ports
(or ICMP type and code), protocol, and a request flag —
`bidirectional_request`, `unidirectional_request`, or `re_request`.

`visareq_worker` enumerates the outcomes: an actor missing or disconnected; no
route between them; attributes needing refresh; expired authentication; an
existing visa still valid under unchanged policy; denied by policy; allowed but
not over any available route, which is also a denial; or permitted — and even
then, if policy changed underneath the request, the request fails and the
caller is expected to retry.

Once permitted, the path is chosen, the affected nodes are computed, the visa
is queued for installation on each, and it is returned to the caller.

### Distributing visas

The visa returns to the requesting node, which must have a directly connected
ingress or egress adapter, and propagates to the nodes along the path. Nodes
attached to the ingress and egress adapters establish the visa association
(stream ID) with the adapter.

The service records active visas and the requests that created them — the state
that makes policy testing possible.

### Expiration, revocation, renewal

Every visa carries an expiration timestamp, embedded and visible to every
recipient. Bounds are administrative settings: **30 seconds minimum, 24 hours
maximum** in this build. The minimum exists because node installation plus the
round trip would otherwise consume the whole lifetime, and because the visa
store's second-granularity TTL truncates sub-second lifetimes to zero.

Nodes determine expiry locally, so **no "visa expired" message is ever sent**.
On expiry a node stops forwarding and tears down the tethers to its adapters;
further traffic looks new and triggers a fresh request. Nodes can be configured
to re-request before expiry.

Revocation is different — it is an administrative action or a push from a
trusted service. Installing a policy can revoke visas by newly disallowing a
communication, by changing trusted services in a way that invalidates prior
authentications, or by changing topology so an existing path is no longer
valid. The visa service sends revocation messages over the VSS-API listing
revoked visa IDs; nodes stop forwarding immediately and tear down the sessions.

For urgent security events the admin API can revoke visas, actors, or trusted
services without installing a policy. **The effective policy is the installed
policy plus any administrative disconnects**, and those disconnects clear when
the next policy is installed.

Renewal covers visas the service can refresh without a new request — node-to-VS
and VS-to-trusted-service traffic — and visas a node asks to renew with the
re-request flag. A renewal is sent over the VSS-API as a replacement carrying
the new visa and the ID of the one it replaces.

---

## Evaluation (`libeval`)

Given source and destination actors plus packet details, `libeval` compares
them to a compiled policy. A policy holds **communication rules** and **join
policies** — the latter govern whether an actor may connect and what attributes
and services it receives on joining.

### Order of decision

1. **All deny statements first.** Any match returns every matching statement in
   policy order with its signals. The visa service takes the first and denies.
2. **Then allow statements.** Every match is returned in policy order, each
   carrying its path constraints, communication properties (addresses, ports,
   reverse-pinhole flag), signals, and the expiration a resulting visa should
   use.
3. The visa service takes the first allow match that the topology manager also
   permits. If none is routable, the visa is denied.

Matching is required to be **deterministic**: the same policy, actors, and
packet must always select the same policies in the same order.

### Service-centric matching

Actors are clients or servers, and a server must have a service registered
through policy. If neither actor hosts a service there is no match. For TCP and
UDP, a destination port mapping to a service on the destination actor is
**forward** communication, and a source port mapping to a service on the source
actor is **reverse**. For ICMP the TYPE field selects the candidate services.

Supported today: TCP with SYN/ACK flag awareness, UDP, and ICMPv6 (source port
carries the type, destination port the code).

### Two-stage API

Evaluation is split so route-aware decisions are possible:

| Stage 1 — `eval_request` returns | Meaning |
|---|---|
| `Deny(FinalDeny::Deny(hits))` | Denied by a matching deny policy, regardless of route. |
| `Deny(FinalDeny::NoMatch(msg))` | No policy matched at all. |
| `AllowWithoutRoute(hits)` | Allowed regardless of route. |
| `NeedsRoute(RouteResidualEvaluator)` | A route is needed to decide. |

Stage 2, `RouteResidualEvaluator::eval_route`, takes a `Route` and returns the
final result. **It is a scaffold and not yet implemented** — `over` clauses
(ZPL link constraints) therefore do not yet reach a route-aware verdict inside
`libeval`, though the visa service does check routability through the topology
manager.

---

## Running and operating

```bash
vs /path/to/policy.bin2        # vs.toml in the working directory is picked up
vs -c my-config.toml -v
```

Needs a running Valkey/Redis (`redis://127.0.0.1:6379` by default) and a
compiled `.bin2` policy. TLS credentials for the admin API default to
`admin-tls-cert.pem` / `admin-tls-key.pem`.

### Configuration (`vs.toml`, `[core]`)

`vs_addr`, `vsapi_port`, `admin_port`, `admin_cert`, `admin_key`, `vk_uri`,
`identity` (ties this instance to its state in the database), `api_keys`, and
`file_ts_dir` (where `<service-id>.json` files for `api = "file"` trusted
services live). Unknown keys are rejected.

### Constants that must stay in sync with the compiler

Changing either side alone will break a ZPRnet, because the compiler writes
policy that assumes these values:

| Value | Default |
|---|---|
| Visa service ZPR address | `fd5a:5052::1` |
| Visa service link-key CN | `vs.zpr` |
| VS-API port (nodes → VS) | 5002 |
| Admin HTTPS port | 8182 |
| Minimum policy compiler version | 0.15.0 |

Other operational bounds: 4-hour default authentication lifetime, 180-second
maximum clock skew during node authentication, 20 visas per request, 1024
request workers and queue depth, a 7-second VSS ping with 3 failures before the
node is dropped, and a 30-second database lock refresh against a 90-second
timeout.

### Admin API

HTTPS on port 8182, JSON in and out, TLS required. Every request carries an
`X-API-Key` header; a missing or unknown key is 401. Keys carry either read or
read/write permission, and write endpoints reject a read-only key with 403.
Generate keys with `vsapikey`.

Endpoints cover actors (`/admin/actors`, plus their visas), services and their
caches, visas (including `/admin/visas/denies`), the network view, statistics,
policies (`GET`/`POST /admin/policies`, `/admin/policies/curr`), and
authentication revocation. Some — the authrevoke endpoints, `DELETE` visas, and
`DELETE` actors — still return placeholder data; `admin-http-api.txt` marks
each one and is the endpoint reference.

Note that unless you are on the same host, reaching the admin API requires
**policy permission** like any other service on the ZPRnet.

### Building and testing

```bash
make            # build the Rust workspace and the Go dashboard
make test       # unit tests
make check      # fmt --check plus warnings as errors
make release    # tarball of vs, vs-admin, zpt into build-release/
```

The root `Makefile` drives both toolchains (`build-rs`, `build-go`), so a
Rust-only change is faster from inside the crate.

Use `zpt` to test policy evaluation without a ZPRnet: load a compiled policy,
set attributes on named actors, and evaluate. It runs as a REPL or over an
instruction file, with `-j` for JSONL output.

---

## Where the implementation diverges from the design document

- **Cap'n Proto, not Thrift.** The design document specifies Thrift for the
  binary policy and the node APIs. The implementation uses Cap'n Proto
  throughout (see `zl-zpr-policy/policy.capnp`, `zl-zpr-vsapi/vs.capnp`); there is no
  Thrift anywhere in the tree.
- **Trusted-service attribute stores are file-backed only.** The factory
  accepts `api = "file"` — attributes loaded from a local `<service-id>.json`
  — and rejects every other API with an error, so the networked `validation/2`
  attribute source described in the compiler configuration is not yet
  instantiated here. Authentication services are a separate service type.
- **Route-aware evaluation is a scaffold.** Stage 2 of `libeval` is defined but
  not implemented.
- **Configuration moved to TOML.** `config-example.yaml` at the repository root
  is prototype-era — YAML, gRPC admin connection, RSA certs "left over from
  prototype". The current service reads `vs.toml`; treat the YAML sample as
  historical.
- The design document leaves several things open, and they remain open: how the
  end-to-end HMAC secret reaches ingress and egress adapters; how path
  constraints interact with deny statements; how administrative disconnects are
  logged when admins act concurrently; when renewal is automatic rather than
  requiring a fresh request; and how nodes are told which other nodes to link
  to. Its sections on state and resiliency, logging, visualization, and
  replication are headings only.

---

## Where the code lives

| Concern | Location |
|---|---|
| Decision logic | `libeval/src/eval.rs`, `eval_result.rs`, `eval_route.rs` |
| Join policy | `libeval/src/joinpolicy.rs` |
| Policy loading | `libeval/src/pio.rs`, `policy.rs` |
| Per-request path | `vs/src/visareq_worker.rs` |
| Policy lifecycle | `vs/src/policy_mgr.rs`, `visa_reconciler.rs` |
| Connection admission | `vs/src/connection_control.rs`, `auth.rs` |
| Routing | `vs/src/topology_mgr.rs`, `router.rs` |
| Attribute sources | `vs/src/trusted_services/` |
| State | `vs/src/db/` |
| Admin API | `vs/src/admin_service.rs`, `admin_apikeys.rs`, `admin-http-api.txt` |
| Configuration and constants | `vs/src/config.rs` |
| Wire formats | `zl-zpr-vsapi/vs.capnp`, `zl-zpr-policy/policy.capnp` |

A change to what policy can express usually spans three repositories — see
[ZPL.md](ZPL.md) and [REPOSITORIES.md](REPOSITORIES.md).
