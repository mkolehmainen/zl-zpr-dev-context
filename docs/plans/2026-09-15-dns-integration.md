# DNS Integration Plan — CoreDNS over the visa service admin API

**Status:** COMPLETE (2026-09-15) — umbrella [zipline#34](https://github.com/mkolehmainen/zipline/issues/34); tasks P1/V1/D1/I1 all merged, see *Issue map*. The current DNS reference is `docs/DNS.md`. Historical record: read it for *why*, not for what the code does now.
**Date:** 2026-09-15
**Repo state this plan was written against:** `zl-zpr-visaservice` @ `9c9dfa0` (admin API "current as of 2026-08-10"), `zl-zpr-dev-context` @ `0d7aed0`, `zl-zpr-common` tag `v0.26.0`.

> Process note: per `skills/zpr/SKILL.md`, each task below becomes one GitHub issue and is worked on a feature branch off `zipline`, PR targeting `zipline`. Read that skill before filing.

**Goal.** A ZPR client inside a ZPRnet resolves `<service>.<zone>` with an ordinary DNS query and gets the AAAA record of the actor currently providing that ZPL service. Resolution is live (a service that moves hosts resolves to the new host within one TTL) and policy-governed end to end (client→resolver and resolver→visa service are both visas).

**Architecture.**

```
                 ZPRnet (overlay)
 ┌──────────┐  DNS UDP/53          ┌────────────────────────┐  HTTPS TCP/8182         ┌──────────────────┐
 │ ZPR      │ ───────────────────▶ │ CoreDNS + `zpr` plugin │ ──────────────────────▶ │ Visa Service     │
 │ client   │  visa: client →      │ adapter CN dns.demo    │  visa: zpr-dns →        │ admin API        │
 │ (adapter)│  zpr-dns service     │ static ZPR address     │  vs-admin service       │ fd5a:5052::1     │
 └──────────┘                      │ X-API-Key (resolve)    │  GET /admin/services/N  │ GET /admin/      │
                                   └────────────────────────┘                         │   services/{N}   │
                                                                                      └──────────────────┘
   `web.demo. AAAA?`  ──▶  plugin strips zone, GET /admin/services/web  ──▶  { zpr_addr: "fd5a:5052:8888::80" }
   ◀── AAAA fd5a:5052:8888::80 TTL 30
```

Three moving parts, three repositories, plus one fixture:

| Part | Where | What changes |
|---|---|---|
| Visa service | `zl-zpr-visaservice` | New `resolve` API-key permission; `vsapikey` accepts it; admin API doc updated. **No new endpoints.** |
| CoreDNS plugin | new repo `zl-zpr-coredns` (Go) | Plugin `zpr`, Corefile syntax, a `make build` that produces a CoreDNS binary with the plugin compiled in. |
| Policy fixture | new `zl-zpr-demo/dns-demo` | `.zplc` declaring `zpr-dns` (UDP/53, static address) and `vs-admin` (TCP/8182, provided by the VS adapter); ZPL allow rules; `[bootstrap]` entry for the resolver's CN. |
| End-to-end demo | new `zl-zpr-demo/dns-demo` | Containerized node + visa service + CoreDNS + one web service + one client, built with the docker techniques of `multinode-demo` and none of its OCI/OpenTofu parts. `dig` from the client proves the flow. `containerized-demo` is **not** the model: it has been unused since 2026-02. |

**Name semantics (normative for the plugin).**

- Zone is Corefile-configured (default `zpr.`). Exactly one label under the zone is a service name: `web.zpr.` → service `web`. Two or more labels → NXDOMAIN. The apex answers SOA and NS only.
- Query label is lowercased before lookup; the VS lookup is exact-match. Policy authors keep DNS-resolvable service names to lowercase DNS labels (`[a-z0-9-]`). Names that are not valid labels simply cannot be queried.
- `AAAA` → the service's `zpr_addr` from `ServiceDescriptor` (the providing actor's address, **not** `dock_zpr_addr`, which is the node it docks to).
- `A` for an existing name → NODATA (NOERROR, empty answer, SOA in authority). ZPR addresses are IPv6 only.
- Unknown service, or known service with no current provider (both 404 from the VS) → NXDOMAIN with SOA in authority. The VS does not distinguish the two and DNS need not either.
- VS unreachable, TLS failure, 401/403, 5xx, malformed body → SERVFAIL. Never NXDOMAIN on infrastructure failure: a client must not cache "does not exist" because the resolver lost its visa.
- TTL is Corefile-configured, default 30s; negative TTL (SOA MINIMUM) default 10s. Addresses are recycled across reconnects (`docs/OIDC.md:649`), so long TTLs risk pointing at a different actor. Do not default higher.
- Positive cache lives in CoreDNS's stock `cache` plugin, not in ours. The plugin is stateless per query.

