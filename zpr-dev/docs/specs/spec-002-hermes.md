# SPEC-002: `zpr-dev agent` — Hermes configuration

Status: implemented
Date: 2026-08-31 (revised after implementation)
Parent spec: `spec-001-zpr-dev.md`

All seven steps of the plan below have landed, and the sections above it were
revised afterward to describe the tool **as built** rather than as designed. The
deviations were small and are noted where they occur.

Implements `zpr-dev agent configure hermes` and `zpr-dev agent status`, the
`agent configure <agent>` command that SPEC-001 §1.2 deferred because the Hermes
configuration path and schema were then unknown. Where this document and
SPEC-001 disagree, this document governs; §8 lists the SPEC-001 text it
supersedes.

---

## 1. Goal

Hermes discovers skills outside its own installation through one key in its
configuration file:

```yaml
skills:
  external_dirs:
    - /home/mathias/src/zpr/zpr-dev-context/skills
```

Setting that key by hand is the last manual step in workspace setup. This
command sets it, and reports whether it is set.

```text
zpr-dev agent configure hermes
zpr-dev agent status
```

### 1.1 In scope

- One agent, `hermes`, and one key, `skills.external_dirs`.
- Adding the workspace's shared skills directory to that key, idempotently,
  without disturbing anything else in the file.
- Reporting the resulting state.

### 1.2 Out of scope

| Deferred | Reason |
|---|---|
| `agent configure claude` / `codex` | Neither needs global configuration once the generated `AGENTS.md` and `CLAUDE.md` exist (SPEC-001 §1.4.2). |
| Removing a directory from `external_dirs` | The developer can delete a line. Adding an `--unset` for it is speculative. |
| Any Hermes key other than `skills.external_dirs` | Model, provider, toolsets, and personality are the developer's business, not the workspace's. |
| `agent status --porcelain` | Nothing scripts this yet. The field order in §5.2 is chosen so it can be added without changing the human output. |
| Configuring Hermes from `setup` | Global agent configuration is machine state, not workspace state, and SPEC-001 §11 forbids touching it implicitly. `agent configure` is explicit only. |
| A `--config` flag for the Hermes configuration path | `$HOME` already locates it, and overriding `$HOME` is how the tests reach a fixture (§7.2). A flag for a path with one real value is surface for nothing. |

### 1.3 Decisions made during this design

1. **Surgical text edit, not parse-and-re-emit.** A Hermes configuration file
   is machine-serialized but hand-annotated: it carries a `_config_version`
   key that Hermes itself maintains, and trailing commented-out blocks that a
   developer added. Deserializing to a `Value` and re-serializing would set the
   key correctly and destroy every comment in the file, and would rewrite
   roughly six hundred lines that `zpr-dev` does not own to change one. The
   edit therefore rewrites only the lines it must, and §4.4's verification pass
   buys back the correctness that a round-trip would have given for free.
2. **A missing configuration file is an error, not something to create.**
   Hermes owns that file's creation and its defaults; a partial file written
   ahead of Hermes' first run is a merge conflict waiting to happen. `hermes`
   being absent from `$PATH` is *not* an error — a developer may have it
   installed under a name or path we cannot guess, and the configuration file
   is the thing that actually matters.
3. **An absolute path is written.** This matches how `zpr-dev` already rewrites
   documentation references (SPEC-001 §4.4) and does not depend on whether
   Hermes expands `~`, which cannot be verified here.
4. **`agent status` is its own subcommand covering only agents that need
   global configuration** — today, only Hermes. The top-level `zpr-dev status`
   is untouched, and its `--porcelain` contract (SPEC-001 §5.4) is unchanged.
   Claude and Codex would contribute rows that only restate the existing
   `AGENT CONTEXT` section.
5. **`validate` gains no Hermes check.** `validate` reports on the workspace;
   whether one developer's machine has Hermes configured is not a property of
   the workspace, and would make `validate` fail for everyone who does not use
   Hermes. `agent status` is where that question is answered. The existing
   `agent.hermes.shared_skills` warning (SPEC-001 §7) stays as it is.

---

## 2. Inputs

| Input | Source | Absent means |
|---|---|---|
| Hermes configuration file | `$HOME/.hermes/config.yaml` | `configure`: command error (§3.3). `status`: `installed no`. |
| Shared skills directory, relative to the context checkout | `agent.hermes.shared_skills` in `workspace.yaml` | Command error: nothing to configure. |

