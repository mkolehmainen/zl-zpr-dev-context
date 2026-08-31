# SPEC-001: `zpr-dev` v0.1 Design

Status: implemented — v0.1 complete
Date: 2026-08-27 (revised after implementation)
Parent spec: `spec-000-parent.md`

This document specifies the first implementation of the `zpr-dev` tool. It
narrows the parent spec to a buildable v0.1 and records the decisions made
during design. Where this document and the parent spec disagree, this
document governs v0.1.

All eleven implementation steps of `spec-001-plan.md` have landed. This
revision folds the plan's `Landed` notes back into the spec, so the text below
describes the tool **as built** rather than as originally designed. The plan
remains the record of *why* each deviation was made.

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

`zpr-dev agent configure <agent>` and `zpr-dev agent status` were added
afterward by SPEC-002; §1.2 records why they were not in v0.1.

### 1.2 Out of scope for v0.1

Each of the following is deliberately deferred. None of them is blocked by a
design decision made here; each can be added without restructuring.

| Deferred | Reason |
|---|---|
| `doctor` | Diagnostic sugar. `validate` covers the failures that matter today. |
| `agent configure <agent>` | Deferred in v0.1 because Hermes' configuration path and schema were not then known, so the command would have wrapped a YAML merge we could not test. **Implemented since, by `spec-002-hermes.md`** — read that document, not this row, for the behavior. Codex and Claude still need no global configuration once repository-local `AGENTS.md` exists. |
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
   a guessed Hermes configuration format. See §1.2. *Superseded:*
   `spec-002-hermes.md` implements it against the real format, and narrows the
   §11 invariant accordingly.
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
6. **"Dirty" means tracked-file changes only.** Because generated files are
   left untracked (§1.4.3), counting untracked files would make every synced
   repository permanently `modified` and would make `update --all` skip every
   repository for `local modifications`. `is_dirty` therefore runs
   `git status --porcelain --untracked-files=no`. No safety is lost: an
   untracked file that a fast-forward would clobber still makes
   `git merge --ff-only` refuse, which is reported as `cannot fast-forward`.

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

All ten default to branch `main`. The shipped `workspace.yaml` omits
`default_branch` entirely and relies on the serde default, so each entry is two
lines plus a descriptive comment.

### 3.3 Local state

v0.1 stores no local state. Everything `status` and `validate` report is
derived from the filesystem and from `git`. There is no
`~/.config/zpr-dev/config.yaml` and no `<workspace>/.zpr-dev/state.yaml`.

---

## 4. Generated Context Files

### 4.1 Inputs and outputs

```text
Inputs:   zpr-dev-context/AGENTS.md          (required)
          zpr-dev-context/*/                 (for reference rewriting)
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
`HEAD`, or `unknown` when `HEAD` cannot be read. Rendering does not fail in
that case, because `validate` reaches its own diagnostics through
`generate::plan` and must not be blocked by a broken context checkout.

Each section body is trimmed and emitted with exactly one trailing newline, so
the byte-for-byte staleness comparison of §4.5 does not depend on trailing
whitespace in the source files.

### 4.3 Rendered `CLAUDE.md`

```markdown
<!-- Generated by zpr-dev. Do not edit manually. -->
See [AGENTS.md](./AGENTS.md) for shared ZPR development context.
```

### 4.4 Context reference rewriting

The shared `AGENTS.md` refers to the context checkout's own contents
relatively — `docs/VISA_SERVICE.md`, or the directories `docs/` and `skills/`
— which is correct there but wrong once the text is embedded in
`zpr-core/AGENTS.md`, where an agent would look in `zpr-core/docs/`.

`zpr-dev` therefore enumerates the **top-level directories** of the context
checkout and replaces occurrences of each directory reference with its
absolute path:

```text
docs/
  -> /home/mathias/src/zpr/zpr-dev-context/docs/

docs/VISA_SERVICE.md
  -> /home/mathias/src/zpr/zpr-dev-context/docs/VISA_SERVICE.md