---

## Global constraints

- **No policy schema change.** `zl-zpr-policy`'s `policy.capnp` and the `zpr` crate are untouched; no `zpr-common` bump, no compiler version floor change. If a task appears to need one, stop and revisit.
- **No new admin endpoints.** `GET /admin/services` and `GET /admin/services/{name}` already carry everything the resolver needs. Adding a bulk or reverse endpoint is deferred (see Out of scope). The one admin API change is authorization, not surface.
- **Least privilege is not optional.** The resolver's key is `resolve`, never `read` or `readwrite`. The Corefile references a key *file*; the key string never appears in the Corefile or in an image layer.
- **TLS is verified, not accepted.** The plugin pins the admin CA/cert (`tls_ca`) and sets `tls_servername`. It does not replicate `vs-admin`'s `danger_accept_invalid_certs(true)` (`vs-admin/src/vsclient.rs:47`).
- **Single-node topology for the fixture.** `setTopology` is a stub and multi-hop forwarding panics on install (`docs/ROUTING.md:295-316`). Resolver, VS adapter and client all dock to the one demo node. Do not write the demo to depend on `over` clauses; they compile but are not enforced.
- **Build gates.** Rust repos: `cargo build`, `cargo fmt -- --check`, `cargo test`, warnings are errors; every non-trivial function has a doc comment; a found bug gets a failing test first. Go repo: `go vet`, `go test ./...`, `gofmt -l` empty.
- **Branches and tags.** Feature branches from `zipline`, PRs target `zipline`; tags carry a `zl-` prefix; never edit a repo's generated `AGENTS.md`/`CLAUDE.md`; never let a `Cargo.lock` with a local path reach a PR (`docs/REPOSITORIES.md`).

---

## Cross-repository interface contracts

### 1. Admin API as consumed by the plugin (existing; `zl-zpr-visaservice/admin-http-api.txt`)

```
GET https://[fd5a:5052::1]:8182/admin/services/{name}     X-API-Key: <resolve key>
  200  ServiceDescriptor { service_name, actor_cn, zpr_addr, dock_zpr_addr, service_kind, service_endpoints }
  401  key missing/malformed/unknown
  403  key lacks resolve permission
  404  no such service, or no current provider
  500  server error
GET https://[fd5a:5052::1]:8182/admin/services             (readiness probe only)
  200  NamedListEntry[]  { "id": string }
```

Types are defined in `zl-zpr-visaservice/admin-api-types/src/admin_api_types.rs`. `{name}` is URL-path-encoded. The plugin reads only `zpr_addr`; other fields are ignored so future additions do not break it.

### 2. API key permission (new; `zl-zpr-visaservice/vs/src/admin_apikeys.rs`)

```rust
#[serde(rename_all = "lowercase")]
pub enum Permission { Resolve, Read, #[serde(rename = "readwrite")] ReadWrite }

impl Permission {
    /// GET /admin/services and GET /admin/services/{name}: any active key.
    pub fn can_resolve(&self) -> bool { true }
    /// Every other GET. Resolve keys are excluded on purpose.
    pub fn can_read(&self) -> bool { matches!(self, Permission::Read | Permission::ReadWrite) }
    pub fn can_write(&self) -> bool { matches!(self, Permission::ReadWrite) }
}
```

Keys file (TOML, `KeysFile`) gains the literal `permission = "resolve"`. Existing files parse unchanged. `vsapikey` accepts `resolve`.

### 3. Corefile syntax (new; `zl-zpr-coredns/plugin/zpr/setup.go`)

