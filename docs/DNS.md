# DNS in ZPR

How a name inside a ZPRnet becomes a ZPR address. This is the orientation
document: read it before touching the resolver, the hosts index, or a demo's
`Corefile`, then go to the plan and the code it points at.

**Authoritative sources.** The code wins. Then, in order:
`zl-zpr-coredns/README.md` (the plugin's own reference — Corefile syntax,
name semantics, resolution order),
`zl-zpr-visaservice/admin-http-api.txt` (the three endpoints),
`docs/plans/2026-09-15-dns-integration.md` (service names — the original
design), `docs/plans/2026-09-17-machine-hostname-dns.md` (machine names).
This document summarizes all four and does not restate their detail.

---

## The model

DNS in ZPR is a **read-only view of the visa service's live state**, served
over the overlay by an ordinary DNS server. There is no zone file, no
secondary, and no push: every query becomes an HTTPS call to the visa
service's admin API, and short TTLs are what make the answer track reality.

```
 ┌──────────┐  DNS UDP/53         ┌────────────────────────┐  HTTPS TCP/8182   ┌──────────────┐
 │ client   │  visa: client →     │  CoreDNS + zpr plugin  │  visa: zpr-dns →  │  admin API   │
 │ (adapter)│  zpr-dns service    │  pinned ZPR address    │  vs-admin service │ fd5a:5052::1 │
 └──────────┘ ───────────────────▶└────────────────────────┘ ─────────────────▶└──────────────┘
   dig AAAA web.demo                GET /admin/services/web        200 { zpr_addr }
                                    then /admin/hosts/web on 404
```

Both hops are ZPR flows governed by policy and carried by visas. The resolver
is not privileged infrastructure: it is an actor on the ZPRnet that holds a
visa to the `vs-admin` service, and it is reachable only by clients holding a
visa to the `zpr-dns` service.

**A name never authorizes.** Resolution tells you *where* something is;
whether you may reach it is still decided by policy, per protocol and port,
at visa issuance and at enforcement. The hostname index is never consulted by
the enforcement path. Resolving `webhost.demo` and then being denied ICMP6 is
the system working correctly.

## Two namespaces, one flat name space

| Kind | Source of truth | Claimed how |
|---|---|---|
| **Service name** (`web`) | the compiled policy's `[services.*]` plus a live provider | declared by the policy author |
| **Machine name** (`webhost`) | the `device.hostname` attribute on an actor record | vended by a trusted service ("naming authority") at authentication |

Both are a **single label under the server's zone**, in one flat namespace.
The service lookup runs first and always wins; the host lookup is attempted
only on a clean 404. Collisions are prevented at claim time, not at
resolution time: a hostname claim equal to a policy service name is rejected,
and installing a policy that introduces such a service releases the held
name. `a.b.<zone>` is NXDOMAIN with no HTTP call.

Names are matched **exactly as stored** — a single lowercase DNS label,
`[a-z0-9]([a-z0-9-]*[a-z0-9])?`, 1–63 bytes. The plugin lowercases the query
label; nothing else is transformed on either side. An invalid hostname claim
is rejected and logged, never mangled into a valid one, so it is simply never
in the index.

## The four moving parts

| Part | Repository | Role |
|---|---|---|
| The `zpr` CoreDNS plugin | `zl-zpr-coredns` | Answers `AAAA <name>.<zone>` by calling the admin API. Stateless per query; the stock `cache` plugin sits in front of it. The only Go repository, and not a fork — its branch is `main`. |
| The resolve surface of the admin API | `zl-zpr-visaservice` | `GET /admin/services/{name}`, `GET /admin/hosts/{name}`, `GET /admin/services`. Nothing else. |
| The hostname index | `zl-zpr-visaservice` | `host:<NAME>` → actor ZPR address, filled from authenticated `device.hostname` claims, first-claim-wins per value. |
| The naming authority | operator-side | A trusted service that returns `hostnames -> device.hostname{}`. In `dns-demo` this is the `file` api over `machines.json`; in production it is the control plane. |

## Configuring it

Four things must line up. Each is small; the failure modes come from one of
them being missing.

**1. Policy** must declare the resolver as a service, declare the admin API as
a service, and allow both hops. The resolver appears as the *subject* of an
`Allow` — this is a supported ZPL shape:

```
Define zpr-dns as a service.        # UDP/53, pinned provider address
Define vs-admin as a service.       # TCP/8182, provided by the VS adapter itself
Allow access:all users to access zpr-dns.
Allow zpr-dns to access vs-admin.
```

**2. The naming authority**, if machine names are wanted, is a
`[trusted_services.*]` block in the `.zplc`:

```toml
[trusted_services.machines]
api = "file"
returns_attributes = ["hostnames -> device.hostname{}"]
expiration_seconds = 3600
```

Note the pruning trap: the compiler drops a trusted service that no ZPL rule
references, and the index only fills from a woven store. Something in the ZPL
must mention the attribute — `dns-demo` does it with
`Allow access:all users to access ping on hostname: devices.`

**3. An API key** with the least-privilege `resolve` permission, minted on the
visa service host and readable only by the resolver:

```sh
vsapikey create resolve dns /path/to/vs_keys.toml > /run/secrets/vs-resolve-key
```

**4. A Corefile.** The zone comes from the server block, not from the plugin;
`api_key_file` and `tls_ca` are required and there is deliberately no
insecure-skip-verify option:

```
demo.:53 {
    zpr {
        endpoint       https://[fd5a:5052::1]:8182
        api_key_file   /conf/vs-resolve.key
        tls_ca         /conf/include/admin-tls-cert.pem
        tls_servername vs.zpr      # the admin cert is issued for a name, not the IP
        ttl            30
        negative_ttl   10
    }
    cache 30
    errors
    log
}
```

