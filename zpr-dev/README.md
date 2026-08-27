# `zpr-dev`

A small command-line tool that creates and maintains the standard ZPR
development workspace: it clones the repositories listed in
`zpr-dev-context/workspace.yaml` side by side, and renders the shared
`AGENTS.md` — plus each repository's optional `AGENTS.repo.md` — into a
generated `<repo>/AGENTS.md` and `<repo>/CLAUDE.md` that any coding agent
picks up automatically.

It is not a build system and does not replace Git. It shells out to the
installed `git` and never touches uncommitted work.

Specification: [`docs/specs/spec-001-zpr-dev.md`](docs/specs/spec-001-zpr-dev.md).
Install and bootstrap ordering: [`../README.md`](../README.md).

---

## Install

```bash
cargo install --path ~/src/zpr/zpr-dev-context/zpr-dev
```

Clone `zpr-dev-context` *into* the workspace first — the tool ships inside the
repository it clones. See the root README for the full sequence.

---

## Workspace layout

```text
~/src/zpr/                    workspace root — a plain directory, not a repository
├── zpr-dev-context/          the context checkout
│   ├── AGENTS.md             shared agent instructions
│   ├── docs/                 shared technical documentation
│   ├── skills/               shared agent skills
│   └── workspace.yaml        the repository manifest
├── zpr-core/
│   ├── AGENTS.repo.md        optional, hand-maintained, committed
│   ├── AGENTS.md             generated, untracked
│   └── CLAUDE.md             generated, untracked
└── ...
```

Paths resolve in this order, first match wins:

| Setting | Order |
|---|---|
| workspace | `--workspace <path>` → `$ZPR_WORKSPACE` → `~/src/zpr` |
| context | `--context <path>` → `<workspace>/zpr-dev-context` |

---

## Global options

```text
--workspace <path>    Override the workspace directory
--context <path>      Override the zpr-dev-context checkout
-v, --verbose         Show additional detail
-q, --quiet           Suppress progress output (results are still printed)
--dry-run             Show intended changes without modifying anything
-h, --help
-V, --version
```

All of them are global, so they may appear before or after the subcommand.

`--dry-run` suppresses every mutation: no clone, no fetch, no merge, no file
write. A `git fetch` counts as a mutation because it rewrites
`.git/refs/remotes`, so dry-run stops before it and does not guess at the
outcome.

### Exit codes

```text
0   success (warnings permitted)
1   validation errors
2   command or configuration error
```

`setup` ends in validation and passes that exit code through, so
`setup --no-clone` on an empty workspace exits 1.

---

## Commands

### `zpr-dev setup`

Create or repair the workspace. Safe to re-run.

```bash
zpr-dev setup
zpr-dev setup --workspace ~/work/zpr --context-url git@github.com:org-zpr/zpr-dev-context.git
```

```text
--context-url <git-url>   Default git@github.com:org-zpr/zpr-dev-context.git
--branch <branch>         Branch to clone for the context repository
--no-clone                Do not clone missing source repositories
```

In order: create the workspace directory, clone the context checkout if absent
(if present, leave it *entirely* alone — no fetch, no checkout, no branch
change), read the manifest, clone each missing repository at its
`default_branch`, generate the context files, then validate.

`setup` never discards local modifications and never changes a branch.

### `zpr-dev update`

Fetch and fast-forward, conservatively.

```bash
zpr-dev update              # context repository only
zpr-dev update --all        # every repository in the manifest
zpr-dev update --repo zpr-core
```

```text
--all                Also update source repositories
--repo <name>        Update only the named repository (may be the context checkout)
--no-generate        Skip regeneration afterward
```

Per repository: `git fetch`, then `git merge --ff-only @{u}`. The *current*
branch is fast-forwarded whatever it is — a feature branch with an upstream is
never switched to `default_branch`, which is used only when cloning.

A repository is skipped, with its reason reported, when it is:

| Condition | Reported as |
|---|---|
| not a Git repository | `not a git repository` |
| dirty working tree (tracked files) | `local modifications` |
| detached `HEAD` | `detached HEAD` |
| no upstream for the current branch | `no upstream` |
| fast-forward not possible | `cannot fast-forward` |

```text
$ zpr-dev update --all
zpr-dev-context: 4ba137c -> 650ad18
zpr-core: current
zpr-visaservice: skipped, local modifications
wrote generated context: 0 created, 2 updated, 18 unchanged
```

Regeneration runs afterward unless `--no-generate`.

### `zpr-dev status`

Report the workspace. **No network access** — it does not fetch, so `behind`
reflects the last fetch.

```bash
zpr-dev status
zpr-dev status --repo zpr-core
zpr-dev status --porcelain
```

```text
WORKSPACE /home/alice/src/zpr

REPOSITORY        BRANCH       STATUS       UPSTREAM
zpr-dev-context   main         clean        current
zpr-core          feature-x    modified     ahead 2
zpr-visaservice   main         clean        behind 3
zpr-utils         -            missing      no upstream

AGENT CONTEXT
zpr-core          current
zpr-visaservice   stale
zpr-utils         missing repository
```

`STATUS` is `clean`, `modified`, `missing`, or `not a git repository`.
`--porcelain` emits tab-separated records whose field order is a contract for
all of v0.x:

```text
repo   <name>  <branch>  <status>  <ahead>  <behind>
agent  <name>  <current|stale|missing repository>
```

`<branch>` is `-` with no repository and `detached` when `HEAD` is detached;
`<ahead>`/`<behind>` are `-` with no upstream.

### `zpr-dev sync`

Regenerate the context files from the local context checkout. No fetch, no
pull, no network access. Run it after editing `AGENTS.md`, a shared document,
or a repository's `AGENTS.repo.md`.

