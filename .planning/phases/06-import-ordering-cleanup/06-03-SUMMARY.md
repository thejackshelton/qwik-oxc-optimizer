---
phase: 06-import-ordering-cleanup
plan: 03
subsystem: transform
tags: [qrl-hoisting, lazy-imports, loop-context, ast-transform]
requires: ["06-01", "06-02"]
provides: ["qrl-hoisting-in-loops", "entry-module-lazy-import-ordering", "entry-module-lazy-import-filtering"]
affects: []
tech-stack:
  added: []
  patterns: ["pending-buffer-flush-at-function-exit", "referenced-ident-filtering-for-lazy-imports"]
key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/transform.rs
key-decisions:
  - "Flush QRL hoists at function/arrow exit when loop_depth == 0 (avoids flushing inside .map() callback arrows)"
  - "Insert hoisted const declarations after variable declarations at function body top (matches SWC positioning)"
  - "Forward iteration order for hoists matches SWC BTreeMap alphabetical ordering"
  - "Lazy imports filtered by referenced-ident analysis in exit_program (Rule 2 deviation: missing critical filtering)"
duration: 11min
completed: 2026-02-20
---

# Phase 6 Plan 3: QRL Hoisting and Lazy Import Ordering Summary

QRL calls inside loops hoisted to const declarations at enclosing function body; entry module lazy imports sorted alphabetically and filtered by reference analysis.

## Performance

- Duration: 11 minutes
- Compilation: clean, no warnings
- Test results: 1 test (162 snapshots), all pass
- Snapshot improvement: 148 diffs -> 138 diffs (10 snapshots fixed, 24 total now match exactly)

## Accomplishments

### QRL Hoisting (Task 1)
- Added `pending_loop_qrl_hoists: Vec<(String, String, Vec<String>)>` buffer to QwikTransform
- Modified `replace_jsx_element_handlers` to detect `loop_depth > 0` and buffer QRL call components instead of inlining
- Inline `qrl()` calls replaced with identifier references to the hoisted const variable name
- Flush logic in `exit_function` and `exit_arrow_function_expression` when `loop_depth == 0`
- `flush_qrl_hoists_to_body` helper inserts const declarations after variable declarations at function body top
- Correctly handles: `.map()` callbacks, `for`/`for-in`/`for-of` loops, `while` loops, nested loops

### Lazy Import Ordering and Filtering (Task 2)
- Sort `import_tracker.lazy_imports` by import path before emission (matches SWC's BTreeMap ordering)
- Filter lazy imports by referenced-ident analysis: only emit imports whose `i_HASH` identifier is actually referenced in the entry module body
- With QRL hoisting, event handler lazy imports are only referenced in segment bodies (not entry module), so they are correctly excluded from the entry module

## Task Commits

| Task | Name | Commit | Key Changes |
|------|------|--------|-------------|
| 1 | QRL hoisting inside function bodies | a276eff | pending_loop_qrl_hoists buffer, flush at function exit, flush_qrl_hoists_to_body helper |
| 2 | Entry module lazy import ordering | c543191 | Sort by import path, filter by referenced-ident analysis |

## Files Modified

- `crates/qwik-optimizer-oxc/src/transform.rs` -- QwikTransform struct (new field), replace_jsx_element_handlers (hoist logic), exit_function/exit_arrow_function_expression (flush logic), exit_program (sort + filter lazy imports), flush_qrl_hoists_to_body (new helper function)

## Decisions Made

1. **Flush at loop_depth == 0**: QRL hoists are flushed when exiting a function/arrow where `loop_depth == 0`. This ensures .map() callback arrows (which exit while loop_depth > 0) don't get the hoists -- they bubble up to the enclosing non-loop function. This elegantly handles nested loops: inner .map() hoists pass through multiple arrow exits until reaching the component body.

2. **Insert position**: Hoisted const declarations are inserted after existing variable declarations at the function body top but before function declarations, loops, and return statements. This matches SWC's positioning.

3. **Forward iteration for hoist order**: The push order (inner elements first due to bottom-up traversal) naturally produces alphabetical order matching SWC's BTreeMap ordering.

4. **Lazy import filtering (Rule 2 deviation)**: Added referenced-ident filtering for lazy imports in exit_program. This was not in the original plan but is critical for correct entry module output. With QRL hoisting, event handler lazy imports move from entry module to segment bodies, so they must be filtered out.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Lazy import filtering in entry module**
- **Found during:** Task 2
- **Issue:** After QRL hoisting moves qrl() calls from entry module into segment bodies, the lazy import identifiers (i_HASH) for event handlers are no longer referenced in the entry module. Without filtering, they would remain as dead declarations in the entry module.
- **Fix:** Extended the referenced-ident filtering (from 06-01) to also cover lazy import declarations, checking if `i_HASH` is in the `referenced_idents` set before emitting.
- **Files modified:** transform.rs (exit_program lazy imports section)
- **Commit:** c543191

## Issues Encountered

- None. Both tasks implemented cleanly with no blocking issues.

## Next Phase Readiness

This is the final plan of Phase 6. Remaining 138 snapshot diffs are from:
- Capture list differences (iteration variables in captures, missing/extra captures)
- _fnSignal hoisting to module level vs segment level
- q:p / var_props ordering differences
- JSX flag differences (21 scope-analysis issues from Phase 5)
- Text normalization (trailing spaces in JSX text nodes)
- A few segment body code differences (_hf naming, _fnSignal usage)
- 1 deferred naming issue (should_extract_single_qrl_2 dedup suffix)

These are all separate issues beyond the scope of the current 6-phase plan.

## Self-Check: PASSED