```
zpr.:53 {
    zpr {
        endpoint       https://[fd5a:5052::1]:8182   # admin API base URL; default shown
        api_key_file   /run/secrets/vs-resolve-key   # required; file contents = key string, trailing newline stripped
        tls_ca         /etc/zpr/admin-tls-cert.pem   # required; PEM bundle used to verify the admin cert
        tls_servername vs.zpr                        # optional; SNI/verify name when the cert is not for the IP
        ttl            30                            # optional; positive TTL seconds
        negative_ttl   10                            # optional; SOA MINIMUM
        timeout        2s                            # optional; per-request HTTP timeout
    }
    cache 30
    errors
    log
}
```

`zpr` may appear once per server block. The zone comes from the server block, not the plugin.

### 4. Policy configuration (new fixture; `.zplc` TOML, shape copied from `zl-zpr-demo/multinode-demo/zpr-conf/admin/multinode-demo.zplc.template`)

```toml
[protocols.dns]
l4protocol = "UDP"
port = 53

[protocols.vs-admin]
l4protocol = "TCP"
port = 8182

[services.web]                      # lowercase: DNS lookups are case-sensitive on the VS side
protocol = "http"
port = 80
provider = [["device.zpr.adapter.cn", "web.demo"], ["zpr.addr", "fd5a:5052:8888::80"]]

[services.zpr-dns]
protocol = "dns"
port = 53
provider = [["device.zpr.adapter.cn", "dns.demo"], ["zpr.addr", "fd5a:5052:8888::53"]]

[services.vs-admin]
protocol = "vs-admin"
port = 8182
provider = [["device.zpr.adapter.cn", "vs.zpr"]]

[bootstrap]
"dns.demo" = "../include/dns-public-key.pem"     # plus node, vs, web, alice as in multinode-demo
```

Static addresses follow `multinode-demo`'s convention of `fd5a:5052:8888::/64` for pinned service addresses (`multinode-demo.zplc.template:49,54`); the resolver's adapter config sets the matching `zpr_addr = ["fd5a:5052:8888::53"]` and `tun_if`, exactly like `adapter-web1-conf.toml.template`. `vs.zpr` is the VS adapter's CN (`docs/VISA_SERVICE.md:291-302`; already in the demo's `[bootstrap]`).

ZPL (`multinode-demo.zpl` style):

```
Define web as a service.
Define zpr-dns as a service.
Define vs-admin as a service.

Allow users to access web.
Allow users to access zpr-dns.
Allow zpr-dns to access vs-admin.
```

Whether a service may appear as the *subject* of an `Allow` (`Allow zpr-dns to access vs-admin`) is exactly what P1 tests; ZRFC 15 permits service subjects (`docs/ZPL.md:182`, `:253` "allow Service2 access to Service1"). Zone in the Corefile is `demo.`, so the resolvable name is `web.demo.`.

---

## Dependency graph and order

```
P1 (dns-demo skeleton: node + vs + web + client, policy with
    zpr-dns / vs-admin; proves the VS can provide a service) ──┐
V1 (visa service: resolve permission) ─────────────────────────┼──▶ I1 (add the dns container; end-to-end)
D1 (coredns plugin + build) ───────────────────────────────────┘
```

P1, V1 and D1 are independent and start in parallel. **P1 first if only one person is working**: it retires the plan's one real unknown (can policy express "the VS itself provides a service"? The OIDC plan's `<TSNAME>-vs` service assumes yes; nothing has compiled and loaded it). D1 can be developed against a `read` key and a locally-run `vs` until V1 lands.

---

## Issue map

