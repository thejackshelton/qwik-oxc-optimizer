---
phase: 12-signal-wrapping-gaps
plan: 03
subsystem: jsx-transform
tags: [fnSignal, hoisted-functions, deduplication, signal-wrapping]

# Dependency graph
requires:
  - phase: 12-01
    provides: dep sorting, missing expression recursion, harmless globals, accept_call_expr
  - phase: 12-02
    provides: is_used_as_object gate, .value detection for complex expressions
provides:
  - _hf hoisted function deduplication via body-string HashMap lookup
  - Correct _hf counter alignment with SWC when identical expressions are wrapped
affects: [13-dce-ctx-kind-misc, 14-final-parity]

# Tech tracking
tech-stack:
  added: []
  patterns: ["body-string dedup for hoisted arrow functions via HashMap<String, u32>"]

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/transform.rs
    - crates/qwik-optimizer-oxc/src/jsx_transform.rs

key-decisions:
  - "Dedup key is the full arrow function source: '(p0) => p0.errors.test' (params + body)"
  - "Dedup check at caller site via hoisted_stmts.iter().any() to avoid duplicate const declarations"
  - "No new return value from build_fn_signal_wrapping -- caller-side dedup is simpler and works at all call sites"

patterns-established:
  - "body-string dedup: build parameterized body first, check HashMap, allocate _hf index only if new"

# Metrics
duration: 3min
completed: 2026-02-23
---

# Phase 12 Plan 03: _hf Hoisted Function Deduplication Summary

**HashMap-based dedup for _fnSignal hoisted functions so identical body strings (after parameter substitution) share the same _hfN constant**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-23T12:34:59Z
- **Completed:** 2026-02-23T12:38:02Z
- **Tasks:** 1
- **Files modified:** 2

## Accomplishments
- Identical parameterized body strings now share the same _hfN index (e.g., 5 `store.errors.test` props produce one `_hf0`)
- No duplicate `const _hfN = ...` declarations in output
- _hf counter numbers better aligned with SWC's dedup behavior
- Zero test regressions (99 snapshot diffs before = 99 after)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add _hf body-string deduplication** - `90d12c8` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added `hoisted_fn_dedup: HashMap<String, u32>` field to ImportTracker
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Restructured `build_fn_signal_wrapping` to check dedup map before allocating index; added dedup guards at both `hoisted_stmts.push()` call sites

## Decisions Made
- Dedup key includes full params + body (`"(p0) => p0.errors.test"`) to distinguish expressions with different parameter counts
- Caller-side dedup via `hoisted_stmts.iter().any()` chosen over return-flag approach for simplicity and uniform handling at both call sites (props path and children path)
- HashMap field on ImportTracker leverages `#[derive(Default)]` for automatic empty-map initialization

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Phase 12 (signal wrapping gaps) fully complete: dep sorting, missing expression recursion, harmless globals, accept_call_expr, is_used_as_object gate, .value detection, and _hf dedup all landed
- Ready for Phase 13 (DCE, ctxKind, entry field, misc) and Phase 14 (final parity)
- 99 snapshot diffs remain across all tests (aesthetic + remaining semantic gaps)

## Self-Check: PASSED

---
*Phase: 12-signal-wrapping-gaps*
*Completed: 2026-02-23*
