# ZPR System Overview

Zero-trust Packet Routing (ZPR, pronounced "zipper") moves IP packets through a
network that enforces communication policy **itself**, at the IP layer, on
every packet, at every hop — rather than leaving security to endpoints and
firewalls.

This is the orientation document. It explains what the system is, what the
pieces are, and how a packet gets from one application to another. The
companion documents go deeper on each part; the map is at the end.

## Sources

The design comes from the ZPR RFCs, principally internal RFC-1.4
(*Zero-Trust Packet Routing*), published RFC-12 (*Overview*), internal RFC-7
(*Topology, Address Assignment and Forwarding*), internal RFC-9 (*Threat
Resistance*), and internal RFC-10.2 (*ZPR as Software-Defined Networking*).
RFCs 4, 12, 15, 16, and 19 are published in `zpr-rfcs`; the rest are internal
to Applied Invention and cited here by number.

Implementation details are read from the code in the workspace. Where design
and code disagree, the code is what runs — §"Implementation status" collects
the gaps.

---

## The approach

Conventionally a network's real policy is an emergent property of accumulated
firewall rules and whatever the endpoints happen to enforce. ZPR instead states
policy in one auditable place and has the network guarantee it.

Policy is written in [ZPL](ZPL.md) against the *attributes* of the
communicators — **never their addresses, and never their identities**. It is
compiled, installed, and enforced by the network itself, as a layer of security
independent of whatever the endpoints and firewalls already do. Because policy
names attributes rather than addresses, a service can move from one cloud to
another with no policy change.

### Four paranoid design principles

Everything below follows from these (RFC-12 §4.1):

1. **Every packet must have an authenticated sender and receiver.**
2. **The network enforces policy continuously and throughout**, not only at
   entry and exit or at session establishment. Every packet is discarded unless
   policy explicitly allows it.
3. **Nothing is trusted to behave** — not users, not the devices implementing
   the network, not the administrators. Policy must hold even when one of them
   turns out to be unreliable or malicious.
4. **The network is configured and managed through the network itself.**

The fourth is the least obvious and among the most important. Forbidding
out-of-band management, and requiring that management traffic itself be
policy-compliant, is what lets a ZPRnet defend against its own administrators —
for example by requiring three authenticated administrators in three different
facilities to activate a configuration. The only out-of-band step is the
initial startup of a component, when it is preloaded with the cryptographic
material that lets it join.

---

## Core concepts

### ZPRnet

A **ZPRnet** is the network — or group of co-managed interconnected networks —
over which one policy is enforced. It consists of **nodes** connected by
**links**, plus internal services: a **visa service** and an **admin service**.

ZPR runs at the Internet layer, so TCP, UDP, and everything above them work
unmodified. Links may be physical, or virtual over an IP **substrate** — in
which case ZPR is a software-defined overlay network, which is how it is
deployed today.

### Visas

A **visa** is a granted permission for one authenticated endpoint to send to
another under specific conditions. It is the system's central object.

Transit packets between nodes **carry no source or destination address**. They
carry a *visa identifier*, and the visa guides them. That one identifier
simultaneously certifies that both endpoints were authenticated, that policy
permits this communication, and under what conditions — and names the key used
for the packet's integrity check.

Visas are **unidirectional**: the reply needs its own.

Two consequences make the design work:

- **The expensive work happens once.** Translating policy into enforcement
  happens at issuance. Forwarding a packet costs a table lookup on the visa
  identifier plus a check of local conditions — no longest-prefix match, no
  firewall-rule evaluation, no hop-count update.
- **Nodes never receive the policy.** Every node enforces policy without
  holding it. Each is told only what it needs: which link to forward to, which
  links a packet may arrive on, what local conditions to check. Only the
  ingress and egress adapters get the integrity key. **Only the visa service
  holds the whole visa.**

### Compliant flows

A **compliant flow** is the tamper-resistant channel ZPR establishes for any
policy-compliant communication. Every packet on it is covered by a valid visa,
and the visa's procedures are applied *at every forwarding step* — so
enforcement tracks changing conditions and attributes, unlike a conventional
session that is checked once at setup.

Flows are unidirectional and normally used in pairs. A flow may outlive any one
visa, and may hold several over time or at once. Packets are best-effort:
compliant flows defend against tampering but provide no flow control or
retransmission, so TCP is layered over them as usual.

Policy often chains them. Remote users reaching a web service traverse one flow
to the load balancer and a second from the load balancer to a server — which
avoids needing a distinct visa per user/server pair.

Crucially, **the control plane runs over compliant flows too.** Node-to-visa-service
traffic is not out-of-band; it is policy-compliant traffic on the ZPRnet, which
is principle 4 in practice.

### Configurations

Policy is part of a **configuration**, so changing policy is reconfiguration —
and reconfiguration is where conventional networks open holes.

ZPR changes configuration while running. Packets continue under the
configuration they entered on, so **multiple configurations operate
concurrently** during a changeover: `UNDEFINED → DEFINED → [TESTING] → ACTIVE`,
with the outgoing one moving to `DEACTIVATING` until its in-flight packets
drain. Exactly one configuration is active, and new packets enter on it.

