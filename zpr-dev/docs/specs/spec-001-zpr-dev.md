# SPEC-001: `zpr-dev` v0.1 Design

Status: draft for review
Date: 2026-08-27
Parent spec: `../../../docs/ZPR_DEV_CONTEXT_SPEC.md`

This document specifies the first implementation of the `zpr-dev` tool. It
narrows the parent spec to a buildable v0.1 and records the decisions made
during design. Where this document and the parent spec disagree, this
document governs v0.1.

---

## 1. Scope

### 1.1 In scope

Five commands:

```text
zpr-dev setup
zpr-dev update
zpr-dev status
zpr-dev sync
zpr-dev validate
```

### 1.2 Out of scope for v0.1

Each of the following is deliberately deferred. None of them is blocked by a
design decision made here; each can be added without restructuring.

| Deferred | Reason |
|---|---|
| `doctor` | Diagnostic sugar. `validate` covers the failures that matter today. |
| `agent configure <agent>` | Hermes is not installed on the development machine and its configuration path and schema are not known. Codex and Claude need no global configuration once repository-local `AGENTS.md` exists, so the command would only wrap a Hermes YAML merge we cannot test. |
| `regenerate` | `sync` writes only on difference and is already idempotent, so `regenerate` would be an alias. |
| `repo list` / `repo path` / `docs` | Convenience lookups. `status` already prints the repository set. |
| `update --rebase` | The parent spec requires rebase to be explicit; nothing needs it yet. |
| `status --short` | `--porcelain` covers the scripting case. |
| Workspace discovery by walking up from the current directory | `--workspace`, `$ZPR_WORKSPACE`, and the default cover the real cases. |
| `.gitignore` / `.git/info/exclude` management | Decided against. Generated files appear as untracked entries in `git status`; that is accepted. |

### 1.3 Decisions carried in from the parent spec

- Policy B: generated `AGENTS.md` files are **not** committed (§7, §14.1).
- The context repository is checked out under its own name,
  `zpr-dev-context` (§14.2).
- `update` is conservative: context only by default, source repositories
  only with `--all` (§14.3).
- No repository groups (§14.4), no agent launching (§14.5).
- Shell out to the installed `git`; never reimplement Git operations (§12).

### 1.4 Decisions made during this design

1. **`agent configure` is deferred entirely** rather than implemented against
   a guessed Hermes configuration format. See §1.2.
2. **A generated `CLAUDE.md` pointer is emitted alongside `AGENTS.md`.**
   Claude Code's documented discovery file is `CLAUDE.md`. Rather than
   duplicate the body, `zpr-dev` writes a two-line generated `CLAUDE.md` that
   directs the reader to `./AGENTS.md`. One source of truth, both agents find
   it.
3. **No ignore-file management.** Generated files are left untracked and
   visible.
4. **Checkout directory name equals the repository name** for source
   repositories, matching the treatment of `zpr-dev-context`.
5. **Generated-file drift is a warning, not an error, in `validate`.** A
   hand-edited generated file and a merely stale one are indistinguishable on
   disk; neither should fail validation outright.

---

## 2. Workspace and Path Resolution

### 2.1 Workspace root

Resolved in order, first match wins:

1. `--workspace <path>`
2. `$ZPR_WORKSPACE`
3. `~/src/zpr`

### 2.2 Context checkout

Resolved in order:

1. `--context <path>`
2. `<workspace>/zpr-dev-context`

### 2.3 Source repository checkout

```text
<workspace>/<repository name>
```

### 2.4 Resulting layout

```text
~/src/zpr/
├── zpr-dev-context/
│   ├── AGENTS.md
│   ├── docs/
│   ├── skills/
│   └── workspace.yaml
├── zpr-core/
├── zpr-common/
├── zpr-visaservice/
└── ...
```

The workspace root is a plain directory and is not a Git repository.

---

## 3. Workspace Manifest

`workspace.yaml` lives at the root of `zpr-dev-context`.

### 3.1 Schema

```yaml
version: 1                      # required, must equal 1

workspace:
  name: zpr                     # optional, informational

repositories:                   # required, non-empty
  - name: zpr-core              # required, unique, non-empty
    url: git@github.com:org-zpr/zpr-core.git   # required
    default_branch: main        # optional, default "main"
    context:                    # optional block
      local: AGENTS.repo.md     # optional, default "AGENTS.repo.md"
      generated: AGENTS.md      # optional, default "AGENTS.md"

documentation:
  root: docs                    # optional, default "docs"

agent:
  hermes:
    shared_skills: skills       # optional; validated for existence only
```

Every optional field is supplied by a serde default, so the common repository
entry is three lines. The `agent.hermes.shared_skills` key is parsed and
validated in v0.1 but not otherwise acted upon, since `agent configure` is
deferred.