`agent.hermes.shared_skills` is already parsed and validated for existence by
SPEC-001; this is the first command to act on it. The path written into the
Hermes configuration is `generate::absolute(context.join(shared_skills))` —
the same helper `status` uses for the context checkout's display name, so the
two cannot disagree about what "absolute" means.

The directory must exist. `configure` refuses to point Hermes at a directory
that is not there; `validate` continues to treat the same condition as a
warning, because a stale manifest entry should not fail an otherwise healthy
workspace.

---

## 3. `zpr-dev agent configure hermes`

### 3.1 Behavior

1. Resolve the shared skills directory (§2). Error if the manifest declares
   none, or if it does not exist.
2. Read `$HOME/.hermes/config.yaml`. Error if it is absent or unreadable.
3. Compute the edited text (§4). If the path is already present, report and
   exit 0 without writing.
4. Verify the edit (§4.4). Refuse and write nothing if verification fails.
5. Back up, write atomically (§4.5), and report.

### 3.2 Output

```text
$ zpr-dev agent configure hermes
configured hermes shared skills: /home/mathias/src/zpr/zpr-dev-context/skills
```

Already configured — no write, no backup, exit 0:

```text
hermes shared skills already configured: /home/mathias/src/zpr/zpr-dev-context/skills
```

Under `--dry-run` the verb is `would configure`, and nothing is read for write,
backed up, or written. `--verbose` additionally prints the lines that would be
inserted, as a unified diff hunk against the existing file.

### 3.3 Errors

All exit 2, and all leave the file untouched:

| Condition | Message |
|---|---|
| Manifest declares no `agent.hermes.shared_skills` | `manifest declares no agent.hermes.shared_skills; nothing to configure` |
| Shared skills directory absent | `shared skills directory missing: <path>` |
| `$HOME` unset or empty | `cannot locate the hermes configuration: $HOME is not set` |
| Configuration file absent | `hermes configuration not found: <path> (run hermes once to create it)` |
| Configuration file unreadable | `cannot read <path>: <reason>` |
| Any guard in §4.3 trips | `<path>: <the guard's own message>; edit the file by hand` |
| Verification in §4.4 fails | `refusing to write: <what differed> (this is a bug; please report it)` |

The wording of the "run hermes once" hint matters: the failure is expected on a
machine where Hermes has been installed but never started, and the developer
should not have to guess the remedy.

*As built:* each guard's own message carries the `edit the file by hand` remedy,
and the command prefixes only the configuration path. The alternative — the
command appending the remedy — put it before the cause in `anyhow`'s `{:#}`
rendering, which read badly and was wrong for a verification failure, where the
remedy is to report a bug rather than to edit the file.

---

## 4. The edit

### 4.1 Shape

One pure function, so every case below is a unit test that touches no
filesystem:

```rust
/// Returns the edited document, or `None` when `path` is already present.
fn add_external_dir(text: &str, path: &str) -> Result<Option<String>>
```

`Err` is a refusal to edit (§4.3), not an I/O failure. Nothing in this function
reads the environment.

### 4.2 Cases

The target is the top-level `skills:` mapping and its `external_dirs:` key.
Indentation for an inserted line is derived from the sibling lines already in
the block, not assumed to be two spaces.

| Existing state | Edit |
|---|---|
| No top-level `skills:` key | Append a `skills:` block with `external_dirs:` and one item at the end of the document |
| `skills:` present, no `external_dirs:` key | Insert `external_dirs:` and one item as the first entry of the `skills` block |
| `external_dirs: []` | Replace the empty flow sequence with a block sequence holding one item |
| `external_dirs:` with block items, `path` absent | Insert one item line after the last existing item |
| `external_dirs:` with block items, `path` present | No edit: return `None` |
| `external_dirs:` with no value at all (an explicit null) | Treated as empty: one item is inserted beneath it |

The block that follows a top-level key runs until the next line that is
non-empty, is not a comment, and has zero indentation. Trailing comments after
the last top-level key therefore stay where they are, and an appended `skills:`
block lands after them — harmless, and honest about who wrote it.

Two smaller preservation rules: an inserted item goes after the last *content*
line of an existing sequence, so trailing comments inside the block stay at the
bottom; and whether the document ended in a newline is carried through, so a
file that lacked one does not gain one.

### 4.3 Guards

