# ZPR

The ZPR system is a REFERENCE IMPLEMENTATION.  That means:
- Code should favor readability over clever succinctness.
- Code must to be appropriately commented.


The ZPR system is a secure networking system.  That means:
- Code must be auditable.
- Follow established best practices for building secure software.


Additional coding guidelines:
- Use the DRY principle, favor code reuse and refactor aggressively to achieve this.
- Unit test everything. When a bug is found, before fixing it write a test that fails.
- Unless a function is exceedingly trivial, every function should have a comment explaining what it does.


## INDEX

- `docs/` -> technical knowledge loaded when relevant.
- `docs/plans/` -> master plans for multi-issue features: ordering, cross-repository interface contracts, per-issue scope and acceptance criteria.
- `skills/` -> specialized, repeatable agent workflows.
- `zpr-dev/` -> binary for configuring the ZPR development environment.


## Required reading by task

Read these before making the change, not after. Paths are relative to the
shared context checkout; a generated `AGENTS.md` rewrites them to absolute
paths, so they can be opened directly.

| When you are | Read |
|---|---|
| Taking a task through GitHub: issue, plan, branch, PR, review | `skills/zpr/SKILL.md` |
| New to ZPR, or unsure how the pieces fit | `docs/SYSTEM_OVERVIEW.md`, `docs/TERMINOLOGY.md` |
| Unsure which repository owns something | `docs/REPOSITORIES.md` |
| Building, testing, or changing a cross-repository dependency | `docs/BUILD.md` |
| Changing ZPL syntax or semantics, or the compiler | `docs/ZPL.md` |
| Changing visa issuance, revocation, or the evaluator | `docs/VISA_SERVICE.md`, `docs/SECURITY_MODEL.md` |
| Changing authentication, identity, attributes, or trusted services | `docs/SECURITY_MODEL.md`, `docs/VISA_SERVICE.md` |
| Changing packet formats, links, docking sessions, forwarding, or compression | `docs/ZDP.md` |
| Changing routing, topology, or address assignment | `docs/ROUTING.md`, `docs/SYSTEM_OVERVIEW.md`, `docs/ZDP.md` |
| Changing anything cryptographic, or touching the enforcement path | `docs/SECURITY_MODEL.md` |
| Writing or reviewing a policy file | `docs/ZPL.md` |
| Changing the topology schema, `Router`/`TopologyMgr`, or how a visa's next hop is chosen | `docs/ROUTING.md` |
| Working any OIDC issue (`mkolehmainen/zipline#1` and its sub-issues) | `docs/OIDC.md`, then that issue's section of `docs/plans/2026-09-02-oidc-implementation-plan.md` |
| Changing OIDC token validation, JWKS handling, or the `api = "oidc"` trusted service | `docs/OIDC.md`, `docs/SECURITY_MODEL.md` |

Two rules that apply to every task above:

- **Where a `docs/plans/` document covers the work, it wins over the spec it
  implements.** `docs/OIDC.md` is the design; the implementation plan fixes ordering,
  interface contracts and acceptance criteria against the code as it actually is, and
  calls out each point where it supersedes the spec. Read the spec for intent and the
  plan for what to do.
- **These documents record design intent, not what runs.** The RFCs describe the
  system as designed; each document in `docs/` has an `## Implementation status`
  section recording where the code diverges, and flags divergence inline where
  it matters. **The code wins.** Check the status section before assuming a
  feature exists, and verify against the source before relying on a detail.
- **A change to what policy can express usually spans three repositories** --
  the grammar and compiler in `zl-zpr-compiler`, the schema in `zl-zpr-policy`, and
  the evaluator in `zl-zpr-visaservice`. See `docs/REPOSITORIES.md`.