```

Keying on the directory rather than on each enumerated document is what makes
both lines above work from one entry: the second is just the first with a tail
left in place. Dot-directories (`.git`, `.claude`) are skipped.

The **trailing slash is part of the key**. Keyed on `docs` alone, the English
word "docs" in prose would be rewritten to a path.

This is a literal string replacement driven by the directory listing, not a
pattern match. A reference under a directory that does not exist in the
context checkout is left untouched; a reference to a nonexistent document
under a directory that does exist becomes an absolute path that also does not
exist. Either way `validate` reports it (§7), because that check tests each
reference against the filesystem independently (§7's `broken_doc_references`)
rather than against this rewrite list.

The replacement is **one left-to-right scan**, not one `String::replace` per
directory. The scan tries the rewrite list longest-key-first at each position
and skips past whatever it emitted, so a rewrite can never match inside a path
it has just produced. A missing context directory yields zero rewrites rather
than an error.

Only the shared body is rewritten. `AGENTS.repo.md` is embedded verbatim,
where `docs/` correctly means the repository's own `docs/`. The corollary is
that the shared `AGENTS.md` must not use a bare `docs/` to mean "each
repository's own docs".

### 4.5 Staleness

Staleness is defined as: **the rendered content differs, byte for byte, from
what is on disk.** There is no digest sidecar and no recorded state.

A new context commit changes the embedded SHA, so it is detected. An
uncommitted edit in the context checkout changes the body, so it is also
detected. A hand-edit of a generated file changes the file, so it too is
detected — though indistinguishably from staleness, which is why §1.4.5
makes this a warning.

One case is *not* mere staleness: a file at a generated path that does not begin
with the generated marker was never written by `zpr-dev`. That is a repository
maintaining its own `AGENTS.md`, and overwriting it destroys content nothing
else holds — `zpr-visaservice` lost its coding conventions exactly this way. The
marker is therefore load-bearing, not decorative: absence of it means **do not
write this file**. See `Action::Foreign` in §4.6. The remedy is for the
repository to rename its file to `AGENTS.repo.md`, which §4.2 includes in the
generated output rather than replacing.

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
    Foreign,       // present, differs, and carries no generated marker
    RepoMissing,   // checkout directory does not exist
}
```

Each `PlannedFile` carries its own `Action`, so `apply` can report *created*
separately from *updated* without re-`stat`ing at apply time. The
`RepoPlan`-level `Action` is the worst-of across its files (`Foreign` >
`Create` > `Update` > `Unchanged`), or `RepoMissing` when the checkout directory
is absent — in which case the file list is empty. `apply` counts **files**, not
repositories; a caller that wants to mention repositories that are not checked
out counts `RepoMissing` plans itself.

`Foreign` leads the ordering because it is the only action a human must resolve:

- `apply` **never writes** a `Foreign` file. It names each one on stdout
  regardless of `--verbose` and tallies them in `ApplySummary.skipped_foreign`.
- `status` shows `not generated by zpr-dev` instead of `stale`.
- `validate` raises an **error** per file, not a warning: `sync` cannot clear
  the finding, so it must not exit zero.

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
write. Intended actions are printed as they would have been performed. A
`git fetch` counts as a mutation because it rewrites `.git/refs/remotes`, so
dry-run stops before it and cannot predict whether a repository would end up
`current` or fast-forwarded.

The dry-run gate lives at each call site in `commands.rs`, not in `git.rs`;
the Git layer is pure and always executes what it is asked to.

`--quiet` gates progress output only. A command's *result* — `status`'s table,
`validate`'s findings — is always printed.

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

`setup` passes `validate`'s exit code straight through, so `setup --no-clone`
on an empty workspace exits 1 (the missing directories are real validation
errors) while a healthy `setup` exits 0. A clone failure is a command error
(exit 2).

The summary is per-phase and chronological rather than one block at the end:
the clone tally prints when cloning finishes, the generation line when
generation finishes, and the validation report ends the run and supplies the
exit code.

Under `--dry-run` with the context checkout absent, `setup` prints the intended
workspace creation and clone and then returns 0 without steps 3–6: there is no
manifest on disk to read, and inventing that output would be a lie.

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

The check order is `is_repo` → `is_dirty` → `branch` → `ahead_behind`, and it
is load-bearing: `ahead_behind` returns `None` for a detached `HEAD` as well as
for a missing upstream, so `detached HEAD` must be decided by `branch(…) ==
None` before the upstream check or it would be misreported as `no upstream`.

There is no separate "behind" check before the merge. `git merge --ff-only
@{u}` is already a no-op when the upstream is an ancestor, so the `current`
verdict is simply `head_short` before == after — one git invocation fewer, and
no second definition of "already up to date" to keep in sync.

