---
phase: 14-final-parity
plan: 01
subsystem: transform
tags: [import-ordering, encounter-order, exit_program, jsx-transform, fragment]

# Dependency graph
requires:
  - phase: 06-import-ordering-cleanup
    provides: "Referenced-ident filtering and synthetic import emission in exit_program"
  - phase: 13-captures-dce
    provides: "Capture processing and segment body DCE"
provides:
  - "Encounter-order import tracking via synthetic_import_order on ImportTracker"
  - "Deferred inlinedQrl/qrl recording in exit_expression for correct import positioning"
  - "Fragment JSX transform defers _jsxSorted recording to after children processing"
  - "_Fragment emission after lazy imports in Phase 7 assembly"
  - "synthetic_import_count for lib.rs hoisted stmt injection positioning"
affects: [14-02, 14-03, 14-04, 15-signal-wrapping, 16-dce-captures]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Encounter-order tracking: synthetic_import_order Vec<String> records first-seen order"
    - "Deferred recording: boolean flags set early, record_synthetic_import called in exit hooks"
    - "Fragment children-first: defer _jsxSorted recording to after children for SWC fold order"

key-files:
  created: []
  modified:
    - crates/qwik-optimizer-oxc/src/transform.rs
    - crates/qwik-optimizer-oxc/src/jsx_transform.rs
    - crates/qwik-optimizer-oxc/src/lib.rs
    - crates/qwik-optimizer-oxc/src/code_move.rs

key-decisions:
  - "SWC uses BTreeMap<Id> where SyntaxContext increases in encounter order -- effective import order is encounter-based, not alphabetical"
  - "componentQrl recorded in enter_call_expression (SWC encounters callee name first in fold), inlinedQrl/qrl deferred to exit_expression (SWC wraps body after folding it)"
  - "Fragment transform defers _jsxSorted recording to after children processing -- child elements record _jsxSorted at the correct position"
  - "_getVarProps and _getConstProps recorded before _jsxSplit (SWC processes call arguments before the call itself)"
  - "synthetic_import_count passed to lib.rs for hoisted _hf* stmt injection after exactly N synthetic import lines"

patterns-established:
  - "Encounter-order tracking: add import names to synthetic_import_order in the order they are first needed during traversal"
  - "Deferred recording pattern: set boolean flag early to prevent duplicate recording, call record_synthetic_import later in exit hooks"

# Metrics
duration: ~45min
completed: 2026-02-24
---

# Phase 14 Plan 01: Import Ordering Summary

**Entry module import ordering restructured from alphabetical to SWC encounter order with Fragment positioned after lazy imports -- 5 new exact matches (77 total)**

## Performance

- **Duration:** ~45 min
- **Started:** 2026-02-24T15:10:00Z
- **Completed:** 2026-02-24T15:55:26Z
- **Tasks:** 1
- **Files modified:** 4

## Accomplishments
- Replaced alphabetical import sorting with encounter-order preservation in exit_program
- Fixed _Fragment import positioning (now after lazy imports and hoisted stmts)
- Fixed Fragment JSX transform to defer _jsxSorted recording to after children
- Fixed _getVarProps/_getConstProps ordering before _jsxSplit
- 5 new exact matches, zero regressions (77 total, was 72)

## Task Commits

Each task was committed atomically:

1. **Task 1: Restructure entry module import ordering** - `868e5d3` (feat)

## Files Created/Modified
- `crates/qwik-optimizer-oxc/src/transform.rs` - Added synthetic_import_order tracking, restructured exit_program Phase 3-7, deferred inlinedQrl/qrl recording to exit_expression
- `crates/qwik-optimizer-oxc/src/jsx_transform.rs` - Added record_synthetic_import calls at all needs_* sites, fixed Fragment children-first ordering, reordered _getVarProps/_getConstProps before _jsxSplit
- `crates/qwik-optimizer-oxc/src/lib.rs` - Changed hoisted stmts injection to use synthetic_import_count instead of "after last import"
- `crates/qwik-optimizer-oxc/src/code_move.rs` - Swapped hoisted stmts and lazy imports order in entry segments

## Decisions Made
- SWC's BTreeMap<Id> uses SyntaxContext (encounter order), not alphabetical -- removed all sort_by calls
- componentQrl recorded in enter (SWC fold enters callee first), inlinedQrl/qrl in exit (SWC wraps after folding body)
- Fragment defers _jsxSorted to after children -- let child elements record it at the right position
- _getVarProps/_getConstProps before _jsxSplit (SWC processes call arguments depth-first)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fragment JSX transform recorded _jsxSorted before children processing**
- **Found during:** Task 1 (after initial implementation)
- **Issue:** Fragment's `transform_jsx_fragment_inner` recorded `_jsxSorted` immediately at function entry, before `transform_jsx_children`. This caused `_jsxSorted` to appear before `_wrapProp` in encounter order (SWC records children's imports first)
- **Fix:** Deferred `_jsxSorted` and `_Fragment` recording to after `transform_jsx_children` call. Let child elements record `_jsxSorted` naturally at the correct position.
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** example_derived_signals_children became exact match
- **Committed in:** 868e5d3

**2. [Rule 1 - Bug] _getVarProps/_getConstProps recorded after _jsxSplit**
- **Found during:** Task 1 (after Fragment fix)
- **Issue:** Spread element path recorded `_jsxSplit` before `_getVarProps`/`_getConstProps`, but SWC processes call arguments before the call itself
- **Fix:** Reordered the recording: `_getVarProps` and `_getConstProps` first, then `_jsxSplit`
- **Files modified:** crates/qwik-optimizer-oxc/src/jsx_transform.rs
- **Verification:** example_jsx became exact match
- **Committed in:** 868e5d3

**3. [Rule 1 - Bug] inlinedQrl recorded before body imports in encounter order**
- **Found during:** Task 1 (initial implementation)
- **Issue:** `inlinedQrl`/`qrl` was recorded in `enter_call_expression` before body traversal, causing it to appear before `_jsxSorted` in inline strategy
- **Fix:** Deferred `record_synthetic_import` for `inlinedQrl`/`qrl` to `exit_expression` while keeping boolean flags in enter
- **Files modified:** crates/qwik-optimizer-oxc/src/transform.rs
- **Verification:** example_optimization_issue_3795 and example_immutable_function_components remained exact matches
- **Committed in:** 868e5d3

---

**Total deviations:** 3 auto-fixed (3 bugs)
**Impact on plan:** All fixes necessary for correct encounter-order import positioning. No scope creep.

## Issues Encountered
- Initial approach of setting Fragment's needs_jsx_sorted flag early prevented child elements from recording _jsxSorted at the correct position. Solved by not setting the flag early and letting children handle it naturally (idempotent recording).

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Import ordering substantially improved (5 fewer diffs)
- Ready for Plan 02 (spread props) and subsequent cosmetic fixes
- Remaining 85 snapshot diffs include: IMPORT_ORDER (~39 remaining, some still have encounter-order-adjacent issues from other categories), SPREAD_PROPS (11), and other categories

## Self-Check: PASSED

---
*Phase: 14-final-parity*
*Completed: 2026-02-24*