Full option list, defaults and the CoreDNS version pin: `zl-zpr-coredns/README.md`.

**And then the client.** Nothing in ZPR points an endpoint's stub resolver at
the resolver's overlay address — that is endpoint work, still unbuilt. Until
it exists, "seamless" means "seamless once the OS is told where the resolver
is"; `dns-demo` writes `/etc/resolv.conf` in the client's entrypoint.

## Query semantics

| Query | Result |
|---|---|
| `<name>.<zone>` AAAA, service found | AAAA `zpr_addr`, TTL = `ttl` |
| `<name>.<zone>` AAAA, service 404, host found | AAAA `zpr_addr` |
| service 404 and host 404 | NXDOMAIN + SOA, MINIMUM = `negative_ttl` |
| name exists, other qtype | NODATA (NOERROR, SOA in authority) |
| two or more labels under the zone | NXDOMAIN, no HTTP call |
| admin API 5xx / 401 / 403 / unreachable / bad JSON | **SERVFAIL**, never NXDOMAIN |
| a *failed* (non-404) service lookup | SERVFAIL — never falls through to hosts |

That last row is the security-relevant one: a hostname can never shadow or
substitute for a service, not even while the service lookup is erroring.

Liveness is TTL-bounded, not event-driven. A provider that goes away becomes
NXDOMAIN within roughly `ttl + negative_ttl`; a service that moves resolves to
its new address on the same scale. Keep TTLs short — ZPR addresses are
recycled across reconnects.

## Security properties worth preserving

- **The `resolve` key asks about one name and gets one address.** It reaches
  exactly three endpoints and cannot read actors, visas or policy. Do not
  widen it; add a `read`-gated endpoint instead.
- **The admin API endpoint must be `https`** — rejected at plugin startup
  otherwise, because plain HTTP would send the `X-API-Key` in the clear.
- **The admin certificate is always verified** against `tls_ca`. Certificates
  need a `subjectAltName`: Go's verifier ignores CN, so a CN-only cert (as in
  `multinode-demo`) cannot be pinned.
- **A hostname is never self-asserted.** It must arrive as an authenticated
  trusted-service claim. Unauthenticated adapter claims are committed to the
  actor whenever a join policy matches, so accepting `device.hostname` from
  that set would let any machine claim any name.
- **Never silently rename.** A rejected claim is surfaced (counted, logged,
  and visible on the loser's `ActorDescriptor.hostname_conflicts`), never
  repaired by the visa service.
- **An index entry's lifetime is the actor record's**, not the attribute's, so
  a name does not vanish mid-session when an attribute cache lapses.

---

## Implementation status

Both plans are fully implemented and merged. The end-to-end proof is
`zl-zpr-demo/dns-demo`: `make && local-compute/deploy-docker.sh &&
local-compute/test-dns.sh` stands up node + VS + web + client + resolver and
exercises resolution, liveness, both policy hops, machine names and aliases,
and the collision / precedence / invalid-name controls.

Not built, in rough order of likely demand:

- **Client stub-resolver configuration** by the endpoint daemon (above).
- **PTR / reverse lookups** for `fd5a:5052::/32` — would widen `resolve` to an
  address-keyed lookup.
- **`GET /admin/hosts` (list)**, `read`-gated, for operators.
- **SRV records** from `service_endpoints` — cheap, no consumer yet.
- **Push invalidation** on actor departure, and any bulk/zone-transfer mode.
  Short TTLs are the deliberate ceiling; revisit if 30 s staleness bites.
- **Multi-node placement** of the resolver — blocked on the data plane.

Deliberately rejected, not deferred: naming actors by X.509 CN or node id
(CNs are not unique and not indexed), multi-answer RRsets for a shared name
(`ping6 somename` must not be a coin flip), and scoped or tenant-qualified
names while one naming authority owns the namespace.

## Where the code lives

| Concern | Location |
|---|---|
| Corefile parsing, TLS/key setup, `https`-only check | `zl-zpr-coredns/plugin/zpr/setup.go` |
| Query handling, zone/label rules, SOA synthesis | `zl-zpr-coredns/plugin/zpr/zpr.go` |
| Admin API client, service-then-host lookup, status mapping | `zl-zpr-coredns/plugin/zpr/client.go` |
| CoreDNS version pin and plugin ordering | `zl-zpr-coredns/Makefile`, `plugin.cfg` |
| Resolve endpoints and their gate | `zl-zpr-visaservice/vs/src/admin_service.rs` (`can_resolve`) |
| `resolve` permission level | `vs/src/admin_apikeys.rs`, `vs/src/bin/vsapikey.rs` |
| Hostname claim, validation, first-claim-wins, reconciliation | `vs/src/actor_mgr.rs`, `vs/src/event_mgr.rs`, `vs/src/db/actor.rs` |
| Endpoint reference and types | `zl-zpr-visaservice/admin-http-api.txt`, `admin-api-types/src/admin_api_types.rs` |
| Trusted-service `returns_attributes` parsing | `zl-zpr-compiler/src/config/trusted_service.rs` |
| Worked example: policy, keys, container, tests | `zl-zpr-demo/dns-demo/` |

**Unrelated use of the word.** `PolicyResolver` in
`zl-zpr-visaservice/vs/src/policy_mgr.rs` does ordinary *substrate* DNS —
resolving node hostnames in a topology to underlay IPs at policy load, on the
public internet's DNS, before any ZPRnet exists. It has nothing to do with
this document. See [ROUTING.md](ROUTING.md).