Unknown top-level keys are ignored rather than rejected, so the manifest can
grow ahead of the tool.

### 3.2 Initial repository set

Public `org-zpr` repositories with the `zpr-` prefix, excluding `zpr-bas`
(excluded by request) and `zpr-dev-context` (it is the context repository and
is cloned separately):

```text
zpr-core          Core ZPR components
zpr-common        Shared zpr crate
zpr-visaservice   ZPR Visa Service component
zpr-vsapi         Visa Service API
zpr-compiler      The ZPL Compiler
zpr-policy        ZPR Policy descriptor source
zpr-rfcs          Zero-Trust Packet Routing RFCs
zpr-demo          Resources to run ZPRnet demos
zpr-utils         Non-ZPR-specific utilities used by ZPR
zpr-dev-tools     Development tools for the ZPR project
```

All ten default to branch `main`.

### 3.3 Local state

v0.1 stores no local state. Everything `status` and `validate` report is
derived from the filesystem and from `git`. There is no
`~/.config/zpr-dev/config.yaml` and no `<workspace>/.zpr-dev/state.yaml`.

---

## 4. Generated Context Files

### 4.1 Inputs and outputs

```text
Inputs:   zpr-dev-context/AGENTS.md          (required)
          zpr-dev-context/docs/*.md          (for reference rewriting)
          <repo>/AGENTS.repo.md              (optional)

Outputs:  <repo>/AGENTS.md                   (generated)
          <repo>/CLAUDE.md                   (generated pointer)
```

### 4.2 Rendered `AGENTS.md`

```markdown
<!-- Generated by zpr-dev. Do not edit manually. -->
<!-- Source: zpr-dev-context @ 4ba137c -->
<!-- Shared docs: /home/mathias/src/zpr/zpr-dev-context/docs -->

# Shared ZPR Development Context

...body of context/AGENTS.md, with docs/ references absolutized...

# Repository-Specific Context

...body of AGENTS.repo.md...
```

If `AGENTS.repo.md` is absent, the `# Repository-Specific Context` heading is
omitted entirely rather than emitted empty.

The `Source:` comment carries the short commit SHA of the context checkout's
`HEAD`.

### 4.3 Rendered `CLAUDE.md`

```markdown
<!-- Generated by zpr-dev. Do not edit manually. -->
See [AGENTS.md](./AGENTS.md) for shared ZPR development context.
```

### 4.4 Documentation reference rewriting

The shared `AGENTS.md` refers to documentation as `docs/VISA_SERVICE.md`,
which is correct relative to the context repository but wrong once the text is
embedded in `zpr-core/AGENTS.md` — an agent would look in `zpr-core/docs/`.

`zpr-dev` therefore enumerates the actual files under
`<context>/<documentation.root>/` and, for each one, replaces occurrences of
its manifest-relative path with its absolute path:

```text
docs/VISA_SERVICE.md
  -> /home/mathias/src/zpr/zpr-dev-context/docs/VISA_SERVICE.md
```

This is a literal string replacement driven by the directory listing, not a
pattern match. Consequently it can only ever rewrite a reference that
resolves to a real file, and a reference to a nonexistent document is left
untouched — where `validate` will report it (§7).

### 4.5 Staleness

Staleness is defined as: **the rendered content differs, byte for byte, from
what is on disk.** There is no digest sidecar and no recorded state.

A new context commit changes the embedded SHA, so it is detected. An
uncommitted edit in the context checkout changes the body, so it is also
detected. A hand-edit of a generated file changes the file, so it too is
detected — though indistinguishably from staleness, which is why §1.4.5
makes this a warning.

### 4.6 Plan / apply

A single function produces the change set:

```rust
fn plan(ctx: &Ctx, manifest: &Manifest) -> Result<Vec<RepoPlan>>
```

```rust
enum Action {
    Create,        // generated file absent
    Update,        // generated file present but differs
    Unchanged,     // byte-identical
    RepoMissing,   // checkout directory does not exist
}
```

- `sync` applies the plan.
- `status` reports it.
- `validate` reports it.

One code path, three consumers. `Unchanged` entries are never written, so
`sync` does not churn mtimes and a second run is a true no-op.

---

## 5. Command Behavior

### 5.1 Global options

```text
--workspace <path>    Override workspace directory
--context <path>      Override zpr-dev-context checkout
-v, --verbose         Show additional detail
-q, --quiet           Suppress non-error output
--dry-run             Show intended changes without modifying anything
-h, --help
--version
```

`--dry-run` suppresses every mutation: no clone, no fetch, no merge, no file
write. Intended actions are printed as they would have been performed.

### 5.2 `setup`

