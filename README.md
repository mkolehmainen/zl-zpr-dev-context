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

No command ever resets, rebases, stashes, pushes, switches branches, or touches
uncommitted work. `--dry-run` suppresses every mutation and prints what would
have happened.

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

### Generated files are untracked

The generated `AGENTS.md` and `CLAUDE.md` are deliberately not committed, and
`zpr-dev` does not manage `.gitignore` or `.git/info/exclude`. They will
therefore show up as untracked files in `git status` in every repository. That
is expected — the context is rendered per workspace, so committing it would
bake one developer's absolute paths into the repository.