This also lets packet field widths be fixed per configuration rather than in
the protocol — the only header field that cannot vary between configurations is
the one naming the configuration.

---

## Components

| Component | What it does |
|---|---|
| **Node** | Forwards packets and enforces policy on every packet passing through. Nodes connect to each other over links. |
| **Dock** | The interface on a node that endpoints connect to, speaking IP. Converts between IP packets and transit packets. |
| **Adapter** | Lets unmodified IP applications reach a ZPRnet. Presents a normal IP interface locally and connects to a dock over a secure **docking session**. |
| **Link** | A connection between two nodes; physical, or virtual over an IP substrate. Carries attributes that ZPL `over` clauses can constrain. |
| **Visa service** | Issues, distributes, and revokes visas; holds policy; admits endpoints. See [VISA_SERVICE.md](VISA_SERVICE.md). |
| **Admin service** | Generates and distributes configurations, including topology and forwarding rules. |
| **Trusted services** | External sources of authentication and attributes — LDAP, Active Directory, cloud services. Named in policy; the only sources ZPR will consult. |

### What an adapter is and is not

An adapter proves the identity of its device — and of any user or service
associated with a flow — to the ZPRnet. That is the whole of its trusted role.

It **does not enforce policy and does not know what the policy is.** It has no
knowledge of network configuration, traffic, connection status, or the policy
governing anyone else. It cannot even see the policy for its own users, beyond
observing which communications succeed. It is trusted only by the specific
users and services that use it.

---

## Life of a flow

Concretely, from writing policy to delivering a packet.

**1. Author and compile.** An operator writes a `.zpl` policy and a `.zplc`
configuration (TOML: nodes, links, protocols, services, trusted services,
bootstrap keys). `zplc` compiles both together into a signed binary policy
(`.bin2`). The signing key must match the one the visa service is configured
with. See [ZPL.md](ZPL.md).

**2. Bootstrap.** A ZPRnet needs a visa service, a docking node, and a trusted
authentication service before ordinary authentication can work. Those come up
on static configuration: the policy's `[bootstrap]` block maps certificate CNs
to RSA public keys, and the holder proves possession by signing a challenge.
The visa service reserves the name `vs.zpr`.

**3. Links come up.** An adapter and a node establish a link with a Noise
handshake, exchanging certificates signed by the deployment's CA. A node
recognizes the visa service's adapter only when it presents a CA-verified
certificate claiming `vs.zpr` — without that, the visa service connects as an
ordinary adapter and nothing routes to it.

**4. An endpoint joins.** The visa service authenticates the endpoint via
trusted services, which return tokens carrying policy-specified attributes.
Some are marked *identity attributes*, and the visa service uses them to query
other trusted services and complete the profile. It then asks policy: may this
endpoint communicate at all, may it host services, does it get a static
address, is it explicitly denied? On success the endpoint receives a ZPR
address and joins.

**5. First packet.** An application sends to the adapter, which finds no entry
for the 5-tuple in its lookup table and issues a **bind request** to its dock,
including the packet.

**6. Visa request.** If the dock has no matching visa it forwards a visa
request to the visa service, packet included. The visa service gathers both
endpoints and their attributes — refreshing expired ones, denying if an
endpoint is unknown or its authentication has expired — and hands the policy,
the endpoints, and a description of the attempt to the evaluator.

**7. Decision.** The evaluator checks *all* deny statements first, then allow
statements, returning every match in policy order. The visa service takes the
first allow match the topology manager can also route. No routable match is a
denial.

**8. Distribution.** The visa goes to the requesting dock and to each node
along the chosen path — each receiving only its own fragment. The dock installs
it, allocates a tether ID, and answers the adapter's bind request with
instructions for compressing packets on this flow.

**9. Steady state.** Subsequent packets hit the adapter's lookup table
directly. The adapter computes the adapter-to-adapter integrity check over the
whole IP packet, compresses the packet — the fields the visa pins, including
addresses and ports, are removed — and sends it with a ZDP transit header. Each
node verifies against the visa and forwards. The egress adapter reconstructs
the original IP packet from the visa's field values, verifies the integrity
check, and delivers it.

Compression removes what is constant over a flow, so a transit packet is often
**smaller than the IP packet it carries** — which leaves room for the ZDP
header without pushing past the substrate MTU.

**10. End.** Visas expire on a timestamp every recipient can see, so no
"expired" message is ever sent; nodes stop forwarding and tear down tethers,
and further traffic looks new. Revocation is separate — an administrative
action or a policy change — and is pushed to the affected nodes, which stop
immediately.

See [ZDP.md](ZDP.md) for the wire protocol and
[VISA_SERVICE.md](VISA_SERVICE.md) for the control plane.

---

## Data plane and control plane

Both run as internal packets between nodes, and both declare their type and
configuration in the header.