```text
--context-url <git-url>   Default git@github.com:org-zpr/zpr-dev-context.git
--branch <branch>         Branch to clone for the context repository
--no-clone                Do not clone missing source repositories
```

Sequence:

1. Create the workspace directory if necessary.
2. If the context checkout is absent, clone it. If present, leave it entirely
   alone — no fetch, no checkout, no branch change.
3. Load and validate the manifest.
4. For each repository: clone if the directory is absent; otherwise leave it
   untouched.
5. Generate context files (§4).
6. Run validation (§7).
7. Print a summary.

`setup` never discards local modifications and never changes a branch.

### 5.3 `update`

```text
--all                Also update source repositories
--repo <name>        Update only the named repository
--no-generate        Skip regeneration afterward
```

Default target set is the context repository alone. `--all` adds every
repository in the manifest. `--repo <name>` targets exactly one, which may be
the context repository.

Per repository:

```text
git fetch
git merge --ff-only @{u}
```

A repository is skipped, with the reason reported, when it is:

| Condition | Reported as |
|---|---|
| not a Git repository | `not a git repository` |
| dirty working tree | `local modifications` |
| detached HEAD | `detached HEAD` |
| no upstream for the current branch | `no upstream` |
| fast-forward not possible | `cannot fast-forward` |

Updated repositories report `<old sha> -> <new sha>`. Unchanged repositories
report `current`.

The current branch is updated whatever it is — a feature branch with an
upstream is fast-forwarded, not switched to `default_branch`. `default_branch`
is used only when cloning.

Regeneration runs afterward unless `--no-generate`.

`update` never resets, rebases, force-pushes, deletes a branch, or stashes.

### 5.4 `status`

```text
--porcelain          Machine-readable, tab-separated
--repo <name>        Restrict to one repository
```

Human output:

```text
WORKSPACE /home/mathias/src/zpr

REPOSITORY        BRANCH       STATUS       UPSTREAM
zpr-dev-context   main         clean        current
zpr-core          feature-x    modified     ahead 2
zpr-visaservice   main         clean        behind 3

AGENT CONTEXT
zpr-core          current
zpr-visaservice   stale
zpr-utils         missing repository
```

Ahead/behind is computed locally with
`git rev-list --left-right --count HEAD...@{u}`. `status` performs no network
access — it does not fetch, so `behind` reflects the last fetch.

`--porcelain` emits one tab-separated record per repository with a stable
field order, followed by generated-context records. Field order is part of
the contract and will not change within v0.x.

### 5.5 `sync`

Apply the plan from §4.6. No fetch, no pull, no network access. Reports
created, updated, and unchanged files.

### 5.6 `validate`

See §7.

---

## 6. Implementation Structure

### 6.1 Dependencies

| Crate | Purpose |
|---|---|
| `clap` (derive) | Argument parsing for five subcommands plus global options |
| `serde` (derive) | Manifest deserialization |
| `serde_yaml_ng` | YAML parsing. A maintained drop-in for the unmaintained `serde_yaml` |
| `anyhow` | Error propagation and context |

Dev-dependency: `tempfile`, for integration-test workspaces.

Git is invoked through `std::process::Command`. There is no async runtime, no
HTTP client, no terminal-color or progress-bar crate, and no `regex`.

### 6.2 Modules

```text
zpr-dev/src/
├── main.rs        clap types, global options, dispatch, exit codes
├── config.rs      manifest types and loading; workspace/context resolution
├── git.rs         thin git wrappers
├── generate.rs    render, plan, apply
└── commands.rs    setup, update, status, sync, validate
```

A single context struct is threaded through the commands:

```rust
struct Ctx {
    workspace: PathBuf,
    context: PathBuf,
    dry_run: bool,
    verbose: bool,
    quiet: bool,
}
```

### 6.3 `git.rs` surface

```rust
fn git(dir: &Path, args: &[&str]) -> Result<String>   // capture stdout; Err on nonzero exit
fn is_repo(dir: &Path) -> bool
fn head_short(dir: &Path) -> Result<String>
fn branch(dir: &Path) -> Result<Option<String>>       // None when detached
fn is_dirty(dir: &Path) -> Result<bool>               // git status --porcelain non-empty
fn ahead_behind(dir: &Path) -> Result<Option<(usize, usize)>>   // None when no upstream
fn clone(url: &str, dest: &Path, branch: Option<&str>) -> Result<()>
fn fetch(dir: &Path) -> Result<()>
fn ff_merge(dir: &Path) -> Result<bool>               // false when fast-forward impossible
```

Every mutating function is a no-op that logs its intent when `ctx.dry_run` is
set.

### 6.4 Exit codes

```text
0   success, warnings permitted
1   validation errors
2   command or configuration error
```

An `anyhow` error reaching `main` exits 2. A `validate` run that accumulated
errors exits 1. Warnings alone do not affect the exit code.

