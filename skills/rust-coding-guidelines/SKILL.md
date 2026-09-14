---
name: rust-coding-guidelines
description: Use when writing or reviewing Rust in a ZPR repository — naming, data types, strings, error handling, memory, concurrency, async, macros, and the clippy lints that map to each. Keywords: naming convention, rustfmt, clippy, lint, code style, code review, best practice, deprecated crate.
version: 1.0.0
license: MIT
metadata:
  tags: [rust, style, clippy, review]
  upstream: https://github.com/actionbook/rust-skills
  upstream_skill: skills/coding-guidelines
  upstream_commit: 5c40d3ad785193231b7d0dbfb8e1eb447e5edd94
  source: https://rust-coding-guidelines.github.io/rust-coding-guidelines-zh/
---

# Rust Coding Guidelines (Core Rules)

## Provenance

Vendored from the `rust-skills` plugin (`actionbook/rust-skills`, MIT), skill
`coding-guidelines`, commit `5c40d3a` (2026-08-23), condensed from the Rust
Coding Guidelines project. It lives here rather than being consumed as a plugin
because Hermes has no plugin mechanism — it reads this shared skills directory
directly. Claude Code users who install `rust-skills@rust-skills` will see the
upstream copy as well; the two say the same thing.

To refresh, diff this file against upstream `skills/coding-guidelines/SKILL.md`
and bump `upstream_commit`.

**The ZPR rules in `AGENTS.md` win where they disagree** — notably, ZPR is a
reference implementation, so readability beats succinctness and nearly every
function carries a comment.

## Naming (Rust-Specific)

| Rule | Guideline |
|------|-----------|
| No `get_` prefix | `fn name()` not `fn get_name()` |
| Iterator convention | `iter()` / `iter_mut()` / `into_iter()` |
| Conversion naming | `as_` (cheap &), `to_` (expensive), `into_` (ownership) |
| Static var prefix | `G_CONFIG` for `static`, no prefix for `const` |

## Data Types

| Rule | Guideline |
|------|-----------|
| Use newtypes | `struct Email(String)` for domain semantics |
| Prefer slice patterns | `if let [first, .., last] = slice` |
| Pre-allocate | `Vec::with_capacity()`, `String::with_capacity()` |
| Avoid Vec abuse | Use arrays for fixed sizes |

## Strings

| Rule | Guideline |
|------|-----------|
| Prefer bytes | `s.bytes()` over `s.chars()` when ASCII |
| Use `Cow<str>` | When might modify borrowed data |
| Use `format!` | Over string concatenation with `+` |
| Avoid nested iteration | `contains()` on string is O(n*m) |

## Error Handling

| Rule | Guideline |
|------|-----------|
| Use `?` propagation | Not `try!()` macro |
| `expect()` over `unwrap()` | When value guaranteed |
| Assertions for invariants | `assert!` at function entry |

## Memory

| Rule | Guideline |
|------|-----------|
| Meaningful lifetimes | `'src`, `'ctx` not just `'a` |
| `try_borrow()` for RefCell | Avoid panic |
| Shadowing for transformation | `let x = x.parse()?` |

## Concurrency

| Rule | Guideline |
|------|-----------|
| Identify lock ordering | Prevent deadlocks |
| Atomics for primitives | Not Mutex for bool/usize |
| Choose memory order carefully | Relaxed/Acquire/Release/SeqCst |

## Async

| Rule | Guideline |
|------|-----------|
| Sync for CPU-bound | Async is for I/O |
| Don't hold locks across await | Use scoped guards |

## Macros

| Rule | Guideline |
|------|-----------|
| Avoid unless necessary | Prefer functions/generics |
| Follow Rust syntax | Macro input should look like Rust |

## Deprecated → Better

| Deprecated | Better | Since |
|------------|--------|-------|
| `lazy_static!` | `std::sync::OnceLock` | 1.70 |
| `once_cell::Lazy` | `std::sync::LazyLock` | 1.80 |
| `std::sync::mpsc` | `crossbeam::channel` | - |
| `std::sync::Mutex` | `parking_lot::Mutex` | - |
| `failure`/`error-chain` | `thiserror`/`anyhow` | - |
| `try!()` | `?` operator | 2018 |

## Clippy Lint → Rule Mapping

| Clippy Lint | Category | Fix |
|-------------|----------|-----|
| `unwrap_used` | Error | Use `?` or `expect()` |
| `needless_clone` | Perf | Use reference |
| `await_holding_lock` | Async | Scope guard before await |
| `linkedlist` | Perf | Use Vec/VecDeque |
| `wildcard_imports` | Style | Explicit imports |
| `missing_safety_doc` | Safety | Add `# Safety` doc |
| `undocumented_unsafe_blocks` | Safety | Add `// SAFETY:` |
| `transmute_ptr_to_ptr` | Safety | Use `pointer::cast()` |
| `large_stack_arrays` | Mem | Use Vec or Box |
| `too_many_arguments` | Design | Use struct params |

Unsafe code in ZPR touches the enforcement path: read `docs/SECURITY_MODEL.md`
before writing or reviewing it.

## Quick Reference

```
Naming: snake_case (fn/var), CamelCase (type), SCREAMING_CASE (const)
Format: rustfmt (just use it)
Docs: /// for public items, //! for module docs
Lint: #![warn(clippy::all)]
```

These are the non-obvious Rust-specific rules; general Rust conventions are
already well known and not restated here.
