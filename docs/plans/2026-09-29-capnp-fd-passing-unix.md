# FEATURE: Cap'n Proto FD passing on unix (reinstate the FD-passing fork, retire `capture.sock`)

**Status:** IN FLIGHT — umbrella [zipline#140](https://github.com/mkolehmainen/zipline/issues/140). **Gated on the Windows umbrella [zipline#126](https://github.com/mkolehmainen/zipline/issues/126):** #140 and its first open task #142 are both `blockedBy` #126, so nothing here is pickable until the Windows port is finished. F1 (#141, the fork itself) is done. F2 is zipline#135, a child of #126, retargeted to 0.25 → 0.26 on 2026-09-29.
**Date:** 2026-09-29
**Repo state this plan was written against:** `zl-zpr-core` @ `6085db1`,
`zl-zpr-dev-context` @ `a970318`, both on `zipline`.
**Fork pin:** `mkolehmainen/capnproto-rust` branch `zipline` @
**`1e1d5ad692b4b2c2b942f04f32550df6445f4216`**. That is emilazy's
`push-xstmntksusmk` @ `cb619b2e`, plus the 8 code commits listed in F1
(ending at `c9a2764d`), plus two docs-only commits adding and correcting `ZIPLINE.md` (`b88adfb8`, `1e1d5ad6`).
**Assumes landed first:** zipline#134 (fork dropped, `capnp-ancillary`
deleted; merged as zl-zpr-core#49), zipline#135 (lockstep capnp 0.25 → **0.26**,
which is F2), and the rest of #126.
**Related upstream:** [org-zpr/zpr-core#1399](https://github.com/org-zpr/zpr-core/issues/1399)
(remove `capture.sock`), [capnproto/capnproto-rust#666](https://github.com/capnproto/capnproto-rust/pull/666)
(FD passing, open, no reviews as of 2026-09-29).

---

**Goal.** On unix, `ph-cli capture set-file` hands the capture file descriptor to
`ph` over the existing Cap'n Proto admin RPC (`setCaptureFile`), the way
org-zpr mainline does it. The stopgap `capture.sock` interface is deleted, which
is what org-zpr#1399 asks for. On Windows, the same code builds and
`setCaptureFile` returns the Unsupported error from C4. Mainline-capnp builds
stay possible.

## Background and findings (2026-09-29)

1. **Why the fork was dropped (zipline#134).** `zl-zpr-core` patched `capnp`,
   `capnp-futures`, `capnp-rpc` and `capnpc` to `emilazy/capnproto-rust@cfbcb9b`
   through `[patch.crates-io]`. That fork failed in two ways:
   - Its `capnp` crate does not compile for Windows. The non-unix stubs in
     `capnp/src/fd/compat.rs` are `pub enum BorrowedFd<'_> {}` (E0637), and
     `match self {}` on a reference to an empty enum (E0004 ×2).
   - `capnp-futures/tokio-unix-fd-stream` does not compile on Linux either. It
     depends on `rustix` with only `["std", "net"]`. `rustix` 1.1.5's
     `linux_raw/net/sockopt.rs` uses `crate::timespec`, which only exists when
     a time-ish feature is on, so it fails with E0433 cannot find `timespec` ×6.
2. **emilazy's latest head `cb619b2e` has the same two breaks, and more.**
   `cb619b2e` is the head of PR #666: upstream tag `capnp-v0.26.2` (released
   2026-07-05) plus 13 commits. Beyond finding 1:
   - `capnp` no longer builds with `alloc` off
     (`--no-default-features`, and `--no-default-features --features std`), on
     any target. The cause is that `FdHooks` uses `Box<dyn ClientHook>`, but
     `private::capability` is `#![cfg(feature = "alloc")]`. Upstream CI builds
     this combination; ZPR doesn't use it.
   - The `capnp-futures` lib tests don't compile (E0599). Upstream's
     `eof_mid_passthrough_run` test (`de1391c0`, which is in the 0.26.2 base)
     calls `read_to_end`, and the fork moved `PackedRead` onto its own
     `AsyncFdRead` trait, which doesn't have that method.
   - It is missing everything upstream released after 0.26.2. That includes a
     **peer-triggered panic** in `capnp-rpc` (upstream `1ed0d0e5`, #672). A
     remote vat that makes us import the same capability twice, and then
     makes us re-send it, reaches `unreachable!()` in `write_descriptor`. `ph`
     builds with `panic = "abort"`, so that kills the process. Verified: the
     upstream regression test fails on the fork with
     `entered unreachable code` at `capnp-rpc/src/rpc.rs:1461`.
     **org-zpr's pin `cfbcb9b` predates this fix too.**
   All of these are fixed on our fork (F1).
3. **The fixes are small.** The Windows and rustix fixes are the ones in F1
   commits 1–2. With all F1 commits applied, every check listed under F1
   Acceptance passes on Linux and `x86_64-pc-windows-msvc`. The
   `UnixFdStream` code was already `cfg(unix)`-gated in the fork.
4. **Our old pin `cfbcb9b` is orphaned.** The fork was force-pushed, and no
   branch contains that commit any more. GitHub still serves it by SHA, but it
   can be garbage-collected. So we pin to our own fork, never to emilazy's
   branch.
5. **Cargo cannot scope a patch to unix or to a feature.** `[patch]` applies to
   the whole workspace, on every target. A unix-only
   `[target.'cfg(unix)'.dependencies]` entry on a renamed fork package would
   pull in a *second* `capnp` crate. Its types (`capnp::Error`, readers,
   builders, `ClientHook`) would not match the ones `zpr`'s generated code and
   `capnp-rpc` use. So the only workable shape is: **the fork builds on every
   target, the patch is unconditional, and the FD-passing *use* is gated to
   unix.**
6. **The fork is 0.26, and the capnp version is lockstep across four repos.**
   `zl-zpr-common` publicly exports the generated `policy_capnp`/`vs_capnp`
   modules (see zipline#135). Also, a `[patch]` only replaces requirements it
   is semver-compatible with: a 0.26.2 fork does not replace a `^0.27`
   requirement. It would be ignored with a "patch was not used" warning, and
   core would silently build against crates.io 0.27. So **`zl-zpr-common`,
   `zl-zpr-compiler`, `zl-zpr-visaservice` and `zl-zpr-core` must all be on
   0.26**. #135 was first written to go 0.25 → 0.27. That would have meant a
   second lockstep round straight after it, to come back down to 0.26. So on
   2026-09-29 #135 was retargeted to 0.25 → 0.26, and F2 is that issue. See D1
   for the alternative, and D2 for what staying on 0.26 costs.
7. **A trial rebase of the fork onto upstream 0.27.2 (`81bc1b81`) conflicts.**
   The conflict is at commit 2 of 13 (`06035ac6` "Split up `futures`
   dependency in `capnp-rpc`", `capnp-rpc/Cargo.toml`). It was not pursued
   further, so the full cost of a 0.27 port is unknown.
8. **Warnings that were already there (not ours, left alone).**
   - Clippy: `manual checked division` in `capnp/src/primitive_list.rs`
     ×2, and `iterate on a map's values` in `capnp-rpc/src/rpc.rs:512,534`.
   - rustc: `unused variable: fmt` in `capnp/src/lib.rs:431` with
     `--no-default-features` (upstream 0.26.2 has it too).
   - rustc: `value assigned to set_cloexec is never read` in
     `capnp-futures/src/io/tokio/unix_fd_stream.rs:134`. This one is harmless.
     On Linux, received FDs get close-on-exec atomically through
     `RecvFlags::CMSG_CLOEXEC`; the `fcntl` fallback is only for macOS and the
     other platforms without it.
   None of these is in the lines F1 changed. They don't affect core, because
   core's `-D warnings` doesn't reach dependency crates.
9. **Adding the patch to an existing lockfile doesn't switch to the fork**
   (raised by Codex on dev-context#42; tested 2026-09-29 with a scratch crate
   against fork `b88adfb8`). If `Cargo.lock` already has crates.io
   `capnp-rpc` 0.26.3, which #135 will leave in core, then adding
   `[patch.crates-io]` keeps 0.26.3. Cargo only warns
   `patch capnp-rpc v0.26.1 (…) was not used in the crate graph`, and core's
   `-D warnings` doesn't catch that, because it's a Cargo warning, not a
   rustc one. The fork's `capnp`/`capnp-futures` are picked up, but
   `capnp-rpc` isn't, so the fork-only RPC API is missing.
   `cargo update -p capnp-rpc`, or resolving a fresh lockfile, does select the
   fork: Cargo prefers a matching patch even when its version is lower. So
   the fix is a required `cargo update` in F3. Raising the fork's crate
   versions isn't needed, and would break the fork's no-bump rule.
10. **A mainline build must drop the feature's declaration, not just leave
    the feature off** (raised by Codex on dev-context#42; tested 2026-09-29 with a
    scratch crate). crates.io `capnp-futures` 0.26/0.27 declares no features at
    all. A manifest whose `capnp-ancillary` feature lists
    `capnp-futures/tokio-unix-fd-stream` fails dependency resolution against
    mainline (`capnp-futures does not have that feature`). That happens with
    default features, with `--no-default-features`, and for the Windows target
    alike, because Cargo checks every declared feature. With the fork patch in
    place, the feature exists on every target, so normal builds, Windows
    included, are unaffected. D4 lists the edits a mainline build needs.

## Decisions

- **D1 — Fork base: stay on 0.26 (operator direction, 2026-09-29).** Use
  emilazy's fork (0.26.2 base) plus our F1 commits. The four repos go from
  0.25 to 0.26 in one round (#135, retargeted). *Alternative, not chosen:*
  rebase the 13 commits onto 0.27.2, and have #135 go to 0.27. That is more
  work up front (finding 7) and a larger divergence from #666.
- **D2 — Security cost of 0.26 instead of 0.27: RESOLVED (F1 audit).**
  - **Core, which is on the fork:** nothing lost. Every security-relevant
    upstream fix from 0.26.2 through 0.27.2 is either already in the fork's
    base or cherry-picked onto the fork (full table under F1). The upstream
    changes not taken are a performance fix, an RPC shutdown-ordering change,
    and the 0.27 API/schema-equality work. None is a security fix.
  - **The other three repos, on crates.io 0.26.x:** they get everything up to
    `capnp` 0.26.2 and `capnp-rpc` 0.26.3. That includes both RPC panic
    fixes. They miss only the two 0.27-only defence-in-depth fixes (F1
    commits 5 and 6: bool `PrimitiveElement` bounds, `get_data_field`
    overflow). Both guard internal APIs that generated code doesn't call with
    attacker-controlled values.
  - If upstream ships a security fix later, follow the fork's `ZIPLINE.md`
    (see "Maintaining the fork" below).
- **D3 — Where the fork lives.** It lives at `mkolehmainen/capnproto-rust`,
  branch `zipline` (the convention for our forks, spec-003). Consumers pin it
  by `rev`, never by branch. Gate 1 already groups rev-pinned git deps by
  `(crate, url)`.
- **D4 — Feature shape.** Bring `capnp-ancillary` back as a `ph`/`ph-cli`
  feature, on by default. It pulls `capnp-futures/tokio-unix-fd-stream` only
  from `[target.'cfg(unix)'.dependencies]`. Code is gated on
  `cfg(all(unix, feature = "capnp-ancillary"))`. Everywhere else,
  `setCaptureFile` returns `capture_unsupported()`. Builds without the
  feature use no fork-only API.

  **Building against mainline** (org-zpr#1399 requirement 2) takes two manifest
  edits, not one (finding 10):
  1. Delete the `[patch.crates-io]` block.
  2. In `adapter/ph/Cargo.toml` and `adapter/cli/Cargo.toml`, delete the
     `capnp-ancillary` feature (and remove it from `default`), and the
     optional `capnp-futures` dependency it enables.

  Turning the feature off with `--no-default-features` isn't enough: Cargo
  rejects the manifest as long as any feature names
  `capnp-futures/tokio-unix-fd-stream`. The result builds and runs, and
  `setCaptureFile` returns Unsupported.
- **D5 — `capture.sock` is deleted, not kept as a fallback.** This follows the
  org-zpr#1399 plan: `set_capture_file_worker.rs`, `capture_path`,
  `--capture-path`, ph-cli `-c/--cap-socket`, the `*_CAP_SOCK` variables in the
  integration tests, and the legacy `set_capture_file(asm, ancillary)` helper
  all go.
- **D6 — Upstreaming (TO DECIDE).** The fixes could go to three places:
  - emilazy on PR #666: F1 commits 1–4, the FD-work fixes.
  - org-zpr: they pin `cfbcb9b`, so they have the Windows break, the
    orphaned-SHA risk, and the peer-triggered panic from finding 2. They may
    want to switch to our fork or cherry-pick commit 7.
  - Upstream capnproto-rust: nothing to send. Commits 5–8 came from there.

  This is outward-facing, so it waits for the operator's go.

## Operator: create the fork — DONE 2026-09-29

`mkolehmainen/capnproto-rust` exists, and its parent is
`emilazy/capnproto-rust`. It was created with:
```sh
gh repo fork emilazy/capnproto-rust --clone=false --default-branch-only=false
git checkout -b zipline cb619b2e && git push -u origin zipline
```
The fork also carries copies of emilazy's `push-xstmntksusmk` (`cb619b2e`)
and `master` (`f605b5d2`). Forks don't sync automatically, so those copies
keep their SHAs whatever emilazy does later.

## Maintaining the fork

The maintenance rules live on the fork itself, in
[`ZIPLINE.md`](https://github.com/mkolehmainen/capnproto-rust/blob/zipline/ZIPLINE.md)
on branch `zipline`. They outlive this plan, so they are not repeated here.
That file covers:
- why the fork exists and who pins it;
- the commits it carries on top of emilazy, one row per commit;
- the rules: fast-forward only, no version bumps, cherry-pick upstream fixes
  together with their tests;
- the remotes to add, and the `libcapnp-dev` prerequisite;
- the verification to run before every push, which is F1's set;
- when to retire the fork.

The operator's checkout is `~/src/capnproto-rust`. While this plan is in
flight, also record any new fork SHA in this plan's header.

## Dependency graph and order

```
F1 fork branch: Windows fix, test fixes, security backports   (mkolehmainen/capnproto-rust)  DONE
F2 = #135 lockstep capnp 0.25 -> 0.26                          (common, compiler, visaservice, core)
#126 Windows umbrella (C3, C4, ... Z1)
        F1 ─┐
        F2 ─┼─► F3 core: fork patch, unix-only capnp-ancillary, delete capture.sock
      #126 ─┘        │
                     ▼
              F4 docs, build set, netns at tip, retire this plan
```

F3 waits for #126, even though the fork now builds on Windows. C3/C4 are
changing the same `ph`/`ph-cli` capture paths, and F3's Windows acceptance
needs #131's Windows build.

## Issue map

| Task | Issue | Repo | Blocked by |
|---|---|---|---|
| Umbrella | [zipline#140](https://github.com/mkolehmainen/zipline/issues/140) | zipline | #126 |
| F1 | [zipline#141](https://github.com/mkolehmainen/zipline/issues/141) (closed, record) | capnproto-rust | — |
| F2 | [zipline#135](https://github.com/mkolehmainen/zipline/issues/135) (child of #126) | common, compiler, visaservice, core | #134 |
| F3 | [zipline#142](https://github.com/mkolehmainen/zipline/issues/142) | zl-zpr-core | #126, #135 |
| F4 | [zipline#143](https://github.com/mkolehmainen/zipline/issues/143) | zl-zpr-dev-context | #142 |

Per `skills/zpr/SKILL.md`, the native `blockedBy` lists and the umbrella's
sub-issue order are the source of truth, and this table is derived from them.
The umbrella's own blocker doesn't stop its children from being picked, so
#142 carries #126 itself.

## Tasks (one issue each, in order)

### F1 — Fork branch: Windows fix, test fixes, security backports — DONE 2026-09-29 — zipline#141 (`mkolehmainen/capnproto-rust`)

`zipline` went from `cb619b2e` to `c9a2764d` by fast-forward push. Crate
versions are unchanged: `capnp` 0.26.2, `capnp-rpc` 0.26.1, `capnp-futures`
0.26.1, `capnpc` 0.26.0.

| # | Commit | What | Why |
|---|---|---|---|
| 1 | `5b027f86` | `capnp/src/fd/compat.rs`: `BorrowedFd<'a>`/`OwnedFd` become structs holding a private `Infallible` (plus `PhantomData<&'a ()>`); `match self.0 {}` in both `AsFd` impls. A comment explains the shape. | Windows build: E0637, E0004 ×2. The types are still impossible to construct. |
| 2 | `cf1bf04e` | `capnp-futures/Cargo.toml`: rustix features `["std", "net", "time"]`, with a comment. | Linux `tokio-unix-fd-stream` build: rustix 1.1.5 E0433 ×6. |
| 3 | `ebe2377d` | `capnp-futures/src/io/serialize_packed.rs`: `eof_mid_passthrough_run` reads in a loop with `AsyncFdReadExt::read`. | The lib tests didn't compile (E0599). Verified that the test still fails if the pass-through EOF fix is reverted. |
| 4 | `9bc9d826` | `capnp/src/fd/{compat,unix}.rs`: `FdHooks` and the imports only it uses are gated on `feature = "alloc"`. | `--no-default-features` (and `--features std`) failed: E0432, E0425 ×2. |
| 5 | `d66f6c62` | cherry-pick upstream `9eb4b9a3` | Bounds checking in `impl PrimitiveElement for bool` (0.27.0). Defence in depth. |
| 6 | `24019c0b` | cherry-pick upstream `081b742d` (#683) | Integer overflow in the `StructReader::get_data_field` bounds check (0.27.2). |
| 7 | `b293800e` | cherry-pick upstream `1ed0d0e5` (#672), conflict resolved | **Peer-triggered panic** on re-sending a twice-imported capability (capnp-rpc 0.26.2). The resolution keeps the fork's `write_descriptor(descriptor, fd_hooks)`, and the test is ported to the fork's `#[tokio::test]` harness. Verified: the test fails with `unreachable!()` without the fix. |
| 8 | `c9a2764d` | cherry-pick upstream `f7269c6e` | Regression test for 6 (`capnp/tests/get_data_field_overflow.rs`). |

**D2 audit: the security fixes #135 listed, and everything upstream shipped
from 0.26.2 to 0.27.2.**

| Fix | Where it landed upstream | In our fork |
|---|---|---|
| UB with `pointer::add()` | capnp 0.25.6 | yes (base) |
| Zeroing missed 7 bytes | capnp 0.26.1 | yes (base) |
| Panic in `BufferSegments::new()` | capnp 0.26.2 | yes (base) |
| Desync in `read_message_no_alloc()` | capnp 0.26.2 | yes (base) |
| Packed-stream EOF/decoding bugs (`de1391c0`) | in the 0.26.2 tag | yes (base; test fixed by commit 3) |
| Panic on Return with a bad question ID | capnp-rpc 0.26.1 | yes (base) |
| Panic re-sending a twice-imported capability | capnp-rpc 0.26.2 | yes (commit 7) |
| Bool `PrimitiveElement` bounds check | capnp 0.27.0 | yes (commit 5) |
| `get_data_field` bounds-check overflow | capnp 0.27.2 | yes (commits 6, 8) |
| `ScratchSpaceHeapAllocator` off-by-one (heap used when not needed) | capnp 0.27.1 | no: performance only |
| `Disconnector` waits for `shutdown()` | capnp-rpc 0.27.0 | no: behaviour change, not security |
| `new_broken_cap()`, schema `Eq`/`Hash`, `pub(crate)` cleanups, `loose_equals` deprecation | 0.26.3 / 0.27.0 | no: API work tied to 0.27 |

**Verification (run 2026-09-29 at `c9a2764d`, Linux x86_64, with the
`libcapnp-dev` standard schemas available):**
- `cargo fmt --all -- --check`: clean.
- `cargo test --workspace`: 211 passed, 0 failed.
- `cargo test -p capnp-futures -p capnp-rpc -p capnp-rpc-test -p capnp-futures-test --features capnp-futures/tokio-unix-fd-stream`:
  53 passed, 0 failed, including `pass_fd`.
- `cargo test`, run in `capnp/`, with each of these feature sets:

  | Features | Passed | Failed |
  |---|---|---|
  | `--no-default-features` | 30 | 0 |
  | `--no-default-features --features alloc` | 78 | 0 |
  | `--no-default-features --features std` | 30 | 0 |
  | `--features sync_reader` | 78 | 0 |
  | `--features unaligned` | 78 | 0 |
- `cargo check --target x86_64-pc-windows-msvc -p capnp -p capnp-futures -p capnp-rpc -p capnpc`
  passes in three variants: plain, with
  `--features capnp-futures/tokio-unix-fd-stream`, and with `--tests`.
  It also passes for `-p capnp` with `--no-default-features`,
  `--no-default-features --features alloc`, and
  `--no-default-features --features std`.
- Before-state, at `cb619b2e`:
  - The Windows check failed with E0637 and E0004 ×2 in `compat.rs`.
  - The Linux `tokio-unix-fd-stream` check failed with E0433 ×6 in rustix.
  - The regression test from commit 7 panicked.
- Clippy: only the warnings that were already there (finding 8).

### F2 — Lockstep capnp 0.25 → 0.26 = zipline#135, IN PROGRESS (`zl-zpr-common`, `zl-zpr-compiler`, `zl-zpr-visaservice`, `zl-zpr-core`)

ZprBot is doing this as zipline#135, which the operator retargeted from 0.27 to
0.26 on 2026-09-29. It runs before #126 finishes: it doesn't touch the
Windows work, and it keeps F3 from needing a version change. When F3 starts,
check that #135's PR met the points below; if it missed one, fix it in F3.

- `zl-zpr-common`: `capnp`/`capnpc` → `"0.26"`. Tag a new `zpr` version.
- Compiler, visa service, core: `capnp`/`capnp-rpc`/`capnpc` → `"0.26"`, and
  bump the `zpr` tag **in the same round** (gate 1).
- **Use requirement `"0.26"`, not `"0.26.3"`.** crates.io has `capnp-rpc`
  0.26.3, but the fork is 0.26.1. A `>= 0.26.3` requirement in core would make
  F3's patch unusable no matter what. `"0.26"` still admits 0.26.3, so F3
  must also re-resolve the lockfile (finding 9).
- 0.26.0 changed the `Allocator` methods to take `NonNull`. #135's grep found
  no custom `Allocator` impls, so expect version bumps and regenerated code
  only.
- Lockfiles should resolve `capnp` 0.26.2 and `capnp-rpc` ≥ 0.26.2. 0.26.2
  has the RPC re-import panic fix (the same fix as F1 commit 7). This matters
  most in the visa service, which faces peers over RPC.
- Core stays on crates.io until F3, which adds the fork patch.

**Acceptance.**
- The unit gate is green in all four repos.
- `cargo tree -d -i capnp` shows no duplicate `capnp` in any repo.
- `zpr-dev build` gate 1 is clean.
- The netns tier is green.

### F3 — zipline#142: Reinstate the fork, unix-only `capnp-ancillary`, delete `capture.sock` (`zl-zpr-core`)

- Root `Cargo.toml`: add `[patch.crates-io]` for `capnp`, `capnp-futures`,
  `capnp-rpc` and `capnpc` →
  `git = "https://github.com/mkolehmainen/capnproto-rust.git", rev = "1e1d5ad692b4b2c2b942f04f32550df6445f4216"`
  (or the latest SHA recorded in this plan's header). Add the
  `capnp-futures = "0.26"` workspace dependency. Put a comment on the patch
  block that says:
  - why the patch exists,
  - that it must build on every target,
  - that a mainline build without FD capture also needs the
    `capnp-ancillary` feature and its `capnp-futures` dependency removed (D4);
  - that after changing it you must run the `cargo update` below.
- **Re-resolve the lockfile onto the fork (finding 9):**
  `cargo update -p capnp -p capnp-futures -p capnp-rpc -p capnpc`. Without it,
  the `capnp-rpc` 0.26.3 locked by #135 stays, and the patch is ignored
  apart from a Cargo warning.
- `adapter/ph/Cargo.toml`, `adapter/cli/Cargo.toml`:
  - Move `capnp-futures` (optional) under `[target.'cfg(unix)'.dependencies]`.
  - Declare `capnp-ancillary = ["dep:capnp-futures", "capnp-futures/tokio-unix-fd-stream"]`.
  - Add it to `default`.
  - On Windows the feature must be a no-op. Check that
    `cargo check --target x86_64-pc-windows-msvc -p ph -p ph-cli` works with
    default features. If Cargo rejects the target-gated `dep:` activation,
    gate in code only and say so in the PR.
- Restore the ancillary code that #134 removed, from the parent of the #134
  merge commit. Then re-gate it:
  - `admin_worker.rs`: `launch_capnp` uses `VatNetwork::new_with_fds` over
    `UnixFdStream` under `cfg(all(unix, feature = "capnp-ancillary"))`, and
    `byte_stream_network` otherwise. `set_capture_file` uses `get_fd` in that
    configuration. In any other configuration it returns `capture_unsupported()`.
  - ph-cli `main.rs`: `CaptureFileImpl`, `set_capture_file_task`, and the
    ancillary transport go under the same gate.
- Delete `capture.sock` per D5, across:
  - ph `main.rs`, `lib.rs`, `config.rs`, `main_args.rs`, `main_argparse.rs`,
    `socket_access.rs`, `set_capture_file_worker.rs`;
  - admin-api `socket_owner.rs`, `lib.rs` (and `rpc_commands.rs` if it is
    unused by then);
  - ph-cli `main_args.rs`, `main.rs`, `README`;
  - integration tests `capture-test.sh`, `one-node-test.sh`,
    `a2a-pubkey-test.sh`, and the `one-node-*oidc*`, `oidc-file-interplay`,
    `attr-query` and `policy-reauth` tests, which carry `*_CAP_SOCK` today.
- Unit tests:
  - An FD sent over a `UnixFdStream` `VatNetwork` arrives as a usable file
    (write, then read back).
  - `setCaptureFile` without an FD, or on a non-ancillary build, returns the
    Unsupported error.

**Acceptance.**
- Every `capnp`, `capnp-futures`, `capnp-rpc` and `capnpc` entry in
  `Cargo.lock` has `source = "git+https://github.com/mkolehmainen/capnproto-rust.git?rev=…"`.
  None of them is a registry entry, and there is no `[[patch.unused]]`
  section.
- `cargo check` prints no `was not used in the crate graph` warning.
- `grep emilazy Cargo.lock` finds nothing.
- `cargo fmt --check`, the `-D warnings` build (workspace, plus
  `libnode2 --all-features`) and `cargo test` are green.
- `cargo build -p ph --no-default-features --features io-uring,rcu-crossbeam-epoch`
  and `cargo build -p ph-cli --no-default-features --features pcap` are green:
  the configuration without the feature.
- **An actual mainline build, done once and not committed.** In a scratch
  worktree, make the two D4 edits and run
  `cargo update -p capnp -p capnp-futures -p capnp-rpc -p capnpc`. Then:
  - `cargo build -p ph -p ph-cli` and `cargo test -p ph -p ph-cli` are green;
  - `Cargo.lock` has only registry `capnp*` entries.

  Quote it in the PR. This is what proves org-zpr#1399 requirement 2.
- Windows: the #126 Windows build and test steps are still green for `ph` and
  `ph-cli`, and `capture set-file` still reports Unsupported there.
- The netns tier is green, including `capture-test.sh`, which now runs over the
  RPC FD path with no `capture.sock` on disk. Quote it in the PR.

### F4 — zipline#143: docs, build set, netns at tip, plan retirement (`zl-zpr-dev-context`)

This is the closure child. It runs the integration re-run and retires this plan
(`skills/zpr/SKILL.md` "Plan retirement").

- `docs/BUILD.md` "Cross-repository dependencies":
  - Replace #134's "mainline everywhere" note with: core patches capnp to
    `mkolehmainen/capnproto-rust@<rev>` for unix FD passing, and the fork
    must build on every target.
  - Record that the capnp minor is lockstep across the four repos, with the
    fork's minor setting it.
  - Link the fork's `ZIPLINE.md` for maintenance and `rev` bumps.
- `zpr-dev/docs/specs/spec-003-build.md` §4.1: the rev-pinned example names
  our fork.
- Cut a build set.
- Re-run the netns tier at tip after #142 merged. Quote it on #140.
- Ask the operator for D6 (upstreaming to emilazy and/or org-zpr). Record the
  answer on #140.
- Retire this plan:
  - Move D1–D6, the F1 audit summary and the orphaned-SHA finding into
    `docs/BUILD.md` `## Design decisions`, with a pointer to `ZIPLINE.md`.
    The maintenance rules stay in `ZIPLINE.md`, not in the spec.
  - Delete this file.
  - Repoint the `AGENTS.md` row.
  - Add a row to `docs/plans/README.md`.

**Acceptance.**
- The netns-at-tip result is on #140.
- The build set is cut.
- `git grep 2026-09-29-capnp-fd-passing-unix` is empty across the workspace.

## Exit / drop-the-fork criteria

When capnproto/capnproto-rust#666 (or a successor) is released on crates.io,
the follow-up is:
1. Delete `[patch.crates-io]`.
2. Move all four repos to that release in one round.
3. Keep `capnp-ancillary` as is.
4. Archive `mkolehmainen/capnproto-rust`.

Before archiving, check that the release builds for Windows, and that it
builds and tests with `alloc` off. Those are the breaks F1 commits 1 and 4
fixed; if #666 lands without them, send them upstream first (D6).