`--repo <name>` makes the manifest repositories candidates on its own, so
`--repo <source-repo>` works without `--all`. An unknown name is a command
error (exit 2).

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

`STATUS` has four values, not the two the parent spec implies: `clean` and
`modified` for a present checkout, plus `missing` for an absent directory and
`not a git repository` for a present non-repository. Those last two rows carry
`-` for BRANCH and `no upstream` for UPSTREAM, and use the same wording as the
corresponding `validate` errors. A detached `HEAD` shows BRANCH `detached`.

The context checkout leads the table, named after its directory. It holds no
generated context of its own, so it appears in no other section.

`--repo <name>` filters both sections and accepts the context checkout; an
unknown name is a command error (exit 2).

Ahead/behind is computed locally with
`git rev-list --left-right --count HEAD...@{u}`. `status` performs no network
access — it does not fetch, so `behind` reflects the last fetch.

`--porcelain` emits one tab-separated record per repository with a stable
field order, followed by generated-context records. Field order is part of the
contract and will not change within v0.x:

```text
repo   <name>  <branch>  <clean|modified|missing|not a git repository>  <ahead>  <behind>
agent  <name>  <current|stale|missing repository>
```

`<branch>` is `-` when there is no repository and `detached` when `HEAD` is
detached; `<ahead>` and `<behind>` are `-` when there is no upstream. The
record-kind prefix is what keeps the two sections distinguishable in one
stream.

### 5.5 `sync`

Apply the plan from §4.6. No fetch, no pull, no network access. Reports
created, updated, and unchanged **files**:

```text
wrote generated context: 2 created, 0 updated, 2 unchanged
```

The verb is `would write` under `--dry-run`; the counts are identical either
way. Repositories that are not checked out add a second line, `skipped N
repositories not checked out`, and do not change the exit code — a missing
repository is `validate`'s error to report, not `sync`'s.

`setup` and `update` share this code path exactly, so their regeneration
output and counts are the same.

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
fn is_dirty(dir: &Path) -> Result<bool>               // tracked-file changes only (§1.4.6)
fn ahead_behind(dir: &Path) -> Result<Option<(usize, usize)>>   // None when no upstream
fn clone(url: &str, dest: &Path, branch: Option<&str>) -> Result<()>
fn fetch(dir: &Path) -> Result<()>
fn ff_merge(dir: &Path) -> Result<bool>               // false when fast-forward impossible
```

`git()` returns trimmed stdout. `is_repo` compares `git rev-parse
--show-toplevel` against the canonicalized directory rather than merely
checking that the command succeeded, so a plain directory nested inside a
repository does not report true. `branch` maps the literal `"HEAD"` to `None`,
so detachment is a value rather than an error. `ahead_behind` returns `None`
for any failure of the `rev-list` range, which covers both "no upstream" and a
detached `HEAD` — callers that need to tell them apart check `branch` first.
`clone` creates the destination's parent directory.

This layer is pure and always executes; the `--dry-run` gate lives at each call
site in `commands.rs`, which is also where the "would have" message is printed
(§5.1).

### 6.4 Exit codes

```text
0   success, warnings permitted
1   validation errors
2   command or configuration error
```

An `anyhow` error reaching `main` exits 2. A `validate` run that accumulated
errors exits 1, and `setup` passes that code through because it ends in
validation. Warnings alone do not affect the exit code.

---

## 7. Validation Checks

| Check | Severity when failing |
|---|---|
| Context checkout exists and is a Git repository | error, ends the run |
| `workspace.yaml` exists and parses, `version` equals 1, `repositories` is non-empty, names are unique and non-empty | error, ends the run |
| `context/AGENTS.md` exists | error |
| Every documentation reference in `context/AGENTS.md` resolves to an existing path | error |
| Each manifest repository directory exists | error |
| Each repository directory is a Git repository | error |
| Generated files match their rendered content | warning, suggests `zpr-dev sync` |
| `agent.hermes.shared_skills` directory exists, when declared | warning |
| `AGENTS.repo.md` present in a repository | informational only; absence is legitimate |

Findings accumulate rather than stopping at the first, with two exceptions: an
absent context directory and a failed manifest load both end the run, because
every later check needs the manifest.

The five manifest checks are one row above because they are one code path.
`config::load` performs them and reports the first structural problem it finds
as an error; `validate` prints it as a single `[ERROR] workspace manifest:` line.
Splitting them into five independently accumulated checks would mean a second
implementation of the parser's validation.

A documentation reference is any token in the raw `AGENTS.md` that begins with
`<documentation.root>/`, after splitting on whitespace and Markdown link
punctuation and stripping a leading `./` and a trailing sentence period. It
must resolve to an existing path — **a directory counts**, so a mention of
`docs/` naming the documentation directory is not a broken reference. This
check is deliberately not built on §4.4's rewrite list: that list names
directories that exist, so it cannot say which document beneath one is
missing.

Output form:

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

`[INFO]` is a fourth tag beyond the parent spec's three, carrying the
`AGENTS.repo.md` row; it touches neither the counts nor the exit code. When
errors are present the summary line is instead `Validation failed with N
error(s) and M warning(s).`

---

## 8. Testing

### 8.1 Integration tests

`zpr-dev/tests/integration.rs` drives the compiled binary through
`env!("CARGO_BIN_EXE_zpr-dev")` against a throwaway workspace assembled from
local `git init --bare` origin repositories addressed by `file://` URL. No
network access and no credentials are required.