| ID | Repo | Title | Blocked by |
|---|---|---|---|
| P1 · [#35](https://github.com/mkolehmainen/zipline/issues/35) | zl-zpr-demo | New `dns-demo`: containerized node + vs + web + client (no OCI), policy declaring `zpr-dns` and `vs-admin`; VS shows up as a service provider | — |
| V1 · [#36](https://github.com/mkolehmainen/zipline/issues/36) | zl-zpr-visaservice | `resolve` API-key permission scoped to `GET /admin/services*`; `vsapikey resolve`; doc | — |
| D1 · [#37](https://github.com/mkolehmainen/zipline/issues/37) | zl-zpr-coredns (new) | CoreDNS `zpr` plugin: AAAA from `GET /admin/services/{name}`, Corefile syntax, `make build` producing `bin/coredns` | — |
| I1 · [#38](https://github.com/mkolehmainen/zipline/issues/38) | zl-zpr-demo | `dns-demo`: add the `dns` container; `dig web.demo` from the client resolves; negative controls scripted | P1, V1, D1 |

---

## Phase P — New demo skeleton (`zl-zpr-demo/dns-demo`)

### Task P1: `dns-demo` without the resolver, proving the policy shape ([zipline#35](https://github.com/mkolehmainen/zipline/issues/35), merged)

**Model to copy:** `zl-zpr-demo/multinode-demo/` — `Dockerfile` (ubuntu:24.04 + valkey + `COPY bin/ /app/bin/`), `Makefile` (builds `ph`, `vs`, `vs-admin`, `vsapikey`, `zplc`, `zpdump` from `ZPR_ROOT` into `bin/`), `docker-compose.yml` (one image, static IPv4 on a private network, `NET_ADMIN` + `/dev/net/tun`, per-container `/conf` and `/logs` mounts), `local-compute/deploy-docker.sh` (render `@@TOKEN@@` templates → mint API key with `vsapikey create --init` → `zplc` → `compose up` → `launch()` each process under tmux), `local-compute/entrypoint-*.sh` (tun device + static ZPR address + the app), `zpr-conf/{admin,confs,include}` layout, `commands/` wrappers.

**Drop:** everything under `oci-compute/`, every `tofu` call and `@@NODE0_PUBLIC_ADDR@@` token in the deploy script, `node0`/`ociweb`/`bob`, the `links.*` topology (single node), `zpr-dashboard`.

**Files (new):**
- `dns-demo/README.md` — contents table, how to build (`make ZPR_ROOT=...`), how to deploy (`local-compute/deploy-docker.sh`), the `dig` walk-through (filled in by I1).
- `dns-demo/Makefile`, `dns-demo/Dockerfile` — copied; Makefile gains a `coredns` target in I1.
- `dns-demo/docker-compose.yml` — services `node` (172.30.1.10, ports 5000 tcp+udp), `vs` (.11), `web` (.12), `client` (.13); `dns` (.14) is added by I1. Network `zpr-dns-demo`, subnet `172.30.1.0/24` so it can coexist with `multinode-demo`'s `172.30.0.0/24`.
- `dns-demo/local-compute/deploy-docker.sh`, `entrypoint-node.sh`, `entrypoint-vs.sh`, `entrypoint-web.sh` (nginx + static banner, from `entrypoint-web1.sh`), `entrypoint-client.sh` (installs nothing at runtime; image gets `dnsutils` + `curl` in the Dockerfile), `vs.toml`.
- `dns-demo/zpr-conf/admin/dns-demo.zpl`, `dns-demo.zplc.template`, `attrfile.json` — contract 4, with `alice` as the one user.
- `dns-demo/zpr-conf/confs/node-conf.toml`, `adapter-vs-conf.toml.template`, `adapter-web-conf.toml.template`, `adapter-client-conf.toml.template`, `adapter-dns-conf.toml.template` (the last used by I1).
- `dns-demo/zpr-conf/include/` — regenerate every key and cert with `zl-zpr-visaservice/tools/zpr-pki` rather than copying `multinode-demo`'s; the admin TLS cert **must** carry `SAN: DNS:vs.zpr, IP:fd5a:5052::1` (see Resolved while planning).
- `dns-demo/commands/demo-vs-admin`, `demo-status`, `demo-shell` — copied with the OCI `NAME` rows removed.

**Produces:** `make && local-compute/deploy-docker.sh` brings up node, vs, web, client; `client` can `curl http://[fd5a:5052:8888::80]`; the policy already contains `zpr-dns` and `vs-admin`.

- Step 1: Copy and prune per the lists above. Every `@@TOKEN@@` in templates resolves from compose's static IPs only; `render()` keeps its unresolved-token check.
- Step 2: Write `dns-demo.zpl` and the `.zplc` template per contract 4. `zplc` compiles; `zpdump` shows `web`, `zpr-dns`, `vs-admin` with the expected endpoints and provider attributes, and shows `zpr-dns` as an allowed *subject* against `vs-admin`. If the compiler rejects a service as a subject, record it under **Findings** and stop: the design needs a device-class subject instead (e.g. `Define resolver as a device with ...` keyed on the `dns.demo` CN).
- Step 3: Deploy. `commands/demo-vs-admin services` lists `vs-admin` and `web`; `demo-vs-admin services get vs-admin` shows `zpr_addr == "fd5a:5052::1"`; `services get zpr-dns` → 404 (no provider yet), not 500. If `vs-admin` is absent, the VS adapter does not present a `services` attribute for itself — record under **Findings**, escalate; V1 and D1 are unaffected, I1 is blocked.
- Step 4: `client` → `curl http://[fd5a:5052:8888::80]` succeeds (proves the skeleton independent of DNS).

**Acceptance:** Steps 2–4 pass from a clean `make` on a machine with only docker and the Rust toolchain; no `tofu` anywhere in `dns-demo/`.

---

## Phase V — Visa service (`zl-zpr-visaservice`)

### Task V1: `resolve` permission ([zipline#36](https://github.com/mkolehmainen/zipline/issues/36), merged)

**Files:**
- `vs/src/admin_apikeys.rs:15-20` (`Permission` enum), `:50-58` (`can_read`/`can_write`)
- `vs/src/admin_service.rs:830-833` (`get_services`), `:858-865` (`get_service`) — the only two handlers that switch to `can_resolve()`
- `vs/src/admin_service.rs:135-165` (`validate_api_key`, `require_api_key`) — unchanged, verify only
- `vs/src/bin/vsapikey.rs:58`, `:96-100` (permission parsing and help text)
- `admin-http-api.txt` "API KEY" section
- `vs-admin/src/vsclient.rs` — unchanged; confirm it still round-trips against a `read` key

**Interfaces — Produces (exact):** contract 2 above.

- Step 1 (test first): in `admin_service.rs` tests, insert a `resolve` key via `ReloadableApiKeys::insert_for_test` (`admin_apikeys.rs:150`), following the shape of `test_flush_service_cache_read_key_forbidden` (`admin_service.rs:2733`). Assert: `GET /admin/services` → 200, `GET /admin/services/{name}` → 200/404, and each of `GET /admin/visas`, `GET /admin/actors`, `GET /admin/policies/curr`, `GET /admin/network`, `GET /admin/stats` → 403, `DELETE /admin/services/{id}/cache` → 403. Tests fail to compile until Step 2.
- Step 2: Add `Permission::Resolve` and `can_resolve()`; switch the two handlers. `can_read()` semantics unchanged so every other handler is untouched.
- Step 3: `vsapikey` accepts `resolve`; error text lists all three.
- Step 4: Add a keys-file parse test: a TOML record with `permission = "resolve"` deserializes; an existing `read` fixture still parses.
- Step 5: Update `admin-http-api.txt` "API KEY": three permission levels; note which two endpoints accept `resolve`. Bump the "Current as of" date.
- Step 6: `cargo build && cargo fmt -- --check && cargo test`.

**Acceptance:** all Step 1 assertions pass; `vsapikey ... resolve` writes a record that `vs` loads; `vs-admin` against a `read` key behaves exactly as before.

---

## Phase D — CoreDNS plugin (`zl-zpr-coredns`, new Go repository)

### Task D1: Plugin, Corefile syntax, build ([zipline#37](https://github.com/mkolehmainen/zipline/issues/37), merged)

**Files (new):**
- `plugin/zpr/setup.go` — Corefile parsing per contract 3; reads the key file once at setup; builds one `http.Client` with the pinned CA, `MinVersion: tls.VersionTLS12`, `ServerName` from `tls_servername`, `Timeout` from `timeout`.
- `plugin/zpr/zpr.go` — `ServeDNS` implementing the **Name semantics** section; `Name()` returns `"zpr"`; `Ready()` performs one `GET /admin/services`.
- `plugin/zpr/client.go` — `lookupService(ctx, name) (netip.Addr, status, error)`; URL-path-encodes the name; maps HTTP status to one of found / notfound / failure.
- `plugin/zpr/zpr_test.go` — table tests against `net/http/httptest` faking the admin API.
- `plugin.cfg` — upstream CoreDNS `plugin.cfg` with `zpr:github.com/<org>/zl-zpr-coredns/plugin/zpr` inserted after `cache` (order in this file is execution order; `zpr` must run after `cache` and before `forward`).
- `Makefile` — `build` (clone CoreDNS at a pinned tag into `work/`, copy `plugin.cfg`, `go generate && go build -o bin/coredns`), `test`, `clean`. Same shape as `zl-zpr-visaservice/zpr-dashboard/Makefile`, which `multinode-demo` already consumes by copying `bin/`. No container image here: the demo bakes `bin/coredns` into its own image.
- `README.md` — contract 3 verbatim plus the Name semantics table.

**Produces:** `bin/coredns` whose `coredns -plugins` lists `dns.zpr`.

- Step 1 (test first): `zpr_test.go` cases, each asserting rcode, answer RRs and authority SOA presence:
  - `web.zpr. AAAA`, fake returns 200 `{zpr_addr:"fd5a:5052:adda:1::7"}` → NOERROR, one AAAA, TTL == configured.
  - `web.zpr. A`, 200 → NOERROR, zero answers, SOA in authority.
  - `nope.zpr. AAAA`, 404 → NXDOMAIN, SOA in authority with MINIMUM == negative_ttl.
  - `a.b.zpr. AAAA` → NXDOMAIN without an HTTP call (assert fake saw zero requests).
  - `WEB.zpr. AAAA` → HTTP path is `/admin/services/web`.
  - `web.zpr. AAAA`, 500 / 401 / connection refused / non-JSON body → SERVFAIL.
  - `zpr. SOA` and `zpr. NS` → synthesized records.
  - `example.com. AAAA` → passed to next plugin (`plugin.NextOrFailure`).
- Step 2: Implement `client.go` and `zpr.go` until Step 1 passes. Keep `ServeDNS` under ~80 lines; response construction goes in one helper per rcode.
- Step 3: `setup.go` with parse tests: missing `api_key_file` or `tls_ca` is a setup error; defaults match contract 3; `zpr` twice in one block is an error.
- Step 4: `Makefile`; `make build && bin/coredns -plugins | grep dns.zpr`.
- Step 5: Manual smoke against a locally running `vs` (from `zl-zpr-visaservice`, `cargo run --bin vs`) with a `read` key if V1 has not landed: `dig @127.0.0.1 -p 1053 AAAA <some-service>.zpr`.

**Acceptance:** Step 1 and Step 3 tests green; Step 4 builds reproducibly from the pinned CoreDNS tag; a service present in the VS resolves, an absent one is NXDOMAIN, a stopped VS is SERVFAIL.

---

## Phase I — Integration (`zl-zpr-demo/dns-demo`)

### Task I1: Add the `dns` container and prove the flow ([zipline#38](https://github.com/mkolehmainen/zipline/issues/38), merged)

**Files:** `dns-demo/Makefile` (new `coredns` target: `cd $(ZPR_ROOT)/zpr-coredns && make build && cp bin/coredns $(BIN)/`, mirroring how `multinode-demo` builds the Go `zpr-dashboard`), `dns-demo/docker-compose.yml` (service `dns`, 172.30.1.14), `local-compute/entrypoint-dns.sh` (tun + `fd5a:5052:8888::53` + `exec /app/bin/coredns -conf /conf/Corefile`), `local-compute/deploy-docker.sh` (render `adapter-dns-conf.toml`, mint a **second** key `vsapikey create resolve dns` into `conf/dns/vs-resolve.key`, copy the admin cert to `conf/dns/`, `launch dns dns-adapter "/app/bin/ph adapter -c adapter-dns-conf.toml"`), `zpr-conf/confs/Corefile` (contract 3 with `endpoint https://[fd5a:5052::1]:8182`, `tls_servername vs.zpr`, zone `demo.`), `local-compute/test-dns.sh`, `README.md`.

**Produces:** the deploy script brings up node, vs, web, client, dns; `test-dns.sh` proves resolution and the negative controls unattended.

- Step 1: Wire the container, entrypoint, adapter config, key and Corefile. `commands/demo-vs-admin services get zpr-dns` → `zpr_addr == "fd5a:5052:8888::53"`.
- Step 2: From `client`: `dig @fd5a:5052:8888::53 AAAA web.demo` returns `fd5a:5052:8888::80`; `curl http://web.demo` works once the client's `/etc/resolv.conf` points at the resolver (entrypoint-client.sh writes `nameserver fd5a:5052:8888::53`).
- Step 3: `commands/demo-stop-ph web`; within `ttl + negative_ttl` seconds the same query is NXDOMAIN. `demo-restart-ph web`; it resolves again.
- Step 4: Negative controls: (a) reinstall a policy without `Allow zpr-dns to access vs-admin.` → `dig` is SERVFAIL and `demo-vs-admin visas denies` shows the resolver's deny to `fd5a:5052::1` port 8182; (b) `docker exec dns curl -H "X-API-Key: $(cat /conf/vs-resolve.key)" https://[fd5a:5052::1]:8182/admin/visas --cacert /conf/include/admin-tls-cert.pem` → 403; (c) same URL with `/admin/services/web` → 200.
- Step 5: `local-compute/test-dns.sh` runs Steps 2–4 and exits non-zero on any failure, following the SUCCESS/FAILED banner style of `zl-zpr-visaservice/integration-test/zpt-test.sh`. README documents the walk-through.

**Acceptance:** from a clean checkout, `make ZPR_ROOT=... && local-compute/deploy-docker.sh && local-compute/test-dns.sh` exits 0.

---

## Findings

Recorded as tasks discover them. Format: `### Finding N — <claim> (confirmed | withdrawn | to be confirmed by test)`.

### Finding 1 — the VS can be the provider of a policy service (to be confirmed by P1)

`docs/VISA_SERVICE.md:310-325` says reaching the admin API from another host "requires policy permission like any other service on the ZPRnet", and the OIDC plan declares a `<TSNAME>-vs` service provided by the VS, but no compiled policy in any repo has yet declared a service with `provider = [["device.zpr.adapter.cn", "vs.zpr"]]` and been loaded. P1 Step 4 settles it.

---

## Out of scope (tracked, not scheduled)

- **Client resolver configuration.** Pointing a ZPR endpoint's stub resolver at the resolver's static address for the ZPRnet's zone (systemd-resolved per-link DNS, or equivalent) is endpoint work that belongs to the ZPR endpoint daemon, not to the VS or CoreDNS. Without it "seamless" means "seamless once the OS is told where the resolver is". The demo configures the client container explicitly.
- **Reverse lookups (PTR)** for `fd5a:5052::/32`. Needs `ActorDescriptor.services` (the DB already has `list_services_for_actor`, `vs/src/db/actor.rs:331`) and would widen the `resolve` scope to `/admin/actors/{addr}`. Add when something needs it.
- **SRV records** from `service_endpoints`. Cheap, but no consumer yet.
- **Bulk zone endpoint / AXFR / zone-sync plugin mode.** Only if per-query load on the VS shows up in `GET /admin/stats` counters.
- **Push invalidation** from the VS on `ActorLeaves` (`vs/src/event_mgr.rs:33`). Short TTLs are the deliberate ceiling; revisit if 30s staleness bites.
- **Actor CN and node ID names.** Rejected in design (2026-09-15).
- **Multi-node placement** of the resolver. Blocked on the data plane, not on this design.

---

## Resolved while planning

- **Service lookup is case-sensitive.** `service_key_for` (`vs/src/db/actor.rs:548`) only percent-encodes via `KeyString` (`vs/src/db/mod.rs:163-167`); no case folding. The plugin lowercases the query label and policy authors use lowercase service names, as stated in Name semantics.
- **The existing admin TLS certs cannot be verified by Go.** Both `zl-zpr-visaservice/vs/admin-tls-cert.pem` and `zl-zpr-demo/multinode-demo/zpr-conf/include/admin-tls-cert.pem` are `CN=vs.zpr` with **no subjectAltName**. Go's verifier ignores CN, so D1's `tls_ca` pinning fails against either. P1 generates `dns-demo`'s admin cert with `SAN: DNS:vs.zpr, IP:fd5a:5052::1` using `zl-zpr-visaservice/tools/zpr-pki`, and the Corefile sets `tls_servername vs.zpr`. Do not work around this with an insecure-skip-verify option; the plugin does not get one.
- **Static address range.** `multinode-demo` pins service addresses in `fd5a:5052:8888::/64`; `dns-demo` uses `::80` for web and `::53` for the resolver.
- **Which demo to model on.** `containerized-demo` has been unused since 2026-02; `multinode-demo` is current. `dns-demo` copies the latter's docker technique and none of its OCI/OpenTofu parts.

## Open questions

None outstanding at plan time. The one design risk (a service as the subject of an `Allow`, and the VS presenting itself as a service provider) is scheduled as P1 Steps 2–3 and tracked under **Findings**.