Hand-rolled YAML editing goes wrong on documents that are more interesting than
the ones it was written for. Each of these refuses the edit rather than
attempting it:

- The file does not parse as YAML, or its root is not a mapping. An empty file
  is the exception: it parses as null, is read as an empty mapping, and gains a
  `skills` block.
- The file contains a tab character. The line scanner's indentation arithmetic
  counts bytes of leading whitespace, which is only sound without them.
- A `---` document separator appears after the first line (multi-document).
- `skills` exists but is not a mapping.
- `skills` exists but is not written as a plain top-level block — a flow mapping
  (`skills: {…}`), a quoted key, an alias. Without this the line scanner would
  fail to find the key and append a *second* `skills:` block, so the check is
  load-bearing rather than fastidious: the parsed document is asked whether the
  key exists, and a mismatch with what the scanner found is a refusal.
- `skills.external_dirs` exists but is not a sequence of strings. An explicit
  null is exempt — it means "none listed", which is a state, not a problem.
- `skills.external_dirs` is a non-empty *flow* sequence (`[a, b]`). Empty
  `[]` is handled in §4.2 because Hermes writes it; rewriting a populated flow
  sequence in place is where this kind of code earns its reputation.

The type guards live in `external_dirs`, which every caller goes through before
editing, so `status` reports the same conditions that `configure` refuses on
rather than reimplementing them.

A refusal is not a failure of the workspace. The message names the condition
and tells the developer to add the two lines by hand.

### 4.4 Verification

The edit is verified before it is written, which is what makes a text edit as
safe as a round-trip. A separate function does it, so it can be tested against
edits the editor would never produce:

```rust
fn verify(original: &str, edited: &str, path: &str) -> Result<()>
```

1. `edited` parses as YAML.
2. Its `skills.external_dirs` is a sequence containing `path`.
3. Reverting only that one key in the parsed `edited` document yields a
   document deep-equal to the parsed `original`.

Check 3 is the load-bearing one: it proves the edit changed exactly one key and
nothing else — not a neighbouring value, not a nesting level, not a key's type.
`add_external_dir` calls `verify` on its own output before returning
`Ok(Some(_))`, so no caller can skip it. Any failure means no write at all.

### 4.5 Writing

- Back up to `config.yaml.bak` in the same directory, overwriting any previous
  backup, before the first byte is written. Only when an edit will actually
  happen.
- Write to `config.yaml.tmp` in the same directory and `rename` it over the
  original, so a process that dies mid-write cannot leave Hermes with a
  truncated configuration.

---

## 5. `zpr-dev agent status`

### 5.1 Output

```text
Hermes
  installed          yes (/home/mathias/.hermes/config.yaml)
  shared skills      configured
  skill source       /home/mathias/src/zpr/zpr-dev-context/skills
  context            ready
```

On a machine where Hermes has never run:

```text
Hermes
  installed          no (/home/mathias/.hermes/config.yaml not found)
  shared skills      not configured
  skill source       /home/mathias/src/zpr/zpr-dev-context/skills
  context            ready
```

### 5.2 Fields

| Field | Values |
|---|---|
| `installed` | `yes (<path>)` when the configuration file exists; `no (<path> not found)` otherwise. The path is always shown, so "no" is never ambiguous about what was looked for. |
| `shared skills` | `configured`; `not configured` when the key is absent or lacks our path; `configured elsewhere (N other director{y,ies})` when the key holds entries but not ours; `unreadable: <reason>` when a §4.3 guard trips or the file cannot be read. |
| `skill source` | The absolute path `configure` would write. `not declared in the manifest` when `agent.hermes.shared_skills` is absent; `missing: <path>` when it is declared but not present. |
| `context` | A rollup of the SPEC-001 §4.6 plan `zpr-dev status` already computes: `ready` when every checked-out repository is `Unchanged`; otherwise the first applicable of
`N file(s) not generated by zpr-dev (run: zpr-dev validate)`, `stale in N repositor{y,ies} (run: zpr-dev sync)`, `N repositor{y,ies} not checked out`. |

`installed` deliberately means "the configuration file exists" rather than
"the binary is on `$PATH`" (§1.3.2). Nothing here reads `$PATH`.

### 5.3 Exit code

Always 0. `agent status` reports; it does not judge. A `$HOME` that cannot be
resolved is still a command error (exit 2), because then there is nothing to
report on.

---

## 6. Implementation structure

