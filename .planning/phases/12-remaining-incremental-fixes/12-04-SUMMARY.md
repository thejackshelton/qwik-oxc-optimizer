---
phase: 12-signal-wrapping-gaps
plan: 04
subsystem: jsx-transform
tags: [signal-wrapping, fnSignal, destructured-props, object-keys, jsx]

# Dependency graph
requires:
  - phase: 12-signal-wrapping-gaps (plans 01-03)
    provides: dep sorting, expression recursion, harmless globals, accept_call_expr, is_used_as_object, .value detection, _hf dedup
provides:
  - _fnSignal wrapping for destructured prop aliases in binary/object expressions
  - Object property key preservation in hoisted function body strings
affects: [12-05, 12-06, 13-remaining-gaps]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "is_object_key_position heuristic for text-based identifier replacement"
    - "has_destructured_raw_props bypass for is_any_dep_used_as_object gate"

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/jsx_transform.rs

key-decisions:
  - "When _rawProps dep comes from destructured prop alias detection, bypass is_any_dep_used_as_object check entirely"
  - "Object key position detected by looking for { or , before identifier and : after it (heuristic)"
  - "Scope resolution :: excluded from object key detection to avoid false positives"

patterns-established:
  - "has_destructured_raw_props: bypass pattern for pre-rewrite expression analysis"
  - "is_object_key_position: conservative text heuristic for object key vs value distinction"

# Metrics
duration: 14min
completed: 2026-02-23
---

# Phase 12 Plan 04: Destructured Prop Aliases and Object Key Preservation Summary

**Fixed _fnSignal wrapping for expressions containing destructured prop aliases (fromLocal + fromProps) and fixed object property key replacement in hoisted function body strings ({props: p0.fromProps} not {p0: p0.fromProps})**

## Performance

- **Duration:** 14 min
- **Started:** 2026-02-23T17:43:35Z
- **Completed:** 2026-02-23T17:57:37Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- Binary expressions and object expressions containing destructured prop aliases now correctly wrapped with _fnSignal in both props and children paths
- Object property keys preserved during identifier replacement in _fnSignal body strings
- No regressions -- snapshot count remains at 99

## Task Commits

Each task was committed atomically:

1. **Task 1: Bypass is_any_dep_used_as_object for destructured prop deps** - `27f6480` (feat)
2. **Task 2: Fix object property key replacement in _fnSignal body string** - `f342c56` (fix)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Added has_destructured_raw_props bypass in props/children paths, added is_object_key_position heuristic to replace_identifier_in_code

## Decisions Made
- When `_rawProps` is a dep from destructured prop alias detection (destructured_props is non-empty), bypass the `is_any_dep_used_as_object` check entirely. This is safe because after body rewriting, the alias becomes `_rawProps.fromProps` which IS a member expression with `_rawProps` as object.
- Object key position detection uses a conservative heuristic: identifier preceded by `{` or `,` (ignoring whitespace) AND followed by `:` (but not `::` for scope resolution). False negatives maintain current behavior; false positives would be a bug, but the heuristic is deliberately conservative.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Stale stash contamination introduced accidental has_chain_depth change**
- **Found during:** Task 1 verification
- **Issue:** Git stash operations contaminated the commit with an unrelated change (`has_chain_depth(expr, 2)` -> `has_chain_depth(expr, 1)`) that caused `example_skip_transform` to regress (99 -> 100 snapshots)
- **Fix:** Detected via snapshot count increase, reverted the accidental change, re-committed cleanly
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** Snapshot count back to 99, example_skip_transform no longer has .snap.new

---

**Total deviations:** 1 auto-fixed (1 bug from tooling contamination)
**Impact on plan:** The contamination was from git stash operations interacting with external tooling. Fixed by careful commit reconstruction.

## Issues Encountered
- An external process (likely rust-analyzer) keeps modifying the file by inserting `const_bindings` parameter additions to `collect_reactive_deps_inner`. This required careful orchestration of git checkout/commit operations to avoid contamination.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Plans 05 and 06 in phase 12 can proceed (wave 1, no dependencies)
- The remaining 99 snapshot diffs include the 4 props_wrapping tests which now only have cosmetic differences (import ordering, single-line vs multi-line formatting, prop ordering)

## Self-Check: PASSED

---
*Phase: 12-signal-wrapping-gaps*
*Completed: 2026-02-23*
