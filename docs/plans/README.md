# Master plans

A master plan sequences a multi-issue feature: ordering, cross-repository
interface contracts, per-issue scope and acceptance criteria. It is named
`YYYY-MM-DD-<topic>.md`, opens with `**Status:** IN FLIGHT`, and gets a row in
the `AGENTS.md` required-reading table while it is in flight.

**Only in-flight plans live here.** When the umbrella closes, its final
documentation-closure child retires the plan: the lasting decisions, findings and
still-open deferred items move into the spec's `## Design decisions` section, the
plan file is deleted, and every reference to it (including its `AGENTS.md` row)
is repointed at the spec. Git history keeps the full text. The procedure is
"Plan retirement" in `skills/zpr/SKILL.md`.

Retired plans, and where their rationale now lives:

| Plan (`git show a35b224:docs/plans/<file>`) | Umbrella | Rationale now in |
|---|---|---|
| `2026-09-02-oidc-implementation-plan.md` | [zipline#1](https://github.com/mkolehmainen/zipline/issues/1) | `docs/OIDC.md` |
| `2026-09-14-trusted-service-interplay.md` | [zipline#22](https://github.com/mkolehmainen/zipline/issues/22) | `docs/VISA_SERVICE.md` |
| `2026-09-15-dns-integration.md` | [zipline#34](https://github.com/mkolehmainen/zipline/issues/34) | `docs/DNS.md` |
| `2026-09-16-silent-oidc-reauth.md` | [zipline#40](https://github.com/mkolehmainen/zipline/issues/40) | `docs/OIDC.md` |
| `2026-09-17-build-sets.md` | [zipline#57](https://github.com/mkolehmainen/zipline/issues/57) | `zpr-dev/docs/specs/spec-003-build.md` §10 |
| `2026-09-17-machine-hostname-dns.md` | [zipline#49](https://github.com/mkolehmainen/zipline/issues/49) | `docs/DNS.md` |
| `2026-09-22-attr-query.md` | [zipline#72](https://github.com/mkolehmainen/zipline/issues/72) | `docs/ATTRIBUTE_SERVICE.md` |
| `2026-09-23-netns-docker-fallback.md` | [zipline#90](https://github.com/mkolehmainen/zipline/issues/90) | `zpr-dev/docs/specs/spec-003-build.md` §10 |
| `2026-09-24-static-address-grants.md` | [zipline#95](https://github.com/mkolehmainen/zipline/issues/95) | `docs/VISA_SERVICE.md` |
| `2026-09-25-retire-authored-address-pins.md` (`git show b817c70:docs/plans/<file>`) | [zipline#106](https://github.com/mkolehmainen/zipline/issues/106) | `docs/VISA_SERVICE.md` |
