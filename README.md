# zl-zpr-dev-context

Agent context for ZPR development: the coding standards, architecture
documentation, and skills that every coding agent working in a `zl-zpr-*`
repository should see.

This is the **zipline** fork of the ZPR workspace. Every repository it names is
a fork under [`mkolehmainen`](https://github.com/mkolehmainen) of the
corresponding upstream repository in the
[`org-zpr`](https://github.com/org-zpr) organization: `zl-zpr-core` forks
`org-zpr/zpr-core`, and so on. Only the names and clone URLs changed — nothing
*inside* the forks has been repointed, so their Cargo Git dependencies,
submodule URLs, and CI workflow references still resolve to `org-zpr`. See
[`docs/REPOSITORIES.md`](docs/REPOSITORIES.md).

Each fork has two long-lived branches: **`zipline`** is the working branch and
the repository default, and **`main`** is a read-only mirror of upstream `main`.
Feature branches start from `zipline` and pull requests target it. See
[`docs/REPOSITORIES.md`](docs/REPOSITORIES.md) ("Branch model") for how to sync
and merge without breaking that.

`docs/` is the documentation agents read; the specs behind the tooling live in
`zpr-dev/docs/specs/` (`spec-001-zpr-dev.md` for the tool as built,
`spec-000-parent.md` for the original design record).

## For humans: what still needs doing by hand

The fork rename and the branch model are done. All eleven repositories exist as
`mkolehmainen/zl-zpr-*`, each has a `zipline` branch created from `main`, and
`zipline` is the default branch in every one. The list below is what tooling
could not or should not do on its own.

**Once per clone** — `gh` points a fork's commands at its parent by default, so
without this a `gh pr create` offers to open your PR against `org-zpr`:

```bash
gh repo set-default mkolehmainen/zl-zpr-<name>
```

**Once per repository, if you want them** (each needs a decision, not just a
command):

| Action | Why it is not automated |
|---|---|
| Protect `main` against direct pushes: `gh api -X PUT repos/mkolehmainen/zl-zpr-<n>/branches/main/protection ...` | `main` is only a mirror by convention right now; nothing enforces it. Protecting it makes the convention real, but it is a policy call about your own workflow. |
| Add the `SLACK_ALERT_WEBHOOK_URL` secret, or delete `pr-notify.yml` | Forks inherit no secrets, so `pr-notify.yml` fails on every PR. Fixing it needs the webhook value, which only you have. Judge builds on the `rust-build-test` checks meanwhile. |
| Widen the CI `push` branch filters to include `zipline` | The workflows use `push: branches: ["main"]`. `pull_request` has no branch filter, so **the PR gate already works on zipline PRs** — only post-merge push builds are silent. Widening it means a commit in each of the ten forks. |
| Repoint cross-repository references to the forks | The `Cargo.toml` Git dependencies, `zl-zpr-common`'s `.gitmodules`, and the reusable workflow refs all still resolve to `org-zpr`. Until they are changed, a tag you push to `zl-zpr-common` reaches no consumer. See [`docs/BUILD.md`](docs/BUILD.md). |
| Confirm the `zipline` board can hold fork issues | Project 5 lives in `org-zpr` while the repositories are personal forks. It currently tracks an *upstream* issue. Adding an issue from a fork has not been tested — try it once before relying on it. |

**Routine, whenever you want upstream changes:**

```bash
gh repo sync mkolehmainen/zl-zpr-<name> --source org-zpr/zpr-<name>   # main only
git checkout zipline && git merge main                                # then merge in
```

## `zpr-dev`

`zpr-dev` makes the workspace reproducible. It clones the repository set listed
in `workspace.yaml` side by side, then renders this repository's `AGENTS.md`
(plus each repository's optional `AGENTS.repo.md`) into a generated
`<repo>/AGENTS.md` and `<repo>/CLAUDE.md`, with documentation references
rewritten to absolute paths so an agent in `zl-zpr-core` can actually open them.

| Command | What it does |
|---|---|
| `zpr-dev setup` | Clone the workspace, generate context files, validate |
| `zpr-dev update` | Fetch and fast-forward (context only; `--all` for every repository) |
| `zpr-dev status` | Report each checkout and whether generated context is current |
| `zpr-dev sync` | Regenerate the context files (no network access) |
| `zpr-dev validate` | Check workspace health; exit 1 on errors |
| `zpr-dev agent configure hermes` | Point Hermes at this repository's `skills/` directory |
| `zpr-dev agent status` | Report whether each agent is configured |

No command ever resets, rebases, stashes, pushes, switches branches, or touches
uncommitted work. `--dry-run` suppresses every mutation and prints what would
have happened.

The one file outside the workspace that `zpr-dev` will edit is
`~/.hermes/config.yaml`, only under `agent configure`, and only to add this
repository's `skills/` directory to `skills.external_dirs`. It backs the file up
first and verifies that the edit changed exactly that one key.

### Install and bootstrap

```bash
cargo install --path zl-zpr-dev-context/zpr-dev
```

There is an ordering wrinkle: the tool ships inside the repository it clones. If
you clone `zl-zpr-dev-context` to an arbitrary location and then run `zpr-dev
setup`, you end up with a second copy under `<workspace>/zl-zpr-dev-context`. Clone
into the workspace from the start instead:

```bash
mkdir -p ~/src/zl_zpr
git clone git@github.com:mkolehmainen/zl-zpr-dev-context.git ~/src/zl_zpr/zl-zpr-dev-context
cargo install --path ~/src/zl_zpr/zl-zpr-dev-context/zpr-dev
zpr-dev setup
```

`setup` then finds the existing context checkout and leaves it alone. If you
keep the checkout elsewhere, pass `--context <path>` (and `--workspace <path>`
if the workspace is not `~/src/zl_zpr`; `$ZPR_WORKSPACE` works too).

Claude and Codex need nothing further: they read the generated `AGENTS.md` and
`CLAUDE.md` from whichever repository they are started in. Hermes discovers
skills through its own configuration, so if you use it, run this once:

```bash
zpr-dev agent configure hermes
```

It is idempotent, and `zpr-dev agent status` reports the result.

### Generated files are untracked

The generated `AGENTS.md` and `CLAUDE.md` are deliberately not committed, and
`zpr-dev` does not manage `.gitignore` or `.git/info/exclude`. They will
therefore show up as untracked files in `git status` in every repository. That
is expected — the context is rendered per workspace, so committing it would
bake one developer's absolute paths into the repository.

A repository that already tracks its own `AGENTS.md` is the one case to handle
by hand. `zpr-dev` will not overwrite a file it did not generate — `sync` prints
`refusing to overwrite ...` and `validate` fails — because doing so silently
destroys conventions nothing else records. Rename that file to
`AGENTS.repo.md`, which generation appends under a "Repository-Specific
Context" heading instead of replacing, and stop tracking `AGENTS.md`/`CLAUDE.md`.

If you would rather the shared context just win, `--force` overwrites the file
instead of refusing: `zpr-dev --force sync` clobbers it and says so, and
`zpr-dev --force validate` reports a warning rather than an error. The clobber
is always announced — the flag suppresses the error, not the notice.