- **Transit packets** are the data plane: a visa identifier, payload length, an
  optional data attribute tag, the payload, and an integrity check computed
  over everything except the visa identifier. The visa identifier is
  **link-local** — it changes as the packet moves from link to link, though the
  visa does not.
- **Management packets** are the control plane: assigning link-local visa
  identifiers, distributing and retracting visas.

## Addressing

ZPR addresses are IPv6 from `fd5a:5052::/32`, assigned by the visa service when
an endpoint joins. In the current implementation:

| Range | Use |
|---|---|
| `fd5a:5052::1` | The visa service |
| `fd5a:5052:90de:1::/64` | Nodes |
| `fd5a:5052:adda:1::/64` | Adapters |
| `10.192.0.0/22`, `10.128.0.0/22` | IPv4 equivalents for nodes and adapters |

An endpoint may be granted a static address by policy. The substrate addresses
that carry virtual links are ordinary IP addresses and are unrelated to these.

RFC-7 describes generating topology, node numbering, and forwarding rules
automatically from connection policy and hardware constraints — deriving the
routing architecture from the policy rather than configuring it by hand. The
current implementation is the direct version of this: links are declared in the
`.zplc` configuration, and the visa service's topology manager maintains the
live graph and selects routes.

---

## What runs where

| Concept | Binary | Repository |
|---|---|---|
| Node | `ph node` | `zpr-core/adapter/ph` |
| Adapter | `ph adapter` | `zpr-core/adapter/ph` |
| Visa service | `vs` | `zpr-visaservice/vs` |
| Policy evaluation | `libeval` (library) | `zpr-visaservice/libeval` |
| Policy compiler | `zplc`, `zpdump` | `zpr-compiler` |
| Admin client / dashboard | `vs-admin`, `zpr-dashboard` | `zpr-visaservice` |
| Policy testing without a network | `zpt` | `zpr-visaservice/zpt` |
| Shared types and wire formats | `zpr` crate | `zpr-common`, with `zpr-policy` and `zpr-vsapi` as submodules |

**The node and the adapter are the same binary.** `ph` — the packet handler —
runs as either depending on its subcommand. Both use a TUN interface for the IP
side.

A minimal working ZPRnet is: one node, one visa service (with its own adapter),
a Valkey/Redis for visa service state, a compiled policy, and at least one more
adapter for something to talk to. `zpr-core/README.md` walks through building
one, key by key; `zpr-demo` packages containerized, multi-node, and IoT demos.
See [BUILD.md](BUILD.md) and [REPOSITORIES.md](REPOSITORIES.md).

---

## Implementation status

`zpr-core` is a **pre-release reference implementation**; its README says the
full suite of end-to-end security features is not yet implemented. Read the
RFCs as design intent, not as a description of current behavior.

Working today: nodes and adapters over Noise-secured links, adapter-to-adapter
integrity, bootstrap authentication, visa issuance and distribution with route
selection, revocation and reconciliation on policy change, the ZPL compiler,
and file-backed attribute sources.

Designed but not yet implemented:

- **Conditions, circumstances, and limits in ZPL** — the caps on bandwidth,
  connections, and transferred data that carry much of the denial-of-service
  and exfiltration argument in the threat model. See [ZPL.md](ZPL.md).
- **Assertions.** RFC-12 describes assertions of intent, checked at compile
  time against the combined permissions and rechecked as attribute values
  change. What exists today is the `never` statement.
- **Route-aware evaluation.** ZPL `over` clauses are recorded and routability
  is checked by the topology manager, but stage 2 of the evaluator is a
  scaffold.
- **Adapter-to-adapter confidentiality.** Integrity only, today.
- **Multiple concurrent configurations.** The changeover state machine
  (`TESTING`, `DEACTIVATING`) is design; the visa service loads one policy at a
  time and can *test* a proposed policy against live visas.
- **Byzantine or replicated internal services.** One visa service instance,
  enforced by a database lock. Multicast and combining flows likewise remain
  design.
- **Automatic topology generation** from connection policy, per RFC-7.

[SECURITY_MODEL.md](SECURITY_MODEL.md) has the full status of the security
properties specifically, including anti-replay and k-of-n administration.

---

## Where to go next

| Document | Covers |
|---|---|
| [TERMINOLOGY.md](TERMINOLOGY.md) | The glossary. Start here if a word above was unfamiliar. |
| [ZPL.md](ZPL.md) | The policy language and the compiler toolchain. |
| [VISA_SERVICE.md](VISA_SERVICE.md) | The control plane: issuance, revocation, attributes, admin API. |
| [ZDP.md](ZDP.md) | The data protocol: packet formats, links, docking sessions. |
| [SECURITY_MODEL.md](SECURITY_MODEL.md) | Threat model, trust boundaries, what ZPR does not defend against. |
| [REPOSITORIES.md](REPOSITORIES.md) | Which repository implements what. |
| [BUILD.md](BUILD.md) | Building and testing everything. |

`zpr-core/packet_walk.md` follows a packet through the whole system and is the
best next read for anyone working on the data path.
