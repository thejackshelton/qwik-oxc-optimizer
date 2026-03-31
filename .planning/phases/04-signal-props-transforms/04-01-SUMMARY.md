---
phase: 04-signal-props-transforms
plan: 01
subsystem: jsx-transform
tags: [_wrapProp, _fnSignal, signal-wrapping, jsx-children]

requires:
  - phase: 03-bugs-correctness
    provides: "All segments extracted, correct ordering"
provides:
  - "JSX children _wrapProp named wrapping (_rawProps + destructured props)"
  - "JSX children _fnSignal complex expression wrapping"
  - "Co-reactive local variable dep collection for _fnSignal"
affects: [04-02, 05-jsx-keys-flags]

tech-stack:
  added: []
  patterns: ["children signal wrapping mirrors attribute-level pattern", "two-tier reactive dep collection (primary + local co-reactive)"]

key-files:
  created: []
  modified: ["crates/qwik-optimizer-oxc/src/jsx_transform.rs"]

key-decisions:
  - "Local variables are co-reactive: they become _fnSignal deps only when primary reactive sources (signal.value, _rawProps, store chains) are present in the same expression"
  - "OXC codegen parentheses stripped from _fnSignal string representation to match SWC format"

patterns-established:
  - "Two-tier dep collection: primary_deps (signal.value, _rawProps, store) vs local_deps (unknown locals). Locals merge only when primaries exist."
  - "Children signal wrapping shares detect_signal_wrap + collect_reactive_deps with attribute-level wrapping"

duration: 6min
completed: 2026-02-20
---

# Phase 4 Plan 1: Signal Wrapping for JSX Children Summary

**Extended transform_jsx_children with _wrapProp named + _fnSignal wrapping, matching SWC for destructured props, signal.value, store chains, and co-reactive local variables**

## Performance

- **Duration:** 6 min
- **Started:** 2026-02-20T13:03:25Z
- **Completed:** 2026-02-20T13:09:08Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments

- JSX children expressions now correctly wrapped with `_wrapProp(_rawProps, "propName")` for destructured prop access
- Complex children expressions wrapped with `_fnSignal(hoisted_fn, [deps], str)` for binary, conditional, and object expressions involving reactive sources
- Fixed co-reactive dep collection: local variables (like `useSignal` results) participate as _fnSignal deps only alongside primary reactive sources, preventing over-wrapping of bare identifiers
- Fixed _fnSignal string representation to strip OXC codegen parentheses, matching SWC format

## Task Commits

Each task was committed atomically:

1. **Task 1: Extend transform_jsx_children for _wrapProp named + _fnSignal** - `0cad3d3` (feat)
2. **Task 2: Verify snapshot improvements and fix edge cases** - `c8287b6` (fix)

**Plan metadata:** TBD (docs: complete plan)

## Files Created/Modified

- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Extended transform_jsx_children with children signal wrapping; refactored collect_reactive_deps to use two-tier primary/local dep system

## Decisions Made

- **Co-reactive local deps**: SWC treats unknown local identifiers (e.g., `fromLocal` from `useSignal(0)`) as reactive deps only when they appear alongside primary reactive sources. Bare `fromLocal` stays unwrapped; `fromLocal + fromProps` produces `_fnSignal(_hf, [_rawProps, fromLocal], ...)`. Implemented via two-tier collection: `primary_deps` (signal.value, _rawProps, store chains) and `local_deps` that merge only when primaries exist.
- **String repr parens**: OXC's codegen wraps object expressions in parentheses `({...})` but SWC's _fnSignal string uses raw `{...}`. Fixed by stripping outer parens from the string representation.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Local variables incorrectly marked as non-reactive**
- **Found during:** Task 2 (snapshot verification)
- **Issue:** `collect_reactive_deps_inner` treated all unknown local identifiers as `has_non_reactive_non_const = true`, preventing _fnSignal wrapping for expressions like `fromLocal + fromProps`
- **Fix:** Introduced two-tier dep collection (primary_deps + local_deps). Local deps are promoted to actual deps only when primary reactive sources exist in the same expression.
- **Files modified:** `crates/qwik-optimizer-oxc/src/jsx_transform.rs`
- **Verification:** `example_props_wrapping_children` snapshot now matches SWC for all 6 children wrapping cases
- **Committed in:** `c8287b6`

**2. [Rule 1 - Bug] _fnSignal string representation had extra parentheses**
- **Found during:** Task 2 (snapshot verification)
- **Issue:** OXC codegen produces `({props:p0.fromProps})` for object expressions, but SWC uses `{props:p0.fromProps}` in the string constant
- **Fix:** Strip outer parentheses from codegen output before building the _str constant
- **Files modified:** `crates/qwik-optimizer-oxc/src/jsx_transform.rs`
- **Verification:** `_hf1_str` now matches SWC golden
- **Committed in:** `c8287b6`

---

**Total deviations:** 2 auto-fixed (2 bugs)
**Impact on plan:** Both bugs found during verification phase. Fixes improve SWC parity without scope creep.

## Issues Encountered

- Snapshot diff file count remains at 160 because each differing file also has unrelated diffs (import ordering, JSX keys, flags) from Phase 5/6 features. The children wrapping improvements are visible within the diffs but don't eliminate entire files.
- Text normalization strips trailing spaces from JSX text nodes (e.g., `"First "` becomes `"First"` in multi-child arrays). This is a separate issue not related to children signal wrapping -- noted for future fix.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Children signal wrapping complete. Ready for 04-02 (attribute-level _fnSignal wrapping for non-signal complex props).
- The co-reactive dep collection pattern established here will be reused by 04-02 for attribute props.
- Known gap: component$ with non-destructured `(props)` parameter -- `props.class` and `props["data-nu"]` not yet wrapped. This may need handling in 04-02 or 04-03.

## Self-Check: PASSED

---
*Phase: 04-signal-props-transforms*
*Completed: 2026-02-20*
