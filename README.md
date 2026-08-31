# zpr-dev-context

Agent context for ZPR development: the coding standards, architecture
documentation, and skills that every coding agent working in an `org-zpr`
repository should see.

`docs/` is the documentation agents read; the specs behind the tooling live in
`zpr-dev/docs/specs/` (`spec-001-zpr-dev.md` for the tool as built,
`spec-000-parent.md` for the original design record).

## `zpr-dev`

`zpr-dev` makes the workspace reproducible. It clones the repository set listed
in `workspace.yaml` side by side, then renders this repository's `AGENTS.md`
(plus each repository's optional `AGENTS.repo.md`) into a generated
`<repo>/AGENTS.md` and `<repo>/CLAUDE.md`, with documentation references
rewritten to absolute paths so an agent in `zpr-core` can actually open them.

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
cargo install --path zpr-dev-context/zpr-dev
```

There is an ordering wrinkle: the tool ships inside the repository it clones. If
you clone `zpr-dev-context` to an arbitrary location and then run `zpr-dev
setup`, you end up with a second copy under `<workspace>/zpr-dev-context`. Clone
into the workspace from the start instead:

```bash
mkdir -p ~/src/zpr
git clone git@github.com:org-zpr/zpr-dev-context.git ~/src/zpr/zpr-dev-context
cargo install --path ~/src/zpr/zpr-dev-context/zpr-dev
zpr-dev setup
```

`setup` then finds the existing context checkout and leaves it alone. If you
keep the checkout elsewhere, pass `--context <path>` (and `--workspace <path>`
if the workspace is not `~/src/zpr`; `$ZPR_WORKSPACE` works too).

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
