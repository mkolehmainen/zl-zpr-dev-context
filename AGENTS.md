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
| Changing authentication, identity, attributes, or trusted services | `docs/SECURITY_MODEL.md`, `docs/VISA_SERVICE.md`, `docs/OIDC.md` |
| Implementing or reviewing OpenID Connect (OIDC) work | `docs/OIDC.md`, `docs/plans/2026-09-02-oidc-implementation-plan.md` |
| Changing packet formats, links, docking sessions, forwarding, or compression | `docs/ZDP.md` |
| Changing routing, topology, or address assignment | `docs/SYSTEM_OVERVIEW.md`, `docs/ZDP.md`, `docs/VISA_SERVICE.md` |
| Changing anything cryptographic, or touching the enforcement path | `docs/SECURITY_MODEL.md` |
| Writing or reviewing a policy file | `docs/ZPL.md` |

Two rules that apply to every task above:

- **These documents record design intent, not what runs.** The RFCs describe the
  system as designed; each document in `docs/` has an `## Implementation status`
  section recording where the code diverges, and flags divergence inline where
  it matters. **The code wins.** Check the status section before assuming a
  feature exists, and verify against the source before relying on a detail.
- **A change to what policy can express usually spans three repositories** --
  the grammar and compiler in `zpr-compiler`, the schema in `zpr-policy`, and
  the evaluator in `zpr-visaservice`. See `docs/REPOSITORIES.md`.
