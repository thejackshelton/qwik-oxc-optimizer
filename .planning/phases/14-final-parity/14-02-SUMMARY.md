---
phase: 14-final-parity
plan: 02
subsystem: jsx-transform
tags: [spread-props, _jsxSplit, _createElement, _getConstProps, _getVarProps, oxc]

# Dependency graph
requires:
  - phase: 14-01
    provides: "Encounter-order synthetic import recording for segment modules"
  - phase: 09-05
    provides: "Multi-spread _jsxSplit and _getConstProps/_getVarProps framework"
provides:
  - "Correct _getConstProps positioning as separate 3rd arg for single-spread _jsxSplit"
  - "_createElement pattern for spread-only elements with user-provided keys"
  - "createElement import emission in both entry modules and segment modules"
affects: [15-signal-wrapping, 18-remaining]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Single-spread _jsxSplit: const_before stays in 2nd arg, const_after goes to 3rd when var_after exists"
    - "_createElement(tag, { ...source, key }) for spread-only elements with keys (non-props-param spread)"
    - "code_move.rs body_code.contains() pattern extended for _createElement segment imports"

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/jsx_transform.rs
    - crates/qwik-optimizer-oxc/src/transform.rs
    - crates/qwik-optimizer-oxc/src/code_move.rs

key-decisions:
  - "Single-spread const_props algorithm: 3 cases based on (has_explicit_const && has_var_after) || var_after_has_fn_signal"
  - "_createElement detection: single spread + user key + spread source NOT component's props param"
  - "const_after entries referencing spread source reclassified to var_after"
  - "createElement import uses aliased form: createElement as _createElement"

patterns-established:
  - "expr_contains_ident() helper for detecting identifier references in expression trees"
  - "spread_source_is_ident flag for tracking spread source type through classification"

# Metrics
duration: 75min
completed: 2026-02-24
---

# Phase 14 Plan 02: Spread Props Summary

**Fixed _getConstProps positioning for single-spread _jsxSplit and implemented _createElement for spread-only keyed elements, gaining 4 new exact matches (81->85)**

## Performance

- **Duration:** ~75 min (across 2 sessions due to context exhaustion)
- **Started:** 2026-02-24T16:22:00Z (estimated)
- **Completed:** 2026-02-24T17:26:44Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- Fixed single-spread _jsxSplit to place _getConstProps as separate 3rd argument instead of spreading inside var_props
- Implemented _createElement pattern for elements with single spread + user key (e.g., `<link {...l} key={l.key} />`)
- Added createElement import emission in entry modules (transform.rs) and segment modules (code_move.rs)
- 4 new exact matches: should_merge_attributes_with_spread_props, should_merge_attributes_with_spread_props_before_and_after, should_split_spread_props_with_additional_prop, should_move_bind_value_to_var_props
- Zero regressions (85/162 exact matches, 77 still failing)

## Task Commits

Each task was committed atomically:

1. **Task 1: Fix single-spread _jsxSplit const_props handling** - `2068647` (fix)
2. **Task 2: Implement _createElement for spread-only elements with keys** - `6eb7f85` (feat)

**Plan metadata:** `c9a1d73` (docs: complete plan)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Fixed single-spread const_props algorithm (3 cases), added expr_contains_ident helper, added _createElement early-return path
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added needs_create_element flag to ImportTracker, createElement import emission in exit_program
- `crates/qwik-optimizer-oxc/src/code_move.rs` - Added _createElement import detection for segment module builds

## Decisions Made

### Task 1: Single-spread _getConstProps Algorithm
Discovered through iterative testing against 12+ golden snapshots. The SWC algorithm for single-spread _jsxSplit is:
1. Separate const props before spread (`const_before`) from const props after spread (`const_after`)
2. `const_before` stays in the 2nd arg (var_props object)
3. `const_after` entries that reference the spread source get reclassified to var_after
4. Three cases for 3rd arg placement:
   - If `(has_explicit_const && has_var_after) || var_after_has_fn_signal_with_source_ref`: put _getConstProps spread in 2nd, explicit const in 3rd
   - If only `has_explicit_const` (no var_after): 3rd = `{ ..._getConstProps, ...explicit_const }`
   - Otherwise: bare `_getConstProps(source)` in 3rd

### Task 2: _createElement Detection
SWC uses _createElement when: single spread source + user-provided key + spread source is NOT the component's props parameter. This produces `_createElement("tag", { ...source, key: value })` instead of `_jsxSplit`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added _createElement import detection in code_move.rs**
- **Found during:** Task 2
- **Issue:** Plan only mentioned jsx_transform.rs and transform.rs, but segment module imports are built in code_move.rs via body_code.contains() pattern. Without this, _createElement calls appeared in segment code but the import was missing.
- **Fix:** Added body_code.contains("_createElement") check in build_segment_code_with_hoisted() with aliased import (createElement as _createElement)
- **Files modified:** crates/qwik-optimizer-oxc/src/code_move.rs
- **Verification:** example_spread_jsx segment now includes createElement import
- **Committed in:** 6eb7f85 (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Essential for correct segment module import emission. No scope creep.

## Issues Encountered

- **Iterative algorithm discovery (Task 1):** The correct single-spread _getConstProps algorithm required 7 iterations of testing against golden snapshots. Initial simple approaches caused 3-8 regressions each. The final 3-case algorithm with const_after reclassification was discovered through methodical bisection of failing tests.
- **OXC comment formatting:** The example_spread_jsx test still has a cosmetic diff (`*/\nexport const` vs `*/ export const`) due to OXC codegen placing JSDoc comment closing on a separate line from the following export. This is an OXC printer limitation, not a transform issue.
- **Context exhaustion:** First execution session ran out of context during Task 2 (after Task 1 was committed). Continuation session completed Task 2.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- 85/162 exact matches (52.5% parity)
- Spread props handling now correct for single-spread and multi-spread patterns
- Remaining spread-related diffs (example_spread_jsx comment formatting) are OXC codegen limitations
- Ready for Plan 04 (ctx_kind/diagnostics) to complete Phase 14
- Phase 15 (signal wrapping & JSX flags) can proceed after Phase 14

## Self-Check: PASSED

---
*Phase: 14-final-parity*
*Completed: 2026-02-24*
