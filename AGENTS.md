# Agent Instructions

## Project Overview

This is a **Rust rewrite of the Qwik optimizer** — porting from SWC to OXC. The optimizer transforms JS/TS/JSX/TSX source files by extracting lazy-loadable segments marked with `$()`, `component$`, etc., producing multiple output modules from a single input.

## TDD Workflow

**The 162 snapshot tests are the sole spec.** There are no separate requirement docs.

- Snapshots (golden SWC output) are in `crates/qwik-optimizer-oxc/tests/snapshots/*.snap`
- Inputs are in `crates/qwik-optimizer-oxc/tests/input/*.tsx`
- Test harness is in `crates/qwik-optimizer-oxc/tests/test.rs`
- Per-case config is in `test.rs:apply_case_overrides()`
- ~144/162 tests currently fail — this is expected

### Snapshot Test Workflow

```bash
# 1. Run tests with --accept to overwrite .snap files with current OXC output
cargo insta test -p qwik-optimizer-oxc --accept --test test -- snapshot_all_transforms

# 2. See exactly what differs between SWC (golden) and OXC (current)
git diff -- crates/qwik-optimizer-oxc/tests/snapshots/

# 3. After analyzing diffs, ALWAYS restore golden snapshots before committing
git checkout -- crates/qwik-optimizer-oxc/tests/snapshots/
```

> **⚠️ CRITICAL: NEVER COMMIT OR PUSH SNAPSHOT CHANGES**
> The `.snap` files contain the SWC golden reference output. Always `git checkout` them back before any commit.
> Only commit changes to source code in `crates/qwik-optimizer-oxc/src/`.

> Note: the golden SWC output snapshots were copy/pasted to the OXC `crates/qwik-optimizer-oxc/tests/snapshots/*.snap` snapshots after an oxfmt applied to all snapshots. Re-running the oxc `cargo insta test -p qwik-optimizer-oxc --accept --test test -- snapshot_all_transforms` did not produce new snapshot diffs, so the snapshots formatting is correct.

### How to Work on a Feature

1. Pick failing snapshot test(s) related to the feature
2. Run the snapshot workflow above to see the current diffs
3. Read the old SWC optimizer source in `crates/swc-optimizer/core/src/` to understand the logic
4. Implement the fix in `crates/qwik-optimizer-oxc/src/`
5. Re-run the snapshot workflow — when diffs shrink or disappear, the feature works
6. `git checkout -- crates/qwik-optimizer-oxc/tests/snapshots/` to restore golden snapshots
7. Commit only source code changes with conventional commits (`feat:`, `fix:`, `refactor:`)

### Reading Snapshots

Each snapshot has sections separated by `=== HEADER ===`:
- `=== INPUT ===` — source code
- `=== <path> ===` — transformed output module
- `/* {...} */` — segment metadata JSON after each segment module
- `=== DIAGNOSTICS ===` — diagnostics JSON array

Hashes appear as `XXXXXXXXXXXX` placeholders.

## Architecture

### Two-Phase Pipeline (REQUIRED)

The OXC optimizer MUST use a two-phase approach:

1. **Analyze** — Single read-only `Traverse` pass. Collect `$`-boundary sites, imports, exports, captures, JSX info. Produce a `TransformPlan`.
2. **Emit** — Execute the plan: mutate main AST in-place, build segment `Program` ASTs from scratch, run codegen on all programs.

**Why:** OXC's semantic analysis (`oxc_semantic`) becomes stale after AST mutation. All analysis must happen before any mutation.

### Key Constraints

- **Arena lifetime `'a`:** Any function creating AST nodes needs `&AstBuilder<'a>` or `&mut TraverseCtx<'a>`. Keep pure analysis functions lifetime-free.
- **Multi-output:** Build segment Programs AFTER traversal in fresh allocators. Cannot build during traversal (borrow conflict).
- **Statement insertion:** Use deferred insertion in `exit_program`/`exit_statements`, not inline.
- **Node replacement:** Use `std::mem::replace` for atomic swap, not `move_expression` (leaves arena garbage).
- **`/* @__PURE__ */`:** Explicitly add comments to `program.comments` for new `qrl()`/`componentQrl()` calls.

## Reference Material

| What | Where |
|------|-------|
| **Known issues & priority plan** | **`.planning/ISSUES.md`** |
| Old optimizer source (SWC) | `crates/swc-optimizer/core/src/` |
| OXC architecture research | `.planning/research/ARCHITECTURE.md` |
| OXC feature patterns | `.planning/research/FEATURES.md` |
| Critical pitfalls (14 items) | `.planning/research/PITFALLS.md` |
| OXC stack details | `.planning/research/STACK.md` |
| Research summary | `.planning/research/SUMMARY.md` |
| Project structure | `.planning/codebase/STRUCTURE.md` |
| Testing workflow | `.planning/codebase/TESTING.md` |
| Tech stack overview | `.planning/codebase/STACK.md` |

## Old Optimizer Module Map

The SWC optimizer's modules and what they do (read these to understand what to port):

| Module | Purpose |
|--------|---------|
| `lib.rs` | Entry point: `transform_modules()`, orchestrates pipeline |
| `parse.rs` | AST parsing, code transformation, segment extraction |
| `collector.rs` | Import/export/declaration collection |
| `code_move.rs` | Build separate module ASTs for extracted segments |
| `entry_strategy.rs` | Segment grouping logic (segment, inline, smart, hook, component, hoist) |
| `props_destructuring.rs` | Component props handling and optimization |
| `filter_exports.rs` | Export filtering for server/client code stripping |
| `const_replace.rs` | Constant inlining (`isServer`, `isBrowser`, `isDev`) |
| `is_const.rs` | Const evaluation utilities |
| `words.rs` | String constants (`BUILDER_IO_QWIK`, QRL function names) |

## Conventions

- **Rust edition:** 2024
- **OXC version:** 0.113 (umbrella crate with features: `semantic`, `codegen`, `serialize`)
- **Commits:** Conventional commits (`feat:`, `fix:`, `refactor:`, `test:`, `chore:`)
- **Testing:** Snapshot-only via `insta` crate. No separate unit tests.
- **Formatting:** Code in snapshots is formatted with `oxfmt` (auto-detected by test harness)