---

## 7. Validation Checks

| Check | Severity when failing |
|---|---|
| Context checkout exists and is a Git repository | error |
| `workspace.yaml` exists and parses | error |
| `version` equals 1 | error |
| `repositories` is non-empty | error |
| Repository names are unique and non-empty | error |
| `context/AGENTS.md` exists | error |
| Every `docs/*.md` reference in `context/AGENTS.md` resolves to a real file | error |
| Each manifest repository directory exists | error |
| Each repository directory is a Git repository | error |
| Generated files match their rendered content | warning, suggests `zpr-dev sync` |
| `agent.hermes.shared_skills` directory exists, when declared | warning |
| `AGENTS.repo.md` present in a repository | informational only; absence is legitimate |

Output form:

```text
$ zpr-dev validate

[OK]   context repository
[OK]   workspace manifest
[OK]   10 source repositories
[WARN] generated context stale in 2 repositories (run: zpr-dev sync)
[OK]   documentation references

Validation completed with 1 warning.
```

---

## 8. Testing

### 8.1 Integration tests

`zpr-dev/tests/integration.rs` drives the compiled binary through
`env!("CARGO_BIN_EXE_zpr-dev")` against a throwaway workspace assembled from
local `git init --bare` origin repositories addressed by `file://` URL. No
network access and no credentials are required.

Fixture construction:

1. Create a temporary directory.
2. `git init --bare` two origin repositories.
3. `git init` a context repository containing `AGENTS.md`, one `docs/*.md`,
   and a `workspace.yaml` whose repository URLs are `file://` paths to the
   bare origins. Commit it.

Cases:

| Case | Asserts |
|---|---|
| `setup` on an empty workspace | Repositories are cloned; each gains an `AGENTS.md` containing the shared body and the generated header, plus a `CLAUDE.md` pointer |
| `sync` run twice | The second run reports no changes and modifies no file mtime |
| `AGENTS.repo.md` added, then `sync` | Generated file gains the `# Repository-Specific Context` section |
| Documentation reference in shared `AGENTS.md` | Rewritten to the absolute path in the generated output |
| Dirty repository, then `update --all` | Repository reported as skipped for `local modifications`; its working tree and `HEAD` are unchanged |
| Clean repository behind its origin, then `update --all` | Fast-forwarded; old and new SHAs reported |
| `validate` on a healthy workspace | Exit code 0 |
| `validate` after breaking a documentation reference | Exit code 1 |
| `--dry-run sync` | Prints intended changes; no file is written |

### 8.2 Unit tests

In `generate.rs`:

- Documentation reference rewriting, including that a reference to a
  nonexistent document is left untouched.
- Header rendering, including SHA placement and the omission of the
  repository-specific section when `AGENTS.repo.md` is absent.

---

## 9. Repository Deliverables

Written as part of this work:

```text
zpr-dev-context/
├── workspace.yaml          new: the ten repositories from §3.2
├── README.md               amended: install and bootstrap notes
└── zpr-dev/
    ├── Cargo.toml          dependencies from §6.1
    ├── src/                modules from §6.2
    └── tests/
        └── integration.rs
```

`AGENTS.md` is supplied separately by the repository owner and is not written
by this work. The integration tests build their own fixture context
repository, so they do not depend on it.

`docs/` stubs and `skills/` content are likewise out of scope here.

---

## 10. Installation and Bootstrap

```bash
cargo install --path zpr-dev-context/zpr-dev
```

There is a bootstrap ordering wrinkle worth documenting in the README: the
tool ships inside the repository it clones. A developer who clones
`zpr-dev-context` to an arbitrary location and then runs `zpr-dev setup` will
have a second copy cloned into `<workspace>/zpr-dev-context`.

The recommended sequence avoids this by cloning into the workspace from the
start:

```bash
mkdir -p ~/src/zpr
git clone git@github.com:org-zpr/zpr-dev-context.git ~/src/zpr/zpr-dev-context
cargo install --path ~/src/zpr/zpr-dev-context/zpr-dev
zpr-dev setup
```

`setup` then finds the existing context checkout and leaves it alone.
Developers who keep the context checkout elsewhere can pass `--context`.

---

## 11. Safety Invariants

`zpr-dev` v0.1 shall not, under any command:

- reset a repository;
- delete a branch;
- discard, stash, or overwrite uncommitted changes;
- force-push, or push at all;
- rebase;
- switch branches;
- modify agent configuration not written by `zpr-dev`;
- write any file other than the generated `AGENTS.md` and `CLAUDE.md` in a
  source repository.

The only files `zpr-dev` writes inside a source repository are the two
generated ones, and it writes them only when their rendered content differs
from what is on disk.