Fixture construction (`tests/common/mod.rs`, a shared module rather than its
own test binary):

1. Create a temporary directory holding `origins/` and `workspace/`. The
   origins live **outside** the workspace so a stray `origins/` entry never
   appears in `status` or `validate` output.
2. `git init --bare` three origin repositories — two source repositories and
   `zpr-dev-context` — each seeded with one commit through a throwaway clone.
3. Clone the context origin into `<workspace>/zpr-dev-context`. It is cloned
   rather than `git init`'d in place so that it has an upstream, which
   `update`'s default target set needs. It contains `AGENTS.md` (referencing
   `docs/EXAMPLE.md`), `docs/EXAMPLE.md`, and a `workspace.yaml` whose
   repository URLs are `file://` paths to the bare origins.

Git is isolated per child process rather than per environment: every spawned
command sets `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_SYSTEM=/dev/null`, the
four `GIT_{AUTHOR,COMMITTER}_{NAME,EMAIL}` variables, and
`GIT_TERMINAL_PROMPT=0`, and removes `ZPR_WORKSPACE` so a developer's shell
cannot leak in. The `git.rs` unit tests cannot do this — `std::env::set_var` is
`unsafe` and process-global in edition 2024 — so they pin `git init -b main`
and per-repository `user.name` / `user.email` / `commit.gpgsign=false` instead.

The fixture manifest deliberately has no `agent` block, so no command test
carries a permanent `shared_skills` warning; the test that needs one appends it
to `workspace.yaml` itself.

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

- Context reference rewriting: documents, directory references, that a bare
  directory word in prose is left untouched, and that a reference under an
  unknown directory is left untouched.
- Header rendering, including SHA placement and the omission of the
  repository-specific section when `AGENTS.repo.md` is absent.

---

## 9. Repository Deliverables

Written as part of this work:

```text
zpr-dev-context/
├── workspace.yaml          new: the ten repositories from §3.2
├── README.md               new: command table, install and bootstrap notes
└── zpr-dev/
    ├── Cargo.toml          dependencies from §6.1
    ├── src/                modules from §6.2
    └── tests/
        ├── common/mod.rs   fixture harness (§8.1)
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
- modify agent configuration not written by `zpr-dev`, **except** as narrowed
  by SPEC-002 below;
- write any file other than the generated `AGENTS.md` and `CLAUDE.md` in a
  source repository.

The only files `zpr-dev` writes inside a source repository are the two
generated ones, and it writes them only when their rendered content differs
from what is on disk.

`spec-002-hermes.md` narrows the agent-configuration invariant rather than
dropping it. `zpr-dev` may modify exactly one key, `skills.external_dirs`, in
exactly one file, `$HOME/.hermes/config.yaml`, only under an explicit
`agent configure` invocation, only through an edit verified to have changed that
key and nothing else, and only after backing the file up. Every other key in
that file, and every other agent's configuration, remains untouchable.

Verified after implementation: `reset`, `rebase`, `stash`, `checkout`, `push`,
and `branch -d` appear only inside `#[cfg(test)]` code and the test binaries,
which push to local bare origins to build fixtures. No command path contains
any of them.