One new module, one new command group. No new dependencies: `serde_yaml_ng` is
already present for the manifest and is what §4.3 and §4.4 parse with.

```text
zpr-dev/src/
├── main.rs      + Command::Agent { .. } with Configure { agent } and Status
├── hermes.rs    new: config path, add_external_dir, guards, verification, state
└── commands.rs  + agent_configure, agent_status
```

```rust
// main.rs
enum AgentCommand {
    Configure { agent: AgentName },
    Status,
}

/// A clap `ValueEnum`, so an unknown agent name is rejected by the parser with
/// the list of valid values rather than by a hand-written match.
enum AgentName { Hermes }
```

```rust
// hermes.rs
pub fn config_path(home: &Path) -> PathBuf                             // <home>/.hermes/config.yaml
pub fn add_external_dir(text: &str, path: &str) -> Result<Option<String>>   // §4.1
pub fn external_dirs(text: &str) -> Result<Vec<String>>                // for `status` §5.2
fn verify(original: &str, edited: &str, path: &str) -> Result<()>      // §4.4
```

`hermes.rs` is pure with respect to the environment: `config_path` takes the
home directory rather than reading `$HOME`, and the other two take the document
text. Resolving `$HOME`, reading, backing up, and writing all live at the
`commands.rs` call site, which is also where `--dry-run` is honoured — the same
split SPEC-001 §6.3 uses for `git.rs`.

`Ctx` is unchanged: `commands.rs` resolves `$HOME` itself, because it needs an
unset `$HOME` to be a hard error where `main` needs it to default to empty for
workspace resolution.

`commands.rs` also gained the small write helpers `back_up`, `write_atomically`,
and `print_hunk`, and a `plural_y` helper that replaced three copies of the
inline `repositor{y,ies}` idiom already in that file.

---

## 7. Testing

### 7.1 Unit tests, in `hermes.rs`

Twenty-four tests: one per §4.2 case, one per §4.3 guard, plus:

- Idempotency: a second `add_external_dir` with the same path returns `None`.
- Comment preservation: a fixture with a leading comment, an inline comment
  inside the `skills` block, and a trailing commented-out block round-trips
  with every comment byte-identical.
- Four-space indentation in the `skills` block produces a four-space-indented
  insertion.
- A second path adds an item and keeps the first.
- `verify` called directly on hand-written "edits" that add the path *and*
  change something else — a neighbouring value, a key's type, a nesting level —
  each rejected. This is what proves §4.4 check 3 is live rather than vacuous;
  it cannot be tested through `add_external_dir`, which never produces such an
  edit.

The fixture is a small synthetic Hermes configuration written inline in the
test — a handful of the real file's top-level keys, `_config_version`, and a
trailing comment block. Enough to exercise the shape; not a copy of anyone's
configuration.

### 7.2 Integration tests, in `tests/integration.rs`

The existing harness already sets a per-child environment (SPEC-001 §8.1). Add
`HOME` pointing at a temporary directory to it, so these cases reach a fixture
configuration and can never touch the developer's real one. The fixture
manifest gains an `agent.hermes.shared_skills` entry and the corresponding
directory.

| Case | Asserts |
|---|---|
| `agent configure hermes` on a fixture config | Key set to the absolute skills path; trailing comment block intact; `config.yaml.bak` written |
| Run twice | Second run reports `already configured`, exit 0, file mtime unchanged, no second backup |
| `--dry-run agent configure hermes` | Reports the intended change; no file written, no backup |
| No `$HOME/.hermes/config.yaml` | Exit 2, message names the path and the remedy |
| Manifest without `agent.hermes` | Exit 2, message names the missing key |
| Config with a tab character | Exit 2, file unchanged |
| Shared skills directory declared but absent | Exit 2, file unchanged |
| Config with a populated inline sequence | Exit 2, file unchanged, no backup or temporary file left behind |
| `agent status` before and after `configure` | `shared skills` moves from `not configured` to `configured` |
| `agent status` on a machine with no Hermes config | `installed no`, `skill source not declared in the manifest` |
| Config listing another tool's directory | `configured elsewhere (1 other directory)`; `configure` then adds ours alongside rather than replacing it |
| `agent status` with repositories uncloned, cloned, and synced | `context` moves from `not checked out` through `stale` to `ready` |
| `agent configure nonesuch` | Exit 2 from clap, listing `hermes` |

Twelve integration cases in total.

---