```bash
cd ~/src/zpr/zpr-dev-context
vim AGENTS.md
zpr-dev sync
```

```text
wrote generated context: 0 created, 2 updated, 18 unchanged
```

Counts are *files*, two per repository. Only files whose rendered content
differs are written, so a second run is a true no-op and mtimes do not churn.

### `zpr-dev validate`

Check workspace health. Findings accumulate rather than stopping at the first.

```text
$ zpr-dev validate

[OK]   context repository
[OK]   workspace manifest
[OK]   10 source repositories
[WARN] generated context stale in 2 repositories (run: zpr-dev sync)
[OK]   documentation references
[INFO] repository-specific context in 3 of 10 repositories

Validation completed with 1 warning.
```

Errors (exit 1): a missing or non-Git context checkout, an unparseable or
invalid manifest, a missing `AGENTS.md`, a documentation reference that does
not resolve, a missing or non-Git repository directory. Warnings (exit 0):
generated files that differ from their rendered content, and a declared
`agent.hermes.shared_skills` directory that does not exist.

Generated-file drift is a warning by design — a hand-edited generated file and
a merely stale one are indistinguishable on disk.

---

## Generated files

```markdown
<!-- Generated by zpr-dev. Do not edit manually. -->
<!-- Source: zpr-dev-context @ 4ba137c -->
<!-- Shared docs: /home/alice/src/zpr/zpr-dev-context/docs -->

# Shared ZPR Development Context

...body of the shared AGENTS.md, with docs/ references absolutized...

# Repository-Specific Context

...body of AGENTS.repo.md, when it exists...
```

`CLAUDE.md` is a two-line pointer at `./AGENTS.md`, so both files stay in sync
from one source.

Documentation references are rewritten by enumerating the real files under
`<context>/docs/` and replacing each manifest-relative path with its absolute
path — so `docs/VISA_SERVICE.md` inside `zpr-core/AGENTS.md` points at the
context checkout, not at a `zpr-core/docs/` that does not exist. A reference to
a document that does not exist is left untouched, and `validate` reports it.

Both generated files are deliberately **untracked and not committed**;
`zpr-dev` does not manage `.gitignore` or `.git/info/exclude`, so they show up
in `git status`. That is expected: they contain workspace-absolute paths.

---

## Safety invariants

No command, ever, will:

- reset a repository, or delete a branch;
- discard, stash, or overwrite uncommitted changes;
- push, or force-push;
- rebase;
- switch branches;
- modify agent configuration not written by `zpr-dev`;
- write any file in a source repository other than the generated `AGENTS.md`
  and `CLAUDE.md`.

---

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                     # unit + integration; no network required
```

Integration tests build a throwaway workspace from local `git init --bare`
origins addressed by `file://` URL, so they need neither network access nor
credentials.

```text
src/
├── main.rs        clap types, global options, dispatch, exit codes
├── config.rs      manifest types and loading; workspace/context resolution
├── git.rs         thin wrappers over the installed git binary
├── generate.rs    render, plan, apply
└── commands.rs    setup, update, status, sync, validate
```

Dependencies are limited to `clap`, `serde`, `serde_yaml_ng`, and `anyhow`,
plus `tempfile` for tests. No async runtime, HTTP client, `regex`, or
Git library.

---

## TODO — deferred from v0.1

Each of these is deliberately out of scope for v0.1 (spec §1.2). None is
blocked by a design decision; each can be added without restructuring.

| Deferred | Why, and what it would take |
|---|---|
| `agent configure hermes` | The one with real value left on the table. Configuration 7 of the parent spec — pointing Hermes at `zpr-dev-context/skills` via `skills.external_dirs` — is entirely unbuilt; `agent.hermes.shared_skills` is parsed and checked for existence, nothing more. Deferred because Hermes is not installed on the development machine and its configuration path and schema are unknown. Needs a Hermes install to test a non-destructive YAML merge against. |
| `agent configure codex` / `claude` | Genuinely a no-op today: the generated `AGENTS.md` and `CLAUDE.md` already give both agents their context with no global configuration. Add only if an agent gains a discovery mechanism these files do not satisfy. |
| `doctor` | Diagnostic sugar — git version, SSH reachability, agent installations. `validate` covers the failures that matter today. |
| `regenerate` | Would be an alias for `sync`, which already writes only on difference and is idempotent. |
| `repo list` / `repo path <name>` / `docs [topic]` | Convenience lookups so scripts can avoid hard-coded paths. `status` already prints the repository set. |
| `update --rebase` | The parent spec requires rebase to be explicit; nothing needs it yet. |
| `status --short` | `--porcelain` covers the scripting case. |
| Workspace discovery by walking up from the current directory | `--workspace`, `$ZPR_WORKSPACE`, and the `~/src/zpr` default cover the real cases. |
| Repository groups (`setup --group core`) | Deferred by the parent spec (§14.4) until the repository set is large enough to hurt. |
| Launching agents (`zpr-dev hermes zpr-core`) | Deferred by the parent spec (§14.5). |
| `.gitignore` / `.git/info/exclude` management | **Decided against**, not merely deferred. Generated files stay visible as untracked entries. |

Separately, before tagging v0.1, three checks need a real workspace of the ten
`org-zpr` repositories over SSH — their fixture equivalents pass, but the real
thing has not been run:

- `git status` in each source repository shows only the two untracked
  generated files;
- a second `zpr-dev sync` writes nothing (`ls --full-time` mtimes unchanged);
- `zpr-dev update --all` with one dirty repository skips it and leaves that
  repository's `HEAD` and working tree untouched.