## 8. SPEC-001 text this supersedes

Three edits to `spec-001-zpr-dev.md` are part of this work, so the two
documents do not contradict each other:

1. **§1.2**, the `agent configure <agent>` deferral row: replaced with a
   pointer to this document.
2. **§1.4.1**, "`agent configure` is deferred entirely": likewise.
3. **§11**, the safety invariant "modify agent configuration not written by
   `zpr-dev`": narrowed. `zpr-dev` may modify exactly one key,
   `skills.external_dirs`, in exactly one file,
   `$HOME/.hermes/config.yaml`, only under an explicit `agent configure`
   invocation, only through the verified edit of §4.4, and only after backing
   the file up. Every other key in that file, and every other agent
   configuration file, remains untouchable.

§1.1's command list gains `agent configure` and `agent status`.

---

## PLAN

Seven steps. Each is independently committable and leaves the tree green
(`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`). Steps 1–3
are pure and testable with no new command surface, which is why they come
first: the risky part of this feature is the YAML edit, and it is fully
verified before anything can invoke it.

### Step 1 — `hermes.rs`: config path and reading `external_dirs`

Create the module with `config_path` and `external_dirs` (§6). `external_dirs`
parses with `serde_yaml_ng` and applies the §4.3 guards that are questions
about types — root is a mapping, `skills` is a mapping, `external_dirs` is a
sequence of strings — returning the current list.

Unit tests: absent `skills`, absent `external_dirs`, `[]`, block sequence with
two items, and one rejection per type guard.

Done when: `external_dirs` reports the current state of every fixture shape in
§4.2 without editing anything.

### Step 2 — `add_external_dir`: the five edit cases

Implement §4.2 on top of a small line scanner: locate the top-level `skills:`
block, find its extent by the zero-indentation rule, locate `external_dirs:`
within it, derive the indentation from its siblings.

Add the two textual guards (§4.3): tab character, and a `---` separator after
the first line.

Unit tests: one per §4.2 row, one per textual guard, the flow-sequence
refusal, idempotency, and the four-space-indent case.

Done when: every §4.2 case produces the expected text and every guard refuses.

### Step 3 — verification

Implement §4.4 inside `add_external_dir`, so no caller can skip it: parse the
edited text, confirm the path is present, revert that one key, and compare
deep-equal against the parsed original.

Unit tests: the comment-preservation fixture; a test that injects a corrupted
edit and asserts verification rejects it.

Done when: the edit cannot return `Ok(Some(_))` for a document that differs
from the original by anything other than that key.

### Step 4 — CLI surface

Add `Command::Agent`, `AgentCommand`, and `AgentName` to `main.rs` (§6), with
both variants dispatching to `commands::agent_configure` and
`commands::agent_status`. Stub bodies that return exit 0 and print nothing.

The existing `cli_definition_is_valid` test covers the clap wiring. Add an
integration case asserting `agent configure nonesuch` exits 2 and names
`hermes`.

Done when: both commands parse and dispatch; `--help` reads correctly at all
three levels.

### Step 5 — `agent configure hermes`

Implement §3 in `commands.rs`: resolve the skills directory, resolve `$HOME`,
read, edit, verify, back up, write atomically (§4.5), report. Honour
`--dry-run`, `--quiet`, and `--verbose` through the existing `report` helper.

Every §3.3 error and its exact message.

Integration tests: the first six rows of §7.2.

Done when: a fixture configuration is correctly edited, a second run is a
no-op, and every error path leaves the file byte-identical.

### Step 6 — `agent status`

Implement §5. The `context` field reuses `generate::plan` and the same
`Action` rollup `status` already uses (SPEC-001 §4.6), rather than a second
notion of staleness.

Integration test: the `agent status` before/after case in §7.2.

Done when: all four fields report correctly for a configured machine, an
unconfigured one, and one with no Hermes configuration at all.

### Step 7 — documentation

- The three SPEC-001 edits of §8.
- `README.md`: `agent configure hermes` and `agent status` in the command
  table, and a note in the bootstrap sequence that Hermes users run
  `zpr-dev agent configure hermes` once after `setup`.
- Mark this document `Status: implemented`, and fold any deviation this plan
  produced back into the sections above — the same convention SPEC-001
  followed, so the spec describes the tool as built.

Done when: no document claims `agent configure` is deferred, and SPEC-001 §11
describes the invariant the code actually holds.
